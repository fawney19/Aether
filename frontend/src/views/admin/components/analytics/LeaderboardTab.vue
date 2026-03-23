<template>
  <div class="space-y-4">
    <div class="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
      <p class="text-xs text-muted-foreground">
        固定按费用排序，点击{{ dimensionLabel }}名称即可在右侧查看同时间范围下的趋势。
      </p>
      <button
        class="inline-flex h-8 items-center rounded-lg border border-border/70 bg-background px-3 text-[10px] font-medium text-muted-foreground transition-colors hover:bg-muted/40 hover:text-foreground"
        @click="exportLeaderboard"
      >
        导出 CSV
      </button>
    </div>

    <div class="grid gap-4 xl:grid-cols-[minmax(0,390px)_minmax(0,1fr)] 2xl:grid-cols-[minmax(0,420px)_minmax(0,1.18fr)]">
      <LeaderboardTable
        :title="leaderboardTitle"
        :items="paginatedLeaderboard"
        :selected-id="selectedId"
        :loading="leaderboardLoading"
        :unavailable="leaderboardUnavailable"
        :has-loaded="leaderboardHasLoaded"
        :current-page="leaderboardCurrentPage"
        :total-items="leaderboard.length"
        :page-size="LEADERBOARD_PAGE_SIZE"
        :show-pagination="true"
        @select="selectedId = $event"
        @update:current-page="leaderboardCurrentPage = $event"
      />

      <Card class="overflow-hidden">
        <div class="border-b border-border/60 px-4 py-4">
          <div class="flex flex-col gap-3 lg:flex-row lg:items-start lg:justify-between">
            <div class="min-w-0">
              <div class="flex flex-wrap items-center gap-2">
                <Badge
                  variant="outline"
                  class="border-amber-500/35 bg-amber-500/8 text-[10px] text-amber-700 dark:text-amber-300"
                >
                  {{ selectedItem ? `#${selectedItem.rank}` : '未选择' }}
                </Badge>
                <h3 class="truncate text-sm font-semibold">
                  {{ selectedLabel || `${dimensionLabel}趋势` }}
                </h3>
              </div>
              <p class="mt-1 text-[11px] text-muted-foreground">
                {{ trendLeadText }}
              </p>
            </div>

            <Tabs
              v-model="currentTrendGranularitySelection"
              class="self-start"
            >
              <div class="flex flex-col items-start gap-2">
                <div
                  v-if="isPanelRefreshing"
                  class="inline-flex h-7 items-center rounded-lg border border-border/70 bg-background px-2.5 text-[10px] text-muted-foreground"
                >
                  更新中
                </div>
                <TabsList class="tabs-button-list flex-wrap justify-start gap-1">
                  <TabsTrigger
                    v-for="option in trendGranularityTabStates"
                    :key="option.value"
                    :value="option.value"
                    :disabled="option.disabled"
                    class="min-w-[38px] px-2 py-0.5 text-[10px]"
                    :class="option.disabled ? 'cursor-not-allowed opacity-40' : ''"
                  >
                    {{ option.label }}
                  </TabsTrigger>
                </TabsList>
              </div>
            </Tabs>
          </div>

          <div
            v-if="selectedSummary"
            class="mt-4 grid grid-cols-2 gap-2 transition-opacity sm:grid-cols-4"
            :class="{ 'opacity-60': isPanelRefreshing }"
          >
            <div class="rounded-xl border border-border/60 bg-muted/15 px-3 py-2.5">
              <div class="text-[10px] uppercase tracking-[0.18em] text-muted-foreground">
                请求数
              </div>
              <div class="mt-1 text-sm font-semibold tabular-nums">
                {{ selectedSummary.total_requests.toLocaleString() }}
              </div>
            </div>
            <div class="rounded-xl border border-border/60 bg-muted/15 px-3 py-2.5">
              <div class="text-[10px] uppercase tracking-[0.18em] text-muted-foreground">
                总计 Tokens
              </div>
              <div class="mt-1 text-sm font-semibold tabular-nums">
                {{ formatTokens(selectedSummary.total_tokens) }}
              </div>
            </div>
            <div class="rounded-xl border border-border/60 bg-muted/15 px-3 py-2.5">
              <div class="text-[10px] uppercase tracking-[0.18em] text-muted-foreground">
                费用
              </div>
              <div class="mt-1 text-sm font-semibold tabular-nums">
                {{ formatCurrency(selectedSummary.total_cost) }}
              </div>
            </div>
            <div class="rounded-xl border border-border/60 bg-muted/15 px-3 py-2.5">
              <div class="text-[10px] uppercase tracking-[0.18em] text-muted-foreground">
                成本
              </div>
              <div class="mt-1 text-sm font-semibold tabular-nums">
                {{ formatCurrency(selectedSummary.total_actual_cost) }}
              </div>
            </div>
          </div>
        </div>

        <LineChart
          v-if="selectedItem && trendChartData"
          :class="{ 'opacity-60 transition-opacity': isPanelRefreshing }"
          :data="trendChartData"
          :options="trendChartOptions"
          :height="320"
        />
        <div
          v-else
          class="flex h-[320px] items-center justify-center px-6 text-center text-xs text-muted-foreground"
        >
          {{ trendEmptyText }}
        </div>
      </Card>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, ref, toRef, watch } from 'vue'
