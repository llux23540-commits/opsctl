<script setup>
import { ref, reactive, onMounted, computed, h } from 'vue';
import { useRouter } from 'vue-router';
import { useMessage, useDialog } from 'naive-ui';
import { api } from '../api';
import { useAuth } from '../store/auth';

const router = useRouter();
const message = useMessage();
const dialog = useDialog();
const auth = useAuth();

const assets = ref([]);
const checkedKeys = ref([]);
const expandedKeys = ref([]);
const search = ref('');
const activeTags = ref([]);
const tags = ref([]);
const templates = ref([]);

const sshTemplates = computed(() => templates.value.filter((t) => t.kind === 'ssh').map((t) => ({ label: t.name, value: t.id })));
const sqlTemplates = computed(() => templates.value.filter((t) => t.kind === 'sql').map((t) => ({ label: t.name, value: t.id })));

// ---- template variable editing (live substitution, mirrors the prototype) ----
const sshTpl = ref(null);
const sqlTpl = ref(null);
const sshVars = reactive({});
const sqlVars = reactive({});

function tplVarDefs(id) {
  const t = templates.value.find((x) => x.id === id);
  if (!t) return [];
  try { return JSON.parse(t.variables || '[]'); } catch (e) { return []; }
}
const sshVarDefs = computed(() => tplVarDefs(sshTpl.value));
const sqlVarDefs = computed(() => tplVarDefs(sqlTpl.value));

// substitute {{var}} with its value; empty values keep the placeholder visible
function substitute(cmd, vars) {
  return cmd.replace(/\{\{\s*([\w.-]+)\s*\}\}/g, (m, name) =>
    vars[name] != null && vars[name] !== '' ? vars[name] : m);
}

function applyTemplate(id, target) {
  const t = templates.value.find((x) => x.id === id);
  const vars = target === 'ssh' ? sshVars : sqlVars;
  Object.keys(vars).forEach((k) => delete vars[k]);
  if (!t) return;
  tplVarDefs(id).forEach((v) => { vars[v.name] = v.default || ''; });
  const cmd = substitute(t.command, vars);
  if (target === 'ssh') sshCmd.value = cmd;
  else sqlCmd.value = cmd;
}

function onVarInput(target) {
  const id = target === 'ssh' ? sshTpl.value : sqlTpl.value;
  const t = templates.value.find((x) => x.id === id);
  if (!t) return;
  const cmd = substitute(t.command, target === 'ssh' ? sshVars : sqlVars);
  if (target === 'ssh') sshCmd.value = cmd;
  else sqlCmd.value = cmd;
}

// picking a quick command detaches from the template (prototype behavior)
function useQuick(cmd, target) {
  if (target === 'ssh') { sshTpl.value = null; applyTemplate(null, 'ssh'); sshCmd.value = cmd; }
  else { sqlTpl.value = null; applyTemplate(null, 'sql'); sqlCmd.value = cmd; }
}

// per-area command + state
const sshCmd = ref('uname -a');
const sqlCmd = ref('SELECT 1');
const sshRunning = ref(false);
const sqlRunning = ref(false);
const sshResults = ref([]);
const sqlResults = ref([]);

const SSH_QUICK = ['uname -a', 'df -h', 'systemctl status nginx', 'free -m'];
const SQL_QUICK = ['SELECT 1', 'SELECT * FROM servers', 'SELECT count(*) FROM servers'];
const DESTRUCTIVE = /\b(drop|delete|update|truncate|alter|rm|reboot|shutdown|mkfs|dd|restart)\b/i;

async function load() {
  try {
    [assets.value, tags.value, templates.value] = await Promise.all([
      api.assets(), api.tags().catch(() => []), api.templates().catch(() => []),
    ]);
    expandAll();
  } catch (e) {
    message.error('加载资产失败');
  }
}
onMounted(load);

// tag id -> {name,color}
const tagMap = computed(() => Object.fromEntries(tags.value.map((t) => [t.id, t])));
// site id -> name (search can hit the site name)
const siteNameById = computed(() =>
  Object.fromEntries(assets.value.filter((a) => a.kind === 'site').map((a) => [a.id, a.name.toLowerCase()])));

