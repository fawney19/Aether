<template>
  <!-- 请求详情抽屉 -->
  <Teleport to="body">
    <Transition name="drawer">
      <div
        v-if="isOpen"
        class="fixed inset-0 z-50 flex justify-end"
        @click.self="handleClose"
      >
        <!-- 背景遮罩 -->
        <div
          class="absolute inset-0 bg-black/30 backdrop-blur-sm"
          @click="handleClose"
        />

        <!-- 抽屉内容 -->
        <Card class="relative h-full w-[800px] max-w-[90vw] rounded-none shadow-2xl flex flex-col">
          <!-- 固定头部 - 整合基本信息 -->
          <div class="sticky top-0 z-10 bg-background border-b px-6 py-4 flex-shrink-0">
            <!-- 第一行：标题、模型、状态、操作按钮 -->
            <div class="flex items-center justify-between gap-4 mb-3">
              <div class="flex items-center gap-3 flex-wrap">
                <h3 class="text-lg font-semibold">
                  请求详情
                </h3>
                <div class="flex items-center gap-1 text-sm font-mono text-muted-foreground bg-muted px-2 py-0.5 rounded">
                  <span>{{ detail?.model || '-' }}</span>
                  <template v-if="detail?.target_model && detail.target_model !== detail.model">
                    <svg
                      xmlns="http://www.w3.org/2000/svg"
                      viewBox="0 0 20 20"
                      fill="currentColor"
                      class="w-3 h-3 flex-shrink-0"
                    >
                      <path
                        fill-rule="evenodd"
                        d="M3 10a.75.75 0 01.75-.75h10.638L10.23 5.29a.75.75 0 111.04-1.08l5.5 5.25a.75.75 0 010 1.08l-5.5 5.25a.75.75 0 11-1.04-1.08l4.158-3.96H3.75A.75.75 0 013 10z"
                        clip-rule="evenodd"
                      />
                    </svg>
                    <span>{{ detail.target_model }}</span>
                  </template>
                </div>
                <Badge
                  v-if="detail?.status_code === 200"
                  variant="success"
                >
                  {{ detail.status_code }}
                </Badge>
                <Badge
                  v-else-if="detail"
                  variant="destructive"
                >
                  {{ detail.status_code }}
                </Badge>
                <Badge
                  v-if="detail"
                  variant="outline"
                  class="text-xs"
                >
                  {{ detail.is_stream ? '流式' : '标准' }}
                </Badge>
              </div>
              <div class="flex items-center gap-1 shrink-0">
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-8 w-8"
                  :disabled="loading"
                  title="刷新"
                  @click="refreshDetail"
                >
                  <RefreshCw
                    class="w-4 h-4"
                    :class="{ 'animate-spin': loading }"
                  />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-8 w-8"
                  title="关闭"
                  @click="handleClose"
                >
                  <X class="w-4 h-4" />
                </Button>
              </div>
            </div>
            <!-- 第二行：关键元信息 -->
            <div
              v-if="detail"
              class="flex items-center flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground"
            >
              <span class="flex items-center gap-1">
                <span class="font-medium text-foreground">ID:</span>
                <span class="font-mono">{{ detail.request_id || detail.id }}</span>
              </span>
              <span class="opacity-40">|</span>
              <span>{{ formatDateTime(detail.created_at) }}</span>
              <span class="opacity-40">|</span>
              <span>{{ formatApiFormat(detail.api_format) }}</span>
              <span class="opacity-40">|</span>
              <span>用户: {{ detail.user?.username || 'Unknown' }}</span>
              <span class="opacity-40">|</span>
              <span class="font-mono">{{ detail.api_key?.display || 'N/A' }}</span>
            </div>
          </div>

          <!-- 可滚动内容区域 -->
          <div class="flex-1 min-h-0 overflow-y-auto px-6 py-4 scrollbar-stable">
            <!-- Loading State -->
            <div
              v-if="loading"
              class="py-8 space-y-4"
            >
              <Skeleton class="h-8 w-full" />
              <Skeleton class="h-32 w-full" />
              <Skeleton class="h-64 w-full" />
            </div>

            <!-- Error State -->
            <Card
              v-else-if="error"
              class="border-red-200 dark:border-red-800"
            >
              <div class="p-4">
                <p class="text-sm text-red-600 dark:text-red-400">
                  {{ error }}
                </p>
              </div>
            </Card>

            <!-- Detail Content -->
            <div
              v-else-if="detail"
              class="space-y-4"
            >
              <!-- 费用与性能概览 -->
              <Card>
                <div class="p-4">
                  <!-- 总费用和响应时间（独立显示） -->
                  <div class="flex items-center mb-4">
                    <div class="flex items-center">
                      <span class="text-xs text-muted-foreground w-[56px]">总费用</span>
                      <span class="text-lg font-bold text-green-600 dark:text-green-400">
                        ${{ ((typeof detail.cost === 'object' ? detail.cost?.total : detail.cost) || detail.total_cost || 0).toFixed(6) }}
                      </span>
                    </div>
                    <Separator
                      orientation="vertical"
                      class="h-6 mx-6"
                    />
                    <div class="flex items-center">
                      <span class="text-xs text-muted-foreground w-[56px]">响应时间</span>
                      <span class="text-lg font-bold">{{ detail.response_time_ms ? formatResponseTime(detail.response_time_ms).value : 'N/A' }}</span>
                      <span class="text-sm text-muted-foreground ml-1">{{ detail.response_time_ms ? formatResponseTime(detail.response_time_ms).unit : '' }}</span>
                    </div>
                  </div>

                  <!-- 分隔线 -->
                  <Separator class="mb-4" />

                  <!-- 统一使用阶梯计费展示方式 -->
                  <!-- 单价信息行 -->
                  <div class="text-xs text-muted-foreground mb-3 flex items-center gap-2 flex-wrap">
                    <span class="text-[10px] px-1.5 py-0.5 rounded bg-muted text-muted-foreground/70">{{ priceSourceLabel }}</span>
                    <span class="text-foreground">|</span>
                    <span>总输入上下文: <span class="font-mono font-medium text-foreground">{{ formatNumber(totalInputContext) }}</span></span>
                    <span class="text-muted-foreground/60">(输入 {{ formatNumber(detail.tokens?.input || detail.input_tokens || 0) }} + 缓存创建 {{ formatNumber(detail.cache_creation_input_tokens || 0) }} + 缓存读取 {{ formatNumber(detail.cache_read_input_tokens || 0) }})</span>
                    <Badge
                      v-if="displayTiers.length > 1"
                      variant="outline"
                      class="text-[10px] px-1.5 py-0 h-4"
                    >
                      命中第 {{ currentTierIndex + 1 }} 阶
                    </Badge>
                  </div>

                  <!-- 统一使用阶梯展示格式 -->
                  <div class="space-y-2">
                    <div
                      v-for="(tier, index) in displayTiers"
                      :key="index"
                      class="rounded-lg p-3 space-y-2"
                      :class="index === currentTierIndex
                        ? 'bg-primary/5 border border-primary/30'
                        : 'bg-muted/20 border border-border/50 opacity-60'"
                    >
                      <!-- 阶梯标题行 -->
                      <div class="flex items-center justify-between text-xs">
                        <div class="flex items-center gap-2">
                          <span
                            class="font-medium"
                            :class="index === currentTierIndex ? 'text-primary' : 'text-muted-foreground'"
                          >
                            第 {{ index + 1 }} 阶
                          </span>
                          <span class="text-muted-foreground">
                            {{ getTierRangeText(tier, index, displayTiers) }}
                          </span>
                          <Badge
                            v-if="index === currentTierIndex"
                            variant="default"
                            class="text-[10px] px-1.5 py-0 h-4"
                          >
                            当前
                          </Badge>
                        </div>
                        <!-- 单价信息 -->
                        <div class="text-muted-foreground flex items-center gap-2">
                          <span>输入 ${{ formatPrice(tier.input_price_per_1m) }}/M</span>
                          <span>输出 ${{ formatPrice(tier.output_price_per_1m) }}/M</span>
                          <span v-if="tier.cache_creation_price_per_1m">
                            缓存创建 ${{ formatPrice(tier.cache_creation_price_per_1m) }}/M
                          </span>
                          <span v-if="tier.cache_read_price_per_1m">
                            缓存读取 ${{ formatPrice(tier.cache_read_price_per_1m) }}/M
                          </span>
                        </div>
                      </div>

                      <!-- 当前阶梯的详细计算 -->
                      <template v-if="index === currentTierIndex">
                        <!-- 输入 输出 -->
                        <div class="flex items-center">
                          <div class="flex items-center flex-1">
                            <span class="text-xs text-muted-foreground w-[56px]">输入</span>
                            <span class="text-sm font-semibold font-mono flex-1 text-center">{{ detail.tokens?.input || detail.input_tokens || 0 }}</span>
                            <span class="text-xs font-mono">${{ (detail.cost?.input || detail.input_cost || 0).toFixed(6) }}</span>
                          </div>
                          <Separator
                            orientation="vertical"
                            class="h-4 mx-4"
                          />
                          <div class="flex items-center flex-1">
                            <span class="text-xs text-muted-foreground w-[56px]">输出</span>
                            <span class="text-sm font-semibold font-mono flex-1 text-center">{{ detail.tokens?.output || detail.output_tokens || 0 }}</span>
                            <span class="text-xs font-mono">${{ (detail.cost?.output || detail.output_cost || 0).toFixed(6) }}</span>
                          </div>
                        </div>
                        <!-- 缓存创建 缓存读取（始终显示） -->
                        <div class="flex items-center">
                          <div class="flex items-center flex-1">
                            <span class="text-xs text-muted-foreground w-[56px]">缓存创建</span>
                            <span class="text-sm font-semibold font-mono flex-1 text-center">{{ detail.cache_creation_input_tokens || 0 }}</span>
                            <span class="text-xs font-mono">${{ (detail.cache_creation_cost || 0).toFixed(6) }}</span>
                          </div>
                          <Separator
                            orientation="vertical"
                            class="h-4 mx-4"
                          />
                          <div class="flex items-center flex-1">
                            <span class="text-xs text-muted-foreground w-[56px]">缓存读取</span>
                            <span class="text-sm font-semibold font-mono flex-1 text-center">{{ detail.cache_read_input_tokens || 0 }}</span>
                            <span class="text-xs font-mono">${{ (detail.cache_read_cost || 0).toFixed(6) }}</span>
                          </div>
                        </div>
                        <!-- 按次计费 -->
                        <div
                          v-if="detail.request_cost"
                          class="flex items-center"
                        >
                          <div class="flex items-center flex-1">
                            <span class="text-xs text-muted-foreground w-[56px]">按次计费</span>
                            <span class="text-sm font-semibold font-mono flex-1 text-center" />
                            <span class="text-xs font-mono">${{ detail.request_cost.toFixed(6) }}</span>
                          </div>
                          <Separator
                            orientation="vertical"
                            class="h-4 mx-4 invisible"
                          />
                          <div class="flex items-center flex-1 invisible">
                            <span class="text-xs text-muted-foreground w-[56px]">占位</span>
                            <span class="text-sm font-semibold font-mono flex-1 text-center">0</span>
                            <span class="text-xs font-mono">$0.000000</span>
                          </div>
                        </div>
                      </template>
                    </div>
                  </div>
                </div>
              </Card>

              <!-- 请求链路追踪卡片 -->
              <div v-if="detail.request_id || detail.id">
                <HorizontalRequestTimeline
                  :request-id="detail.request_id || detail.id"
                  :override-status-code="detail.status_code"
                />
              </div>

              <!-- 响应客户端错误卡片 -->
              <Card
                v-if="detail.error_message"
                class="border-red-200 dark:border-red-800"
              >
                <div class="p-4">
                  <h4 class="text-sm font-semibold text-red-600 dark:text-red-400 mb-2">
                    响应客户端错误
                  </h4>
                  <div class="bg-red-50 dark:bg-red-900/20 rounded-lg p-3">
                    <p class="text-sm text-red-800 dark:text-red-300">
                      {{ detail.error_message }}
                    </p>
                  </div>
                </div>
              </Card>

              <!-- Tabs 区域 -->
              <Card>
                <div class="p-4">
                  <Tabs
                    v-model="activeTab"
                    :default-value="activeTab"
                  >
                    <!-- Tab + 图标工具栏同行 -->
                    <div class="flex items-center justify-between border-b pb-2 mb-3">
                      <!-- 左侧 Tab -->
                      <div class="flex items-center">
                        <button
                          v-for="tab in visibleTabs"
                          :key="tab.name"
                          class="px-3 py-1.5 text-sm transition-colors border-b-2 -mb-[9px]"
                          :class="activeTab === tab.name
                            ? 'border-primary text-foreground font-medium'
                            : 'border-transparent text-muted-foreground hover:text-foreground'"
                          @click="activeTab = tab.name"
                        >
                          {{ tab.label }}
                        </button>
                      </div>
                      <!-- 右侧图标工具栏 -->
                      <div class="flex items-center gap-0.5">
                        <!-- 请求头专用：对比/客户端/提供商 切换组 -->
                        <template v-if="activeTab === 'request-headers' && hasProviderHeaders">
                          <button
                            title="对比"
                            class="p-1.5 rounded transition-colors"
                            :class="viewMode === 'compare' ? 'bg-primary text-primary-foreground' : 'text-muted-foreground hover:bg-muted'"
                            @click="viewMode = 'compare'"
                          >
                            <Columns2 class="w-4 h-4" />
                          </button>
                          <button
                            title="客户端"
                            class="p-1.5 rounded transition-colors"
                            :class="viewMode === 'formatted' && dataSource === 'client' ? 'bg-primary text-primary-foreground' : 'text-muted-foreground hover:bg-muted'"
                            @click="viewMode = 'formatted'; dataSource = 'client'"
                          >
                            <Monitor class="w-4 h-4" />
                          </button>
                          <button
                            title="提供商"
                            class="p-1.5 rounded transition-colors"
                            :class="viewMode === 'formatted' && dataSource === 'provider' ? 'bg-primary text-primary-foreground' : 'text-muted-foreground hover:bg-muted'"
                            @click="viewMode = 'formatted'; dataSource = 'provider'"
                          >
                            <Server class="w-4 h-4" />
                          </button>
                          <Separator
                            orientation="vertical"
                            class="h-4 mx-1"
                          />
                        </template>
                        <!-- 展开/收缩 -->
                        <button
                          :title="currentExpandDepth === 0 ? '展开全部' : '收缩全部'"
                          class="p-1.5 rounded transition-colors"
                          :class="viewMode === 'compare'
                            ? 'text-muted-foreground/40 cursor-not-allowed'
                            : 'text-muted-foreground hover:bg-muted'"
                          :disabled="viewMode === 'compare'"
                          @click="currentExpandDepth === 0 ? expandAll() : collapseAll()"
                        >
                          <Maximize2
                            v-if="currentExpandDepth === 0"
                            class="w-4 h-4"
                          />
                          <Minimize2
                            v-else
                            class="w-4 h-4"
                          />
                        </button>
                        <!-- 复制 -->
                        <button
                          :title="copiedStates[activeTab] ? '已复制' : '复制'"
                          class="p-1.5 rounded transition-colors"
                          :class="viewMode === 'compare'
                            ? 'text-muted-foreground/40 cursor-not-allowed'
                            : 'text-muted-foreground hover:bg-muted'"
                          :disabled="viewMode === 'compare'"
                          @click="copyJsonToClipboard(activeTab)"
                        >
                          <Check
                            v-if="copiedStates[activeTab]"
                            class="w-4 h-4 text-green-500"
                          />
                          <Copy
                            v-else
                            class="w-4 h-4"
                          />
                        </button>
                      </div>
                    </div>

                    <!-- Tab 内容 -->
                    <TabsContent value="request-headers">
                      <RequestHeadersContent
                        :detail="detail"
                        :view-mode="viewMode"
                        :data-source="dataSource"
                        :current-header-data="currentHeaderData"
                        :current-expand-depth="currentExpandDepth"
                        :has-provider-headers="hasProviderHeaders"
                        :client-headers-with-diff="clientHeadersWithDiff"
                        :provider-headers-with-diff="providerHeadersWithDiff"
                        :header-stats="headerStats"
                        :is-dark="isDark"
                      />
                    </TabsContent>

                    <TabsContent value="request-body">
                      <JsonContent
                        :data="detail.request_body"
                        :view-mode="viewMode"
                        :expand-depth="currentExpandDepth"
                        :is-dark="isDark"
                        empty-message="无请求体信息"
                      />
                    </TabsContent>

                    <TabsContent value="response-headers">
                      <JsonContent
                        :data="actualResponseHeaders"
                        :view-mode="viewMode"
                        :expand-depth="currentExpandDepth"
                        :is-dark="isDark"
                        empty-message="无响应头信息"
                      />
                    </TabsContent>

                    <TabsContent value="response-body">
                      <JsonContent
                        :data="detail.response_body"
                        :view-mode="viewMode"
                        :expand-depth="currentExpandDepth"
                        :is-dark="isDark"
                        empty-message="无响应体信息"
                      />
                    </TabsContent>

                    <TabsContent value="metadata">
                      <JsonContent
                        :data="detail.metadata"
                        :view-mode="viewMode"
                        :expand-depth="currentExpandDepth"
                        :is-dark="isDark"
                        empty-message="无元数据信息"
                      />
                    </TabsContent>
                  </Tabs>
                </div>
              </Card>
            </div>
          </div>
        </Card>
      </div>
    </Transition>
  </Teleport>
