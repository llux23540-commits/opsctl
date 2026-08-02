<script setup>
import { computed, h, ref, onMounted, onUnmounted, watch } from 'vue';
import { useRouter, useRoute } from 'vue-router';
import { NBadge, useNotification } from 'naive-ui';
import { useAuth } from '../store/auth';
import { api } from '../api';
import { wsConnect, wsClose, wsOn } from '../ws';

const router = useRouter();
const route = useRoute();
const auth = useAuth();
const notification = useNotification();

const pendingCount = ref(0);
const unread = ref(0);
async function refreshPending() {
  if (!auth.isAdmin) return;
  try {
    const list = await api.approvals();
    pendingCount.value = list.filter((a) => a.state === 'pending').length;
  } catch (e) {
    /* ignore badge fetch errors */
  }
}
async function refreshUnread() {
  try { unread.value = (await api.unreadCount()).count; } catch (e) { /* ignore */ }
}

// bell → inline notification panel (mirrors prototype notify.js: recent items +
// 查看全部, instead of jumping straight to the messages page)
const recentMsgs = ref([]);
const notifKind = {
  login: { icon: '🔑', color: '#19b8a6' },
  approval: { icon: '⏳', color: '#e0a83e' },
  sync: { icon: '🔄', color: '#3ecf8e' },
  exec: { icon: '▶', color: '#8b93a3' },
};
async function loadRecent() {
  try { recentMsgs.value = (await api.messages()).slice(0, 6); } catch (e) { recentMsgs.value = []; }
}
async function openNotif(m) {
  if (!m.read) { try { await api.markRead(m.id); m.read = 1; refreshUnread(); } catch (e) {} }
  router.push(m.link || '/messages');
}
const fmtTime = (t) => new Date(t * 1000).toLocaleString('zh-CN');
// ---- WebSocket 实时通道:集群广播弹通知,会话被撤销即登出 ----
const wsOff = [];
onMounted(() => {
  refreshPending();
  refreshUnread();
  window.addEventListener('approvals-changed', refreshPending);
  window.addEventListener('messages-changed', refreshUnread);
  wsConnect();
  wsOff.push(
    wsOn('broadcast', (m) => {
      notification.info({
        title: m.title || '集群消息',
        content: m.body || '',
        meta: `${m.from || ''} · ${fmtTime(m.ts)}`,
        duration: 8000,
      });
      refreshUnread();
      loadRecent();
    }),
    wsOn('bye', () => {
      notification.warning({ title: '会话已撤销', content: '当前登录已被管理员撤销,请重新登录', duration: 5000 });
      logout();
    }),
  );
});
onUnmounted(() => {
  window.removeEventListener('approvals-changed', refreshPending);
  window.removeEventListener('messages-changed', refreshUnread);
  wsOff.forEach((off) => off());
});
// re-check whenever navigation lands (approve/reject/read changes counts)
watch(() => route.path, () => { refreshPending(); refreshUnread(); });

function labelWithBadge(text, count) {
  if (!count) return text;
  return () => h('span', { style: 'display:inline-flex;align-items:center;gap:8px' }, [
    text,
    h(NBadge, { value: count, type: 'warning' }),
  ]);
}

const menuOptions = computed(() => {
  const items = [
    { label: '节点执行', key: '/console' },
    { label: '消息', key: '/messages' },
    { label: '执行记录', key: '/audit' },
    { label: '设置', key: '/settings' },
  ];
  if (auth.isAdmin) items.splice(1, 0,
    { label: '用户与权限', key: '/users' },
    { label: '在线与广播', key: '/online' },
    { label: '资产管理', key: '/assets' },
    { label: 'Nacos 管理', key: '/nacos' },
    { label: '授权规则', key: '/access' },
    { label: '执行模板', key: '/templates' },
    { label: labelWithBadge('审批确认', pendingCount.value), key: '/approvals' });
  return items;
});

const activeKey = computed(() => route.path);
function onMenu(key) {
  router.push(key);
}

// avatar initials from the display name (falls back to email / "?")
const initials = computed(() => {
  const n = auth.user?.name || auth.user?.email || '?';
  return n.slice(0, 2).toUpperCase();
});

// user dropdown (mirrors the prototype's "点击用户名进入个人信息与设置")
const userOptions = [
  { label: '个人信息与设置', key: 'settings' },
  { label: '我的会话与设备', key: 'sessions' },
  { type: 'divider', key: 'd1' },
  { label: '退出登录', key: 'logout' },
];
function onUserSelect(key) {
  if (key === 'logout') logout();
  else if (key === 'settings') router.push('/settings');
  else if (key === 'sessions') router.push('/settings#sessions');
}