// assets after search + tag filter (sites always kept so the tree has structure)
const filtered = computed(() => {
  const kw = search.value.trim().toLowerCase();
  const tagSel = activeTags.value;
  const leafOk = (a) => {
    if (kw) {
      const hitName = a.name.toLowerCase().includes(kw);
      const hitHost = (a.host || '').toLowerCase().includes(kw);
      const hitSite = a.parent_id && (siteNameById.value[a.parent_id] || '').includes(kw);
      if (!hitName && !hitHost && !hitSite) return false;
    }
    if (tagSel.length && !(a.tag_ids || []).some((t) => tagSel.includes(t))) return false;
    return true;
  };
  const keptLeaves = assets.value.filter((a) => a.kind !== 'site' && leafOk(a));
  const keepSiteIds = new Set(keptLeaves.map((a) => a.parent_id).filter(Boolean));
  return assets.value.filter((a) => (a.kind === 'site' ? keepSiteIds.has(a.id) : leafOk(a)));
});

function buildTree(list) {
  const byId = {};
  list.forEach((a) => (byId[a.id] = { key: a.id, label: a.name, kind: a.kind, raw: a, children: [] }));
  const roots = [];
  list.forEach((a) => {
    const node = byId[a.id];
    if (a.parent_id && byId[a.parent_id]) byId[a.parent_id].children.push(node);
    else roots.push(node);
  });
  // empty sites stay disabled; sites with children are checkable (cascade selects the whole site)
  const mark = (n) => {
    if (n.kind === 'site' && !n.children.length) n.disabled = true;
    n.children.forEach(mark);
  };
  roots.forEach(mark);
  return roots;
}
const treeData = computed(() => buildTree(filtered.value));

// rich node row: icon-colored name + host/type meta + tag dots + disabled badge
function renderLabel({ option }) {
  const a = option.raw || {};
  if (option.kind === 'site') {
    return h('span', { class: 'tnode' }, [
      h('b', option.label),
      h('span', { class: 'tmeta' }, `${option.children?.length || 0} 个节点`),
    ]);
  }
  const isDb = a.kind === 'database';
  const meta = [a.host, isDb ? '数据库' : '服务器'].filter(Boolean).join(' · ');
  const dots = (a.tag_ids || [])
    .map((id) => tagMap.value[id])
    .filter(Boolean)
    .map((t) => h('span', { class: 'tdot', style: { background: t.color || 'var(--accent)' }, title: t.name }));
  return h('span', { class: 'tnode' }, [
    h('span', { class: isDb ? 'tname db' : 'tname' }, option.label),
    dots.length ? h('span', { class: 'tdots' }, dots) : null,
    meta ? h('span', { class: 'tmeta' }, meta) : null,
    a.status === 'disabled' ? h('span', { class: 'tbadge' }, '停用') : null,
    h('span', {
      class: 'tlog', title: '该节点执行记录',
      onClick: (e) => { e.stopPropagation(); openNodeHistory(a); },
    }, '记录'),
  ]);
}

// ---- single-node execution history drawer (prototype console.js openNodeHistory) ----
const histOpen = ref(false);
const histNode = ref(null);
const histRows = ref([]);
const histLoading = ref(false);
async function openNodeHistory(a) {
  histNode.value = a;
  histOpen.value = true;
  histLoading.value = true;
  try { histRows.value = await api.nodeHistory(a.id); }
  catch (e) { histRows.value = []; message.error('加载节点记录失败'); }
  finally { histLoading.value = false; }
}
const histStats = computed(() => {
  const rows = histRows.value;
  const ok = rows.filter((r) => r.status === 'ok').length;
  return { total: rows.length, ok, fail: rows.length - ok };
});
const fmtTs = (t) => new Date(t * 1000).toLocaleString('zh-CN');

// ---- tree tools: select-visible / expand / collapse / clear ----
function selectAllVisible() {
  checkedKeys.value = filtered.value.filter((a) => a.kind !== 'site').map((a) => a.id);
}
function expandAll() {
  expandedKeys.value = assets.value.filter((a) => a.kind === 'site').map((a) => a.id);
}
function collapseAll() { expandedKeys.value = []; }
function clearChecked() { checkedKeys.value = []; }

function toggleTag(id) {
  const i = activeTags.value.indexOf(id);
  if (i >= 0) activeTags.value.splice(i, 1);
  else activeTags.value.push(id);
}