import type { ChartData, ChartOptions } from 'chart.js'
import { Badge, Card, Tabs, TabsList, TabsTrigger } from '@/components/ui'
import { LeaderboardTable } from '@/components/stats'
import LineChart from '@/components/charts/LineChart.vue'
import { useInjectedAnalyticsFilters } from '@/composables/useAnalyticsFilters'
import { useLeaderboardData, type LeaderboardDimension } from '@/composables/analytics'
import {
  analyticsGranularityTabs,
  formatAnalyticsTrendBucketLabel,
  formatAnalyticsTrendDescription,
  formatAnalyticsTrendTooltipTitle,
  isAnalyticsSingleDayRange,
} from '@/utils/analyticsGranularity'
import { downloadCsvFromObjects } from '@/utils/csvExport'
import { formatCurrency, formatTokens } from '@/utils/format'

const props = defineProps<{
  dimension: LeaderboardDimension
}>()

const dimension = toRef(props, 'dimension')
const filters = useInjectedAnalyticsFilters()
const {
  leaderboard,
  leaderboardLoading,
  leaderboardError,
  leaderboardHasLoaded,
  selectedId,
  selectedItem,
  selectedLabel,
  selectedSummary,
  selectedTrendBuckets,
  panelLoading,
  panelError,
  panelHasLoaded,
  resolvedTrendGranularity,
  currentTrendGranularitySelection,
} = useLeaderboardData(dimension)

const trendAxisTickCount = 6
const LEADERBOARD_PAGE_SIZE = 5
const leaderboardCurrentPage = ref(1)

const dimensionLabel = computed(() => (
  props.dimension === 'users' ? '用户' : 'API Key'
))

const leaderboardTitle = computed(() => `${dimensionLabel.value}费用排行榜`)
const leaderboardUnavailable = computed(() => (
  leaderboardError.value && !leaderboardHasLoaded.value
))
const leaderboardTotalPages = computed(() => (
  Math.max(1, Math.ceil(leaderboard.value.length / LEADERBOARD_PAGE_SIZE))
))
const paginatedLeaderboard = computed(() => {
  const start = (leaderboardCurrentPage.value - 1) * LEADERBOARD_PAGE_SIZE
  return leaderboard.value.slice(start, start + LEADERBOARD_PAGE_SIZE)
})

const trendGranularityTabStates = computed(() => (
  analyticsGranularityTabs.map(option => ({
    ...option,
    disabled: option.value === 'hour' ? !isAnalyticsSingleDayRange(filters.timeRange.value) : false,
  }))
))

const trendChartDescription = computed(() => (
  formatAnalyticsTrendDescription(
    resolvedTrendGranularity.value,
    '请求量、费用、成本与总计 Tokens',
  )
))

const trendLeadText = computed(() => {
  if (leaderboardUnavailable.value) {
    return `${dimensionLabel.value}排行榜接口未返回结果，请稍后重试。`
  }
  if (!selectedItem.value) {
    return `从左侧选择${dimensionLabel.value}后，这里会显示与首页仪表盘一致的趋势视图。`
  }
  return `${selectedLabel.value} · ${trendChartDescription.value}`
})

