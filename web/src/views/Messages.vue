<script setup>
import { ref, onMounted, computed, watch } from 'vue';
import { useRoute, useRouter } from 'vue-router';
import { useMessage } from 'naive-ui';
import { api } from '../api';

const route = useRoute();
const router = useRouter();
const message = useMessage();
const list = ref([]);
const filter = ref('all');
const loading = ref(false);
const selected = ref(null); // currently opened message (right pane)

const kindMeta = {
  login: { label: '登录', color: '#19b8a6', icon: '🔑', source: '会话服务' },
  approval: { label: '审批', color: '#e0a83e', icon: '⏳', source: '审批引擎' },
  sync: { label: '同步', color: '#3ecf8e', icon: '🔄', source: 'Git 同步' },
  exec: { label: '执行', color: '#8b93a3', icon: '▶', source: '作业队列' },
};
const meta = (k) => kindMeta[k] || { label: k, color: 'var(--muted)', icon: '•', source: '系统' };

const filters = computed(() => [
  { key: 'all', label: '全部', n: list.value.length },
  { key: 'unread', label: '未读', n: list.value.filter((m) => !m.read).length },
  { key: 'login', label: '登录', n: list.value.filter((m) => m.kind === 'login').length },
  { key: 'approval', label: '审批', n: list.value.filter((m) => m.kind === 'approval').length },
  { key: 'sync', label: '同步', n: list.value.filter((m) => m.kind === 'sync').length },
  { key: 'exec', label: '执行', n: list.value.filter((m) => m.kind === 'exec').length },
]);

const shown = computed(() => {
  if (filter.value === 'all') return list.value;
  if (filter.value === 'unread') return list.value.filter((m) => !m.read);
  return list.value.filter((m) => m.kind === filter.value);
});

async function load() {
  loading.value = true;
  try {
    list.value = await api.messages();
    // deep-link: #/messages?id=xxx selects that message
    const wantId = route.query.id;
    if (wantId) {
      const hit = list.value.find((m) => m.id === wantId);
      if (hit) openMsg(hit);
    }
  } catch (e) { message.error('加载消息失败'); }
  finally { loading.value = false; }
}
onMounted(load);
watch(() => route.query.id, (id) => {
  if (id) { const hit = list.value.find((m) => m.id === id); if (hit) openMsg(hit); }
});

async function openMsg(m) {
  selected.value = m;
  if (!m.read) {
    try { await api.markRead(m.id); m.read = 1; window.dispatchEvent(new Event('messages-changed')); } catch (e) {}
  }
}
async function markUnread(m) {
  try { await api.markUnread(m.id); m.read = 0; window.dispatchEvent(new Event('messages-changed')); message.success('已标为未读'); }
  catch (e) { message.error('操作失败'); }
}
function openLink(m) {
  if (m.link) router.push(m.link);
}
async function markAll() {
  try { await api.markAllRead(); list.value.forEach((m) => (m.read = 1)); window.dispatchEvent(new Event('messages-changed')); message.success('全部已读'); }
  catch (e) { message.error('操作失败'); }
}
async function del(m) {
  try {
    await api.deleteMessage(m.id);
    list.value = list.value.filter((x) => x.id !== m.id);
    if (selected.value?.id === m.id) selected.value = null;
    window.dispatchEvent(new Event('messages-changed'));
  } catch (e) { message.error('删除失败'); }
}
const fmt = (t) => new Date(t * 1000).toLocaleString('zh-CN');
</script>

<template>
  <n-card title="消息中心" size="small">
    <template #header-extra>
      <n-space align="center">
        <n-button size="small" @click="markAll">全部已读</n-button>
        <n-button size="small" @click="load" :loading="loading">刷新</n-button>
      </n-space>
    </template>

    <n-space style="margin-bottom:12px">
      <n-tag v-for="f in filters" :key="f.key" :bordered="false" checkable :checked="filter === f.key"
        style="cursor:pointer" @click="filter = f.key">{{ f.label }} {{ f.n }}</n-tag>
    </n-space>

    <div class="msg-layout">
      <!-- left: list -->
      <div class="msg-list">
        <n-list hoverable clickable>
          <n-list-item v-for="m in shown" :key="m.id" @click="openMsg(m)"
            :class="{ active: selected && selected.id === m.id }">
            <n-thing>
              <template #header>
                <span :style="{ opacity: m.read ? 0.6 : 1 }">
                  <span :style="{ color: meta(m.kind).color }">{{ meta(m.kind).icon }}</span>
                  {{ m.title }}
                  <n-tag v-if="!m.read" size="tiny" type="info" :bordered="false" style="margin-left:6px">未读</n-tag>
                </span>
              </template>
              <template #description>
                <span style="color:var(--muted);font-size:12px">{{ fmt(m.ts) }}</span>
              </template>
            </n-thing>
          </n-list-item>
        </n-list>
        <n-empty v-if="!shown.length && !loading" description="暂无消息" style="margin-top:20px" />
      </div>

      <!-- right: detail -->
      <div class="msg-detail">
        <n-empty v-if="!selected" description="选择左侧一条消息查看详情" style="margin-top:60px" />
        <template v-else>
          <div class="d-head">
            <span class="d-icon" :style="{ color: meta(selected.kind).color }">{{ meta(selected.kind).icon }}</span>
            <h3>{{ selected.title }}</h3>
          </div>
          <n-descriptions :column="2" label-placement="left" size="small" bordered style="margin:12px 0">
            <n-descriptions-item label="类型">{{ meta(selected.kind).label }}</n-descriptions-item>
            <n-descriptions-item label="来源">{{ meta(selected.kind).source }}</n-descriptions-item>
            <n-descriptions-item label="时间">{{ fmt(selected.ts) }}</n-descriptions-item>
            <n-descriptions-item label="状态">{{ selected.read ? '已读' : '未读' }}</n-descriptions-item>
          </n-descriptions>
          <p class="d-body">{{ selected.body }}</p>
          <n-space style="margin-top:16px">
            <n-button v-if="selected.link" type="primary" size="small" @click="openLink(selected)">前往处理 ↗</n-button>
            <n-button size="small" @click="markUnread(selected)">标记为未读</n-button>
            <n-popconfirm @positive-click="() => del(selected)">
              <template #trigger><n-button size="small" type="error" tertiary>删除</n-button></template>
              删除这条消息?
            </n-popconfirm>
          </n-space>
        </template>
      </div>
    </div>
  </n-card>
</template>

<style scoped>
.msg-layout { display: grid; grid-template-columns: 340px 1fr; gap: 16px; }
.msg-list { border-right: 1px solid rgba(255,255,255,.08); padding-right: 8px; max-height: 68vh; overflow: auto; }
.msg-list :deep(.n-list-item.active) { background: var(--surface-warm); border-radius: 8px; }
.msg-detail { padding: 4px 8px; }
.d-head { display: flex; align-items: center; gap: 10px; }
.d-head .d-icon { font-size: 20px; }
.d-head h3 { margin: 0; font-size: 17px; }
.d-body { color: var(--fg-2); font-size: 14px; line-height: 1.6; white-space: pre-wrap; }
@media (max-width: 720px) { .msg-layout { grid-template-columns: 1fr; } }
</style>
