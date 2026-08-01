<script setup>
import { ref, onMounted, computed, h } from 'vue';
import { NButton, NPopconfirm, NTag, useMessage } from 'naive-ui';
import { api } from '../api';

const message = useMessage();
const tab = ref('assets');

// view the SSH node's git file (config encrypted in git; this view masks secrets)
const nodeFile = ref(null);
async function viewNodeFile(id) {
  try { nodeFile.value = await api.assetFile(id); }
  catch (e) { message.error('读取节点文件失败'); }
}
async function revealNodeFile() {
  if (!nodeFile.value) return;
  try { await api.gitReveal(nodeFile.value.path); }
  catch (e) { message.error(e?.response?.data?.error || '打开失败'); }
}

const assets = ref([]);
const accounts = ref([]);
const tags = ref([]);

async function load() {
  try {
    [assets.value, accounts.value, tags.value] = await Promise.all([
      api.assets(), api.accounts(), api.tags(),
    ]);
  } catch (e) {
    message.error('加载失败(需 admin)');
  }
}
onMounted(load);

function actionButtons(onEdit, onDelete) {
  return (row) =>
    h('div', { style: 'display:flex;gap:8px' }, [
      h(NButton, { size: 'tiny', onClick: () => onEdit(row) }, { default: () => '编辑' }),
      h(
        NPopconfirm,
        { onPositiveClick: () => onDelete(row), positiveText: '删除', negativeText: '取消' },
        {
          trigger: () => h(NButton, { size: 'tiny', type: 'error', tertiary: true }, { default: () => '删除' }),
          default: () => `确定删除「${row.name}」?`,
        }
      ),
    ]);
}

function apiErr(e, fallback) {
  message.error(e?.response?.data?.error || fallback);
}

// ---------- 资产 ----------
const showAsset = ref(false);
const savingAsset = ref(false);
const assetForm = ref(null);

// 测试连通:server → TCP 探测,database → 文件可访问,不涉及账密
const probing = ref(false);
const probeResult = ref(null); // { ok, message, latency_ms }
async function testConn() {
  const f = assetForm.value;
  if (!f.host || !f.host.trim()) { probeResult.value = { ok: false, message: '请先填写主机 / 地址' }; return; }
  probing.value = true;
  probeResult.value = null;
  try {
    probeResult.value = await api.probeAsset({ kind: f.kind, host: f.host, port: f.port || 0 });
  } catch (e) {
    probeResult.value = { ok: false, message: e?.response?.data?.error || '探测失败' };
  } finally { probing.value = false; }
}
const kindOpts = [
  { label: '站点(分组)', value: 'site' },
  { label: '服务器', value: 'server' },
  { label: '数据库', value: 'database' },
];
const siteOpts = computed(() =>
  assets.value.filter((a) => a.kind === 'site').map((a) => ({ label: a.name, value: a.id })));
const tagOpts = computed(() => tags.value.map((t) => ({ label: t.name, value: t.id })));
const accountOpts = computed(() =>
  accounts.value.map((a) => ({ label: `${a.name} (${a.username})`, value: a.id })));

