import axios from 'axios';

// Per-browser stable device id (web has no machine code; a random id is fine).
export function deviceId() {
  let id = localStorage.getItem('opsctl_device');
  if (!id) {
    id = 'web-' + Math.random().toString(36).slice(2) + Date.now().toString(36);
    localStorage.setItem('opsctl_device', id);
  }
  return id;
}

const http = axios.create({ baseURL: '/api', timeout: 30000 });

http.interceptors.request.use((cfg) => {
  const token = localStorage.getItem('opsctl_token');
  if (token) cfg.headers.Authorization = `Bearer ${token}`;
  cfg.headers['x-device-id'] = deviceId();
  return cfg;
});

http.interceptors.response.use(
  (r) => r,
  (err) => {
    if (err.response && err.response.status === 401) {
      // session invalid → force logout
      localStorage.removeItem('opsctl_token');
      if (location.hash !== '#/login') location.hash = '#/login';
    }
    return Promise.reject(err);
  }
);

export const api = {
  login: (username, password) =>
    http.post('/login', { username, password, device_id: deviceId() }).then((r) => r.data),
  assets: () => http.get('/assets').then((r) => r.data),
  asset: (id) => http.get(`/assets/${id}`).then((r) => r.data),
  createAsset: (body) => http.post('/assets', body).then((r) => r.data),
  updateAsset: (id, body) => http.put(`/assets/${id}`, body).then((r) => r.data),
  deleteAsset: (id) => http.delete(`/assets/${id}`).then((r) => r.data),
  assetFile: (id) => http.get(`/assets/${id}/file`).then((r) => r.data),
  accounts: () => http.get('/accounts').then((r) => r.data),
  createAccount: (body) => http.post('/accounts', body).then((r) => r.data),
  updateAccount: (id, body) => http.put(`/accounts/${id}`, body).then((r) => r.data),
  deleteAccount: (id) => http.delete(`/accounts/${id}`).then((r) => r.data),
  tags: () => http.get('/tags').then((r) => r.data),
  createTag: (body) => http.post('/tags', body).then((r) => r.data),
  updateTag: (id, body) => http.put(`/tags/${id}`, body).then((r) => r.data),
  deleteTag: (id) => http.delete(`/tags/${id}`).then((r) => r.data),
  rules: () => http.get('/rules').then((r) => r.data),
  createRule: (body) => http.post('/rules', body).then((r) => r.data),
  updateRule: (id, body) => http.put(`/rules/${id}`, body).then((r) => r.data),
  deleteRule: (id) => http.delete(`/rules/${id}`).then((r) => r.data),
  users: () => http.get('/users').then((r) => r.data),
  createUser: (body) => http.post('/users', body).then((r) => r.data),
  updateUser: (id, body) => http.put(`/users/${id}`, body).then((r) => r.data),
  deleteUser: (id) => http.delete(`/users/${id}`).then((r) => r.data),
  resetUserPassword: (id, password) => http.post(`/users/${id}/reset-password`, { password }).then((r) => r.data),
  changePassword: (old_password, new_password) => http.put('/profile/password', { old_password, new_password }).then((r) => r.data),
  runSsh: (targets, command, templateId) =>
    http.post('/jobs/ssh', { targets, command, template_id: templateId || null }).then((r) => r.data),
  runSql: (targets, query, templateId) =>
    http.post('/jobs/sql', { targets, query, template_id: templateId || null }).then((r) => r.data),
  backupStatus: () => http.get('/backup/status').then((r) => r.data),
  backupRun: () => http.post('/backup/run').then((r) => r.data),
  audit: () => http.get('/audit').then((r) => r.data),
  jobs: (params) => http.get('/jobs', { params }).then((r) => r.data),
  jobDetail: (id) => http.get(`/jobs/${id}`).then((r) => r.data),
  nodeHistory: (id) => http.get(`/assets/${id}/history`).then((r) => r.data),
  probeAsset: (body) => http.post('/assets/probe', body).then((r) => r.data),
  approvals: () => http.get('/approvals').then((r) => r.data),
  decideApproval: (id, body) => http.post(`/approvals/${id}/decide`, body).then((r) => r.data),
  profile: () => http.get('/profile').then((r) => r.data),
  updateProfile: (body) => http.put('/profile', body).then((r) => r.data),
  totpStart: () => http.post('/profile/totp/start').then((r) => r.data),
  totpConfirm: (code) => http.post('/profile/totp/confirm', { code }).then((r) => r.data),
  totpDisable: () => http.post('/profile/totp/disable').then((r) => r.data),
  sessions: () => http.get('/sessions').then((r) => r.data),
  revokeSession: (sid) => http.post(`/sessions/${sid}/revoke`).then((r) => r.data),
  tgBindStart: () => http.post('/telegram/bind/start').then((r) => r.data),
  tgBindConfirm: (code) => http.post('/telegram/bind/confirm', { code }).then((r) => r.data),
  tgUnbind: () => http.post('/telegram/unbind').then((r) => r.data),
  gitConfig: () => http.get('/settings/git').then((r) => r.data),
  updateGitConfig: (body) => http.put('/settings/git', body).then((r) => r.data),
  gitAction: (what) => http.post(`/settings/git/${what}`).then((r) => r.data),
  gitReveal: (path = '') => http.post('/settings/git/reveal', { path }).then((r) => r.data),
  gitInstall: () => http.post('/settings/git/install').then((r) => r.data),
  templates: () => http.get('/templates').then((r) => r.data),
  saveTemplate: (body) => http.post('/templates', body).then((r) => r.data),
  deleteTemplate: (id) => http.delete(`/templates/${id}`).then((r) => r.data),
  templateFile: (id) => http.get(`/templates/${id}/file`).then((r) => r.data),
  auditFiltered: (params) => http.get('/audit', { params }).then((r) => r.data),
  auditExport: (format, params) =>
    http.get('/audit/export', { params: { ...params, format }, responseType: 'blob' }).then((r) => r.data),
  vaultStatus: () => http.get('/vault/status').then((r) => r.data),
  vaultUnseal: (passphrase) => http.post('/vault/unseal', { passphrase }).then((r) => r.data),
  vaultSeal: () => http.post('/vault/seal').then((r) => r.data),
  decideBatch: (body) => http.post('/approvals/decide-batch', body).then((r) => r.data),
  messages: () => http.get('/messages').then((r) => r.data),
  unreadCount: () => http.get('/messages/unread-count').then((r) => r.data),
  markRead: (id) => http.post(`/messages/${id}/read`).then((r) => r.data),
  markUnread: (id) => http.post(`/messages/${id}/unread`).then((r) => r.data),
  markAllRead: () => http.post('/messages/read-all').then((r) => r.data),
  deleteMessage: (id) => http.delete(`/messages/${id}`).then((r) => r.data),
  flags: () => http.get('/flags').then((r) => r.data),
  updateFlags: (body) => http.put('/flags', body).then((r) => r.data),
  loginOtp: (pending_id, code) => http.post('/login/otp', { pending_id, code }).then((r) => r.data),
  register: (body) => http.post('/register', body).then((r) => r.data),
  // ---- Nacos 管理 ----
  nacosClusters: () => http.get('/nacos/clusters').then((r) => r.data),
  createNacosCluster: (body) => http.post('/nacos/clusters', body).then((r) => r.data),
  updateNacosCluster: (id, body) => http.put(`/nacos/clusters/${id}`, body).then((r) => r.data),
  deleteNacosCluster: (id) => http.delete(`/nacos/clusters/${id}`).then((r) => r.data),
  nacosNodes: (id) => http.get(`/nacos/clusters/${id}/nodes`).then((r) => r.data),
  nacosConfigs: (id, params) => http.get(`/nacos/clusters/${id}/configs`, { params }).then((r) => r.data),
  nacosInit: (id, body) => http.post(`/nacos/clusters/${id}/init`, body).then((r) => r.data),
  nacosProbe: (body) => http.post('/nacos/probe', body).then((r) => r.data),
  nacosTemplates: () => http.get('/nacos/templates').then((r) => r.data),
  saveNacosTemplate: (body) => http.post('/nacos/templates', body).then((r) => r.data),
  deleteNacosTemplate: (id) => http.delete(`/nacos/templates/${id}`).then((r) => r.data),
  nacosRuns: (params) => http.get('/nacos/runs', { params }).then((r) => r.data),
  // ---- Nacos 集群管理:命名空间 / 账号 / 角色 / 权限(全部直连远端 Nacos)----
  nacosNamespaces: (id) => http.get(`/nacos/clusters/${id}/namespaces`).then((r) => r.data),
  createNacosNamespace: (id, body) => http.post(`/nacos/clusters/${id}/namespaces`, body).then((r) => r.data),
  updateNacosNamespace: (id, body) => http.put(`/nacos/clusters/${id}/namespaces`, body).then((r) => r.data),
  deleteNacosNamespace: (id, ns) => http.delete(`/nacos/clusters/${id}/namespaces/${encodeURIComponent(ns)}`).then((r) => r.data),
  nacosUsers: (id, params) => http.get(`/nacos/clusters/${id}/users`, { params }).then((r) => r.data),
  createNacosUser: (id, body) => http.post(`/nacos/clusters/${id}/users`, body).then((r) => r.data),
  resetNacosUser: (id, body) => http.put(`/nacos/clusters/${id}/users`, body).then((r) => r.data),
  deleteNacosUser: (id, name) => http.delete(`/nacos/clusters/${id}/users/${encodeURIComponent(name)}`).then((r) => r.data),
  nacosRoles: (id, params) => http.get(`/nacos/clusters/${id}/roles`, { params }).then((r) => r.data),
  bindNacosRole: (id, body) => http.post(`/nacos/clusters/${id}/roles`, body).then((r) => r.data),
  unbindNacosRole: (id, params) => http.delete(`/nacos/clusters/${id}/roles`, { params }).then((r) => r.data),
  nacosPermissions: (id, params) => http.get(`/nacos/clusters/${id}/permissions`, { params }).then((r) => r.data),
  grantNacosPermission: (id, body) => http.post(`/nacos/clusters/${id}/permissions`, body).then((r) => r.data),
  revokeNacosPermission: (id, params) => http.delete(`/nacos/clusters/${id}/permissions`, { params }).then((r) => r.data),
  // ---- Nacos 配置:读正文 / 删除 / 整库同步为模板 ----
  nacosConfigDetail: (id, params) => http.get(`/nacos/clusters/${id}/configs/detail`, { params }).then((r) => r.data),
  deleteNacosConfig: (id, params) => http.delete(`/nacos/clusters/${id}/configs`, { params }).then((r) => r.data),
  syncNacosConfigs: (id, body) => http.post(`/nacos/clusters/${id}/sync`, body).then((r) => r.data),
};

export default http;
