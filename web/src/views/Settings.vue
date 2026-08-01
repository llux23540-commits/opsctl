<script setup>
import { ref, onMounted, computed, h, watch, nextTick } from 'vue';
import { useRoute } from 'vue-router';
import { NButton, NPopconfirm, NTag, useMessage } from 'naive-ui';
import { api } from '../api';
import { useAuth } from '../store/auth';

const message = useMessage();
const auth = useAuth();

// deep-link: /settings#sessions(头像菜单「我的会话与设备」)、/settings#vault
// (其它页面「前往解封」)滚动到对应卡片并闪一下,让落点显而易见。
const route = useRoute();
const sessionsCard = ref(null);
const vaultCard = ref(null);
const flashKey = ref('');
async function scrollToHash() {
  const key = route.hash.replace('#', '');
  const target = { sessions: sessionsCard, vault: vaultCard }[key];
  if (!target) return;
  flashKey.value = key;
  // 卡片内容(会话表 / git 配置 / 金库状态)都是异步载入的,首帧滚动会落空,
  // 所以内容稳定后再滚一次。
  for (const delay of [0, 400]) {
    await nextTick();
    if (delay) await new Promise((r) => setTimeout(r, delay));
    const el = target.value?.$el || target.value;
    if (el) el.scrollIntoView({ behavior: 'smooth', block: 'start' });
  }
  setTimeout(() => { flashKey.value = ''; }, 1800);
}
watch(() => route.hash, scrollToHash);

// ---- 个人信息 ----
const profile = ref({ name: '', email: '', login_ttl_secs: 604800, login_alert: true, telegram_bound: false });
const savingProfile = ref(false);
const ttlOptions = [
  { label: '1 天', value: 86400 },
  { label: '7 天', value: 604800 },
  { label: '14 天', value: 1209600 },
  { label: '30 天', value: 2592000 },
];

async function loadProfile() {
  try { profile.value = await api.profile(); } catch (e) { message.error('加载个人信息失败'); }
}
async function saveProfile() {
  if (!profile.value.name.trim()) { message.warning('显示名不能为空'); return; }
  savingProfile.value = true;
  try {
    await api.updateProfile({
      name: profile.value.name, email: profile.value.email,
      login_ttl_secs: profile.value.login_ttl_secs, login_alert: profile.value.login_alert,
    });
    message.success('已保存');
    // reflect the new display name in the header immediately
    if (auth.user) { auth.user.name = profile.value.name; localStorage.setItem('opsctl_user', JSON.stringify(auth.user)); }
  } catch (e) {
    message.error(e?.response?.data?.error || '保存失败');
  } finally { savingProfile.value = false; }
}

// ---- 活跃会话 ----
const sessions = ref([]);
async function loadSessions() {
  try { sessions.value = await api.sessions(); } catch (e) { message.error('加载会话失败'); }
}
async function revoke(sid) {
  try { await api.revokeSession(sid); message.success('已撤销'); await loadSessions(); }
  catch (e) { message.error(e?.response?.data?.error || '撤销失败'); }
}
const fmt = (t) => (t ? new Date(t * 1000).toLocaleString('zh-CN') : '—');
const sessionCols = [
  { title: '设备', key: 'device_id', ellipsis: { tooltip: true },
    render: (r) => h('span', {}, [r.device_id, r.current ? h(NTag, { size: 'tiny', type: 'success', bordered: false, style: 'margin-left:6px' }, () => '本机') : null]) },
  { title: 'IP', key: 'ip', render: (r) => r.ip || '—', width: 130 },
  { title: '创建', key: 'created_at', render: (r) => fmt(r.created_at), width: 170 },
  { title: '最近活跃', key: 'last_seen', render: (r) => fmt(r.last_seen), width: 170 },
  { title: '操作', key: 'ops', width: 90,
    render: (r) => r.current ? h('span', { style: 'color:var(--muted)' }, '—') :
      h(NPopconfirm, { onPositiveClick: () => revoke(r.sid) },
        { trigger: () => h(NButton, { size: 'tiny', type: 'error', tertiary: true }, () => '撤销'),
          default: () => '撤销后该设备需重新登录' }) },
];