</template>

<script setup lang="ts">
import { ref, watch, computed } from 'vue'
import Button from '@/components/ui/button.vue'
import { useEscapeKey } from '@/composables/useEscapeKey'
import { useClipboard } from '@/composables/useClipboard'
import Card from '@/components/ui/card.vue'
import Badge from '@/components/ui/badge.vue'
import Separator from '@/components/ui/separator.vue'
import Skeleton from '@/components/ui/skeleton.vue'
import Tabs from '@/components/ui/tabs.vue'
import TabsContent from '@/components/ui/tabs-content.vue'
import { Copy, Check, Maximize2, Minimize2, Columns2, RefreshCw, X, Monitor, Server } from 'lucide-vue-next'
import { dashboardApi, type RequestDetail } from '@/api/dashboard'
import { log } from '@/utils/logger'

// 子组件
import RequestHeadersContent from './RequestDetailDrawer/RequestHeadersContent.vue'
import JsonContent from './RequestDetailDrawer/JsonContent.vue'
import HorizontalRequestTimeline from './HorizontalRequestTimeline.vue'

const props = defineProps<{
  isOpen: boolean
  requestId: string | null
}>()

const emit = defineEmits<{
  close: []
}>()

const loading = ref(false)
const error = ref<string | null>(null)
const detail = ref<RequestDetail | null>(null)
const activeTab = ref('request-body')
const copiedStates = ref<Record<string, boolean>>({})
const viewMode = ref<'compare' | 'formatted' | 'raw'>('compare')
const currentExpandDepth = ref(1)
const dataSource = ref<'client' | 'provider'>('client')
const { copyToClipboard } = useClipboard()
const historicalPricing = ref<{
  input_price: string
  output_price: string
  cache_creation_price: string
  cache_read_price: string
  request_price: string
} | null>(null)

