<template>
  <Dialog
    :model-value="modelValue"
    title="提供商批量处理"
    :description="dialogDescription"
    :icon="Users"
    size="4xl"
    persistent
    @update:model-value="handleDialogUpdate"
  >
    <div class="space-y-3.5">
      <div class="space-y-1.5">
        <div class="flex items-center justify-between gap-2">
          <span class="text-xs font-medium text-muted-foreground">选择提供商</span>
          <span class="text-xs text-muted-foreground">{{ selectedCount }}/{{ providers.length }}</span>
        </div>
        <MultiSelect
          :model-value="selectedProviderIds"
          :options="providerOptions"
          placeholder="选择要批量处理的提供商"
          empty-text="当前范围暂无提供商"
          no-results-text="未找到提供商"
          trigger-class="h-10 rounded-md"
          dropdown-min-width="28rem"
          :disabled="executing"
          :search-threshold="4"
          @update:model-value="selectedProviderIds = $event"
        />
        <p class="text-xs text-muted-foreground">
          选择范围来自当前列表页和筛选结果，不会跨页自动包含其它提供商。
        </p>
      </div>

      <div class="grid grid-cols-3 gap-1.5 rounded-xl border bg-muted/30 p-1.5">
        <button
          v-for="mode in modeOptions"
          :key="mode.value"
          type="button"
          class="group flex items-center justify-center gap-2 rounded-lg px-3 py-2 text-sm font-medium transition-all disabled:opacity-60"
          :class="getModeClass(mode.value)"
          :disabled="executing"
          @click="selectedMode = mode.value"
        >
          <component
            :is="mode.icon"
            class="h-4 w-4 shrink-0"
          />
          <span>{{ mode.label }}</span>
        </button>
      </div>

      <div
        v-if="selectedMode === 'status'"
        class="space-y-3 rounded-lg border p-3"
      >
        <div class="grid gap-2 md:grid-cols-3">
          <button
            v-for="action in statusActionOptions"
            :key="action.value"
            type="button"
            class="flex min-h-20 flex-col items-start justify-center gap-1.5 rounded-md border px-3 py-2 text-left transition-colors disabled:opacity-60"
            :class="getStatusActionClass(action)"
            :disabled="executing"
            @click="statusAction = action.value"
          >
            <span class="flex items-center gap-2 text-sm font-medium">
              <component
                :is="action.icon"
                class="h-4 w-4"
              />
              {{ action.label }}
            </span>
            <span
              class="text-xs leading-relaxed"
              :class="action.destructive && statusAction === action.value ? 'text-destructive/80' : 'text-muted-foreground'"
            >
              {{ action.hint }}
            </span>
          </button>
        </div>
      </div>

      <ProviderBatchBasicInfoPanel
        v-else-if="selectedMode === 'basic'"
        :state="basicState"
        :disabled="executing"
        @update:state="basicState = $event"
      />

      <ProviderBatchEndpointPanel
        v-else
        :state="endpointState"
        :format-options="endpointFormatOptions"
        :selected-formats="selectedFormats"
        :disabled="executing"
        :loading="endpointLoading"
        @update:state="endpointState = $event"
        @update:selected-formats="selectedFormats = $event"
      />

      <div
        v-if="previewTitle"
        class="rounded-lg border bg-muted/15 px-3 py-2.5 text-xs"
      >
        <div class="font-medium text-foreground">
          {{ previewTitle }}
        </div>
        <div class="mt-1.5 space-y-1 text-muted-foreground">
          <p
            v-for="line in previewDetails"
            :key="line"
          >
            {{ line }}
          </p>
        </div>
      </div>

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
            :class="executeButtonVariant === 'destructive' ? 'bg-destructive' : 'bg-primary'"
            :style="{ width: `${progressPercent}%` }"
          />
        </div>
      </div>

      <div
        v-else-if="lastResultTitle"
        class="rounded-lg border bg-background px-3 py-2.5 text-xs text-muted-foreground"
      >
        <div class="font-medium text-foreground">
          {{ lastResultTitle }}
        </div>
        <div
          v-if="lastResultDetails.length"
          class="mt-1.5 space-y-1"
        >
          <p
            v-for="line in lastResultDetails"
            :key="line"
          >
            {{ line }}
          </p>
        </div>
      </div>
    </div>

    <template #footer>
      <div class="flex w-full flex-wrap items-center justify-between gap-3">
        <div class="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
          <span>已选 <span class="font-semibold tabular-nums text-foreground">{{ selectedCount }}</span></span>
          <span class="text-border">·</span>
          <span>当前范围 <span class="tabular-nums">{{ providers.length }}</span></span>
          <span class="text-emerald-600 dark:text-emerald-400">活跃 <span class="tabular-nums">{{ activeProviderCount }}</span></span>
          <span>停用 <span class="tabular-nums">{{ inactiveProviderCount }}</span></span>
        </div>
        <div class="flex items-center gap-2">
          <Button
            variant="outline"
            :disabled="executing"
            @click="requestClose"
          >
            关闭
          </Button>
          <Button
            :variant="executeButtonVariant"
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
import {
  Power,
  PowerOff,
  Settings2,
  SquarePen,
  Trash2,
  Users,
} from 'lucide-vue-next'
import { Button, Dialog } from '@/components/ui'
import MultiSelect from '@/components/common/MultiSelect.vue'
import ProviderBatchBasicInfoPanel from '@/features/providers/components/batch-edit/ProviderBatchBasicInfoPanel.vue'
import ProviderBatchEndpointPanel from '@/features/providers/components/batch-edit/ProviderBatchEndpointPanel.vue'
import {
  deleteProvider,
  getProviderDeleteTask,
  getProviderEndpoints,
  updateEndpoint,
  updateProvider,
  type BodyRule,
  type HeaderRule,
  type ProviderEndpoint,
  type ProviderWithEndpointsSummary,
} from '@/api/endpoints'
import { useConfirm } from '@/composables/useConfirm'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'
import {
  buildProviderBasicPatch,
  buildUpstreamStreamConfig,
  collectFormatUnion,
  computeEndpointTargets,
  createDefaultProviderBasicState,
  getProviderBasicFieldLabels,
  hasProviderBasicPatch,
  mergeRules,
  toProviderOptions,
  type BatchProviderEndpoints,
  type BatchSkippedProvider,
  type EndpointBatchTarget,
  type EndpointBatchState,
  type ProviderBasicBatchState,
  validateProviderBasicState,
} from '@/features/providers/utils/batchEdit'

