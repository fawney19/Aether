<template>
  <!-- 模型详情抽屉 -->
  <Teleport to="body">
    <Transition name="drawer">
      <div
        v-if="open && model"
        class="fixed inset-0 z-50 flex justify-end"
        @click.self="handleBackdropClick"
      >
        <!-- 背景遮罩 -->
        <div
          class="absolute inset-0 bg-black/30 backdrop-blur-sm"
          @click="handleBackdropClick"
        />

        <!-- 抽屉内容 -->
        <Card class="relative h-full w-full sm:w-[700px] sm:max-w-[90vw] rounded-none shadow-2xl overflow-y-auto">
          <div class="sticky top-0 z-10 bg-background border-b p-4 sm:p-6">
            <div class="flex items-start justify-between gap-3 sm:gap-4">
              <div class="space-y-1 flex-1 min-w-0">
                <div class="flex items-center gap-2">
                  <h3 class="text-lg sm:text-xl font-bold truncate">
                    {{ model.display_name }}
                  </h3>
                  <Badge
                    :variant="model.is_active ? 'default' : 'secondary'"
                    class="text-xs shrink-0"
                  >
                    {{ model.is_active ? '活跃' : '停用' }}
                  </Badge>
                </div>
                <div class="flex items-center gap-2 text-sm text-muted-foreground min-w-0">
                  <span class="font-mono shrink-0">{{ model.name }}</span>
                  <button
                    class="p-0.5 rounded hover:bg-muted transition-colors shrink-0"
                    title="复制模型 ID"
                    @click="copyToClipboard(model.name)"
                  >
                    <Copy class="w-3 h-3" />
                  </button>
                  <template v-if="model.config?.description">
                    <span class="shrink-0">·</span>
                    <span
                      class="text-xs truncate"
                      :title="model.config?.description"
                    >{{ model.config?.description }}</span>
                  </template>
                </div>
              </div>
              <div class="flex items-center gap-1 shrink-0">
                <Button
                  variant="ghost"
                  size="icon"
                  title="编辑模型"
                  @click="$emit('editModel', model)"
                >
                  <Edit class="w-4 h-4" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  :title="model.is_active ? '点击停用' : '点击启用'"
                  @click="$emit('toggleModelStatus', model)"
                >
                  <Power class="w-4 h-4" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  title="关闭"
                  @click="handleClose"
                >
                  <X class="w-4 h-4" />
                </Button>
              </div>
            </div>
          </div>

          <div class="p-4 sm:p-6">
            <!-- 自定义 Tab 切换 -->
            <div class="flex gap-1 p-1 bg-muted/40 rounded-lg mb-4">
              <button
                type="button"
                class="flex-1 px-2 sm:px-4 py-2 text-xs sm:text-sm font-medium rounded-md transition-all duration-200"
                :class="[
                  detailTab === 'basic'
                    ? 'bg-primary text-primary-foreground shadow-sm'
                    : 'text-muted-foreground hover:text-foreground hover:bg-background/50'
                ]"
                @click="detailTab = 'basic'"
              >
                基本信息
              </button>
              <button
                type="button"
                class="flex-1 px-2 sm:px-4 py-2 text-xs sm:text-sm font-medium rounded-md transition-all duration-200"
                :class="[
                  detailTab === 'routing'
                    ? 'bg-primary text-primary-foreground shadow-sm'
                    : 'text-muted-foreground hover:text-foreground hover:bg-background/50'
                ]"
                @click="detailTab = 'routing'"
              >
                <span class="hidden sm:inline">链路控制</span>
                <span class="sm:hidden">链路</span>
              </button>
              <button
                type="button"
                class="flex-1 px-2 sm:px-4 py-2 text-xs sm:text-sm font-medium rounded-md transition-all duration-200"
                :class="[
                  detailTab === 'mappings'
                    ? 'bg-primary text-primary-foreground shadow-sm'
                    : 'text-muted-foreground hover:text-foreground hover:bg-background/50'
                ]"
                @click="detailTab = 'mappings'"
              >
                <span class="hidden sm:inline">模型映射</span>
                <span class="sm:hidden">映射</span>
              </button>
            </div>

            <!-- Tab 内容 -->
            <div
              v-show="detailTab === 'basic'"
              class="space-y-6"
            >
              <!-- 基础属性 -->
              <div class="space-y-4">
                <h4 class="font-semibold text-sm">
                  基础属性
                </h4>
                <div class="grid grid-cols-2 gap-4">
                  <div>
                    <Label class="text-xs text-muted-foreground">创建时间</Label>
                    <p class="text-sm mt-1">
                      {{ formatDate(model.created_at) }}
                    </p>
                  </div>
                </div>
              </div>

              <!-- 默认定价 -->
              <div class="space-y-3">
                <h4 class="font-semibold text-sm">
                  默认定价
                </h4>

                <!-- 图片输出计费 -->
                <div
                  v-if="hasImagePricing"
                  class="space-y-2"
                >
                  <div class="flex items-center justify-between gap-3 text-sm text-muted-foreground">
                    <div class="flex items-center gap-2">
                      <span>图片输出计费</span>
                      <Badge
                        v-if="imagePricingEntries.length > 0"
                        variant="outline"
                        class="text-[10px] h-5 px-1.5"
                      >
                        矩阵
                      </Badge>
                      <Badge
                        v-if="imagePriceRangeEntries.length > 0"
                        variant="outline"
                        class="text-[10px] h-5 px-1.5"
                      >
                        区间
                      </Badge>
                    </div>
                    <span
                      v-if="imageOutputDefaultPrice !== null"
                      class="text-xs font-mono"
                    >默认 ${{ imageOutputDefaultPrice.toFixed(6) }}/张</span>
                  </div>
                  <div
                    v-if="imagePricingEntries.length > 0"
                    class="border rounded-lg overflow-hidden"
                  >
                    <Table>
                      <TableHeader>
                        <TableRow class="bg-muted/30">
                          <TableHead class="text-xs h-9">
                            分辨率
                          </TableHead>
                          <TableHead
                            v-for="quality in IMAGE_OUTPUT_QUALITIES"
                            :key="quality"
                            class="text-xs h-9 text-right"
                          >
                            {{ quality }}
                          </TableHead>
                        </TableRow>
                      </TableHeader>
                      <TableBody>
                        <TableRow
                          v-for="entry in imagePricingEntries"
                          :key="entry.size"
                          class="text-xs"
                        >
                          <TableCell class="py-2 font-mono">
                            {{ formatImageSize(entry.size) }}
                          </TableCell>
                          <TableCell
                            v-for="quality in IMAGE_OUTPUT_QUALITIES"
                            :key="`${entry.size}-${quality}`"
                            class="py-2 text-right font-mono"
                          >
                            {{ formatImagePrice(entry.prices[quality]) }}
                          </TableCell>
                        </TableRow>
                      </TableBody>
                    </Table>
                  </div>
                  <div
                    v-if="imagePriceRangeEntries.length > 0"
                    class="border rounded-lg overflow-hidden"
                  >
                    <Table>
                      <TableHeader>
                        <TableRow class="bg-muted/30">
                          <TableHead class="text-xs h-9">
                            上限像素
                          </TableHead>
                          <TableHead
                            v-for="quality in IMAGE_OUTPUT_QUALITIES"
                            :key="quality"
                            class="text-xs h-9 text-right"
                          >
                            {{ quality }}
                          </TableHead>
                        </TableRow>
                      </TableHeader>
                      <TableBody>
                        <TableRow
                          v-for="entry in imagePriceRangeEntries"
                          :key="entry.key"
                          class="text-xs"
                        >
                          <TableCell class="py-2 font-mono">
                            {{ formatPixelLimit(entry.upToPixels) }}
                          </TableCell>
                          <TableCell
                            v-for="quality in IMAGE_OUTPUT_QUALITIES"
                            :key="`${entry.key}-${quality}`"
                            class="py-2 text-right font-mono"
                          >
                            {{ formatImagePrice(entry.prices[quality]) }}
                          </TableCell>
                        </TableRow>
                      </TableBody>
                    </Table>
                  </div>
                </div>

                <!-- 单阶梯（固定价格）展示 -->
                <div
                  v-if="getTierCount(model.default_tiered_pricing) <= 1"
                  class="space-y-3"
                >
                  <div class="grid grid-cols-2 sm:grid-cols-2 gap-3">
                    <!-- 按 Token 计费 -->
                    <div class="p-3 rounded-lg border">
                      <Label class="text-xs text-muted-foreground">输入价格 ($/M)</Label>
                      <p class="text-lg font-semibold mt-1">
                        {{ getFirstTierPrice(model.default_tiered_pricing, 'input_price_per_1m') }}
                      </p>
                    </div>
                    <div class="p-3 rounded-lg border">
                      <Label class="text-xs text-muted-foreground">输出价格 ($/M)</Label>
                      <p class="text-lg font-semibold mt-1">
                        {{ getFirstTierPrice(model.default_tiered_pricing, 'output_price_per_1m') }}
                      </p>
                    </div>
                    <div class="p-3 rounded-lg border">
                      <Label class="text-xs text-muted-foreground">{{ getFirst1hCachePrice(model.default_tiered_pricing) !== '-' ? '5min 缓存创建 ($/M)' : '缓存创建 ($/M)' }}</Label>
                      <p class="text-sm font-mono mt-1">
                        {{ getFirstTierPrice(model.default_tiered_pricing, 'cache_creation_price_per_1m') }}
                      </p>
                    </div>
                    <div class="p-3 rounded-lg border">
                      <Label class="text-xs text-muted-foreground">缓存读取 ($/M)</Label>
                      <p class="text-sm font-mono mt-1">
                        {{ getFirstTierPrice(model.default_tiered_pricing, 'cache_read_price_per_1m') }}
                      </p>
                    </div>
                  </div>
                  <!-- 1h 缓存 -->
                  <div
                    v-if="getFirst1hCachePrice(model.default_tiered_pricing) !== '-'"
                    class="flex items-center gap-3 p-3 rounded-lg border bg-muted/20"
                  >
                    <Label class="text-xs text-muted-foreground whitespace-nowrap">1h 缓存创建</Label>
                    <span class="text-sm font-mono">{{ getFirst1hCachePrice(model.default_tiered_pricing) }}</span>
                  </div>
                  <!-- 按次计费 -->
                  <div
                    v-if="model.default_price_per_request && model.default_price_per_request > 0"
                    class="flex items-center gap-3 p-3 rounded-lg border bg-muted/20"
                  >
                    <Label class="text-xs text-muted-foreground whitespace-nowrap">按次计费</Label>
                    <span class="text-sm font-mono">${{ model.default_price_per_request.toFixed(3) }}/次</span>
                  </div>
                  <!-- 视频分辨率计费 -->
                  <div
                    v-if="hasVideoPricing"
                    class="space-y-2"
                  >
                    <div class="flex items-center gap-2 text-sm text-muted-foreground">
                      <Video class="w-4 h-4" />
                      <span>视频分辨率计费 ({{ videoPricingEntries.length }} 种)</span>
                    </div>
                    <div class="border rounded-lg overflow-hidden">
                      <Table>
                        <TableHeader>
                          <TableRow class="bg-muted/30">
                            <TableHead class="text-xs h-9">
                              分辨率
                            </TableHead>
                            <TableHead class="text-xs h-9 text-right">
                              单价 ($/秒)
                            </TableHead>
                          </TableRow>
                        </TableHeader>
                        <TableBody>
                          <TableRow
                            v-for="[res, price] in videoPricingEntries"
                            :key="res"
                            class="text-xs"
                          >
                            <TableCell class="py-2">
                              {{ res }}
                            </TableCell>
                            <TableCell class="py-2 text-right font-mono">
                              ${{ (price as number).toFixed(4) }}
                            </TableCell>
                          </TableRow>
                        </TableBody>
                      </Table>
                    </div>
                  </div>
                </div>

                <!-- 多阶梯计费展示 -->
                <div
                  v-else
                  class="space-y-3"
                >
                  <div class="flex items-center gap-2 text-sm text-muted-foreground">
                    <Layers class="w-4 h-4" />
                    <span>阶梯计费 ({{ getTierCount(model.default_tiered_pricing) }} 档)</span>
                  </div>

                  <!-- 阶梯价格表格 -->
                  <div class="border rounded-lg overflow-hidden">
                    <Table>
                      <TableHeader>
                        <TableRow class="bg-muted/30">
                          <TableHead class="text-xs h-9">
                            阶梯
                          </TableHead>
                          <TableHead class="text-xs h-9 text-right">
                            输入 ($/M)
                          </TableHead>
                          <TableHead class="text-xs h-9 text-right">
                            输出 ($/M)
                          </TableHead>
                          <TableHead class="text-xs h-9 text-right">
                            缓存创建
                          </TableHead>
                          <TableHead class="text-xs h-9 text-right">
                            缓存读取
                          </TableHead>
                          <TableHead class="text-xs h-9 text-right">
                            1h 缓存
                          </TableHead>
                        </TableRow>
                      </TableHeader>
                      <TableBody>
                        <TableRow
                          v-for="(tier, index) in model.default_tiered_pricing?.tiers || []"
                          :key="index"
                          class="text-xs"
                        >
                          <TableCell class="py-2">
                            <span
                              v-if="tier.up_to === null"
                              class="text-muted-foreground"
                            >
                              {{ index === 0 ? '所有' : `> ${formatTierLimit((model.default_tiered_pricing?.tiers || [])[index - 1]?.up_to)}` }}
                            </span>
                            <span v-else>
                              {{ index === 0 ? '0' : formatTierLimit((model.default_tiered_pricing?.tiers || [])[index - 1]?.up_to) }} - {{ formatTierLimit(tier.up_to) }}
                            </span>
                          </TableCell>
                          <TableCell class="py-2 text-right font-mono">
                            ${{ tier.input_price_per_1m?.toFixed(2) || '0.00' }}
                          </TableCell>
                          <TableCell class="py-2 text-right font-mono">
                            ${{ tier.output_price_per_1m?.toFixed(2) || '0.00' }}
                          </TableCell>
                          <TableCell class="py-2 text-right font-mono text-muted-foreground">
                            {{ tier.cache_creation_price_per_1m != null ? `$${tier.cache_creation_price_per_1m.toFixed(2)}` : '-' }}
                          </TableCell>
                          <TableCell class="py-2 text-right font-mono text-muted-foreground">
                            {{ tier.cache_read_price_per_1m != null ? `$${tier.cache_read_price_per_1m.toFixed(2)}` : '-' }}
                          </TableCell>
                          <TableCell class="py-2 text-right font-mono text-muted-foreground">
                            {{ get1hCachePrice(tier) }}
                          </TableCell>
                        </TableRow>
                      </TableBody>
                    </Table>
                  </div>

                  <!-- 按次计费（多阶梯时也显示） -->
                  <div
                    v-if="model.default_price_per_request && model.default_price_per_request > 0"
                    class="flex items-center gap-3 p-3 rounded-lg border bg-muted/20"
                  >
                    <Label class="text-xs text-muted-foreground whitespace-nowrap">按次计费</Label>
                    <span class="text-sm font-mono">${{ model.default_price_per_request.toFixed(3) }}/次</span>
                  </div>
                  <!-- 视频分辨率计费（多阶梯时也显示） -->
                  <div
                    v-if="hasVideoPricing"
                    class="space-y-2"
                  >
                    <div class="flex items-center gap-2 text-sm text-muted-foreground">
                      <Video class="w-4 h-4" />
                      <span>视频分辨率计费 ({{ videoPricingEntries.length }} 种)</span>
                    </div>
                    <div class="border rounded-lg overflow-hidden">
                      <Table>
                        <TableHeader>
                          <TableRow class="bg-muted/30">
                            <TableHead class="text-xs h-9">
                              分辨率
                            </TableHead>
                            <TableHead class="text-xs h-9 text-right">
                              单价 ($/秒)
                            </TableHead>
                          </TableRow>
                        </TableHeader>
                        <TableBody>
                          <TableRow
                            v-for="[res, price] in videoPricingEntries"
                            :key="res"
                            class="text-xs"
                          >
                            <TableCell class="py-2">
                              {{ res }}
                            </TableCell>
                            <TableCell class="py-2 text-right font-mono">
                              ${{ (price as number).toFixed(4) }}
                            </TableCell>
                          </TableRow>
                        </TableBody>
                      </Table>
                    </div>
                  </div>
                </div>
              </div>

              <!-- 统计信息 -->
              <div class="space-y-3">
                <h4 class="font-semibold text-sm">
                  统计信息
                </h4>
                <div class="grid grid-cols-2 gap-3">
                  <div class="p-3 rounded-lg border bg-muted/20">
                    <div class="flex items-center justify-between">
                      <Label class="text-xs text-muted-foreground">关联提供商</Label>
                      <Building2 class="w-4 h-4 text-muted-foreground" />
                    </div>
                    <p class="text-2xl font-bold mt-1">
                      {{ model.active_provider_count || 0 }}<span class="text-sm text-muted-foreground font-normal">/{{ model.provider_count || 0 }}</span>
                    </p>
                  </div>
                  <div class="p-3 rounded-lg border bg-muted/20">
                    <div class="flex items-center justify-between">
                      <Label class="text-xs text-muted-foreground">调用次数</Label>
                      <BarChart3 class="w-4 h-4 text-muted-foreground" />
                    </div>
                    <p class="text-2xl font-bold mt-1">
                      {{ model.usage_count || 0 }}
                    </p>
                  </div>
                </div>
              </div>
            </div>

            <!-- Tab 2: 链路控制 -->
            <div v-show="detailTab === 'routing'">
              <RoutingTab
                v-if="model"
                ref="routingTabRef"
                :global-model-id="model.id"
                :routing-data="routingData"
                :loading="routingLoading"
                :error="routingError"
                @refresh="loadRoutingData"
                @add-provider="$emit('addProvider')"
                @edit-provider="handleEditProviderFromRouting"
                @toggle-provider-status="handleToggleProviderFromRouting"
                @delete-provider="handleDeleteProviderFromRouting"
              />
            </div>

            <!-- Tab 3: 模型映射 -->
            <div v-show="detailTab === 'mappings'">
              <ModelMappingsTab
                v-if="model"
                ref="modelMappingsTabRef"
                :global-model-id="model.id"
                :model-name="model.name"
                :mappings="model.config?.model_mappings || []"
                :routing-data="routingData"
                :loading-preview="routingLoading"
                @update="handleMappingsUpdate"
                @refresh="loadRoutingData"
                @link-provider="(providerId) => $emit('linkProvider', providerId)"
                @link-providers="(providerIds) => $emit('linkProviders', providerIds)"
              />
            </div>
          </div>
        </Card>
      </div>
    </Transition>
  </Teleport>