// 监听标签页切换
watch(activeTab, (newTab) => {
  if (newTab !== 'request-headers' && viewMode.value === 'compare') {
    viewMode.value = 'formatted'
  }
})

// 检测暗色模式
const isDark = computed(() => {
  return document.documentElement.classList.contains('dark')
})

// 检测是否有提供商请求头
const hasProviderHeaders = computed(() => {
  return !!(detail.value?.provider_request_headers &&
         Object.keys(detail.value.provider_request_headers).length > 0)
})

// 获取当前数据源的请求头数据
const currentHeaderData = computed(() => {
  if (!detail.value) return null
  return dataSource.value === 'client'
    ? detail.value.request_headers
    : detail.value.provider_request_headers
})

// 价格来源标签
// tiered_pricing.source 表示定价来源: 'provider' 或 'global'
const priceSourceLabel = computed(() => {
  if (!detail.value) return '历史定价'

  const source = detail.value.tiered_pricing?.source
  if (source === 'provider') {
    return '提供商定价'
  } else if (source === 'global') {
    return '全局定价'
  }

  // 没有 tiered_pricing 时，使用历史价格
  return '历史定价'
})

// 统一的阶梯显示数据
// 如果有 tiered_pricing，使用它；否则用历史价格构建单阶梯
const displayTiers = computed(() => {
  if (!detail.value) return []

  // 如果有阶梯定价数据，直接使用
  if (detail.value.tiered_pricing?.tiers && detail.value.tiered_pricing.tiers.length > 0) {
    return detail.value.tiered_pricing.tiers
  }

  // 否则用历史价格构建单阶梯（无上限）
  return [{
    up_to: null,
    input_price_per_1m: detail.value.input_price_per_1m || 0,
    output_price_per_1m: detail.value.output_price_per_1m || 0,
    cache_creation_price_per_1m: detail.value.cache_creation_price_per_1m,
    cache_read_price_per_1m: detail.value.cache_read_price_per_1m
  }]
})

