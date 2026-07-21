<script setup>
import { ref, onMounted, h } from 'vue';
import { useRouter } from 'vue-router';
import { NButton, NTag, useMessage } from 'naive-ui';
import { api } from '../api';
import { useAuth } from '../store/auth';

const message = useMessage();
const router = useRouter();
const auth = useAuth();

const tab = ref('jobs');

// ---- Tab 1: 执行记录 (job aggregation, visible to everyone) ----
const jobs = ref([]);
const jobsLoading = ref(false);
const jobFilters = ref({ range: '', status: '', kind: '', operator: '', q: '' });
const jobDetail = ref(null); // { job, targets, approvals }
const showJob = ref(false);

const rangeOptions = [
  { label: '全部时间', value: '' },
  { label: '今天', value: 'today' },
  { label: '近 7 天', value: '7d' },
  { label: '近 30 天', value: '30d' },
];
const jobStatusOptions = [
  { label: '全部状态', value: '' },
  { label: '成功', value: 'ok' },
  { label: '部分失败', value: 'partial' },
  { label: '失败', value: 'fail' },
  { label: '待审批', value: 'pending' },
];
const kindOptions = [
  { label: '全部类型', value: '' },
  { label: 'SSH', value: 'ssh' },
  { label: 'SQL', value: 'sql' },
];

function fromTs(range) {
  const now = new Date();
  if (range === 'today') {
    return Math.floor(new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime() / 1000);
  }
  if (range === '7d') return Math.floor(now.getTime() / 1000) - 7 * 86400;
  if (range === '30d') return Math.floor(now.getTime() / 1000) - 30 * 86400;
  return 0;
}

async function loadJobs() {
  jobsLoading.value = true;
  try {
    const f = jobFilters.value;
    jobs.value = await api.jobs({
      status: f.status, kind: f.kind, operator: f.operator, q: f.q, from_ts: fromTs(f.range),
    });
  } catch (e) {
    message.error(e?.response?.data?.error || '加载执行记录失败');
  } finally { jobsLoading.value = false; }
}
function resetJobs() { jobFilters.value = { range: '', status: '', kind: '', operator: '', q: '' }; loadJobs(); }

async function openJob(row) {
  try {
    jobDetail.value = await api.jobDetail(row.id);
    showJob.value = true;
  } catch (e) {
    message.error(e?.response?.data?.error || '加载详情失败');
  }
}
function openRecord(id) { router.push(`/record/${id}`); }

const fmt = (t) => (t ? new Date(t * 1000).toLocaleString('zh-CN') : '—');

const jobStatusTag = (s) => {
  const map = {
    ok: { type: 'success', label: '✓ 成功' },
    partial: { type: 'warning', label: '◐ 部分失败' },
    fail: { type: 'error', label: '✗ 失败' },
    pending: { type: 'default', label: '⏳ 待审批' },
  };
  const m = map[s] || { type: 'default', label: s };
  return h(NTag, { type: m.type, size: 'small', bordered: false }, () => m.label);
};
const targetStatusTag = (s) => {
  const map = {
    ok: { type: 'success', label: '✓ ok' },
    fail: { type: 'error', label: '✗ fail' },
    pending: { type: 'warning', label: '⏳ 待审批' },
    rejected: { type: 'default', label: '⊘ 已驳回' },
  };
  const m = map[s] || { type: 'default', label: s };
  return h(NTag, { type: m.type, size: 'small', bordered: false }, () => m.label);
};

