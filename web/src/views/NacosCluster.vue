<script setup>
// Nacos 集群管理:命名空间 / 账号 / 角色绑定 / 权限赋权。
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
const tab = ref('namespaces');
const loading = reactive({
  cluster: false,
  namespaces: false,
  users: false,
  roles: false,
  perms: false,
});
/// 角色 / 权限两张表按需加载:一进页面就打四次远端有点重。
const visited = reactive({ roles: false, perms: false });

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

function nsTypePill(type) {
  const t = NS_TYPE[Number(type)] || { text: '未知', tone: 'muted' };
  return h('span', { class: `pill pill-${t.tone}` }, [
    h(Icon, { name: 'dot', size: 11 }),
    h('span', t.text),
  ]);
}

async function loadNamespaces() {
  loading.namespaces = true;
  try {
    const r = await api.nacosNamespaces(clusterId);
    namespaces.value = r.items || [];
    nsFlavor.value = r.flavor || '';
    nsNotice.value = r.message || '';
  } catch (e) {
    fail(e, '加载命名空间失败');
  } finally {
    loading.namespaces = false;
  }
}

const nsColumns = [
  {
    title: '命名空间 ID',
    key: 'namespace_id',
    width: 240,
    render: (r) =>
      isPublicNs(r)
        ? h('span', { class: 'mono dim' }, 'public')
        : h('span', { class: 'mono' }, r.namespace_id),
  },
  { title: '名称', key: 'name', render: (r) => r.name || '—' },
  { title: '描述', key: 'desc', render: (r) => r.desc || '—' },
  {
    title: '配置数',
    key: 'config_count',
    width: 92,
    render: (r) =>
      h('span', { class: 'num' }, r.config_count === null || r.config_count === undefined ? '—' : r.config_count),
  },
  { title: '类型', key: 'type', width: 116, render: (r) => nsTypePill(r.type) },
  {
    title: '操作',
    key: 'ops',
    width: 150,
    render: (r) => {
      if (isPublicNs(r)) {
        return h('div', { class: 'row-ops' }, [
          h('span', { class: 'row-note' }, '内置,不可修改'),
        ]);
      }
      return h('div', { class: 'row-ops' }, [
        h(NButton, { size: 'tiny', onClick: () => openNsEdit(r) }, { default: () => '编辑' }),
        h(
          NPopconfirm,
          { onPositiveClick: () => removeNs(r), positiveText: '删除', negativeText: '取消' },
          {
            trigger: () =>
              h(NButton, { size: 'tiny', type: 'error', tertiary: true }, { default: () => '删除' }),
            default: () =>
              `删除命名空间「${r.name || r.namespace_id}」?其中的配置会一并被 Nacos 清除,且不可恢复。`,
          }
        ),
      ]);
    },
  },
];

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

// ============ 2. 账号 ============

const users = ref([]);
const userTotal = ref(0);
const userPage = reactive({ pageNo: 1, pageSize: 20 });

async function loadUsers() {
  loading.users = true;
  try {
    const r = await api.nacosUsers(clusterId, { page_no: userPage.pageNo, page_size: userPage.pageSize });
    users.value = r.items || [];
    userTotal.value = r.total || 0;
  } catch (e) {
    fail(e, '加载账号失败');
  } finally {
    loading.users = false;
  }
}

const userColumns = [
  { title: '用户名', key: 'username', render: (r) => h('span', { class: 'mono' }, r.username) },
  {
    title: '操作',
    key: 'ops',
    width: 190,
    render: (r) =>
      h('div', { class: 'row-ops' }, [
        h(NButton, { size: 'tiny', onClick: () => openReset(r) }, { default: () => '重置密码' }),
        h(
          NPopconfirm,
          { onPositiveClick: () => removeUser(r), positiveText: '删除', negativeText: '取消' },
          {
            trigger: () =>
              h(NButton, { size: 'tiny', type: 'error', tertiary: true }, { default: () => '删除' }),
            default: () =>
              `删除远端账号「${r.username}」?持有 ROLE_ADMIN 的账号会被 Nacos 拒绝删除。`,
          }
        ),
      ]),
  },
];

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
    await api.createNacosUser(clusterId, {
      username: userForm.model.username.trim(),
      password: userForm.model.password,
    });
    message.success('账号已创建');
    userForm.show = false;
    await loadUsers();
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
    await loadUsers();
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

// ============ 3. 角色绑定 ============

const roles = ref([]);
const roleTotal = ref(0);
const rolePage = reactive({ pageNo: 1, pageSize: 20 });

