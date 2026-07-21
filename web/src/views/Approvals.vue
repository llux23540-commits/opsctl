<script setup>
import { ref, onMounted, computed, h } from 'vue';
import { useRouter } from 'vue-router';
import { NButton, NTag, useMessage } from 'naive-ui';
import { api } from '../api';

const router = useRouter();
const message = useMessage();
const rows = ref([]);
const loading = ref(false);

// detail drawer
const detail = ref(null);
const showDetail = ref(false);
function openDetail(row) { detail.value = row; showDetail.value = true; }
function openRecord(id) { router.push(`/record/${id}`); }

// reject modal — target modes: single approval | whole job | (none)
const showReject = ref(false);
const rejecting = ref(false);
const rejectTarget = ref(null); // single approval row
const rejectJob = ref(null);    // whole job (all its pending subs)
const reason = ref('');
const reasonErr = ref('');

// which jobs are expanded (job_id -> bool)
const expanded = ref({});
function toggleJob(jid) { expanded.value[jid] = !expanded.value[jid]; }

async function load() {
  loading.value = true;
  try {
    rows.value = await api.approvals();
  } catch (e) {
    message.error(e?.response?.data?.error || '加载审批失败(需 admin)');
  } finally {
    loading.value = false;
  }
}
onMounted(load);

const pending = computed(() => rows.value.filter((r) => r.state === 'pending'));
const decided = computed(() => rows.value.filter((r) => r.state !== 'pending'));

// group approvals into JOBS (subtasks share a job_id); keep jobs with ≥1 pending
const pendingJobs = computed(() => {
  const groups = {};
  rows.value.forEach((a) => {
    const jid = a.job_id || a.id;
    if (!groups[jid]) {
      groups[jid] = { job_id: jid, requester_email: a.requester_email, action: a.action,
        env: a.env, created_at: a.created_at, subs: [] };
    }
    groups[jid].subs.push(a);
  });
  return Object.values(groups)
    .map((g) => ({ ...g, pendingCount: g.subs.filter((s) => s.state === 'pending').length }))
    .filter((g) => g.pendingCount > 0)
    .sort((x, y) => (y.created_at || 0) - (x.created_at || 0));
});

const todayStats = computed(() => {
  const start = new Date(); start.setHours(0, 0, 0, 0);
  const s0 = start.getTime() / 1000;
  let approved = 0, rejected = 0;
  decided.value.forEach((r) => {
    if ((r.decided_at || 0) >= s0) {
      if (r.state === 'approved') approved += 1;
      else if (r.state === 'rejected') rejected += 1;
    }
  });
  return { approved, rejected };
});

async function approve(row) {
  try {
    const res = await api.decideApproval(row.id, { verdict: 'approve' });
    if (res.state === 'pending') {
      message.info(`已投批准票(${res.votes}/${res.need}),等待其他管理员会签`);
    } else {
      const r = res.result;
      if (r && r.ok) message.success(`已放行并执行:${r.target}`);
      else message.warning(`已放行;执行返回:${r?.error || '非零退出'}`);
    }
    await load();
    window.dispatchEvent(new Event('approvals-changed'));
  } catch (e) {
    message.error(e?.response?.data?.error || '放行失败');
  }
}

// approve every pending subtask of a job (按任务一键放行)
async function approveJob(job) {
  let ok = 0, votepend = 0, fail = 0;
  for (const s of job.subs.filter((x) => x.state === 'pending')) {
    try {
      const r = await api.decideApproval(s.id, { verdict: 'approve' });
      if (r.state === 'approved') ok += 1; else if (r.state === 'pending') votepend += 1; else fail += 1;
    } catch (e) { fail += 1; }
  }
  const parts = [];
  if (ok) parts.push(`已放行 ${ok}`);
  if (votepend) parts.push(`已投票待会签 ${votepend}`);
  if (fail) parts.push(`失败 ${fail}`);
  message.success('按任务放行:' + (parts.join(' · ') || '完成'));
  await load();
  window.dispatchEvent(new Event('approvals-changed'));
}

function openReject(row) { rejectTarget.value = row; rejectJob.value = null; reason.value = ''; reasonErr.value = ''; showReject.value = true; }
function openRejectJob(job) { rejectJob.value = job; rejectTarget.value = null; reason.value = ''; reasonErr.value = ''; showReject.value = true; }