// selected assets split by kind
const selectedAssets = computed(() =>
  checkedKeys.value.map((k) => assets.value.find((a) => a.id === k)).filter((a) => a && a.kind !== 'site'));
const sshTargets = computed(() => selectedAssets.value.filter((a) => a.kind === 'server'));
const sqlTargets = computed(() => selectedAssets.value.filter((a) => a.kind === 'database'));

function unselect(id) {
  checkedKeys.value = checkedKeys.value.filter((k) => k !== id);
}

function hotkey(e, fn) {
  if ((e.ctrlKey || e.metaKey) && e.key === 'Enter') {
    e.preventDefault();
    fn();
  }
}

function withConfirm(cmd, run) {
  if (DESTRUCTIVE.test(cmd)) {
    dialog.warning({
      title: '破坏性命令二次确认',
      content: `命令包含高危关键字,确定执行?\n\n${cmd}`,
      positiveText: '确认执行',
      negativeText: '取消',
      onPositiveClick: run,
    });
  } else {
    run();
  }
}

async function runSsh() {
  const targets = sshTargets.value.map((a) => a.id);
  if (!targets.length) { message.warning('请先勾选服务器节点'); return; }
  withConfirm(sshCmd.value, async () => {
    sshRunning.value = true;
    sshResults.value = [];
    try {
      const res = await api.runSsh(targets, sshCmd.value, sshTpl.value);
      sshResults.value = res.results;
    } catch (e) {
      message.error(e?.response?.data?.error || '执行失败');
    } finally {
      sshRunning.value = false;
    }
  });
}

async function runSql() {
  const targets = sqlTargets.value.map((a) => a.id);
  if (!targets.length) { message.warning('请先勾选数据库节点'); return; }
  withConfirm(sqlCmd.value, async () => {
    sqlRunning.value = true;
    sqlResults.value = [];
    try {
      const res = await api.runSql(targets, sqlCmd.value, sqlTpl.value);
      sqlResults.value = res.results;
    } catch (e) {
      message.error(e?.response?.data?.error || '执行失败');
    } finally {
      sqlRunning.value = false;
    }
  });
}

const operatorName = computed(() => auth.user?.name || auth.user?.email || '');
</script>

