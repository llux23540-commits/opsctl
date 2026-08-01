<script setup>
// Nacos 管理:集群总览(实时节点状态)+ 配置初始化(模板 / 变量 / 试运行)。
// 视觉沿用平台既有的深色 + 青色 token,遵循数据密集型运维面板的规则:
// 状态用「图标 + 文案 + 颜色」三重编码、地址与 dataId 用等宽字体、
// 破坏性操作二次确认、空状态给下一步动作、动画 150–220ms 且尊重 reduced-motion。
import { computed, h, onMounted, reactive, ref } from 'vue';
import { useRouter } from 'vue-router';
import { NButton, NPopconfirm, useDialog, useMessage } from 'naive-ui';
import { api } from '../api';
import Icon from '../components/Icon.vue';
import NacosConfigItems from '../components/NacosConfigItems.vue';

const message = useMessage();
const dialog = useDialog();
const router = useRouter();

const tab = ref('clusters');
const clusters = ref([]);
const templates = ref([]);
const runs = ref([]);
const loading = reactive({ clusters: false, templates: false, runs: false });

/// cluster id → { loading, ok, source, latency_ms, message, nodes[] }
const health = reactive({});

const fmtTime = (t) => (t ? new Date(t * 1000).toLocaleString('zh-CN') : '—');

// 金库封存时,带口令的集群既不能登记也不能取用凭据(建/改/节点/配置/初始化全 503)。
// 先摆明状态并给出解封入口,而不是让用户点到一半吃一个 toast。
const vaultSealed = ref(false);
const credentialedCount = computed(() => clusters.value.filter((c) => c.has_secret).length);
async function loadVault() {
  try {
    vaultSealed.value = !!(await api.vaultStatus()).sealed;
  } catch (e) {
    vaultSealed.value = false;
  }
}

// ---- 集群 ----

async function loadClusters(probe = true) {
  loading.clusters = true;
  try {
    clusters.value = await api.nacosClusters();
    if (probe) probeAll();
  } catch (e) {
    message.error(e?.response?.data?.error || '加载集群失败(需 admin)');
  } finally {
    loading.clusters = false;
  }
}

function probeAll() {
  clusters.value.forEach((c) => probeOne(c.id));
}

async function probeOne(id) {
  health[id] = { ...(health[id] || {}), loading: true };
  try {
    health[id] = { ...(await api.nacosNodes(id)), loading: false };
  } catch (e) {
    health[id] = {
      loading: false,
      ok: false,
      nodes: [],
      source: 'error',
      message: e?.response?.data?.error || '检测失败',
    };
  }
}

const summary = computed(() => {
  const total = clusters.value.length;
  let up = 0;
  let down = 0;
  let pending = 0;
  for (const c of clusters.value) {
    const h0 = health[c.id];
    if (!h0 || h0.loading) pending += 1;
    else if (h0.ok) up += 1;
    else down += 1;
  }
  return { total, up, down, pending };
});

const nodeSummary = (id) => {
  const h0 = health[id];
  if (!h0 || h0.loading) return { text: '检测中…', tone: 'muted' };
  const nodes = h0.nodes || [];
  if (!nodes.length) return { text: h0.message || '无节点信息', tone: 'danger' };
  const up = nodes.filter((n) => n.ok).length;
  // 降级探活时只能证明地址可达,不能声称是集群成员状态
  const noun = h0.source === 'v2' || h0.source === 'v1' ? '节点在线' : '地址可达';
  return {
    text: `${up}/${nodes.length} ${noun}`,
    tone: up === nodes.length ? 'ok' : up ? 'warn' : 'danger',
  };
};

const clusterVersion = (id) =>
  (health[id]?.nodes || []).map((n) => n.version).find((v) => v) || '';

// ---- 集群表单 ----

const form = reactive({
  show: false,
  saving: false,
  probing: false,
  editingId: null,
  probe: null,
  errors: {},
  model: blankCluster(),
});

function blankCluster() {
  return {
    name: '',
    env: 'test',
    server_addr: '',
    context_path: '/nacos',
    namespace: '',
    username: '',
    password: '',
    status: 'enabled',
    note: '',
  };
}

const envOpts = [
  { label: '开发 dev', value: 'dev' },
  { label: '测试 test', value: 'test' },
  { label: '生产 prod', value: 'prod' },
  { label: '未分类', value: '' },
];

function openNew() {
  form.editingId = null;
  form.model = blankCluster();
  form.errors = {};
  form.probe = null;
  form.show = true;
}

function openEdit(c) {
  form.editingId = c.id;
  form.model = {
    name: c.name,
    env: c.env || '',
    server_addr: c.server_addr,
    context_path: c.context_path || '/nacos',
    namespace: c.namespace || '',
    username: c.username || '',
    password: '',
    status: c.status || 'enabled',
    note: c.note || '',
  };
  form.errors = {};
  form.probe = null;
  form.show = true;
}

function validate() {
  const e = {};
  if (!form.model.name.trim()) e.name = '请填写集群名称';
  if (!form.model.server_addr.trim()) e.server_addr = '请填写至少一个地址,如 10.0.0.1:8848';
  form.errors = e;
  return !Object.keys(e).length;
}

async function saveCluster() {
  if (!validate()) return;
  form.saving = true;
  try {
    if (form.editingId) await api.updateNacosCluster(form.editingId, form.model);
    else await api.createNacosCluster(form.model);
    message.success(form.editingId ? '集群已更新' : '集群已登记');
    form.show = false;
    await loadClusters();
  } catch (e) {
    message.error(e?.response?.data?.error || '保存失败');
  } finally {
    form.saving = false;
  }
}

async function probeForm() {
  if (!form.model.server_addr.trim()) {
    form.errors = { ...form.errors, server_addr: '请先填写地址' };
    return;
  }
  form.probing = true;
  form.probe = null;
  try {
    form.probe = await api.nacosProbe({
      server_addr: form.model.server_addr,
      context_path: form.model.context_path,
      namespace: form.model.namespace,
      username: form.model.username,
      password: form.model.password,
    });
  } catch (e) {
    form.probe = { ok: false, nodes: [], message: e?.response?.data?.error || '检测失败' };
  } finally {
    form.probing = false;
  }
}

async function removeCluster(c) {
  try {
    await api.deleteNacosCluster(c.id);
    message.success(`已删除集群「${c.name}」`);
    await loadClusters(false);
  } catch (e) {
    message.error(e?.response?.data?.error || '删除失败');
  }
}

// ---- 节点抽屉 ----

