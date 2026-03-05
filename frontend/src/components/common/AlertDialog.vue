<template>
  <Dialog
    :model-value="modelValue"
    :z-index="120"
    @update:model-value="handleClose"
  >
    <template #header>
      <div class="border-b border-border px-6 py-4">
        <div class="flex items-center gap-3">
          <component
            :is="icon"
            class="h-5 w-5 flex-shrink-0"
            :class="iconColorClass"
          />
          <div class="flex-1 min-w-0">
            <h3 class="text-lg font-semibold text-foreground leading-tight">
              {{ title }}
            </h3>
          </div>
        </div>
      </div>
    </template>

    <template #default>
      <!-- 描述 -->
      <div class="space-y-3">
        <p
          v-for="(line, index) in descriptionLines"
          :key="index"
          :class="getLineClass(index)"
          v-html="renderLine(line)"
        />
      </div>

      <!-- 自定义内容插槽 -->
      <slot />
    </template>

    <template #footer>
      <!-- 取消按钮 -->
      <Button
        variant="outline"
        :disabled="loading"
        class="h-10 px-5"
        @click="handleCancel"
      >
        {{ cancelText }}
      </Button>

      <!-- 确认按钮 -->
      <Button
        :variant="confirmVariant"
        :disabled="loading"
        class="h-10 px-5"
        @click="handleConfirm"
      >
        <Loader2
          v-if="loading"
          class="animate-spin h-4 w-4 mr-2"
        />
        {{ confirmText }}
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { Dialog } from '@/components/ui'
import Button from '@/components/ui/button.vue'
import { AlertTriangle, AlertCircle, Info, Trash2, HelpCircle, Loader2 } from 'lucide-vue-next'

export type AlertType = 'danger' | 'destructive' | 'warning' | 'info' | 'question'

interface Props {
  modelValue: boolean
  title: string
  description: string
  type?: AlertType
  confirmText?: string
  cancelText?: string
  loading?: boolean
}

interface Emits {
  (e: 'update:modelValue', value: boolean): void
  (e: 'confirm'): void
  (e: 'cancel'): void
}

const props = withDefaults(defineProps<Props>(), {
  type: 'warning',
  confirmText: '确认',
  cancelText: '取消',
  loading: false
})

const emit = defineEmits<Emits>()

// 解析描述文本为多行
const descriptionLines = computed(() => {
  return props.description.split('\n').filter(line => line.trim())
})

function escapeHtml(raw: string): string {
  return raw
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&#39;')
}

function renderLine(line: string): string {
  const escaped = escapeHtml(line)
  // 支持最小语法加粗：**text**
  return escaped.replace(
    /\*\*(.+?)\*\*/g,
    '<strong class="font-semibold text-foreground">$1</strong>'
  )
}

// 根据行索引获取样式（中间行高亮）
function getLineClass(index: number): string {
  const total = descriptionLines.value.length
  if (total <= 1) {
    return 'text-sm text-muted-foreground'
  }
  // 中间行（不是第一行也不是最后一行）使用高亮样式
  if (index > 0 && index < total - 1) {
    return 'text-sm font-mono font-medium text-foreground bg-muted/50 px-3 py-2 rounded-md'
  }
  return 'text-sm text-muted-foreground'
}

// 根据类型获取图标
const icon = computed(() => {
  switch (props.type) {
    case 'danger':
    case 'destructive':
      return Trash2
    case 'warning':
      return AlertTriangle
    case 'info':
      return Info
    case 'question':
      return HelpCircle
    default:
      return AlertCircle
  }
})

// 根据类型获取图标颜色样式
const iconColorClass = computed(() => {
  switch (props.type) {
    case 'danger':
    case 'destructive':
      return 'text-rose-600 dark:text-rose-400'
    case 'warning':
      return 'text-amber-600 dark:text-amber-400'
    case 'info':
      return 'text-primary'
    case 'question':
      return 'text-gray-600 dark:text-muted-foreground'
    default:
      return 'text-primary'
  }
})

// 根据类型获取确认按钮样式
const confirmVariant = computed(() => {
  switch (props.type) {
    case 'danger':
    case 'destructive':
      return 'destructive' as const
    case 'warning':
    case 'info':
    case 'question':
    default:
      return 'default' as const
  }
})

function handleConfirm() {
  emit('confirm')
}

function handleCancel() {
  emit('cancel')
  emit('update:modelValue', false)
}

function handleClose(value: boolean) {
  if (!value && !props.loading) {
    emit('update:modelValue', value)
    emit('cancel')
  }
}
</script>