async function doReject() {
  if (!reason.value.trim()) { reasonErr.value = '请填写驳回理由'; return; }
  rejecting.value = true;
  try {
    if (rejectTarget.value) {
      await api.decideApproval(rejectTarget.value.id, { verdict: 'reject', reason: reason.value });
      message.success('已驳回并记录理由');
    } else if (rejectJob.value) {
      let n = 0;
      for (const s of rejectJob.value.subs.filter((x) => x.state === 'pending')) {
        try { await api.decideApproval(s.id, { verdict: 'reject', reason: reason.value }); n += 1; } catch (e) { /* skip */ }
      }
      message.success(`按任务驳回 ${n} 个子任务`);
    }
    showReject.value = false;
    await load();
    window.dispatchEvent(new Event('approvals-changed'));
  } catch (e) {
    message.error(e?.response?.data?.error || '驳回失败');
  } finally {
    rejecting.value = false;
  }
}

const stateTag = (s) =>
  s === 'pending'
    ? h(NTag, { type: 'warning', size: 'small', bordered: false }, { default: () => '待确认' })
    : s === 'approved'
      ? h(NTag, { type: 'success', size: 'small', bordered: false }, { default: () => '已放行' })
      : h(NTag, { type: 'error', size: 'small', bordered: false }, { default: () => '已驳回' });

const fmt = (t) => (t ? new Date(t * 1000).toLocaleString('zh-CN') : '—');
const initials = (s) => (s || '?').slice(0, 2).toUpperCase();

const envMeta = { prod: { label: 'prod', type: 'error' }, staging: { label: 'staging', type: 'warning' }, dev: { label: 'dev', type: 'info' } };
const envTag = (env) => (envMeta[env]
  ? h(NTag, { size: 'small', bordered: false, type: envMeta[env].type }, { default: () => envMeta[env].label })
  : null);
const subStateLabel = (s) => ({ pending: '待批', approved: '已放行', rejected: '已驳回' }[s] || s);

// review channel: console = strong-auth console approval; tg = inline one-tap (demo)
const quickTag = (q) => (q === 'tg'
  ? h('span', { style: 'display:inline-flex;gap:4px' }, [
      h(NTag, { size: 'small', bordered: false, type: 'info' }, { default: () => 'TG 一键' }),
      h(NTag, { size: 'small', bordered: false, type: 'warning' }, { default: () => '演示' }),
    ])
  : h(NTag, { size: 'small', bordered: false }, { default: () => '控制台' }));
const quickLabel = (q) => (q === 'tg' ? 'TG 内联一键(演示)' : '控制台登录批准(强认证)');

const baseCols = [
  { title: '提交人', key: 'requester_email', width: 150 },
  { title: '目标', key: 'target_name', width: 120 },
  { title: '环境', key: 'env', width: 80, render: (r) => envTag(r.env) || '—' },
  { title: '账号', key: 'account_name', width: 110 },
  { title: '动作', key: 'action', width: 70 },
  { title: '审核方式', key: 'quick', width: 110, render: (r) => quickTag(r.quick) },
  { title: '命令', key: 'command', ellipsis: { tooltip: true } },
  { title: '提交时间', key: 'created_at', width: 170, render: (r) => fmt(r.created_at) },
];

const decidedCols = [
  ...baseCols,
  { title: '状态', key: 'state', width: 90, render: (r) => stateTag(r.state) },
  { title: '决策人', key: 'decided_by', width: 150, render: (r) => r.decided_by || '—' },
  { title: '驳回理由', key: 'reason', ellipsis: { tooltip: true }, render: (r) => r.reason || '—' },
  { title: '', key: 'ops', width: 70,
    render: (r) => h(NButton, { size: 'tiny', text: true, onClick: () => openDetail(r) }, { default: () => '详情' }) },
];
</script>