const nodesDrawer = reactive({ show: false, cluster: null });
function openNodes(c) {
  nodesDrawer.cluster = c;
  nodesDrawer.show = true;
  if (!health[c.id] || health[c.id].source === 'error') probeOne(c.id);
}

const nodeColumns = [
  {
    title: '状态',
    key: 'state',
    width: 118,
    render: (r) => statePill(r.state, r.ok),
  },
  { title: '地址', key: 'address', render: (r) => h('span', { class: 'mono' }, r.address) },
  {
    title: '版本',
    key: 'version',
    width: 100,
    render: (r) => h('span', { class: 'mono' }, r.version || '—'),
  },
  {
    title: '延迟',
    key: 'latency_ms',
    width: 90,
    render: (r) => h('span', { class: 'num' }, r.latency_ms ? `${r.latency_ms} ms` : '—'),
  },
  { title: '说明', key: 'message', render: (r) => r.message || '—' },
];

function statePill(state, ok) {
  const up = ok || String(state).toUpperCase() === 'UP';
  const tone = up ? 'ok' : String(state).toUpperCase() === 'SUSPICIOUS' ? 'warn' : 'danger';
  return h('span', { class: `pill pill-${tone}` }, [
    h(Icon, { name: up ? 'check' : tone === 'warn' ? 'alert' : 'close', size: 13 }),
    h('span', state || 'UNKNOWN'),
  ]);
}

// ---- 集群管理入口 ----

/// 进集群管理页:命名空间 / 配置(含同步)/ 账号 / 角色绑定 / 权限 都在那儿。
/// 卡片上原来的「已有配置」抽屉是只读列表,已被管理页的「配置」Tab 完全覆盖
/// (多了正文预览、删除、同步为模板),所以直接换成入口,不留两套。
function openManage(c) {
  router.push(`/nacos/${c.id}`);
}

// ---- 初始化抽屉 ----

const VAR_RE = /\$\{([A-Za-z0-9_.-]+)\}/g;

const init = reactive({
  show: false,
  cluster: null,
  mode: 'template', // template | custom
  templateId: null,
  items: [],
  vars: {},
  namespace: '',
  /// 默认收起:目标空间已由模板/集群决定,只有要发到别处才展开
  nsOverride: false,
  overwrite: false,
  running: false,
  result: null,
});

function openInit(c) {
  init.cluster = c;
  init.mode = templates.value.length ? 'template' : 'custom';
  init.templateId = templates.value[0]?.id || null;
  init.items = [];
  init.vars = {};
  init.namespace = '';
  init.nsOverride = false;
  init.overwrite = false;
  init.result = null;
  init.show = true;
}

/// 目标命名空间的解析顺序,和后端保持一致:显式覆盖 > 模板归属 > 集群默认。
/// UI 只是把这条规则显式说出来,免得人猜配置会落到哪儿。
const selectedTpl = computed(() =>
  init.mode === 'template' ? templates.value.find((t) => t.id === init.templateId) : null
);
const effectiveNs = computed(() => {
  if (init.namespace.trim()) return init.namespace.trim();
  if (selectedTpl.value?.namespace) return selectedTpl.value.namespace;
  return init.cluster?.namespace || '';
});
const nsOrigin = computed(() => {
  if (init.namespace.trim()) return '· 本次指定';
  if (selectedTpl.value?.namespace) return '· 跟随模板归属';
  return '· 集群默认';
});

const templateOpts = computed(() =>
  templates.value.map((t) => ({ label: `${t.name}(${itemsOf(t).length} 项)`, value: t.id }))
);

function itemsOf(t) {
  try {
    return JSON.parse(t.items || '[]');
  } catch (e) {
    return [];
  }
}

const initItems = computed(() => {
  if (init.mode === 'custom') return init.items;
  const t = templates.value.find((x) => x.id === init.templateId);
  return t ? itemsOf(t) : [];
});

/// 同步回来的模板是「原文下发」:里面的 ${...} 是应用自己的占位符(Spring 之类),
/// 不能当成 opsctl 的模板变量去要人填,否则整批回放全部失败。
const initLiteral = computed(() => {
  if (init.mode === 'custom') return false;
  const t = templates.value.find((x) => x.id === init.templateId);
  return !!(t && t.literal);
});

const initVarNames = computed(() => {
  if (initLiteral.value) return [];
  const found = new Set();
  for (const it of initItems.value) {
    for (const field of [it.data_id, it.group, it.content]) {
      const s = String(field || '');
      let m;
      VAR_RE.lastIndex = 0;
      while ((m = VAR_RE.exec(s))) found.add(m[1]);
    }
  }
  return [...found].sort();
});

const initReady = computed(() => initItems.value.length > 0);

async function runInit(dryRun) {
  if (!initReady.value) {
    message.warning('请先选择模板或添加配置项');
    return;
  }
  const go = async () => {
    init.running = true;
    try {
      const body = {
        template_id: init.mode === 'template' ? init.templateId : null,
        items: init.mode === 'custom' ? init.items : [],
        vars: init.vars,
        namespace: init.namespace || null,
        overwrite: init.overwrite,
        dry_run: dryRun,
      };
      init.result = await api.nacosInit(init.cluster.id, body);
      const r = init.result;
      const tone = r.status === 'ok' ? 'success' : r.status === 'partial' ? 'warning' : 'error';
      message[tone](
        `${dryRun ? '试运行' : '初始化'}完成:${r.ok_count}/${r.total} 项${
          r.status === 'ok' ? '成功' : r.status === 'partial' ? '部分成功' : '失败'
        }`
      );
      if (!dryRun) {
        await loadClusters(false);
        await loadRuns();
      }
    } catch (e) {
      message.error(e?.response?.data?.error || '初始化失败');
    } finally {
      init.running = false;
    }
  };
  if (!dryRun && init.overwrite) {
    dialog.warning({
      title: '确认覆盖已有配置',
      content: `将向「${init.cluster.name}」写入 ${initItems.value.length} 项配置,已存在的 dataId 会被覆盖。此操作会记入审计。`,
      positiveText: '确认覆盖',
      negativeText: '取消',
      onPositiveClick: go,
    });
  } else {
    go();
  }
}

const ITEM_STATUS = {
  created: { text: '新建', tone: 'ok', icon: 'plus' },
  updated: { text: '覆盖更新', tone: 'warn', icon: 'refresh' },
  skipped: { text: '跳过', tone: 'muted', icon: 'skip' },
  fail: { text: '失败', tone: 'danger', icon: 'close' },
  would_created: { text: '将新建', tone: 'ok', icon: 'plus' },
  would_updated: { text: '将更新', tone: 'warn', icon: 'refresh' },
  would_skipped: { text: '将跳过', tone: 'muted', icon: 'skip' },
};