const envOpts = [
  { label: '— 无 —', value: '' },
  { label: '生产 prod', value: 'prod' },
  { label: '预发 staging', value: 'staging' },
  { label: '开发 dev', value: 'dev' },
];
const envMeta = { prod: { label: 'prod', type: 'error' }, staging: { label: 'staging', type: 'warning' }, dev: { label: 'dev', type: 'info' } };
function blankAsset() {
  return { id: null, name: '', kind: 'server', parent_id: null, host: '', port: 22,
    status: 'enabled', env: '', tag_ids: [], account_ids: [] };
}
function newAsset() { assetForm.value = blankAsset(); probeResult.value = null; showAsset.value = true; }
async function editAsset(row) {
  try {
    const d = await api.asset(row.id);
    assetForm.value = {
      id: d.asset.id, name: d.asset.name, kind: d.asset.kind, parent_id: d.asset.parent_id,
      host: d.asset.host, port: d.asset.port, status: d.asset.status, env: d.asset.env || '',
      tag_ids: d.tag_ids, account_ids: d.account_ids,
    };
    probeResult.value = null;
    showAsset.value = true;
  } catch (e) { apiErr(e, '加载资产详情失败'); }
}
async function saveAsset() {
  const f = assetForm.value;
  if (!f.name) { message.warning('请填名称'); return; }
  savingAsset.value = true;
  try {
    if (f.id) {
      await api.updateAsset(f.id, { name: f.name, kind: f.kind, parent_id: f.parent_id,
        host: f.host, port: f.port, status: f.status, env: f.env, tag_ids: f.tag_ids, account_ids: f.account_ids });
    } else {
      const created = await api.createAsset({ name: f.name, kind: f.kind, parent_id: f.parent_id,
        host: f.host, port: f.port, status: f.status, env: f.env, tag_ids: f.tag_ids });
      // create endpoint binds a single account; bind the chosen set via update
      if (f.account_ids.length) await api.updateAsset(created.id, { name: f.name, kind: f.kind,
        parent_id: f.parent_id, host: f.host, port: f.port, status: f.status, env: f.env, account_ids: f.account_ids });
    }
    message.success('已保存');
    showAsset.value = false;
    await load();
  } catch (e) { apiErr(e, '保存失败'); } finally { savingAsset.value = false; }
}
async function removeAsset(row) {
  try { await api.deleteAsset(row.id); message.success('已删除'); await load(); }
  catch (e) { apiErr(e, '删除失败'); }
}
// inline enable/disable toggle (omitting tag_ids/account_ids keeps them unchanged)
async function toggleAssetStatus(row) {
  const next = row.status === 'enabled' ? 'disabled' : 'enabled';
  try {
    await api.updateAsset(row.id, { name: row.name, kind: row.kind, parent_id: row.parent_id,
      host: row.host, port: row.port, status: next });
    message.success(next === 'enabled' ? '已启用' : '已停用');
    await load();
  } catch (e) { apiErr(e, '操作失败'); }
}
const kindLabel = { site: '站点', server: '服务器', database: '数据库' };
const assetCols = [
  { title: '名称', key: 'name' },
  { title: '类型', key: 'kind', width: 90, render: (r) => kindLabel[r.kind] || r.kind },
  { title: '父级', key: 'parent_id', width: 130,
    render: (r) => assets.value.find((a) => a.id === r.parent_id)?.name || '—' },
  { title: '地址', key: 'host', render: (r) => (r.host ? `${r.host}:${r.port}` : '—') },
  { title: '状态', key: 'status', width: 80,
    render: (r) => h(NTag, { size: 'small', bordered: false, type: r.status === 'enabled' ? 'success' : 'default' },
      { default: () => (r.status === 'enabled' ? '启用' : '停用') }) },
  { title: '环境', key: 'env', width: 90,
    render: (r) => (envMeta[r.env]
      ? h(NTag, { size: 'small', bordered: false, type: envMeta[r.env].type }, { default: () => envMeta[r.env].label })
      : '—') },
  { title: '操作', key: 'ops', width: 190, render: (row) => h('div', { style: 'display:flex;gap:8px' }, [
    h(NButton, { size: 'tiny', onClick: () => toggleAssetStatus(row) },
      { default: () => (row.status === 'enabled' ? '停用' : '启用') }),
    h(NButton, { size: 'tiny', onClick: () => editAsset(row) }, { default: () => '编辑' }),
    h(NPopconfirm, { onPositiveClick: () => removeAsset(row), positiveText: '删除', negativeText: '取消' },
      { trigger: () => h(NButton, { size: 'tiny', type: 'error', tertiary: true }, { default: () => '删除' }),
        default: () => `确定删除「${row.name}」?` }),
  ]) },
];

// ---------- 账号 ----------
const showAccount = ref(false);
const savingAccount = ref(false);
const accountForm = ref(null);
const accountKindOpts = [
  { label: 'SSH 密码', value: 'ssh_pw' },
  { label: 'SSH 密钥', value: 'ssh_key' },
  { label: '数据库密码', value: 'db_pw' },
];
function newAccount() {
  accountForm.value = { id: null, name: '', kind: 'ssh_pw', username: '', secret: '' };
  showAccount.value = true;
}
function editAccount(row) {
  accountForm.value = { id: row.id, name: row.name, kind: row.kind, username: row.username, secret: '' };
  showAccount.value = true;
}
async function saveAccount() {
  const f = accountForm.value;
  if (!f.name || !f.username) { message.warning('请填名称与登录用户名'); return; }
  savingAccount.value = true;
  try {
    if (f.id) await api.updateAccount(f.id, { name: f.name, kind: f.kind, username: f.username, secret: f.secret });
    else await api.createAccount({ name: f.name, kind: f.kind, username: f.username, secret: f.secret });
    message.success('已保存');
    showAccount.value = false;
    await load();
  } catch (e) { apiErr(e, '保存失败'); } finally { savingAccount.value = false; }
}
async function removeAccount(row) {
  try { await api.deleteAccount(row.id); message.success('已删除'); await load(); }
  catch (e) { apiErr(e, '删除失败'); }
}
const accountKindLabel = { ssh_pw: 'SSH 密码', ssh_key: 'SSH 密钥', db_pw: '数据库密码' };
const accountCols = [
  { title: '名称', key: 'name' },
  { title: '类型', key: 'kind', width: 110, render: (r) => accountKindLabel[r.kind] || r.kind },
  { title: '登录用户名', key: 'username' },
  { title: '凭据', key: 'secret', width: 150,
    render: () => h('span', { style: 'display:inline-flex;align-items:center;gap:6px;color:var(--muted)' }, [
      '🔒 ••••••',
      h(NTag, { size: 'tiny', bordered: false, type: 'success' }, () => 'SOPS'),
    ]) },
  { title: '操作', key: 'ops', width: 130, render: actionButtons(editAccount, removeAccount) },
];

