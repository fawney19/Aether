<template>
  <Dialog
    :model-value="modelValue"
    title="提供商批量处理"
    :description="dialogDescription"
    :icon="Users"
    size="2xl"
    persistent
    @update:model-value="handleDialogUpdate"
  >
    <div class="space-y-3.5">
      <!-- 步骤 1：选择动作（顶部分段控件） -->
      <div>
        <div class="grid grid-cols-3 gap-1.5 rounded-xl border bg-muted/30 p-1.5">
          <button
            v-for="action in actionOptions"
            :key="action.value"
            type="button"
            class="group flex items-center justify-center gap-2 rounded-lg px-3 py-2 text-sm font-medium transition-all disabled:opacity-60"
            :class="getSegmentClass(action)"
            :disabled="executing"
            @click="selectedAction = action.value"
          >
            <component
              :is="action.icon"
              class="h-4 w-4 shrink-0"
            />
            <span>{{ action.label }}</span>
          </button>
        </div>
        <p
          class="mt-2 flex items-center gap-1.5 px-1 text-xs leading-relaxed"
          :class="selectedActionMeta?.destructive ? 'text-destructive' : 'text-muted-foreground'"
        >
          <AlertTriangle
            v-if="selectedActionMeta?.destructive"
            class="h-3.5 w-3.5 shrink-0"
          />
          <span>{{ selectedActionMeta?.hint }}</span>
        </p>
      </div>

      <!-- 步骤 2：筛选 + 选择 -->
      <div class="flex flex-wrap items-center gap-2">
        <div class="relative min-w-0 flex-1">
          <Search class="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
          <Input
            v-model="searchText"
            class="h-9 w-full pl-9"
            placeholder="搜索提供商名称 / 类型 / 备注"
            :disabled="executing"
          />
        </div>
        <label class="flex h-9 cursor-pointer items-center gap-2 rounded-md border bg-background px-3 text-xs text-muted-foreground transition-colors hover:bg-muted/40">
          <Checkbox
            :checked="allFilteredSelected"
            :disabled="filteredProviders.length === 0 || executing"
            @update:checked="toggleFilteredSelection"
          />
          <span>全选{{ searchText.trim() ? '筛选结果' : '本页' }}</span>
        </label>
        <Button
          variant="ghost"
          size="sm"
          class="h-9 px-3 text-xs"
          :disabled="selectedCount === 0 || executing"
          @click="clearSelection"
        >
          清空选择
        </Button>
      </div>

      <!-- 提供商列表（占满整宽） -->
      <div class="flex min-w-0 flex-col overflow-hidden rounded-lg border">
        <div class="grid grid-cols-[1.75rem_minmax(0,1fr)_8.5rem] items-center gap-2 border-b bg-muted/30 px-3 py-2 text-[11px] font-medium uppercase tracking-wide text-muted-foreground">
          <Checkbox
            :checked="allFilteredSelected"
            :disabled="filteredProviders.length === 0 || executing"
            @update:checked="toggleFilteredSelection"
          />
          <span class="normal-case tracking-normal">提供商</span>
          <span class="text-right normal-case tracking-normal">资源（活跃 / 总数）</span>
        </div>
        <div class="max-h-[min(52vh,440px)] overflow-y-auto">
          <div
            v-if="filteredProviders.length === 0"
            class="flex flex-col items-center justify-center gap-1.5 py-14 text-center"
          >
            <Search class="h-6 w-6 text-muted-foreground/50" />
            <span class="text-sm text-muted-foreground">无匹配提供商</span>
          </div>
          <label
            v-for="provider in filteredProviders"
            :key="provider.id"
            class="grid cursor-pointer grid-cols-[1.75rem_minmax(0,1fr)_8.5rem] items-center gap-2 border-b px-3 py-2.5 transition-colors last:border-b-0"
            :class="rowClass(provider.id)"
          >
            <Checkbox
              :checked="selectedIdSet.has(provider.id)"
              :disabled="executing"
              @update:checked="(checked) => toggleProvider(provider.id, checked)"
            />
            <div class="min-w-0">
              <div class="flex items-center gap-1.5">
                <span class="truncate text-sm font-medium">{{ provider.name }}</span>
                <Badge
                  :variant="provider.is_active ? 'success' : 'secondary'"
                  class="shrink-0 text-[10px]"
                >
                  {{ provider.is_active ? '活跃' : '停用' }}
                </Badge>
              </div>
              <div class="mt-0.5 flex min-w-0 items-center gap-1.5 text-[11px] text-muted-foreground">
                <span class="shrink-0 rounded bg-muted px-1.5 py-px font-mono text-[10px]">{{ provider.provider_type || 'custom' }}</span>
                <span
                  v-if="provider.description"
                  class="truncate"
                >{{ provider.description }}</span>
              </div>
            </div>
            <div class="flex flex-col items-end gap-0.5 text-[11px] leading-tight text-muted-foreground">
              <span><span class="text-muted-foreground/70">端点</span> <span class="font-medium tabular-nums text-foreground/80">{{ provider.active_endpoints }}/{{ provider.total_endpoints }}</span></span>
              <span><span class="text-muted-foreground/70">账号</span> <span class="font-medium tabular-nums text-foreground/80">{{ provider.active_keys }}/{{ provider.total_keys }}</span></span>
              <span><span class="text-muted-foreground/70">模型</span> <span class="font-medium tabular-nums text-foreground/80">{{ provider.active_models }}/{{ provider.total_models }}</span></span>
            </div>
          </label>
        </div>
      </div>

      <!-- 执行进度 / 结果 -->
      <div
        v-if="executing"
        class="space-y-1.5 rounded-lg border bg-muted/15 px-3 py-2.5"
      >
        <div class="flex items-center justify-between text-xs">
          <span class="truncate text-foreground">{{ progressLabel }}</span>
          <span class="shrink-0 font-medium tabular-nums text-muted-foreground">{{ progressDone }} / {{ progressTotal }}</span>
        </div>
        <div class="h-1.5 overflow-hidden rounded-full bg-muted">
          <div
            class="h-full rounded-full transition-all duration-150"
            :class="selectedActionMeta?.destructive ? 'bg-destructive' : 'bg-primary'"
            :style="{ width: `${progressPercent}%` }"
          />
        </div>
      </div>

      <div
        v-else-if="lastResultMessage"
        class="rounded-lg border bg-background px-3 py-2.5 text-xs text-muted-foreground"
      >
        {{ lastResultMessage }}
      </div>
    </div>

    <template #footer>
      <div class="flex w-full flex-wrap items-center justify-between gap-3">
        <div class="flex items-center gap-2 text-xs text-muted-foreground">
          <span>已选 <span class="font-semibold tabular-nums text-foreground">{{ selectedCount }}</span></span>
          <span class="text-border">·</span>
          <span>本页 <span class="tabular-nums">{{ providers.length }}</span></span>
          <span class="text-emerald-600 dark:text-emerald-400">活跃 <span class="tabular-nums">{{ activeProviderCount }}</span></span>
          <span>停用 <span class="tabular-nums">{{ inactiveProviderCount }}</span></span>
        </div>
        <div class="flex items-center gap-2">
          <Button
            variant="outline"
            :disabled="executing"
            @click="emit('update:modelValue', false)"
          >
            关闭
          </Button>
          <Button
            :variant="selectedAction === 'delete' ? 'destructive' : 'default'"
            :disabled="!canExecute"
            @click="confirmAndExecute"
          >
            {{ executeButtonLabel }}
          </Button>
        </div>
      </div>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { computed, ref, watch, type Component } from 'vue'