// 当前命中的阶梯索引
const currentTierIndex = computed(() => {
  if (!detail.value) return 0

  // 如果有阶梯定价，使用它的 tier_index
  if (detail.value.tiered_pricing?.tier_index !== undefined) {
    return detail.value.tiered_pricing.tier_index
  }

  // 单阶梯时默认是第0阶
  return 0
})

// 总输入上下文（输入 + 缓存创建 + 缓存读取）
const totalInputContext = computed(() => {
  if (!detail.value) return 0

  // 优先使用 tiered_pricing 中的值
  if (detail.value.tiered_pricing?.total_input_context !== undefined) {
    return detail.value.tiered_pricing.total_input_context
  }

  // 否则手动计算
  const input = detail.value.tokens?.input || detail.value.input_tokens || 0
  const cacheCreation = detail.value.cache_creation_input_tokens || 0
  const cacheRead = detail.value.cache_read_input_tokens || 0
  return input + cacheCreation + cacheRead
})

const tabs = [
  { name: 'request-headers', label: '请求头' },
  { name: 'request-body', label: '请求体' },
  { name: 'response-headers', label: '响应头' },
  { name: 'response-body', label: '响应体' },
  { name: 'metadata', label: '元数据' },
]

// 判断数据是否有实际内容（非空对象/数组）
function hasContent(data: unknown): boolean {
  if (data === null || data === undefined) return false
  if (typeof data === 'object') {
    return Object.keys(data as object).length > 0
  }
  return true
}