<template>
  <n-grid :cols="24" :x-gap="16" style="height:100%">
    <n-gi :span="8">
      <n-card size="small" :bordered="true">
        <template #header>
          资产树
          <n-text depth="3" style="font-size:12px">· 已选 {{ selectedAssets.length }} 个节点</n-text>
        </template>
        <n-input v-model:value="search" placeholder="搜索节点名 / IP / 站点…" clearable size="small" style="margin-bottom:10px" />
        <div v-if="tags.length" class="tagbar">
          <n-tag
            v-for="t in tags"
            :key="t.id"
            :bordered="false"
            size="small"
            checkable
            :checked="activeTags.includes(t.id)"
            :color="activeTags.includes(t.id) ? { color: (t.color || '#19b8a6') + '33', textColor: t.color || '#19b8a6' } : undefined"
            style="cursor:pointer;margin:0 6px 6px 0"
            @click="toggleTag(t.id)"
          >
            ● {{ t.name }}
          </n-tag>
        </div>
        <n-tree
          block-line
          checkable
          cascade
          :data="treeData"
          :checked-keys="checkedKeys"
          :expanded-keys="expandedKeys"
          :render-label="renderLabel"
          key-field="key"
          label-field="label"
          children-field="children"
          @update:checked-keys="(k) => (checkedKeys = k)"
          @update:expanded-keys="(k) => (expandedKeys = k)"
        />
        <n-empty v-if="!treeData.length" description="无匹配资产" style="margin-top:16px" />
        <template #action>
          <n-space size="small">
            <n-button size="tiny" @click="selectAllVisible">全选可见</n-button>
            <n-button size="tiny" @click="expandAll">展开</n-button>
            <n-button size="tiny" @click="collapseAll">折叠</n-button>
            <n-button size="tiny" @click="clearChecked">清空</n-button>
            <n-button size="tiny" quaternary @click="load">刷新</n-button>
          </n-space>
        </template>
      </n-card>
    </n-gi>

    <n-gi :span="16">
      <n-space vertical :size="16">
        <!-- 服务器区 (SSH) -->
        <n-card size="small" :bordered="true" class="typecard srv">
          <template #header>服务器区 · SSH <n-text depth="3" style="font-size:12px">({{ sshTargets.length }} 台)</n-text></template>
          <template #header-extra>
            <n-space size="small" align="center">
              <n-select v-if="sshTemplates.length" v-model:value="sshTpl" :options="sshTemplates" placeholder="载入模板" size="small"
                style="width:160px" clearable @update:value="(v) => applyTemplate(v, 'ssh')" />
              <n-button text size="small" type="primary" title="管理执行模板" @click="router.push('/templates')">模板</n-button>
            </n-space>
          </template>
          <n-space size="small" style="margin-bottom:8px" v-if="sshTargets.length">
            <n-tag v-for="a in sshTargets" :key="a.id" size="small" closable @close="unselect(a.id)">{{ a.name }}</n-tag>
          </n-space>
          <div v-if="sshVarDefs.length" class="varbar">
            <span class="varlabel">模板变量</span>
            <n-input-group v-for="v in sshVarDefs" :key="v.name" size="small" style="width:auto">
              <n-input-group-label size="small">{{ v.name }}</n-input-group-label>
              <n-input v-model:value="sshVars[v.name]" size="small" style="width:120px"
                :placeholder="v.default || v.name" @update:value="onVarInput('ssh')" />
            </n-input-group>
          </div>
          <div @keydown.capture="(e) => hotkey(e, runSsh)">
            <n-input
              v-model:value="sshCmd"
              type="textarea"
              :autosize="{ minRows: 1, maxRows: 4 }"
              placeholder="SSH 命令,如 uname -a"
              style="font-family:monospace"
            />
          </div>
          <n-space size="small" style="margin:8px 0">
            <n-tag v-for="q in SSH_QUICK" :key="q" size="small" style="cursor:pointer" @click="useQuick(q, 'ssh')">{{ q }}</n-tag>
          </n-space>
          <div>
            <n-button type="primary" :loading="sshRunning" @click="runSsh">▶ 执行</n-button>
            <n-text depth="3" style="margin-left:12px;font-size:12px">Ctrl/⌘+Enter</n-text>
          </div>
          <ResultBlock :rows="sshResults" :running="sshRunning" :operator="operatorName" empty="勾选服务器并执行" />
        </n-card>

        <!-- 数据库区 (SQL) -->
        <n-card size="small" :bordered="true" class="typecard db">
          <template #header>数据库区 · SQL <n-text depth="3" style="font-size:12px">({{ sqlTargets.length }} 个 · 仅 sqlite)</n-text></template>
          <template #header-extra>
            <n-space size="small" align="center">
              <n-select v-if="sqlTemplates.length" v-model:value="sqlTpl" :options="sqlTemplates" placeholder="载入模板" size="small"
                style="width:160px" clearable @update:value="(v) => applyTemplate(v, 'sql')" />
              <n-button text size="small" type="primary" title="管理执行模板" @click="router.push('/templates')">模板</n-button>
            </n-space>
          </template>
          <n-space size="small" style="margin-bottom:8px" v-if="sqlTargets.length">
            <n-tag v-for="a in sqlTargets" :key="a.id" size="small" closable @close="unselect(a.id)">{{ a.name }}</n-tag>
          </n-space>
          <div v-if="sqlVarDefs.length" class="varbar">
            <span class="varlabel">模板变量</span>
            <n-input-group v-for="v in sqlVarDefs" :key="v.name" size="small" style="width:auto">
              <n-input-group-label size="small">{{ v.name }}</n-input-group-label>
              <n-input v-model:value="sqlVars[v.name]" size="small" style="width:120px"
                :placeholder="v.default || v.name" @update:value="onVarInput('sql')" />
            </n-input-group>
          </div>
          <div @keydown.capture="(e) => hotkey(e, runSql)">
            <n-input
              v-model:value="sqlCmd"
              type="textarea"
              :autosize="{ minRows: 1, maxRows: 4 }"
              placeholder="SQL 查询,如 SELECT 1"
              style="font-family:monospace"
            />
          </div>
          <n-space size="small" style="margin:8px 0">
            <n-tag v-for="q in SQL_QUICK" :key="q" size="small" style="cursor:pointer" @click="useQuick(q, 'sql')">{{ q }}</n-tag>
          </n-space>
          <div>
            <n-button :loading="sqlRunning" class="dbrun" @click="runSql">▶ 执行</n-button>
            <n-text depth="3" style="margin-left:12px;font-size:12px">Ctrl/⌘+Enter</n-text>
          </div>
          <ResultBlock :rows="sqlResults" :running="sqlRunning" :operator="operatorName" empty="勾选数据库并执行" />
        </n-card>

        <n-empty v-if="!selectedAssets.length" description="在左侧勾选服务器或数据库节点" style="margin-top:8px" />
      </n-space>
    </n-gi>
  </n-grid>

  <!-- 单节点执行记录抽屉 -->
  <n-drawer v-model:show="histOpen" :width="460">
      <n-drawer-content closable>
        <template #header>
          节点记录 · {{ histNode?.name }}
          <n-text depth="3" style="font-size:12px;margin-left:8px">
            共 {{ histStats.total }} · 成功 {{ histStats.ok }}<span v-if="histStats.fail"> · 失败 {{ histStats.fail }}</span>
          </n-text>
        </template>
        <n-spin :show="histLoading">
          <n-empty v-if="!histRows.length && !histLoading" description="该节点暂无执行记录" style="margin-top:40px" />
          <div v-for="(r, i) in histRows" :key="i" class="nhrow">
            <div class="nhh">
              <span class="nhtime">{{ fmtTs(r.ts) }}</span>
              <n-tag size="tiny" :bordered="false" :type="r.status === 'ok' ? 'success' : r.status === 'rejected' ? 'default' : 'error'">
                {{ r.status === 'ok' ? '成功' : r.status === 'rejected' ? '已驳回' : (r.status === 'pending' ? '待审批' : '失败') }}
              </n-tag>
              <span class="nhms">{{ r.duration_ms }} ms<span v-if="r.exit_code != null"> · exit {{ r.exit_code }}</span></span>
            </div>
            <pre class="nhcmd">{{ r.command }}</pre>
            <pre v-if="r.stdout" class="nhout">{{ r.stdout }}</pre>
            <pre v-if="r.stderr" class="nhout warn">{{ r.stderr }}</pre>
            <div class="nhmeta">
              <span>{{ r.operator_email }}</span>
              <a @click="router.push(`/record/${r.job_id}`)">查看完整 →</a>
            </div>
          </div>
        </n-spin>
      </n-drawer-content>
    </n-drawer>