function itemStatusPill(status) {
  const s = ITEM_STATUS[status] || { text: status, tone: 'muted', icon: 'dot' };
  return h('span', { class: `pill pill-${s.tone}` }, [
    h(Icon, { name: s.icon, size: 13 }),
    h('span', s.text),
  ]);
}

const resultColumns = [
  { title: '结果', key: 'status', width: 118, render: (r) => itemStatusPill(r.status) },
  { title: 'dataId', key: 'data_id', render: (r) => h('span', { class: 'mono' }, r.data_id) },
  { title: 'group', key: 'group', width: 150, render: (r) => h('span', { class: 'mono' }, r.group) },
  { title: '说明', key: 'message', render: (r) => r.message || '—' },
];

// ---- 配置模板 ----

async function loadTemplates() {
  loading.templates = true;
  try {
    templates.value = await api.nacosTemplates();
  } catch (e) {
    /* 与集群同权限,错误已在集群加载处提示 */
  } finally {
    loading.templates = false;
  }
}

const tplForm = reactive({
  show: false, saving: false, id: null, name: '', note: '', namespace: '', literal: false, items: [],
});

function openTplNew() {
  tplForm.id = null;
  tplForm.name = '';
  tplForm.note = '';
  tplForm.namespace = '';
  tplForm.literal = false;
  tplForm.items = [{ data_id: '', group: 'DEFAULT_GROUP', type: 'properties', content: '' }];
  tplForm.show = true;
}
function openTplEdit(t) {
  tplForm.id = t.id;
  tplForm.name = t.name;
  tplForm.note = t.note || '';
  tplForm.namespace = t.namespace || '';
  tplForm.literal = !!t.literal;
  tplForm.items = itemsOf(t);
  tplForm.show = true;
}
async function saveTpl() {
  if (!tplForm.name.trim()) {
    message.warning('请填写模板名称');
    return;
  }
  tplForm.saving = true;
  try {
    await api.saveNacosTemplate({
      id: tplForm.id,
      name: tplForm.name,
      note: tplForm.note,
      namespace: tplForm.namespace,
      literal: tplForm.literal,
      items: tplForm.items,
    });
    message.success('模板已保存');
    tplForm.show = false;
    await loadTemplates();
  } catch (e) {
    message.error(e?.response?.data?.error || '保存失败');
  } finally {
    tplForm.saving = false;
  }
}
async function removeTpl(t) {
  try {
    await api.deleteNacosTemplate(t.id);
    message.success('已删除');
    await loadTemplates();
  } catch (e) {
    message.error(e?.response?.data?.error || '删除失败');
  }
}

/// 模板按「归属命名空间」分组展示 —— Nacos 是按命名空间硬隔离的,
/// 一份配置集合脱离命名空间就说不清该发到哪。同步产生的模板自带来源命名空间。
const tplGroups = computed(() => {
  const by = new Map();
  for (const t of templates.value) {
    const key = t.namespace || '';
    if (!by.has(key)) by.set(key, []);
    by.get(key).push(t);
  }
  return [...by.entries()]
    .sort((a, b) => (a[0] === '' ? -1 : b[0] === '' ? 1 : a[0].localeCompare(b[0])))
    .map(([ns, list]) => ({ ns, label: ns || '未指定命名空间', list }));
});

const tplColumns = [
  { title: '模板名称', key: 'name' },
  {
    title: '配置项',
    key: 'items',
    width: 90,
    render: (t) => h('span', { class: 'num' }, itemsOf(t).length),
  },
  {
    title: '下发方式',
    key: 'literal',
    width: 110,
    render: (t) =>
      h('span', { class: `pill ${t.literal ? 'pill-ok' : 'pill-muted'}` }, t.literal ? '原文' : '变量代入'),
  },
  { title: '备注', key: 'note', render: (t) => t.note || '—' },
  { title: '创建时间', key: 'created_at', width: 170, render: (t) => fmtTime(t.created_at) },
  {
    title: '操作',
    key: 'ops',
    width: 140,
    render: (t) =>
      h('div', { class: 'row-ops' }, [
        h(NButton, { size: 'tiny', onClick: () => openTplEdit(t) }, { default: () => '编辑' }),
        h(
          NPopconfirm,
          { onPositiveClick: () => removeTpl(t), positiveText: '删除', negativeText: '取消' },
          {
            trigger: () =>
              h(NButton, { size: 'tiny', type: 'error', tertiary: true }, { default: () => '删除' }),
            default: () => `确定删除模板「${t.name}」?`,
          }
        ),
      ]),
  },
];

// ---- 初始化记录 ----

const runDrawer = reactive({ show: false, run: null });

async function loadRuns() {
  loading.runs = true;
  try {
    runs.value = await api.nacosRuns({ limit: 200 });
  } catch (e) {
    /* 同上 */
  } finally {
    loading.runs = false;
  }
}

const RUN_STATUS = {
  ok: { text: '成功', tone: 'ok', icon: 'check' },
  partial: { text: '部分成功', tone: 'warn', icon: 'alert' },
  fail: { text: '失败', tone: 'danger', icon: 'close' },
};

const runColumns = [
  { title: '时间', key: 'ts', width: 180, render: (r) => fmtTime(r.ts) },
  { title: '集群', key: 'cluster_name' },
  {
    title: '模板',
    key: 'template_name',
    render: (r) => r.template_name || '即席配置项',
  },
  {
    title: '命名空间',
    key: 'namespace',
    width: 130,
    render: (r) => h('span', { class: 'mono' }, r.namespace || 'public'),
  },
  { title: '操作人', key: 'operator_email', width: 170 },
  {
    title: '结果',
    key: 'status',
    width: 190,
    render: (r) => {
      const s = RUN_STATUS[r.status] || RUN_STATUS.fail;
      return h('div', { class: 'run-res' }, [
        h('span', { class: `pill pill-${s.tone}` }, [
          h(Icon, { name: s.icon, size: 13 }),
          h('span', s.text),
        ]),
        h('span', { class: 'num dim' }, `${r.ok_count}/${r.total}`),
        r.dry_run ? h('span', { class: 'pill pill-muted' }, '试运行') : null,
      ]);
    },
  },
  {
    title: '',
    key: 'ops',
    width: 80,
    render: (r) =>
      h(
        NButton,
        { size: 'tiny', quaternary: true, onClick: () => ((runDrawer.run = r), (runDrawer.show = true)) },
        { default: () => '详情' }
      ),
  },
];