// 获取实际的响应头（优先 client_response_headers，回退到 response_headers）
const actualResponseHeaders = computed(() => {
  if (!detail.value) return null
  // 优先返回客户端响应头，如果没有则回退到提供商响应头
  if (hasContent(detail.value.client_response_headers)) {
    return detail.value.client_response_headers
  }
  return detail.value.response_headers
})

// 根据实际数据决定显示哪些 Tab
const visibleTabs = computed(() => {
  if (!detail.value) return []

  return tabs.filter(tab => {
    switch (tab.name) {
      case 'request-headers':
        return hasContent(detail.value!.request_headers)
      case 'request-body':
        return hasContent(detail.value!.request_body)
      case 'response-headers':
        return hasContent(actualResponseHeaders.value)
      case 'response-body':
        return hasContent(detail.value!.response_body)
      case 'metadata':
        return hasContent(detail.value!.metadata)
      default:
        return false
    }
  })
})

watch(() => props.requestId, async (newId) => {
  if (newId && props.isOpen) {
    await loadDetail(newId)
  }
})

watch(() => props.isOpen, async (isOpen) => {
  if (isOpen && props.requestId) {
    await loadDetail(props.requestId)
  }
})

async function loadDetail(id: string) {
  loading.value = true
  error.value = null
  historicalPricing.value = null
  try {
    detail.value = await dashboardApi.getRequestDetail(id)

    // 默认显示有内容的第一个可见 tab
    const visibleTabNames = visibleTabs.value.map(t => t.name)
    if (detail.value.request_body && visibleTabNames.includes('request-body')) {
      activeTab.value = 'request-body'
    } else if (detail.value.response_body && visibleTabNames.includes('response-body')) {
      activeTab.value = 'response-body'
    } else if (visibleTabNames.length > 0) {
      activeTab.value = visibleTabNames[0]
    }

    // 使用请求记录中保存的历史价格
    if (detail.value.input_price_per_1m || detail.value.output_price_per_1m || detail.value.price_per_request) {
      historicalPricing.value = {
        input_price: detail.value.input_price_per_1m ? detail.value.input_price_per_1m.toFixed(4) : 'N/A',
        output_price: detail.value.output_price_per_1m ? detail.value.output_price_per_1m.toFixed(4) : 'N/A',
        cache_creation_price: detail.value.cache_creation_price_per_1m ? detail.value.cache_creation_price_per_1m.toFixed(4) : 'N/A',
        cache_read_price: detail.value.cache_read_price_per_1m ? detail.value.cache_read_price_per_1m.toFixed(4) : 'N/A',
        request_price: detail.value.price_per_request ? detail.value.price_per_request.toFixed(4) : 'N/A'
      }
    }
  } catch (err) {
    log.error('Failed to load request detail:', err)
    error.value = '加载请求详情失败'
  } finally {
    loading.value = false
  }
}