</template>

<script>
import { h } from 'vue';
import { NCollapse, NCollapseItem, NAlert, NTag, NSpace, NEmpty } from 'naive-ui';

// Inline result renderer shared by both areas (server + database).
const ResultBlock = {
  props: {
    rows: { type: Array, default: () => [] },
    running: Boolean,
    empty: String,
    operator: { type: String, default: '' },
  },
  setup(props) {
    return () => {
      if (!props.rows.length) {
        return props.running ? null : h(NEmpty, { description: props.empty, style: 'margin-top:14px' });
      }
      const s = {
        ok: props.rows.filter((r) => r.ok).length,
        fail: props.rows.filter((r) => !r.ok && !r.pending).length,
        pending: props.rows.filter((r) => r.pending).length,
      };
      const tags = h(NSpace, { style: 'margin:10px 0 6px' }, () => [
        h(NTag, { type: 'success', bordered: false }, () => `✓ 成功 ${s.ok}`),
        h(NTag, { type: 'error', bordered: false }, () => `✗ 失败 ${s.fail}`),
        s.pending ? h(NTag, { type: 'warning', bordered: false }, () => `⏳ 待审批 ${s.pending}`) : null,
      ]);
      const banner = s.pending
        ? h(NAlert, { type: 'warning', bordered: false, title: '已挂起,等待管理员审批', style: 'margin-bottom:8px' },
            () => `${s.pending} 个目标命中「需审批」规则,已提交审批请求。`)
        : null;
      const items = props.rows.map((r, i) =>
        h(NCollapseItem, { name: String(i), key: i }, {
          header: () => h('span', { style: { color: r.pending ? 'var(--warn)' : (r.ok ? 'var(--success)' : 'var(--danger)'), fontWeight: 600 } },
            `${r.pending ? '⏳' : (r.ok ? '✓' : '✗')} ${r.target}${r.pending ? ' · 待审批' : (r.exit_code != null ? ' · ' + r.exit_code : '')}${!r.pending && r.duration_ms ? ' · ' + r.duration_ms + ' ms' : ''}`),
          default: () => [
            r.pending ? h(NAlert, { type: 'warning', bordered: false, style: 'margin-bottom:8px' }, () => '已提交审批,等待管理员放行。') : null,
            !r.pending && r.error ? h(NAlert, { type: 'error', bordered: false, style: 'margin-bottom:8px' }, () => r.error) : null,
            r.stdout ? h('pre', { style: 'white-space:pre-wrap;font-family:monospace;font-size:13px;margin:0;color:var(--fg)' }, r.stdout) : null,
            r.stderr ? h('pre', { style: 'color:var(--warn);white-space:pre-wrap' }, r.stderr) : null,
          ],
        }));
      const footer = h('div', {
        style: 'margin-top:8px;font-size:12px;color:var(--muted)',
      }, `汇总 ${s.ok}/${props.rows.length} 成功${s.pending ? ` · ${s.pending} 待审批` : ''} · 审计已记录${props.operator ? ' · ' + props.operator : ''}`);
      return h('div', { style: 'margin-top:8px' }, [
        tags,
        banner,
        h(NCollapse, { defaultExpandedNames: props.rows.map((_, i) => String(i)) }, () => items),
        footer,
      ]);
    };
  },
};