type BatchMode = 'status' | 'basic' | 'endpoint'
type ProviderStatusAction = 'enable' | 'disable' | 'delete'
type EndpointPatch = Parameters<typeof updateEndpoint>[1]
type ProviderPatch = Parameters<typeof updateProvider>[1]

interface BatchModeOption {
  value: BatchMode
  label: string
  icon: Component
}

interface ProviderStatusActionOption {
  value: ProviderStatusAction
  label: string
  hint: string
  icon: Component
  destructive?: boolean
}

interface ParsedEndpointRules {
  header_rules: HeaderRule[]
  body_rules: BodyRule[]
}

type ParseResult<T> =
  | { ok: true; value: T }
  | { ok: false; error: string }

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

const modeOptions: BatchModeOption[] = [
  { value: 'status', label: '状态操作', icon: Power },
  { value: 'basic', label: '基础信息', icon: SquarePen },
  { value: 'endpoint', label: '端点配置', icon: Settings2 },
]

const statusActionOptions: ProviderStatusActionOption[] = [
  { value: 'enable', label: '启用', hint: '恢复所选提供商参与调度。', icon: Power },
  { value: 'disable', label: '停用', hint: '停止所选提供商参与调度，保留配置。', icon: PowerOff },
  { value: 'delete', label: '删除', hint: '永久删除所选提供商及其端点、账号和配置。', icon: Trash2, destructive: true },
]