import { AlertTriangle, Power, PowerOff, Search, Trash2, Users } from 'lucide-vue-next'
import { Badge, Button, Checkbox, Dialog, Input } from '@/components/ui'
import {
  deleteProvider,
  getProviderDeleteTask,
  updateProvider,
  type ProviderWithEndpointsSummary,
} from '@/api/endpoints'
import { useConfirm } from '@/composables/useConfirm'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'

type ProviderBatchAction = 'enable' | 'disable' | 'delete'

interface ProviderBatchActionOption {
  value: ProviderBatchAction
  label: string
  hint: string
  icon: Component
  destructive?: boolean
}

const props = defineProps<{
  modelValue: boolean
  providers: ProviderWithEndpointsSummary[]
}>()

const emit = defineEmits<{
  'update:modelValue': [value: boolean]
  changed: []
}>()

const { confirm } = useConfirm()
const { success, warning, error: showError } = useToast()

const actionOptions: ProviderBatchActionOption[] = [
  { value: 'enable', label: '启用', hint: '恢复所选提供商参与调度。', icon: Power },
  { value: 'disable', label: '停用', hint: '停止所选提供商参与调度，保留配置。', icon: PowerOff },
  { value: 'delete', label: '删除', hint: '永久删除所选提供商及其端点、账号和配置，此操作不可恢复。', icon: Trash2, destructive: true },
]