function handleClose() {
  emit('close')
}

async function refreshDetail() {
  if (props.requestId) {
    await loadDetail(props.requestId)
  }
}

function formatDateTime(dateStr: string | null | undefined): string {
  if (!dateStr) return 'N/A'
  const date = new Date(dateStr)
  return date.toLocaleString('zh-CN', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit'
  })
}

function formatApiFormat(format: string | null | undefined): string {
  if (!format) return '-'
  const formatMap: Record<string, string> = {
    'CLAUDE': 'Claude',
    'CLAUDE_CLI': 'Claude CLI',
    'OPENAI': 'OpenAI',
    'OPENAI_CLI': 'OpenAI CLI',
    'GEMINI': 'Gemini',
    'GEMINI_CLI': 'Gemini CLI',
  }
  return formatMap[format.toUpperCase()] || format
}

function formatNumber(num: number): string {
  if (num >= 1_000_000) {
    return `${(num / 1_000_000).toFixed(1)  }M`
  } else if (num >= 1_000) {
    return `${(num / 1_000).toFixed(1)  }K`
  }
  return num.toLocaleString()
}

// 格式化响应时间，自动选择合适的单位
function formatResponseTime(ms: number): { value: string; unit: string } {
  if (ms >= 1_000) {
    return { value: (ms / 1_000).toFixed(2), unit: 's' }
  }
  return { value: ms.toString(), unit: 'ms' }
}

