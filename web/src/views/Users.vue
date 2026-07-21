<script setup>
import { ref, onMounted, h } from 'vue';
import { NButton, NPopconfirm, NTag, useMessage } from 'naive-ui';
import { api } from '../api';

const message = useMessage();
const users = ref([]);
const loading = ref(false);

const roleOpts = [
  { label: '管理员', value: 'admin' },
  { label: '操作员', value: 'operator' },
  { label: '只读', value: 'viewer' },
];
const roleLabel = { admin: '管理员', operator: '操作员', viewer: '只读' };
const roleType = { admin: 'error', operator: 'info', viewer: 'default' };

async function load() {
  loading.value = true;
  try { users.value = await api.users(); } catch (e) { message.error('加载失败(需 admin)'); }
  finally { loading.value = false; }
}
onMounted(load);

// ---- create / edit ----
const show = ref(false);
const saving = ref(false);
const form = ref(null);
function openNew() { form.value = { id: null, name: '', email: '', role: 'viewer', password: '' }; show.value = true; }
function openEdit(u) { form.value = { id: u.id, name: u.name, email: u.email || '', role: u.role, password: '' }; show.value = true; }
async function save() {
  const f = form.value;
  if (!f.name.trim()) { message.warning('请填用户名'); return; }
  if (!f.id && f.password.length < 6) { message.warning('密码至少 6 位'); return; }
  saving.value = true;
  try {
    if (f.id) await api.updateUser(f.id, { name: f.name, email: f.email, role: f.role });
    else await api.createUser({ name: f.name, email: f.email, role: f.role, password: f.password });
    message.success('已保存');
    show.value = false;
    await load();
  } catch (e) { message.error(e?.response?.data?.error || '保存失败'); }
  finally { saving.value = false; }
}
async function remove(u) {
  try { await api.deleteUser(u.id); message.success('已删除'); await load(); }
  catch (e) { message.error(e?.response?.data?.error || '删除失败'); }
}

// ---- reset password ----
const showPw = ref(false);
const pwTarget = ref(null);
const newPw = ref('');
function openReset(u) { pwTarget.value = u; newPw.value = ''; showPw.value = true; }
async function doReset() {
  if (newPw.value.length < 6) { message.warning('密码至少 6 位'); return; }
  try { await api.resetUserPassword(pwTarget.value.id, newPw.value); message.success('密码已重置'); showPw.value = false; }
  catch (e) { message.error(e?.response?.data?.error || '重置失败'); }
}

const columns = [
  { title: '用户名', key: 'name' },
  { title: '邮箱', key: 'email', render: (u) => u.email || '—' },
  { title: '角色', key: 'role', width: 100,
    render: (u) => h(NTag, { size: 'small', bordered: false, type: roleType[u.role] || 'default' }, { default: () => roleLabel[u.role] || u.role }) },
  { title: '两步验证', key: 'totp_enabled', width: 100, render: (u) => (u.totp_enabled ? '已开启' : '—') },
  { title: '操作', key: 'ops', width: 200,
    render: (u) => h('div', { style: 'display:flex;gap:8px' }, [
      h(NButton, { size: 'tiny', onClick: () => openEdit(u) }, { default: () => '编辑' }),
      h(NButton, { size: 'tiny', onClick: () => openReset(u) }, { default: () => '重置密码' }),
      h(NPopconfirm, { onPositiveClick: () => remove(u) }, {
        trigger: () => h(NButton, { size: 'tiny', type: 'error', tertiary: true }, { default: () => '删除' }),
        default: () => `确定删除「${u.name}」?`,
      }),
    ]) },
];
</script>

<template>
  <n-card title="用户与权限" size="small">
    <template #header-extra>
      <n-space>
        <n-button size="small" type="primary" @click="openNew">+ 新建用户</n-button>
        <n-button size="small" @click="load" :loading="loading">刷新</n-button>
      </n-space>
    </template>
    <n-data-table :columns="columns" :data="users" size="small" :bordered="false" :loading="loading" />

    <n-modal v-model:show="show" preset="card" :title="form?.id ? '编辑用户' : '新建用户'" style="width:460px">
      <n-form v-if="form" label-placement="left" :label-width="80">
        <n-form-item label="用户名"><n-input v-model:value="form.name" :disabled="!!form.id" /></n-form-item>
        <n-form-item label="邮箱"><n-input v-model:value="form.email" /></n-form-item>
        <n-form-item label="角色"><n-select v-model:value="form.role" :options="roleOpts" style="max-width:160px" /></n-form-item>
        <n-form-item v-if="!form.id" label="密码"><n-input v-model:value="form.password" type="password" show-password-on="click" placeholder="至少 6 位" /></n-form-item>
      </n-form>
      <template #footer><n-button type="primary" :loading="saving" @click="save">保存</n-button></template>
    </n-modal>

    <n-modal v-model:show="showPw" preset="card" title="重置密码" style="width:400px">
      <n-form-item :label="`新密码`" label-placement="left" :label-width="70">
        <n-input v-model:value="newPw" type="password" show-password-on="click" placeholder="至少 6 位" @keyup.enter="doReset" />
      </n-form-item>
      <template #footer><n-button type="primary" @click="doReset">重置</n-button></template>
    </n-modal>
  </n-card>
</template>
