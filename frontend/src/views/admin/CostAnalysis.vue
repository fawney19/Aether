<template>
  <div class="space-y-6 px-4 sm:px-6 lg:px-0">
    <div class="flex flex-col sm:flex-row sm:items-center sm:justify-between gap-3">
      <div>
        <h1 class="text-lg font-semibold">
          成本分析
        </h1>
        <p class="text-xs text-muted-foreground">
          成本趋势、预测与节省统计
        </p>
      </div>
      <TimeRangePicker v-model="timeRange" />
    </div>

    <div class="grid grid-cols-1 lg:grid-cols-3 gap-4">
      <Card class="p-4 space-y-2">
        <div class="text-xs text-muted-foreground">
          缓存节省
        </div>
        <div class="text-lg font-semibold">
          {{ formatCurrency(costSavings?.cache_savings ?? 0) }}
        </div>
        <div class="text-xs text-muted-foreground">
          读取成本 {{ formatCurrency(costSavings?.cache_read_cost ?? 0) }}
        </div>
      </Card>
      <Card class="p-4 space-y-2">
        <div class="text-xs text-muted-foreground">
          缓存读取 Tokens
        </div>
        <div class="text-lg font-semibold">
          {{ formatTokens(costSavings?.cache_read_tokens ?? 0) }}
        </div>
        <div class="text-xs text-muted-foreground">
          预计全额成本 {{ formatCurrency(costSavings?.estimated_full_cost ?? 0) }}
        </div>
      </Card>
      <Card class="p-4 space-y-2">
        <div class="text-xs text-muted-foreground">
          缓存创建成本
        </div>
        <div class="text-lg font-semibold">
          {{ formatCurrency(costSavings?.cache_creation_cost ?? 0) }}
        </div>
        <div class="text-xs text-muted-foreground">
          基于当前时间范围
        </div>
      </Card>
    </div>

    <div class="grid grid-cols-1 lg:grid-cols-2 gap-4">
      <Card class="p-4">
        <CostForecastChart
          title="成本趋势预测"
          :history="forecastHistory"
          :forecast="forecastFuture"
          :loading="forecastLoading"
        />
      </Card>
      <QuotaProgressCard
        title="月卡消耗进度"
        :providers="quotaProviders"
        :loading="quotaLoading"
      />
    </div>

    <LeaderboardTable
      title="API Key 用量排行"
      :items="apiKeyLeaderboard"
      :metric="apiKeyLeaderboardMetric"
      :loading="apiKeyLeaderboardLoading"
      :show-metric-select="false"
      @update:metric="apiKeyLeaderboardMetric = $event"
    >
      <template #actions>
        <LeaderboardControls
          :metric="apiKeyLeaderboardMetric"
          :time-range="apiKeyLeaderboardTimeRange"
          @update:metric="apiKeyLeaderboardMetric = $event"
          @update:time-range="apiKeyLeaderboardTimeRange = $event"
        />
      </template>
      <template #pagination>
        <Pagination
          v-if="apiKeyLeaderboardTotal > 0"
          :current="apiKeyLeaderboardPage"
          :total="apiKeyLeaderboardTotal"
          :page-size="apiKeyLeaderboardPageSize"
          :page-size-options="apiKeyLeaderboardPageSizeOptions"
          @update:current="apiKeyLeaderboardPage = $event"
          @update:page-size="apiKeyLeaderboardPageSize = $event"
        />
      </template>
    </LeaderboardTable>

    <div class="grid grid-cols-1 xl:grid-cols-2 gap-4">
      <UsageProviderTable
        :data="providerStats"
        :is-admin="true"
      />

      <Card class="p-4 space-y-4">
        <div class="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
          <div>
            <h3 class="text-sm font-medium">
              提供商成本归因
            </h3>
            <p class="text-xs text-muted-foreground">
              查看选定提供商在当前时间范围内的用户贡献占比
            </p>
          </div>
          <div class="flex flex-wrap gap-2">
            <select
              v-model="selectedProviderKey"
              class="h-8 rounded-md border border-input bg-background px-2 text-xs"
            >
              <option
                v-for="provider in providerStats"
                :key="provider.providerKey ?? provider.providerId ?? provider.provider"
                :value="provider.providerKey ?? provider.providerId ?? provider.provider"
              >
                {{ provider.provider }}
              </option>
            </select>
            <select
              v-model="attributionMetric"
              class="h-8 rounded-md border border-input bg-background px-2 text-xs"
            >
              <option value="actual_cost">实际成本</option>
              <option value="total_cost">展示成本</option>
              <option value="tokens">Tokens</option>
              <option value="requests">请求数</option>
            </select>
          </div>
        </div>

        <div
          v-if="providerAttributionLoading"
          class="py-8 text-center text-xs text-muted-foreground"
        >
          正在加载归因数据...
        </div>
        <div
          v-else-if="!selectedProvider"
          class="py-8 text-center text-xs text-muted-foreground"
        >
          暂无可归因的提供商数据
        </div>
        <div
          v-else-if="providerAttributionItems.length === 0"
          class="py-8 text-center text-xs text-muted-foreground"
        >
          当前时间范围内暂无用户贡献数据
        </div>
        <div
          v-else
          class="space-y-3"
        >
          <div class="rounded-lg border p-3">
            <div class="flex items-center justify-between text-xs text-muted-foreground">
              <span>{{ selectedProvider.provider }}</span>
              <span>{{ attributionMetricLabel }}</span>
            </div>
            <div class="mt-1 text-lg font-semibold">
              {{ formatAttributionMetric(providerAttribution?.total ?? 0) }}
            </div>
          </div>

          <div class="space-y-2">
            <div
              v-for="item in providerAttributionDisplayItems"
              :key="item.id"
              class="space-y-1"
            >
              <div class="flex items-center justify-between gap-3 text-xs">
                <span class="truncate font-medium">{{ item.name }}</span>
                <span class="text-muted-foreground">{{ formatShare(item.share) }}</span>
              </div>
              <div class="h-2 overflow-hidden rounded-full bg-muted">
                <div
                  class="h-full rounded-full bg-primary"
                  :style="{ width: `${Math.min(item.share * 100, 100)}%` }"
                />
              </div>
              <div class="flex flex-wrap gap-x-3 gap-y-1 text-[11px] text-muted-foreground">
                <span>请求 {{ item.requests }}</span>
                <span>Tokens {{ formatTokens(item.total_tokens) }}</span>
                <span>展示 {{ formatCurrency(item.total_cost) }}</span>
                <span>实际 {{ formatCurrency(item.actual_cost) }}</span>
              </div>
            </div>
          </div>
        </div>
      </Card>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, watch } from 'vue'
