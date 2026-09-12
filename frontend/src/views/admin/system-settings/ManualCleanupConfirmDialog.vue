<template>
  <Dialog
    :open="open"
    size="lg"
    title="立即清理请求记录"
    description="默认按当前分级保留策略执行，也可以选择指定范围。操作不可逆。"
    :persistent="isLocked"
    @update:open="handleOpenChange"
  >
    <div class="px-4 sm:px-6 py-4 space-y-4">
      <div>
        <Label class="block text-sm font-medium">
          清理方式
        </Label>
        <div class="mt-2 grid grid-cols-1 sm:grid-cols-3 gap-2">
          <button
            v-for="item in modeOptions"
            :key="item.value"
            type="button"
            class="rounded-md border px-3 py-2 text-left text-sm transition-colors"
            :class="mode === item.value ? 'border-primary bg-primary/10 text-primary' : 'border-border bg-card hover:bg-muted/60'"
            :disabled="isLocked"
            @click="setMode(item.value)"
          >
            <span class="font-medium">{{ item.label }}</span>
            <span class="mt-1 block text-xs text-muted-foreground">{{ item.description }}</span>
          </button>
        </div>
      </div>

      <div v-if="mode === 'older_than_days'">
        <Label
          for="manual-cleanup-older-than-days"
          class="block text-sm font-medium"
        >
          清理 N 天前的记录
        </Label>
        <Input
          id="manual-cleanup-older-than-days"
          :model-value="olderThanDays ?? ''"
          type="number"
          min="1"
          placeholder="例如 30"
          class="mt-1"
          :disabled="isLocked"
          @update:model-value="handleDaysChange"
        />
        <p class="mt-1 text-xs text-muted-foreground">
          该值会与当前策略取更保守的时间点，不会清理比策略更新的数据。
        </p>
      </div>

      <div>
        <Label class="block text-sm font-medium">
          清理范围
        </Label>
        <div class="mt-2 grid grid-cols-1 sm:grid-cols-2 gap-2">
          <label
            v-for="target in targetOptions"
            :key="target.value"
            class="flex min-h-16 items-start gap-3 rounded-md border border-border bg-card px-3 py-2"
          >
            <Checkbox
              class="mt-0.5"
              :checked="selectedTargets.includes(target.value)"
              :disabled="isLocked"
              @update:checked="toggleTarget(target.value, $event)"
            />
            <span>
              <span class="block text-sm font-medium">{{ target.label }}</span>
              <span class="block text-xs text-muted-foreground">{{ target.description }}</span>
            </span>
          </label>
        </div>
        <p
          v-if="mode === 'before_now'"
          class="mt-2 text-xs text-amber-600"
        >
          当前时刻之前模式只允许清理详细请求体和压缩请求体，不会清请求头或整条记录。
        </p>
        <p
          v-if="targetError"
          class="mt-2 text-xs text-destructive"
        >
          {{ targetError }}
        </p>
      </div>

      <div class="rounded-md border border-border bg-muted/30 px-4 py-3">
        <div class="flex items-center justify-between">
          <h4 class="text-sm font-medium">
            预计影响
          </h4>
          <button
            v-if="!previewLoading"
            type="button"
            class="text-xs text-muted-foreground hover:text-foreground"
            :disabled="isLocked"
            @click="loadPreview"
          >
            刷新预估
          </button>
          <span
            v-else
            class="text-xs text-muted-foreground"
          >
            正在计算…
          </span>
        </div>
        <div
          v-if="previewError"
          class="mt-2 text-xs text-destructive"
        >
          {{ previewError }}
        </div>
        <div
          v-else-if="preview"
          class="mt-2 grid grid-cols-2 gap-y-1 gap-x-4 text-xs text-muted-foreground"
        >
          <div>详细记录待压缩</div>
          <div class="text-right text-foreground">
            {{ formatCount(preview.counts.detail) }}
          </div>
          <div>压缩记录待清体</div>
          <div class="text-right text-foreground">
            {{ formatCount(preview.counts.compressed) }}
          </div>
          <div>请求头待清空</div>
          <div class="text-right text-foreground">
            {{ formatCount(preview.counts.header) }}
          </div>
          <div>整条记录待删除</div>
          <div class="text-right text-destructive font-medium">
            {{ formatCount(preview.counts.log) }}
          </div>
        </div>
        <div
          v-else-if="!previewLoading"
          class="mt-2 text-xs text-muted-foreground"
        >
          尚未计算预估数据
        </div>
      </div>

      <div
        v-if="activeTask || taskError"
        class="rounded-md border border-border bg-card px-4 py-3"
      >
        <div class="flex items-center justify-between gap-3">
          <div class="min-w-0">
            <div class="text-sm font-medium">
              {{ activeTask?.message || '请求记录清理失败' }}
            </div>
            <div class="mt-1 text-xs text-muted-foreground">
              {{ activeTask ? cleanupStatusLabel(activeTask.status) : taskError }}
            </div>
          </div>
          <span
            v-if="activeTask"
            :class="cleanupStatusClass(activeTask.status)"
            class="shrink-0 text-xs"
          >
            {{ cleanupStatusLabel(activeTask.status) }}
          </span>
        </div>
        <div
          v-if="activeTask"
          class="mt-3 h-2 overflow-hidden rounded-full bg-muted"
        >
          <div
            class="h-full rounded-full bg-primary transition-all"
            :class="{ 'animate-pulse': activeTask.status === 'processing' }"
            :style="{ width: `${taskProgressPercent}%` }"
          />
        </div>
        <div
          v-if="activeTask"
          class="mt-2 text-xs text-muted-foreground"
        >
          {{ cleanupSummaryText(activeTask.summary) }}
        </div>
      </div>

      <div>
        <Label
          for="manual-cleanup-confirm-phrase"
          class="block text-sm font-medium"
        >
          输入「{{ confirmPhrase }}」以确认清理
        </Label>
        <Input
          id="manual-cleanup-confirm-phrase"
          :model-value="typedPhrase"
          class="mt-1"
          autocomplete="off"
          :placeholder="confirmPhrase"
          :disabled="isLocked || isFinished"
          @update:model-value="typedPhrase = String($event)"
          @keydown.enter.prevent="maybeSubmitOnEnter"
        />
        <p class="mt-1 text-xs text-muted-foreground">
          确认后会在当前弹窗中显示执行状态，完成前不能关闭。
        </p>
      </div>
    </div>

    <template #footer>
      <Button
        v-if="!isFinished"
        variant="destructive"
        :disabled="!canSubmit"
        @click="handleConfirm"
      >
        {{ isLocked ? '清理中…' : '确认清理' }}
      </Button>
      <Button
        variant="outline"
        :disabled="isLocked"
        @click="handleCancel"
      >
        {{ isFinished ? '关闭' : '取消' }}
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { Dialog } from '@/components/ui'
import Button from '@/components/ui/button.vue'
import Checkbox from '@/components/ui/checkbox.vue'
import Input from '@/components/ui/input.vue'
import Label from '@/components/ui/label.vue'
import {
  adminApi,
  type CleanupRunRecord,
  type ManualUsageCleanupPreview,
  type ManualUsageCleanupRequest,
} from '@/api/admin'
import { parseApiError } from '@/utils/errorParser'
import {
  MANUAL_USAGE_CLEANUP_CONFIRM_PHRASE,
  allowedTargetsForMode,
  defaultManualCleanupTargets,
  isConfirmPhraseMatched,
  normalizeManualCleanupTargets,
  normalizeOlderThanDaysInput,
  type ManualCleanupMode,
  type ManualCleanupTarget,
} from './manualCleanupForm'