function logout() {
  wsClose();
  auth.logout();
  router.push('/login');
}
</script>

<template>
  <n-layout has-sider position="absolute">
    <n-layout-sider bordered :width="220" :native-scrollbar="false" style="background:var(--bg)">
      <div class="brand">
        <span class="mark">◆</span> opsctl
      </div>
      <n-menu :value="activeKey" :options="menuOptions" @update:value="onMenu" />
    </n-layout-sider>
    <n-layout>
      <n-layout-header bordered class="hdr">
        <div class="spacer" />
        <n-popover trigger="click" placement="bottom-end" @update:show="(s) => s && loadRecent()" style="padding:0" :width="320">
          <template #trigger>
            <n-badge :value="unread" :max="99" style="margin-right:16px">
              <n-button text style="font-size:18px" title="消息">🔔</n-button>
            </n-badge>
          </template>
          <div class="notif-panel">
            <div class="notif-hd">通知</div>
            <div v-if="!recentMsgs.length" class="notif-empty">暂无消息</div>
            <div v-for="m in recentMsgs" :key="m.id" class="notif-row" :class="{ unread: !m.read }" @click="openNotif(m)">
              <span class="ni" :style="{ color: (notifKind[m.kind] || {}).color }">{{ (notifKind[m.kind] || {}).icon || '•' }}</span>
              <div class="nx">
                <div class="nt">{{ m.title }}</div>
                <div class="nm">{{ fmtTime(m.ts) }}</div>
              </div>
            </div>
            <div class="notif-ft"><a @click="router.push('/messages')">查看全部消息 →</a></div>
          </div>
        </n-popover>
        <n-dropdown trigger="click" :options="userOptions" @select="onUserSelect" placement="bottom-end">
          <div class="who" title="个人信息与设置">
            <span class="av">{{ initials }}</span>
            <span class="who-txt">{{ auth.user?.name }} · <b>{{ auth.roleLabel }}</b></span>
            <svg class="caret" viewBox="0 0 24 24"><path d="M6 9l6 6 6-6" fill="none" stroke="currentColor" stroke-width="2"/></svg>
          </div>
        </n-dropdown>
      </n-layout-header>
      <n-layout-content class="content" :native-scrollbar="false">
        <router-view />
      </n-layout-content>
    </n-layout>
  </n-layout>
</template>

<style scoped>
.brand { height: 56px; display: flex; align-items: center; gap: 8px; padding: 0 18px;
  font-weight: 700; font-size: 18px; color: var(--fg); }
.brand .mark { color: var(--accent); }
.hdr { height: 52px; display: flex; align-items: center; padding: 0 18px; }
.hdr .spacer { flex: 1; }
.content { padding: 18px; }

/* user area (avatar + name·role + caret), click → dropdown */
.who { display: flex; align-items: center; gap: 8px; cursor: pointer; padding: 4px 8px;
  border-radius: 8px; transition: background .15s; user-select: none; }
.who:hover { background: var(--surface-warm); }
.who .av { width: 28px; height: 28px; border-radius: 7px; background: var(--accent); color: #fff;
  display: grid; place-items: center; font-size: 12px; font-weight: 700; letter-spacing: .02em; }
.who .who-txt { font-size: 13px; color: var(--fg-2); }
.who .who-txt b { color: var(--fg); font-weight: 600; }
.who .caret { width: 14px; height: 14px; color: var(--muted); }

/* bell notification panel */
.notif-panel { display: flex; flex-direction: column; }
.notif-hd { padding: 10px 14px; font-weight: 600; border-bottom: 1px solid rgba(255,255,255,.08); }
.notif-empty { padding: 20px; text-align: center; color: var(--muted); font-size: 13px; }
.notif-row { display: flex; gap: 10px; align-items: flex-start; padding: 10px 14px; cursor: pointer; }
.notif-row:hover { background: var(--surface-warm); }
.notif-row.unread { background: color-mix(in oklab, var(--accent), transparent 92%); }
.notif-row .ni { font-size: 15px; line-height: 1.3; }
.notif-row .nt { font-size: 13px; color: var(--fg); }
.notif-row .nm { font-size: 11px; color: var(--muted); margin-top: 2px; }
.notif-ft { padding: 10px 14px; border-top: 1px solid rgba(255,255,255,.08); }
.notif-ft a { color: var(--accent); cursor: pointer; font-size: 13px; }
</style>
