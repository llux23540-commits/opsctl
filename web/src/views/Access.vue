<script setup>
import { ref, onMounted, computed, h } from 'vue';
import { NButton, NPopconfirm, useMessage } from 'naive-ui';
import { api } from '../api';

const message = useMessage();
const rules = ref([]);
const users = ref([]);
const tags = ref([]);
const accounts = ref([]);
const showForm = ref(false);
const saving = ref(false);
const editingId = ref(null);

function blankForm() {
  return {
    name: '',
    subject_user_id: null,
    selector_kind: 'tag',
    selector: null,
    system_user_id: null,
    actions: ['ssh'],
    needs_approval: false,
    min_approvals: 1,
    approver_ids: [],
    quick: 'console',
  };
}
const form = ref(blankForm());

function openNew() {
  editingId.value = null;
  form.value = blankForm();
  showForm.value = true;
}
function openEdit(row) {
  editingId.value = row.id;
  form.value = {
    name: row.name,
    subject_user_id: row.subject_user_id,
    selector_kind: row.selector_kind,
    selector: row.selector,
    system_user_id: row.system_user_id || null,
    actions: String(row.actions || '').split(',').filter(Boolean),
    needs_approval: !!row.needs_approval,
    min_approvals: row.min_approvals || 1,
    approver_ids: String(row.approver_ids || '').split(',').filter(Boolean),
    quick: row.quick || 'console',
  };
  showForm.value = true;
}
async function removeRule(row) {
  try {
    await api.deleteRule(row.id);
    message.success('已删除');
    await load();
  } catch (e) {
    message.error(e?.response?.data?.error || '删除失败');
  }
}

const userOpts = computed(() => users.value.map((u) => ({ label: `${u.name} (${u.role})`, value: u.id })));
const tagOpts = computed(() => tags.value.map((t) => ({ label: t.name, value: t.id })));
const accountOpts = computed(() => accounts.value.map((a) => ({ label: `${a.name} (${a.username})`, value: a.id })));
const actionOpts = [
  { label: 'SSH', value: 'ssh' },
  { label: 'SQL', value: 'sql' },
  { label: '上传', value: 'upload' },
];
const kindOpts = [
  { label: '按标签', value: 'tag' },
  { label: '按子树(节点id)', value: 'subtree' },
  { label: '按资产集(逗号分隔id)', value: 'assets' },
];
const quickOpts = [
  { label: '控制台登录批准(强认证)', value: 'console' },
  { label: 'TG 内联一键(演示)', value: 'tg' },
];

async function load() {
  try {
    [rules.value, users.value, tags.value, accounts.value] = await Promise.all([
      api.rules(), api.users(), api.tags(), api.accounts(),
    ]);
  } catch (e) {
    message.error('加载失败(需 admin)');
  }
}

const userName = (id) => users.value.find((u) => u.id === id)?.name || id;
const accountName = (id) => accounts.value.find((a) => a.id === id)?.name || id || '—';
const tagName = (id) => tags.value.find((t) => t.id === id)?.name || id;

const columns = [
  { title: '名称', key: 'name' },
  { title: '主体', key: 'subject_user_id', render: (r) => userName(r.subject_user_id) },
  { title: '资产选择', key: 'sel',
    render: (r) => (r.selector_kind === 'tag' ? `标签: ${tagName(r.selector)}` : `${r.selector_kind}: ${r.selector}`) },
  { title: '账号', key: 'system_user_id', render: (r) => accountName(r.system_user_id) },
  { title: '动作', key: 'actions' },
  { title: '需审批', key: 'needs_approval', width: 90,
    render: (r) => (r.needs_approval ? (r.min_approvals > 1 ? `是·会签${r.min_approvals}` : '是') : '否') },
  { title: '操作', key: 'ops', width: 130,
    render: (r) =>
      h('div', { style: 'display:flex;gap:8px' }, [
        h(NButton, { size: 'tiny', onClick: () => openEdit(r) }, { default: () => '编辑' }),
        h(
          NPopconfirm,
          { onPositiveClick: () => removeRule(r), positiveText: '删除', negativeText: '取消' },
          {
            trigger: () => h(NButton, { size: 'tiny', type: 'error', tertiary: true }, { default: () => '删除' }),
            default: () => `确定删除规则「${r.name}」?`,
          }
        ),
      ]) },
];