</template>

<script setup lang="ts">
import { ref, watch, computed } from 'vue'
import {
  X,
  Building2,
  Edit,
  Power,
  Copy,
  Layers,
  BarChart3,
  Video
} from 'lucide-vue-next'
import { useEscapeKey } from '@/composables/useEscapeKey'
import { useClipboard } from '@/composables/useClipboard'
import Card from '@/components/ui/card.vue'
import Badge from '@/components/ui/badge.vue'
import Button from '@/components/ui/button.vue'
import Label from '@/components/ui/label.vue'
import Table from '@/components/ui/table.vue'
import TableHeader from '@/components/ui/table-header.vue'
import TableBody from '@/components/ui/table-body.vue'
import TableRow from '@/components/ui/table-row.vue'
import TableHead from '@/components/ui/table-head.vue'
import TableCell from '@/components/ui/table-cell.vue'
import RoutingTab from './RoutingTab.vue'
import ModelMappingsTab from './ModelMappingsTab.vue'
import { sortResolutionEntries } from '@/utils/form'
import { parseApiError } from '@/utils/errorParser'
import { formatCompactNumber, formatTokens } from '@/utils/format'
import { getGlobalModelRoutingPreview } from '@/api/global-models'

// 使用外部类型定义
import type { GlobalModelResponse } from '@/api/global-models'
import type { TieredPricingConfig, PricingTier, ModelRoutingPreviewResponse } from '@/api/endpoints/types'
import type { RoutingProviderInfo } from '@/api/global-models'

