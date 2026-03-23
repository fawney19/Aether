<template>
  <div class="space-y-6 px-4 sm:px-6 lg:px-0">
    <div class="flex flex-wrap items-center justify-between gap-3">
      <Badge
        :variant="isAdmin ? 'default' : 'secondary'"
        class="uppercase tracking-[0.35em]"
      >
        {{ isAdmin ? 'ADMIN ANALYTICS' : 'PERSONAL ANALYTICS' }}
      </Badge>

      <div class="flex flex-wrap items-center gap-3">
        <TimeRangePicker
          v-model="timeRange"
          :show-granularity="false"
          :include-auto-granularity="true"
          :compact="true"
        />
        <Select
          v-if="isAdmin && userOptions.length > 0"
          v-model="selectedUserValue"
        >
          <SelectTrigger class="h-8 w-full sm:w-40 text-xs border-border/60">
            <SelectValue placeholder="全部用户" />
          </SelectTrigger>
          <SelectContent class="w-[min(18rem,var(--radix-select-trigger-width))]">
            <SelectItem value="__all__">
              全部用户
            </SelectItem>
            <SelectItem
              v-for="option in userOptions"
              :key="option.value"
              :value="option.value"
              :text-value="option.label"
            >
              {{ option.label }}
            </SelectItem>
          </SelectContent>
        </Select>
        <Select
          v-if="apiKeyOptions.length > 0"
          v-model="selectedApiKeyValue"
        >
          <SelectTrigger class="h-8 w-full sm:w-36 text-xs border-border/60">
            <SelectValue placeholder="全部 Key" />
          </SelectTrigger>
          <SelectContent class="w-[min(18rem,var(--radix-select-trigger-width))]">
            <SelectItem value="__all__">
              全部 Key
            </SelectItem>
            <SelectItem
              v-for="option in apiKeyOptions"
              :key="option.value"
              :value="option.value"
              :text-value="option.label"
            >
              {{ option.label }}
            </SelectItem>
          </SelectContent>
        </Select>
      </div>
    </div>

    <section class="flex flex-col gap-4 lg:flex-row lg:items-start">
      <div ref="activityPanelRef" class="min-w-0 space-y-3 lg:flex-1">
        <div class="grid grid-cols-2 gap-3 sm:grid-cols-3 xl:grid-cols-6">
          <Card class="p-3">
            <div class="text-[10px] uppercase tracking-[0.2em] text-muted-foreground">
              总请求
            </div>
            <div class="mt-1.5 text-xl font-semibold">
              {{ analyticsUnavailable ? '不可用' : overview ? overview.requests_total.toLocaleString() : '--' }}
            </div>
          </Card>
          <Card class="p-3">
            <div class="text-[10px] uppercase tracking-[0.2em] text-muted-foreground">
              总计 Tokens
            </div>
            <div class="mt-1.5 text-xl font-semibold">
              {{ analyticsUnavailable ? '不可用' : overview ? formatTokens(overview.total_tokens) : '--' }}
            </div>
          </Card>
          <Card class="p-3">
            <div class="text-[10px] uppercase tracking-[0.2em] text-muted-foreground">
              总费用
            </div>
            <div class="mt-1.5 text-xl font-semibold">
              {{ analyticsUnavailable ? '不可用' : overview ? formatCurrency(overview.total_cost_usd) : '--' }}
            </div>
          </Card>
          <Card class="p-3">
            <div class="text-[10px] uppercase tracking-[0.2em] text-muted-foreground">
              缓存命中率
            </div>
            <div class="mt-1.5 text-xl font-semibold">
              {{ analyticsUnavailable ? '不可用' : overview ? `${overview.cache_hit_rate.toFixed(1)}%` : '--' }}
            </div>
          </Card>
          <Card class="p-3">
            <div class="text-[10px] uppercase tracking-[0.2em] text-muted-foreground">
              平均响应
            </div>
            <div class="mt-1.5 text-xl font-semibold">
              {{ analyticsUnavailable ? '不可用' : overview && overview.requests_total > 0 ? formatResponseTimeMs(overview.avg_response_time_ms) : '--' }}
            </div>
          </Card>
          <Card class="p-3">
            <div class="text-[10px] uppercase tracking-[0.2em] text-muted-foreground">
              TTFB
            </div>
            <div class="mt-1.5 text-xl font-semibold">
              {{ analyticsUnavailable ? '不可用' : overview && overview.avg_first_byte_time_ms > 0 ? formatResponseTimeMs(overview.avg_first_byte_time_ms) : '--' }}
            </div>
          </Card>
        </div>

        <ActivityHeatmapCard
          :data="heatmapData"
          :title="heatmapTitle"
          :is-loading="loadingHeatmap"
          :has-error="heatmapError"
          :min-height="120"
          :value-label="heatmapValueLabel"
          :actual-value-label="heatmapActualValueLabel"
        />
      </div>

      <div
        class="order-first flex min-h-0 flex-col lg:order-none lg:flex-[0_0_220px]"
        :style="announcementsContainerStyle"
      >
        <Card class="flex h-full min-h-0 flex-1 flex-col overflow-hidden p-3 max-h-[280px] lg:max-h-none">
          <div class="mb-2 flex flex-shrink-0 items-center justify-between">
            <h2 class="text-xs font-semibold">
              系统公告
            </h2>
            <Badge
              variant="outline"
              class="uppercase tracking-[0.3em] text-[9px] px-1.5 py-0"
            >
              Live
            </Badge>
          </div>

          <div
            v-if="loadingAnnouncements"
            class="flex flex-1 items-center justify-center text-xs text-muted-foreground"
          >
            加载中...
          </div>
          <div
            v-else-if="announcements.length === 0"
            class="flex flex-1 items-center justify-center text-xs text-muted-foreground"
          >
            暂无公告
          </div>
          <div
            v-else
            class="min-h-0 flex-1 space-y-2 overflow-auto pr-0.5"
          >
            <button
              v-for="item in announcements"
              :key="item.id"
              type="button"
              class="w-full rounded-lg border border-border/60 bg-muted/15 px-2.5 py-2 text-left transition-colors hover:bg-muted/30"
              @click="openAnnouncement(item)"
            >
              <div class="flex items-start justify-between gap-1.5">
                <div class="min-w-0 text-[12px] font-medium leading-snug line-clamp-2">
                  {{ item.title }}
                </div>
                <span
                  class="shrink-0 rounded-full px-1.5 py-0.5 text-[9px] font-medium leading-none"
                  :class="announcementTagClass(item.type)"
                >
                  {{ announcementTagLabel(item.type) }}
                </span>
              </div>
              <div class="mt-1 text-[10px] text-muted-foreground">
                {{ formatDate(item.created_at) }}
              </div>
            </button>
          </div>
        </Card>
      </div>
    </section>

    <section
      v-if="showBreakdownSection"
      class="grid gap-4"
      :class="isAdmin ? 'xl:auto-rows-fr xl:grid-cols-[220px_minmax(0,1fr)_220px_220px]' : 'lg:grid-cols-[220px_minmax(0,1fr)_220px]'"
    >
      <Card class="min-w-0 overflow-hidden p-3 xl:h-full">
        <h2 class="mb-2 text-xs font-semibold">
          {{ primaryBreakdownTitle }}
        </h2>
        <DoughnutChart
          v-if="primaryBreakdownChartData"
          :data="primaryBreakdownChartData"
          :height="breakdownChartHeight"
          :show-legend="false"
        />
        <div
          v-else
          class="flex items-center justify-center text-xs text-muted-foreground"
          :style="{ height: `${breakdownChartHeight}px` }"
        >
          {{ analyticsEmptyStateText }}
        </div>
      </Card>

      <Card
        v-if="compositionSegments.length > 0"
        class="min-w-0 xl:h-full"
        :class="isAdmin ? 'p-2.5' : 'p-3'"
      >
        <div class="flex items-center justify-between gap-3">
          <h2 :class="isAdmin ? 'text-xs font-semibold' : 'text-sm font-semibold'">
            用量构成
          </h2>
          <span :class="isAdmin ? 'text-[10px] text-muted-foreground' : 'text-[11px] text-muted-foreground'">
            {{ analyticsUnavailable ? '不可用' : overview ? formatTokens(overview.total_tokens) : '--' }}
          </span>
        </div>

        <div :class="isAdmin ? 'mt-2.5 h-2 overflow-hidden rounded-full bg-muted/60' : 'mt-3 h-2.5 overflow-hidden rounded-full bg-muted/60'">
          <div class="flex h-full w-full">
            <div
              v-for="(segment, index) in compositionSegments"
              :key="segment.key"
              class="h-full transition-all"
              :style="{
                width: `${segment.percentage}%`,
                backgroundColor: segment.color,
                borderTopLeftRadius: index === 0 ? '999px' : '0',
                borderBottomLeftRadius: index === 0 ? '999px' : '0',
                borderTopRightRadius: index === compositionSegments.length - 1 ? '999px' : '0',
                borderBottomRightRadius: index === compositionSegments.length - 1 ? '999px' : '0',
              }"
            />
          </div>
        </div>

        <div :class="isAdmin ? 'mt-2.5 grid grid-cols-1 gap-1.5 sm:grid-cols-2' : 'mt-3 grid grid-cols-2 gap-1.5'">
          <div
            v-for="segment in compositionSegments"
            :key="`${segment.key}-legend`"
            :class="isAdmin ? 'rounded-lg border border-border/60 bg-muted/15 px-2 py-1.5' : 'rounded-lg border border-border/60 bg-muted/15 px-2.5 py-1.5'"
          >
            <div class="flex items-center justify-between gap-2">
              <div class="flex min-w-0 items-center gap-1.5">
                <span
                  class="h-2 w-2 shrink-0 rounded-full"
                  :style="{ backgroundColor: segment.color }"
                />
                <span :class="isAdmin ? 'truncate text-[10px] text-muted-foreground' : 'truncate text-[11px] text-muted-foreground'">
                  {{ segment.label }}
                </span>
              </div>
              <span :class="isAdmin ? 'text-[10px] font-medium' : 'text-[11px] font-medium'">
                {{ segment.percentage.toFixed(1) }}%
              </span>
            </div>
            <div :class="isAdmin ? 'mt-0.5 text-[11px] font-semibold' : 'mt-0.5 text-xs font-semibold'">
              {{ formatTokens(segment.value) }}
            </div>
          </div>
        </div>
      </Card>

      <Card
        v-else
        class="min-w-0 p-3 xl:h-full"
      >
        <div class="flex h-full items-center justify-center text-xs text-muted-foreground">
          {{ analyticsEmptyStateText }}
        </div>
      </Card>

      <Card class="min-w-0 overflow-hidden p-3 xl:h-full">
        <h3 class="mb-2 text-xs font-semibold">
          {{ secondaryBreakdownTitle }}
        </h3>
        <DoughnutChart
          v-if="secondaryBreakdownChartData"
          :data="secondaryBreakdownChartData"
          :height="breakdownChartHeight"
          :show-legend="false"
        />
        <div
          v-else
          class="flex items-center justify-center text-xs text-muted-foreground"
          :style="{ height: `${breakdownChartHeight}px` }"
        >
          {{ analyticsEmptyStateText }}
        </div>
      </Card>

      <Card
        v-if="isAdmin"
        class="min-w-0 overflow-hidden p-3 xl:h-full"
      >
        <h3 class="mb-2 text-xs font-semibold">
          模型收入分布
        </h3>
        <DoughnutChart
          v-if="tertiaryBreakdownChartData"
          :data="tertiaryBreakdownChartData"
          :height="breakdownChartHeight"
          :show-legend="false"
        />
        <div
          v-else
          class="flex items-center justify-center text-xs text-muted-foreground"
          :style="{ height: `${breakdownChartHeight}px` }"
        >
          {{ analyticsEmptyStateText }}
        </div>
      </Card>
    </section>

    <!-- Announcement detail dialog -->
    <Teleport to="body">
      <Transition name="fade">
        <div
          v-if="activeAnnouncement"
          class="fixed inset-0 z-50 flex items-center justify-center p-4"
        >
          <div
            class="absolute inset-0 bg-black/40 backdrop-blur-sm"
            @click="activeAnnouncement = null"
          />
          <div class="relative w-full max-w-lg rounded-xl border border-border bg-background p-5 shadow-xl">
            <div class="flex items-start justify-between gap-3">
              <h3 class="text-base font-semibold leading-snug">
                {{ activeAnnouncement.title }}
              </h3>
              <span
                class="shrink-0 rounded-full px-2 py-0.5 text-[10px] font-medium"
                :class="announcementTagClass(activeAnnouncement.type)"
              >
                {{ announcementTagLabel(activeAnnouncement.type) }}
              </span>
            </div>
            <div class="mt-1.5 text-xs text-muted-foreground">
              {{ formatDate(activeAnnouncement.created_at) }}
            </div>
            <p class="mt-4 whitespace-pre-line text-sm leading-relaxed">
              {{ activeAnnouncement.content }}
            </p>
            <div class="mt-5 flex justify-end">
              <button
                type="button"
                class="rounded-lg border border-border px-4 py-1.5 text-xs transition-colors hover:bg-muted/40"
                @click="activeAnnouncement = null"
              >
                关闭
              </button>
            </div>
          </div>
        </div>
      </Transition>
    </Teleport>

    <section>
      <Card class="min-w-0 p-4">
        <div class="mb-3 flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
          <div>
            <h2 class="text-sm font-semibold">
              趋势
            </h2>
            <p class="mt-1 text-[11px] text-muted-foreground">
              {{ trendChartDescription }}
            </p>
          </div>
          <Tabs
            v-model="currentTrendGranularitySelection"
            class="self-start"
          >
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
          </Tabs>
        </div>
        <LineChart
          v-if="trendChartData"
          :data="trendChartData"
          :options="trendChartOptions"
          :height="300"
        />
        <div
          v-else
          class="flex h-[300px] items-center justify-center text-xs text-muted-foreground"
        >
          {{ analyticsEmptyStateText }}
        </div>
      </Card>
    </section>
  </div>
