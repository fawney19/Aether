<template>
  <Card class="overflow-hidden">
    <!-- 标题头部 -->
    <div class="p-4 border-b border-border/60">
      <div class="flex items-center justify-between">
        <h3 class="text-sm font-semibold flex items-center gap-2">
          模型列表
        </h3>
        <Button
          variant="outline"
          size="sm"
          class="h-8"
          @click="openBatchAssignDialog"
        >
          <Layers class="w-3.5 h-3.5 mr-1.5" />
          关联模型
        </Button>
      </div>
    </div>

    <!-- 加载状态 -->
    <div
      v-if="isLoading"
      class="flex items-center justify-center py-12"
    >
      <div class="animate-spin rounded-full h-8 w-8 border-b-2 border-primary" />
    </div>

    <!-- 模型列表 -->
    <div
      v-else-if="models.length > 0"
      class="overflow-hidden"
    >
      <table
        ref="modelsListRef"
        class="w-full text-sm table-fixed"
      >
        <colgroup>
          <col class="w-[45%]">
          <col class="w-[30%]">
          <col class="w-[25%]">
        </colgroup>
        <tbody>
          <tr
            v-for="model in paginatedModels"
            :key="model.id"
            class="border-b border-border/40 last:border-b-0 hover:bg-muted/30 transition-colors"
          >
            <td class="align-top px-4 py-3">
              <div class="flex items-center gap-2.5">
                <!-- 状态指示灯 -->
                <div
                  class="w-2 h-2 rounded-full shrink-0"
                  :class="getStatusIndicatorClass(model)"
                  :title="getStatusTitle(model)"
                />
                <!-- 模型信息 -->
                <div class="text-left flex-1 min-w-0">
                  <div class="flex items-center gap-1.5">
                    <span class="font-semibold text-sm">
                      {{ model.global_model_display_name || model.provider_model_name }}
                    </span>
                  </div>
                  <div class="text-xs text-muted-foreground mt-1 flex items-center gap-1">
                    <span class="font-mono truncate">{{ model.provider_model_name }}</span>
                    <button
                      class="p-0.5 hover:bg-muted rounded transition-colors shrink-0"
                      title="复制模型 ID"
                      @click.stop="copyModelId(model.provider_model_name)"
                    >
                      <Copy class="w-3 h-3" />
                    </button>
                  </div>
                </div>
              </div>
            </td>
            <td class="align-top px-4 py-3 text-xs whitespace-nowrap">
              <div
                class="grid gap-1"
                style="grid-template-columns: auto 1fr;"
              >
                <!-- 按 Token 计费 -->
                <template v-if="hasTokenPricing(model)">
                  <span class="text-muted-foreground text-right">入/出:</span>
                  <span class="font-mono font-semibold">
                    ${{ formatPrice(model.effective_input_price) }}/${{ formatPrice(model.effective_output_price) }}
                  </span>
                </template>
                <template v-if="getEffectiveCachePrice(model, 'creation') > 0 || getEffectiveCachePrice(model, 'read') > 0">
                  <span class="text-muted-foreground text-right">{{ get1hCachePrice(model) > 0 ? '5min 缓存:' : '缓存:' }}</span>
                  <span class="font-mono font-semibold">
                    ${{ formatPrice(getEffectiveCachePrice(model, 'creation')) }}/${{ formatPrice(getEffectiveCachePrice(model, 'read')) }}
                  </span>
                </template>
                <!-- 1h 缓存价格 -->
                <template v-if="get1hCachePrice(model) > 0">
                  <span class="text-muted-foreground text-right">1h 缓存创建:</span>
                  <span class="font-mono font-semibold">
                    ${{ formatPrice(get1hCachePrice(model)) }}
                  </span>
                </template>
                <!-- 按次计费 -->
                <template v-if="hasRequestPricing(model)">
                  <span class="text-muted-foreground text-right">按次:</span>
                  <span class="font-mono font-semibold">
                    ${{ formatPrice(model.effective_price_per_request ?? model.price_per_request) }}/次
                  </span>
                </template>
                <!-- 视频费用计费 -->
                <template v-if="hasVideoPricing(model)">
                  <span class="text-muted-foreground text-right">视频:</span>
                  <span
                    class="font-mono font-semibold"
                    :title="getVideoPricingTooltip(model)"
                  >
                    {{ getVideoPricingDisplay(model) }}
                  </span>
                </template>
                <!-- 无计费配置 -->
                <template v-if="!hasTokenPricing(model) && !hasRequestPricing(model) && !hasVideoPricing(model)">
                  <span class="text-muted-foreground">—</span>
                </template>
              </div>
            </td>
            <td class="align-top px-4 py-3">
              <div class="flex justify-end gap-1">
                <!-- 测试按钮（模拟外部请求） -->
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-8 w-8"
                  title="测试模型"
                  :disabled="modelTest.testing.value && pendingTestModel?.id === model.id"
                  @click="testModelConnection(model)"
                >
                  <Loader2
                    v-if="modelTest.testing.value && pendingTestModel?.id === model.id"
                    class="w-3.5 h-3.5 animate-spin"
                  />
                  <Play
                    v-else
                    class="w-3.5 h-3.5"
                  />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-8 w-8"
                  title="编辑"
                  @click="editModel(model)"
                >
                  <Edit class="w-3.5 h-3.5" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-8 w-8"
                  :disabled="togglingModelId === model.id"
                  :title="model.is_active ? '点击停用' : '点击启用'"
                  @click="toggleModelActive(model)"
                >
                  <Power class="w-3.5 h-3.5" />
                </Button>
              </div>
            </td>
          </tr>
        </tbody>
      </table>
      <!-- 分页控制 -->
      <div
        v-if="shouldPaginateModels"
        class="px-4 py-2 border-t border-border/40 flex items-center justify-between text-xs text-muted-foreground"
      >
        <span>共 {{ sortedModels.length }} 个模型</span>
        <div class="flex items-center gap-1.5">
          <Button
            variant="ghost"
            size="sm"
            class="h-6 px-2 text-xs"
            :disabled="currentModelPage <= 1"
            @click="currentModelPage--"
          >
            ‹
          </Button>
          <span class="tabular-nums">{{ currentModelPage }} / {{ totalModelPages }}</span>
          <Button
            variant="ghost"
            size="sm"
            class="h-6 px-2 text-xs"
            :disabled="currentModelPage >= totalModelPages"
            @click="currentModelPage++"
          >
            ›
          </Button>
        </div>
      </div>
    </div>

    <!-- 空状态 -->
    <div
      v-else
      class="p-8 text-center text-muted-foreground"
    >
      <Box class="w-12 h-12 mx-auto mb-3 opacity-50" />
      <p class="text-sm">
        暂无模型
      </p>
      <p class="text-xs mt-1">
        请前往"模型目录"页面添加模型
      </p>
    </div>
  </Card>

  <ModelTestDialog
    :open="modelTest.dialogOpen.value"
    :result="modelTest.testResult.value"
    :mode="modelTest.testMode.value"
    :provider-type="provider.provider_type"
    :selecting-model-name="pendingTestModel ? (pendingTestModel.global_model_display_name || pendingTestModel.provider_model_name) : null"
    :requested-model-name="pendingRequestedModelName"
    :endpoints="activeEndpoints"
    :selected-endpoint="selectedTestEndpoint"
    :testing="modelTest.testing.value"
    :trace="modelTest.testTrace.value"
    :request-id="modelTest.requestId.value"
    :request-headers-draft="testRequestHeadersDraft"
    :request-headers-reset-value="testRequestHeadersResetValue"
    :request-headers-error="testRequestHeadersError"
    :request-body-draft="testRequestBodyDraft"
    :request-body-reset-value="testRequestBodyResetValue"
    :request-body-error="testRequestBodyError"
    :model-mapping-available="testModelMappingAvailable"
    :model-mapping-options="testModelMappingOptions"
    :selected-model-mapping="selectedTestMappedModelName"
    :key-options="testKeyOptions"
    :selected-key-ids="selectedTestKeyIds"
    :key-options-loading="loadingModelTestKeys"
    :start-disabled="!selectedTestEndpoint || !!testRequestHeadersError || !!testRequestBodyError"
    @close="handleTestDialogClose"
    @back="handleTestDialogBack"
    @start="handleStartPendingTest"
    @select-endpoint="handleSelectTestEndpoint"
    @select-model-mapping="handleSelectModelMapping"
    @update:selected-key-ids="handleSelectTestKeyIds"
    @update:request-headers-draft="testRequestHeadersDraft = $event"
    @update:request-body-draft="testRequestBodyDraft = $event"
  />
