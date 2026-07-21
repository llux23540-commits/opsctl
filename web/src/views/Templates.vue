<script setup>
import { ref, onMounted, computed } from 'vue';
import { NButton, NPopconfirm, NTag, useMessage } from 'naive-ui';
import { api } from '../api';

const message = useMessage();
const list = ref([]);
const editing = ref(null); // current template being edited
const saving = ref(false);
const users = ref([]);
const approverOptions = computed(() =>
  users.value.map((u) => ({ label: `${u.name} (${u.role})`, value: u.id })));

const BUILTIN = {
  ssh: ['target.name', 'target.host', 'target.port', 'operator', 'now'],
  sql: ['target.name', 'operator', 'now'],
  doc: [],
  pipeline: [],
};

async function load() {
  try { list.value = await api.templates(); } catch (e) { message.error('加载模板失败'); }
}
onMounted(async () => {
  await load();
  try { users.value = await api.users(); } catch (e) { /* page is admin-only; ignore */ }
});

// top-level templates (not a pipeline step) for the left list
const topLevel = computed(() => list.value.filter((t) => !t.parent_id));
function stepsOf(pid) {
  return list.value.filter((t) => t.parent_id === pid).sort((a, b) => (a.sort || 0) - (b.sort || 0));
}
const editingSteps = computed(() => (editing.value?.id ? stepsOf(editing.value.id) : []));
const parentOf = computed(() => (editing.value?.parent_id ? list.value.find((t) => t.id === editing.value.parent_id) : null));

function blank() {
  return { id: null, name: '', kind: 'ssh', command: '', variables: [], approver_ids: [], parent_id: null, sort: 0 };
}
function edit(t) {
  editing.value = {
    id: t.id, name: t.name, kind: t.kind, command: t.command,
    variables: JSON.parse(t.variables || '[]'),
    approver_ids: (t.approver_ids || '').split(',').filter(Boolean),
    parent_id: t.parent_id || null, sort: t.sort || 0,
  };
}
function create() { editing.value = blank(); }

function insertVar(v) {
  if (!editing.value) return;
  editing.value.command += `{{${v}}}`;
}
function addCustomVar() { editing.value.variables.push({ name: '', default: '' }); }
function removeVar(i) { editing.value.variables.splice(i, 1); }

const preview = computed(() => {
  if (!editing.value) return '';
  let s = editing.value.command;
  const demo = { 'target.name': 'web-01', 'target.host': '10.0.0.1', 'target.port': '22',
    operator: 'admin', now: '2026-07-04 10:00' };
  editing.value.variables.forEach((v) => { if (v.name) s = s.replaceAll(`{{${v.name}}}`, v.default || `<${v.name}>`); });
  Object.entries(demo).forEach(([k, val]) => { s = s.replaceAll(`{{${k}}}`, val); });
  return s;
});

// low-level save of a raw template row (reorder steps without opening the editor)
async function saveRaw(t, overrides = {}) {
  await api.saveTemplate({
    id: t.id, name: t.name, kind: t.kind, command: t.command,
    variables: JSON.parse(t.variables || '[]'),
    approver_ids: (t.approver_ids || '').split(',').filter(Boolean),
    parent_id: t.parent_id || null, sort: t.sort || 0, ...overrides,
  });
}

async function save() {
  const e = editing.value;
  if (!e.name.trim()) { message.warning('模板名不能为空'); return; }
  saving.value = true;
  try {
    await api.saveTemplate({
      id: e.id || undefined, name: e.name, kind: e.kind,
      command: e.kind === 'pipeline' ? '' : e.command,
      variables: e.kind === 'pipeline' ? [] : e.variables.filter((v) => v.name),
      approver_ids: e.approver_ids, parent_id: e.parent_id || null, sort: e.sort || 0,
    });
    message.success('已保存');
    editing.value = null;
    await load();
  } catch (e2) {
    message.error(e2?.response?.data?.error || '保存失败');
  } finally { saving.value = false; }
}
async function remove(t) {
  try { await api.deleteTemplate(t.id); message.success('已删除'); if (editing.value?.id === t.id) editing.value = null; await load(); }
  catch (e) { message.error('删除失败'); }
}