const trendEmptyText = computed(() => {
  if (leaderboardUnavailable.value) {
    return `${dimensionLabel.value}排行榜暂不可用`
  }
  if (!selectedItem.value) {
    return `请选择一个${dimensionLabel.value}查看趋势`
  }
  if (panelError.value && !panelHasLoaded.value) {
    return `${dimensionLabel.value}趋势暂不可用`
  }
  return isInitialPanelLoading.value ? '加载中...' : '暂无趋势数据'
})

const isInitialPanelLoading = computed(() => (
  panelLoading.value && !panelHasLoaded.value && selectedTrendBuckets.value.length === 0
))

const isPanelRefreshing = computed(() => (
  panelLoading.value && panelHasLoaded.value && selectedTrendBuckets.value.length > 0
))

watch(
  () => [
    props.dimension,
    filters.timeRange.value.preset,
    filters.timeRange.value.start_date,
    filters.timeRange.value.end_date,
  ],
  () => {
    leaderboardCurrentPage.value = 1
  },
)

watch(
  leaderboardTotalPages,
  (totalPages) => {
    if (leaderboardCurrentPage.value > totalPages) {
      leaderboardCurrentPage.value = totalPages
    }
  },
)

watch(
  [paginatedLeaderboard, selectedId],
  ([items, currentSelectedId]) => {
    if (!items.length) return
    if (!items.some(item => item.id === currentSelectedId)) {
      selectedId.value = items[0].id
    }
  },
  { immediate: true },
)

function computeNiceAxisMax(maxValue: number, intervals: number): number {
  if (!Number.isFinite(maxValue) || maxValue <= 0) return 1
  const rawStep = maxValue / intervals
  const magnitude = 10 ** Math.floor(Math.log10(rawStep))
  const normalized = rawStep / magnitude
  const niceNormalized = normalized <= 1
    ? 1
    : normalized <= 2
      ? 2
      : normalized <= 2.5
        ? 2.5
        : normalized <= 5
          ? 5
          : 10
  return niceNormalized * magnitude * intervals
}

const trendScaleMaxima = computed(() => {
  const intervals = Math.max(trendAxisTickCount - 1, 1)
  const requestMax = Math.max(0, ...selectedTrendBuckets.value.map(bucket => bucket.requests_total))
  const tokenMax = Math.max(0, ...selectedTrendBuckets.value.map(bucket => bucket.total_tokens))
  const costMax = Math.max(
    0,
    ...selectedTrendBuckets.value.map(bucket => Math.max(bucket.total_cost_usd, bucket.actual_total_cost_usd)),
  )

  return {
    requests: computeNiceAxisMax(requestMax, intervals),
    tokens: computeNiceAxisMax(tokenMax, intervals),
    cost: computeNiceAxisMax(costMax, intervals),
  }
})

const trendChartData = computed<ChartData<'line'> | null>(() => {
  if (!selectedTrendBuckets.value.length) return null

  return {
    labels: selectedTrendBuckets.value.map(bucket => (
      formatAnalyticsTrendBucketLabel(bucket.bucket_start, resolvedTrendGranularity.value)
    )),
    datasets: [
      {
        label: '费用',
        data: selectedTrendBuckets.value.map(bucket => bucket.total_cost_usd),
        borderColor: 'rgb(232, 145, 89)',
        backgroundColor: 'rgba(232, 145, 89, 0.12)',
        borderWidth: 2.2,
        fill: false,
        tension: 0.28,
        pointRadius: 0,
        pointHoverRadius: 4,
        pointHitRadius: 14,
        yAxisID: 'y2',
      },
      {
        label: '成本',
        data: selectedTrendBuckets.value.map(bucket => bucket.actual_total_cost_usd),
        borderColor: 'rgb(196, 92, 124)',
        backgroundColor: 'rgba(196, 92, 124, 0.12)',
        borderWidth: 2,
        borderDash: [7, 5],
        fill: false,
        tension: 0.28,
        pointRadius: 0,
        pointHoverRadius: 4,
        pointHitRadius: 14,
        yAxisID: 'y2',
      },
      {
        label: '总计 Tokens',
        data: selectedTrendBuckets.value.map(bucket => bucket.total_tokens),
        borderColor: 'rgb(103, 149, 230)',
        backgroundColor: 'rgba(103, 149, 230, 0.16)',
        borderWidth: 2.6,
        fill: true,
        tension: 0.3,
        pointRadius: 0,
        pointHoverRadius: 4,
        pointHitRadius: 14,
        yAxisID: 'y1',
      },
      {
        label: '请求数',
        data: selectedTrendBuckets.value.map(bucket => bucket.requests_total),
        borderColor: 'rgb(146, 126, 214)',
        backgroundColor: 'rgba(146, 126, 214, 0.08)',
        borderWidth: 2.2,
        borderDash: [7, 5],
        fill: false,
        tension: 0.28,
        pointRadius: 0,
        pointHoverRadius: 4,
        pointHitRadius: 14,
        yAxisID: 'y',
      },
    ],
  }
})