const MODE_ACCENT: Record<BatchMode, string> = {
  status: 'bg-background text-primary shadow-sm ring-1 ring-primary/25',
  basic: 'bg-background text-primary shadow-sm ring-1 ring-primary/25',
  endpoint: 'bg-background text-primary shadow-sm ring-1 ring-primary/25',
}

const STATUS_ACCENT: Record<ProviderStatusAction, string> = {
  enable: 'border-emerald-500/30 bg-emerald-500/5 text-emerald-700 dark:text-emerald-300',
  disable: 'border-primary/30 bg-primary/5 text-primary',
  delete: 'border-destructive/40 bg-destructive/5 text-destructive',
}

const selectedProviderIds = ref<string[]>([])
const selectedMode = ref<BatchMode>('status')
const statusAction = ref<ProviderStatusAction>('disable')
const basicState = ref<ProviderBasicBatchState>(createDefaultProviderBasicState())
const endpointState = ref(createDefaultEndpointState())
const selectedFormats = ref<string[]>([])

const endpointLoading = ref(false)
const endpointCache = ref(new Map<string, ProviderEndpoint[]>())

const executing = ref(false)
const progressDone = ref(0)
const progressTotal = ref(0)
const progressLabel = ref('')
const lastResultTitle = ref('')
const lastResultDetails = ref<string[]>([])

const dialogDescription = computed(() => '在当前列表范围内选择多个提供商，并批量处理状态、基础信息或端点配置')
const providerOptions = computed(() => toProviderOptions(props.providers))
const providerById = computed(() => new Map(props.providers.map(provider => [provider.id, provider])))
const selectedProviders = computed(() =>
  selectedProviderIds.value
    .map(id => providerById.value.get(id))
    .filter((provider): provider is ProviderWithEndpointsSummary => Boolean(provider)),
)
const selectedCount = computed(() => selectedProviders.value.length)
const activeProviderCount = computed(() => props.providers.filter(provider => provider.is_active).length)
const inactiveProviderCount = computed(() => props.providers.length - activeProviderCount.value)

const selectedStatusAction = computed(() => statusActionOptions.find(action => action.value === statusAction.value))
const selectedStatusLabel = computed(() => selectedStatusAction.value?.label || '操作')
const endpointDataReady = computed(() =>
  selectedProviderIds.value.length > 0
  && selectedProviderIds.value.every(id => endpointCache.value.has(id)),
)
const endpointProviderRows = computed<BatchProviderEndpoints[]>(() =>
  selectedProviders.value.map(provider => ({
    id: provider.id,
    name: provider.name,
    endpoints: endpointCache.value.get(provider.id) || [],
  })),
)
const endpointFormatOptions = computed(() =>
  collectFormatUnion(endpointDataReady.value ? endpointProviderRows.value : [])
    .map(format => ({ value: format, label: format })),
)
const endpointTargetResult = computed(() => {
  if (!endpointDataReady.value || selectedFormats.value.length === 0) {
    return { targets: [], matchedProviderCount: 0, skippedProviders: [] }
  }
  return computeEndpointTargets(endpointProviderRows.value, selectedFormats.value)
})

const endpointRulesParseResult = computed(() =>
  endpointState.value.rules.include
    ? parseEndpointRulesJson(endpointState.value.rules.json)
    : ({ ok: true, value: { header_rules: [], body_rules: [] } } as ParseResult<ParsedEndpointRules>),
)

