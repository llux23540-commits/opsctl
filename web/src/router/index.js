import { createRouter, createWebHashHistory } from 'vue-router';

const routes = [
  { path: '/login', component: () => import('../views/Login.vue'), meta: { public: true } },
  {
    path: '/',
    component: () => import('../layouts/MainLayout.vue'),
    redirect: '/console',
    children: [
      { path: 'console', component: () => import('../views/Console.vue'), meta: { title: '节点执行' } },
      { path: 'assets', component: () => import('../views/Assets.vue'), meta: { title: '资产管理', admin: true } },
      { path: 'users', component: () => import('../views/Users.vue'), meta: { title: '用户与权限', admin: true } },
      { path: 'online', component: () => import('../views/Online.vue'), meta: { title: '在线与广播', admin: true } },
      { path: 'access', component: () => import('../views/Access.vue'), meta: { title: '授权规则', admin: true } },
      { path: 'nacos', component: () => import('../views/Nacos.vue'), meta: { title: 'Nacos 管理', admin: true } },
      { path: 'nacos/:id', component: () => import('../views/NacosCluster.vue'), meta: { title: 'Nacos 集群管理', admin: true } },
      { path: 'templates', component: () => import('../views/Templates.vue'), meta: { title: '执行模板', admin: true } },
      { path: 'approvals', component: () => import('../views/Approvals.vue'), meta: { title: '审批确认', admin: true } },
      { path: 'audit', component: () => import('../views/Audit.vue'), meta: { title: '执行记录' } },
      { path: 'record/:id', component: () => import('../views/Record.vue'), meta: { title: '执行记录详情' } },
      { path: 'messages', component: () => import('../views/Messages.vue'), meta: { title: '消息' } },
      { path: 'settings', component: () => import('../views/Settings.vue'), meta: { title: '设置' } },
    ],
  },
  { path: '/:pathMatch(.*)*', redirect: '/console' },
];

const router = createRouter({ history: createWebHashHistory(), routes });

router.beforeEach((to) => {
  const token = localStorage.getItem('opsctl_token');
  if (!to.meta.public && !token) return '/login';
  if (to.meta.admin) {
    const user = JSON.parse(localStorage.getItem('opsctl_user') || 'null');
    if (!user || String(user.role).toLowerCase() !== 'admin') return '/console';
  }
  return true;
});

export default router;