const props = withDefaults(defineProps<Props>(), {
  hasBlockingDialogOpen: false,
})
const emit = defineEmits<{
  'update:open': [value: boolean]
  'editModel': [model: GlobalModelResponse]
  'toggleModelStatus': [model: GlobalModelResponse]
  'addProvider': []
  'editProvider': [provider: Record<string, unknown>]
  'deleteProvider': [provider: Record<string, unknown>]
  'toggleProviderStatus': [provider: Record<string, unknown>]
  'refreshModel': []
  'linkProvider': [providerId: string]
  'linkProviders': [providerIds: string[]]
}>()
const { copyToClipboard } = useClipboard()

interface Props {
  model: GlobalModelResponse | null
  open: boolean
  hasBlockingDialogOpen?: boolean
}

// RoutingTab 引用
const routingTabRef = ref<InstanceType<typeof RoutingTab> | null>(null)
// ModelMappingsTab 引用
const modelMappingsTabRef = ref<InstanceType<typeof ModelMappingsTab> | null>(null)

// 统一管理 routing 数据，避免子组件重复请求
const routingData = ref<ModelRoutingPreviewResponse | null>(null)
const routingLoading = ref(false)
const routingError = ref<string | null>(null)

// 加载 routing 数据（统一入口）
async function loadRoutingData() {
  if (!props.model?.id) return

  routingLoading.value = true
  routingError.value = null

  try {
    routingData.value = await getGlobalModelRoutingPreview(props.model.id)
  } catch (err: unknown) {
    routingError.value = parseApiError(err, '加载失败')
  } finally {
    routingLoading.value = false
  }
}