async function loadRoles() {
  loading.roles = true;
  try {
    const r = await api.nacosRoles(clusterId, { page_no: rolePage.pageNo, page_size: rolePage.pageSize });
    roles.value = r.items || [];
    roleTotal.value = r.total || 0;
  } catch (e) {
    fail(e, '加载角色绑定失败');
  } finally {
    loading.roles = false;
  }
}

const roleColumns = [
  { title: '角色', key: 'role', render: (r) => h('span', { class: 'mono' }, r.role) },
  { title: '账号', key: 'username', render: (r) => h('span', { class: 'mono' }, r.username) },
  {
    title: '操作',
    key: 'ops',
    width: 110,
    render: (r) =>
      h(
        NPopconfirm,
        { onPositiveClick: () => unbindRole(r), positiveText: '解绑', negativeText: '取消' },
        {
          trigger: () =>
            h(NButton, { size: 'tiny', type: 'error', tertiary: true }, { default: () => '解绑' }),
          default: () => `解除「${r.username}」与角色「${r.role}」的绑定?该角色的权限会立即对其失效。`,
        }
      ),
  },
];

const bindForm = reactive({ show: false, saving: false, errors: {}, model: { role: '', username: null } });

function openBind() {
  bindForm.errors = {};
  bindForm.model = { role: '', username: users.value[0]?.username || null };
  bindForm.show = true;
}

const userOpts = computed(() => users.value.map((u) => ({ label: u.username, value: u.username })));

async function saveBind() {
  const e = {};
  const role = bindForm.model.role.trim();
  if (!role) e.role = '请填写角色名';
  else if (role.toUpperCase() === 'ROLE_ADMIN') e.role = 'Nacos 拒绝通过接口创建 ROLE_ADMIN';
  if (!bindForm.model.username) e.username = '请选择账号';
  bindForm.errors = e;
  if (Object.keys(e).length) return;
  bindForm.saving = true;
  try {
    await api.bindNacosRole(clusterId, { role, username: bindForm.model.username });
    message.success(`已把「${bindForm.model.username}」绑定到角色「${role}」`);
    bindForm.show = false;
    await loadRoles();
  } catch (err) {
    fail(err, '绑定角色失败');
  } finally {
    bindForm.saving = false;
  }
}

async function unbindRole(r) {
  try {
    await api.unbindNacosRole(clusterId, { role: r.role, username: r.username });
    message.success('已解绑');
    await loadRoles();
  } catch (e) {
    fail(e, '解绑失败');
  }
}

// ============ 4. 权限 ============

const perms = ref([]);
const permTotal = ref(0);
const permPage = reactive({ pageNo: 1, pageSize: 20 });
const permRoleFilter = ref('');

async function loadPerms() {
  loading.perms = true;
  try {
    const params = { page_no: permPage.pageNo, page_size: permPage.pageSize };
    if (permRoleFilter.value.trim()) params.role = permRoleFilter.value.trim();
    const r = await api.nacosPermissions(clusterId, params);
    perms.value = r.items || [];
    permTotal.value = r.total || 0;
  } catch (e) {
    fail(e, '加载权限失败');
  } finally {
    loading.perms = false;
  }
}

function filterPerms() {
  permPage.pageNo = 1;
  loadPerms();
}

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

const permColumns = [
  { title: '角色', key: 'role', width: 200, render: (r) => h('span', { class: 'mono' }, r.role) },
  { title: '资源', key: 'resource', render: (r) => h('span', { class: 'mono' }, r.resource) },
  { title: '动作', key: 'action', width: 120, render: (r) => actionPill(r.action) },
  {
    title: '操作',
    key: 'ops',
    width: 110,
    render: (r) =>
      h(
        NPopconfirm,
        { onPositiveClick: () => revokePerm(r), positiveText: '收回', negativeText: '取消' },
        {
          trigger: () =>
            h(NButton, { size: 'tiny', type: 'error', tertiary: true }, { default: () => '收回' }),
          default: () => `收回角色「${r.role}」对 ${r.resource} 的 ${r.action} 权限?`,
        }
      ),
  },
];

const RES_TYPES = [
  { label: '整个命名空间 *', value: '*' },
  { label: '配置 config', value: 'config' },
  { label: '服务 naming', value: 'naming' },
];

const ACTION_OPTS = [
  { label: 'r 只读', value: 'r' },
  { label: 'w 只写', value: 'w' },
  { label: 'rw 读写', value: 'rw' },
];

const grantForm = reactive({
  show: false,
  saving: false,
  errors: {},
  model: { role: '', namespace_id: '', res_type: '*', group: '*', name: '*', action: 'r' },
});

