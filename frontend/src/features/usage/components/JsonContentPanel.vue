<template>
  <div class="json-content-panel">
    <div class="json-panel-toolbar">
      <span class="json-panel-title">{{ title }}</span>
      <div class="json-panel-actions">
        <slot name="toolbar-actions-before" />
        <button
          :title="panelExpandDepth === 0 ? '展开全部' : '收缩全部'"
          class="json-panel-action"
          :class="{ 'is-disabled': expandDisabled }"
          :disabled="expandDisabled"
          type="button"
          @click="toggleExpand"
        >
          <Maximize2
            v-if="panelExpandDepth === 0"
            class="w-3.5 h-3.5"
          />
          <Minimize2
            v-else
            class="w-3.5 h-3.5"
          />
        </button>
        <button
          :title="panelCopied ? '已复制' : '复制'"
          class="json-panel-action"
          :class="{ 'is-disabled': copyDisabled }"
          :disabled="copyDisabled"
          type="button"
          @click="copyJson"
        >
          <Check
            v-if="panelCopied"
            class="w-3.5 h-3.5 text-green-500"
          />
          <Copy
            v-else
            class="w-3.5 h-3.5"
          />
        </button>
      </div>
    </div>
    <div class="json-panel-content">
      <slot :expand-depth="panelExpandDepth">
        <JsonContent
          :data="data"
          view-mode="formatted"
          :expand-depth="panelExpandDepth"
          :is-dark="isDark"
          :empty-message="emptyMessage"
        />
      </slot>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, ref } from 'vue'
import { Check, Copy, Maximize2, Minimize2 } from 'lucide-vue-next'
import { useClipboard } from '@/composables/useClipboard'
import JsonContent from './RequestDetailDrawer/JsonContent.vue'

type JsonValue = Record<string, unknown> | unknown[] | string | number | boolean | null | undefined

const props = withDefaults(defineProps<{
  data: JsonValue
  isDark: boolean
  emptyMessage?: string
  title?: string
  maxHeight?: string
  expandDepth?: number
  copied?: boolean
  customCopy?: boolean
  expandDisabled?: boolean
  copyDisabled?: boolean
}>(), {
  emptyMessage: '无数据',
  title: 'JSON',
  maxHeight: '360px',
  expandDepth: undefined,
  copied: undefined,
  customCopy: false,
  expandDisabled: false,
  copyDisabled: false,
})

const emit = defineEmits<{
  'update:expandDepth': [value: number]
  copy: []
}>()

const { copyToClipboard } = useClipboard()
const internalExpandDepth = ref(0)
const internalCopied = ref(false)

const panelExpandDepth = computed({
  get: () => props.expandDepth ?? internalExpandDepth.value,
  set: (value: number) => {
    if (props.expandDepth === undefined) {
      internalExpandDepth.value = value
    }
    emit('update:expandDepth', value)
  },
})

const panelCopied = computed(() => props.copied ?? internalCopied.value)

const toggleExpand = () => {
  if (props.expandDisabled) return
  panelExpandDepth.value = panelExpandDepth.value === 0 ? 999 : 0
}

const copyJson = () => {
  if (props.copyDisabled) return
  if (props.customCopy) {
    emit('copy')
    return
  }
  if (props.data == null) return

  copyToClipboard(JSON.stringify(props.data, null, 2), false)
  internalCopied.value = true
  window.setTimeout(() => {
    internalCopied.value = false
  }, 2000)
}
</script>

<style scoped>
.json-content-panel {
  border: 1px solid var(--border);
  border-radius: 10px;
  overflow: hidden;
  background: hsl(var(--card));
  box-shadow: 0 1px 2px color-mix(in srgb, var(--foreground) 6%, transparent);
}

.json-panel-toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 0.75rem;
  padding: 0.35rem 0.55rem 0.35rem 0.75rem;
  border-bottom: 1px solid var(--border);
  background: hsl(var(--muted) / 0.55);
}

.json-panel-title {
  font-size: 0.72rem;
  font-weight: 600;
  color: hsl(var(--muted-foreground));
  letter-spacing: 0.02em;
}

.json-panel-actions {
  display: inline-flex;
  align-items: center;
  gap: 0.15rem;
}

.json-panel-action {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 1.45rem;
  height: 1.45rem;
  border: 0;
  border-radius: 6px;
  background: transparent;
  color: hsl(var(--muted-foreground));
  cursor: pointer;
  transition: background-color 0.15s ease, color 0.15s ease;
}

.json-panel-action:hover {
  background: hsl(var(--muted));
  color: hsl(var(--foreground));
}

.json-panel-action.is-disabled,
.json-panel-action:disabled {
  color: hsl(var(--muted-foreground) / 0.4);
  cursor: not-allowed;
}

.json-panel-action.is-disabled:hover,
.json-panel-action:disabled:hover {
  background: transparent;
  color: hsl(var(--muted-foreground) / 0.4);
}

.json-panel-content :deep(.json-viewer) {
  max-height: v-bind(maxHeight);
}

.json-panel-content :deep(.bg-muted\/30) {
  border: 0;
  border-radius: 0;
  background: transparent;
}
</style>