<template>
  <n-space vertical :size="16">
    <n-card title="待审批" size="small">
      <template #header-extra>
        <n-space align="center">
          <n-tag v-if="pending.length" type="warning" :bordered="false">{{ pendingJobs.length }} 任务 · {{ pending.length }} 子任务待批</n-tag>
          <n-tag type="success" :bordered="false">今日已放行 {{ todayStats.approved }}</n-tag>
          <n-tag type="error" :bordered="false">今日已驳回 {{ todayStats.rejected }}</n-tag>
          <n-button size="small" @click="load" :loading="loading">刷新</n-button>
        </n-space>
      </template>

      <div v-for="job in pendingJobs" :key="job.job_id" class="task" :class="{ open: expanded[job.job_id] }">
        <div class="task-hd" @click="toggleJob(job.job_id)">
          <span class="caret">▸</span>
          <span class="av">{{ initials(job.requester_email) }}</span>
          <div class="who">
            <b>{{ job.requester_email }}</b>
            <span class="sub">{{ (job.action || '').toUpperCase() }} · 提交于 {{ fmt(job.created_at) }}</span>
          </div>
          <component :is="() => envTag(job.env)" v-if="job.env" />
          <span class="prog"><b>{{ job.subs.length }}</b> 子任务 · <b>{{ job.pendingCount }}</b> 待批</span>
          <div class="acts" @click.stop>
            <n-button size="small" type="primary" @click="approveJob(job)">全部放行</n-button>
            <n-button size="small" type="error" tertiary @click="openRejectJob(job)">全部驳回</n-button>
          </div>
        </div>
        <div v-if="expanded[job.job_id]" class="subs">
          <div v-for="(s, i) in job.subs" :key="s.id" class="subrow">
            <span class="sn">{{ i + 1 }}</span>
            <div class="sbody">
              <div class="sh">
                {{ s.target_name }}
                <span class="sacc">{{ s.account_name }}</span>
                <component :is="() => stateTag(s.state)" />
                <component :is="() => quickTag(s.quick)" />
              </div>
              <pre class="scmd">{{ s.command }}</pre>
              <div v-if="s.reason" class="sreason">驳回理由:{{ s.reason }}</div>
            </div>
            <div class="sa">
              <n-button size="tiny" @click="openDetail(s)">详情</n-button>
              <template v-if="s.state === 'pending'">
                <n-button size="tiny" type="primary" @click="approve(s)">放行</n-button>
                <n-button size="tiny" type="error" tertiary @click="openReject(s)">驳回</n-button>
              </template>
            </div>
          </div>
        </div>
      </div>
      <n-empty v-if="!pendingJobs.length && !loading" description="没有待审批任务" style="margin:16px 0" />
    </n-card>

    <n-card title="近期决策" size="small">
      <n-data-table :columns="decidedCols" :data="decided" size="small" :bordered="false" />
      <n-empty v-if="!decided.length && !loading" description="暂无决策记录" style="margin:16px 0" />
    </n-card>

    <n-modal v-model:show="showReject" preset="card" :title="rejectJob ? '按任务驳回' : '驳回审批'" style="width:480px">
      <n-form-item label="驳回理由" :validation-status="reasonErr ? 'error' : undefined" :feedback="reasonErr">
        <n-input
          v-model:value="reason"
          type="textarea"
          :rows="3"
          placeholder="理由将写入审计并对提交人可见,例如:生产高峰期禁止重启"
          @input="reasonErr = ''"
        />
      </n-form-item>
      <template #footer>
        <n-button type="error" :loading="rejecting" @click="doReject">确认驳回</n-button>
      </template>
    </n-modal>

    <!-- 审批详情抽屉 -->
    <n-drawer v-model:show="showDetail" :width="480">
      <n-drawer-content closable>
        <template #header>
          审批详情
          <n-tag size="small" :bordered="false" style="margin-left:8px">{{ (detail?.action || '').toUpperCase() }}</n-tag>
        </template>
        <template v-if="detail">
          <n-descriptions :column="1" label-placement="left" size="small" bordered>
            <n-descriptions-item label="状态"><component :is="() => stateTag(detail.state)" /></n-descriptions-item>
            <n-descriptions-item label="提交人">{{ detail.requester_email }}</n-descriptions-item>
            <n-descriptions-item label="目标">{{ detail.target_name }}</n-descriptions-item>
            <n-descriptions-item v-if="detail.env" label="环境"><component :is="() => envTag(detail.env)" /></n-descriptions-item>
            <n-descriptions-item label="连接账号">{{ detail.account_name }}</n-descriptions-item>
            <n-descriptions-item label="动作">{{ detail.action }}</n-descriptions-item>
            <n-descriptions-item label="提交时间">{{ fmt(detail.created_at) }}</n-descriptions-item>
            <n-descriptions-item label="审核方式">{{ quickLabel(detail.quick) }}</n-descriptions-item>
            <n-descriptions-item v-if="detail.min_approvals > 1" label="会签进度">{{ detail.approve_votes }}/{{ detail.min_approvals }} 已批准</n-descriptions-item>
            <n-descriptions-item v-if="detail.approvers && detail.approvers.length" label="指定审批人">{{ detail.approvers.join('、') }}</n-descriptions-item>
            <n-descriptions-item v-if="detail.decided_by" label="决策人">{{ detail.decided_by }}</n-descriptions-item>
            <n-descriptions-item v-if="detail.decided_at" label="决策时间">{{ fmt(detail.decided_at) }}</n-descriptions-item>
            <n-descriptions-item v-if="detail.reason" label="驳回理由">{{ detail.reason }}</n-descriptions-item>
          </n-descriptions>
          <div class="d-lbl">完整命令</div>
          <pre class="d-cmd">{{ detail.command }}</pre>
          <n-space style="margin-top:16px">
            <template v-if="detail.state === 'pending'">
              <n-button type="primary" size="small" @click="() => { showDetail = false; approve(detail); }">放行</n-button>
              <n-button type="error" size="small" tertiary @click="() => { showDetail = false; openReject(detail); }">驳回</n-button>
            </template>
            <n-button v-if="detail.job_id" size="small" @click="openRecord(detail.job_id)">查看执行记录 ↗</n-button>
          </n-space>
        </template>
      </n-drawer-content>
    </n-drawer>
  </n-space>