// ---- Telegram (演示) ----
const tg = ref({ show: false, code: '', note: '' });
async function tgStart() {
  try { const r = await api.tgBindStart(); tg.value = { show: true, code: r.code, note: r.note }; }
  catch (e) { message.error('生成绑定码失败'); }
}
async function tgConfirm() {
  try { await api.tgBindConfirm(tg.value.code); message.success('已绑定'); tg.value.show = false; await loadProfile(); }
  catch (e) { message.error(e?.response?.data?.error || '绑定失败'); }
}
async function tgUnbind() {
  try { await api.tgUnbind(); message.success('已解绑'); await loadProfile(); }
  catch (e) { message.error('解绑失败'); }
}

// ---- Git 同步 (admin) ----
const git = ref({ mode: 'folder', url: '', branch: 'main', username: '', credential: '', auto_push: false, work_dir: '' });
const gitStatus = ref({ git_installed: false, git_version: null, last_commit: null, credential_set: false, work_dir_abs: '' });
const savingGit = ref(false);
const gitBusy = ref('');
const gitModes = [
  { label: '本地文件夹', value: 'folder' },
  { label: '本地 Git', value: 'local' },
  { label: '远程 Git', value: 'remote' },
];
async function loadGit() {
  if (!auth.isAdmin) return;
  try {
    const r = await api.gitConfig();
    git.value = { ...git.value, ...r.config, credential: '' };
    gitStatus.value = { git_installed: r.git_installed, git_version: r.git_version, last_commit: r.last_commit, credential_set: r.config?.credential_set, work_dir_abs: r.work_dir_abs || '' };
  } catch (e) { /* non-admin or none */ }
}
async function saveGit() {
  savingGit.value = true;
  try { await api.updateGitConfig(git.value); message.success('Git 配置已保存'); await loadGit(); }
  catch (e) { message.error(e?.response?.data?.error || '保存失败'); }
  finally { savingGit.value = false; }
}
async function gitAction(what) {
  gitBusy.value = what;
  try {
    const r = await api.gitAction(what);
    message.success(r.note || '完成');
    await loadGit();
  } catch (e) { message.error(e?.response?.data?.error || '操作失败'); }
  finally { gitBusy.value = ''; }
}
async function gitRevealDir() {
  try { await api.gitReveal(); }
  catch (e) { message.error(e?.response?.data?.error || '打开失败'); }
}
async function gitInstall() {
  gitBusy.value = 'install';
  try {
    const r = await api.gitInstall();
    if (r.ok) message.success(r.note || 'git 已安装'); else message.warning(r.note || '安装未完成');
    await loadGit();
  } catch (e) { message.error(e?.response?.data?.error || '安装失败'); }
  finally { gitBusy.value = ''; }
}

// ---- 登录与注册开关 (admin) ----
const flags = ref({ register_open: false, otp_enabled: false });
const savingFlags = ref(false);
async function loadFlags() {
  try { flags.value = await api.flags(); } catch (e) { /* ignore */ }
}
async function saveFlags() {
  savingFlags.value = true;
  try { await api.updateFlags(flags.value); message.success('已保存'); }
  catch (e) { message.error(e?.response?.data?.error || '保存失败'); }
  finally { savingFlags.value = false; }
}

// ---- 凭据金库 (admin) ----
const vault = ref({ sealed: true });
const passphrase = ref('');
const vaultBusy = ref(false);
async function loadVault() {
  if (!auth.isAdmin) return;
  try { vault.value = await api.vaultStatus(); } catch (e) { /* ignore */ }
}
async function unseal() {
  if (!passphrase.value) { message.warning('请输入解封口令'); return; }
  vaultBusy.value = true;
  try {
    const r = await api.vaultUnseal(passphrase.value);
    message.success(`已解封${r.migrated ? `,加密了 ${r.migrated} 条历史凭据` : ''}`);
    passphrase.value = '';
    await loadVault();
  } catch (e) { message.error(e?.response?.data?.error || '解封失败'); }
  finally { vaultBusy.value = false; }
}
async function seal() {
  try { await api.vaultSeal(); message.success('已封存'); await loadVault(); }
  catch (e) { message.error('封存失败'); }
}

// ---- 修改密码 ----
const pw = ref({ old_password: '', new_password: '' });
const pwBusy = ref(false);
async function changePw() {
  if (pw.value.new_password.length < 6) { message.warning('新密码至少 6 位'); return; }
  pwBusy.value = true;
  try {
    await api.changePassword(pw.value.old_password, pw.value.new_password);
    message.success('密码已修改');
    pw.value = { old_password: '', new_password: '' };
  } catch (e) { message.error(e?.response?.data?.error || '修改失败'); }
  finally { pwBusy.value = false; }
}