</template>

<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { useAuthStore } from '@/stores/auth'
import { analyticsApi, type AnalyticsBreakdownMetric, type AnalyticsBreakdownRow, type AnalyticsFilterOption, type AnalyticsOverviewResponse, type AnalyticsTimeseriesBucket } from '@/api/analytics'
import { announcementApi, type Announcement } from '@/api/announcements'
import { getDateRangeFromPeriod } from '@/features/usage/composables'
import type { DateRangeParams } from '@/features/usage/types'
import { buildTimeRangeParams, createLoader, createRequestGuard } from '@/composables/useAnalyticsFilters'
import { Badge, Card, Tabs, TabsList, TabsTrigger, Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui'
import { TimeRangePicker } from '@/components/common'
import LineChart from '@/components/charts/LineChart.vue'
import DoughnutChart from '@/components/charts/DoughnutChart.vue'
import { ActivityHeatmapCard } from '@/features/usage/components'
import { formatCurrency, formatDate, formatTokens } from '@/utils/format'
import {
  analyticsGranularityTabs,
  formatAnalyticsTrendBucketLabel,
  formatAnalyticsTrendDescription,
  formatAnalyticsTrendTooltipTitle,
  isAnalyticsSingleDayRange,
  resolveAnalyticsAutoGranularity,
  type AnalyticsGranularityOption,
} from '@/utils/analyticsGranularity'
import { fillMissingTimeseriesBuckets } from '@/utils/analyticsTimeseries'
import type { ChartData, ChartOptions } from 'chart.js'
import type { ActivityHeatmap } from '@/types/activity'

const authStore = useAuthStore()
const isAdmin = computed(() => authStore.user?.role === 'admin')
const timeRange = ref<DateRangeParams>({ ...getDateRangeFromPeriod('today'), granularity: 'auto' })
const userFilter = ref<string[]>([])
const apiKeyFilter = ref<string[]>([])
const userOptions = ref<AnalyticsFilterOption[]>([])
const apiKeyOptions = ref<AnalyticsFilterOption[]>([])

const overviewData = ref<AnalyticsOverviewResponse | null>(null)
const trendBuckets = ref<AnalyticsTimeseriesBucket[]>([])
const primaryBreakdownRows = ref<AnalyticsBreakdownRow[]>([])
const secondaryBreakdownRows = ref<AnalyticsBreakdownRow[]>([])
const tertiaryBreakdownRows = ref<AnalyticsBreakdownRow[]>([])
const announcements = ref<Announcement[]>([])
const activeAnnouncement = ref<Announcement | null>(null)
const heatmapData = ref<ActivityHeatmap | null>(null)

const loading = ref(false)
const hasLoadedAnalytics = ref(false)
const analyticsError = ref(false)
const loadingAnnouncements = ref(false)
const loadingHeatmap = ref(false)
const heatmapError = ref(false)
const guard = createRequestGuard()
const filterOptionsGuard = createRequestGuard()
const heatmapGuard = createRequestGuard()
let suppressNextUserFilterLoad = false
let suppressNextApiKeyFilterLoad = false

const activityPanelRef = ref<HTMLElement | null>(null)
const announcementsHeight = ref<number | null>(null)
const isLargeScreen = ref(false)

const announcementsContainerStyle = computed(() => {
  if (!isLargeScreen.value || !announcementsHeight.value) return {}
  return { height: `${announcementsHeight.value}px` }
})

const ANNOUNCEMENT_TAG_MAP: Record<string, { label: string; class: string }> = {
  important: { label: '重要', class: 'bg-red-100 text-red-700 dark:bg-red-900/40 dark:text-red-400' },
  warning: { label: '警告', class: 'bg-amber-100 text-amber-700 dark:bg-amber-900/40 dark:text-amber-400' },
  update: { label: '更新', class: 'bg-blue-100 text-blue-700 dark:bg-blue-900/40 dark:text-blue-400' },
  maintenance: { label: '维护', class: 'bg-orange-100 text-orange-700 dark:bg-orange-900/40 dark:text-orange-400' },
  info: { label: '通知', class: 'bg-slate-100 text-slate-600 dark:bg-slate-800/60 dark:text-slate-400' },
}

function openAnnouncement(item: Announcement) {
  activeAnnouncement.value = item
}

function announcementTagLabel(type?: string): string {
  return ANNOUNCEMENT_TAG_MAP[type || 'info']?.label ?? '通知'
}

function announcementTagClass(type?: string): string {
  return ANNOUNCEMENT_TAG_MAP[type || 'info']?.class ?? ANNOUNCEMENT_TAG_MAP.info.class
}

function checkScreenSize() {
  if (typeof window !== 'undefined') {
    isLargeScreen.value = window.innerWidth >= 1024
  }
}

let activityPanelObserver: ResizeObserver | null = null

function updateAnnouncementsHeight() {
  if (typeof window === 'undefined') return
  const panel = activityPanelRef.value
  if (!panel) return
  const { height } = panel.getBoundingClientRect()
  if (height <= 0) return
  announcementsHeight.value = Math.round(height)
}

function handleWindowResize() {
  checkScreenSize()
  updateAnnouncementsHeight()
}

function setupResizeObserver() {
  if (typeof window === 'undefined') return
  const panel = activityPanelRef.value
  if (!panel || !('ResizeObserver' in window)) return
  activityPanelObserver = new ResizeObserver(() => updateAnnouncementsHeight())
  activityPanelObserver.observe(panel)
  updateAnnouncementsHeight()
}

const CHART_COLORS = [
  'rgb(220, 96, 108)',
  'rgb(232, 145, 89)',
  'rgb(214, 179, 83)',
  'rgb(113, 183, 125)',
  'rgb(96, 176, 192)',
  'rgb(103, 149, 230)',
  'rgb(146, 126, 214)',
]

const overview = computed(() => overviewData.value?.summary ?? null)
const analyticsUnavailable = computed(() => analyticsError.value && !hasLoadedAnalytics.value)
const analyticsEmptyStateText = computed(() => (
  analyticsUnavailable.value
    ? '数据暂不可用'
    : loading.value
      ? '加载中...'
      : '暂无数据'
))

const compositionSegments = computed(() => {
  const segments = overviewData.value?.composition.token_segments ?? []
  return segments
    .filter(segment => segment.value > 0)
    .map(segment => ({
      ...segment,
      label: segment.key === 'input'
        ? '输入'
        : segment.key === 'output'
          ? '输出'
          : segment.key === 'cache_creation'
            ? '缓存创建'
            : '缓存读取',
      color: segment.key === 'input'
        ? 'rgb(103, 149, 230)'
        : segment.key === 'output'
          ? 'rgb(146, 126, 214)'
          : segment.key === 'cache_creation'
            ? 'rgb(232, 145, 89)'
            : 'rgb(113, 183, 125)',
    }))
})

const selectedUserValue = computed({
  get: () => userFilter.value[0] ?? '__all__',
  set: (value: string) => {
    userFilter.value = value && value !== '__all__' ? [value] : []
  },
})

const selectedApiKeyValue = computed({
  get: () => apiKeyFilter.value[0] ?? '__all__',
  set: (value: string) => {
    apiKeyFilter.value = value && value !== '__all__' ? [value] : []
  },
})

const selectedUserLabel = computed(() => (
  userOptions.value.find(option => option.value === userFilter.value[0])?.label ?? null
))

const heatmapTitle = computed(() => {
  if (!isAdmin.value) return '我的活跃'
  return selectedUserLabel.value ? `${selectedUserLabel.value} 活跃` : '整体活跃'
})

const heatmapValueLabel = computed(() => '费用')
const heatmapActualValueLabel = computed(() => (isAdmin.value ? '成本' : undefined))

const dashboardFilters = computed(() => ({
  user_ids: isAdmin.value ? userFilter.value : [],
  api_key_ids: apiKeyFilter.value,
}))

function areStringArraysEqual(left: string[], right: string[]): boolean {
  if (left.length !== right.length) return false
  return left.every((value, index) => value === right[index])
}

function buildBreakdownChartData(
  rows: AnalyticsBreakdownRow[],
  metric: AnalyticsBreakdownMetric = 'total_cost_usd',
): ChartData<'doughnut'> | null {
  if (!rows.length) return null
  const values = rows.map(row => Number(row[metric] ?? 0))
  if (values.every(value => value <= 0)) return null
  return {
    labels: rows.map(row => row.label),
    datasets: [{
      data: values,
      backgroundColor: rows.map((_, index) => CHART_COLORS[index % CHART_COLORS.length]),
      borderColor: 'rgba(255,255,255,0.96)',
      borderWidth: 2,
    }],
  }
}

const primaryBreakdownTitle = computed(() => isAdmin.value ? '提供商成本分布' : '按模型费用构成')
const secondaryBreakdownTitle = computed(() => isAdmin.value ? '用户收入分布' : '按 API Key 费用构成')
const breakdownChartHeight = computed(() => isLargeScreen.value ? (isAdmin.value ? 132 : 140) : 176)
const trendAxisTickCount = 6

const currentTrendGranularitySelection = computed<AnalyticsGranularityOption>({
  get: () => timeRange.value.granularity || 'auto',
  set: value => setTrendGranularity(value),
})

const trendGranularityTabStates = computed(() => (
  analyticsGranularityTabs.map(option => ({
    ...option,
    disabled: option.value === 'hour' ? !isAnalyticsSingleDayRange(timeRange.value) : false,
  }))
))

function setTrendGranularity(granularity: AnalyticsGranularityOption) {
  if (granularity === 'hour' && !isAnalyticsSingleDayRange(timeRange.value)) {
    return
  }
  timeRange.value = {
    ...timeRange.value,
    granularity,
  }
}

watch(
  () => [timeRange.value.preset, timeRange.value.start_date, timeRange.value.end_date],
  () => {
    if (timeRange.value.granularity === 'hour' && !isAnalyticsSingleDayRange(timeRange.value)) {
      timeRange.value = {
        ...timeRange.value,
        granularity: 'auto',
      }
    }
  },
)

const trendGranularity = computed(() => resolveAnalyticsAutoGranularity(timeRange.value))

const trendTimeRangePayload = computed(() => ({
  ...buildTimeRangeParams(timeRange.value),
  granularity: trendGranularity.value,
}))

const trendChartDescription = computed(() => (
  formatAnalyticsTrendDescription(trendGranularity.value, '请求量、费用与总计 Tokens')
))

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

const primaryBreakdownChartData = computed<ChartData<'doughnut'> | null>(() => (
  buildBreakdownChartData(
    primaryBreakdownRows.value,
    isAdmin.value ? 'actual_total_cost_usd' : 'total_cost_usd',
  )
))

const secondaryBreakdownChartData = computed<ChartData<'doughnut'> | null>(() => (
  buildBreakdownChartData(secondaryBreakdownRows.value, 'total_cost_usd')
))

const tertiaryBreakdownChartData = computed<ChartData<'doughnut'> | null>(() => (
  buildBreakdownChartData(tertiaryBreakdownRows.value, 'total_cost_usd')
))

const showBreakdownSection = computed(() => (
  loading.value
  || analyticsUnavailable.value
  || compositionSegments.value.length > 0
  || Boolean(primaryBreakdownChartData.value)
  || Boolean(secondaryBreakdownChartData.value)
  || Boolean(tertiaryBreakdownChartData.value)
))

const trendScaleMaxima = computed(() => {
  const intervals = Math.max(trendAxisTickCount - 1, 1)
  const requestMax = Math.max(0, ...trendBuckets.value.map(bucket => bucket.requests_total))
  const tokenMax = Math.max(0, ...trendBuckets.value.map(bucket => bucket.total_tokens))
  const costMax = Math.max(0, ...trendBuckets.value.map(bucket => bucket.total_cost_usd))

  return {
    requests: computeNiceAxisMax(requestMax, intervals),
    tokens: computeNiceAxisMax(tokenMax, intervals),
    cost: computeNiceAxisMax(costMax, intervals),
  }
})

const trendChartData = computed<ChartData<'line'> | null>(() => {
  if (!trendBuckets.value.length) return null
  return {
    labels: trendBuckets.value.map(bucket => (
      formatAnalyticsTrendBucketLabel(bucket.bucket_start, trendGranularity.value)
    )),
    datasets: [
      {
        label: '费用',
        data: trendBuckets.value.map(bucket => bucket.total_cost_usd),
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
        label: '总计 Tokens',
        data: trendBuckets.value.map(bucket => bucket.total_tokens),
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
        data: trendBuckets.value.map(bucket => bucket.requests_total),
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
        maxTicksLimit: trendGranularity.value === 'hour' ? 12 : undefined,
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
          const bucket = trendBuckets.value[items[0]?.dataIndex ?? -1]
          if (!bucket) return ''
          return formatAnalyticsTrendTooltipTitle(bucket.bucket_start, trendGranularity.value)
        },
        label: context => {
          const value = Number(context.raw ?? 0)
          if (context.dataset.label === '总计 Tokens') {
            return `${context.dataset.label}: ${formatTokens(value)}`
          }
          if (context.dataset.label === '费用') {
            return `${context.dataset.label}: ${formatCurrency(value)}`
          }
          return `${context.dataset.label}: ${value.toLocaleString()}`
        },
      },
    },
  },
}))