</template>

<script setup lang="ts">
import { ref, computed, watch } from 'vue'
import { useSmartPagination } from '@/composables/useSmartPagination'
import { useModelTest } from '@/composables/useModelTest'
import { Box, Edit, Layers, Power, Copy, Loader2, Play } from 'lucide-vue-next'
import Card from '@/components/ui/card.vue'
import Button from '@/components/ui/button.vue'
import { useToast } from '@/composables/useToast'
import { useClipboard } from '@/composables/useClipboard'
import {
  getVideoBillingMode,
  getVideoDefaultPricePerSecond,
  getVideoPricePerSecondTable,
  getVideoTokenPriceEntries,
  hasVideoPricing as hasVideoPricingConfigured,
  sortResolutionEntries,
  type VideoTokenPriceEntry,
} from '@/utils/form'
import {
  type Model,
  type ProviderEndpoint,
} from '@/api/endpoints'
import { getProviderKeys, type EndpointAPIKey } from '@/api/endpoints/keys'
import { updateModel } from '@/api/endpoints/models'
import { parseApiError } from '@/utils/errorParser'
import { formatApiFormat } from '@/api/endpoints/types/api-format'
import type { ProviderWithEndpointsSummary } from '@/api/endpoints'
import ModelTestDialog from './ModelTestDialog.vue'
import {
  buildDefaultModelTestRequestHeaders,
  buildDefaultModelTestRequestBody,
  isModelTestableApiFormat,
  isModelTestableEndpoint,
  listModelTestMappedModelOptions,
  modelTestKeySupportsEndpoint,
  normalizeModelTestMappedModelSelection,
  parseModelTestRequestHeadersDraft,
  parseModelTestRequestBodyDraft,
  selectPreferredModelTestEndpoint,
  syncModelTestRequestBodyDraft,
} from './model-test-request'