onMounted(async () => {
  await Promise.all([loadClusters(), loadTemplates(), loadRuns(), loadVault()]);
});
</script>

<template>
  <div class="nacos">
    <!-- 概览条:先给整体健康度,再进入明细 -->
    <header class="page-hd">
      <div class="ttl">
        <Icon name="server" :size="18" />
        <h2>Nacos 管理</h2>
        <span class="sub">集群总览 · 配置初始化</span>
      </div>
      <div class="stats" role="group" aria-label="集群健康概览">
        <span class="stat"><b class="num">{{ summary.total }}</b> 集群</span>
        <span class="stat ok"><Icon name="check" :size="13" /><b class="num">{{ summary.up }}</b> 在线</span>
        <span class="stat danger"><Icon name="close" :size="13" /><b class="num">{{ summary.down }}</b> 异常</span>
        <span v-if="summary.pending" class="stat muted"><b class="num">{{ summary.pending }}</b> 检测中</span>
      </div>
      <div class="acts">
        <n-button size="small" :loading="loading.clusters" @click="loadClusters()">
          <Icon name="refresh" :size="15" style="margin-right:6px" /> 刷新
        </n-button>
        <n-button size="small" type="primary" @click="openNew">
          <Icon name="plus" :size="15" style="margin-right:6px" /> 登记集群
        </n-button>
      </div>
    </header>

    <!-- 金库封存:先给状态和恢复路径,别让用户点到一半才吃错误 -->
    <div v-if="vaultSealed" class="sealed">
      <Icon name="alert" :size="15" />
      <div class="sealed-txt">
        <b>凭据金库已封存</b>
        <span>
          带口令的集群无法登记<template v-if="credentialedCount">,已登记的
          {{ credentialedCount }} 个带鉴权集群也取不到凭据(查看节点 / 已有配置 / 初始化均会失败)</template>。无鉴权的集群不受影响。
        </span>
      </div>
      <n-button size="small" type="warning" ghost @click="router.push('/settings#vault')">
        前往解封
      </n-button>
    </div>

    <n-tabs v-model:value="tab" type="line" animated>
      <!-- ============ 集群总览 ============ -->
      <n-tab-pane name="clusters" tab="集群总览">
        <div v-if="!clusters.length && !loading.clusters" class="empty-page">
          <Icon name="server" :size="30" />
          <h3>还没有登记任何 Nacos 集群</h3>
          <p>登记后即可在这里看到所有集群的节点状态,并一键下发初始化配置。</p>
          <n-button type="primary" @click="openNew">
            <Icon name="plus" :size="15" style="margin-right:6px" /> 登记第一个集群
          </n-button>
        </div>

        <div v-else class="grid">
          <article v-for="c in clusters" :key="c.id" class="card" :class="{ off: c.status === 'disabled' }">
            <div class="card-hd">
              <h3 :title="`${c.name} · 点击进入集群管理`">
                <button class="name-link" type="button" @click="openManage(c)">{{ c.name }}</button>
              </h3>
              <span v-if="c.env" class="pill" :class="`env-${c.env}`">{{ c.env }}</span>
              <span v-if="c.status === 'disabled'" class="pill pill-muted">已停用</span>
              <span v-if="vaultSealed && c.has_secret" class="pill pill-warn" title="集群口令在金库里,封存态取不到">
                <Icon name="alert" :size="12" /> 需解封
              </span>
              <div class="hd-ops">
                <n-button size="tiny" quaternary :aria-label="`编辑 ${c.name}`" @click="openEdit(c)">
                  <Icon name="edit" :size="14" />
                </n-button>
                <n-popconfirm positive-text="删除" negative-text="取消" @positive-click="removeCluster(c)">
                  <template #trigger>
                    <n-button size="tiny" quaternary type="error" :aria-label="`删除 ${c.name}`">
                      <Icon name="trash" :size="14" />
                    </n-button>
                  </template>
                  删除集群「{{ c.name }}」及其初始化记录?远端配置不受影响。
                </n-popconfirm>
              </div>
            </div>

            <ul class="meta">
              <li v-for="ep in c.endpoints" :key="ep" class="mono ellip" :title="ep">{{ ep }}</li>
            </ul>

            <div class="kv">
              <span>命名空间 <b class="mono">{{ c.namespace || 'public' }}</b></span>
              <span v-if="c.username">鉴权 <b class="mono">{{ c.username }}</b></span>
              <span v-else class="dim">未启用鉴权</span>
            </div>

            <!-- 健康条:图标 + 文案 + 颜色三重编码,不只靠颜色 -->
            <div class="health" :class="`h-${nodeSummary(c.id).tone}`">
              <Icon
                :name="health[c.id]?.loading ? 'clock' : nodeSummary(c.id).tone === 'ok' ? 'check' : nodeSummary(c.id).tone === 'warn' ? 'alert' : 'close'"
                :size="14"
              />
              <span>{{ nodeSummary(c.id).text }}</span>
              <span v-if="clusterVersion(c.id)" class="mono dim">v{{ clusterVersion(c.id) }}</span>
              <span v-if="health[c.id]?.latency_ms" class="num dim">{{ health[c.id].latency_ms }} ms</span>
              <button class="link" type="button" @click="probeOne(c.id)">重测</button>
            </div>
            <p v-if="health[c.id]?.message" class="hint">{{ health[c.id].message }}</p>

            <div class="last">
              <template v-if="c.last_init">
                <Icon name="clock" :size="13" />
                最近初始化 {{ fmtTime(c.last_init.ts) }} ·
                <b :class="c.last_init.status === 'ok' ? 'ok-txt' : c.last_init.status === 'partial' ? 'warn-txt' : 'danger-txt'">
                  {{ c.last_init.ok_count }}/{{ c.last_init.total }}
                </b>
                <span v-if="c.last_init.template_name" class="dim">· {{ c.last_init.template_name }}</span>
              </template>
              <span v-else class="dim">尚未初始化过配置</span>
            </div>

            <div class="card-ft">
              <n-button size="small" type="primary" :disabled="c.status === 'disabled'" @click="openInit(c)">
                <Icon name="play" :size="14" style="margin-right:6px" /> 初始化配置
              </n-button>
              <!-- 命名空间 / 账号 / 角色 / 权限 / 配置 都在集群管理页,这里是唯一入口 -->
              <n-button size="small" secondary @click="openManage(c)">
                <Icon name="list" :size="14" style="margin-right:6px" /> 集群管理
              </n-button>
              <n-button size="small" quaternary @click="openNodes(c)">
                <Icon name="server" :size="14" style="margin-right:6px" /> 节点
              </n-button>
            </div>
            <p class="manage-hint">
              集群管理:命名空间 · 配置(含同步) · 账号 · 角色绑定 · 权限
            </p>
          </article>
        </div>
      </n-tab-pane>

      <!-- ============ 配置模板 ============ -->
      <n-tab-pane name="templates" tab="配置模板">
        <n-card size="small" :bordered="false">
          <template #header>
            <span class="card-title">初始化模板</span>
            <span class="card-sub">按归属命名空间分组;可一键回放到任意集群</span>
          </template>
          <template #header-extra>
            <n-button size="small" type="primary" @click="openTplNew">
              <Icon name="plus" :size="15" style="margin-right:6px" /> 新建模板
            </n-button>
          </template>
          <div v-if="!templates.length && !loading.templates" class="empty-page sm">
            <Icon name="file" :size="26" />
            <h3>还没有配置模板</h3>
            <p>去集群管理页的「命名空间与配置」把远端配置同步下来,或在这里手写一份。</p>
            <n-button type="primary" size="small" @click="openTplNew">新建模板</n-button>
          </div>
          <div v-else class="tpl-groups">
            <section v-for="g in tplGroups" :key="g.ns" class="tpl-group">
              <h4>
                <Icon name="database" :size="13" />
                <span class="mono">{{ g.ns || 'public / 未指定' }}</span>
                <span class="dim num">{{ g.list.length }} 个模板</span>
              </h4>
              <n-data-table
                :columns="tplColumns"
                :data="g.list"
                :loading="loading.templates"
                :bordered="false"
                :scroll-x="760"
                size="small"
              />
            </section>
          </div>
        </n-card>
      </n-tab-pane>

      <!-- ============ 初始化记录 ============ -->
      <n-tab-pane name="runs" tab="初始化记录">
        <n-card size="small" :bordered="false">
          <template #header>
            <span class="card-title">初始化记录</span>
            <span class="card-sub">每次下发逐条留痕,同步写入审计</span>
          </template>
          <template #header-extra>
            <n-button size="small" :loading="loading.runs" @click="loadRuns">
              <Icon name="refresh" :size="15" style="margin-right:6px" /> 刷新
            </n-button>
          </template>
          <div v-if="!runs.length && !loading.runs" class="empty-page sm">
            <Icon name="clock" :size="26" />
            <h3>还没有初始化记录</h3>
            <p>在「集群总览」里对某个集群执行初始化后,这里会留下逐条结果。</p>
          </div>
          <n-data-table
            v-else
            :columns="runColumns"
            :data="runs"
            :loading="loading.runs"
            :bordered="false"
            :scroll-x="980"
            size="small"
          />
        </n-card>
      </n-tab-pane>
    </n-tabs>

    <!-- ============ 集群表单 ============ -->
    <n-modal
      v-model:show="form.show"
      preset="card"
      :title="form.editingId ? '编辑 Nacos 集群' : '登记 Nacos 集群'"
      style="width:600px;max-width:94vw"
    >
      <n-form label-placement="top" :show-feedback="false">
        <div class="f-grid">
          <n-form-item label="集群名称" required>
            <n-input v-model:value="form.model.name" placeholder="如:订单中心 Nacos" />
            <p v-if="form.errors.name" class="err">{{ form.errors.name }}</p>
          </n-form-item>
          <n-form-item label="环境">
            <n-select v-model:value="form.model.env" :options="envOpts" />
          </n-form-item>
        </div>

        <n-form-item label="服务地址" required>
          <n-input
            v-model:value="form.model.server_addr"
            class="mono"
            type="textarea"
            :autosize="{ minRows: 2, maxRows: 4 }"
            placeholder="10.0.0.1:8848,10.0.0.2:8848(逗号分隔,省略端口按 8848)"
          />
          <p v-if="form.errors.server_addr" class="err">{{ form.errors.server_addr }}</p>
          <p v-else class="help">支持 host、host:port 或完整 URL;仅 http,暂不支持 IPv6 字面量。</p>
        </n-form-item>

        <div class="f-grid">
          <n-form-item label="上下文路径">
            <n-input v-model:value="form.model.context_path" class="mono" placeholder="/nacos" />
          </n-form-item>
          <n-form-item label="命名空间 ID">
            <n-input v-model:value="form.model.namespace" class="mono" placeholder="留空 = public" />
          </n-form-item>
        </div>

        <!-- 这是「远端 Nacos 的账号」,不是本平台登录账号:必须挡掉浏览器把
             opsctl 的登录口令自动灌进来(灌进来会被当成集群口令加密入库)。 -->
        <div class="f-grid">
          <n-form-item label="用户名">
            <n-input
              v-model:value="form.model.username"
              placeholder="未开鉴权可留空"
              :input-props="{ name: 'nacos-cluster-user', autocomplete: 'off' }"
            />
          </n-form-item>
          <n-form-item label="密码">
            <n-input
              v-model:value="form.model.password"
              type="password"
              show-password-on="click"
              :input-props="{ name: 'nacos-cluster-secret', autocomplete: 'new-password' }"
              :placeholder="form.editingId ? '留空 = 保持原密码' : '存入金库加密'"
            />
          </n-form-item>
        </div>

        <n-form-item label="备注">
          <n-input v-model:value="form.model.note" placeholder="用途、负责人等" />
        </n-form-item>

        <n-form-item label="启用">
          <n-switch
            :value="form.model.status === 'enabled'"
            @update:value="(v) => (form.model.status = v ? 'enabled' : 'disabled')"
          />
          <span class="help" style="margin-left:10px">停用后不可执行初始化</span>
        </n-form-item>

        <div v-if="form.probe" class="probe" :class="form.probe.ok ? 'p-ok' : 'p-bad'">
          <Icon :name="form.probe.ok ? 'check' : 'close'" :size="14" />
          <span>
            {{ form.probe.ok ? '连通正常' : '无法连通' }}
            <template v-if="form.probe.nodes?.length">
              · {{ form.probe.nodes.filter((n) => n.ok).length }}/{{ form.probe.nodes.length }} 节点在线
            </template>
            <template v-if="form.probe.latency_ms"> · {{ form.probe.latency_ms }} ms</template>
          </span>
          <span v-if="form.probe.message" class="dim">{{ form.probe.message }}</span>
        </div>
      </n-form>

      <template #footer>
        <div class="modal-ft">
          <n-button size="small" :loading="form.probing" @click="probeForm">
            <Icon name="eye" :size="15" style="margin-right:6px" /> 测试连通
          </n-button>
          <span class="sp" />
          <n-button size="small" @click="form.show = false">取消</n-button>
          <n-button size="small" type="primary" :loading="form.saving" @click="saveCluster">保存</n-button>
        </div>
      </template>
    </n-modal>

    <!-- ============ 节点抽屉 ============ -->
    <n-drawer v-model:show="nodesDrawer.show" :width="620" placement="right">
      <n-drawer-content :title="`${nodesDrawer.cluster?.name || ''} · 集群节点`" closable>
        <div class="drawer-bar">
          <span class="src">
            数据来源:{{
              { v2: 'Nacos v2 集群接口', v1: 'Nacos v1 集群接口', probe: '地址探活(降级)', error: '检测失败' }[
                health[nodesDrawer.cluster?.id]?.source
              ] || '—'
            }}
          </span>
          <n-button
            size="tiny"
            :loading="health[nodesDrawer.cluster?.id]?.loading"
            @click="probeOne(nodesDrawer.cluster.id)"
          >
            <Icon name="refresh" :size="13" style="margin-right:5px" /> 重新检测
          </n-button>
        </div>
        <n-alert
          v-if="health[nodesDrawer.cluster?.id]?.message"
          type="warning"
          :bordered="false"
          style="margin-bottom:12px"
        >
          {{ health[nodesDrawer.cluster?.id].message }}
        </n-alert>
        <n-data-table
          :columns="nodeColumns"
          :data="health[nodesDrawer.cluster?.id]?.nodes || []"
          :loading="health[nodesDrawer.cluster?.id]?.loading"
          :bordered="false"
          :scroll-x="620"
          size="small"
        />
      </n-drawer-content>
    </n-drawer>

    <!-- 已有配置抽屉已移除:改由集群管理页的「配置」Tab 承载(带正文预览/删除/同步) -->

    <!-- ============ 初始化抽屉 ============ -->
    <n-drawer v-model:show="init.show" :width="760" placement="right">
      <n-drawer-content :title="`初始化配置 · ${init.cluster?.name || ''}`" closable>
        <!-- 1. 来源 -->
        <section class="step">
          <h4><span class="idx">1</span> 配置来源</h4>
          <n-radio-group v-model:value="init.mode" size="small">
            <n-radio-button value="template" :disabled="!templates.length">使用模板</n-radio-button>
            <n-radio-button value="custom">即席配置项</n-radio-button>
          </n-radio-group>
          <n-select
            v-if="init.mode === 'template'"
            v-model:value="init.templateId"
            :options="templateOpts"
            placeholder="选择配置模板"
            size="small"
            style="margin-top:10px"
          />
          <NacosConfigItems v-else v-model="init.items" style="margin-top:10px" />
          <ul v-if="init.mode === 'template' && initItems.length" class="preview mono">
            <li v-for="(it, i) in initItems" :key="i">
              {{ it.data_id }} <span class="dim">· {{ it.group }} · {{ it.type }}</span>
            </li>
          </ul>
          <p v-if="initLiteral" class="literal-note">
            <Icon name="check" :size="13" />
            该模板由「同步」生成,按<b>原文</b>下发 —— 配置里的
            <code>${...}</code> 是应用自己的占位符,不做变量代入。
          </p>
        </section>

        <!-- 2. 变量(仅在模板含占位符时出现:渐进式披露) -->
        <section v-if="initVarNames.length" class="step">
          <h4><span class="idx">2</span> 变量取值</h4>
          <div class="f-grid">
            <n-form-item v-for="name in initVarNames" :key="name" :label="`\${${name}}`" label-placement="top" :show-feedback="false">
              <n-input
                :value="init.vars[name] || ''"
                size="small"
                class="mono"
                :placeholder="`填写 ${name}`"
                @update:value="(v) => (init.vars[name] = v)"
              />
            </n-form-item>
          </div>
          <p class="help">未填写的变量会让对应配置项直接失败,不会写入半成品内容。</p>
        </section>

        <!-- 3. 目标与策略 -->
        <section class="step">
          <h4><span class="idx">{{ initVarNames.length ? 3 : 2 }}</span> 目标与策略</h4>
          <!-- 简化:目标命名空间默认跟随模板归属(同步下来的模板自带来源空间),
               只有真要发到别处时才展开改 -->
          <div class="target-ns">
            <Icon name="database" :size="13" />
            <span>目标命名空间</span>
            <b class="mono">{{ effectiveNs || 'public' }}</b>
            <span class="dim">{{ nsOrigin }}</span>
            <button class="link" type="button" @click="init.nsOverride = !init.nsOverride">
              {{ init.nsOverride ? '收起' : '改到别的空间' }}
            </button>
          </div>
          <div v-if="init.nsOverride" class="f-grid" style="margin-top:10px">
            <n-form-item label="命名空间 ID" label-placement="top" :show-feedback="false">
              <n-input
                v-model:value="init.namespace"
                size="small"
                class="mono"
                :placeholder="init.cluster?.namespace || 'public(集群默认)'"
              />
            </n-form-item>
          </div>
          <div class="f-grid" style="margin-top:10px">
            <n-form-item label="已存在的 dataId" label-placement="top" :show-feedback="false">
              <n-switch v-model:value="init.overwrite" size="small" />
              <span class="help" style="margin-left:10px">
                {{ init.overwrite ? '覆盖为模板内容' : '保留远端现值(推荐)' }}
              </span>
            </n-form-item>
          </div>
          <n-alert v-if="init.overwrite" type="warning" :bordered="false" style="margin-top:10px">
            覆盖会改写线上正在生效的配置,建议先「试运行」确认影响范围。
          </n-alert>
        </section>

        <!-- 4. 结果 -->
        <section v-if="init.result" class="step">
          <h4><span class="idx">✓</span> {{ init.result.dry_run ? '试运行结果' : '执行结果' }}</h4>
          <div class="res-sum" :class="`h-${init.result.status === 'ok' ? 'ok' : init.result.status === 'partial' ? 'warn' : 'danger'}`">
            <Icon :name="init.result.status === 'ok' ? 'check' : init.result.status === 'partial' ? 'alert' : 'close'" :size="15" />
            <b class="num">{{ init.result.ok_count }}/{{ init.result.total }}</b>
            <span>
              {{ init.result.dry_run ? '项可执行' : '项完成' }} · 命名空间
              <b class="mono">{{ init.result.namespace || 'public' }}</b>
            </span>
            <span v-if="init.result.dry_run" class="pill pill-muted">未写入远端</span>
          </div>
          <n-data-table
            :columns="resultColumns"
            :data="init.result.items"
            :bordered="false"
            :scroll-x="620"
            size="small"
            style="margin-top:10px"
          />
        </section>

        <template #footer>
          <div class="modal-ft">
            <span class="help">
              共 <b class="num">{{ initItems.length }}</b> 项待下发
            </span>
            <span class="sp" />
            <n-button size="small" :loading="init.running" :disabled="!initReady" @click="runInit(true)">
              <Icon name="eye" :size="15" style="margin-right:6px" /> 试运行
            </n-button>
            <n-button
              size="small"
              type="primary"
              :loading="init.running"
              :disabled="!initReady"
              @click="runInit(false)"
            >
              <Icon name="play" :size="14" style="margin-right:6px" /> 执行初始化
            </n-button>
          </div>
        </template>
      </n-drawer-content>
    </n-drawer>

    <!-- ============ 模板编辑 ============ -->
    <n-modal
      v-model:show="tplForm.show"
      preset="card"
      :title="tplForm.id ? '编辑配置模板' : '新建配置模板'"
      style="width:820px;max-width:96vw"
    >
      <n-form label-placement="top" :show-feedback="false">
        <div class="f-grid">
          <n-form-item label="模板名称" required>
            <n-input v-model:value="tplForm.name" placeholder="如:微服务上线基线" />
          </n-form-item>
          <n-form-item label="归属命名空间">
            <n-input v-model:value="tplForm.namespace" class="mono" placeholder="留空 = public" />
          </n-form-item>
        </div>
        <n-form-item label="备注">
          <n-input v-model:value="tplForm.note" placeholder="适用范围" />
        </n-form-item>
        <n-form-item label="下发方式">
          <n-switch v-model:value="tplForm.literal" size="small" />
          <span class="help" style="margin-left:10px">
            {{ tplForm.literal ? '按原文下发,不做 ${} 变量代入(同步下来的真实配置用这个)' : '做 ${变量} 代入,下发前需填值' }}
          </span>
        </n-form-item>
        <n-form-item label="配置项">
          <NacosConfigItems v-model="tplForm.items" />
        </n-form-item>
      </n-form>
      <template #footer>
        <div class="modal-ft">
          <span class="sp" />
          <n-button size="small" @click="tplForm.show = false">取消</n-button>
          <n-button size="small" type="primary" :loading="tplForm.saving" @click="saveTpl">保存</n-button>
        </div>
      </template>
    </n-modal>

    <!-- ============ 记录详情 ============ -->
    <n-drawer v-model:show="runDrawer.show" :width="700" placement="right">
      <n-drawer-content title="初始化记录详情" closable>
        <div v-if="runDrawer.run" class="run-detail">
          <div class="kv-grid">
            <span>集群</span><b>{{ runDrawer.run.cluster_name }}</b>
            <span>命名空间</span><b class="mono">{{ runDrawer.run.namespace || 'public' }}</b>
            <span>模板</span><b>{{ runDrawer.run.template_name || '即席配置项' }}</b>
            <span>操作人</span><b>{{ runDrawer.run.operator_email }}</b>
            <span>时间</span><b>{{ fmtTime(runDrawer.run.ts) }}</b>
            <span>模式</span><b>{{ runDrawer.run.dry_run ? '试运行(未写入)' : '实际下发' }}</b>
          </div>
          <n-data-table
            :columns="resultColumns"
            :data="runDrawer.run.items || []"
            :bordered="false"
            :scroll-x="620"
            size="small"
            style="margin-top:14px"
          />
        </div>
      </n-drawer-content>
    </n-drawer>
  </div>