function formatResponseTimeMs(value: number | null | undefined) {
  if (value == null || Number.isNaN(value)) return '-'
  if (value < 1000) return `${Math.round(value)}ms`
  return `${(value / 1000).toFixed(2)}s`
}

async function loadAnalytics() {
  const requestId = guard.next()
  loading.value = true
  analyticsError.value = false
  try {
    const timeRangePayload = buildTimeRangeParams(timeRange.value)
    const trendPayload = trendTimeRangePayload.value
    const scope = isAdmin.value ? { kind: 'global' as const } : { kind: 'me' as const }
    const basePayload = {
      scope,
      time_range: timeRangePayload,
      filters: dashboardFilters.value,
    }

    const primaryBreakdownPromise = analyticsApi.getBreakdown({
      ...basePayload,
      dimension: isAdmin.value ? 'provider' : 'model',
      metric: isAdmin.value ? 'actual_total_cost_usd' : 'total_cost_usd',
      limit: 8,
    })
    const secondaryBreakdownPromise = analyticsApi.getBreakdown({
      ...basePayload,
      dimension: isAdmin.value ? 'user' : 'api_key',
      metric: 'total_cost_usd',
      limit: 8,
    })
    const tertiaryBreakdownPromise = isAdmin.value
      ? analyticsApi.getBreakdown({
          ...basePayload,
          dimension: 'model',
          metric: 'total_cost_usd',
          limit: 8,
        })
      : Promise.resolve(null)

    const [overviewResponse, timeseriesResponse, primaryBreakdownResponse, secondaryBreakdownResponse, tertiaryBreakdownResponse] = await Promise.all([
      analyticsApi.getOverview(basePayload),
      analyticsApi.getTimeseries({
        ...basePayload,
        time_range: trendPayload,
      }),
      primaryBreakdownPromise,
      secondaryBreakdownPromise,
      tertiaryBreakdownPromise,
    ])

    if (guard.isStale(requestId)) return
    const filledTrendBuckets = fillMissingTimeseriesBuckets(
      timeseriesResponse.buckets,
      trendPayload,
    )
    overviewData.value = overviewResponse
    trendBuckets.value = filledTrendBuckets
    primaryBreakdownRows.value = primaryBreakdownResponse.rows
    secondaryBreakdownRows.value = secondaryBreakdownResponse.rows
    tertiaryBreakdownRows.value = tertiaryBreakdownResponse?.rows ?? []
    hasLoadedAnalytics.value = true
  } catch {
    if (guard.isStale(requestId)) return
    analyticsError.value = true
  } finally {
    if (guard.isCurrent(requestId)) {
      loading.value = false
    }
  }
}