export default { components: { ResultBlock } };
</script>

<style scoped>
.tagbar { margin-bottom: 10px; display: flex; flex-wrap: wrap; }
.varbar { display: flex; flex-wrap: wrap; gap: 8px; align-items: center; margin-bottom: 8px; }
.varlabel { font-size: 12px; color: var(--muted); }
.typecard.srv :deep(.n-card-header__main) { color: var(--accent); }
.typecard.db :deep(.n-card-header__main) { color: var(--accent-2); }
.dbrun { background: var(--accent-2); color: #fff; border: none; }
</style>

<style>
/* tree node rows (render-label is teleport-free but naive-ui scopes poorly) */
.tnode { display: inline-flex; align-items: center; gap: 6px; min-width: 0; }
.tnode .tname { font-weight: 500; }
.tnode .tname.db { color: var(--accent-2); }
.tnode .tmeta { font-size: 11px; color: var(--muted); }
.tnode .tdots { display: inline-flex; gap: 3px; }
.tnode .tdot { width: 7px; height: 7px; border-radius: 50%; display: inline-block; }
.tnode .tbadge { font-size: 10px; padding: 0 5px; border-radius: 4px; background: var(--surface-warm); color: var(--muted); border: 1px solid rgba(255,255,255,.08); }
.tnode .tlog { font-size: 11px; color: var(--muted); padding: 0 5px; border-radius: 4px; border: 1px solid rgba(255,255,255,.1); cursor: pointer; transition: color .12s, background .12s; }
.tnode .tlog:hover { color: var(--accent); background: var(--surface-warm); }

/* single-node history drawer rows */
.nhrow { border: 1px solid rgba(255,255,255,.08); border-radius: 8px; padding: 10px 12px; margin-bottom: 10px; }
.nhh { display: flex; align-items: center; gap: 8px; }
.nhh .nhtime { font-size: 12px; color: var(--muted); }
.nhh .nhms { font-size: 11px; color: var(--muted); font-family: monospace; margin-left: auto; }
.nhcmd { white-space: pre-wrap; font-family: monospace; font-size: 12px; margin: 8px 0 0; background: var(--bg); padding: 6px 8px; border-radius: 6px; }
.nhout { white-space: pre-wrap; font-family: monospace; font-size: 12px; margin: 6px 0 0; background: var(--bg); padding: 6px 8px; border-radius: 6px; max-height: 160px; overflow: auto; color: var(--fg-2); }
.nhout.warn { color: var(--warn); }
.nhmeta { display: flex; justify-content: space-between; align-items: center; margin-top: 8px; font-size: 12px; color: var(--muted); }
.nhmeta a { color: var(--accent); cursor: pointer; }
</style>