const basicValidationMessage = computed(() => validateProviderBasicState(basicState.value))
const hasBasicPatch = computed(() => hasProviderBasicPatch(basicState.value))
const basicDraftDirty = computed(() => hasBasicPatch.value)
const hasEndpointPatch = computed(() =>
  endpointState.value.proxy.include
  || endpointState.value.formatConversion.include
  || endpointState.value.upstreamPolicy.include
  || endpointState.value.active.include
  || endpointState.value.rules.include,
)
const endpointDraftDirty = computed(() =>
  selectedFormats.value.length > 0
  || hasEndpointPatch.value
  || endpointState.value.rules.json.trim().length > 0,
)
const hasDraftChanges = computed(() =>
  selectedCount.value > 0
  || statusAction.value !== 'disable'
  || basicDraftDirty.value
  || endpointDraftDirty.value,
)
const canExecute = computed(() => {
  if (executing.value || selectedCount.value === 0) return false
  if (selectedMode.value === 'status') return true
  if (selectedMode.value === 'basic') {
    return hasBasicPatch.value && !basicValidationMessage.value
  }
  return endpointDataReady.value
    && !endpointLoading.value
    && selectedFormats.value.length > 0
    && hasEndpointPatch.value
    && endpointRulesParseResult.value.ok
    && endpointTargetResult.value.targets.length > 0
})
const executeButtonVariant = computed(() => (
  selectedMode.value === 'status' && statusAction.value === 'delete' ? 'destructive' : 'default'
))
const executeButtonLabel = computed(() => {
  if (executing.value) return '执行中...'
  if (selectedMode.value === 'status') {
    return selectedCount.value > 0 ? `${selectedStatusLabel.value} ${selectedCount.value} 项` : `执行${selectedStatusLabel.value}`
  }
  if (selectedMode.value === 'basic') {
    return selectedCount.value > 0 ? `更新基础信息 ${selectedCount.value} 个提供商` : '更新基础信息'
  }
  return selectedCount.value > 0 ? `更新端点配置 ${selectedCount.value} 个提供商` : '更新端点配置'
})
const progressPercent = computed(() => {
  if (progressTotal.value <= 0) return 0
  return Math.min(100, Math.round((progressDone.value / progressTotal.value) * 100))
})

const previewTitle = computed(() => {
  if (selectedCount.value === 0) return '先选择需要处理的提供商'
  if (selectedMode.value === 'status') {
    return `将对 ${selectedCount.value} 个提供商执行：${selectedStatusLabel.value}`
  }
  if (selectedMode.value === 'basic') {
    if (!hasBasicPatch.value) return '请选择至少一个要纳入批量的基础信息字段'
    if (basicValidationMessage.value) return basicValidationMessage.value
    return `将更新 ${selectedCount.value} 个提供商的基础信息`
  }
  if (endpointLoading.value || !endpointDataReady.value) return '正在加载所选提供商的端点'
  if (selectedFormats.value.length === 0) return '请选择目标 API 格式'
  if (!hasEndpointPatch.value) return '请选择至少一个要纳入批量的端点字段'
  return `将更新 ${endpointTargetResult.value.targets.length} 个端点`
})
const previewDetails = computed(() => {
  if (selectedCount.value === 0) {
    return [`当前可选范围 ${props.providers.length} 个提供商。`]
  }
  if (selectedMode.value === 'status') {
    return [
      statusAction.value === 'delete'
        ? '删除会同时删除端点、账号和配置，执行前会再次确认。'
        : '仅更新提供商启用状态，不改动端点、账号或模型配置。',
    ]
  }
  if (selectedMode.value === 'basic') {
    return [
      `纳入字段：${getProviderBasicFieldLabels(basicState.value).join('、') || '未选择'}`,
      '未勾选的基础信息字段不会被修改。',
    ]
  }
  if (endpointLoading.value || !endpointDataReady.value) return ['端点数据加载完成后会显示命中数量。']
  const result = endpointTargetResult.value
  return [
    `命中 ${result.matchedProviderCount} 个提供商，跳过 ${result.skippedProviders.length} 个无匹配端点的提供商。`,
    `命中提供商：${formatEndpointTargetProviders(result.targets)}`,
    `未命中提供商：${formatProviderNames(result.skippedProviders.map(provider => provider.name))}`,
    `目标格式：${selectedFormats.value.length > 0 ? selectedFormats.value.join(', ') : '未选择'}`,
    `纳入字段：${getEndpointFieldLabels().join('、') || '未选择'}`,
  ]
})