async function loadFilterOptions() {
  const requestId = filterOptionsGuard.next()
  const filterOptions = await analyticsApi.getFilterOptions({
    scope: isAdmin.value ? { kind: 'global' } : { kind: 'me' },
    time_range: buildTimeRangeParams(timeRange.value),
    filters: dashboardFilters.value,
  }).catch(() => null)
  if (filterOptionsGuard.isStale(requestId)) return

  const nextUserOptions = isAdmin.value ? (filterOptions?.users ?? []) : []
  const nextApiKeyOptions = filterOptions?.api_keys ?? []
  userOptions.value = nextUserOptions
  apiKeyOptions.value = nextApiKeyOptions

  const validUserValues = new Set(nextUserOptions.map(option => option.value))
  const nextUserFilter = userFilter.value.filter(value => validUserValues.has(value))
  if (!areStringArraysEqual(nextUserFilter, userFilter.value)) {
    suppressNextUserFilterLoad = true
    userFilter.value = nextUserFilter
  }

  const validApiKeyValues = new Set(nextApiKeyOptions.map(option => option.value))
  const nextApiKeyFilter = apiKeyFilter.value.filter(value => validApiKeyValues.has(value))
  if (!areStringArraysEqual(nextApiKeyFilter, apiKeyFilter.value)) {
    suppressNextApiKeyFilterLoad = true
    apiKeyFilter.value = nextApiKeyFilter
  }
}