// 将 RoutingProviderInfo 转换为父组件期望的格式
function convertRoutingProviderToLegacyFormat(provider: RoutingProviderInfo) {
  return {
    id: provider.id,
    model_id: provider.model_id,
    name: provider.name,
    is_active: provider.model_is_active
  }
}

// 处理从 RoutingTab 来的编辑事件
function handleEditProviderFromRouting(provider: RoutingProviderInfo) {
  emit('editProvider', convertRoutingProviderToLegacyFormat(provider))
}

// 处理从 RoutingTab 来的状态切换事件
function handleToggleProviderFromRouting(provider: RoutingProviderInfo) {
  emit('toggleProviderStatus', convertRoutingProviderToLegacyFormat(provider))
}

// 处理从 RoutingTab 来的删除事件
function handleDeleteProviderFromRouting(provider: RoutingProviderInfo) {
  emit('deleteProvider', convertRoutingProviderToLegacyFormat(provider))
}

// 刷新路由数据
function refreshRoutingData() {
  loadRoutingData()
}

// 处理模型映射更新
function handleMappingsUpdate(_mappings: string[]) {
  // 映射已在 ModelMappingsTab 内部保存到服务器
  // 路由数据刷新由 @refresh 事件处理，这里只需通知父组件刷新模型数据
  emit('refreshModel')
}

