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
    :selecting-model-name="pendingTestModel ? (pendingTestModel.global_model_display_name || pendingTestModel.provider_model_name) : null"
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
    :start-disabled="!selectedTestEndpoint || !!testRequestHeadersError || !!testRequestBodyError"
    @close="handleTestDialogClose"
    @back="handleTestDialogBack"
    @start="handleStartPendingTest"
    @select-endpoint="handleSelectTestEndpoint"
    @update:request-headers-draft="testRequestHeadersDraft = $event"
    @update:request-body-draft="testRequestBodyDraft = $event"
  />
</template>

<script setup lang="ts">
import { ref, computed } from 'vue'
import { useSmartPagination } from '@/composables/useSmartPagination'
import { useModelTest } from '@/composables/useModelTest'
import { Box, Edit, Layers, Power, Copy, Loader2, Play } from 'lucide-vue-next'
import Card from '@/components/ui/card.vue'
import Button from '@/components/ui/button.vue'
import { useToast } from '@/composables/useToast'
import { useClipboard } from '@/composables/useClipboard'
import { sortResolutionEntries } from '@/utils/form'
import {
  type Model,
  type ProviderEndpoint,
} from '@/api/endpoints'
import { updateModel } from '@/api/endpoints/models'
import { parseApiError } from '@/utils/errorParser'
import { formatApiFormat } from '@/api/endpoints/types/api-format'
import type { ProviderWithEndpointsSummary } from '@/api/endpoints'
import ModelTestDialog from './ModelTestDialog.vue'
import {
  buildDefaultModelTestRequestHeaders,
  buildDefaultModelTestRequestBody,
  parseModelTestRequestHeadersDraft,
  parseModelTestRequestBodyDraft,
  POOL_TEST_CONCURRENCY,
  SINGLE_TEST_CONCURRENCY,
} from './model-test-request'

const props = defineProps<{
  provider: ProviderWithEndpointsSummary
  models?: Model[]
  endpoints?: ProviderEndpoint[]
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
const isPoolManagedProvider = computed(() => Boolean(props.provider.pool_advanced))
const activeEndpoints = computed(() => (props.endpoints ?? []).filter(endpoint => endpoint.is_active))
const parsedTestRequestHeaders = computed(() => parseModelTestRequestHeadersDraft(testRequestHeadersDraft.value))
const testRequestHeadersError = computed(() => parsedTestRequestHeaders.value.error)
const parsedTestRequestBody = computed(() => parseModelTestRequestBodyDraft(testRequestBodyDraft.value))
const testRequestBodyError = computed(() => parsedTestRequestBody.value.error)
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

// 检查是否有视频分辨率计费配置
function hasVideoPricing(model: Model): boolean {
  const priceByResolution = model.effective_config?.billing?.video?.price_per_second_by_resolution
    || model.config?.billing?.video?.price_per_second_by_resolution
  return priceByResolution && typeof priceByResolution === 'object' && Object.keys(priceByResolution).length > 0
}

// 获取视频计费的显示文本
function getVideoPricingDisplay(model: Model): string {
  const priceByResolution = model.effective_config?.billing?.video?.price_per_second_by_resolution
    || model.config?.billing?.video?.price_per_second_by_resolution
  if (!priceByResolution || typeof priceByResolution !== 'object') return ''
  const entries = sortResolutionEntries(Object.entries(priceByResolution))
  if (entries.length === 0) return ''
  // 获取最低分辨率和价格
  const [firstRes, firstPrice] = entries[0]
  const priceStr = `${firstRes} $${(firstPrice as number).toFixed(2)}/s`
  if (entries.length > 1) {
    return `${priceStr} [${entries.length}种]`
  }
  return priceStr
}

// 获取视频计费详情的 tooltip
function getVideoPricingTooltip(model: Model): string {
  const priceByResolution = model.effective_config?.billing?.video?.price_per_second_by_resolution
    || model.config?.billing?.video?.price_per_second_by_resolution
  if (!priceByResolution || typeof priceByResolution !== 'object') return ''
  const entries = sortResolutionEntries(Object.entries(priceByResolution))
  return entries.map(([res, price]) => `${res}: $${(price as number).toFixed(4)}/s`).join('\n')
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
  const model = pendingTestModel.value
  const modelName = model.global_model_name || model.provider_model_name
  const endpointPrefix = `[${formatApiFormat(endpoint.api_format)}] `
  await modelTest.startTest({
    mode: 'global',
    modelName,
    displayLabel: `${endpointPrefix}${modelName}`,
    apiFormat: endpoint.api_format,
    endpointId: endpoint.id,
    requestHeaders,
    requestBody,
    concurrency: isPoolManagedProvider.value ? POOL_TEST_CONCURRENCY : SINGLE_TEST_CONCURRENCY,
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
  selectedTestEndpoint.value = activeEndpoints.value[0] ?? null
  testRequestHeadersResetValue.value = buildDefaultModelTestRequestHeaders()
  testRequestHeadersDraft.value = testRequestHeadersResetValue.value
  testRequestBodyResetValue.value = buildDefaultModelTestRequestBody(
    model.global_model_name || model.provider_model_name,
  )
  testRequestBodyDraft.value = testRequestBodyResetValue.value
  modelTest.testResult.value = null
  modelTest.dialogOpen.value = true
}

// 暴露给父组件
defineExpose({
  reload: refresh
})
</script>
