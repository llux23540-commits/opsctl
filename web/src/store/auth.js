import { defineStore } from 'pinia';
import { api } from '../api';

export const useAuth = defineStore('auth', {
  state: () => ({
    token: localStorage.getItem('opsctl_token') || '',
    user: JSON.parse(localStorage.getItem('opsctl_user') || 'null'),
  }),
  getters: {
    isLoggedIn: (s) => !!s.token,
    isAdmin: (s) => s.user && String(s.user.role).toLowerCase() === 'admin',
    roleLabel: (s) => (s.user ? s.user.role : ''),
  },
  actions: {
    // Returns { need_otp, pending_id, demo_code } when OTP is enabled;
    // otherwise stores the session and returns the login response.
    async login(username, password) {
      const res = await api.login(username, password);
      if (res.need_otp) return res;
      this.setSession(res);
      return res;
    },
    async completeOtp(pending_id, code) {
      const res = await api.loginOtp(pending_id, code);
      this.setSession(res);
      return res;
    },
    setSession(res) {
      this.token = res.token;
      this.user = res.user;
      localStorage.setItem('opsctl_token', res.token);
      localStorage.setItem('opsctl_user', JSON.stringify(res.user));
    },
    logout() {
      this.token = '';
      this.user = null;
      localStorage.removeItem('opsctl_token');
      localStorage.removeItem('opsctl_user');
    },
  },
});