async function loadAnnouncements() {
  loadingAnnouncements.value = true
  try {
    const result = await announcementApi.getActiveAnnouncements()
    announcements.value = result.items.slice(0, 6)
  } catch {
    announcements.value = []
  } finally {
    loadingAnnouncements.value = false
  }
}

async function loadHeatmap() {
  const requestId = heatmapGuard.next()
  loadingHeatmap.value = true
  heatmapError.value = false
  try {
    const nextHeatmapData = await analyticsApi.getHeatmap({
      scope: isAdmin.value ? { kind: 'global' } : { kind: 'me' },
      user_id: isAdmin.value ? (userFilter.value[0] ?? null) : null,
      api_key_id: isAdmin.value ? (apiKeyFilter.value[0] ?? null) : null,
    })
    if (heatmapGuard.isStale(requestId)) return
    heatmapData.value = nextHeatmapData
  } catch {
    if (heatmapGuard.isStale(requestId)) return
    heatmapError.value = true
    heatmapData.value = null
  } finally {
    if (heatmapGuard.isCurrent(requestId)) {
      loadingHeatmap.value = false
    }
  }
}

const loader = createLoader(loadAnalytics)
const filterOptionsLoader = createLoader(loadFilterOptions)
const heatmapLoader = createLoader(loadHeatmap)

