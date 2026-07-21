<script setup>
import { ref, computed, onMounted } from 'vue';
import { useRoute } from 'vue-router';
import { api } from '../api';

const route = useRoute();
const data = ref(null); // { job, targets, approvals }
const missing = ref(false);

const backup = ref(null);

onMounted(async () => {
  try {
    data.value = await api.jobDetail(route.params.id);
  } catch (e) {
    missing.value = true;
  }
  backup.value = await api.backupStatus().catch(() => null);
});

const job = computed(() => data.value?.job);
const targets = computed(() => data.value?.targets || []);
const approvals = computed(() => data.value?.approvals || []);

const fmt = (t) => (t ? new Date(t * 1000).toLocaleString('zh-CN') : '—');

const verdict = computed(() => {
  const map = {
    ok: { cls: 'ok', icon: '✓', label: '全部成功' },
    partial: { cls: 'partial', icon: '◐', label: '部分失败' },
    fail: { cls: 'fail', icon: '✗', label: '执行失败' },
    pending: { cls: 'pending', icon: '⏳', label: '等待审批' },
  };
  return map[job.value?.status] || map.pending;
});

const totalMs = computed(() => targets.value.reduce((s, t) => s + (t.duration_ms || 0), 0));
const firstFail = computed(() => targets.value.find((t) => t.status === 'fail' || t.status === 'rejected'));
const failHint = (t) => t.error || (t.stderr || '').split('\n')[0] || `exit ${t.exit_code}`;

const targetCls = (s) => ({ ok: 'ok', fail: 'fail', rejected: 'fail', pending: 'pending' }[s] || '');
const targetIcon = (s) => ({ ok: '✓', fail: '✗', rejected: '⊘', pending: '⏳' }[s] || '·');
const targetLabel = (s) => ({ ok: '成功', fail: '失败', rejected: '已驳回', pending: '待审批' }[s] || s);

// "Web 控制台 · <ip> · <device>"; old jobs without ip/device degrade gracefully
const sourceText = computed(() => {
  const parts = ['Web 控制台'];
  if (job.value?.source_ip) parts.push(job.value.source_ip);
  if (job.value?.source_device) parts.push(job.value.source_device);
  return parts.join(' · ');
});

function doPrint() { window.print(); }
</script>

<template>
  <div class="record">
    <n-empty v-if="missing" description="未找到该执行记录,或你无权查看" style="margin:80px 0" />
    <template v-else-if="job">
      <!-- header -->
      <div class="rhead no-print-btn">
        <div>
          <div class="rid">JOB {{ job.id }}</div>
          <h2 class="rtitle">{{ job.command.split('\n')[0] || '执行记录' }}</h2>
        </div>
        <n-button size="small" class="noprint" @click="doPrint">🖨 打印存档</n-button>
      </div>

      <!-- verdict banner -->
      <div class="verdict" :class="verdict.cls">
        <span class="vicon">{{ verdict.icon }}</span>
        <div>
          <b>{{ verdict.label }}</b> · {{ job.ok_count }}/{{ job.total }} 目标成功
          <template v-if="totalMs"> · 总耗时 {{ totalMs >= 1000 ? (totalMs / 1000).toFixed(1) + ' s' : totalMs + ' ms' }}</template>
          · {{ fmt(job.created_at) }}
          <div v-if="firstFail && job.status !== 'ok'" class="vfail">
            首个失败:{{ firstFail.asset_name || firstFail.asset_id }} — {{ failHint(firstFail) }}
          </div>
        </div>
      </div>

      <!-- summary -->
      <n-descriptions :column="2" label-placement="left" size="small" bordered class="kv">
        <n-descriptions-item label="类型">{{ job.kind.toUpperCase() }}</n-descriptions-item>
        <n-descriptions-item label="执行来源">{{ sourceText }}</n-descriptions-item>
        <n-descriptions-item label="执行人">{{ job.operator_email }}</n-descriptions-item>
        <n-descriptions-item label="完成时间">{{ fmt(job.finished_at) }}</n-descriptions-item>
        <n-descriptions-item label="模板">{{ job.template_name || '—' }}</n-descriptions-item>
      </n-descriptions>

      <!-- approval trail -->
      <div class="sec">审批追溯</div>
      <div v-if="!approvals.length" class="apnone">未命中审批规则,直接执行。</div>
      <div v-for="a in approvals" :key="a.id" class="aprow">
        <div>
          <n-tag size="small" :bordered="false" :type="a.state === 'approved' ? 'success' : a.state === 'rejected' ? 'error' : 'warning'">
            {{ a.state === 'approved' ? '已放行' : a.state === 'rejected' ? '已驳回' : '待审批' }}
          </n-tag>
          <b style="margin-left:8px">{{ a.target_name || a.asset_id }}</b>
          <span v-if="a.decided_by" class="apmeta">{{ a.decided_by }} · {{ fmt(a.decided_at) }}</span>
          <span v-if="a.reason" class="apmeta">理由:{{ a.reason }}</span>
          <span v-if="a.min_approvals > 1" class="apmeta">会签 {{ (a.votes || []).filter(v => v.verdict === 'approve').length }}/{{ a.min_approvals }}</span>
        </div>
        <div v-if="(a.votes || []).length" class="votes">
          <span v-for="v in a.votes" :key="v.approver_id" class="vote">
            {{ v.verdict === 'approve' ? '✓' : '✗' }} {{ v.approver_email }} · {{ fmt(v.ts) }}
          </span>
        </div>
      </div>

      <!-- command -->
      <div class="sec">命令</div>
      <pre class="cmd">{{ job.command }}</pre>

      <!-- per-target results -->
      <div class="sec">逐目标结果({{ targets.length }})</div>
      <div v-for="t in targets" :key="t.id" class="trow" :class="targetCls(t.status)">
        <div class="thead">
          <span class="ticon">{{ targetIcon(t.status) }}</span>
          <b>{{ t.asset_name || t.asset_id }}</b>
          <span class="tmeta">{{ targetLabel(t.status) }}</span>
          <span v-if="t.exit_code != null" class="tmeta mono">exit {{ t.exit_code }}</span>
          <span v-if="t.duration_ms" class="tmeta mono">{{ t.duration_ms }} ms</span>
        </div>
        <div v-if="t.error" class="terr">{{ t.error }}</div>
        <pre v-if="t.stdout" class="tout">{{ t.stdout }}</pre>
        <pre v-if="t.stderr" class="tout warn">{{ t.stderr }}</pre>
      </div>

      <!-- audit & backup provenance -->
      <div class="sec">审计与备份</div>
      <div class="provenance" data-test="audit-backup">
        <div>审计记录已写入服务端数据库(audit 表),可在「执行记录 → 审计流水」检索与导出 CSV/JSON。</div>
        <div v-if="backup && backup.enabled">
          自动备份:每日 03:00 快照至本地 {{ backup.dir || 'data/backups' }} ·
          {{ backup.last_at ? '上次成功 ' + fmt(backup.last_at) : '尚未执行' }} ·
          保留 {{ backup.retention_days }} 天
        </div>
        <div>本页含固定任务 id,可打印留存。</div>
      </div>
    </template>
  </div>