const SEGMENT_ACCENT: Record<ProviderBatchAction, string> = {
  enable: 'bg-background text-emerald-600 shadow-sm ring-1 ring-emerald-500/25 dark:text-emerald-400',
  disable: 'bg-background text-primary shadow-sm ring-1 ring-primary/25',
  delete: 'bg-background text-destructive shadow-sm ring-1 ring-destructive/30',
}

const searchText = ref('')
const selectedProviderIds = ref<string[]>([])
const selectedAction = ref<ProviderBatchAction>('disable')
const executing = ref(false)
const progressDone = ref(0)
const progressTotal = ref(0)
const progressLabel = ref('')
const lastResultMessage = ref('')

const dialogDescription = computed(() => '批量启用、停用或删除当前页提供商')
const selectedIdSet = computed(() => new Set(selectedProviderIds.value))
const selectedCount = computed(() => selectedProviderIds.value.length)
const selectedActionLabel = computed(() => actionOptions.find(action => action.value === selectedAction.value)?.label || '')
const selectedActionMeta = computed(() => actionOptions.find(action => action.value === selectedAction.value))
const executeButtonLabel = computed(() => {
  if (executing.value) return '执行中...'
  if (selectedCount.value > 0) return `${selectedActionLabel.value} ${selectedCount.value} 项`
  return `执行${selectedActionLabel.value}`
})
const activeProviderCount = computed(() => props.providers.filter(provider => provider.is_active).length)
const inactiveProviderCount = computed(() => props.providers.length - activeProviderCount.value)
const providerById = computed(() => new Map(props.providers.map(provider => [provider.id, provider])))
const filteredProviders = computed(() => {
  const keyword = searchText.value.trim().toLowerCase()
  if (!keyword) return props.providers
  return props.providers.filter((provider) => {
    return [
      provider.name,
      provider.provider_type,
      provider.description,
      provider.website,
    ].some(value => String(value || '').toLowerCase().includes(keyword))
  })
})
const allFilteredSelected = computed(() => {
  return filteredProviders.value.length > 0
    && filteredProviders.value.every(provider => selectedIdSet.value.has(provider.id))
})
const progressPercent = computed(() => {
  if (progressTotal.value <= 0) return 0
  return Math.min(100, Math.round((progressDone.value / progressTotal.value) * 100))
})
const canExecute = computed(() => selectedCount.value > 0 && !executing.value)

function toggleProvider(providerId: string, checked: boolean): void {
  const next = new Set(selectedProviderIds.value)
  if (checked) next.add(providerId)
  else next.delete(providerId)
  selectedProviderIds.value = [...next]
}

function toggleFilteredSelection(checked: boolean): void {
  const next = new Set(selectedProviderIds.value)
  for (const provider of filteredProviders.value) {
    if (checked) next.add(provider.id)
    else next.delete(provider.id)
  }
  selectedProviderIds.value = [...next]
}

function clearSelection(): void {
  selectedProviderIds.value = []
}

function handleDialogUpdate(open: boolean): void {
  if (executing.value && !open) return
  emit('update:modelValue', open)
}

function getSegmentClass(action: ProviderBatchActionOption): string {
  if (selectedAction.value !== action.value) {
    return 'text-muted-foreground hover:bg-background/60 hover:text-foreground'
  }
  return SEGMENT_ACCENT[action.value]
}