const props = defineProps<{
  provider: ProviderWithEndpointsSummary
  models?: Model[]
  endpoints?: ProviderEndpoint[]
  providerKeys?: EndpointAPIKey[]
  loading?: boolean
}>()

const emit = defineEmits<{
  'editModel': [model: Model]
  'batchAssign': []
  'refresh': []
}>()

const { error: showError, success: showSuccess } = useToast()
const { copyToClipboard } = useClipboard()

// 模型测试 composable
const modelTest = useModelTest({ providerId: () => props.provider.id })

// 状态
const localLoading = ref(false)
const localModels = ref<Model[]>([])
const togglingModelId = ref<string | null>(null)
const pendingTestModel = ref<Model | null>(null)
const selectedTestEndpoint = ref<ProviderEndpoint | null>(null)
const testRequestHeadersDraft = ref('')
const testRequestHeadersResetValue = ref('')
const testRequestBodyDraft = ref('')
const testRequestBodyResetValue = ref('')
const selectedTestMappedModelName = ref<string | null>(null)
const selectedTestKeyIds = ref<string[]>([])
const modelTestProviderKeys = ref<EndpointAPIKey[]>([])
const modelTestKeysLoadedProviderId = ref<string | null>(null)
const loadingModelTestKeys = ref(false)
const isPoolManagedProvider = computed(() => Boolean(props.provider.pool_advanced))
const activeEndpoints = computed(() => (props.endpoints ?? [])
  .filter(endpoint => {
    if (typeof endpoint.active_keys === 'number') {
      return endpoint.is_active !== false
        && isModelTestableApiFormat(endpoint.api_format)
        && (endpoint.active_keys > 0
          || isModelTestableEndpoint(endpoint, props.providerKeys ?? [], props.provider.provider_type))
    }
    return isModelTestableEndpoint(endpoint, props.providerKeys ?? [], props.provider.provider_type)
  }))