</template>

<style scoped>
.record { max-width: 860px; margin: 0 auto; }
.rhead { display: flex; align-items: flex-start; justify-content: space-between; margin-bottom: 14px; }
.rid { font-family: monospace; font-size: 11px; color: var(--muted); letter-spacing: .04em; }
.rtitle { margin: 4px 0 0; font-size: 20px; font-family: monospace; }

.verdict { display: flex; gap: 12px; align-items: flex-start; padding: 14px 16px; border-radius: 8px;
  border: 1px solid transparent; margin-bottom: 16px; }
.verdict .vicon { font-size: 22px; line-height: 1.2; }
.verdict.ok { background: rgba(62,207,142,.12); border-color: rgba(62,207,142,.4); color: var(--success); }
.verdict.partial { background: rgba(224,168,62,.12); border-color: rgba(224,168,62,.4); color: var(--warn); }
.verdict.fail { background: rgba(229,100,95,.12); border-color: rgba(229,100,95,.4); color: var(--danger); }
.verdict.pending { background: var(--surface-warm); border-color: rgba(255,255,255,.1); color: var(--muted); }
.vfail { font-size: 12px; margin-top: 4px; opacity: .9; }

.kv { margin-bottom: 4px; }
.sec { margin: 18px 0 8px; font-size: 12px; font-weight: 600; color: var(--muted); letter-spacing: .05em; }
.apnone { font-size: 13px; color: var(--muted); }
.aprow { border: 1px solid rgba(255,255,255,.08); border-radius: 6px; padding: 8px 10px; margin-bottom: 8px; }
.apmeta { margin-left: 10px; font-size: 12px; color: var(--muted); }
.votes { margin-top: 6px; display: flex; flex-wrap: wrap; gap: 10px; }
.vote { font-size: 12px; color: var(--muted); font-family: monospace; }

.cmd { white-space: pre-wrap; font-family: monospace; font-size: 13px; background: var(--bg);
  padding: 12px; border-radius: 8px; margin: 0; }

.trow { border: 1px solid rgba(255,255,255,.08); border-radius: 8px; padding: 10px 12px; margin-bottom: 10px; }
.trow.fail { border-color: rgba(229,100,95,.45); background: rgba(229,100,95,.06); }
.thead { display: flex; align-items: center; gap: 8px; }
.ticon { font-weight: 700; }
.trow.ok .ticon { color: var(--success); }
.trow.fail .ticon { color: var(--danger); }
.trow.pending .ticon { color: var(--warn); }
.tmeta { font-size: 12px; color: var(--muted); }
.mono { font-family: monospace; }
.terr { color: var(--danger); font-size: 12px; margin-top: 6px; }
.tout { white-space: pre-wrap; font-family: monospace; font-size: 12px; margin: 8px 0 0;
  background: var(--bg); padding: 8px 10px; border-radius: 6px; max-height: 280px; overflow: auto; }
.tout.warn { color: var(--warn); }
.provenance { border: 1px dashed rgba(255,255,255,.15); border-radius: 8px; padding: 10px 12px;
  font-size: 12px; color: var(--muted); display: flex; flex-direction: column; gap: 4px; }
</style>

<style>
/* archive printing: keep only the record body */
@media print {
  .n-layout-sider, .hdr, .noprint { display: none !important; }
  .n-layout, .n-layout-content { position: static !important; background: #fff !important; }
  .record { max-width: 100%; color: #000; }
  .record .tout, .record .cmd { max-height: none; background: #f5f5f5; color: #000; }
}
</style>