import Card from '@/components/ui/card.vue'
import { Pagination } from '@/components/ui'
import { TimeRangePicker } from '@/components/common'
import { CostForecastChart, LeaderboardControls, LeaderboardTable, QuotaProgressCard } from '@/components/stats'
import { UsageProviderTable } from '@/features/usage/components'
import { adminApi, type CostForecastResponse, type CostSavingsResponse, type LeaderboardItem, type QuotaUsageProvider } from '@/api/admin'
import { usageApi, type UsageAttributionItem, type UsageAttributionMetric, type UsageAttributionResponse } from '@/api/usage'
import { formatCurrency, formatTokens } from '@/utils/format'
import { getDateRangeFromPeriod } from '@/features/usage/composables'
import { normalizeUsageProviderStats } from '@/features/usage/utils/providerStats'
import type { DateRangeParams } from '@/features/usage/types'
import type { ProviderStatsItem } from '@/features/usage/types'

const timeRange = ref<DateRangeParams>(getDateRangeFromPeriod('last30days'))

const forecast = ref<CostForecastResponse | null>(null)
const costSavings = ref<CostSavingsResponse | null>(null)
const quotaProviders = ref<QuotaUsageProvider[]>([])
const providerStats = ref<ProviderStatsItem[]>([])
const selectedProviderKey = ref('')
const providerAttribution = ref<UsageAttributionResponse | null>(null)
const attributionMetric = ref<UsageAttributionMetric>('actual_cost')
const apiKeyLeaderboard = ref<LeaderboardItem[]>([])
const apiKeyLeaderboardMetric = ref<'requests' | 'tokens' | 'cost'>('cost')
const apiKeyLeaderboardTimeRange = ref<DateRangeParams>(getDateRangeFromPeriod('last30days'))
const apiKeyLeaderboardPage = ref(1)
const apiKeyLeaderboardPageSize = ref(10)
const apiKeyLeaderboardTotal = ref(0)
const apiKeyLeaderboardPageSizeOptions = [10, 20, 50, 100]

const forecastLoading = ref(false)
const quotaLoading = ref(false)
const providerAttributionLoading = ref(false)
const apiKeyLeaderboardLoading = ref(false)
let forecastRequestId = 0
let savingsRequestId = 0
let quotaRequestId = 0
let providerStatsRequestId = 0
let providerAttributionRequestId = 0
let apiKeyLeaderboardRequestId = 0
let loadAllPromise: Promise<void> | null = null
let hasPendingLoadAll = false
let loadAllDebounceTimer: ReturnType<typeof setTimeout> | null = null
let providerAttributionDebounceTimer: ReturnType<typeof setTimeout> | null = null
let apiKeyLeaderboardDebounceTimer: ReturnType<typeof setTimeout> | null = null

