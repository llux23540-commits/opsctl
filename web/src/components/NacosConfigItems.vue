<script setup>
// Repeatable dataId/group/type/content rows — shared by the config template
// editor and the ad-hoc side of the init drawer.
import Icon from './Icon.vue';

const props = defineProps({
  modelValue: { type: Array, default: () => [] },
  /// Hide the "变量" hint when the caller already explains it.
  hint: { type: Boolean, default: true },
});
const emit = defineEmits(['update:modelValue']);

const TYPES = ['properties', 'yaml', 'json', 'text', 'xml', 'html'].map((v) => ({
  label: v,
  value: v,
}));

function patch(next) {
  emit('update:modelValue', next);
}
function addRow() {
  patch([
    ...props.modelValue,
    { data_id: '', group: 'DEFAULT_GROUP', type: 'properties', content: '' },
  ]);
}
function removeRow(i) {
  patch(props.modelValue.filter((_, idx) => idx !== i));
}
function setField(i, key, value) {
  patch(props.modelValue.map((row, idx) => (idx === i ? { ...row, [key]: value } : row)));
}
</script>

<template>
  <div class="items">
    <div v-if="!modelValue.length" class="empty">
      <Icon name="file" :size="22" />
      <p>还没有配置项</p>
      <span>每一项对应 Nacos 里的一个 dataId,初始化时逐条下发。</span>
    </div>

    <div v-for="(row, i) in modelValue" :key="i" class="row">
      <div class="row-hd">
        <n-input
          :value="row.data_id"
          placeholder="dataId,如 order-service.properties"
          size="small"
          class="mono"
          :aria-label="`第 ${i + 1} 项 dataId`"
          @update:value="(v) => setField(i, 'data_id', v)"
        />
        <n-input
          :value="row.group"
          placeholder="DEFAULT_GROUP"
          size="small"
          class="mono grp"
          :aria-label="`第 ${i + 1} 项 group`"
          @update:value="(v) => setField(i, 'group', v)"
        />
        <n-select
          :value="row.type"
          :options="TYPES"
          size="small"
          class="typ"
          :aria-label="`第 ${i + 1} 项格式`"
          @update:value="(v) => setField(i, 'type', v)"
        />
        <n-button
          size="small"
          quaternary
          type="error"
          :aria-label="`删除第 ${i + 1} 项`"
          @click="removeRow(i)"
        >
          <Icon name="minus" :size="15" />
        </n-button>
      </div>
      <n-input
        :value="row.content"
        type="textarea"
        size="small"
        class="mono"
        placeholder="配置内容"
        :autosize="{ minRows: 3, maxRows: 12 }"
        :aria-label="`第 ${i + 1} 项内容`"
        @update:value="(v) => setField(i, 'content', v)"
      />
    </div>

    <div class="ft">
      <n-button size="small" dashed @click="addRow">
        <Icon name="plus" :size="15" style="margin-right:6px" /> 添加配置项
      </n-button>
      <span v-if="hint" class="tip">
        支持 <code>${变量名}</code> 占位,初始化时统一填值
      </span>
    </div>
  </div>
</template>

<style scoped>
.items { display: flex; flex-direction: column; gap: 12px; }
.row { border: 1px solid rgba(255,255,255,.08); border-radius: 10px; padding: 10px;
  background: var(--bg); display: flex; flex-direction: column; gap: 8px; }
.row-hd { display: flex; gap: 8px; align-items: center; }
.row-hd .grp { max-width: 190px; }
.row-hd .typ { max-width: 130px; }
.mono :deep(input), .mono :deep(textarea) {
  font-family: ui-monospace, SFMono-Regular, "JetBrains Mono", Consolas, monospace;
  font-size: 12.5px;
}
.ft { display: flex; align-items: center; gap: 12px; }
.tip { font-size: 12px; color: var(--muted); }
.tip code { font-family: ui-monospace, Consolas, monospace; color: var(--fg-2); }
.empty { border: 1px dashed rgba(255,255,255,.12); border-radius: 10px; padding: 18px;
  text-align: center; color: var(--muted); }
.empty p { margin: 6px 0 2px; color: var(--fg-2); font-size: 13px; }
.empty span { font-size: 12px; }
@media (max-width: 720px) {
  .row-hd { flex-wrap: wrap; }
  .row-hd .grp, .row-hd .typ { max-width: none; flex: 1 1 45%; }
}
</style>