// ---- pipeline orchestration ----
async function addStep(pipeline) {
  const sort = editingSteps.value.length;
  try {
    await api.saveTemplate({ name: `步骤 ${sort + 1}`, kind: 'ssh', command: '', variables: [],
      approver_ids: [], parent_id: pipeline.id, sort });
    await load();
  } catch (e) { message.error('新增子任务失败'); }
}
async function moveStep(i, dir) {
  const steps = editingSteps.value;
  const j = i + dir;
  if (j < 0 || j >= steps.length) return;
  const a = steps[i], b = steps[j];
  try {
    await saveRaw(a, { sort: b.sort });
    await saveRaw(b, { sort: a.sort });
    await load();
  } catch (e) { message.error('调整顺序失败'); }
}
async function removeStep(step) {
  try { await api.deleteTemplate(step.id); await load(); }
  catch (e) { message.error('删除子任务失败'); }
}
function gotoParent() {
  if (parentOf.value) edit(parentOf.value);
}
function editStep(step) { edit(step); }

const kindLabel = (k) => ({ ssh: 'SSH', sql: 'SQL', doc: '文档', pipeline: '编排' }[k] || k);
const kindTag = (k) => ({ ssh: 'info', sql: 'warning', doc: 'success', pipeline: 'error' }[k] || 'default');

// view the rendered git file (.sh/.sql/.md)
const fileView = ref(null);
async function viewFile(t) {
  try { fileView.value = await api.templateFile(t.id); }
  catch (e) { message.error('读取文件失败'); }
}
function downloadFile() {
  const f = fileView.value;
  const blob = new Blob([f.content], { type: 'text/plain' });
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url; a.download = f.filename; a.click();
  URL.revokeObjectURL(url);
}
async function revealFile() {
  if (!fileView.value) return;
  try { await api.gitReveal(fileView.value.path); }
  catch (e) { message.error(e?.response?.data?.error || '打开失败'); }
}
</script>