// 暴露刷新方法给父组件
defineExpose({
  refreshRoutingData
})

// 检测是否有视频分辨率计费配置
const hasVideoPricing = computed(() => {
  const priceByResolution = props.model?.config?.billing?.video?.price_per_second_by_resolution
  return priceByResolution && typeof priceByResolution === 'object' && Object.keys(priceByResolution).length > 0
})

// 获取视频分辨率计费条目（按分辨率从低到高排序）
const videoPricingEntries = computed(() => {
  const priceByResolution = props.model?.config?.billing?.video?.price_per_second_by_resolution
  if (!priceByResolution || typeof priceByResolution !== 'object') return []
  return sortResolutionEntries(Object.entries(priceByResolution))
})

const IMAGE_OUTPUT_QUALITIES = ['low', 'medium', 'high'] as const

const imageOutputDefaultPrice = computed(() => {
  const value = props.model?.default_tiered_pricing?.image_output_price_default
  return typeof value === 'number' && Number.isFinite(value) ? value : null
})

const imagePricingEntries = computed(() => {
  const prices = props.model?.default_tiered_pricing?.image_output_prices
  if (!prices || typeof prices !== 'object') return []
  return sortResolutionEntries(Object.entries(prices)).map(([size, qualityPrices]) => ({
    size,
    prices: normalizeImageQualityPrices(qualityPrices),
  })).filter(entry => Object.values(entry.prices).some(price => price !== null))
})