const props = defineProps<{
  open: boolean
}>()

const emit = defineEmits<{
  'update:open': [value: boolean]
  'running-change': [value: boolean]
  completed: [task: CleanupRunRecord]
}>()

const confirmPhrase = MANUAL_USAGE_CLEANUP_CONFIRM_PHRASE

const mode = ref<ManualCleanupMode>('policy')
const olderThanDays = ref<number | null>(null)
const selectedTargets = ref<ManualCleanupTarget[]>(defaultManualCleanupTargets('policy'))
const targetsTouched = ref(false)
const typedPhrase = ref('')
const preview = ref<ManualUsageCleanupPreview | null>(null)
const previewLoading = ref(false)
const previewError = ref<string | null>(null)
const submitting = ref(false)
const activeTask = ref<CleanupRunRecord | null>(null)
const taskError = ref<string | null>(null)

let previewDebounceTimer: ReturnType<typeof setTimeout> | null = null
let previewSeq = 0
let taskPollTimer: number | null = null

const modeOptions: Array<{ value: ManualCleanupMode; label: string; description: string }> = [
  { value: 'policy', label: '按当前策略', description: '沿用页面上配置的保留天数' },
  { value: 'older_than_days', label: '指定天数前', description: '在策略内取更保守时间点' },
  { value: 'before_now', label: '当前时刻之前', description: '只清已选请求体内容' },
]