const parsedTestRequestHeaders = computed(() => parseModelTestRequestHeadersDraft(testRequestHeadersDraft.value))
const testRequestHeadersError = computed(() => parsedTestRequestHeaders.value.error)
const parsedTestRequestBody = computed(() => parseModelTestRequestBodyDraft(testRequestBodyDraft.value))
const testRequestBodyError = computed(() => parsedTestRequestBody.value.error)
const pendingRequestedModelName = computed(() => getModelTestRequestedModelName(pendingTestModel.value))
const testModelMappingOptions = computed(() => {
  const requestedModelName = pendingRequestedModelName.value.trim()
  return listModelTestMappedModelOptions(pendingTestModel.value, selectedTestEndpoint.value)
    .filter(option => option.name !== requestedModelName)
})
const mappedTestModelName = computed(() => {
  const selected = selectedTestMappedModelName.value?.trim()
  if (!selected) return null
  return testModelMappingOptions.value.some(option => option.name === selected)
    ? selected
    : null
})
const testModelMappingAvailable = computed(() => testModelMappingOptions.value.length > 0)
const providerKeysForModelTest = computed(() => (
  modelTestKeysLoadedProviderId.value === props.provider.id
    ? modelTestProviderKeys.value
    : props.providerKeys ?? []
))
const testKeyOptions = computed(() => {
  const endpoint = selectedTestEndpoint.value
  if (!endpoint) return []

  const seen = new Set<string>()
  return [...providerKeysForModelTest.value]
    .filter((key) => {
      if (seen.has(key.id)) return false
      seen.add(key.id)
      return modelTestKeySupportsEndpoint(key, endpoint, props.provider.provider_type)
    })
    .sort((left, right) => {
      const priority = left.internal_priority - right.internal_priority
      if (priority !== 0) return priority
      return formatTestKeyOptionLabel(left).localeCompare(formatTestKeyOptionLabel(right))
    })
    .map(key => ({
      value: key.id,
      label: formatTestKeyOptionLabel(key),
    }))
})
const effectiveTestRequestModelName = computed(() => (
  mappedTestModelName.value || pendingRequestedModelName.value
))
const models = computed(() => props.models ?? localModels.value)
const isLoading = computed(() => Boolean(props.loading) || localLoading.value)
// 按名称排序的模型列表
const sortedModels = computed(() => {
  return [...models.value].sort((a, b) => {
    const nameA = (a.global_model_display_name || a.provider_model_name || '').toLowerCase()
    const nameB = (b.global_model_display_name || b.provider_model_name || '').toLowerCase()
    return nameA.localeCompare(nameB)
  })
})

// ===== 模型列表智能分页 =====
const modelsListRef = ref<HTMLElement | null>(null)
const {
  currentPage: currentModelPage,
  totalPages: totalModelPages,
  shouldPaginate: shouldPaginateModels,
  paginatedItems: paginatedModels,
} = useSmartPagination(sortedModels, modelsListRef)

// 复制模型 ID 到剪贴板
async function copyModelId(modelId: string) {
  await copyToClipboard(modelId)
}

// 刷新数据（通知父组件刷新）
function refresh() {
  emit('refresh')
}

// 格式化价格显示
function formatPrice(price: number | null | undefined): string {
  if (price === null || price === undefined) return '-'
  // 如果是整数或小数点后只有1-2位，直接显示
  if (price >= 0.01 || price === 0) {
    return price.toFixed(2)
  }
  // 对于非常小的数字，使用科学计数法
  if (price < 0.0001) {
    return price.toExponential(2)
  }
  // 其他情况保留4位小数
  return price.toFixed(4)
}

// 检查是否有按 Token 计费
function hasTokenPricing(model: Model): boolean {
  const inputPrice = model.effective_input_price
  const outputPrice = model.effective_output_price
  return (inputPrice != null && inputPrice > 0) || (outputPrice != null && outputPrice > 0)
}

// 获取有效的缓存价格（从 effective_tiered_pricing 或 tiered_pricing 中提取）
function getEffectiveCachePrice(model: Model, type: 'creation' | 'read'): number {
  const tiered = model.effective_tiered_pricing || model.tiered_pricing
  if (!tiered?.tiers?.length) return 0
  const firstTier = tiered.tiers[0]
  if (type === 'creation') {
    return firstTier.cache_creation_price_per_1m || 0
  }
  return firstTier.cache_read_price_per_1m || 0
}

// 获取 1h 缓存价格
function get1hCachePrice(model: Model): number {
  const tiered = model.effective_tiered_pricing || model.tiered_pricing
  if (!tiered?.tiers?.length) return 0
  const firstTier = tiered.tiers[0]
  const ttl1h = firstTier.cache_ttl_pricing?.find(t => t.ttl_minutes === 60)
  return ttl1h?.cache_creation_price_per_1m || 0
}