function openGrant() {
  grantForm.errors = {};
  grantForm.model = {
    role: roles.value[0]?.role || '',
    namespace_id: namespaces.value[0]?.namespace_id ?? '',
    res_type: '*',
    group: '*',
    name: '*',
    action: 'r',
  };
  grantForm.show = true;
}

/// 命名空间下拉:public 的 id 是空串,原样传给 Nacos(资源串第一段留空)。
const nsOpts = computed(() =>
  namespaces.value.map((n) => ({
    label: n.namespace_id ? `${n.name || n.namespace_id}(${n.namespace_id})` : 'public(默认命名空间)',
    value: n.namespace_id || '',
  }))
);

/// 角色下拉从已有绑定里去重;Nacos 只认已存在的角色,所以这里不鼓励乱填。
const roleOpts = computed(() => {
  const seen = new Set();
  const out = [];
  for (const r of roles.value) {
    if (r.role && !seen.has(r.role)) {
      seen.add(r.role);
      out.push({ label: r.role, value: r.role });
    }
  }
  return out;
});

/// 资源串格式:<namespaceId>:<group>:<type>/<name>;类型选 * 时整个第三段塌缩成 *。
/// 官方控制台也只写 <namespaceId>:*:*,这里把三段拆开只是为了避免手敲出错。
const grantResource = computed(() => {
  const m = grantForm.model;
  const group = (m.group || '').trim() || '*';
  const third = m.res_type === '*' ? '*' : `${m.res_type}/${(m.name || '').trim() || '*'}`;
  return `${m.namespace_id || ''}:${group}:${third}`;
});

async function saveGrant() {
  const e = {};
  const role = String(grantForm.model.role || '').trim();
  if (!role) e.role = '请填写角色名';
  grantForm.errors = e;
  if (Object.keys(e).length) return;
  grantForm.saving = true;
  try {
    await api.grantNacosPermission(clusterId, {
      role,
      resource: grantResource.value,
      action: grantForm.model.action,
    });
    message.success(`已为角色「${role}」赋权 ${grantResource.value}`);
    grantForm.show = false;
    await loadPerms();
  } catch (err) {
    fail(err, '赋权失败(角色不存在时 Nacos 会直接拒绝)');
  } finally {
    grantForm.saving = false;
  }
}

async function revokePerm(r) {
  try {
    await api.revokeNacosPermission(clusterId, {
      role: r.role,
      resource: r.resource,
      action: r.action,
    });
    message.success('权限已收回');
    await loadPerms();
  } catch (e) {
    fail(e, '收回失败');
  }
}

// ---- 标签页:角色 / 权限首次进入才拉数据 ----

function onTab(v) {
  if (v === 'roles' && !visited.roles) {
    visited.roles = true;
    loadRoles();
  }
  if (v === 'perms' && !visited.perms) {
    visited.perms = true;
    loadPerms();
  }
}