const targetLabels: Record<ManualCleanupTarget, { label: string; description: string }> = {
  detail_body: { label: '详细请求体', description: '把详细 body 移入压缩/外置存储' },
  compressed_body: { label: '压缩请求体', description: '删除已压缩或外置的 body 内容' },
  headers: { label: '请求头', description: '清空请求/响应 headers 字段' },
  records: { label: '整条记录', description: '删除超过记录保留期的 usage 行' },
}

const targetOptions = computed(() =>
  allowedTargetsForMode(mode.value).map(value => ({
    value,
    ...targetLabels[value],
  }))
)

const normalizedTargets = computed(() =>
  normalizeManualCleanupTargets(mode.value, selectedTargets.value)
)

const targetError = computed(() => {
  if (normalizedTargets.value.length === 0) return '至少选择一个清理范围'
  return null
})

const currentTaskRunning = computed(() => activeTask.value?.status === 'processing')
const isLocked = computed(() => submitting.value || currentTaskRunning.value)
const isFinished = computed(() =>
  activeTask.value?.status === 'completed' || activeTask.value?.status === 'failed'
)

const canSubmit = computed(
  () =>
    !isLocked.value &&
    !isFinished.value &&
    !previewLoading.value &&
    !targetError.value &&
    modeIsValid.value &&
    isConfirmPhraseMatched(typedPhrase.value),
)

const modeIsValid = computed(() => mode.value !== 'older_than_days' || olderThanDays.value !== null)

const taskProgressPercent = computed(() => {
  const task = activeTask.value
  if (!task) return 0
  if (task.status === 'completed') return 100
  if (task.status === 'failed') return 100
  const raw = task.summary?.progress_percent
  if (typeof raw === 'number' && Number.isFinite(raw) && raw > 0) {
    return Math.max(1, Math.min(99, Math.round(raw)))
  }
  return 12
})

watch(
  () => props.open,
  (isOpen) => {
    if (isOpen) {
      resetForm()
      void loadPreview()
    } else {
      clearPreviewTimer()
      stopTaskPolling()
    }
  },
)

function resetForm() {
  mode.value = 'policy'
  olderThanDays.value = null
  selectedTargets.value = defaultManualCleanupTargets('policy')
  targetsTouched.value = false
  typedPhrase.value = ''
  preview.value = null
  previewError.value = null
  previewLoading.value = false
  submitting.value = false
  activeTask.value = null
  taskError.value = null
  emit('running-change', false)
}

function setMode(nextMode: ManualCleanupMode) {
  if (isLocked.value || mode.value === nextMode) return
  mode.value = nextMode
  olderThanDays.value = null
  selectedTargets.value = defaultManualCleanupTargets(nextMode)
  targetsTouched.value = false
  activeTask.value = null
  taskError.value = null
  schedulePreview()
}

function handleDaysChange(value: string | number) {
  olderThanDays.value = normalizeOlderThanDaysInput(value)
  schedulePreview()
}

function toggleTarget(target: ManualCleanupTarget, checked: boolean) {
  targetsTouched.value = true
  activeTask.value = null
  taskError.value = null
  const current = new Set(selectedTargets.value)
  if (checked) {
    current.add(target)
  } else {
    current.delete(target)
  }
  selectedTargets.value = normalizeManualCleanupTargets(mode.value, Array.from(current))
  schedulePreview()
}