// ---- 两步验证 (TOTP, per-user) ----
const totp = ref({ enrolling: false, secret: '', uri: '', code: '' });
const totpBusy = ref(false);
async function totpStart() {
  try {
    const r = await api.totpStart();
    totp.value = { enrolling: true, secret: r.secret, uri: r.otpauth_uri, code: '' };
  } catch (e) { message.error('生成密钥失败'); }
}
async function totpConfirm() {
  if (!/^\d{6}$/.test(totp.value.code)) { message.warning('请输入 6 位验证码'); return; }
  totpBusy.value = true;
  try {
    await api.totpConfirm(totp.value.code);
    message.success('两步验证已开启');
    totp.value = { enrolling: false, secret: '', uri: '', code: '' };
    await loadProfile();
  } catch (e) { message.error(e?.response?.data?.error || '验证失败'); }
  finally { totpBusy.value = false; }
}
async function totpDisable() {
  try { await api.totpDisable(); message.success('已关闭两步验证'); await loadProfile(); }
  catch (e) { message.error('关闭失败'); }
}

onMounted(() => { loadProfile(); loadSessions(); loadGit(); loadFlags(); loadVault(); scrollToHash(); });
</script>

<template>
  <n-space vertical :size="16" style="max-width:820px">
    <!-- 个人信息 -->
    <n-card title="个人信息" size="small">
      <n-form label-placement="left" :label-width="110">
        <n-form-item label="显示名"><n-input v-model:value="profile.name" style="max-width:280px" /></n-form-item>
        <n-form-item label="邮箱"><n-input v-model:value="profile.email" style="max-width:280px" /></n-form-item>
        <n-form-item label="登录有效期"><n-select v-model:value="profile.login_ttl_secs" :options="ttlOptions" style="max-width:160px" /></n-form-item>
        <n-form-item label="新设备登录提醒"><n-switch v-model:value="profile.login_alert" /></n-form-item>
        <n-form-item label=" ">
          <n-button type="primary" :loading="savingProfile" @click="saveProfile">保存</n-button>
        </n-form-item>
      </n-form>
    </n-card>

    <!-- 修改密码 -->
    <n-card title="修改密码" size="small">
      <n-form label-placement="left" :label-width="90">
        <n-form-item label="原密码"><n-input v-model:value="pw.old_password" type="password" show-password-on="click" style="max-width:260px" /></n-form-item>
        <n-form-item label="新密码"><n-input v-model:value="pw.new_password" type="password" show-password-on="click" placeholder="至少 6 位" style="max-width:260px" /></n-form-item>
        <n-form-item label=" "><n-button type="primary" :loading="pwBusy" @click="changePw">修改</n-button></n-form-item>
      </n-form>
    </n-card>

    <!-- 两步验证 (TOTP) -->
    <n-card title="两步验证(TOTP)" size="small">
      <n-space vertical :size="12">
        <n-space align="center">
          <n-tag :type="profile.totp_enabled ? 'success' : 'default'" :bordered="false">
            {{ profile.totp_enabled ? '已开启' : '未开启' }}
          </n-tag>
          <n-button v-if="!profile.totp_enabled && !totp.enrolling" size="small" @click="totpStart">开启</n-button>
          <n-button v-else-if="profile.totp_enabled" size="small" type="error" tertiary @click="totpDisable">关闭</n-button>
        </n-space>
        <template v-if="totp.enrolling">
          <n-text depth="3" style="font-size:12px">用认证器 App(Google Authenticator / Authy / 1Password 等)扫描下方二维码,或手动输入密钥,然后输入当前 6 位动态码确认:</n-text>
          <div class="totp-enroll">
            <n-qr-code :value="totp.uri" :size="168" error-correction-level="M" class="totp-qr" />
            <div class="totp-secret">
              <div class="lbl">无法扫码?手动输入密钥</div>
              <code>{{ totp.secret }}</code>
            </div>
          </div>
          <n-space align="center">
            <n-input v-model:value="totp.code" placeholder="6 位验证码" style="width:160px" @keyup.enter="totpConfirm" />
            <n-button type="primary" :loading="totpBusy" @click="totpConfirm">确认开启</n-button>
            <n-button text @click="totp.enrolling = false">取消</n-button>
          </n-space>
        </template>
        <n-text depth="3" style="font-size:12px">开启后登录需输入认证器动态码;密钥经金库加密存储(需金库已解封才能开启)。</n-text>
      </n-space>
    </n-card>

    <!-- Telegram -->
    <n-card size="small">
      <template #header>Telegram 通知 <n-tag size="tiny" :bordered="false" type="warning">演示</n-tag></template>
      <n-space align="center">
        <n-tag :type="profile.telegram_bound ? 'success' : 'default'" :bordered="false">
          {{ profile.telegram_bound ? '已绑定' : '未绑定' }}
        </n-tag>
        <n-button v-if="!profile.telegram_bound" size="small" @click="tgStart">生成绑定码</n-button>
        <n-button v-else size="small" type="error" tertiary @click="tgUnbind">解绑</n-button>
      </n-space>
    </n-card>

    <!-- 活跃会话 -->
    <n-card id="sessions" ref="sessionsCard" title="活跃会话 / 设备" size="small"
      :class="{ 'flash-target': flashKey === 'sessions' }">
      <template #header-extra><n-button size="tiny" @click="loadSessions">刷新</n-button></template>
      <n-data-table :columns="sessionCols" :data="sessions" size="small" :bordered="false" />
    </n-card>

    <!-- 凭据金库 (admin) -->
    <n-card v-if="auth.isAdmin" id="vault" ref="vaultCard" title="凭据金库(加密静态存储)" size="small"
      :class="{ 'flash-target': flashKey === 'vault' }">
      <n-space vertical :size="12">
        <n-space align="center">
          <span>状态:</span>
          <n-tag :type="vault.sealed ? 'error' : 'success'" :bordered="false">
            {{ vault.sealed ? '已封存(凭据不可用)' : '已解封' }}
          </n-tag>
          <n-button v-if="!vault.sealed" size="small" @click="seal">封存</n-button>
        </n-space>
        <n-space v-if="vault.sealed" align="center">
          <n-input v-model:value="passphrase" type="password" show-password-on="click" placeholder="解封口令" style="width:240px" @keyup.enter="unseal" />
          <n-button type="primary" size="small" :loading="vaultBusy" @click="unseal">解封</n-button>
        </n-space>
        <n-text depth="3" style="font-size:12px">系统用户密码/密钥用口令派生的密钥(argon2 + ChaCha20-Poly1305)加密后落库,解封后方可执行远程操作。首次解封即设定口令。</n-text>
      </n-space>
    </n-card>

    <!-- 注册开关 (admin) -->
    <n-card v-if="auth.isAdmin" title="自助注册" size="small">
      <n-form label-placement="left" :label-width="140">
        <n-form-item label="开放自助注册"><n-switch v-model:value="flags.register_open" /></n-form-item>
        <n-form-item label=" "><n-button type="primary" :loading="savingFlags" @click="saveFlags">保存</n-button></n-form-item>
      </n-form>
      <n-text depth="3" style="font-size:12px">两步验证已改为每个用户在上方「两步验证(TOTP)」卡自助开启。</n-text>
    </n-card>

    <!-- Git 同步 (admin) -->
    <n-card v-if="auth.isAdmin" title="Git 同步(配置变更留痕)" size="small">
      <!-- git 安装状态 -->
      <n-alert v-if="gitStatus.git_installed" type="success" :bordered="false" style="margin-bottom:14px">
        本地 git 可用:{{ gitStatus.git_version }}
        <span v-if="gitStatus.last_commit" style="color:var(--muted)">· 最近提交 {{ gitStatus.last_commit }}</span>
      </n-alert>
      <n-alert v-else type="error" :bordered="false" style="margin-bottom:14px" title="未检测到本地 git">
        <n-space align="center">
          需要 git 才能同步配置。
          <n-button size="small" type="primary" :loading="gitBusy==='install'" @click="gitInstall">自动安装 git</n-button>
        </n-space>
      </n-alert>

      <n-form label-placement="left" :label-width="110">
        <n-form-item label="模式">
          <n-radio-group v-model:value="git.mode">
            <n-radio-button v-for="m in gitModes" :key="m.value" :value="m.value" :label="m.label" />
          </n-radio-group>
        </n-form-item>
        <n-form-item v-if="git.mode === 'folder'" label="本地文件夹">
          <n-input v-model:value="git.work_dir" placeholder="配置导出目录,如 D:\\opsctl-config(留空用默认 data/opsctl-config)" style="max-width:420px" />
        </n-form-item>
        <n-form-item v-if="git.mode === 'local'" label="本地仓库">
          <n-input v-model:value="git.work_dir" placeholder="本地 git 仓库路径(留空用默认 data/opsctl-config)" style="max-width:420px" />
        </n-form-item>
        <template v-if="git.mode === 'remote'">
          <n-form-item label="仓库地址"><n-input v-model:value="git.url" placeholder="https://github.com/org/repo.git 或 git@…" style="max-width:380px" /></n-form-item>
          <n-form-item label="分支"><n-input v-model:value="git.branch" style="max-width:160px" /></n-form-item>
          <n-form-item label="用户名"><n-input v-model:value="git.username" placeholder="https 用户名(留空默认)" style="max-width:240px" /></n-form-item>
          <n-form-item label="密码/令牌">
            <n-input v-model:value="git.credential" type="password" show-password-on="click"
              :placeholder="gitStatus.credential_set ? '已保存,留空则不修改' : 'token / 密码(ssh 用 deploy key 免填)'" style="max-width:380px" />
          </n-form-item>
          <n-form-item label="同步时自动 push"><n-switch v-model:value="git.auto_push" /></n-form-item>
        </template>
        <n-form-item label=" ">
          <n-space>
            <n-button type="primary" :loading="savingGit" @click="saveGit">保存配置</n-button>
            <n-button :disabled="!gitStatus.git_installed" :loading="gitBusy==='test'" @click="gitAction('test')">测试连接</n-button>
            <n-button :disabled="!gitStatus.git_installed" :loading="gitBusy==='sync'" @click="gitAction('sync')">立即同步(commit)</n-button>
            <template v-if="git.mode === 'remote'">
              <n-button :disabled="!gitStatus.git_installed" :loading="gitBusy==='push'" @click="gitAction('push')">推送 push</n-button>
              <n-button :disabled="!gitStatus.git_installed" :loading="gitBusy==='pull'" @click="gitAction('pull')">拉取 pull</n-button>
            </template>
            <n-button @click="gitRevealDir">打开文件夹</n-button>
          </n-space>
        </n-form-item>
      </n-form>
      <n-text depth="3" style="font-size:12px">「同步」把用户/规则/资产/标签/账号/模板导出为 JSON 并 commit 到工作副本({{ gitStatus.work_dir_abs || git.work_dir || 'data/opsctl-config' }});远程模式可 push/pull。账号密码为密文(金库加密)。「打开文件夹」在服务端所在机器上打开该目录。</n-text>
    </n-card>

    <!-- Telegram 绑定弹窗 -->
    <n-modal v-model:show="tg.show" preset="card" title="绑定 Telegram(演示)" style="width:440px">
      <p style="color:var(--muted);font-size:13px;margin:0 0 12px">{{ tg.note }}</p>
      <div style="font:700 26px/1 monospace;letter-spacing:4px;text-align:center;padding:14px;background:var(--surface);border-radius:8px">{{ tg.code }}</div>
      <p style="color:var(--muted);font-size:12px;margin:12px 0 0">把此码发给 <b>@opsctl_bot</b>,或点下方按钮模拟完成。</p>
      <template #footer>
        <n-button type="primary" @click="tgConfirm">我已发送(演示)</n-button>
      </template>
    </n-modal>
  </n-space>
</template>

<style scoped>
/* landing highlight for the #sessions deep-link */
.flash-target { animation: flash-ring 1.8s ease-out; }
@keyframes flash-ring {
  0%, 60% { box-shadow: 0 0 0 2px var(--accent); }
  100% { box-shadow: 0 0 0 2px transparent; }
}
.totp-enroll { display: flex; gap: 18px; align-items: center; flex-wrap: wrap; margin: 6px 0; }
/* QR renders black-on-white; keep a white plate so it scans on the dark theme */
.totp-qr { background: #fff; padding: 10px; border-radius: 10px; }
.totp-secret .lbl { font-size: 12px; color: var(--muted); margin-bottom: 6px; }
.totp-secret code { font: 700 15px/1.5 monospace; background: var(--surface);
  padding: 8px 12px; border-radius: 6px; word-break: break-all; display: inline-block; max-width: 300px; }
</style>
