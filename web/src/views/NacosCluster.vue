<script setup>
// Nacos 集群管理:命名空间 / 配置 / 账号与权限。
// 与「Nacos 管理」总览页不同,这一页的每个动作都直接打到远端 Nacos 集群,
// 而不是 opsctl 自己的库。因此:
//   1) 破坏性动作(删命名空间 / 删账号 / 解绑 / 收回)一律二次确认;
//   2) 失败原样透传服务端文案(Nacos 的报错常常是唯一线索,别自己编);
//   3) 任何写操作成功后立刻重拉当前这张表 —— 集群侧有传播延迟,
//      不重拉就会让用户对着旧数据继续操作。
import { computed, h, onMounted, reactive, ref } from 'vue';
import { useRoute, useRouter } from 'vue-router';
import { NButton, NPopconfirm, useMessage } from 'naive-ui';
import { api } from '../api';
import Icon from '../components/Icon.vue';

const route = useRoute();
const router = useRouter();
const message = useMessage();

const clusterId = route.params.id;
const cluster = ref(null);
const tab = ref('ns');
const loading = reactive({
  cluster: false,
  namespaces: false,
  configs: false,
  users: false,
  access: false,
});

const fail = (e, fallback) => message.error(e?.response?.data?.error || fallback);

// ---- 集群信息(总览接口里挑出当前这一个) ----

async function loadCluster() {
  loading.cluster = true;
  try {
    const list = await api.nacosClusters();
    cluster.value = (list || []).find((c) => String(c.id) === String(clusterId)) || null;
    if (!cluster.value) message.error('集群不存在或已被删除');
  } catch (e) {
    fail(e, '加载集群信息失败(需 admin)');
  } finally {
    loading.cluster = false;
  }
}

// ============ 1. 命名空间 ============

const namespaces = ref([]);
const nsFlavor = ref('');
const nsNotice = ref('');

/// public 命名空间在 Nacos 里的 id 是空串,不能改也不能删。
const isPublicNs = (r) => !r.namespace_id;

const NS_TYPE = {
  0: { text: '全局', tone: 'muted' },
  1: { text: '默认私有', tone: 'ok' },
  2: { text: '自定义', tone: 'warn' },
};

async function loadNamespaces() {
  loading.namespaces = true;
  try {
    const r = await api.nacosNamespaces(clusterId);
    namespaces.value = r.items || [];
    nsFlavor.value = r.flavor || '';
    nsNotice.value = r.message || '';
    /// 列表变了就校正选中项 —— 右侧的配置永远跟着左侧走,不能指向一个已经没了的空间。
    syncSelection();
  } catch (e) {
    fail(e, '加载命名空间失败');
  } finally {
    loading.namespaces = false;
  }
}