function buildRequest(): ManualUsageCleanupRequest {
  const request: ManualUsageCleanupRequest = { mode: mode.value }
  if (mode.value === 'older_than_days' && olderThanDays.value !== null) {
    request.older_than_days = olderThanDays.value
  }
  if (targetsTouched.value) {
    request.targets = normalizedTargets.value
  }
  return request
}

function schedulePreview() {
  clearPreviewTimer()
  previewDebounceTimer = setTimeout(() => {
    previewDebounceTimer = null
    void loadPreview()
  }, 300)
}

function clearPreviewTimer() {
  if (previewDebounceTimer) {
    clearTimeout(previewDebounceTimer)
    previewDebounceTimer = null
  }
}

async function loadPreview() {
  const seq = ++previewSeq
  previewLoading.value = true
  previewError.value = null
  try {
    const result = await adminApi.previewManualUsageCleanup(buildRequest())
    if (seq === previewSeq) {
      preview.value = result
    }
  } catch (error) {
    if (seq === previewSeq) {
      preview.value = null
      previewError.value = parseApiError(error)
    }
  } finally {
    if (seq === previewSeq) {
      previewLoading.value = false
    }
  }
}

function handleOpenChange(value: boolean) {
  if (!value && isLocked.value) {
    return
  }
  emit('update:open', value)
}

function handleCancel() {
  if (isLocked.value) return
  emit('update:open', false)
}

function maybeSubmitOnEnter() {
  if (canSubmit.value) {
    void handleConfirm()
  }
}

async function handleConfirm() {
  if (!canSubmit.value) return
  submitting.value = true
  taskError.value = null
  try {
    const response = await adminApi.runManualUsageCleanup(buildRequest())
    if ('detail' in response) {
      taskError.value = response.message
      emit('running-change', false)
      return
    }
    activeTask.value = response.task
    emit('running-change', response.task.status === 'processing')
    if (response.task.status === 'processing') {
      startTaskPolling(response.task.id)
    } else {
      emit('completed', response.task)
    }
  } catch (error) {
    taskError.value = parseApiError(error)
    emit('running-change', false)
  } finally {
    submitting.value = false
  }
}

function startTaskPolling(taskId: string) {
  stopTaskPolling()
  void pollTask(taskId)
  taskPollTimer = window.setInterval(() => {
    void pollTask(taskId)
  }, 1_500)
}

function stopTaskPolling() {
  if (taskPollTimer) {
    window.clearInterval(taskPollTimer)
    taskPollTimer = null
  }
}

async function pollTask(taskId: string) {
  try {
    const response = await adminApi.getCleanupRuns()
    const task = response.items.find(item => item.id === taskId)
    if (!task) return
    activeTask.value = task
    const running = task.status === 'processing'
    emit('running-change', running)
    if (!running) {
      stopTaskPolling()
      emit('completed', task)
      void loadPreview()
    }
  } catch (error) {
    taskError.value = parseApiError(error)
  }
}

function cleanupStatusLabel(status: string): string {
  if (status === 'processing') return '执行中'
  if (status === 'failed') return '失败'
  return '完成'
}

function cleanupStatusClass(status: string): string {
  if (status === 'processing') return 'text-amber-500'
  if (status === 'failed') return 'text-destructive'
  return 'text-emerald-500'
}

function cleanupSummaryText(summary: Record<string, unknown>): string {
  const total = typeof summary.total === 'number' ? summary.total : null
  if (total !== null && total > 0) return `影响 ${total} 项`
  const entries = Object.entries(summary)
    .filter(([key, value]) => key !== 'progress_percent' && typeof value === 'number' && value > 0)
    .map(([key, value]) => `${summaryLabel(key)} ${value}`)
  return entries.length > 0 ? entries.join(' / ') : '等待后台返回结果'
}

function summaryLabel(key: string): string {
  const labels: Record<string, string> = {
    body_externalized: '详细体',
    legacy_body_refs_migrated: '迁移',
    body_cleaned: '清体',
    header_cleaned: '清头',
    keys_cleaned: 'Key',
    records_deleted: '删记录',
  }
  return labels[key] || key
}

function formatCount(value: number): string {
  return value.toLocaleString()
}

onBeforeUnmount(() => {
  clearPreviewTimer()
  stopTaskPolling()
})
</script>