watch(
  timeRange,
  () => {
    void (async () => {
      await filterOptionsLoader.execute()
      loader.schedule()
      if (isAdmin.value) {
        heatmapLoader.schedule()
      }
    })()
  },
  { deep: true },
)

watch(
  () => [userFilter.value.slice(), apiKeyFilter.value.slice()],
  () => {
    if (suppressNextUserFilterLoad || suppressNextApiKeyFilterLoad) {
      suppressNextUserFilterLoad = false
      suppressNextApiKeyFilterLoad = false
      return
    }
    void (async () => {
      await filterOptionsLoader.execute()
      loader.schedule()
      if (isAdmin.value) {
        heatmapLoader.schedule()
      }
    })()
  },
  { deep: true },
)

onMounted(() => {
  checkScreenSize()
  window.addEventListener('resize', handleWindowResize)
  void (async () => {
    await filterOptionsLoader.execute()
    await loader.execute()
    await heatmapLoader.execute()
  })()
  void loadAnnouncements()
  nextTick(() => setupResizeObserver())
})

onUnmounted(() => {
  loader.cleanup()
  filterOptionsLoader.cleanup()
  heatmapLoader.cleanup()
  guard.invalidate()
  filterOptionsGuard.invalidate()
  heatmapGuard.invalidate()
  window.removeEventListener('resize', handleWindowResize)
  if (activityPanelObserver) {
    activityPanelObserver.disconnect()
    activityPanelObserver = null
  }
})
</script>

<style scoped>
.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.15s ease;
}
.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}
</style>