</template>

<style scoped>
.nacos { display: flex; flex-direction: column; gap: 14px; }

/* ---- 页头 ---- */
.page-hd { display: flex; align-items: center; gap: 16px; flex-wrap: wrap; }
.page-hd .ttl { display: flex; align-items: baseline; gap: 8px; color: var(--accent); }
.page-hd h2 { margin: 0; font-size: 18px; color: var(--fg); font-weight: 650; }
.page-hd .sub { font-size: 12px; color: var(--muted); }
.stats { display: flex; gap: 14px; align-items: center; font-size: 12px; color: var(--fg-2); }
.stat { display: inline-flex; align-items: center; gap: 5px; }
.stat.ok { color: var(--success); }
.stat.danger { color: var(--danger); }
.stat.muted { color: var(--muted); }
.acts { margin-left: auto; display: flex; gap: 8px; }

/* ---- 金库封存提示条 ---- */
.sealed { display: flex; align-items: center; gap: 12px; padding: 10px 14px; border-radius: 10px;
  color: var(--warn); background: color-mix(in oklab, var(--warn), transparent 90%);
  border: 1px solid color-mix(in oklab, var(--warn), transparent 72%); }
.sealed-txt { display: flex; flex-direction: column; gap: 2px; flex: 1; min-width: 0; }
.sealed-txt b { font-size: 13px; font-weight: 620; }
.sealed-txt span { font-size: 12px; color: var(--fg-2); }