const imagePriceRangeEntries = computed(() => {
  const ranges = props.model?.default_tiered_pricing?.image_output_price_ranges
  if (!Array.isArray(ranges)) return []
  return ranges.map((range, index) => {
    const object = range && typeof range === 'object' ? range as Record<string, unknown> : {}
    const rawPrices = object.prices && typeof object.prices === 'object'
      ? object.prices
      : object
    return {
      key: `${object.up_to_pixels ?? 'unbounded'}-${index}`,
      upToPixels: toFiniteNumber(object.up_to_pixels),
      prices: normalizeImageQualityPrices(rawPrices),
    }
  }).filter(entry => Object.values(entry.prices).some(price => price !== null))
})

const hasImagePricing = computed(() =>
  imageOutputDefaultPrice.value !== null
    || imagePricingEntries.value.length > 0
    || imagePriceRangeEntries.value.length > 0,
)

function normalizeImageQualityPrices(value: unknown): Record<typeof IMAGE_OUTPUT_QUALITIES[number], number | null> {
  const object = value && typeof value === 'object' ? value as Record<string, unknown> : {}
  return {
    low: toFiniteNumber(object.low),
    medium: toFiniteNumber(object.medium),
    high: toFiniteNumber(object.high),
  }
}

function toFiniteNumber(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

function formatImagePrice(value: number | null): string {
  return value === null ? '-' : `$${value.toFixed(6)}`
}

function formatImageSize(value: string): string {
  return value.replace(/\s*[xX×]\s*/g, ' x ')
}

function formatPixelLimit(value: number | null): string {
  return value === null ? '无上限' : `<= ${formatPixels(value)}`
}

function formatPixels(value: number): string {
  return `${formatCompactNumber(value)} px`
}

const detailTab = ref('basic')

// 处理背景点击
function handleBackdropClick() {
  if (!props.hasBlockingDialogOpen) {
    handleClose()
  }
}

// 关闭抽屉
function handleClose() {
  if (!props.hasBlockingDialogOpen) {
    emit('update:open', false)
  }
}

// 格式化日期
function formatDate(dateStr: string): string {
  if (!dateStr) return '-'
  const date = new Date(dateStr)
  return date.toLocaleDateString('zh-CN', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit'
  })
}