<template>
  <n-grid :cols="24" :x-gap="16" style="height:100%">
    <n-gi :span="8">
      <n-card title="模板列表" size="small">
        <template #header-extra><n-button size="tiny" type="primary" @click="create">+ 新建</n-button></template>
        <n-list hoverable clickable>
          <template v-for="t in topLevel" :key="t.id">
            <n-list-item @click="edit(t)">
              <n-thing>
                <template #header>
                  {{ t.name }}
                  <n-tag size="tiny" :bordered="false" :type="kindTag(t.kind)" style="margin-left:6px">{{ kindLabel(t.kind) }}</n-tag>
                  <n-tag v-if="t.kind === 'pipeline'" size="tiny" :bordered="false" style="margin-left:4px">{{ stepsOf(t.id).length }} 步</n-tag>
                </template>
                <template #description><span style="font-family:monospace;font-size:12px;color:var(--muted)">{{ t.command }}</span></template>
              </n-thing>
              <template #suffix>
                <n-space :size="6">
                  <n-button v-if="t.kind !== 'pipeline'" size="tiny" tertiary @click.stop="viewFile(t)">查看文件</n-button>
                  <n-popconfirm @positive-click="() => remove(t)">
                    <template #trigger><n-button size="tiny" tertiary type="error" @click.stop>删除</n-button></template>
                    确定删除「{{ t.name }}」?{{ t.kind === 'pipeline' ? '(其子任务需先移除)' : '' }}
                  </n-popconfirm>
                </n-space>
              </template>
            </n-list-item>
            <n-list-item v-for="(s, i) in stepsOf(t.id)" :key="s.id" class="step-item" @click="edit(s)">
              <n-thing>
                <template #header>
                  <span class="step-no">{{ i + 1 }}</span> {{ s.name }}
                  <n-tag size="tiny" :bordered="false" :type="kindTag(s.kind)" style="margin-left:6px">{{ kindLabel(s.kind) }}</n-tag>
                </template>
                <template #description><span style="font-family:monospace;font-size:12px;color:var(--muted)">{{ s.command }}</span></template>
              </n-thing>
            </n-list-item>
          </template>
        </n-list>
        <n-empty v-if="!topLevel.length" description="暂无模板" style="margin-top:16px" />
      </n-card>
    </n-gi>

    <n-gi :span="16">
      <n-card :title="editing ? (editing.id ? '编辑模板' : '新建模板') : '模板编辑器'" size="small">
        <n-empty v-if="!editing" description="从左侧选择或新建模板" style="margin:40px 0" />
        <n-form v-else label-placement="left" :label-width="90">
          <div v-if="parentOf" class="pipebar">
            子任务 · 属于编排 <b>{{ parentOf.name }}</b>
            <n-button text size="small" type="primary" style="margin-left:auto" @click="gotoParent">↑ 跳转父任务</n-button>
          </div>
          <n-form-item label="名称"><n-input v-model:value="editing.name" style="max-width:320px" /></n-form-item>
          <n-form-item label="适用类型">
            <n-radio-group v-model:value="editing.kind" :disabled="!!parentOf">
              <n-radio-button value="ssh" label="服务器 SSH" />
              <n-radio-button value="sql" label="数据库 SQL" />
              <n-radio-button value="doc" label="文档 MD" />
              <n-radio-button v-if="!parentOf" value="pipeline" label="编排 Pipeline" />
            </n-radio-group>
          </n-form-item>

          <template v-if="editing.kind === 'pipeline'">
            <n-form-item label="子任务">
              <div style="width:100%">
                <div v-if="!editing.id" class="hint-box">先「保存」编排,再添加子任务。</div>
                <template v-else>
                  <div v-for="(s, i) in editingSteps" :key="s.id" class="steprow">
                    <span class="step-no">{{ i + 1 }}</span>
                    <n-tag size="tiny" :bordered="false" :type="kindTag(s.kind)">{{ kindLabel(s.kind) }}</n-tag>
                    <span class="step-name" @click="editStep(s)">{{ s.name }}</span>
                    <span class="step-cmd">{{ s.command }}</span>
                    <n-button size="tiny" tertiary :disabled="i === 0" title="上移" @click="moveStep(i, -1)">↑</n-button>
                    <n-button size="tiny" tertiary :disabled="i === editingSteps.length - 1" title="下移" @click="moveStep(i, 1)">↓</n-button>
                    <n-popconfirm positive-text="确定" negative-text="取消" @positive-click="() => removeStep(s)">
                      <template #trigger><n-button size="tiny" tertiary type="error" title="移出并删除">×</n-button></template>
                      移出并删除「{{ s.name }}」?
                    </n-popconfirm>
                  </div>
                  <n-empty v-if="!editingSteps.length" description="暂无子任务" size="small" style="margin:10px 0" />
                  <n-button size="tiny" dashed @click="addStep(editing)">+ 新增子任务</n-button>
                </template>
              </div>
            </n-form-item>
          </template>

          <template v-else>
            <n-form-item label="内置变量">
              <n-space>
                <n-tag v-for="v in BUILTIN[editing.kind]" :key="v" size="small" style="cursor:pointer" @click="insertVar(v)">+ {{ v }}</n-tag>
              </n-space>
            </n-form-item>
            <n-form-item label="命令">
              <n-input v-model:value="editing.command" type="textarea" :autosize="{ minRows: 2, maxRows: 6 }"
                placeholder="用 {{变量}} 占位" style="font-family:monospace" />
            </n-form-item>
            <n-form-item label="自定义变量">
              <div style="width:100%">
                <div v-for="(v, i) in editing.variables" :key="i" style="display:flex;gap:8px;margin-bottom:6px">
                  <n-input v-model:value="v.name" placeholder="变量名" size="small" style="max-width:160px" />
                  <n-input v-model:value="v.default" placeholder="默认值" size="small" style="max-width:200px" />
                  <n-button size="small" tertiary type="error" @click="removeVar(i)">移除</n-button>
                </div>
                <n-button size="tiny" dashed @click="addCustomVar">+ 变量</n-button>
              </div>
            </n-form-item>
            <n-form-item label="命令预览">
              <pre style="margin:0;white-space:pre-wrap;font-family:monospace;font-size:13px;color:var(--success);background:var(--bg);padding:8px 10px;border-radius:6px;width:100%">{{ preview }}</pre>
            </n-form-item>
          </template>

          <n-form-item label="审批人">
            <n-select v-model:value="editing.approver_ids" multiple clearable :options="approverOptions"
              placeholder="指定审批人(空 = 任意管理员)" style="max-width:420px" />
          </n-form-item>
          <n-form-item label=" ">
            <n-space>
              <n-button type="primary" :loading="saving" @click="save">保存</n-button>
              <n-button @click="editing = null">取消</n-button>
            </n-space>
          </n-form-item>
        </n-form>
      </n-card>
    </n-gi>
  </n-grid>

  <!-- outside the n-grid: grid drops non-Gi children, so a modal inside it never renders -->
  <n-modal :show="!!fileView" preset="card" :title="fileView?.filename" style="width:640px" @update:show="(v) => { if (!v) fileView = null }">
    <n-text depth="3" style="font-size:12px">git 路径:{{ fileView?.path }}(同步后提交到仓库)</n-text>
    <br v-if="fileView?.abs_path" />
    <n-text v-if="fileView?.abs_path" depth="3" style="font-size:12px">磁盘位置:{{ fileView.abs_path }}<template v-if="!fileView.exists">(尚未同步)</template></n-text>
    <pre style="margin:10px 0 0;white-space:pre-wrap;font-family:monospace;font-size:13px;background:var(--bg);color:var(--fg);padding:12px;border-radius:6px;max-height:52vh;overflow:auto">{{ fileView?.content }}</pre>
    <template #footer>
      <n-space>
        <n-button type="primary" @click="downloadFile">下载文件</n-button>
        <n-tooltip v-if="fileView?.abs_path" :disabled="fileView?.exists" trigger="hover">
          <template #trigger>
            <n-button :disabled="!fileView?.exists" @click="revealFile">打开所在位置</n-button>
          </template>
          尚未同步:请先在「设置 → Git 同步」执行一次同步
        </n-tooltip>
        <n-button @click="fileView = null">关闭</n-button>
      </n-space>
    </template>
  </n-modal>
</template>

<style scoped>
.step-item { padding-left: 22px; background: color-mix(in oklab, var(--surface-warm), transparent 40%); }
.step-no { display: inline-grid; place-items: center; width: 18px; height: 18px; border-radius: 5px;
  background: var(--surface-warm); font-size: 11px; color: var(--muted); margin-right: 4px; }
.pipebar { display: flex; align-items: center; gap: 6px; padding: 8px 12px; margin-bottom: 12px;
  background: var(--surface-warm); border-radius: 8px; font-size: 13px; }
.hint-box { color: var(--muted); font-size: 13px; padding: 8px 0; }
.steprow { display: flex; align-items: center; gap: 8px; padding: 6px 8px; margin-bottom: 6px;
  border: 1px solid rgba(255,255,255,.08); border-radius: 8px; }
.steprow .step-name { font-weight: 500; cursor: pointer; }
.steprow .step-name:hover { color: var(--accent); }
.steprow .step-cmd { flex: 1; font-family: monospace; font-size: 12px; color: var(--muted);
  overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
</style>