async function save() {
  if (!form.value.subject_user_id || !form.value.selector) {
    message.warning('请填主体与资产选择');
    return;
  }
  saving.value = true;
  try {
    const body = {
      name: form.value.name || '规则',
      subject_user_id: form.value.subject_user_id,
      selector_kind: form.value.selector_kind,
      selector: form.value.selector,
      system_user_id: form.value.system_user_id || '',
      actions: form.value.actions,
      needs_approval: form.value.needs_approval,
      min_approvals: form.value.needs_approval ? Number(form.value.min_approvals) || 1 : 1,
      approver_ids: form.value.needs_approval ? form.value.approver_ids : [],
      quick: form.value.needs_approval ? form.value.quick || 'console' : 'console',
    };
    if (editingId.value) await api.updateRule(editingId.value, body);
    else await api.createRule(body);
    message.success('已保存');
    showForm.value = false;
    await load();
  } catch (e) {
    message.error(e?.response?.data?.error || '保存失败');
  } finally {
    saving.value = false;
  }
}

onMounted(load);
</script>

<template>
  <n-card title="资产授权规则" size="small">
    <template #header-extra>
      <n-button type="primary" size="small" @click="openNew">+ 新建规则</n-button>
    </template>
    <n-data-table :columns="columns" :data="rules" :bordered="false" size="small" />

    <n-modal v-model:show="showForm" preset="card" :title="editingId ? '编辑授权规则' : '新建授权规则(五槽位)'" style="width:560px">
      <n-form label-placement="left" :label-width="90">
        <n-form-item label="名称"><n-input v-model:value="form.name" /></n-form-item>
        <n-form-item label="主体"><n-select v-model:value="form.subject_user_id" :options="userOpts" placeholder="选用户" /></n-form-item>
        <n-form-item label="资产维度"><n-select v-model:value="form.selector_kind" :options="kindOpts" /></n-form-item>
        <n-form-item label="资产选择">
          <n-select v-if="form.selector_kind === 'tag'" v-model:value="form.selector" :options="tagOpts" placeholder="选标签" />
          <n-input v-else v-model:value="form.selector" :placeholder="form.selector_kind === 'subtree' ? '站点/节点 id' : '资产 id,逗号分隔'" />
        </n-form-item>
        <n-form-item label="账号"><n-select v-model:value="form.system_user_id" :options="accountOpts" placeholder="连接用系统用户" clearable /></n-form-item>
        <n-form-item label="动作"><n-checkbox-group v-model:value="form.actions"><n-space><n-checkbox v-for="o in actionOpts" :key="o.value" :value="o.value" :label="o.label" /></n-space></n-checkbox-group></n-form-item>
        <n-form-item label="需审批"><n-switch v-model:value="form.needs_approval" /></n-form-item>
        <n-form-item v-if="form.needs_approval" label="会签人数">
          <n-input-number v-model:value="form.min_approvals" :min="1" :max="9" style="width:120px" />
          <span style="margin-left:10px;color:var(--muted);font-size:12px">需 N 个批准才放行</span>
        </n-form-item>
        <n-form-item v-if="form.needs_approval" label="指定审批人">
          <n-select v-model:value="form.approver_ids" :options="userOpts" multiple clearable placeholder="留空=任意管理员" />
        </n-form-item>
        <n-form-item v-if="form.needs_approval" label="审核方式">
          <n-select v-model:value="form.quick" :options="quickOpts" />
        </n-form-item>
      </n-form>
      <template #footer>
        <n-button type="primary" :loading="saving" @click="save">保存</n-button>
      </template>
    </n-modal>
  </n-card>
</template>