// 检查是否有按次计费
function hasRequestPricing(model: Model): boolean {
  const requestPrice = model.effective_price_per_request ?? model.price_per_request
  return requestPrice != null && requestPrice > 0
}

// 检查是否有生效的视频计费配置（按秒或按 token）
function hasVideoPricing(model: Model): boolean {
  return hasVideoPricingConfigured(model.effective_config)
    || hasVideoPricingConfigured(model.config)
}

/** 优先取 effective_config，回退到模型自身 config。 */
function videoPricePerSecondEntries(model: Model): [string, number][] {
  const effective = getVideoPricePerSecondTable(model.effective_config)
  const table = Object.keys(effective).length > 0
    ? effective
    : getVideoPricePerSecondTable(model.config)
  return sortResolutionEntries(Object.entries(table))
}

function videoDefaultPricePerSecond(model: Model): number | null {
  return getVideoDefaultPricePerSecond(model.effective_config)
    ?? getVideoDefaultPricePerSecond(model.config)
}

function videoTokenEntries(model: Model): VideoTokenPriceEntry[] {
  const effective = getVideoTokenPriceEntries(model.effective_config)
  return effective.length > 0 ? effective : getVideoTokenPriceEntries(model.config)
}

/** 一条 token 单价的紧凑写法：有输入价时写成 `in/out`，否则只写输出价。 */
function formatTokenPricePair(entry: VideoTokenPriceEntry, digits: number): string {
  const output = entry.outputPricePer1m
  const input = entry.inputPricePer1m
  if (input !== null && output !== null) {
    return `$${input.toFixed(digits)}/$${output.toFixed(digits)}`
  }
  const single = output ?? input
  return single === null ? '' : `$${single.toFixed(digits)}`
}

function videoBillsPerToken(model: Model): boolean {
  return getVideoBillingMode(model.effective_config) === 'per_token'
    || getVideoBillingMode(model.config) === 'per_token'
}

// 获取视频计费的显示文本
function getVideoPricingDisplay(model: Model): string {
  // 按 token 计费时视频价格来自 billing.video，而不是模型的 token 阶梯目录，
  // 所以这一列必须自己呈现，否则整行会退化成「-」。
  if (videoBillsPerToken(model)) {
    const entries = videoTokenEntries(model)
    if (entries.length === 0) return ''
    const first = `${entries[0].resolution} ${formatTokenPricePair(entries[0], 2)}/M`
    return entries.length > 1 ? `${first} [${entries.length}种]` : first
  }

  const entries = videoPricePerSecondEntries(model)
  const defaultPrice = videoDefaultPricePerSecond(model)
  if (entries.length === 0) {
    return defaultPrice === null ? '' : `默认 $${defaultPrice.toFixed(2)}/s`
  }
  // 获取最低分辨率和价格
  const [firstRes, firstPrice] = entries[0]
  const priceStr = `${firstRes} $${firstPrice.toFixed(2)}/s`
  const variants = entries.length + (defaultPrice === null ? 0 : 1)
  if (variants > 1) {
    return `${priceStr} [${variants}种]`
  }
  return priceStr
}

// 获取视频计费详情的 tooltip
function getVideoPricingTooltip(model: Model): string {
  if (videoBillsPerToken(model)) {
    return videoTokenEntries(model)
      .map(entry => `${entry.resolution}: ${formatTokenPricePair(entry, 4)}/M`)
      .join('\n')
  }

  const lines = videoPricePerSecondEntries(model)
    .map(([res, price]) => `${res}: $${price.toFixed(4)}/s`)
  const defaultPrice = videoDefaultPricePerSecond(model)
  if (defaultPrice !== null) {
    lines.push(`默认: $${defaultPrice.toFixed(4)}/s`)
  }
  return lines.join('\n')
}

// 获取状态指示灯样式
function getStatusIndicatorClass(model: Model): string {
  if (!model.is_active) {
    // 停用 - 灰色
    return 'bg-gray-400 dark:bg-gray-600'
  }
  if (model.is_available) {
    // 活跃且可用 - 绿色
    return 'bg-green-500 dark:bg-green-400'
  }
  // 活跃但不可用 - 红色
  return 'bg-red-500 dark:bg-red-400'
}