// ---------- 标签 ----------
const showTag = ref(false);
const savingTag = ref(false);
const tagForm = ref(null);
function newTag() { tagForm.value = { id: null, name: '', color: '#19b8a6' }; showTag.value = true; }
function editTag(row) { tagForm.value = { id: row.id, name: row.name, color: row.color || '#19b8a6' }; showTag.value = true; }
async function saveTag() {
  const f = tagForm.value;
  if (!f.name) { message.warning('请填名称'); return; }
  savingTag.value = true;
  try {
    if (f.id) await api.updateTag(f.id, { name: f.name, color: f.color });
    else await api.createTag({ name: f.name, color: f.color });
    message.success('已保存');
    showTag.value = false;
    await load();
  } catch (e) { apiErr(e, '保存失败'); } finally { savingTag.value = false; }
}
async function removeTag(row) {
  try { await api.deleteTag(row.id); message.success('已删除'); await load(); }
  catch (e) { apiErr(e, '删除失败'); }
}
const tagCols = [
  { title: '名称', key: 'name',
    render: (r) => h(NTag, { size: 'small', bordered: false, color: r.color ? { color: r.color + '22', textColor: r.color } : undefined },
      { default: () => r.name }) },
  { title: '颜色', key: 'color', width: 120 },
  { title: '使用', key: 'usage_count', width: 110,
    render: (r) => (r.usage_count
      ? h(NTag, { size: 'small', bordered: false, type: 'info' }, { default: () => `${r.usage_count} 个节点` })
      : h('span', { style: 'color:var(--muted)' }, '未使用')) },
  { title: '操作', key: 'ops', width: 130, render: actionButtons(editTag, removeTag) },
];
</script>