const jobColumns = [
  { title: '时间', key: 'created_at', width: 165, render: (r) => fmt(r.created_at) },
  { title: '执行人', key: 'operator_email', width: 150 },
  { title: '类型', key: 'kind', width: 60, render: (r) => r.kind.toUpperCase() },
  { title: '模板', key: 'template_name', width: 110, ellipsis: { tooltip: true },
    render: (r) => r.template_name || '—' },
  { title: '命令', key: 'command', ellipsis: { tooltip: true },
    render: (r) => h('span', { style: 'font-family:monospace;font-size:12px' }, r.command) },
  { title: '目标', key: 'okc', width: 80, render: (r) => `${r.ok_count}/${r.total} 成功` },
  { title: '状态', key: 'status', width: 110, render: (r) => jobStatusTag(r.status) },
  { title: '耗时', key: 'duration_ms', width: 90,
    render: (r) => (r.duration_ms != null ? (r.duration_ms >= 1000 ? (r.duration_ms / 1000).toFixed(1) + ' s' : r.duration_ms + ' ms') : '—') },
  { title: '', key: 'ops', width: 60,
    render: (r) => h(NButton, { size: 'tiny', text: true, onClick: () => openJob(r) }, () => '详情') },
];

// ---- Tab 2: 审计流水 (raw audit trail, admin only) ----
const rows = ref([]);
const loading = ref(false);
const detail = ref(null);
const showDetail = ref(false);

const filters = ref({ action: '', result: '', operator: '', q: '' });
const actionOptions = [
  { label: '全部动作', value: '' },
  { label: 'login', value: 'login' },
  { label: 'ssh.exec', value: 'ssh.exec' },
  { label: 'ssh.request', value: 'ssh.request' },
  { label: 'ssh.reject', value: 'ssh.reject' },
  { label: 'sql.exec', value: 'sql.exec' },
  { label: 'sql.request', value: 'sql.request' },
  { label: 'sql.reject', value: 'sql.reject' },
  { label: 'git.sync', value: 'git.sync' },
];
const resultOptions = [
  { label: '全部结果', value: '' },
  { label: 'ok', value: 'ok' },
  { label: 'fail', value: 'fail' },
  { label: 'pending', value: 'pending' },
  { label: 'rejected', value: 'rejected' },
];

// pending/rejected are states, not failures — render them neutrally
const resultTag = (r) => {
  const map = {
    ok: { type: 'success', label: '✓ ok' },
    fail: { type: 'error', label: '✗ fail' },
    pending: { type: 'warning', label: '⏳ pending' },
    rejected: { type: 'default', label: '⊘ rejected' },
  };
  const m = map[r.result] || { type: 'default', label: r.result };
  return h(NTag, { type: m.type, size: 'small', bordered: false }, () => m.label);
};

const columns = [
  { title: '时间', key: 'ts', width: 165, render: (r) => fmt(r.ts) },
  { title: '操作人', key: 'operator_email', width: 150 },
  { title: '动作', key: 'action', width: 110 },
  { title: '目标', key: 'targets', ellipsis: { tooltip: true } },
  { title: '命令/载荷', key: 'payload', ellipsis: { tooltip: true } },
  { title: '结果', key: 'result', width: 110, render: resultTag },
  { title: '', key: 'ops', width: 90,
    render: (r) => h('span', { style: 'display:inline-flex;gap:8px' }, [
      h(NButton, { size: 'tiny', text: true, onClick: () => open(r) }, () => '详情'),
      r.job_id ? h(NButton, { size: 'tiny', text: true, type: 'primary', onClick: () => openRecord(r.job_id) }, () => '记录') : null,
    ]) },
];

function open(r) { detail.value = r; showDetail.value = true; }

async function load() {
  loading.value = true;
  try {
    rows.value = await api.auditFiltered({
      action: filters.value.action, result: filters.value.result,
      operator: filters.value.operator, q: filters.value.q,
    });
  } catch (e) {
    message.error('加载审计失败(需 admin)');
  } finally { loading.value = false; }
}
function reset() { filters.value = { action: '', result: '', operator: '', q: '' }; load(); }

async function exportAs(format) {
  try {
    const blob = await api.auditExport(format, filters.value);
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `audit.${format}`;
    a.click();
    URL.revokeObjectURL(url);
  } catch (e) {
    message.error(e?.response?.data?.error || '导出失败');
  }
}