const forecastHistory = computed(() => forecast.value?.history || [])
const forecastFuture = computed(() => forecast.value?.forecast || [])
const selectedProvider = computed(() => providerStats.value.find(provider => providerSelectionKey(provider) === selectedProviderKey.value))
const providerAttributionItems = computed(() => providerAttribution.value?.items ?? [])
const providerAttributionDisplayItems = computed<UsageAttributionItem[]>(() => {
  const items = [...providerAttributionItems.value]
  const others = providerAttribution.value?.others
  if (others && others.requests > 0) {
    items.push({ ...others, id: 'others', name: others.name || 'Others' })
  }
  return items
})
const attributionMetricLabel = computed(() => {
  switch (attributionMetric.value) {
    case 'actual_cost': return '实际成本'
    case 'total_cost': return '展示成本'
    case 'tokens': return 'Tokens'
    case 'requests': return '请求数'
    default: return '指标'
  }
})

function buildTimeRangeParams() {
  return {
    start_date: timeRange.value.start_date,
    end_date: timeRange.value.end_date,
    preset: timeRange.value.preset,
    timezone: timeRange.value.timezone,
    tz_offset_minutes: timeRange.value.tz_offset_minutes
  }
}

function providerSelectionKey(provider: ProviderStatsItem) {
  return provider.providerKey ?? provider.providerId ?? provider.provider
}

function formatShare(share: number) {
  return `${(share * 100).toFixed(1)}%`
}

function formatAttributionMetric(value: number) {
  if (attributionMetric.value === 'actual_cost' || attributionMetric.value === 'total_cost') {
    return formatCurrency(value)
  }
  if (attributionMetric.value === 'tokens') {
    return formatTokens(value)
  }
  return Math.round(value).toLocaleString('zh-CN')
}

async function loadForecast() {
  const requestId = ++forecastRequestId
  forecastLoading.value = true
  try {
    const data = await adminApi.getCostForecast(buildTimeRangeParams())
    if (requestId !== forecastRequestId) return
    forecast.value = data
  } finally {
    if (requestId === forecastRequestId) {
      forecastLoading.value = false
    }
  }
}

async function loadSavings() {
  const requestId = ++savingsRequestId
  const data = await adminApi.getCostSavings(buildTimeRangeParams())
  if (requestId !== savingsRequestId) return
  costSavings.value = data
}

async function loadQuotaUsage() {
  const requestId = ++quotaRequestId
  quotaLoading.value = true
  try {
    const response = await adminApi.getQuotaUsage()
    if (requestId !== quotaRequestId) return
    quotaProviders.value = response.providers
  } finally {
    if (requestId === quotaRequestId) {
      quotaLoading.value = false
    }
  }
}

async function loadProviderStats() {
  const requestId = ++providerStatsRequestId
  const stats = await usageApi.getUsageByProvider({
    ...buildTimeRangeParams(),
    limit: 8
  })
  if (requestId !== providerStatsRequestId) return
  providerStats.value = normalizeUsageProviderStats(stats)
  if (!providerStats.value.some(provider => providerSelectionKey(provider) === selectedProviderKey.value)) {
    selectedProviderKey.value = providerStats.value[0] ? providerSelectionKey(providerStats.value[0]) : ''
  }
  scheduleProviderAttributionLoad()
}

async function loadProviderAttribution() {
  const provider = selectedProvider.value
  if (!provider) {
    providerAttribution.value = null
    return
  }
  const requestId = ++providerAttributionRequestId
  providerAttributionLoading.value = true
  try {
    const response = await usageApi.getUsageAttribution({
      ...buildTimeRangeParams(),
      provider_id: provider.providerId && provider.providerIdentitySource !== 'legacy_name' ? provider.providerId : undefined,
      provider_name: provider.providerIdentitySource === 'legacy_name' || !provider.providerId ? provider.provider : undefined,
      group_by: 'user',
      metric: attributionMetric.value,
      limit: 8,
    })
    if (requestId !== providerAttributionRequestId) return
    providerAttribution.value = response
  } finally {
    if (requestId === providerAttributionRequestId) {
      providerAttributionLoading.value = false
    }
  }
}