const trendChartOptions = computed<ChartOptions<'line'>>(() => ({
  responsive: true,
  maintainAspectRatio: false,
  interaction: {
    mode: 'index',
    intersect: false,
  },
  scales: {
    x: {
      grid: {
        color: 'rgba(156, 163, 175, 0.05)',
      },
      ticks: {
        color: 'rgb(92, 98, 109)',
        maxRotation: 0,
        autoSkip: true,
        maxTicksLimit: resolvedTrendGranularity.value === 'hour' ? 12 : undefined,
      },
    },
    y: {
      type: 'linear',
      display: true,
      position: 'left',
      min: 0,
      max: trendScaleMaxima.value.requests,
      beginAtZero: true,
      alignToPixels: true,
      bounds: 'ticks',
      grid: {
        color: 'rgba(156, 163, 175, 0.05)',
      },
      ticks: {
        color: 'rgb(92, 98, 109)',
        count: trendAxisTickCount,
      },
    },
    y1: {
      type: 'linear',
      display: true,
      position: 'right',
      min: 0,
      max: trendScaleMaxima.value.tokens,
      beginAtZero: true,
      alignToPixels: true,
      bounds: 'ticks',
      grid: {
        drawOnChartArea: false,
      },
      ticks: {
        color: 'rgb(92, 98, 109)',
        count: trendAxisTickCount,
        callback: value => {
          const tokenValue = Number(value)
          const tokenMax = trendScaleMaxima.value.tokens
          const costMax = trendScaleMaxima.value.cost
          const ratio = tokenMax > 0 ? tokenValue / tokenMax : 0
          const costValue = ratio * costMax
          return [formatTokens(tokenValue), formatCurrency(costValue)]
        },
      },
    },
    y2: {
      type: 'linear',
      display: false,
      position: 'right',
      min: 0,
      max: trendScaleMaxima.value.cost,
      beginAtZero: true,
      alignToPixels: true,
      bounds: 'ticks',
      grid: {
        drawOnChartArea: false,
      },
      ticks: {
        color: 'rgb(92, 98, 109)',
        count: trendAxisTickCount,
        callback: value => formatCurrency(Number(value)),
      },
    },
  },
  plugins: {
    legend: {
      labels: {
        usePointStyle: true,
      },
    },
    tooltip: {
      callbacks: {
        title: items => {
          const bucket = selectedTrendBuckets.value[items[0]?.dataIndex ?? -1]
          if (!bucket) return ''
          return formatAnalyticsTrendTooltipTitle(bucket.bucket_start, resolvedTrendGranularity.value)
        },
        label: context => {
          const value = Number(context.raw ?? 0)
          if (context.dataset.label === '总计 Tokens') {
            return `${context.dataset.label}: ${formatTokens(value)}`
          }
          if (context.dataset.label === '费用' || context.dataset.label === '成本') {
            return `${context.dataset.label}: ${formatCurrency(value)}`
          }
          return `${context.dataset.label}: ${value.toLocaleString()}`
        },
      },
    },
  },
}))

function exportLeaderboard() {
  if (!leaderboard.value.length) return
  downloadCsvFromObjects(
    `leaderboard-${props.dimension}-${new Date().toISOString().slice(0, 10)}`,
    leaderboard.value as unknown as Record<string, unknown>[],
    [
      { key: 'rank', label: '排名' },
      { key: 'name', label: '名称' },
      { key: 'requests', label: '请求数' },
      { key: 'tokens', label: 'Tokens' },
      { key: 'cost', label: '费用' },
      { key: 'actualCost', label: '成本' },
    ],
  )
}
</script>