/* ---- 集群卡片网格 ---- */
.grid { display: grid; gap: 14px; grid-template-columns: repeat(auto-fill, minmax(360px, 1fr)); }
.card { background: var(--surface); border: 1px solid rgba(255,255,255,.07); border-radius: 12px;
  padding: 14px; display: flex; flex-direction: column; gap: 10px;
  transition: border-color .18s ease, box-shadow .18s ease; }
.card:hover { border-color: color-mix(in oklab, var(--accent), transparent 55%);
  box-shadow: 0 6px 20px rgba(0,0,0,.28); }
.card.off { opacity: .68; }
.card-hd { display: flex; align-items: center; gap: 8px; }
.card-hd h3 { flex: 1; min-width: 0; margin: 0; font-size: 15px; font-weight: 620; color: var(--fg);
  overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.name-link { all: unset; cursor: pointer; display: block; max-width: 100%;
  overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  border-radius: 4px; transition: color .15s ease; }
.name-link:hover { color: var(--accent); text-decoration: underline; }
.name-link:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
.manage-hint { margin: -2px 0 0; font-size: 11.5px; color: var(--muted); }
.hd-ops { display: flex; gap: 2px; margin-left: 4px; }
.meta { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 2px; }
.meta li { font-size: 12px; color: var(--fg-2); }
.ellip { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.kv { display: flex; gap: 14px; flex-wrap: wrap; font-size: 12px; color: var(--muted); }
.kv b { color: var(--fg-2); font-weight: 500; }

.health { display: flex; align-items: center; gap: 8px; font-size: 12.5px;
  padding: 7px 10px; border-radius: 8px; background: var(--bg); }
.h-ok { color: var(--success); }
.h-warn { color: var(--warn); }
.h-danger { color: var(--danger); }
.h-muted { color: var(--muted); }
.health .link { margin-left: auto; background: none; border: 0; padding: 2px 4px; cursor: pointer;
  color: var(--accent); font-size: 12px; border-radius: 5px; }
.health .link:hover { text-decoration: underline; }
.hint { margin: -4px 0 0; font-size: 11.5px; color: var(--muted); }

.last { font-size: 12px; color: var(--fg-2); display: flex; align-items: center; gap: 5px; flex-wrap: wrap; }
.ok-txt { color: var(--success); }
.warn-txt { color: var(--warn); }
.danger-txt { color: var(--danger); }

.card-ft { display: flex; align-items: center; gap: 8px; margin-top: 2px; }
.sp { flex: 1; }

/* ---- 状态 pill:图标 + 文案 + 颜色 ---- */
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
.env-dev { color: var(--accent-2); background: rgba(255,255,255,.05); border-color: rgba(255,255,255,.12); }
.env-test { color: var(--warn); background: color-mix(in oklab, var(--warn), transparent 88%);
  border-color: color-mix(in oklab, var(--warn), transparent 70%); }
.env-prod { color: var(--danger); background: color-mix(in oklab, var(--danger), transparent 86%);
  border-color: color-mix(in oklab, var(--danger), transparent 62%); }

/* ---- 空状态 ---- */
.empty-page { padding: 56px 20px; text-align: center; color: var(--muted); }
.empty-page.sm { padding: 34px 16px; }
.empty-page h3 { margin: 10px 0 4px; font-size: 15px; color: var(--fg-2); font-weight: 600; }
.empty-page p { margin: 0 0 14px; font-size: 12.5px; }

/* ---- 卡片/表格通用 ---- */
.card-title { font-weight: 620; }
.card-sub { margin-left: 10px; font-size: 12px; color: var(--muted); font-weight: 400; }
:deep(.row-ops) { display: flex; gap: 8px; }
:deep(.run-res) { display: flex; align-items: center; gap: 8px; }

/* ---- 表单 ---- */
.f-grid { display: grid; gap: 12px; grid-template-columns: 1fr 1fr; }
.err { margin: 4px 0 0; font-size: 12px; color: var(--danger); }
.help { margin: 4px 0 0; font-size: 12px; color: var(--muted); }
.modal-ft { display: flex; align-items: center; gap: 8px; }
.probe { display: flex; align-items: center; gap: 8px; margin-top: 14px; padding: 9px 12px;
  border-radius: 8px; font-size: 12.5px; }
.p-ok { color: var(--success); background: color-mix(in oklab, var(--success), transparent 90%); }
.p-bad { color: var(--danger); background: color-mix(in oklab, var(--danger), transparent 90%); }

.tpl-groups { display: flex; flex-direction: column; gap: 18px; }
.tpl-group h4 { display: flex; align-items: center; gap: 8px; margin: 0 0 8px;
  font-size: 12.5px; font-weight: 600; color: var(--fg-2); }
.tpl-group h4 .dim { margin-left: auto; font-size: 11.5px; }
.target-ns { display: flex; align-items: center; gap: 8px; padding: 9px 12px;
  border-radius: 8px; background: var(--bg); font-size: 12.5px; color: var(--fg-2); }
.target-ns b { color: var(--fg); font-weight: 600; }
.target-ns .link { margin-left: auto; background: none; border: 0; padding: 2px 4px;
  cursor: pointer; color: var(--accent); font-size: 12px; border-radius: 5px; }
.target-ns .link:hover { text-decoration: underline; }
/* ---- 抽屉 ---- */
.drawer-bar { display: flex; align-items: center; gap: 10px; margin: 0 0 12px; }
.drawer-bar .src { font-size: 12px; color: var(--muted); }
.step { padding: 0 0 18px; }
.step h4 { display: flex; align-items: center; gap: 8px; margin: 0 0 10px;
  font-size: 13px; color: var(--fg); font-weight: 620; }
.step .idx { display: grid; place-items: center; width: 20px; height: 20px; border-radius: 6px;
  background: color-mix(in oklab, var(--accent), transparent 82%); color: var(--accent);
  font-size: 11px; font-weight: 700; }
.preview { list-style: none; margin: 10px 0 0; padding: 10px; border-radius: 8px;
  background: var(--bg); font-size: 12px; color: var(--fg-2); max-height: 168px; overflow: auto; }
.preview li { padding: 1px 0; }
.literal-note { display: flex; align-items: center; gap: 6px; margin: 8px 0 0;
  font-size: 12px; color: var(--success); }
.literal-note b { color: var(--fg); font-weight: 600; }
.literal-note code { font-family: ui-monospace, Consolas, monospace; color: var(--fg-2); }
.res-sum { display: flex; align-items: center; gap: 8px; padding: 9px 12px; border-radius: 8px;
  background: var(--bg); font-size: 12.5px; }
.kv-grid { display: grid; grid-template-columns: 92px 1fr 92px 1fr; gap: 8px 12px;
  font-size: 12.5px; align-items: baseline; }
.kv-grid span { color: var(--muted); }
.kv-grid b { color: var(--fg); font-weight: 550; }

/* ---- 排版细节 ---- */
.mono, :deep(.mono) { font-family: ui-monospace, SFMono-Regular, "JetBrains Mono", Consolas, monospace; }
.num, :deep(.num) { font-variant-numeric: tabular-nums; }
.dim, :deep(.dim) { color: var(--muted); }
.mono :deep(input), .mono :deep(textarea) {
  font-family: ui-monospace, SFMono-Regular, "JetBrains Mono", Consolas, monospace; font-size: 12.5px;
}

/* 键盘可达性:所有可点元素保留清晰焦点环 */
.card :deep(button:focus-visible), .health .link:focus-visible, .acts :deep(button:focus-visible) {
  outline: 2px solid var(--accent); outline-offset: 2px;
}

@media (max-width: 860px) {
  .f-grid { grid-template-columns: 1fr; }
  .kv-grid { grid-template-columns: 88px 1fr; }
  .acts { margin-left: 0; width: 100%; }
}

@media (prefers-reduced-motion: reduce) {
  .card { transition: none; }
}
</style>