<template>
  <n-card size="small">
    <n-tabs v-model:value="tab" type="line">
      <n-tab-pane name="assets" tab="站点与节点">
        <div style="margin-bottom:12px">
          <n-button type="primary" size="small" @click="newAsset">+ 新建资产</n-button>
        </div>
        <n-data-table :columns="assetCols" :data="assets" :bordered="false" size="small" />
      </n-tab-pane>
      <n-tab-pane name="accounts" tab="系统账号">
        <div style="margin-bottom:12px">
          <n-button type="primary" size="small" @click="newAccount">+ 新建账号</n-button>
        </div>
        <n-data-table :columns="accountCols" :data="accounts" :bordered="false" size="small" />
      </n-tab-pane>
      <n-tab-pane name="tags" tab="标签">
        <div style="margin-bottom:12px">
          <n-button type="primary" size="small" @click="newTag">+ 新建标签</n-button>
        </div>
        <n-data-table :columns="tagCols" :data="tags" :bordered="false" size="small" />
      </n-tab-pane>
    </n-tabs>

    <n-modal v-model:show="showAsset" preset="card" :title="assetForm?.id ? '编辑资产' : '新建资产'" style="width:540px">
      <n-form v-if="assetForm" label-placement="left" :label-width="90">
        <n-form-item label="名称"><n-input v-model:value="assetForm.name" /></n-form-item>
        <n-form-item label="类型"><n-select v-model:value="assetForm.kind" :options="kindOpts" /></n-form-item>
        <n-form-item v-if="assetForm.kind !== 'site'" label="所属站点">
          <n-select v-model:value="assetForm.parent_id" :options="siteOpts" placeholder="选站点(可空)" clearable />
        </n-form-item>
        <template v-if="assetForm.kind !== 'site'">
          <n-form-item label="主机"><n-input v-model:value="assetForm.host" placeholder="IP 或域名" /></n-form-item>
          <n-form-item label="端口"><n-input-number v-model:value="assetForm.port" :min="1" :max="65535" /></n-form-item>
          <n-form-item label="标签"><n-select v-model:value="assetForm.tag_ids" :options="tagOpts" multiple clearable /></n-form-item>
          <n-form-item label="绑定账号"><n-select v-model:value="assetForm.account_ids" :options="accountOpts" multiple clearable /></n-form-item>
          <n-form-item label="连通性">
            <n-space align="center" :size="10">
              <n-button size="small" :loading="probing" @click="testConn">测试连通</n-button>
              <span v-if="probeResult" style="font-size:12px;font-family:monospace"
                :style="{ color: probeResult.ok ? 'var(--success)' : 'var(--danger)' }">
                {{ probeResult.ok ? '✓' : '✗' }} {{ probeResult.message }}<template v-if="probeResult.ok && probeResult.latency_ms != null"> · {{ probeResult.latency_ms }} ms</template>
              </span>
              <n-text v-else depth="3" style="font-size:12px">server 走 TCP 探测,database 校验文件可访问,不涉及账密</n-text>
            </n-space>
          </n-form-item>
        </template>
        <n-form-item label="状态">
          <n-switch v-model:value="assetForm.status" checked-value="enabled" unchecked-value="disabled">
            <template #checked>启用</template><template #unchecked>停用</template>
          </n-switch>
        </n-form-item>
        <n-form-item label="环境">
          <n-select v-model:value="assetForm.env" :options="envOpts" style="max-width:200px" />
          <n-text depth="3" style="font-size:12px;margin-left:10px">站点标 prod,审批时高亮提示;节点留空则继承站点</n-text>
        </n-form-item>
      </n-form>
      <template #footer>
        <n-space>
          <n-button type="primary" :loading="savingAsset" @click="saveAsset">保存</n-button>
          <n-button v-if="assetForm?.id && assetForm.kind === 'server'" @click="viewNodeFile(assetForm.id)">查看 git 文件</n-button>
        </n-space>
      </template>
    </n-modal>

    <n-modal :show="!!nodeFile" preset="card" :title="nodeFile?.filename" style="width:600px" @update:show="(v) => { if (!v) nodeFile = null }">
      <n-text depth="3" style="font-size:12px">git 路径:{{ nodeFile?.path }}(同步后正文为密文;此处密码已打码)</n-text>
      <br v-if="nodeFile?.abs_path" />
      <n-text v-if="nodeFile?.abs_path" depth="3" style="font-size:12px">磁盘位置:{{ nodeFile.abs_path }}<template v-if="!nodeFile.exists">(尚未同步)</template></n-text>
      <pre style="margin:10px 0 0;white-space:pre-wrap;font-family:monospace;font-size:13px;background:var(--bg);color:var(--fg);padding:12px;border-radius:6px;max-height:52vh;overflow:auto">{{ nodeFile?.content }}</pre>
      <template #footer>
        <n-space justify="end">
          <n-tooltip :disabled="nodeFile?.exists" trigger="hover">
            <template #trigger>
              <n-button :disabled="!nodeFile?.exists" @click="revealNodeFile">打开所在位置</n-button>
            </template>
            尚未同步:请先在「设置 → Git 同步」执行一次同步
          </n-tooltip>
          <n-button @click="nodeFile = null">关闭</n-button>
        </n-space>
      </template>
    </n-modal>

    <n-modal v-model:show="showAccount" preset="card" :title="accountForm?.id ? '编辑账号' : '新建账号'" style="width:480px">
      <n-form v-if="accountForm" label-placement="left" :label-width="100">
        <n-form-item label="名称"><n-input v-model:value="accountForm.name" placeholder="如 web-ssh" /></n-form-item>
        <n-form-item label="类型"><n-select v-model:value="accountForm.kind" :options="accountKindOpts" /></n-form-item>
        <!-- 目标机器的登录账号,不是本平台账号:挡掉浏览器灌入 opsctl 的登录口令 -->
        <n-form-item label="登录用户名">
          <n-input v-model:value="accountForm.username"
            :input-props="{ name: 'sys-account-user', autocomplete: 'off' }" />
        </n-form-item>
        <n-form-item label="密码/密钥">
          <n-input v-model:value="accountForm.secret" type="password" show-password-on="click"
            :input-props="{ name: 'sys-account-secret', autocomplete: 'new-password' }"
            :placeholder="accountForm.id ? '留空则不修改' : ''" />
        </n-form-item>
      </n-form>
      <template #footer>
        <n-button type="primary" :loading="savingAccount" @click="saveAccount">保存</n-button>
      </template>
    </n-modal>

    <n-modal v-model:show="showTag" preset="card" :title="tagForm?.id ? '编辑标签' : '新建标签'" style="width:420px">
      <n-form v-if="tagForm" label-placement="left" :label-width="70">
        <n-form-item label="名称"><n-input v-model:value="tagForm.name" /></n-form-item>
        <n-form-item label="颜色"><n-color-picker v-model:value="tagForm.color" :show-alpha="false" /></n-form-item>
      </n-form>
      <template #footer>
        <n-button type="primary" :loading="savingTag" @click="saveTag">保存</n-button>
      </template>
    </n-modal>
  </n-card>
</template>