const NS_ID_RE = /^[\w-]+$/;
const NS_NAME_RE = /^[^@#$%^&*]+$/;

const nsForm = reactive({
  show: false,
  saving: false,
  editing: false,
  errors: {},
  model: { namespace_id: '', name: '', desc: '' },
});

function openNsNew() {
  nsForm.editing = false;
  nsForm.errors = {};
  nsForm.model = { namespace_id: '', name: '', desc: '' };
  nsForm.show = true;
}

function openNsEdit(r) {
  nsForm.editing = true;
  nsForm.errors = {};
  nsForm.model = { namespace_id: r.namespace_id, name: r.name || '', desc: r.desc || '' };
  nsForm.show = true;
}

function validateNs() {
  const e = {};
  const id = nsForm.model.namespace_id.trim();
  // 新建时 id 可留空(交给 Nacos 生成 UUID);编辑时 id 是主键,必填且不可改。
  if (id) {
    if (!NS_ID_RE.test(id)) e.namespace_id = '只能包含字母、数字、下划线和连字符';
    else if (id.length > 128) e.namespace_id = '不能超过 128 个字符';
  } else if (nsForm.editing) {
    e.namespace_id = '命名空间 ID 缺失';
  }
  const name = nsForm.model.name.trim();
  if (!name) e.name = '请填写命名空间名称';
  else if (!NS_NAME_RE.test(name)) e.name = '不能包含 @#$%^&* 这些字符';
  nsForm.errors = e;
  return !Object.keys(e).length;
}

async function saveNs() {
  if (!validateNs()) return;
  nsForm.saving = true;
  try {
    const body = {
      namespace_id: nsForm.model.namespace_id.trim(),
      name: nsForm.model.name.trim(),
      desc: nsForm.model.desc,
    };
    if (nsForm.editing) await api.updateNacosNamespace(clusterId, body);
    else await api.createNacosNamespace(clusterId, body);
    message.success(nsForm.editing ? '命名空间已更新' : '命名空间已创建');
    nsForm.show = false;
    await loadNamespaces();
  } catch (e) {
    fail(e, nsForm.editing ? '更新失败' : '创建失败');
  } finally {
    nsForm.saving = false;
  }
}

async function removeNs(r) {
  try {
    await api.deleteNacosNamespace(clusterId, r.namespace_id);
    message.success(`已删除命名空间「${r.name || r.namespace_id}」`);
    await loadNamespaces();
  } catch (e) {
    fail(e, '删除失败');
  }
}

// ============ 2. 命名空间与配置(上下级)============
// Nacos 是按命名空间硬隔离的:配置永远从属于某个命名空间。所以这里做成主从版面 ——
// 左边选空间、右边列它里面的配置,「当前在哪个空间」由位置本身表达,不用再问一遍。
// 查看正文 / 删除 / 同步为模板全部带上选中的那个 namespace:一旦回落到集群登记的默认值,
// 就会出现「列表看的是 A、删除打到 B」这种最危险的错位。
// 同步(把整个命名空间拉回 opsctl 存成模板,再到总览页回放到别的集群)是右边的主操作,
// 删除只是次要动作。

const configs = ref([]);
const configTotal = ref(0);
/// 一次拉满 200 条:主从版面里再翻页很割裂,超过 200 才需要分页器兜底。
const configPage = reactive({ pageNo: 1, pageSize: 200 });
const configNotice = ref('');

/// 当前选中的命名空间 id;'' 就是 Nacos 的 public,null 表示还没选出来。
const selectedNs = ref(null);

const currentNs = computed(() =>
  selectedNs.value === null
    ? null
    : namespaces.value.find((n) => (n.namespace_id || '') === selectedNs.value) || null
);
/// 标题里给人看的名字:public 在有些 Nacos 版本 name 是空的,兜底成 public。
const currentNsLabel = computed(
  () => currentNs.value?.name || currentNs.value?.namespace_id || 'public'
);
const currentNsType = computed(() => NS_TYPE[Number(currentNs.value?.type)] || null);

/// 后端语义已明确为「字段缺失 = 集群默认;显式给值(含空串)= 就用这个」,
/// 所以选中什么就原样送什么 —— public 的 id 本来就是空串,不需要任何别名兜底。
function nsParam(id) {
  return id || '';
}

/// 命名空间列表刷新后校正选中项:用户已经选中的那个还在就别动他(刷新不该把人弹回默认项),
/// 否则落到集群自己登记的那个,再不行就第一个。
function syncSelection() {
  const list = namespaces.value;
  if (!list.length) {
    selectedNs.value = null;
    configs.value = [];
    configTotal.value = 0;
    return;
  }
  if (list.some((n) => (n.namespace_id || '') === selectedNs.value)) return;
  const want = cluster.value?.namespace || '';
  const hit = list.find((n) => (n.namespace_id || '') === want) || list[0];
  selectedNs.value = hit.namespace_id || '';
  configPage.pageNo = 1;
  loadConfigs();
}

function selectNs(r) {
  const id = r.namespace_id || '';
  if (id === selectedNs.value) return;
  selectedNs.value = id;
  /// 换空间等于换了一张表:旧数据立刻清掉,免得新数据回来之前对着上一个空间的配置动手。
  configPage.pageNo = 1;
  configs.value = [];
  configTotal.value = 0;
  configNotice.value = '';
  loadConfigs();
}

/// 左侧徽章:选中的那个用刚拉到的 total(删一条能立刻对上),其余用列表接口给的计数。
function nsCount(r) {
  if ((r.namespace_id || '') === selectedNs.value && !configNotice.value) return configTotal.value;
  const c = r.config_count;
  return c === null || c === undefined ? '—' : c;
}

/// 刷新按钮:命名空间列表和右侧配置是一体的,一个按钮把两边都拉新。
function refreshNs() {
  loadNamespaces();
  if (selectedNs.value !== null) loadConfigs();
}

async function loadConfigs() {
  if (selectedNs.value === null) return;
  loading.configs = true;
  try {
    const r = await api.nacosConfigs(clusterId, {
      page_no: configPage.pageNo,
      page_size: configPage.pageSize,
      namespace: nsParam(selectedNs.value),
    });
    configs.value = r.items || [];
    configTotal.value = r.total || 0;
    /// 读失败时后端给的是 ok:false + message(不抛 400),原样展示,别吞掉。
    configNotice.value = r.ok === false ? r.message || '读取配置失败' : '';
  } catch (e) {
    fail(e, '加载配置失败');
  } finally {
    loading.configs = false;
  }
}

/// 配置格式只是个标签,没有风险分级,统一 muted,免得和状态色抢注意力。
function typePill(type) {
  return h('span', { class: 'pill pill-muted' }, [
    h(Icon, { name: 'file', size: 11 }),
    h('span', type || 'text'),
  ]);
}

const configColumns = [
  {
    title: 'dataId',
    key: 'data_id',
    render: (r) =>
      h(
        NButton,
        { text: true, class: 'mono', onClick: () => openConfigView(r) },
        { default: () => r.data_id }
      ),
  },
  { title: 'group', key: 'group', width: 180, render: (r) => h('span', { class: 'mono' }, r.group) },
  { title: '格式', key: 'type', width: 112, render: (r) => typePill(r.type) },
  {
    title: '操作',
    key: 'ops',
    width: 150,
    render: (r) =>
      h('div', { class: 'row-ops' }, [
        h(NButton, { size: 'tiny', onClick: () => openConfigView(r) }, { default: () => '查看' }),
        h(
          NPopconfirm,
          { onPositiveClick: () => removeConfig(r), positiveText: '删除', negativeText: '取消' },
          {
            trigger: () =>
              h(NButton, { size: 'tiny', type: 'error', tertiary: true }, { default: () => '删除' }),
            default: () =>
              `删除的是远端 Nacos 命名空间「${currentNsLabel.value}」里的配置「${r.data_id}」` +
              `(group ${r.group}),不可恢复,请先确认没有服务还在读它。`,
          }
        ),
      ]),
  },
];

// ---- 查看正文 ----

const cfgView = reactive({
  show: false,
  loading: false,
  data_id: '',
  group: '',
  namespace: '',
  bytes: 0,
  content: '',
  error: '',
});

async function openConfigView(r) {
  cfgView.show = true;
  cfgView.loading = true;
  cfgView.data_id = r.data_id;
  cfgView.group = r.group;
  cfgView.namespace = selectedNs.value || '';
  cfgView.bytes = 0;
  cfgView.content = '';
  cfgView.error = '';
  try {
    /// 必须带上选中的 namespace:详情要和列表落在同一个 tenant,否则读到的是另一个空间的同名配置。
    const d = await api.nacosConfigDetail(clusterId, {
      data_id: r.data_id,
      group: r.group,
      namespace: nsParam(selectedNs.value),
    });
    if (d.ok) {
      cfgView.namespace = d.namespace || '';
      cfgView.bytes = d.bytes || 0;
      cfgView.content = d.content || '';
    } else {
      cfgView.error = d.message || '读取配置正文失败';
    }
  } catch (e) {
    cfgView.error = e?.response?.data?.error || '读取配置正文失败';
  } finally {
    cfgView.loading = false;
  }
}

/// 非 HTTPS 或用户拒权时 clipboard 直接抛错,降级成「请手动复制」而不是静默失败。
async function copyConfig() {
  try {
    await navigator.clipboard.writeText(cfgView.content || '');
    message.success('配置正文已复制');
  } catch {
    message.warning('浏览器不允许写剪贴板,请手动选中正文复制');
  }
}

async function removeConfig(r) {
  try {
    /// 删除同样要带 namespace:回落到集群默认就会删掉另一个空间里的同名配置。
    await api.deleteNacosConfig(clusterId, {
      data_id: r.data_id,
      group: r.group,
      namespace: nsParam(selectedNs.value),
    });
    message.success(`已删除配置「${r.data_id}」`);
    /// 正在看的就是被删的那条,抽屉里的内容已经失效,直接收起来。
    if (cfgView.show && cfgView.data_id === r.data_id && cfgView.group === r.group) {
      cfgView.show = false;
    }
    await loadConfigs();
  } catch (e) {
    fail(e, '删除配置失败');
  }
}

// ---- 同步为模板 ----

const sync = reactive({ show: false, running: false, templateName: '', result: null });

/// 简化:同步范围就是左侧选中的那个命名空间(位置已经表达了目标),不再让人填一遍;
/// 模板名也先替他起好 —— 不改也能直接点。
function openSync() {
  sync.templateName = `${cluster.value?.name || 'nacos'} · ${currentNsLabel.value}`;
  sync.result = null;
  sync.show = true;
}

const syncColumns = [
  { title: 'dataId', key: 'data_id', render: (r) => h('span', { class: 'mono' }, r.data_id) },
  { title: 'group', key: 'group', width: 160, render: (r) => h('span', { class: 'mono' }, r.group) },
  { title: '格式', key: 'type', width: 112, render: (r) => typePill(r.type) },
  {
    title: '字节数',
    key: 'bytes',
    width: 150,
    render: (r) =>
      h('div', { class: 'row-ops' }, [
        h('span', { class: 'num' }, r.bytes ?? 0),
        /// 空内容多半是远端本来就没写全,回放时会覆盖出一个空配置,值得标出来。
        r.empty
          ? h('span', { class: 'pill pill-warn' }, [
              h(Icon, { name: 'alert', size: 11 }),
              h('span', '空内容'),
            ])
          : null,
      ]),
  },
];

async function runSync(dryRun) {
  sync.running = true;
  try {
    const r = await api.syncNacosConfigs(clusterId, {
      /// 后端 template_name 是字符串(不接受 null),留空即由后端自动生成名字。
      template_name: sync.templateName.trim(),
      namespace: nsParam(selectedNs.value),
      dry_run: dryRun,
    });
    sync.result = r;
    if (dryRun) {
      message.info(`试运行完成:读到 ${r.total} 条配置,未落库`);
    } else {
      message.success(
        `已同步 ${r.total} 条配置为模板「${r.template_name}」,` +
          '可在 Nacos 总览页 → 初始化配置 中回放到其它集群'
      );
    }
  } catch (e) {
    fail(e, dryRun ? '试运行失败' : '同步失败');
  } finally {
    sync.running = false;
  }
}

// ============ 3. 账号与权限 ============
// Nacos 的真实模型是「账号 → 角色 → 权限」,原来三张平铺的表逼着人自己在脑子里做 join,
// 「这个账号到底能动哪些命名空间」根本看不出来。这里改成以账号为中心:
// 左边选人,右边直接列出它能操作的命名空间;授权只问「命名空间 + 动作」两件事,
// 中间那层角色由后端自动创建并绑定 —— 用户不需要理解 Nacos 的内部模型才能授权。

const users = ref([]);
const userTotal = ref(0);
const userPage = reactive({ pageNo: 1, pageSize: 20 });
/// 左侧选中的账号名;右侧的一切都跟着它走。
const selected = ref('');

const access = reactive({ roles: [], global_admin: false, grants: [], error: '' });

async function loadUsers() {
  loading.users = true;
  try {
    const r = await api.nacosUsers(clusterId, { page_no: userPage.pageNo, page_size: userPage.pageSize });
    users.value = r.items || [];
    userTotal.value = r.total || 0;
    /// 右侧不能空着:选中的账号被删掉或翻页翻走了,就落回当前页第一个。
    if (!users.value.some((u) => u.username === selected.value)) {
      selectUser(users.value[0]?.username || '');
    }
  } catch (e) {
    fail(e, '加载账号失败');
  } finally {
    loading.users = false;
  }
}

function selectUser(name) {
  selected.value = name || '';
  access.roles = [];
  access.global_admin = false;
  access.grants = [];
  access.error = '';
  if (selected.value) loadAccess();
}

async function loadAccess() {
  const name = selected.value;
  if (!name) return;
  loading.access = true;
  try {
    const r = await api.nacosUserAccess(clusterId, name);
    /// 等待期间用户可能又点了别的账号,这份回包已经过期,直接丢掉免得覆盖新选中项。
    if (selected.value !== name) return;
    access.roles = r.roles || [];
    access.global_admin = !!r.global_admin;
    access.grants = r.grants || [];
    access.error = '';
  } catch (e) {
    if (selected.value !== name) return;
    access.roles = [];
    access.grants = [];
    access.error = e?.response?.data?.error || '读取该账号的权限失败';
  } finally {
    /// 过期请求不许熄灯:此刻在飞的是后一次请求,loading 归它管。
    if (selected.value === name) loading.access = false;
  }
}

/// 刷新按钮:账号列表和权限全景是一体的,一个按钮把两边都拉新。
function refreshAccess() {
  loadUsers();
  if (selected.value) loadAccess();
}

// ---- 账号增删改 ----

const userForm = reactive({ show: false, saving: false, errors: {}, model: { username: '', password: '' } });

function openUserNew() {
  userForm.errors = {};
  userForm.model = { username: '', password: '' };
  userForm.show = true;
}

async function saveUser() {
  const e = {};
  if (!userForm.model.username.trim()) e.username = '请填写用户名';
  if (!userForm.model.password) e.password = '请填写密码';
  userForm.errors = e;
  if (Object.keys(e).length) return;
  userForm.saving = true;
  try {
    const name = userForm.model.username.trim();
    await api.createNacosUser(clusterId, { username: name, password: userForm.model.password });
    message.success('账号已创建');
    userForm.show = false;
    await loadUsers();
    /// 新账号必然还没有任何权限,直接选中它,下一步授权就在眼前。
    selectUser(name);
  } catch (err) {
    fail(err, '创建账号失败');
  } finally {
    userForm.saving = false;
  }
}

const resetForm = reactive({ show: false, saving: false, error: '', username: '', new_password: '' });

function openReset(r) {
  resetForm.username = r.username;
  resetForm.new_password = '';
  resetForm.error = '';
  resetForm.show = true;
}

async function saveReset() {
  if (!resetForm.new_password) {
    resetForm.error = '请填写新密码';
    return;
  }
  resetForm.saving = true;
  try {
    await api.resetNacosUser(clusterId, {
      username: resetForm.username,
      new_password: resetForm.new_password,
    });
    message.success(`已重置「${resetForm.username}」的密码`);
    resetForm.show = false;
  } catch (e) {
    fail(e, '重置密码失败');
  } finally {
    resetForm.saving = false;
  }
}

async function removeUser(r) {
  try {
    await api.deleteNacosUser(clusterId, r.username);
    message.success(`已删除账号「${r.username}」`);
    await loadUsers();
  } catch (e) {
    fail(e, '删除账号失败');
  }
}

// ---- 权限全景 ----

const ACTIONS = {
  r: { text: '只读 r', tone: 'muted' },
  w: { text: '只写 w', tone: 'warn' },
  rw: { text: '读写 rw', tone: 'danger' },
};

function actionPill(action) {
  const a = ACTIONS[String(action).toLowerCase()] || { text: action || '—', tone: 'muted' };
  return h('span', { class: `pill pill-${a.tone}` }, [
    h(Icon, { name: 'dot', size: 11 }),
    h('span', a.text),
  ]);
}

/// grants 里只有命名空间 id,能在命名空间列表里对上就补一个人话名字。
const nsNameById = computed(() => {
  const m = new Map();
  for (const n of namespaces.value) m.set(n.namespace_id || '', n.name || '');
  return m;
});

function nsLabel(id) {
  return id ? `${nsNameById.value.get(id) || id}(${id})` : 'public(默认命名空间)';
}

const accessColumns = [
  {
    title: '命名空间',
    key: 'namespace_id',
    width: 200,
    render: (r) => {
      const id = r.namespace_id || '';
      const name = nsNameById.value.get(id);
      return h('div', { class: 'ns-cell' }, [
        h('span', { class: 'mono' }, id || 'public'),
        name ? h('span', { class: 'row-note' }, name) : null,
      ]);
    },
  },
  {
    /// 命名空间只是资源串的第一段。授权可能只落在某个 group / 某种类型 / 某条配置上,
    /// 不把原串摆出来,用户就分不清「整个空间」和「空间里的一条配置」。
    title: '资源',
    key: 'resource',
    render: (r) => h('span', { class: 'mono' }, r.resource || '—'),
  },
  { title: '动作', key: 'action', width: 120, render: (r) => actionPill(r.action) },
  {
    /// 角色是 Nacos 的实现细节,留一列交代权限从哪来,但压成小字不抢视线。
    title: '来源角色',
    key: 'role',
    width: 200,
    render: (r) => h('span', { class: 'mono row-note' }, r.role),
  },
  {
    title: '操作',
    key: 'ops',
    width: 110,
    render: (r) =>
      h(
        NPopconfirm,
        { onPositiveClick: () => revokeGrant(r), positiveText: '收回', negativeText: '取消' },
        {
          trigger: () =>
            h(NButton, { size: 'tiny', type: 'error', tertiary: true }, { default: () => '收回' }),
          default: () =>
            `收回「${selected.value}」对资源 ${r.resource} 的 ${r.action} 权限?` +
            `该权限挂在角色「${r.role}」上,同角色的其他账号会一起失去它。`,
        }
      ),
  },
];

async function revokeGrant(r) {
  try {
    /// resource 原样回传:Nacos 按资源串逐字匹配,少一段多一段都删不掉这一行。
    await api.revokeNacosUser(clusterId, {
      username: selected.value,
      action: r.action,
      resource: r.resource,
    });
    message.success('权限已收回');
    await loadAccess();
  } catch (e) {
    fail(e, '收回失败');
  }
}

// ---- 授权:命名空间(可多选)+ 动作,资源范围收在「高级」里 ----

const ACTION_OPTS = [
  { label: 'r 只读', value: 'r' },
  { label: 'w 只写', value: 'w' },
  { label: 'rw 读写', value: 'rw' },
];

/// Nacos 只认这三种类型;「全部」用 * 表示,且会让资源串第三段整体塌缩(见 buildResource)。
const KIND_OPTS = [
  { label: '全部(配置 + 服务)', value: '*' },
  { label: '配置 config', value: 'config' },
  { label: '服务 naming', value: 'naming' },
];

/// public 的 id 是空串,原样传给 Nacos(资源串第一段留空)。
const nsOpts = computed(() =>
  namespaces.value.map((n) => ({ label: nsLabel(n.namespace_id || ''), value: n.namespace_id || '' }))
);

const grantForm = reactive({
  show: false,
  saving: false,
  namespaces: [],
  action: 'r',
  /// 高级项默认折叠:九成场景就是「整个命名空间」,把 group/类型/名称摆在明面上
  /// 只会让人以为必须填。
  advanced: false,
  group: '',
  kind: '*',
  name: '',
  result: null,
});

/// Nacos 的资源串 = <namespaceId>:<group>:<type>/<name>,分隔符 ':',通配 '*'。
/// 三条一写错就「授了等于没授」的规则,预览和实际下发必须共用同一套:
///   1) public 的 namespaceId 是空串,所以它的整空间授权写出来就是 ':*:*';
///   2) 类型选「全部」时,第三段整体塌缩成 '*' —— 写成 '*/*' 在 Nacos 侧永远匹配不上;
///   3) group / 名称留空一律按 '*'。
function buildResource(nsId) {
  const group = grantForm.group.trim() || '*';
  const kind = grantForm.kind || '*';
  const tail = kind === '*' ? '*' : `${kind}/${grantForm.name.trim() || '*'}`;
  return `${nsId || ''}:${group}:${tail}`;
}

/// 实时预览:选几个命名空间就几行,所见即所写。
const grantPreview = computed(() =>
  grantForm.namespaces.map((id) => ({ id, label: nsLabel(id), resource: buildResource(id) }))
);

/// 折叠高级项时把它们复位:留着看不见的 group/名称继续生效,预览之外再无线索,太容易误伤。
function toggleAdvanced() {
  grantForm.advanced = !grantForm.advanced;
  if (!grantForm.advanced) {
    grantForm.group = '';
    grantForm.kind = '*';
    grantForm.name = '';
  }
}

function pickAllNs() {
  grantForm.namespaces = namespaces.value.map((n) => n.namespace_id || '');
}

function clearNs() {
  grantForm.namespaces = [];
}

function openGrant() {
  /// 默认就是左侧正在看的那个空间;没有就退回第一个,免得开局是空表单。
  const def = namespaces.value.some((n) => (n.namespace_id || '') === selectedNs.value)
    ? selectedNs.value
    : namespaces.value[0]?.namespace_id ?? '';
  grantForm.namespaces = namespaces.value.length ? [def] : [];
  grantForm.action = 'r';
  grantForm.advanced = false;
  grantForm.group = '';
  grantForm.kind = '*';
  grantForm.name = '';
  grantForm.result = null;
  grantForm.show = true;
}

const grantResultColumns = [
  {
    title: '命名空间',
    key: 'namespace_id',
    width: 180,
    render: (r) => h('span', { class: 'mono' }, r.namespace_id || 'public'),
  },
  { title: '资源', key: 'resource', render: (r) => h('span', { class: 'mono' }, r.resource || '—') },
  {
    title: '结果',
    key: 'status',
    width: 200,
    render: (r) => {
      const ok = r.status === 'ok';
      return h('div', { class: 'row-ops' }, [
        h('span', { class: `pill pill-${ok ? 'ok' : 'danger'}` }, [
          h(Icon, { name: ok ? 'check' : 'close', size: 11 }),
          h('span', ok ? '已授权' : '失败'),
        ]),
        r.message ? h('span', { class: 'row-note' }, r.message) : null,
      ]);
    },
  },
];

function grantDone(r) {
  message.success(
    r.created_role
      ? `已授权;该账号原本没有角色,已自动创建并绑定角色「${r.role}」`
      : `已授权,经由角色「${r.role}」生效`
  );
  grantForm.show = false;
}

async function saveGrant() {
  const username = selected.value;
  if (!username) return;
  if (!grantForm.namespaces.length) {
    message.warning('至少选一个命名空间');
    return;
  }
  const kind = grantForm.kind || '*';
  const scope = {
    action: grantForm.action,
    group: grantForm.group.trim() || '*',
    kind,
    /// 类型为「全部」时第三段没有名称的落点,统一按 '*' 传,和预览保持逐字一致。
    name: kind === '*' ? '*' : grantForm.name.trim() || '*',
  };
  grantForm.saving = true;
  try {
    if (grantForm.namespaces.length === 1) {
      const r = await api.grantNacosUser(clusterId, {
        username,
        namespace_id: grantForm.namespaces[0] || '',
        ...scope,
      });
      grantForm.result = null;
      grantDone(r);
    } else {
      const r = await api.grantNacosUserBatch(clusterId, {
        username,
        namespaces: grantForm.namespaces,
        ...scope,
      });
      grantForm.result = r;
      /// 部分成功时留在弹窗里:哪个空间没授上、为什么,只有这张表说得清,
      /// 关掉就等于把失败咽了。
      if (r.ok_count === r.total) grantDone(r);
      else message.warning(`部分成功:${r.ok_count}/${r.total} 个命名空间已授权`);
    }
    await loadAccess();
  } catch (e) {
    fail(e, '授权失败');
  } finally {
    grantForm.saving = false;
  }
}

/// 顺序有讲究:默认选中哪个命名空间要看集群登记的那个,所以先等集群信息回来,再拉命名空间
/// —— 拉完会自动选中并带出右侧的配置。授权下拉也要用命名空间列表,账号列表是
/// 「账号与权限」的主线索,一起进页面就加载。
onMounted(async () => {
  await loadCluster();
  loadNamespaces();
  loadUsers();
});
</script>

<template>
  <div class="ncluster">
    <!-- 页头:先说清楚「在改哪个集群」,再给返回路径 -->
    <header class="page-hd">
      <n-button size="small" quaternary @click="router.push('/nacos')">
        <Icon name="list" :size="15" style="margin-right:6px" /> 返回
      </n-button>
      <div class="ttl">
        <Icon name="server" :size="18" />
        <h2>{{ cluster?.name || 'Nacos 集群' }}</h2>
        <span v-if="cluster?.env" class="pill" :class="`env-${cluster.env}`">{{ cluster.env }}</span>
        <span v-if="cluster?.status === 'disabled'" class="pill pill-muted">已停用</span>
        <span class="sub">命名空间 · 配置 · 账号与权限</span>
      </div>
      <ul v-if="cluster?.endpoints?.length" class="eps">
        <li v-for="ep in cluster.endpoints" :key="ep" class="mono ellip" :title="ep">{{ ep }}</li>
      </ul>
    </header>

    <div class="notice">
      <Icon name="alert" :size="15" />
      <span>
        本页所有操作直接作用于远端 Nacos,不经过 opsctl 的配置库;集群模式下权限变更可能有
        <b class="num">~15</b> 秒传播延迟。
      </span>
    </div>

    <n-tabs v-model:value="tab" type="line" animated>
      <!-- ============ 命名空间与配置:左选空间 / 右看它里面的配置 ============ -->
      <!-- Nacos 按命名空间硬隔离,所以「看配置」必须先落在某个空间上,而不是只能看集群登记的那个 -->
      <n-tab-pane name="ns" tab="命名空间与配置">
        <n-card size="small" :bordered="false">
          <template #header>
            <span class="card-title">命名空间与配置</span>
            <span class="card-sub">
              选左边的命名空间,右边就是它里面的配置
              <template v-if="nsFlavor"> · 接口版本 {{ nsFlavor }}</template>
            </span>
          </template>
          <template #header-extra>
            <div class="row-ops">
              <n-button
                size="small"
                :loading="loading.namespaces || loading.configs"
                @click="refreshNs"
              >
                <Icon name="refresh" :size="15" style="margin-right:6px" /> 刷新
              </n-button>
              <n-button size="small" type="primary" :disabled="selectedNs === null" @click="openSync">
                <Icon name="database" :size="15" style="margin-right:6px" /> 同步为模板
              </n-button>
            </div>
          </template>

          <p v-if="nsNotice" class="help">{{ nsNotice }}</p>

          <div v-if="!namespaces.length && !loading.namespaces" class="empty-page sm">
            <Icon name="database" :size="26" />
            <h3>没有读到任何命名空间</h3>
            <p>确认集群可连通且账号有 admin 权限,或直接新建一个业务命名空间。</p>
            <n-button size="small" type="primary" @click="openNsNew">新建命名空间</n-button>
          </div>

          <div v-else class="ns-split">
            <aside class="ns-side">
              <div class="ns-hd">
                <span class="lbl">命名空间</span>
                <n-button size="tiny" type="primary" tertiary @click="openNsNew">
                  <Icon name="plus" :size="13" style="margin-right:4px" /> 新建
                </n-button>
              </div>
              <ul>
                <li
                  v-for="n in namespaces"
                  :key="n.namespace_id || 'public'"
                  :class="{ on: (n.namespace_id || '') === selectedNs }"
                >
                  <button type="button" class="ns-pick" :title="n.desc || n.name || ''" @click="selectNs(n)">
                    <span class="tx">
                      <span class="nm ellip">{{ n.name || 'public' }}</span>
                      <span class="id mono ellip" :class="{ dim: isPublicNs(n) }">
                        {{ n.namespace_id || 'public' }}
                      </span>
                    </span>
                    <span class="cnt num">{{ nsCount(n) }}</span>
                  </button>
                  <!-- public 是 Nacos 内置的(id 为空串),改不了也删不了,所以不出这两个按钮 -->
                  <span v-if="!isPublicNs(n)" class="ns-acts">
                    <n-button size="tiny" quaternary title="编辑命名空间" @click="openNsEdit(n)">
                      <Icon name="edit" :size="13" />
                    </n-button>
                    <n-popconfirm positive-text="删除" negative-text="取消" @positive-click="removeNs(n)">
                      <template #trigger>
                        <n-button size="tiny" quaternary type="error" title="删除命名空间">
                          <Icon name="trash" :size="13" />
                        </n-button>
                      </template>
                      删除命名空间「{{ n.name || n.namespace_id }}」?其中的配置会一并被 Nacos 清除,且不可恢复。
                    </n-popconfirm>
                  </span>
                </li>
              </ul>
            </aside>

            <section class="ns-main">
              <div class="ns-main-hd">
                <span class="nm">{{ currentNsLabel }}</span>
                <span class="mono row-note">{{ selectedNs || 'public' }}</span>
                <span v-if="currentNsType" class="pill" :class="`pill-${currentNsType.tone}`">
                  <Icon name="dot" :size="11" />
                  <span>{{ currentNsType.text }}</span>
                </span>
                <span class="cnt row-note">共 <b class="num">{{ configTotal }}</b> 条配置</span>
              </div>

              <p v-if="configNotice" class="help">{{ configNotice }}</p>

              <div v-if="!configs.length && !loading.configs" class="empty-page sm">
                <Icon name="file" :size="26" />
                <h3>「{{ currentNsLabel }}」里还没有配置</h3>
                <p>可以到 Nacos 总览页用「初始化配置」把模板下发到这个命名空间,再回来查看。</p>
                <n-button size="small" @click="router.push('/nacos')">去总览页</n-button>
              </div>
              <template v-else>
                <n-data-table
                  :columns="configColumns"
                  :data="configs"
                  :loading="loading.configs"
                  :bordered="false"
                  :scroll-x="760"
                  size="small"
                />
                <div v-if="configTotal > configPage.pageSize" class="pager">
                  <n-pagination
                    v-model:page="configPage.pageNo"
                    :page-size="configPage.pageSize"
                    :item-count="configTotal"
                    @update:page="loadConfigs"
                  />
                </div>
              </template>
            </section>
          </div>
        </n-card>
      </n-tab-pane>

      <!-- ============ 账号与权限 ============ -->
      <!-- 左选人、右看权限:把「谁能动哪些命名空间」摆成一屏,而不是让人在三张表里对 -->
      <n-tab-pane name="users" tab="账号与权限">
        <n-card size="small" :bordered="false">
          <template #header>
            <span class="card-title">账号与权限</span>
            <span class="card-sub">选一个账号,右侧就是它能操作的命名空间</span>
          </template>
          <template #header-extra>
            <n-button size="small" :loading="loading.users || loading.access" @click="refreshAccess">
              <Icon name="refresh" :size="15" style="margin-right:6px" /> 刷新
            </n-button>
          </template>

          <div v-if="!users.length && !loading.users" class="empty-page sm">
            <Icon name="server" :size="26" />
            <h3>没有读到任何账号</h3>
            <p>新建一个只给业务用的账号,再在右侧把它需要的命名空间勾上。</p>
            <n-button size="small" type="primary" @click="openUserNew">新建账号</n-button>
          </div>

          <div v-else class="acc-split">
            <aside class="acc-list">
              <div class="acc-hd">
                <span class="lbl">账号</span>
                <n-button size="tiny" type="primary" tertiary @click="openUserNew">
                  <Icon name="plus" :size="13" style="margin-right:4px" /> 新建账号
                </n-button>
              </div>
              <ul>
                <li v-for="u in users" :key="u.username" :class="{ on: u.username === selected }">
                  <button type="button" class="pick mono ellip" :title="u.username" @click="selectUser(u.username)">
                    {{ u.username }}
                  </button>
                  <!-- 行内动作平时藏起来:列表的主职责是「选人」,别让两个图标抢走点击 -->
                  <span class="acts">
                    <n-button size="tiny" quaternary title="重置密码" @click="openReset(u)">
                      <Icon name="edit" :size="13" />
                    </n-button>
                    <n-popconfirm positive-text="删除" negative-text="取消" @positive-click="removeUser(u)">
                      <template #trigger>
                        <n-button size="tiny" quaternary type="error" title="删除账号">
                          <Icon name="trash" :size="13" />
                        </n-button>
                      </template>
                      删除远端账号「{{ u.username }}」?持有 ROLE_ADMIN 的账号会被 Nacos 拒绝删除。
                    </n-popconfirm>
                  </span>
                </li>
              </ul>
              <div v-if="userTotal > userPage.pageSize" class="pager sm">
                <n-pagination
                  v-model:page="userPage.pageNo"
                  :page-size="userPage.pageSize"
                  :item-count="userTotal"
                  size="small"
                  :page-slot="5"
                  @update:page="loadUsers"
                />
              </div>
            </aside>

            <section class="acc-main">
              <div class="acc-main-hd">
                <div class="who">
                  <span class="mono nm">{{ selected || '—' }}</span>
                  <span class="row-note">
                    <template v-if="access.roles.length">角色:{{ access.roles.join(', ') }}</template>
                    <template v-else>尚未绑定角色</template>
                  </span>
                </div>
                <n-button
                  v-if="!access.global_admin"
                  size="small"
                  type="primary"
                  :disabled="!selected"
                  @click="openGrant"
                >
                  <Icon name="plus" :size="15" style="margin-right:6px" /> 授权
                </n-button>
              </div>

              <!-- 全局管理员天然拥有一切,再给它列「有哪些权限」或摆授权表单都是噪音 -->
              <div v-if="access.global_admin" class="notice danger">
                <Icon name="alert" :size="15" />
                <span>全局管理员(<b class="mono">ROLE_ADMIN</b>),拥有全部权限,无需逐个命名空间授权。</span>
              </div>

              <p v-else-if="access.error" class="err">{{ access.error }}</p>

              <div v-else-if="!access.grants.length && !loading.access" class="empty-page sm">
                <Icon name="check" :size="26" />
                <h3>该账号还没有任何权限</h3>
                <p>选一个命名空间和动作即可授权,缺少的角色会自动创建并绑定。</p>
                <n-button size="small" type="primary" :disabled="!selected" @click="openGrant">授权</n-button>
              </div>

              <n-data-table
                v-else
                :columns="accessColumns"
                :data="access.grants"
                :loading="loading.access"
                :bordered="false"
                :scroll-x="900"
                size="small"
              />
            </section>
          </div>
        </n-card>
      </n-tab-pane>
    </n-tabs>

    <!-- ============ 命名空间表单 ============ -->
    <n-modal
      v-model:show="nsForm.show"
      preset="card"
      :title="nsForm.editing ? '编辑命名空间' : '新建命名空间'"
      style="width:560px;max-width:94vw"
    >
      <n-form label-placement="top" :show-feedback="false">
        <n-form-item label="命名空间 ID">
          <n-input
            v-model:value="nsForm.model.namespace_id"
            class="mono"
            :disabled="nsForm.editing"
            placeholder="如 order-dev"
          />
          <p v-if="nsForm.errors.namespace_id" class="err">{{ nsForm.errors.namespace_id }}</p>
          <p v-else-if="nsForm.editing" class="help">ID 是主键,创建后不可修改。</p>
          <p v-else class="help">只能含字母 / 数字 / 下划线 / 连字符,≤128 字符,留空由 Nacos 生成 UUID。</p>
        </n-form-item>

        <n-form-item label="名称" required>
          <n-input v-model:value="nsForm.model.name" placeholder="如 订单-开发环境" />
          <p v-if="nsForm.errors.name" class="err">{{ nsForm.errors.name }}</p>
          <p v-else class="help">不能含 @#$%^&amp;* 这些字符。</p>
        </n-form-item>

        <n-form-item label="描述">
          <n-input
            v-model:value="nsForm.model.desc"
            type="textarea"
            :autosize="{ minRows: 2, maxRows: 4 }"
            placeholder="用途、归属团队等"
          />
        </n-form-item>
      </n-form>

      <template #footer>
        <div class="modal-ft">
          <span class="sp" />
          <n-button size="small" @click="nsForm.show = false">取消</n-button>
          <n-button size="small" type="primary" :loading="nsForm.saving" @click="saveNs">保存</n-button>
        </div>
      </template>
    </n-modal>

    <!-- ============ 新建账号 ============ -->
    <n-modal v-model:show="userForm.show" preset="card" title="新建 Nacos 账号" style="width:520px;max-width:94vw">
      <!-- 这是「远端 Nacos 的账号」,不是本平台登录账号:必须挡掉浏览器把
           opsctl 的登录口令自动灌进来。 -->
      <n-form label-placement="top" :show-feedback="false">
        <n-form-item label="用户名" required>
          <n-input
            v-model:value="userForm.model.username"
            class="mono"
            placeholder="如 order-app"
            :input-props="{ name: 'nacos-remote-user', autocomplete: 'off' }"
          />
          <p v-if="userForm.errors.username" class="err">{{ userForm.errors.username }}</p>
        </n-form-item>
        <n-form-item label="密码" required>
          <n-input
            v-model:value="userForm.model.password"
            type="password"
            show-password-on="click"
            placeholder="直接写入远端 Nacos"
            :input-props="{ name: 'nacos-remote-secret', autocomplete: 'new-password' }"
          />
          <p v-if="userForm.errors.password" class="err">{{ userForm.errors.password }}</p>
          <p v-else class="help">opsctl 不保存这个密码,忘记只能重置。</p>
        </n-form-item>
      </n-form>

      <template #footer>
        <div class="modal-ft">
          <span class="sp" />
          <n-button size="small" @click="userForm.show = false">取消</n-button>
          <n-button size="small" type="primary" :loading="userForm.saving" @click="saveUser">创建</n-button>
        </div>
      </template>
    </n-modal>

    <!-- ============ 重置密码 ============ -->
    <n-modal v-model:show="resetForm.show" preset="card" title="重置密码" style="width:480px;max-width:94vw">
      <n-form label-placement="top" :show-feedback="false">
        <n-form-item label="账号">
          <n-input :value="resetForm.username" class="mono" disabled />
        </n-form-item>
        <n-form-item label="新密码" required>
          <n-input
            v-model:value="resetForm.new_password"
            type="password"
            show-password-on="click"
            placeholder="立即生效"
            :input-props="{ name: 'nacos-remote-secret', autocomplete: 'new-password' }"
          />
          <p v-if="resetForm.error" class="err">{{ resetForm.error }}</p>
        </n-form-item>
      </n-form>

      <template #footer>
        <div class="modal-ft">
          <span class="sp" />
          <n-button size="small" @click="resetForm.show = false">取消</n-button>
          <n-button size="small" type="primary" :loading="resetForm.saving" @click="saveReset">重置</n-button>
        </div>
      </template>
    </n-modal>

    <!-- ============ 授权:账号 → 命名空间(可多选)============ -->
    <!-- 常规路径只有「选空间 + 选动作」两步;资源范围是少数场景,折起来不占视线 -->
    <n-modal v-model:show="grantForm.show" preset="card" title="授权" style="width:600px;max-width:94vw">
      <n-form label-placement="top" :show-feedback="false">
        <n-form-item label="账号">
          <n-input :value="selected" class="mono" disabled />
        </n-form-item>
        <n-form-item>
          <template #label>
            <span class="lbl-row">
              命名空间
              <span class="sp" />
              <n-button text size="tiny" :disabled="!namespaces.length" @click="pickAllNs">全选</n-button>
              <n-button text size="tiny" :disabled="!grantForm.namespaces.length" @click="clearNs">清空</n-button>
            </span>
          </template>
          <n-select
            v-model:value="grantForm.namespaces"
            :options="nsOpts"
            multiple
            filterable
            placeholder="可多选,一次授给多个命名空间"
          />
        </n-form-item>
        <n-form-item label="动作">
          <n-select v-model:value="grantForm.action" :options="ACTION_OPTS" />
        </n-form-item>
      </n-form>

      <n-button text size="small" class="adv-tg" @click="toggleAdvanced">
        <Icon :name="grantForm.advanced ? 'minus' : 'plus'" :size="14" style="margin-right:6px" />
        限定资源范围
        <span class="row-note" style="margin-left:8px">不展开就是整个命名空间</span>
      </n-button>

      <transition name="adv">
        <n-form v-if="grantForm.advanced" label-placement="top" :show-feedback="false" class="adv-box">
          <n-form-item label="类型">
            <n-select v-model:value="grantForm.kind" :options="KIND_OPTS" />
          </n-form-item>
          <div class="adv-row">
            <n-form-item label="group">
              <n-input v-model:value="grantForm.group" class="mono" placeholder="*" />
            </n-form-item>
            <n-form-item label="名称">
              <!-- 类型为「全部」时资源串第三段整体塌缩成 *,名称没有落点,禁掉免得白填 -->
              <n-input
                v-model:value="grantForm.name"
                class="mono"
                placeholder="*"
                :disabled="grantForm.kind === '*'"
              />
            </n-form-item>
          </div>
          <p class="help">
            类型为「全部」时名称不可用:资源串第三段会整体写成 <b class="mono">*</b>。
            group 与名称留空都按 <b class="mono">*</b> 处理。
          </p>
        </n-form>
      </transition>

      <!-- 资源串是 Nacos 侧唯一生效的东西,先摆出来再让人点授权 -->
      <div class="prev-hd">
        将写入 <b class="num">{{ grantPreview.length }}</b> 条资源
      </div>
      <ul v-if="grantPreview.length" class="prev">
        <li v-for="p in grantPreview" :key="p.id">
          <span class="mono">{{ p.resource }}</span>
          <span class="row-note">{{ p.label }}</span>
        </li>
      </ul>
      <p v-else class="help">还没选命名空间。</p>

      <!-- 批量结果:哪个空间成了、哪个没成,逐行交代 -->
      <section v-if="grantForm.result" class="gr-res">
        <div class="res-sum" :class="{ warn: grantForm.result.ok_count !== grantForm.result.total }">
          <Icon
            :name="grantForm.result.ok_count === grantForm.result.total ? 'check' : 'alert'"
            :size="15"
          />
          <span>
            <b class="num">{{ grantForm.result.ok_count }}</b> /
            <b class="num">{{ grantForm.result.total }}</b> 个命名空间已授权
            <template v-if="grantForm.result.role">
              · 角色 <b class="mono">{{ grantForm.result.role }}</b>
            </template>
          </span>
          <span v-if="grantForm.result.ok_count !== grantForm.result.total" class="pill pill-warn">
            部分成功
          </span>
        </div>
        <n-data-table
          :columns="grantResultColumns"
          :data="grantForm.result.items || []"
          :bordered="false"
          :scroll-x="620"
          size="small"
          style="margin-top:10px"
        />
      </section>

      <p class="help gr-note">
        Nacos 的模型是 账号 → 角色 → 权限;该账号没有角色时会自动创建并绑定一个。
        集群模式下变更可能有 <b class="num">~15</b> 秒传播延迟。
      </p>

      <template #footer>
        <div class="modal-ft">
          <span class="sp" />
          <n-button size="small" @click="grantForm.show = false">关闭</n-button>
          <n-button
            size="small"
            type="primary"
            :loading="grantForm.saving"
            :disabled="!grantPreview.length"
            @click="saveGrant"
          >
            授权
          </n-button>
        </div>
      </template>
    </n-modal>

    <!-- ============ 配置正文 ============ -->
    <n-drawer v-model:show="cfgView.show" :width="720" placement="right">
      <n-drawer-content :title="cfgView.data_id || '配置正文'" closable>
        <div class="cfg-meta">
          <span>group <b class="mono">{{ cfgView.group }}</b></span>
          <span>命名空间 <b class="mono">{{ cfgView.namespace || 'public' }}</b></span>
          <span>字节数 <b class="num">{{ cfgView.bytes }}</b></span>
          <span class="sp" />
          <n-button size="tiny" :disabled="!cfgView.content" @click="copyConfig">
            <Icon name="file" :size="14" style="margin-right:6px" /> 复制
          </n-button>
        </div>

        <n-alert v-if="cfgView.error" type="error" :bordered="false">{{ cfgView.error }}</n-alert>
        <n-spin v-else :show="cfgView.loading">
          <p v-if="!cfgView.loading && !cfgView.content" class="help">这条配置在远端的正文是空的。</p>
          <pre v-else class="mono cfg-body">{{ cfgView.content }}</pre>
        </n-spin>
      </n-drawer-content>
    </n-drawer>

    <!-- ============ 同步为模板 ============ -->
    <!-- 用抽屉而不是模态:结果明细是一张表,步骤式披露也需要更宽的版面。 -->
    <n-drawer v-model:show="sync.show" :width="760" placement="right">
      <n-drawer-content :title="`同步为模板 · ${cluster?.name || ''}`" closable>
        <div class="notice">
          <Icon name="alert" :size="15" />
          <span>同步只读远端 Nacos、只写 opsctl 自己的模板库,不会改动集群上的任何配置。</span>
        </div>

        <section class="step">
          <h4><span class="idx">1</span> 同步范围</h4>
          <!-- 命名空间不再让人填一遍:同步的就是左侧选中的那个,填第二遍只会填错 -->
          <div class="res-sum">
            <Icon name="database" :size="15" />
            <span>
              命名空间 <b class="mono">{{ selectedNs || 'public' }}</b>
              <template v-if="currentNs?.name"> · {{ currentNs.name }}</template>
              · 共 <b class="num">{{ configTotal }}</b> 条配置
            </span>
          </div>
          <n-form-item
            label="模板名称"
            label-placement="top"
            :show-feedback="false"
            style="margin-top:12px"
          >
            <n-input v-model:value="sync.templateName" size="small" placeholder="留空自动生成" />
          </n-form-item>
          <p class="help">先「试运行」看清会同步哪些配置,确认无误再「同步」落库。</p>
        </section>

        <!-- 结果区:跑过才出现(渐进式披露),试运行明确标「未落库」 -->
        <section v-if="sync.result" class="step">
          <h4>
            <span class="idx">2</span>
            {{ sync.result.dry_run ? '试运行结果' : '同步结果' }}
          </h4>
          <div class="res-sum">
            <Icon :name="sync.result.dry_run ? 'eye' : 'check'" :size="15" />
            <span>
              共 <b class="num">{{ sync.result.total }}</b> 条 · 命名空间
              <b class="mono">{{ sync.result.namespace || 'public' }}</b>
            </span>
            <span v-if="sync.result.dry_run" class="pill pill-muted">未落库</span>
            <span v-else class="pill pill-ok">模板 {{ sync.result.template_name }}</span>
          </div>
          <n-data-table
            :columns="syncColumns"
            :data="sync.result.items || []"
            :bordered="false"
            :scroll-x="640"
            size="small"
            style="margin-top:10px"
          />
        </section>

        <template #footer>
          <div class="modal-ft">
            <span class="help">同步后到 Nacos 总览页 →「初始化配置」即可回放到其它集群。</span>
            <span class="sp" />
            <n-button size="small" :loading="sync.running" @click="runSync(true)">
              <Icon name="eye" :size="15" style="margin-right:6px" /> 试运行
            </n-button>
            <n-button size="small" type="primary" :loading="sync.running" @click="runSync(false)">
              <Icon name="play" :size="14" style="margin-right:6px" /> 同步
            </n-button>
          </div>
        </template>
      </n-drawer-content>
    </n-drawer>
  </div>
</template>

<style scoped>
.ncluster { display: flex; flex-direction: column; gap: 14px; }

/* ---- 页头 ---- */
.page-hd { display: flex; align-items: center; gap: 14px; flex-wrap: wrap; }
.page-hd .ttl { display: flex; align-items: center; gap: 8px; color: var(--accent); }
.page-hd h2 { margin: 0; font-size: 18px; color: var(--fg); font-weight: 650; }
.page-hd .sub { font-size: 12px; color: var(--muted); }
.eps { list-style: none; margin: 0 0 0 auto; padding: 0; display: flex; flex-direction: column; gap: 2px;
  max-width: 42%; }
.eps li { font-size: 12px; color: var(--fg-2); }
.ellip { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

/* ---- 提示条 ---- */
.notice { display: flex; align-items: center; gap: 10px; padding: 9px 13px; border-radius: 10px;
  font-size: 12.5px; color: var(--fg-2);
  background: color-mix(in oklab, var(--accent), transparent 92%);
  border: 1px solid color-mix(in oklab, var(--accent), transparent 78%); }
.notice > :deep(.ic) { color: var(--accent); }
.notice.warn { margin-bottom: 14px; color: var(--warn);
  background: color-mix(in oklab, var(--warn), transparent 90%);
  border-color: color-mix(in oklab, var(--warn), transparent 72%); }
.notice.warn > :deep(.ic) { color: var(--warn); }

/* ---- 卡片 / 表格 ---- */
.card-title { font-weight: 620; }
.card-sub { margin-left: 10px; font-size: 12px; color: var(--muted); font-weight: 400; }
.row-ops, :deep(.row-ops) { display: flex; align-items: center; gap: 8px; }
:deep(.row-note) { font-size: 12px; color: var(--muted); }
.pager { display: flex; justify-content: flex-end; margin-top: 12px; }

/* ---- 状态 pill:图标 + 文案 + 颜色三重编码 ---- */
:deep(.pill), .pill { display: inline-flex; align-items: center; gap: 4px; font-size: 11.5px;
  padding: 2px 8px; border-radius: 999px; border: 1px solid transparent; white-space: nowrap; }
:deep(.pill-ok), .pill-ok { color: var(--success); background: color-mix(in oklab, var(--success), transparent 88%);
  border-color: color-mix(in oklab, var(--success), transparent 70%); }
:deep(.pill-warn), .pill-warn { color: var(--warn); background: color-mix(in oklab, var(--warn), transparent 88%);
  border-color: color-mix(in oklab, var(--warn), transparent 70%); }
:deep(.pill-danger), .pill-danger { color: var(--danger); background: color-mix(in oklab, var(--danger), transparent 88%);
  border-color: color-mix(in oklab, var(--danger), transparent 70%); }
:deep(.pill-muted), .pill-muted { color: var(--muted); background: rgba(255,255,255,.05);
  border-color: rgba(255,255,255,.1); }
.env-dev { color: var(--accent); background: rgba(255,255,255,.05); border-color: rgba(255,255,255,.12); }
.env-test { color: var(--warn); background: color-mix(in oklab, var(--warn), transparent 88%);
  border-color: color-mix(in oklab, var(--warn), transparent 70%); }
.env-prod { color: var(--danger); background: color-mix(in oklab, var(--danger), transparent 86%);
  border-color: color-mix(in oklab, var(--danger), transparent 62%); }

/* ---- 空状态:always 给下一步动作 ---- */
.empty-page { padding: 56px 20px; text-align: center; color: var(--muted); }
.empty-page.sm { padding: 34px 16px; }
.empty-page h3 { margin: 10px 0 4px; font-size: 15px; color: var(--fg-2); font-weight: 600; }
.empty-page p { margin: 0 0 14px; font-size: 12.5px; }

/* ---- 表单 ---- */
.err { margin: 4px 0 0; font-size: 12px; color: var(--danger); }
.help { margin: 4px 0 0; font-size: 12px; color: var(--muted); }
.modal-ft { display: flex; align-items: center; gap: 8px; }
.sp { flex: 1; }

/* ---- 命名空间与配置:左选空间 / 右看它里面的配置 ---- */
/* 和「账号与权限」用同一套主从观感:上下级关系靠版面表达,比一行只读文字更难看错 */
.ns-split { display: grid; grid-template-columns: 260px 1fr; gap: 16px; align-items: start; }
.ns-side { border-right: 1px solid rgba(255,255,255,.07); padding-right: 14px; }
.ns-hd { display: flex; align-items: center; gap: 8px; margin-bottom: 8px; }
.ns-hd .lbl { font-size: 12px; color: var(--muted); }
.ns-hd :deep(button) { margin-left: auto; }
.ns-side ul { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 2px;
  max-height: 58vh; overflow: auto; }
.ns-side li { position: relative; display: flex; align-items: center; gap: 2px; border-radius: 8px;
  padding: 0 4px 0 0; transition: background .16s ease; }
/* 选中标记用左侧 2px 竖条:颜色之外再给一个位置信号,弱色觉也分得清 */
.ns-side li::before { content: ''; position: absolute; left: 0; top: 50%; width: 2px; height: 0;
  border-radius: 2px; background: var(--accent); transform: translateY(-50%);
  transition: height .18s ease; }
.ns-side li:hover { background: rgba(255,255,255,.04); }
.ns-side li.on { background: color-mix(in oklab, var(--accent), transparent 86%); }
.ns-side li.on::before { height: 62%; }
.ns-pick { flex: 1; min-width: 0; display: flex; align-items: center; gap: 8px; text-align: left;
  background: none; border: 0; cursor: pointer; padding: 7px 8px; border-radius: 8px; }
.ns-pick:focus-visible { outline: 2px solid var(--accent); outline-offset: -2px; }
.ns-pick .tx { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 1px; }
.ns-pick .nm { font-size: 12.5px; color: var(--fg-2); }
.ns-side li.on .ns-pick .nm { color: var(--accent); font-weight: 600; }
.ns-pick .id { font-size: 11px; color: var(--muted); }
.ns-pick .cnt { flex: none; font-size: 11px; color: var(--muted); padding: 1px 7px; border-radius: 999px;
  background: rgba(255,255,255,.05); }
/* 行内动作靠 hover / 焦点浮出:列表的主职责是选空间 */
.ns-side .ns-acts { display: flex; gap: 2px; opacity: 0; transition: opacity .16s ease; }
.ns-side li:hover .ns-acts, .ns-side li.on .ns-acts, .ns-side .ns-acts:focus-within { opacity: 1; }

.ns-main { min-width: 0; display: flex; flex-direction: column; gap: 12px; }
.ns-main-hd { display: flex; align-items: baseline; gap: 10px; flex-wrap: wrap; }
.ns-main-hd .nm { font-size: 14px; color: var(--fg); font-weight: 600; }
.ns-main-hd .cnt { margin-left: auto; }
.ns-main-hd b { color: var(--fg-2); }

/* ---- 账号与权限:左选人 / 右看权限 ---- */
.acc-split { display: grid; grid-template-columns: 240px 1fr; gap: 16px; align-items: start; }
.acc-list { border-right: 1px solid rgba(255,255,255,.07); padding-right: 14px; }
.acc-hd { display: flex; align-items: center; gap: 8px; margin-bottom: 8px; }
.acc-hd .lbl { font-size: 12px; color: var(--muted); }
.acc-hd :deep(button) { margin-left: auto; }
.acc-list ul { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 2px; }
.acc-list li { display: flex; align-items: center; gap: 2px; border-radius: 8px; padding: 0 4px 0 0;
  transition: background .16s ease; }
.acc-list li:hover { background: rgba(255,255,255,.04); }
.acc-list li.on { background: color-mix(in oklab, var(--accent), transparent 86%); }
.acc-list li.on .pick { color: var(--accent); }
.pick { flex: 1; min-width: 0; text-align: left; background: none; border: 0; cursor: pointer;
  padding: 7px 8px; font-size: 12.5px; color: var(--fg-2); border-radius: 8px; }
.pick:focus-visible { outline: 2px solid var(--accent); outline-offset: -2px; }
/* 行内动作靠 hover / 焦点浮出:列表的主职责是选人 */
.acc-list .acts { display: flex; gap: 2px; opacity: 0; transition: opacity .16s ease; }
.acc-list li:hover .acts, .acc-list li.on .acts, .acc-list .acts:focus-within { opacity: 1; }
.pager.sm { margin-top: 10px; justify-content: center; }

.acc-main { min-width: 0; display: flex; flex-direction: column; gap: 12px; }
.acc-main-hd { display: flex; align-items: center; gap: 12px; flex-wrap: wrap; }
.acc-main-hd .who { display: flex; align-items: baseline; gap: 10px; min-width: 0; }
.acc-main-hd .nm { font-size: 14px; color: var(--fg); font-weight: 600; }
.acc-main-hd :deep(button) { margin-left: auto; }
.notice.danger { color: var(--danger);
  background: color-mix(in oklab, var(--danger), transparent 90%);
  border-color: color-mix(in oklab, var(--danger), transparent 72%); }
.notice.danger > :deep(.ic) { color: var(--danger); }
:deep(.ns-cell) { display: flex; align-items: baseline; gap: 8px; min-width: 0; }
.gr-note { margin-top: 14px; line-height: 1.7; }
.gr-note b { color: var(--fg-2); }

/* ---- 授权弹窗:多选空间 + 折叠的资源范围 + 资源预览 ---- */
.lbl-row { display: flex; align-items: center; gap: 10px; width: 100%; }
.adv-tg { margin-top: 14px; }
.adv-box { margin-top: 10px; padding: 12px 14px; border-radius: 8px;
  background: var(--surface-warm); border: 1px solid rgba(255,255,255,.06); }
.adv-row { display: grid; grid-template-columns: 1fr 1fr; gap: 12px; margin-top: 10px; }
/* 展开是「更多细节」而不是「换了个界面」:短距离下滑 + 淡入,别抢注意力 */
.adv-enter-active, .adv-leave-active { transition: opacity .18s ease, transform .18s ease; }
.adv-enter-from, .adv-leave-to { opacity: 0; transform: translateY(-4px); }

.prev-hd { margin-top: 14px; font-size: 12px; color: var(--muted); }
.prev-hd b { color: var(--fg-2); }
/* 资源串是 Nacos 侧唯一生效的东西,给它一块和正文分开的底,一眼看得出「这就是要写的」 */
.prev { margin: 6px 0 0; padding: 10px 12px; border-radius: 8px; background: var(--bg);
  border: 1px solid rgba(255,255,255,.06); list-style: none;
  display: flex; flex-direction: column; gap: 4px; max-height: 168px; overflow: auto; }
.prev li { display: flex; align-items: baseline; gap: 10px; min-width: 0;
  font-size: 12.5px; color: var(--fg-2); }
.prev li .mono { word-break: break-all; }
.prev li .row-note { margin-left: auto; flex: none; }

.gr-res { margin-top: 14px; }
.res-sum.warn { color: var(--warn);
  background: color-mix(in oklab, var(--warn), transparent 90%); }
.res-sum.warn > :deep(.ic) { color: var(--warn); }

/* ---- 配置抽屉 ---- */
.cfg-meta { display: flex; align-items: center; gap: 14px; margin: 0 0 12px;
  font-size: 12.5px; color: var(--muted); }
.cfg-meta b { color: var(--fg-2); font-weight: 600; }
.cfg-body { margin: 0; padding: 12px 14px; border-radius: 8px; background: var(--bg);
  border: 1px solid rgba(255,255,255,.06); font-size: 12.5px; line-height: 1.65; color: var(--fg-2);
  white-space: pre-wrap; word-break: break-all; max-height: 62vh; overflow: auto; }

/* ---- 同步抽屉:沿用总览页「初始化配置」的步骤条观感 ---- */
.step { padding: 0 0 18px; }
.step h4 { display: flex; align-items: center; gap: 8px; margin: 0 0 10px;
  font-size: 13px; color: var(--fg); font-weight: 620; }
.step .idx { display: grid; place-items: center; width: 20px; height: 20px; border-radius: 6px;
  background: color-mix(in oklab, var(--accent), transparent 82%); color: var(--accent);
  font-size: 11px; font-weight: 700; }
.res-sum { display: flex; align-items: center; gap: 8px; padding: 9px 12px; border-radius: 8px;
  background: var(--bg); font-size: 12.5px; color: var(--fg-2);
  transition: background .18s ease; }
.res-sum > :deep(.ic) { color: var(--accent); }

/* ---- 排版细节 ---- */
.mono, :deep(.mono) { font-family: ui-monospace, SFMono-Regular, "JetBrains Mono", Consolas, monospace; }
.num, :deep(.num) { font-variant-numeric: tabular-nums; }
.dim, :deep(.dim) { color: var(--muted); }
.mono :deep(input), .mono :deep(textarea) {
  font-family: ui-monospace, SFMono-Regular, "JetBrains Mono", Consolas, monospace; font-size: 12.5px;
}

/* 键盘可达性:所有可点元素保留清晰焦点环 */
.page-hd :deep(button:focus-visible), .row-ops :deep(button:focus-visible) {
  outline: 2px solid var(--accent); outline-offset: 2px;
}

@media (max-width: 860px) {
  .eps { margin-left: 0; max-width: 100%; }
  /* 窄屏放不下侧栏,改成上下堆叠;右侧不再需要左边框 */
  .acc-split, .ns-split { grid-template-columns: 1fr; }
  .acc-list, .ns-side { border-right: 0; border-bottom: 1px solid rgba(255,255,255,.07);
    padding: 0 0 12px; }
}

@media (prefers-reduced-motion: reduce) {
  .res-sum, .acc-list li, .acc-list .acts,
  .adv-enter-active, .adv-leave-active,
  .ns-side li, .ns-side li::before, .ns-side .ns-acts { transition: none; }
}
</style>
