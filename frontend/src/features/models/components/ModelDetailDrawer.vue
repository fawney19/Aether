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

              <!-- 模型能力 -->
              <div class="space-y-3">
                <h4 class="font-semibold text-sm">
                  模型能力
                </h4>
                <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
                  <div class="flex items-center gap-2 p-3 rounded-lg border">
                    <Zap class="w-5 h-5 text-muted-foreground" />
                    <div class="flex-1">
                      <p class="text-sm font-medium">
                        Streaming
                      </p>
                      <p class="text-xs text-muted-foreground">
                        流式输出
                      </p>
                    </div>
                    <Badge
                      :variant="model.config?.streaming !== false ? 'default' : 'secondary'"
                      class="text-xs"
                    >
                      {{ model.config?.streaming !== false ? '支持' : '不支持' }}
                    </Badge>
                  </div>
                  <div class="flex items-center gap-2 p-3 rounded-lg border">
                    <Image class="w-5 h-5 text-muted-foreground" />
                    <div class="flex-1">
                      <p class="text-sm font-medium">
                        Image Generation
                      </p>
                      <p class="text-xs text-muted-foreground">
                        图像生成
                      </p>
                    </div>
                    <Badge
                      :variant="model.config?.image_generation === true ? 'default' : 'secondary'"
                      class="text-xs"
                    >
                      {{ model.config?.image_generation === true ? '支持' : '不支持' }}
                    </Badge>
                  </div>
                  <div class="flex items-center gap-2 p-3 rounded-lg border">
                    <Eye class="w-5 h-5 text-muted-foreground" />
                    <div class="flex-1">
                      <p class="text-sm font-medium">
                        Vision
                      </p>
                      <p class="text-xs text-muted-foreground">
                        视觉理解
                      </p>
                    </div>
                    <Badge
                      :variant="model.config?.vision === true ? 'default' : 'secondary'"
                      class="text-xs"
                    >
                      {{ model.config?.vision === true ? '支持' : '不支持' }}
                    </Badge>
                  </div>
                  <div class="flex items-center gap-2 p-3 rounded-lg border">
                    <Wrench class="w-5 h-5 text-muted-foreground" />
                    <div class="flex-1">
                      <p class="text-sm font-medium">
                        Tool Use
                      </p>
                      <p class="text-xs text-muted-foreground">
                        工具调用
                      </p>
                    </div>
                    <Badge
                      :variant="model.config?.function_calling === true ? 'default' : 'secondary'"
                      class="text-xs"
                    >
                      {{ model.config?.function_calling === true ? '支持' : '不支持' }}
                    </Badge>
                  </div>
                  <div class="flex items-center gap-2 p-3 rounded-lg border">
                    <Brain class="w-5 h-5 text-muted-foreground" />
                    <div class="flex-1">
                      <p class="text-sm font-medium">
                        Extended Thinking
                      </p>
                      <p class="text-xs text-muted-foreground">
                        深度思考
                      </p>
                    </div>
                    <Badge
                      :variant="model.config?.extended_thinking === true ? 'default' : 'secondary'"
                      class="text-xs"
                    >
                      {{ model.config?.extended_thinking === true ? '支持' : '不支持' }}
                    </Badge>
                  </div>
                </div>
              </div>

              <!-- 模型偏好 -->
              <div
                v-if="model.supported_capabilities && model.supported_capabilities.length > 0"
                class="space-y-3"
              >
                <h4 class="font-semibold text-sm">
                  模型偏好
                </h4>
                <div class="flex flex-wrap gap-2">
                  <Badge
                    v-for="cap in model.supported_capabilities"
                    :key="cap"
                    variant="outline"
                    class="text-xs"
                  >
                    {{ getCapabilityDisplayName(cap) }}
                  </Badge>
                </div>
              </div>

              <!-- 默认定价 -->
              <div class="space-y-3">
                <h4 class="font-semibold text-sm">
                  默认定价
                </h4>

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
                      <Label class="text-xs text-muted-foreground">缓存创建 ($/M)</Label>
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
                      {{ model.provider_count || 0 }}
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
                @add-provider="$emit('addProvider')"
                @edit-provider="handleEditProviderFromRouting"
                @toggle-provider-status="handleToggleProviderFromRouting"
                @delete-provider="handleDeleteProviderFromRouting"
              />
            </div>
          </div>
        </Card>
      </div>
    </Transition>
  </Teleport>
</template>

<script setup lang="ts">
import { ref, watch } from 'vue'
import {
  X,
  Eye,
  Wrench,
  Brain,
  Zap,
  Image,
  Building2,
  Edit,
  Power,
  Copy,
  Layers,
  BarChart3
} from 'lucide-vue-next'
import { useEscapeKey } from '@/composables/useEscapeKey'
import { useToast } from '@/composables/useToast'
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

// 使用外部类型定义
import type { GlobalModelResponse } from '@/api/global-models'
import type { TieredPricingConfig, PricingTier } from '@/api/endpoints/types'
import type { CapabilityDefinition } from '@/api/endpoints'
import type { RoutingProviderInfo } from '@/api/global-models'

const props = withDefaults(defineProps<Props>(), {
  hasBlockingDialogOpen: false,
})
const emit = defineEmits<{
  'update:open': [value: boolean]
  'editModel': [model: GlobalModelResponse]
  'toggleModelStatus': [model: GlobalModelResponse]
  'addProvider': []
  'editProvider': [provider: any]
  'deleteProvider': [provider: any]
  'toggleProviderStatus': [provider: any]
}>()
const { success: showSuccess, error: showError } = useToast()
const { copyToClipboard } = useClipboard()

interface Props {
  model: GlobalModelResponse | null
  open: boolean
  hasBlockingDialogOpen?: boolean
  capabilities?: CapabilityDefinition[]
}

// RoutingTab 引用
const routingTabRef = ref<InstanceType<typeof RoutingTab> | null>(null)

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
  routingTabRef.value?.loadRoutingData?.()
}

// 暴露刷新方法给父组件
defineExpose({
  refreshRoutingData
})

// 根据能力名称获取显示名称
function getCapabilityDisplayName(capName: string): string {
  const cap = props.capabilities?.find(c => c.name === capName)
  return cap?.display_name || capName
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
  if (limit >= 1000000) {
    return `${(limit / 1000000).toFixed(1)}M`
  } else if (limit >= 1000) {
    return `${(limit / 1000).toFixed(0)}K`
  }
  return limit.toString()
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

// 监听 open 变化，重置 tab
watch(() => props.open, (newOpen) => {
  if (newOpen) {
    // 直接设置为 basic，不需要先重置为空
    detailTab.value = 'basic'
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
