<script setup>
import { ref, onMounted } from 'vue';
import { useRouter } from 'vue-router';
import { useMessage } from 'naive-ui';
import { useAuth } from '../store/auth';
import { api } from '../api';

const router = useRouter();
const message = useMessage();
const auth = useAuth();

const step = ref('login'); // login | otp | register
const username = ref('');
const password = ref('');
const loading = ref(false);
const registerOpen = ref(false);
const remember = ref(true);

// otp state: 6 independent digit boxes
const otp = ref({ pending_id: '' });
const otpDigits = ref(['', '', '', '', '', '']);
const otpInputs = ref([]);
const otpCode = () => otpDigits.value.join('');

function onOtpInput(i, e) {
  const v = (e.target.value || '').replace(/\D/g, '').slice(-1);
  otpDigits.value[i] = v;
  e.target.value = v;
  if (v && i < 5) otpInputs.value[i + 1]?.focus();
}
function onOtpKeydown(i, e) {
  if (e.key === 'Backspace' && !otpDigits.value[i] && i > 0) {
    otpInputs.value[i - 1]?.focus();
  } else if (e.key === 'Enter' && otpCode().length === 6) {
    submitOtp();
  }
}
function onOtpPaste(e) {
  const digits = (e.clipboardData?.getData('text') || '').replace(/\D/g, '').slice(0, 6).split('');
  if (!digits.length) return;
  e.preventDefault();
  for (let i = 0; i < 6; i++) otpDigits.value[i] = digits[i] || '';
  otpInputs.value[Math.min(digits.length, 5)]?.focus();
}
function resetOtp() {
  otpDigits.value = ['', '', '', '', '', ''];
}

function forgotPassword() {
  message.info('请联系管理员在「用户与权限」中重置密码');
}

// map raw backend/technical errors (e.g. "unauthorized") to a friendly message
function friendlyErr(e, fallback) {
  const err = e?.response?.data?.error || '';
  if (!err || /^(unauthorized|invalid|forbidden|bad request|not found)/i.test(err)) return fallback;
  return err;
}

// register state
const reg = ref({ username: '', password: '', email: '' });

onMounted(async () => {
  try { registerOpen.value = (await api.flags()).register_open; } catch (e) { /* ignore */ }
});

async function submit() {
  loading.value = true;
  try {
    const res = await auth.login(username.value, password.value);
    if (res.need_otp) {
      otp.value = { pending_id: res.pending_id };
      resetOtp();
      step.value = 'otp';
      message.info('请输入认证器中的动态验证码');
    } else {
      message.success('登录成功');
      router.push('/console');
    }
  } catch (e) {
    message.error(friendlyErr(e, '账号或密码错误'));
  } finally { loading.value = false; }
}

async function submitOtp() {
  if (otpCode().length !== 6) { message.warning('请输入 6 位验证码'); return; }
  loading.value = true;
  try {
    await auth.completeOtp(otp.value.pending_id, otpCode());
    message.success('登录成功');
    router.push('/console');
  } catch (e) {
    message.error(friendlyErr(e, '验证码错误或已过期'));
  } finally { loading.value = false; }
}

async function submitRegister() {
  loading.value = true;
  try {
    await api.register(reg.value);
    message.success('注册成功,请登录');
    username.value = reg.value.username;
    step.value = 'login';
  } catch (e) {
    message.error(e?.response?.data?.error || '注册失败');
  } finally { loading.value = false; }
}
</script>

<template>
  <div class="login-wrap">
    <n-card class="login-card" :bordered="true">
      <div class="brand">
        <span class="mark">◆</span>
        <div>
          <div class="title">opsctl</div>
          <div class="sub">运维平台 · 登录到 vault 服务端</div>
        </div>
      </div>

      <!-- 步骤1:账号密码 -->
      <template v-if="step === 'login'">
        <n-form @keyup.enter="submit">
          <n-form-item label="账号"><n-input v-model:value="username" placeholder="用户名" /></n-form-item>
          <n-form-item label="密码"><n-input v-model:value="password" type="password" show-password-on="click" placeholder="密码" /></n-form-item>
          <div class="login-row">
            <n-checkbox v-model:checked="remember">记住此设备</n-checkbox>
            <a class="forgot" @click="forgotPassword">忘记密码?</a>
          </div>
          <n-button type="primary" block :loading="loading" @click="submit">登 录</n-button>
        </n-form>
        <div class="hint">
          <template v-if="registerOpen"><a @click="step = 'register'">没有账号?注册</a></template>
          <template v-else>没有账号?请联系管理员开通</template>
        </div>
      </template>

      <!-- 步骤2:OTP -->
      <template v-else-if="step === 'otp'">
        <n-form @keyup.enter="submitOtp">
          <n-alert type="info" :bordered="false" style="margin-bottom:12px">
            两步验证 · 请输入认证器(Google Authenticator 等)显示的 6 位动态码
          </n-alert>
          <div class="otp-boxes" @paste="onOtpPaste">
            <input
              v-for="(d, i) in otpDigits"
              :key="i"
              ref="otpInputs"
              class="otp-box"
              inputmode="numeric"
              maxlength="1"
              :value="d"
              @input="(e) => onOtpInput(i, e)"
              @keydown="(e) => onOtpKeydown(i, e)"
            />
          </div>
          <n-button type="primary" block :loading="loading" @click="submitOtp" style="margin-top:16px">验 证</n-button>
        </n-form>
        <div class="hint"><a @click="step = 'login'">返回上一步</a></div>
      </template>

      <!-- 注册 -->
      <template v-else>
        <n-form @keyup.enter="submitRegister">
          <n-form-item label="用户名"><n-input v-model:value="reg.username" /></n-form-item>
          <n-form-item label="邮箱"><n-input v-model:value="reg.email" /></n-form-item>
          <n-form-item label="密码"><n-input v-model:value="reg.password" type="password" show-password-on="click" placeholder="至少 6 位" /></n-form-item>
          <n-button type="primary" block :loading="loading" @click="submitRegister">注 册</n-button>
        </n-form>
        <div class="hint"><a @click="step = 'login'">返回登录</a></div>
      </template>
    </n-card>
  </div>
</template>

<style scoped>
.login-wrap { height: 100vh; display: grid; place-items: center; background: var(--bg); }
.login-card { width: 360px; background: var(--surface); }
.brand { display: flex; align-items: center; gap: 12px; margin-bottom: 18px; }
.brand .mark { width: 40px; height: 40px; border-radius: 10px; background: var(--accent); color: #fff;
  display: grid; place-items: center; font-size: 20px; }
.brand .title { font-size: 22px; font-weight: 700; color: var(--fg); }
.brand .sub { font-size: 12px; color: var(--muted); }
.hint { margin-top: 14px; font-size: 12px; color: var(--muted); text-align: center; }
.hint a { color: var(--accent); cursor: pointer; }
.login-row { display: flex; align-items: center; justify-content: space-between; margin: 4px 0 14px; }
.login-row .forgot { font-size: 12px; color: var(--accent); cursor: pointer; }
.otp-boxes { display: flex; gap: 8px; justify-content: space-between; }
.otp-box { width: 44px; height: 52px; text-align: center; font-size: 22px; font-weight: 700;
  background: var(--surface-warm, #252b35); color: var(--fg, #e8ebf0);
  border: 1px solid rgba(255,255,255,.14); border-radius: 8px; outline: none; transition: border-color .15s; }
.otp-box:focus { border-color: var(--accent); }
</style>
