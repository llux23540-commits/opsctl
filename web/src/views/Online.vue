<script setup>
import { computed, h, onMounted, onUnmounted, ref } from 'vue';
import { NTag, useMessage } from 'naive-ui';
import { api } from '../api';

const msg = useMessage();

// ---- 在线连接(跨节点,来自服务端 ws_presence 表)----
const rows = ref([]);
const node = ref('');
const loading = ref(false);
let timer = null;

async function refresh() {
  loading.value = true;
  try {
    const d = await api.wsOnline();
    rows.value = d.items || [];
    node.value = d.node || '';
  } catch (e) {
    msg.error(e?.response?.data?.error || '在线列表加载失败');
  } finally {
    loading.value = false;
  }
}

const fmt = (t) => (t ? new Date(t * 1000).toLocaleString('zh-CN') : '-');
const clientTag = { web: { label: 'Web', type: 'info' }, desktop: { label: '桌面', type: 'success' } };
const columns = [
  { title: '账号', key: 'email' },
  { title: '角色', key: 'role', width: 90 },
  {
    title: '客户端', key: 'client', width: 90,
    render: (r) => {
      const c = clientTag[r.client] || { label: r.client, type: 'default' };
      return h(NTag, { size: 'small', type: c.type, bordered: false }, { default: () => c.label });
    },
  },
  { title: '登录设备', key: 'device_id', ellipsis: { tooltip: true } },
  { title: 'IP', key: 'ip', width: 130 },
  { title: '上线时间', key: 'connected_at', width: 170, render: (r) => fmt(r.connected_at) },
  { title: '最后心跳', key: 'last_seen', width: 170, render: (r) => fmt(r.last_seen) },
  {
    title: '节点', key: 'node_id', width: 110, ellipsis: { tooltip: true },
    render: (r) => h('span', { class: 'mono' }, r.node_id === node.value ? `${r.node_id.slice(0, 8)}(本节点)` : r.node_id.slice(0, 8)),
  },
];

// ---- 集群消息(广播 / 定向)----
const form = ref({ title: '', body: '', user_id: null });
const sending = ref(false);
const users = ref([]);
const userOptions = computed(() =>
  users.value.map((u) => ({ label: `${u.name}(${u.email})`, value: u.id }))
);

async function send() {
  if (!form.value.title.trim() && !form.value.body.trim()) {
    msg.warning('标题和内容至少填一个');
    return;
  }
  sending.value = true;
  try {
    const d = await api.wsBroadcast({
      title: form.value.title.trim(),
      body: form.value.body.trim(),
      user_id: form.value.user_id || '',
    });
    msg.success(`已发送:${form.value.user_id ? '定向 1 人' : `全员 ${d.notified} 人`}(在线连接实时收到,离线进消息中心)`);
    form.value = { title: '', body: '', user_id: null };
  } catch (e) {
    msg.error(e?.response?.data?.error || '发送失败');
  } finally {
    sending.value = false;
  }
}

onMounted(async () => {
  refresh();
  timer = setInterval(refresh, 10000);
  try { users.value = await api.users(); } catch (e) { users.value = []; }
});
onUnmounted(() => clearInterval(timer));
</script>

<template>
  <div class="page">
    <n-card :bordered="false">
      <template #header>
        在线连接
        <span class="sub">跨节点实时状态(心跳超 30s 视为下线);会话撤销即踢线</span>
      </template>
      <template #header-extra>
        <n-button size="small" :loading="loading" @click="refresh">刷新</n-button>
      </template>
      <n-data-table
        :columns="columns"
        :data="rows"
        :loading="loading"
        :bordered="false"
        size="small"
        :row-key="(r) => r.conn_id"
      />
      <p class="hint">共 {{ rows.length }} 个连接;本服务节点 <span class="mono">{{ node.slice(0, 8) }}</span></p>
    </n-card>

    <n-card :bordered="false" style="margin-top: 16px">
      <template #header>
        发送集群消息
        <span class="sub">所有节点上的在线连接实时收到;同时写入站内信,离线用户上线后可见</span>
      </template>
      <div class="bc-grid">
        <n-form-item label="接收人" label-placement="left">
          <n-select
            v-model:value="form.user_id"
            :options="userOptions"
            clearable
            placeholder="全员广播(清空 = 全员)"
            style="min-width: 260px"
          />
        </n-form-item>
        <n-form-item label="标题" label-placement="left">
          <n-input v-model:value="form.title" placeholder="如:停机维护通知" maxlength="80" show-count />
        </n-form-item>
      </div>
      <n-input
        v-model:value="form.body"
        type="textarea"
        :autosize="{ minRows: 3, maxRows: 6 }"
        placeholder="消息内容"
        maxlength="500"
        show-count
      />
      <div style="margin-top: 12px; text-align: right">
        <n-button type="primary" :loading="sending" @click="send">发送</n-button>
      </div>
    </n-card>
  </div>
</template>

<style scoped>
.sub { margin-left: 10px; font-size: 12px; font-weight: 400; color: var(--muted); }
.hint { margin: 10px 2px 0; font-size: 12px; color: var(--muted); }
.mono { font-family: ui-monospace, monospace; }
.bc-grid { display: grid; grid-template-columns: auto 1fr; gap: 0 24px; }
</style>