// ---- backup banner (real local snapshot state from GET /backup/status) ----
const backup = ref(null);
async function loadBackup() {
  backup.value = await api.backupStatus().catch(() => null);
}
const backupText = () => {
  const b = backup.value;
  if (!b) return '';
  const last = b.last_at ? `上次成功 ${fmt(b.last_at)}` : '尚未执行';
  return `自动备份:每日 03:00 快照至本地 ${b.dir || 'data/backups'} · ${last} · 保留 ${b.retention_days} 天 · 共 ${b.count} 份`;
};

onMounted(() => {
  loadJobs();
  loadBackup();
  if (auth.isAdmin) load();
});
</script>

<template>
  <n-tabs v-model:value="tab" type="line" animated>
    <!-- 执行记录:job 级聚合,operator 只见自己 -->
    <n-tab-pane name="jobs" tab="执行记录">
      <n-alert v-if="backup && backup.enabled" type="success" :show-icon="true" style="margin-bottom:12px" data-test="backup-banner">
        {{ backupText() }}
      </n-alert>
      <n-card size="small" :bordered="true">
        <template #header-extra>
          <n-button size="small" @click="loadJobs" :loading="jobsLoading">刷新</n-button>
        </template>
        <n-space style="margin-bottom:12px" :wrap="true">
          <n-select v-model:value="jobFilters.range" :options="rangeOptions" size="small" style="width:120px" />
          <n-select v-model:value="jobFilters.status" :options="jobStatusOptions" size="small" style="width:130px" />
          <n-select v-model:value="jobFilters.kind" :options="kindOptions" size="small" style="width:110px" />
          <n-input v-if="auth.isAdmin" v-model:value="jobFilters.operator" placeholder="操作人 id" size="small" style="width:150px" clearable />
          <n-input v-model:value="jobFilters.q" placeholder="搜索命令/操作人" size="small" style="width:200px" clearable @keyup.enter="loadJobs" />
          <n-button size="small" type="primary" @click="loadJobs">筛选</n-button>
          <n-button size="small" @click="resetJobs">重置</n-button>
        </n-space>
        <n-data-table :columns="jobColumns" :data="jobs" size="small" :bordered="false" :loading="jobsLoading" />
        <n-empty v-if="!jobs.length && !jobsLoading" description="暂无执行记录" style="margin:16px 0" />
      </n-card>

      <n-drawer v-model:show="showJob" :width="560">
        <n-drawer-content closable>
          <template #header>
            执行详情
            <n-button v-if="jobDetail" size="tiny" type="primary" text style="margin-left:12px"
              @click="openRecord(jobDetail.job.id)">单独打开 ↗</n-button>
          </template>
          <template v-if="jobDetail">
            <n-descriptions :column="1" label-placement="left" size="small" bordered>
              <n-descriptions-item label="状态">
                <component :is="() => jobStatusTag(jobDetail.job.status)" />
                <span style="margin-left:8px">{{ jobDetail.job.ok_count }}/{{ jobDetail.job.total }} 成功</span>
              </n-descriptions-item>
              <n-descriptions-item label="执行人">{{ jobDetail.job.operator_email }}</n-descriptions-item>
              <n-descriptions-item label="类型">{{ jobDetail.job.kind.toUpperCase() }}</n-descriptions-item>
              <n-descriptions-item label="提交时间">{{ fmt(jobDetail.job.created_at) }}</n-descriptions-item>
              <n-descriptions-item label="完成时间">{{ fmt(jobDetail.job.finished_at) }}</n-descriptions-item>
            </n-descriptions>
            <div style="margin-top:14px;font-size:12px;color:var(--muted)">命令</div>
            <pre class="cmdblock">{{ jobDetail.job.command }}</pre>
            <div style="margin-top:14px;font-size:12px;color:var(--muted)">逐目标结果</div>
            <div v-for="t in jobDetail.targets" :key="t.id" class="targetrow">
              <div class="trhead">
                <component :is="() => targetStatusTag(t.status)" />
                <b style="margin-left:8px">{{ t.asset_name || t.asset_id }}</b>
                <span v-if="t.exit_code != null" class="trmeta">exit {{ t.exit_code }}</span>
                <span v-if="t.duration_ms" class="trmeta">{{ t.duration_ms }} ms</span>
              </div>
              <div v-if="t.error" class="trerr">{{ t.error }}</div>
              <pre v-if="t.stdout" class="trout">{{ t.stdout }}</pre>
              <pre v-if="t.stderr" class="trout warn">{{ t.stderr }}</pre>
            </div>
          </template>
        </n-drawer-content>
      </n-drawer>
    </n-tab-pane>

    <!-- 审计流水:全量治理事件,仅 admin -->
    <n-tab-pane v-if="auth.isAdmin" name="trail" tab="审计流水">
      <n-card size="small" :bordered="true">
        <template #header-extra>
          <n-space>
            <n-button size="small" @click="exportAs('csv')">导出 CSV</n-button>
            <n-button size="small" @click="exportAs('json')">导出 JSON</n-button>
            <n-button size="small" @click="load" :loading="loading">刷新</n-button>
          </n-space>
        </template>

        <n-space style="margin-bottom:12px" :wrap="true">
          <n-select v-model:value="filters.action" :options="actionOptions" size="small" style="width:150px" />
          <n-select v-model:value="filters.result" :options="resultOptions" size="small" style="width:130px" />
          <n-input v-model:value="filters.operator" placeholder="操作人邮箱" size="small" style="width:160px" clearable />
          <n-input v-model:value="filters.q" placeholder="搜索目标/命令" size="small" style="width:200px" clearable @keyup.enter="load" />
          <n-button size="small" type="primary" @click="load">筛选</n-button>
          <n-button size="small" @click="reset">重置</n-button>
        </n-space>

        <n-data-table :columns="columns" :data="rows" size="small" :bordered="false" :loading="loading" />

        <n-drawer v-model:show="showDetail" :width="440">
          <n-drawer-content title="审计详情" closable>
            <template v-if="detail">
              <n-descriptions :column="1" label-placement="left" size="small" bordered>
                <n-descriptions-item label="时间">{{ fmt(detail.ts) }}</n-descriptions-item>
                <n-descriptions-item label="操作人">{{ detail.operator_email }}</n-descriptions-item>
                <n-descriptions-item label="动作">{{ detail.action }}</n-descriptions-item>
                <n-descriptions-item label="目标">{{ detail.targets || '—' }}</n-descriptions-item>
                <n-descriptions-item label="结果"><component :is="() => resultTag(detail)" /></n-descriptions-item>
              </n-descriptions>
              <div style="margin-top:14px;font-size:12px;color:var(--muted)">命令 / 载荷</div>
              <pre class="cmdblock">{{ detail.payload || '—' }}</pre>
              <n-button v-if="detail.job_id" size="small" type="primary" style="margin-top:12px"
                @click="openRecord(detail.job_id)">打开所属执行记录 ↗</n-button>
            </template>
          </n-drawer-content>
        </n-drawer>
      </n-card>
    </n-tab-pane>
  </n-tabs>
</template>

<style scoped>
.cmdblock { white-space: pre-wrap; font-family: monospace; font-size: 13px; background: var(--bg); padding: 10px; border-radius: 6px; margin-top: 6px; }
.targetrow { border: 1px solid rgba(255,255,255,.08); border-radius: 6px; padding: 8px 10px; margin-top: 8px; }
.trhead { display: flex; align-items: center; }
.trmeta { margin-left: 10px; font-size: 12px; color: var(--muted); font-family: monospace; }
.trerr { color: var(--danger); font-size: 12px; margin-top: 6px; }
.trout { white-space: pre-wrap; font-family: monospace; font-size: 12px; margin: 6px 0 0; background: var(--bg); padding: 8px; border-radius: 6px; max-height: 220px; overflow: auto; }
.trout.warn { color: var(--warn); }
</style>