</template>

<style scoped>
.d-lbl { margin: 16px 0 6px; font-size: 12px; color: var(--muted); }
.d-cmd { white-space: pre-wrap; font-family: monospace; font-size: 13px; background: var(--bg);
  padding: 10px 12px; border-radius: 6px; margin: 0; word-break: break-all; }

.task { border: 1px solid rgba(255,255,255,.08); border-radius: 10px; margin-bottom: 10px; overflow: hidden; }
.task-hd { display: flex; align-items: center; gap: 12px; padding: 12px 14px; cursor: pointer; }
.task-hd:hover { background: var(--surface-warm); }
.task-hd .caret { color: var(--muted); font-size: 12px; transition: transform .15s; }
.task.open .task-hd .caret { transform: rotate(90deg); }
.task-hd .av { width: 30px; height: 30px; border-radius: 8px; background: var(--accent); color: #fff;
  display: grid; place-items: center; font-size: 12px; font-weight: 700; }
.task-hd .who { display: flex; flex-direction: column; }
.task-hd .who b { font-size: 14px; }
.task-hd .who .sub { font-size: 12px; color: var(--muted); font-family: monospace; }
.task-hd .prog { margin-left: auto; font-size: 12px; color: var(--muted); font-family: monospace; }
.task-hd .prog b { color: var(--fg-2); }
.task-hd .acts { display: flex; gap: 8px; }
.subs { padding: 4px 14px 10px 40px; border-top: 1px solid rgba(255,255,255,.06); }
.subrow { display: grid; grid-template-columns: 22px 1fr auto; gap: 12px; align-items: start;
  padding: 10px 0; border-bottom: 1px solid rgba(255,255,255,.06); }
.subrow:last-child { border-bottom: 0; }
.subrow .sn { display: grid; place-items: center; width: 20px; height: 20px; border-radius: 5px;
  background: var(--surface-warm); font-size: 11px; color: var(--muted); }
.subrow .sh { font-size: 13px; display: flex; align-items: center; gap: 8px; }
.subrow .sacc { font-size: 11px; color: var(--muted); font-family: monospace; }
.subrow .scmd { margin: 6px 0 0; font-family: monospace; font-size: 12px; background: var(--bg);
  border-radius: 6px; padding: 7px 10px; white-space: pre-wrap; word-break: break-all; color: var(--fg-2); }
.subrow .sreason { margin-top: 4px; font-size: 12px; color: var(--danger); }
.subrow .sa { display: flex; gap: 6px; align-items: center; }
</style>