// 格式化价格，修复浮点数精度问题
function formatPrice(price: number): string {
  // 处理浮点数精度问题，最多保留4位小数，去掉尾部的0
  const fixed = price.toFixed(4)
  return parseFloat(fixed).toString()
}

// 获取阶梯范围文本
function getTierRangeText(tier: { up_to?: number | null }, index: number, tiers: Array<{ up_to?: number | null }>): string {
  const prevTier = index > 0 ? tiers[index - 1] : null
  const start = prevTier?.up_to ? prevTier.up_to + 1 : 0

  if (tier.up_to) {
    if (start === 0) {
      return `0 ~ ${formatNumber(tier.up_to)} tokens`
    }
    return `${formatNumber(start)} ~ ${formatNumber(tier.up_to)} tokens`
  }
  // 无上限的情况
  return `> ${formatNumber(start)} tokens`
}

function copyJsonToClipboard(tabName: string) {
  if (!detail.value) return
  // 对比模式下不允许复制
  if (viewMode.value === 'compare') return

  let data: any = null
  switch (tabName) {
    case 'request-headers':
      // 根据当前数据源选择要复制的数据
      data = dataSource.value === 'provider'
        ? detail.value.provider_request_headers
        : detail.value.request_headers
      break
    case 'request-body':
      data = detail.value.request_body
      break
    case 'response-headers':
      data = actualResponseHeaders.value
      break
    case 'response-body':
      data = detail.value.response_body
      break
    case 'metadata':
      data = detail.value.metadata
      break
  }

  if (data) {
    copyToClipboard(JSON.stringify(data, null, 2), false)
    copiedStates.value[tabName] = true
    setTimeout(() => {
      copiedStates.value[tabName] = false
    }, 2000)
  }
}