function createDefaultEndpointState(): EndpointBatchState {
  return {
    proxy: { include: false, nodeId: '' },
    formatConversion: { include: false, enabled: true },
    upstreamPolicy: { include: false, value: 'auto' },
    active: { include: false, value: true },
    rules: { include: false, mode: 'append', json: '', error: null },
  }
}

function getModeClass(mode: BatchMode): string {
  if (selectedMode.value !== mode) {
    return 'text-muted-foreground hover:bg-background/60 hover:text-foreground'
  }
  return MODE_ACCENT[mode]
}

function getStatusActionClass(action: ProviderStatusActionOption): string {
  if (statusAction.value !== action.value) {
    return 'bg-background text-foreground hover:bg-muted/40'
  }
  return STATUS_ACCENT[action.value]
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function parseEndpointRulesJson(text: string): ParseResult<ParsedEndpointRules> {
  const trimmed = text.trim()
  if (!trimmed) {
    return { ok: true, value: { header_rules: [], body_rules: [] } }
  }

  let parsed: unknown
  try {
    parsed = JSON.parse(trimmed)
  } catch {
    return { ok: false, error: '请求规则 JSON 格式不正确' }
  }

  if (!isPlainObject(parsed)) {
    return { ok: false, error: '请求规则必须是对象，包含 header_rules / body_rules' }
  }

  const headerRules = parsed.header_rules ?? []
  const bodyRules = parsed.body_rules ?? []
  if (!Array.isArray(headerRules)) {
    return { ok: false, error: 'header_rules 必须是数组' }
  }
  if (!Array.isArray(bodyRules)) {
    return { ok: false, error: 'body_rules 必须是数组' }
  }

  return {
    ok: true,
    value: {
      header_rules: headerRules as HeaderRule[],
      body_rules: bodyRules as BodyRule[],
    },
  }
}

function getEndpointFieldLabels(): string[] {
  const labels: string[] = []
  if (endpointState.value.proxy.include) labels.push('代理节点')
  if (endpointState.value.formatConversion.include) labels.push('格式转换')
  if (endpointState.value.upstreamPolicy.include) labels.push('上游流式策略')
  if (endpointState.value.active.include) labels.push('启用状态')
  if (endpointState.value.rules.include) labels.push('请求规则')
  return labels
}

function formatProviderNames(names: string[]): string {
  const uniqueNames = [...new Set(names.filter(Boolean))]
  return uniqueNames.length > 0 ? uniqueNames.join('、') : '无'
}

function formatEndpointTargetProviders(targets: EndpointBatchTarget[]): string {
  const counts = new Map<string, number>()
  for (const target of targets) {
    counts.set(target.providerName, (counts.get(target.providerName) || 0) + 1)
  }
  const parts = [...counts.entries()].map(([name, count]) => `${name}（${count} 个端点）`)
  return parts.length > 0 ? parts.join('、') : '无'
}

function buildEndpointResultDetails(
  successTargets: EndpointBatchTarget[],
  failedTargets: EndpointBatchTarget[],
  skippedProviders: BatchSkippedProvider[],
): string[] {
  const details = [
    `成功提供商：${formatEndpointTargetProviders(successTargets)}`,
  ]
  if (failedTargets.length > 0) {
    details.push(`失败提供商：${formatEndpointTargetProviders(failedTargets)}`)
  }
  if (skippedProviders.length > 0) {
    details.push(`未命中提供商：${formatProviderNames(skippedProviders.map(provider => provider.name))}`)
  }
  return details
}

function buildBasicResultDetails(successProviders: string[], failedProviders: string[]): string[] {
  const details = [
    `纳入字段：${getProviderBasicFieldLabels(basicState.value).join('、') || '未选择'}`,
    `成功提供商：${formatProviderNames(successProviders)}`,
  ]
  if (failedProviders.length > 0) {
    details.push(`失败提供商：${formatProviderNames(failedProviders)}`)
  }
  return details
}

function setEndpointRulesError(message: string | null): void {
  if (endpointState.value.rules.error === message) return
  endpointState.value.rules.error = message
}

function handleDialogUpdate(open: boolean): void {
  if (open) {
    emit('update:modelValue', true)
    return
  }
  void requestClose()
}

async function requestClose(): Promise<void> {
  if (executing.value) return
  if (hasDraftChanges.value) {
    const confirmed = await confirm({
      title: '放弃批量编辑',
      message: '当前批量处理面板中还有未应用的选择或配置，关闭后会丢失这些草稿。是否关闭？',
      confirmText: '关闭',
      variant: 'warning',
    })
    if (!confirmed) return
  }
  emit('update:modelValue', false)
}

function resetDraft(): void {
  selectedProviderIds.value = []
  selectedMode.value = 'status'
  statusAction.value = 'disable'
  basicState.value = createDefaultProviderBasicState()
  endpointState.value = createDefaultEndpointState()
  selectedFormats.value = []
  endpointLoading.value = false
  endpointCache.value = new Map()
  progressDone.value = 0
  progressTotal.value = 0
  progressLabel.value = ''
  lastResultTitle.value = ''
  lastResultDetails.value = []
}

function resetAppliedDraft(): void {
  selectedProviderIds.value = []
  basicState.value = createDefaultProviderBasicState()
  endpointState.value = createDefaultEndpointState()
  selectedFormats.value = []
  statusAction.value = 'disable'
}

async function loadEndpointDataForSelection(): Promise<void> {
  if (selectedMode.value !== 'endpoint' || selectedProviderIds.value.length === 0) return
  const missingIds = selectedProviderIds.value.filter(id => !endpointCache.value.has(id))
  if (missingIds.length === 0) return

  endpointLoading.value = true
  try {
    const loaded = await Promise.all(missingIds.map(async (providerId) => {
      const endpoints = await getProviderEndpoints(providerId)
      return { providerId, endpoints }
    }))
    const next = new Map(endpointCache.value)
    for (const item of loaded) {
      next.set(item.providerId, item.endpoints)
    }
    endpointCache.value = next
  } catch (err) {
    showError(parseApiError(err, '加载端点配置失败'), '错误')
  } finally {
    endpointLoading.value = false
  }
}

function buildEndpointPatch(endpoint: ProviderEndpoint, rules: ParsedEndpointRules): EndpointPatch {
  const patch: EndpointPatch = {}
  if (endpointState.value.proxy.include) {
    patch.proxy = endpointState.value.proxy.nodeId
      ? { node_id: endpointState.value.proxy.nodeId, enabled: true }
      : null
  }
  if (endpointState.value.formatConversion.include) {
    patch.format_acceptance_config = endpointState.value.formatConversion.enabled
      ? { enabled: true }
      : null
  }
  if (endpointState.value.upstreamPolicy.include) {
    patch.config = buildUpstreamStreamConfig(endpoint.config, endpointState.value.upstreamPolicy.value)
  }
  if (endpointState.value.active.include) {
    patch.is_active = endpointState.value.active.value
  }
  if (endpointState.value.rules.include) {
    patch.header_rules = mergeRules(endpoint.header_rules, rules.header_rules, endpointState.value.rules.mode)
    patch.body_rules = mergeRules(endpoint.body_rules, rules.body_rules, endpointState.value.rules.mode)
  }
  return patch
}

function getSelectedTargets(): ProviderWithEndpointsSummary[] {
  return selectedProviders.value
}

async function confirmAndExecute(): Promise<void> {
  if (!canExecute.value) return

  const confirmed = await confirm({
    title: getConfirmTitle(),
    message: getConfirmMessage(),
    confirmText: selectedMode.value === 'status' && statusAction.value === 'delete' ? '确认删除' : '确认执行',
    ...(executeButtonVariant.value === 'destructive' ? { variant: 'destructive' as const } : {}),
  })
  if (!confirmed) return

  await executeBatchAction()
}

function getConfirmTitle(): string {
  if (selectedMode.value === 'status') return `批量${selectedStatusLabel.value}提供商`
  if (selectedMode.value === 'basic') return '批量更新基础信息'
  return '批量更新端点配置'
}

function getConfirmMessage(): string {
  if (selectedMode.value === 'status') {
    return statusAction.value === 'delete'
      ? `将删除 ${selectedCount.value} 个提供商，并同时删除其所有端点、账号和配置。此操作不可恢复，是否继续？`
      : `将对 ${selectedCount.value} 个提供商执行：${selectedStatusLabel.value}，是否继续？`
  }
  if (selectedMode.value === 'basic') {
    return `将更新 ${selectedCount.value} 个提供商的基础信息：${getProviderBasicFieldLabels(basicState.value).join('、')}。是否继续？`
  }
  const result = endpointTargetResult.value
  return `将更新 ${result.targets.length} 个端点，覆盖 ${result.matchedProviderCount} 个提供商；${result.skippedProviders.length} 个提供商无匹配格式会跳过。是否继续？`
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
  if (selectedMode.value === 'status') {
    await executeStatusBatch()
    return
  }
  if (selectedMode.value === 'basic') {
    await executeBasicBatch()
    return
  }
  await executeEndpointBatch()
}

async function executeStatusBatch(): Promise<void> {
  const targets = getSelectedTargets()
  if (targets.length === 0) {
    warning('请先选择提供商')
    return
  }

  executing.value = true
  progressDone.value = 0
  progressTotal.value = targets.length
  lastResultTitle.value = ''
  lastResultDetails.value = []
  let successCount = 0
  let failedCount = 0

  try {
    for (const provider of targets) {
      progressLabel.value = `正在${selectedStatusLabel.value}：${provider.name}`
      try {
        if (statusAction.value === 'delete') {
          const result = await deleteProvider(provider.id)
          await pollProviderDeleteTask(provider.id, result.task_id)
        } else {
          await updateProvider(provider.id, { is_active: statusAction.value === 'enable' })
        }
        successCount += 1
      } catch (err) {
        failedCount += 1
        // eslint-disable-next-line no-console
        console.error(`[ProviderBatchActionDialog] ${statusAction.value} failed (${provider.id}):`, err)
      } finally {
        progressDone.value += 1
      }
    }

    finishBatchResult('执行完成', successCount, failedCount)
  } catch (err) {
    showError(parseApiError(err, '批量处理提供商失败'), '错误')
  } finally {
    finishExecuting()
  }
}

async function executeBasicBatch(): Promise<void> {
  const validationMessage = basicValidationMessage.value
  if (validationMessage) {
    warning(validationMessage)
    return
  }

  const targets = getSelectedTargets()
  if (targets.length === 0) {
    warning('请先选择提供商')
    return
  }

  const patch: ProviderPatch = buildProviderBasicPatch(basicState.value)
  if (Object.keys(patch).length === 0) {
    warning('请选择至少一个要纳入批量的基础信息字段')
    return
  }

  executing.value = true
  progressDone.value = 0
  progressTotal.value = targets.length
  lastResultTitle.value = ''
  lastResultDetails.value = []
  let successCount = 0
  let failedCount = 0
  const successProviders: string[] = []
  const failedProviders: string[] = []

  try {
    for (const provider of targets) {
      progressLabel.value = `正在更新基础信息：${provider.name}`
      try {
        await updateProvider(provider.id, patch)
        successCount += 1
        successProviders.push(provider.name)
      } catch (err) {
        failedCount += 1
        failedProviders.push(provider.name)
        // eslint-disable-next-line no-console
        console.error(`[ProviderBatchActionDialog] basic info update failed (${provider.id}):`, err)
      } finally {
        progressDone.value += 1
      }
    }

    finishBatchResult(
      '基础信息更新完成',
      successCount,
      failedCount,
      buildBasicResultDetails(successProviders, failedProviders),
    )
  } catch (err) {
    showError(parseApiError(err, '批量更新基础信息失败'), '错误')
  } finally {
    finishExecuting()
  }
}

async function executeEndpointBatch(): Promise<void> {
  const parsedRules = endpointRulesParseResult.value
  if (!parsedRules.ok) {
    setEndpointRulesError(parsedRules.error)
    return
  }

  const endpointById = new Map<string, ProviderEndpoint>()
  for (const provider of endpointProviderRows.value) {
    for (const endpoint of provider.endpoints) {
      endpointById.set(endpoint.id, endpoint as ProviderEndpoint)
    }
  }
  const targetResult = endpointTargetResult.value
  const targets = targetResult.targets
  if (targets.length === 0) {
    warning('没有命中可更新的端点')
    return
  }

  executing.value = true
  progressDone.value = 0
  progressTotal.value = targets.length
  lastResultTitle.value = ''
  lastResultDetails.value = []
  let successCount = 0
  let failedCount = 0
  const successTargets: EndpointBatchTarget[] = []
  const failedTargets: EndpointBatchTarget[] = []

  try {
    for (const target of targets) {
      progressLabel.value = `正在更新端点：${target.providerName} / ${target.apiFormat}`
      const endpoint = endpointById.get(target.endpointId)
      if (!endpoint) {
        failedCount += 1
        failedTargets.push(target)
        progressDone.value += 1
        continue
      }
      try {
        await updateEndpoint(endpoint.id, buildEndpointPatch(endpoint, parsedRules.value))
        successCount += 1
        successTargets.push(target)
      } catch (err) {
        failedCount += 1
        failedTargets.push(target)
        // eslint-disable-next-line no-console
        console.error(`[ProviderBatchActionDialog] endpoint update failed (${endpoint.id}):`, err)
      } finally {
        progressDone.value += 1
      }
    }

    endpointCache.value = new Map()
    finishBatchResult(
      '端点更新完成',
      successCount,
      failedCount,
      buildEndpointResultDetails(successTargets, failedTargets, targetResult.skippedProviders),
    )
  } catch (err) {
    showError(parseApiError(err, '批量更新端点配置失败'), '错误')
  } finally {
    finishExecuting()
  }
}

function finishBatchResult(prefix: string, successCount: number, failedCount: number, details: string[] = []): void {
  lastResultTitle.value = `${prefix}：成功 ${successCount}，失败 ${failedCount}`
  lastResultDetails.value = details
  if (failedCount > 0) warning(lastResultTitle.value)
  else success(lastResultTitle.value)
  if (successCount > 0) {
    resetAppliedDraft()
    emit('changed')
  }
}

function finishExecuting(): void {
  executing.value = false
  progressDone.value = 0
  progressTotal.value = 0
  progressLabel.value = ''
}

watch(
  () => props.modelValue,
  (open) => {
    if (open) resetDraft()
  },
)

watch(
  () => props.providers.map(provider => provider.id).join('|'),
  () => {
    const availableIds = new Set(props.providers.map(provider => provider.id))
    selectedProviderIds.value = selectedProviderIds.value.filter(id => availableIds.has(id))
  },
)

watch(
  () => [selectedMode.value, selectedProviderIds.value.join('|')],
  () => {
    if (selectedMode.value === 'endpoint') {
      void loadEndpointDataForSelection()
    }
  },
)

watch(
  endpointFormatOptions,
  (options) => {
    const validFormats = new Set(options.map(option => option.value))
    selectedFormats.value = selectedFormats.value.filter(format => validFormats.has(format))
  },
)

watch(
  [
    () => endpointState.value.rules.include,
    () => endpointState.value.rules.json,
  ],
  () => {
    const result = endpointRulesParseResult.value
    setEndpointRulesError(result.ok ? null : result.error)
  },
)
</script>