// 获取状态提示文本
function getStatusTitle(model: Model): string {
  if (!model.is_active) {
    return '停用'
  }
  if (model.is_available) {
    return '活跃且可用'
  }
  return '活跃但不可用'
}

// 编辑模型
function editModel(model: Model) {
  emit('editModel', model)
}

// 打开批量关联对话框
function openBatchAssignDialog() {
  emit('batchAssign')
}

// 切换模型启用状态
async function toggleModelActive(model: Model) {
  if (togglingModelId.value) return

  togglingModelId.value = model.id
  try {
    const newStatus = !model.is_active
    await updateModel(props.provider.id, model.id, { is_active: newStatus })
    model.is_active = newStatus
    showSuccess(newStatus ? '模型已启用' : '模型已停用')
  } catch (err: unknown) {
    showError(parseApiError(err, '操作失败'), '错误')
  } finally {
    togglingModelId.value = null
  }
}

function handleTestDialogClose() {
  modelTest.resetState()
  pendingTestModel.value = null
  selectedTestEndpoint.value = null
  selectedTestMappedModelName.value = null
  selectedTestKeyIds.value = []
  testRequestHeadersDraft.value = ''
  testRequestHeadersResetValue.value = ''
  testRequestBodyDraft.value = ''
  testRequestBodyResetValue.value = ''
}

function handleTestDialogBack() {
  if (modelTest.testing.value) return
  modelTest.testResult.value = null
  modelTest.stopPolling()
}

function handleSelectTestEndpoint(endpointId: string) {
  const endpoint = activeEndpoints.value.find(item => item.id === endpointId)
  if (!endpoint) return
  selectedTestEndpoint.value = endpoint
  syncSelectedTestModelMapping()
  resetTestRequestBodyForSelectedEndpoint()
  pruneSelectedTestKeyIds()
}

function handleSelectModelMapping(modelName: string) {
  selectedTestMappedModelName.value = normalizeModelTestMappedModelSelection(
    testModelMappingOptions.value,
    modelName,
  )
  syncTestRequestBodyModel()
}

function handleSelectTestKeyIds(ids: string[]) {
  selectedTestKeyIds.value = normalizeSelectedTestKeyIds(ids)
}

async function handleStartPendingTest() {
  if (modelTest.testing.value) return
  if (!pendingTestModel.value) return

  const endpoint = selectedTestEndpoint.value || activeEndpoints.value[0]
  if (!endpoint) {
    showError('请选择要测试的端点')
    return
  }

  const { value: requestHeaders, error: requestHeadersError } = parsedTestRequestHeaders.value
  if (!requestHeaders || requestHeadersError) {
    showError(`测试请求头无效: ${requestHeadersError || '无效 JSON'}`)
    return
  }

  const { value: requestBody, error } = parsedTestRequestBody.value
  if (!requestBody || error) {
    showError(`测试请求体无效: ${error || '无效 JSON'}`)
    return
  }

  selectedTestEndpoint.value = endpoint
  pruneSelectedTestKeyIds()
  const model = pendingTestModel.value
  const modelName = model.global_model_name || model.provider_model_name
  const endpointPrefix = `[${formatApiFormat(endpoint.api_format)}] `
  await modelTest.startTest({
    mode: isPoolManagedProvider.value ? 'pool' : 'global',
    modelName,
    displayLabel: `${endpointPrefix}${modelName}`,
    apiFormat: endpoint.api_format,
    endpointId: endpoint.id,
    endpointBaseUrl: endpoint.base_url,
    apiKeyIds: selectedTestKeyIds.value,
    applyModelMapping: Boolean(mappedTestModelName.value),
    mappedModelName: mappedTestModelName.value ?? undefined,
    requestHeaders,
    requestBody,
    onError: () => {
      if (activeEndpoints.value.length > 1) {
        return true
      }
    },
  })
}