async function loadApiKeyLeaderboard() {
  const requestId = ++apiKeyLeaderboardRequestId
  apiKeyLeaderboardLoading.value = true
  try {
    const response = await adminApi.getLeaderboardApiKeys({
      ...buildApiKeyLeaderboardTimeRangeParams(),
      metric: apiKeyLeaderboardMetric.value,
      order: 'desc',
      limit: apiKeyLeaderboardPageSize.value,
      offset: (apiKeyLeaderboardPage.value - 1) * apiKeyLeaderboardPageSize.value,
      include_inactive: false,
      exclude_admin: false
    })
    if (requestId !== apiKeyLeaderboardRequestId) return
    apiKeyLeaderboard.value = response.items
    apiKeyLeaderboardTotal.value = response.total
    if (response.items.length === 0 && response.total > 0 && apiKeyLeaderboardPage.value > 1) {
      apiKeyLeaderboardPage.value = 1
      scheduleApiKeyLeaderboardLoad()
    }
  } finally {
    if (requestId === apiKeyLeaderboardRequestId) {
      apiKeyLeaderboardLoading.value = false
    }
  }
}

function buildApiKeyLeaderboardTimeRangeParams() {
  return {
    start_date: apiKeyLeaderboardTimeRange.value.start_date,
    end_date: apiKeyLeaderboardTimeRange.value.end_date,
    preset: apiKeyLeaderboardTimeRange.value.preset,
    timezone: apiKeyLeaderboardTimeRange.value.timezone,
    tz_offset_minutes: apiKeyLeaderboardTimeRange.value.tz_offset_minutes
  }
}

async function loadAll() {
  if (loadAllPromise) {
    hasPendingLoadAll = true
    return loadAllPromise
  }
  loadAllPromise = Promise.all([
    loadForecast(),
    loadSavings(),
    loadQuotaUsage(),
    loadProviderStats(),
    loadProviderAttribution(),
    loadApiKeyLeaderboard()
  ])
    .then(() => undefined)
    .finally(() => {
      loadAllPromise = null
      if (hasPendingLoadAll) {
        hasPendingLoadAll = false
        void loadAll()
      }
    })
  return loadAllPromise
}

function scheduleLoadAll() {
  if (loadAllDebounceTimer) {
    clearTimeout(loadAllDebounceTimer)
  }
  loadAllDebounceTimer = setTimeout(() => {
    loadAllDebounceTimer = null
    void loadAll()
  }, 120)
}

function scheduleApiKeyLeaderboardLoad() {
  if (apiKeyLeaderboardDebounceTimer) {
    clearTimeout(apiKeyLeaderboardDebounceTimer)
  }
  apiKeyLeaderboardDebounceTimer = setTimeout(() => {
    apiKeyLeaderboardDebounceTimer = null
    void loadApiKeyLeaderboard()
  }, 120)
}

function scheduleProviderAttributionLoad() {
  if (providerAttributionDebounceTimer) {
    clearTimeout(providerAttributionDebounceTimer)
  }
  providerAttributionDebounceTimer = setTimeout(() => {
    providerAttributionDebounceTimer = null
    void loadProviderAttribution()
  }, 120)
}

function resetApiKeyLeaderboardPage() {
  if (apiKeyLeaderboardPage.value === 1) {
    return
  }
  apiKeyLeaderboardPage.value = 1
}

watch(timeRange, () => {
  resetApiKeyLeaderboardPage()
  scheduleLoadAll()
  scheduleProviderAttributionLoad()
}, { deep: true })
watch([selectedProviderKey, attributionMetric], scheduleProviderAttributionLoad)
watch(apiKeyLeaderboardMetric, () => {
  resetApiKeyLeaderboardPage()
  scheduleApiKeyLeaderboardLoad()
})
watch(apiKeyLeaderboardTimeRange, () => {
  resetApiKeyLeaderboardPage()
  scheduleApiKeyLeaderboardLoad()
}, { deep: true })
watch([apiKeyLeaderboardPage, apiKeyLeaderboardPageSize], scheduleApiKeyLeaderboardLoad)

onMounted(() => {
  void loadAll()
})

onUnmounted(() => {
  if (loadAllDebounceTimer) {
    clearTimeout(loadAllDebounceTimer)
    loadAllDebounceTimer = null
  }
  if (apiKeyLeaderboardDebounceTimer) {
    clearTimeout(apiKeyLeaderboardDebounceTimer)
    apiKeyLeaderboardDebounceTimer = null
  }
  if (providerAttributionDebounceTimer) {
    clearTimeout(providerAttributionDebounceTimer)
    providerAttributionDebounceTimer = null
  }
  hasPendingLoadAll = false
  loadAllPromise = null
  forecastRequestId += 1
  savingsRequestId += 1
  quotaRequestId += 1
  providerStatsRequestId += 1
  providerAttributionRequestId += 1
  apiKeyLeaderboardRequestId += 1
})
</script>