function rowClass(providerId: string): string {
  if (!selectedIdSet.value.has(providerId)) {
    return 'hover:bg-muted/40'
  }
  return selectedActionMeta.value?.destructive
    ? 'bg-destructive/5 hover:bg-destructive/10'
    : 'bg-primary/5 hover:bg-primary/10'
}

async function confirmAndExecute(): Promise<void> {
  if (!canExecute.value) return
  const action = actionOptions.find(item => item.value === selectedAction.value)
  const actionLabel = action?.label || '批量操作'
  const confirmed = await confirm({
    title: `批量${actionLabel}提供商`,
    message: selectedAction.value === 'delete'
      ? `将删除 ${selectedCount.value} 个提供商，并同时删除其所有端点、账号和配置。此操作不可恢复，是否继续？`
      : `将对 ${selectedCount.value} 个提供商执行：${actionLabel}，是否继续？`,
    confirmText: selectedAction.value === 'delete' ? '确认删除' : '确认执行',
    ...(selectedAction.value === 'delete' ? { variant: 'destructive' as const } : {}),
  })
  if (!confirmed) return
  await executeBatchAction()
}

const DELETE_POLL_INTERVAL_MS = 2000
const DELETE_POLL_MAX_MS = 30 * 60 * 1000
const DELETE_POLL_MAX_FAILURES = 3

async function pollProviderDeleteTask(providerId: string, taskId: string): Promise<void> {
  const deadline = Date.now() + DELETE_POLL_MAX_MS
  let consecutiveFailures = 0
  while (Date.now() < deadline) {
    try {
      const task = await getProviderDeleteTask(providerId, taskId)
      consecutiveFailures = 0
      if (task.status === 'completed') return
      if (task.status === 'failed') {
        throw new Error(task.message || 'provider delete task failed')
      }
    } catch (err) {
      consecutiveFailures += 1
      if (consecutiveFailures >= DELETE_POLL_MAX_FAILURES) {
        throw err
      }
    }
    await new Promise(resolve => setTimeout(resolve, DELETE_POLL_INTERVAL_MS))
  }
  throw new Error('provider delete task timeout')
}

async function executeBatchAction(): Promise<void> {
  if (executing.value) return

  const targets = selectedProviderIds.value
    .map(id => providerById.value.get(id))
    .filter((provider): provider is ProviderWithEndpointsSummary => Boolean(provider))
  if (targets.length === 0) {
    warning('请先选择提供商')
    return
  }

  executing.value = true
  progressDone.value = 0
  progressTotal.value = targets.length
  lastResultMessage.value = ''
  let successCount = 0
  let failedCount = 0

  try {
    for (const provider of targets) {
      progressLabel.value = `正在${selectedActionLabel.value}：${provider.name}`
      try {
        if (selectedAction.value === 'delete') {
          const result = await deleteProvider(provider.id)
          await pollProviderDeleteTask(provider.id, result.task_id)
        } else {
          await updateProvider(provider.id, { is_active: selectedAction.value === 'enable' })
        }
        successCount += 1
      } catch (err) {
        failedCount += 1
        // eslint-disable-next-line no-console
        console.error(`[ProviderBatchActionDialog] ${selectedAction.value} failed (${provider.id}):`, err)
      } finally {
        progressDone.value += 1
      }
    }

    lastResultMessage.value = `执行完成：成功 ${successCount}，失败 ${failedCount}`
    if (failedCount > 0) warning(lastResultMessage.value)
    else success(lastResultMessage.value)
    if (successCount > 0) {
      clearSelection()
      emit('changed')
    }
  } catch (err) {
    showError(parseApiError(err, '批量处理提供商失败'), '错误')
  } finally {
    executing.value = false
    progressDone.value = 0
    progressTotal.value = 0
    progressLabel.value = ''
  }
}

watch(
  () => props.modelValue,
  (open) => {
    if (!open) return
    searchText.value = ''
    selectedProviderIds.value = []
    selectedAction.value = 'disable'
    lastResultMessage.value = ''
  },
)

watch(
  () => props.providers.map(provider => provider.id),
  () => {
    const availableIds = new Set(props.providers.map(provider => provider.id))
    selectedProviderIds.value = selectedProviderIds.value.filter(id => availableIds.has(id))
  },
)
</script>