function expandAll() {
  currentExpandDepth.value = 999
}

function collapseAll() {
  currentExpandDepth.value = 0
}

// 请求头合并对比逻辑
interface HeaderEntry {
  key: string
  status: 'added' | 'modified' | 'removed' | 'unchanged'
  originalValue?: any
  newValue?: any
}

const mergedHeaderEntries = computed(() => {
  if (!detail.value?.request_headers && !detail.value?.provider_request_headers) {
    return []
  }

  const clientHeaders = detail.value?.request_headers || {}
  const providerHeaders = detail.value?.provider_request_headers || {}

  const clientKeys = new Set(Object.keys(clientHeaders))
  const providerKeys = new Set(Object.keys(providerHeaders))
  const allKeys = new Set([...clientKeys, ...providerKeys])

  const entries: HeaderEntry[] = []

  for (const key of Array.from(allKeys).sort()) {
    const entry: HeaderEntry = { key, status: 'unchanged' }

    if (clientKeys.has(key) && providerKeys.has(key)) {
      if (clientHeaders[key] !== providerHeaders[key]) {
        entry.status = 'modified'
        entry.originalValue = clientHeaders[key]
        entry.newValue = providerHeaders[key]
      } else {
        entry.status = 'unchanged'
        entry.originalValue = clientHeaders[key]
      }
    } else if (clientKeys.has(key)) {
      entry.status = 'removed'
      entry.originalValue = clientHeaders[key]
    } else {
      entry.status = 'added'
      entry.newValue = providerHeaders[key]
    }

    entries.push(entry)
  }

  return entries
})

const headerStats = computed(() => {
  const counts = {
    added: 0,
    modified: 0,
    removed: 0,
    unchanged: 0
  }

  for (const entry of mergedHeaderEntries.value) {
    counts[entry.status]++
  }

  return counts
})

const clientHeadersWithDiff = computed(() => {
  if (!detail.value?.request_headers) return []

  const headers = detail.value.request_headers
  const result = []

  for (const [key, value] of Object.entries(headers)) {
    const diffEntry = mergedHeaderEntries.value.find(e => e.key === key)
    result.push({
      key,
      value,
      status: diffEntry?.status || 'unchanged'
    })
  }

  return result
})

const providerHeadersWithDiff = computed(() => {
  if (!detail.value?.provider_request_headers) return []

  const headers = detail.value.provider_request_headers
  const result = []

  for (const [key, value] of Object.entries(headers)) {
    const diffEntry = mergedHeaderEntries.value.find(e => e.key === key)
    result.push({
      key,
      value,
      status: diffEntry?.status || 'unchanged'
    })
  }

  return result
})

// 添加 ESC 键监听
useEscapeKey(() => {
  if (props.isOpen) {
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

<style>
/* 滚动条始终预留空间，保持宽度稳定 */
.scrollbar-stable {
  scrollbar-gutter: stable;
}

/* Webkit 浏览器滚动条样式 */
.scrollbar-stable::-webkit-scrollbar {
  width: 8px;
}

.scrollbar-stable::-webkit-scrollbar-track {
  background: transparent;
}

.scrollbar-stable::-webkit-scrollbar-thumb {
  background-color: rgba(128, 128, 128, 0.5);
  border-radius: 4px;
}

.scrollbar-stable::-webkit-scrollbar-thumb:hover {
  background-color: rgba(128, 128, 128, 0.7);
}
</style>