/// 绑定角色需要账号下拉、赋权需要命名空间下拉,所以这两张表进页面就加载。
onMounted(() => {
  loadCluster();
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
        <span class="sub">命名空间 · 账号 · 角色 · 权限</span>
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

    <n-tabs v-model:value="tab" type="line" animated @update:value="onTab">
      <!-- ============ 命名空间 ============ -->
      <n-tab-pane name="namespaces" tab="命名空间">
        <n-card size="small" :bordered="false">
          <template #header>
            <span class="card-title">命名空间</span>
            <span class="card-sub">
              public 为内置命名空间(ID 为空),不可修改
              <template v-if="nsFlavor"> · 接口版本 {{ nsFlavor }}</template>
            </span>
          </template>
          <template #header-extra>
            <div class="row-ops">
              <n-button size="small" :loading="loading.namespaces" @click="loadNamespaces">
                <Icon name="refresh" :size="15" style="margin-right:6px" /> 刷新
              </n-button>
              <n-button size="small" type="primary" @click="openNsNew">
                <Icon name="plus" :size="15" style="margin-right:6px" /> 新建命名空间
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
          <n-data-table
            v-else
            :columns="nsColumns"
            :data="namespaces"
            :loading="loading.namespaces"
            :bordered="false"
            :scroll-x="900"
            size="small"
          />
        </n-card>
      </n-tab-pane>

      <!-- ============ 账号 ============ -->
      <n-tab-pane name="users" tab="账号">
        <n-card size="small" :bordered="false">
          <template #header>
            <span class="card-title">Nacos 账号</span>
            <span class="card-sub">远端鉴权账号,与 opsctl 登录账号无关</span>
          </template>
          <template #header-extra>
            <div class="row-ops">
              <n-button size="small" :loading="loading.users" @click="loadUsers">
                <Icon name="refresh" :size="15" style="margin-right:6px" /> 刷新
              </n-button>
              <n-button size="small" type="primary" @click="openUserNew">
                <Icon name="plus" :size="15" style="margin-right:6px" /> 新建账号
              </n-button>
            </div>
          </template>

          <div v-if="!users.length && !loading.users" class="empty-page sm">
            <Icon name="server" :size="26" />
            <h3>没有读到任何账号</h3>
            <p>新建一个只给业务用的账号,再到「角色绑定」里挂上角色。</p>
            <n-button size="small" type="primary" @click="openUserNew">新建账号</n-button>
          </div>
          <template v-else>
            <n-data-table
              :columns="userColumns"
              :data="users"
              :loading="loading.users"
              :bordered="false"
              :scroll-x="560"
              size="small"
            />
            <div class="pager">
              <n-pagination
                v-model:page="userPage.pageNo"
                :page-size="userPage.pageSize"
                :item-count="userTotal"
                @update:page="loadUsers"
              />
            </div>
          </template>
        </n-card>
      </n-tab-pane>

      <!-- ============ 角色绑定 ============ -->
      <n-tab-pane name="roles" tab="角色绑定">
        <n-card size="small" :bordered="false">
          <template #header>
            <span class="card-title">角色绑定</span>
            <span class="card-sub">角色是「账号 → 权限」的中间层,绑定即创建角色</span>
          </template>
          <template #header-extra>
            <div class="row-ops">
              <n-button size="small" :loading="loading.roles" @click="loadRoles">
                <Icon name="refresh" :size="15" style="margin-right:6px" /> 刷新
              </n-button>
              <n-button size="small" type="primary" @click="openBind">
                <Icon name="plus" :size="15" style="margin-right:6px" /> 绑定角色
              </n-button>
            </div>
          </template>

          <div v-if="!roles.length && !loading.roles" class="empty-page sm">
            <Icon name="list" :size="26" />
            <h3>还没有任何角色绑定</h3>
            <p>先把账号绑定到一个自定义角色,再到「权限」里给这个角色赋权。</p>
            <n-button size="small" type="primary" @click="openBind">绑定角色</n-button>
          </div>
          <template v-else>
            <n-data-table
              :columns="roleColumns"
              :data="roles"
              :loading="loading.roles"
              :bordered="false"
              :scroll-x="620"
              size="small"
            />
            <div class="pager">
              <n-pagination
                v-model:page="rolePage.pageNo"
                :page-size="rolePage.pageSize"
                :item-count="roleTotal"
                @update:page="loadRoles"
              />
            </div>
          </template>
        </n-card>
      </n-tab-pane>

      <!-- ============ 权限 ============ -->
      <n-tab-pane name="perms" tab="权限">
        <n-card size="small" :bordered="false">
          <template #header>
            <span class="card-title">权限</span>
            <span class="card-sub">资源串 = 命名空间:分组:类型/名称,动作 r / w / rw</span>
          </template>
          <template #header-extra>
            <div class="row-ops">
              <n-input
                v-model:value="permRoleFilter"
                class="mono flt"
                size="small"
                clearable
                placeholder="按角色过滤"
                @keyup.enter="filterPerms"
                @clear="filterPerms"
              />
              <n-button size="small" :loading="loading.perms" @click="filterPerms">
                <Icon name="refresh" :size="15" style="margin-right:6px" /> 刷新
              </n-button>
              <n-button size="small" type="primary" @click="openGrant">
                <Icon name="plus" :size="15" style="margin-right:6px" /> 赋权
              </n-button>
            </div>
          </template>

          <div v-if="!perms.length && !loading.perms" class="empty-page sm">
            <Icon name="check" :size="26" />
            <h3>没有匹配的权限记录</h3>
            <p>赋权前角色必须已存在;可先到「角色绑定」创建角色,再回来赋权。</p>
            <n-button size="small" type="primary" @click="openGrant">赋权</n-button>
          </div>
          <template v-else>
            <n-data-table
              :columns="permColumns"
              :data="perms"
              :loading="loading.perms"
              :bordered="false"
              :scroll-x="820"
              size="small"
            />
            <div class="pager">
              <n-pagination
                v-model:page="permPage.pageNo"
                :page-size="permPage.pageSize"
                :item-count="permTotal"
                @update:page="loadPerms"
              />
            </div>
          </template>
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

    <!-- ============ 绑定角色 ============ -->
    <n-modal v-model:show="bindForm.show" preset="card" title="绑定角色" style="width:520px;max-width:94vw">
      <div class="notice warn">
        <Icon name="alert" :size="15" />
        <span>Nacos 拒绝通过接口创建 <b class="mono">ROLE_ADMIN</b>,请使用自定义角色名。</span>
      </div>

      <n-form label-placement="top" :show-feedback="false">
        <n-form-item label="角色" required>
          <n-input v-model:value="bindForm.model.role" class="mono" placeholder="如 order-ro" />
          <p v-if="bindForm.errors.role" class="err">{{ bindForm.errors.role }}</p>
          <p v-else class="help">角色不存在时会由这次绑定自动创建。</p>
        </n-form-item>
        <n-form-item label="账号" required>
          <n-select
            v-model:value="bindForm.model.username"
            :options="userOpts"
            filterable
            placeholder="从「账号」标签页读取"
          />
          <p v-if="bindForm.errors.username" class="err">{{ bindForm.errors.username }}</p>
        </n-form-item>
      </n-form>

      <template #footer>
        <div class="modal-ft">
          <span class="sp" />
          <n-button size="small" @click="bindForm.show = false">取消</n-button>
          <n-button size="small" type="primary" :loading="bindForm.saving" @click="saveBind">绑定</n-button>
        </div>
      </template>
    </n-modal>

    <!-- ============ 赋权 ============ -->
    <n-modal v-model:show="grantForm.show" preset="card" title="赋权" style="width:600px;max-width:94vw">
      <div class="notice warn">
        <Icon name="alert" :size="15" />
        <span>赋权前角色必须已存在;集群模式下变更可能需要 <b class="num">~15</b> 秒才全节点生效。</span>
      </div>

      <n-form label-placement="top" :show-feedback="false">
        <n-form-item label="角色" required>
          <n-select
            v-model:value="grantForm.model.role"
            :options="roleOpts"
            filterable
            tag
            placeholder="选择已有角色,或输入角色名"
          />
          <p v-if="grantForm.errors.role" class="err">{{ grantForm.errors.role }}</p>
          <p v-else class="help">下拉项来自「角色绑定」已读到的角色。</p>
        </n-form-item>

        <div class="f-grid">
          <n-form-item label="命名空间">
            <n-select v-model:value="grantForm.model.namespace_id" :options="nsOpts" filterable />
          </n-form-item>
          <n-form-item label="资源类型">
            <n-select v-model:value="grantForm.model.res_type" :options="RES_TYPES" />
          </n-form-item>
        </div>

        <div class="f-grid">
          <n-form-item label="分组 group">
            <n-input
              v-model:value="grantForm.model.group"
              class="mono"
              :disabled="grantForm.model.res_type === '*'"
              placeholder="*"
            />
          </n-form-item>
          <n-form-item label="名称">
            <n-input
              v-model:value="grantForm.model.name"
              class="mono"
              :disabled="grantForm.model.res_type === '*'"
              placeholder="*"
            />
          </n-form-item>
        </div>

        <n-form-item label="动作">
          <n-select v-model:value="grantForm.model.action" :options="ACTION_OPTS" />
        </n-form-item>

        <div class="res-preview">
          <span class="lbl">资源串</span>
          <b class="mono">{{ grantResource }}</b>
          <span class="dim">选「整个命名空间」时第三段塌缩为 *,public 的第一段为空</span>
        </div>
      </n-form>

      <template #footer>
        <div class="modal-ft">
          <span class="sp" />
          <n-button size="small" @click="grantForm.show = false">取消</n-button>
          <n-button size="small" type="primary" :loading="grantForm.saving" @click="saveGrant">赋权</n-button>
        </div>
      </template>
    </n-modal>
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
.flt { width: 180px; }
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
.f-grid { display: grid; gap: 12px; grid-template-columns: 1fr 1fr; }
.err { margin: 4px 0 0; font-size: 12px; color: var(--danger); }
.help { margin: 4px 0 0; font-size: 12px; color: var(--muted); }
.modal-ft { display: flex; align-items: center; gap: 8px; }
.sp { flex: 1; }
.res-preview { display: flex; align-items: baseline; gap: 10px; flex-wrap: wrap; margin-top: 14px;
  padding: 9px 12px; border-radius: 8px; background: var(--bg); font-size: 12.5px;
  transition: background .18s ease; }
.res-preview .lbl { color: var(--muted); }
.res-preview b { color: var(--accent); font-weight: 600; }

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
  .f-grid { grid-template-columns: 1fr; }
  .eps { margin-left: 0; max-width: 100%; }
  .flt { width: 130px; }
}

@media (prefers-reduced-motion: reduce) {
  .res-preview { transition: none; }
}
</style>