// 从 tiered_pricing 获取第一阶梯的价格
function getFirstTierPrice(
  tieredPricing: TieredPricingConfig | undefined | null,
  priceKey: 'input_price_per_1m' | 'output_price_per_1m' | 'cache_creation_price_per_1m' | 'cache_read_price_per_1m'
): string {
  if (!tieredPricing?.tiers?.length) return '-'
  const firstTier = tieredPricing.tiers[0]
  const value = firstTier[priceKey]
  if (value == null || value === 0) return '-'
  return `$${value.toFixed(2)}`
}

// 获取阶梯数量
function getTierCount(tieredPricing: TieredPricingConfig | undefined | null): number {
  return tieredPricing?.tiers?.length || 0
}

// 格式化阶梯上限（tokens 数量简化显示）
function formatTierLimit(limit: number | null | undefined): string {
  if (limit == null) return ''
  return formatTokens(limit)
}

// 获取 1h 缓存价格
function get1hCachePrice(tier: PricingTier): string {
  const ttl1h = tier.cache_ttl_pricing?.find(t => t.ttl_minutes === 60)
  if (ttl1h) {
    return `$${ttl1h.cache_creation_price_per_1m.toFixed(2)}`
  }
  return '-'
}

// 获取第一阶梯的 1h 缓存价格
function getFirst1hCachePrice(tieredPricing: TieredPricingConfig | undefined | null): string {
  if (!tieredPricing?.tiers?.length) return '-'
  return get1hCachePrice(tieredPricing.tiers[0])
}

// 监听 open 变化，重置 tab 并加载数据
watch(() => props.open, (newOpen) => {
  if (newOpen) {
    // 直接设置为 basic，不需要先重置为空
    detailTab.value = 'basic'
    // 加载 routing 数据
    loadRoutingData()
  } else {
    // 关闭时清空数据
    routingData.value = null
    routingError.value = null
  }
})

// 添加 ESC 键监听
useEscapeKey(() => {
  if (props.open) {
    handleClose()
  }
}, {
  disableOnInput: true,
  once: false
})
</script>

<style scoped>
/* 抽屉过渡动画 */
.drawer-enter-active,
.drawer-leave-active {
  transition: opacity 0.3s ease;
}

.drawer-enter-active .relative,
.drawer-leave-active .relative {
  transition: transform 0.3s ease;
}

.drawer-enter-from,
.drawer-leave-to {
  opacity: 0;
}

.drawer-enter-from .relative {
  transform: translateX(100%);
}

.drawer-leave-to .relative {
  transform: translateX(100%);
}

.drawer-enter-to .relative,
.drawer-leave-from .relative {
  transform: translateX(0);
}
</style>