async function testModelConnection(model: Model) {
  if (modelTest.testing.value) return

  if (activeEndpoints.value.length === 0) {
    showError('暂无可用于测试的活跃端点')
    return
  }

  pendingTestModel.value = model
  selectedTestEndpoint.value = selectPreferredModelTestEndpoint(model, activeEndpoints.value)
  const requestedModelName = getModelTestRequestedModelName(model)
  selectedTestMappedModelName.value = null
  selectedTestKeyIds.value = []
  testRequestHeadersResetValue.value = buildDefaultModelTestRequestHeaders()
  testRequestHeadersDraft.value = testRequestHeadersResetValue.value
  testRequestBodyResetValue.value = buildDefaultModelTestRequestBody(
    requestedModelName,
    selectedTestEndpoint.value?.api_format,
    model,
  )
  testRequestBodyDraft.value = testRequestBodyResetValue.value
  modelTest.testResult.value = null
  modelTest.dialogOpen.value = true
  void ensureModelTestKeysLoaded()
}

function normalizeSelectedTestKeyIds(ids: string[]): string[] {
  const allowed = new Set(testKeyOptions.value.map(option => option.value))
  const selected = ids
    .map(id => id.trim())
    .filter(id => id && allowed.has(id))
  return [...new Set(selected)]
}

function pruneSelectedTestKeyIds() {
  if (selectedTestKeyIds.value.length === 0) return
  selectedTestKeyIds.value = normalizeSelectedTestKeyIds(selectedTestKeyIds.value)
}

async function ensureModelTestKeysLoaded() {
  if (modelTestKeysLoadedProviderId.value === props.provider.id || loadingModelTestKeys.value) {
    return
  }

  loadingModelTestKeys.value = true
  try {
    modelTestProviderKeys.value = await getProviderKeys(props.provider.id)
    modelTestKeysLoadedProviderId.value = props.provider.id
    pruneSelectedTestKeyIds()
  } catch (err: unknown) {
    showError(parseApiError(err, '加载测试 Key 失败'), '错误')
  } finally {
    loadingModelTestKeys.value = false
  }
}

function formatTestKeyOptionLabel(key: EndpointAPIKey): string {
  const name = key.name?.trim()
  const masked = key.api_key_masked?.trim()
  const authType = key.auth_type?.trim()
  const primary = name || masked || key.id
  const suffix = [
    masked && masked !== primary ? masked : '',
    authType || '',
  ].filter(Boolean)
  return suffix.length > 0 ? `${primary} · ${suffix.join(' · ')}` : primary
}

function getModelTestRequestedModelName(model: Model | null): string {
  return model?.global_model_name || model?.provider_model_name || ''
}

function syncSelectedTestModelMapping(preferredName?: string | null) {
  const options = testModelMappingOptions.value
  if (options.length === 0) {
    selectedTestMappedModelName.value = null
    return
  }
  const preferred = preferredName ?? selectedTestMappedModelName.value
  selectedTestMappedModelName.value = normalizeModelTestMappedModelSelection(options, preferred)
}

function syncTestRequestBodyModel() {
  const modelName = effectiveTestRequestModelName.value
  if (!modelName) return

  const resetDraft = testRequestBodyResetValue.value
    || buildDefaultModelTestRequestBody(
      modelName,
      selectedTestEndpoint.value?.api_format,
      pendingTestModel.value,
    )
  const next = syncModelTestRequestBodyDraft(
    testRequestBodyDraft.value,
    testRequestBodyResetValue.value,
    resetDraft,
    modelName,
  )
  testRequestBodyResetValue.value = next.resetValue
  testRequestBodyDraft.value = next.draft
}

function resetTestRequestBodyForSelectedEndpoint() {
  const modelName = effectiveTestRequestModelName.value
  if (!modelName) return

  const nextResetValue = buildDefaultModelTestRequestBody(
    modelName,
    selectedTestEndpoint.value?.api_format,
    pendingTestModel.value,
  )
  const next = syncModelTestRequestBodyDraft(
    testRequestBodyDraft.value,
    testRequestBodyResetValue.value,
    nextResetValue,
    modelName,
  )
  testRequestBodyResetValue.value = next.resetValue
  testRequestBodyDraft.value = next.draft
}

watch(
  [effectiveTestRequestModelName, () => selectedTestEndpoint.value?.api_format],
  () => syncTestRequestBodyModel(),
)

watch(testKeyOptions, () => pruneSelectedTestKeyIds())

watch(
  () => props.provider.id,
  () => {
    modelTestProviderKeys.value = []
    modelTestKeysLoadedProviderId.value = null
    selectedTestKeyIds.value = []
  },
)

// 暴露给父组件
defineExpose({
  reload: refresh
})
</script>
