<template>
  <div class="space-y-6 px-4 sm:px-6 lg:px-0">
    <div class="flex flex-col gap-3 xl:flex-row xl:items-center xl:justify-between">
      <div>
        <h1 class="text-lg font-semibold">
          性能分析
        </h1>
        <p class="text-xs text-muted-foreground">
          实时性能监控与历史延迟趋势
        </p>
      </div>
      <div class="flex flex-wrap items-center gap-2">
        <Badge variant="outline">
          实时 10s 刷新
        </Badge>
        <span class="text-xs text-muted-foreground">
          上次更新 {{ liveLastUpdatedLabel }}
        </span>
        <RefreshButton
          :loading="isRefreshing"
          title="刷新实时与历史性能数据"
          @click="handleManualRefresh"
        />
        <TimeRangePicker v-model="timeRange" />
      </div>
    </div>

    <Card class="overflow-hidden">
      <div class="border-b border-border/70 bg-muted/20 px-4 py-3 sm:px-5">
        <div class="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
          <div>
            <h2 class="text-sm font-semibold">
              实时性能面板
            </h2>
            <p class="text-xs text-muted-foreground">
              聚合系统状态、并发门、Tunnel 与 fallback 指标
            </p>
          </div>
          <div class="flex flex-wrap items-center gap-2">
            <Badge :variant="healthStatusVariant">
              {{ healthStatusText }}
            </Badge>
            <Badge variant="outline">
              {{ metricsAvailabilityText }}
            </Badge>
          </div>
        </div>
      </div>

      <div class="p-4 sm:p-5">
        <div
          v-if="liveLoading && !liveReady"
          class="py-6"
        >
          <LoadingState message="加载实时性能数据中" />
        </div>

        <div
          v-else-if="!liveReady"
          class="rounded-xl border border-dashed border-border/70 bg-muted/15 px-4 py-6 text-sm text-muted-foreground"
        >
          实时性能数据暂不可用，请稍后重试。
        </div>

        <div
          v-else
          class="space-y-4"
        >
          <div
            v-if="liveLoadError"
            class="rounded-lg border border-yellow-300/70 bg-yellow-50/80 px-3 py-2 text-xs text-yellow-900 dark:border-yellow-900/60 dark:bg-yellow-950/30 dark:text-yellow-100"
          >
            {{ liveLoadError }}
          </div>

          <div class="grid grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-3 2xl:grid-cols-6">
            <div
              v-for="card in liveSummaryCards"
              :key="card.title"
              class="rounded-xl border border-border/70 bg-card/70 px-4 py-3"
            >
              <div class="flex items-center justify-between gap-3">
                <span class="text-xs text-muted-foreground">{{ card.title }}</span>
                <component
                  :is="card.icon"
                  class="h-4 w-4"
                  :class="card.iconClass"
                />
              </div>
              <div class="mt-3 text-2xl font-semibold tracking-tight">
                {{ card.value }}
              </div>
              <div class="mt-2 text-xs text-muted-foreground">
                {{ card.hint }}
              </div>
            </div>
          </div>

          <div class="grid grid-cols-1 gap-4 xl:grid-cols-3">
            <div class="grid grid-cols-1 gap-4 lg:grid-cols-2 xl:col-span-2">
              <section class="rounded-xl border border-border/70 bg-card/60 p-4">
                <div class="flex items-center justify-between gap-3">
                  <h3 class="text-sm font-semibold">
                    本地并发门
                  </h3>
                  <Badge variant="outline">
                    gateway_requests
                  </Badge>
                </div>
                <div class="mt-4 grid grid-cols-2 gap-3 text-sm">
                  <div>
                    <div class="text-xs text-muted-foreground">
                      In Flight
                    </div>
                    <div class="mt-1 text-lg font-semibold">
                      {{ formatMetricNumber(gatewayMetrics?.local.inFlight) }}
                    </div>
                  </div>
                  <div>
                    <div class="text-xs text-muted-foreground">
                      Available
                    </div>
                    <div class="mt-1 text-lg font-semibold">
                      {{ formatMetricNumber(gatewayMetrics?.local.availablePermits) }}
                    </div>
                  </div>
                  <div>
                    <div class="text-xs text-muted-foreground">
                      High Watermark
                    </div>
                    <div class="mt-1 text-lg font-semibold">
                      {{ formatMetricNumber(gatewayMetrics?.local.highWatermark) }}
                    </div>
                  </div>
                  <div>
                    <div class="text-xs text-muted-foreground">
                      Rejected Total
                    </div>
                    <div class="mt-1 text-lg font-semibold">
                      {{ formatMetricNumber(gatewayMetrics?.local.rejectedTotal) }}
                    </div>
                  </div>
                </div>
              </section>

              <section class="rounded-xl border border-border/70 bg-card/60 p-4">
                <div class="flex items-center justify-between gap-3">
                  <h3 class="text-sm font-semibold">
                    分布式并发门
                  </h3>
                  <Badge :variant="distributedGateVariant">
                    {{ distributedGateText }}
                  </Badge>
                </div>
                <div class="mt-4 grid grid-cols-2 gap-3 text-sm">
                  <div>
                    <div class="text-xs text-muted-foreground">
                      In Flight
                    </div>
                    <div class="mt-1 text-lg font-semibold">
                      {{ formatMetricNumber(gatewayMetrics?.distributed.inFlight) }}
                    </div>
                  </div>
                  <div>
                    <div class="text-xs text-muted-foreground">
                      Available
                    </div>
                    <div class="mt-1 text-lg font-semibold">
                      {{ formatMetricNumber(gatewayMetrics?.distributed.availablePermits) }}
                    </div>
                  </div>
                  <div>
                    <div class="text-xs text-muted-foreground">
                      High Watermark
                    </div>
                    <div class="mt-1 text-lg font-semibold">
                      {{ formatMetricNumber(gatewayMetrics?.distributed.highWatermark) }}
                    </div>
                  </div>
                  <div>
                    <div class="text-xs text-muted-foreground">
                      Rejected Total
                    </div>
                    <div class="mt-1 text-lg font-semibold">
                      {{ formatMetricNumber(gatewayMetrics?.distributed.rejectedTotal) }}
                    </div>
                  </div>
                </div>
                <p
                  v-if="gatewayMetrics?.distributed.unavailable"
                  class="mt-3 text-xs text-yellow-700 dark:text-yellow-300"
                >
                  Redis 分布式并发快照当前不可用，需要检查 gate 后端连接。
                </p>
              </section>

              <section class="rounded-xl border border-border/70 bg-card/60 p-4">
                <div class="flex items-center justify-between gap-3">
                  <h3 class="text-sm font-semibold">
                    Tunnel / 代理
                  </h3>
                  <Badge variant="outline">
                    实时连接
                  </Badge>
                </div>
                <div class="mt-4 grid grid-cols-2 gap-3 text-sm">
                  <div>
                    <div class="text-xs text-muted-foreground">
                      Nodes
                    </div>
                    <div class="mt-1 text-lg font-semibold">
                      {{ formatMetricNumber(currentTunnelNodes) }}
                    </div>
                  </div>
                  <div>
                    <div class="text-xs text-muted-foreground">
                      Proxy Connections
                    </div>
                    <div class="mt-1 text-lg font-semibold">
                      {{ formatMetricNumber(currentProxyConnections) }}
                    </div>
                  </div>
                  <div>
                    <div class="text-xs text-muted-foreground">
                      Active Streams
                    </div>
                    <div class="mt-1 text-lg font-semibold">
                      {{ formatMetricNumber(currentActiveStreams) }}
                    </div>
                  </div>
                  <div>
                    <div class="text-xs text-muted-foreground">
                      Service Up
                    </div>
                    <div class="mt-1 text-lg font-semibold">
                      {{ gatewayMetrics?.serviceUp === 1 ? '在线' : '未知' }}
                    </div>
                  </div>
                </div>
              </section>

              <section class="rounded-xl border border-border/70 bg-card/60 p-4">
                <div class="flex items-center justify-between gap-3">
                  <h3 class="text-sm font-semibold">
                    今日请求概况
                  </h3>
                  <Badge variant="outline">
                    {{ systemStatus?.internal_gateway.status || 'gateway' }}
                  </Badge>
                </div>
                <div class="mt-4 grid grid-cols-2 gap-3 text-sm">
                  <div>
                    <div class="text-xs text-muted-foreground">
                      Requests
                    </div>
                    <div class="mt-1 text-lg font-semibold">
                      {{ formatMetricNumber(systemStatus?.today_stats.requests) }}
                    </div>
                  </div>
                  <div>
                    <div class="text-xs text-muted-foreground">
                      Tokens
                    </div>
                    <div class="mt-1 text-lg font-semibold">
                      {{ formatTokens(systemStatus?.today_stats.tokens) }}
                    </div>
                  </div>
                  <div>
                    <div class="text-xs text-muted-foreground">
                      Cost
                    </div>
                    <div class="mt-1 text-lg font-semibold">
                      {{ systemStatus?.today_stats.cost_usd || '-' }}
                    </div>
                  </div>
                  <div>
                    <div class="text-xs text-muted-foreground">
                      Active Providers / Keys
                    </div>
                    <div class="mt-1 text-lg font-semibold">
                      {{ providerAndKeySummary }}
                    </div>
                  </div>
                </div>
              </section>
            </div>

            <section class="rounded-xl border border-border/70 bg-card/60 p-4">
              <div class="flex items-center justify-between gap-3">
                <h3 class="text-sm font-semibold">
                  Fallback 统计
                </h3>
                <span class="text-xs text-muted-foreground">
                  总计 {{ formatMetricNumber(gatewayMetrics?.fallbackTotal) }}
                </span>
              </div>

              <div
                v-if="!fallbackRows.length"
                class="mt-4 rounded-lg border border-dashed border-border/70 px-3 py-4 text-sm text-muted-foreground"
              >
                当前没有记录到 fallback 计数。
              </div>

              <div
                v-else
                class="mt-4 space-y-3"
              >
                <div
                  v-for="item in fallbackRows"
                  :key="item.name"
                  class="rounded-lg border border-border/60 bg-background/50 px-3 py-3"
                >
                  <div class="flex items-center justify-between gap-3">
                    <span class="text-sm font-medium">{{ item.label }}</span>
                    <span class="text-sm font-semibold">{{ formatMetricNumber(item.total) }}</span>
                  </div>
                  <div class="mt-2 h-2 overflow-hidden rounded-full bg-muted">
                    <div
                      class="h-full rounded-full bg-primary/80"
                      :style="{ width: `${Math.max(item.ratio, 8)}%` }"
                    />
                  </div>
                </div>
              </div>
            </section>
          </div>

          <div class="grid grid-cols-1 gap-4 xl:grid-cols-2">
            <section class="rounded-xl border border-border/70 bg-card/60 p-4">
              <div class="flex items-center justify-between gap-3">
                <h3 class="text-sm font-semibold">
                  最近错误
                </h3>
                <span class="text-xs text-muted-foreground">
                  {{ formatMetricNumber(resilienceStatus?.error_statistics.total_errors) }} / 24h
                </span>
              </div>

              <div
                v-if="!recentErrors.length"
                class="mt-4 rounded-lg border border-dashed border-border/70 px-3 py-4 text-sm text-muted-foreground"
              >
                当前没有最近错误。
              </div>

              <div
                v-else
                class="mt-4 space-y-3"
              >
                <article
                  v-for="item in recentErrors"
                  :key="item.error_id"
                  class="rounded-lg border border-border/60 bg-background/50 px-3 py-3"
                >
                  <div class="flex items-start justify-between gap-4">
                    <div>
                      <div class="text-sm font-medium">
                        {{ item.error_type }}
                      </div>
                      <div class="mt-1 text-xs text-muted-foreground">
                        {{ item.operation }}
                      </div>
                    </div>
                    <span class="shrink-0 text-xs text-muted-foreground">
                      {{ formatDate(item.timestamp) }}
                    </span>
                  </div>

                  <div class="mt-2 flex flex-wrap gap-2">
                    <Badge variant="outline">
                      HTTP {{ item.context.status_code ?? '-' }}
                    </Badge>
                    <Badge variant="outline">
                      {{ item.context.provider_name || item.context.provider_id || '未知 Provider' }}
                    </Badge>
                    <Badge variant="outline">
                      {{ item.context.api_format || item.context.model || '未知格式' }}
                    </Badge>
                  </div>

                  <p
                    v-if="item.context.error_message"
                    class="mt-2 break-words text-xs text-muted-foreground"
                  >
                    {{ item.context.error_message }}
                  </p>
                </article>
              </div>
            </section>

            <section class="rounded-xl border border-border/70 bg-card/60 p-4">
              <div class="flex items-center justify-between gap-3">
                <h3 class="text-sm font-semibold">
                  熔断历史与建议
                </h3>
                <span class="text-xs text-muted-foreground">
                  开路 {{ formatMetricNumber(resilienceStatus?.error_statistics.open_circuit_breakers) }}
                </span>
              </div>

              <div
                v-if="!circuitHistory.length"
                class="mt-4 rounded-lg border border-dashed border-border/70 px-3 py-4 text-sm text-muted-foreground"
              >
                当前没有熔断事件。
              </div>

              <div
                v-else
                class="mt-4 space-y-3"
              >
                <article
                  v-for="item in circuitHistory"
                  :key="`${item.key_id}-${item.api_format}-${item.timestamp}`"
                  class="rounded-lg border border-border/60 bg-background/50 px-3 py-3"
                >
                  <div class="flex items-start justify-between gap-4">
                    <div>
                      <div class="flex flex-wrap items-center gap-2">
                        <span class="text-sm font-medium">
                          {{ item.provider_name || item.provider_id }}
                        </span>
                        <Badge :variant="item.event === 'opened' ? 'destructive' : 'warning'">
                          {{ item.event === 'opened' ? '已打开' : '半开' }}
                        </Badge>
                      </div>
                      <div class="mt-1 text-xs text-muted-foreground">
                        {{ item.key_name || item.key_id }} · {{ item.api_format || '未知格式' }}
                      </div>
                    </div>
                    <span class="shrink-0 text-xs text-muted-foreground">
                      {{ formatDate(item.timestamp) }}
                    </span>
                  </div>

                  <div class="mt-2 text-xs text-muted-foreground">
                    原因：{{ item.reason || '未提供' }}
                  </div>
                  <div class="mt-1 text-xs text-muted-foreground">
                    恢复窗口：{{ item.recovery_seconds != null ? `${item.recovery_seconds}s` : '-' }}
                  </div>
                </article>
              </div>

              <div class="mt-4 border-t border-border/70 pt-4">
                <h4 class="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                  建议
                </h4>
                <ul
                  v-if="resilienceRecommendations.length"
                  class="mt-3 space-y-2 text-sm"
                >
                  <li
                    v-for="item in resilienceRecommendations"
                    :key="item"
                    class="rounded-lg border border-border/60 bg-background/50 px-3 py-2"
                  >
                    {{ item }}
                  </li>
                </ul>
                <div
                  v-else
                  class="mt-3 text-sm text-muted-foreground"
                >
                  当前没有额外运维建议。
                </div>
              </div>
            </section>
          </div>
        </div>
      </div>
    </Card>

    <div class="grid grid-cols-1 gap-4 lg:grid-cols-2">
      <Card class="p-4">
        <PercentileChart
          title="响应延迟百分位"
          :series="percentiles"
          mode="response"
          :loading="percentileLoading"
        />
      </Card>
      <Card class="p-4">
        <PercentileChart
          title="首字节延迟百分位"
          :series="percentiles"
          mode="ttfb"
          :loading="percentileLoading"
        />
      </Card>
    </div>

    <div class="grid grid-cols-1 gap-4 lg:grid-cols-2">
      <Card class="p-4">
        <ErrorDistributionChart
          title="错误分布"
          :distribution="errorDistribution"
          :loading="errorLoading"
        />
      </Card>
      <Card class="space-y-3 p-4">
        <h3 class="text-sm font-semibold">
          错误趋势
        </h3>
        <div
          v-if="errorLoading"
          class="p-6"
        >
          <LoadingState />
        </div>
        <div
          v-else
          class="h-[260px]"
        >
          <LineChart :data="errorTrendChartData" />
        </div>
      </Card>
    </div>

    <Card class="space-y-3 p-4">
      <h3 class="text-sm font-semibold">
        提供商健康度
      </h3>
      <div
        v-if="providerLoading"
        class="p-4"
      >
        <LoadingState />
      </div>
      <div
        v-else-if="providerStatus.length"
        class="grid grid-cols-1 gap-3 text-sm sm:grid-cols-2 lg:grid-cols-3"
      >
        <div
          v-for="provider in providerStatus"
          :key="provider.name"
          class="rounded-lg border p-3"
        >
          <div class="flex items-center justify-between">
            <span class="font-medium">{{ provider.name }}</span>
            <span class="text-xs text-muted-foreground">{{ provider.requests }} 请求</span>
          </div>
          <div class="mt-1 text-xs text-muted-foreground">
            状态: {{ provider.status }}
          </div>
        </div>
      </div>
      <div
        v-else
        class="rounded-lg border border-dashed border-border/70 px-3 py-4 text-sm text-muted-foreground"
      >
        当前没有提供商状态数据。
      </div>
    </Card>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import {
  Activity,
  AlertTriangle,
  Cable,
  GitBranch,
  ShieldCheck,
  Workflow,
} from 'lucide-vue-next'
import { adminApi, type ErrorDistributionResponse, type PercentileItem } from '@/api/admin'
import { dashboardApi, type ProviderStatus } from '@/api/dashboard'
import {
  monitoringApi,
  type AdminMonitoringCircuitHistoryItem,
  type AdminMonitoringResilienceStatus,
  type AdminMonitoringSystemStatus,
  type GatewayMetricsSummary,
} from '@/api/monitoring'
import LineChart from '@/components/charts/LineChart.vue'
import { LoadingState, TimeRangePicker } from '@/components/common'
import { ErrorDistributionChart, PercentileChart } from '@/components/stats'
import Badge from '@/components/ui/badge.vue'
import Card from '@/components/ui/card.vue'
import RefreshButton from '@/components/ui/refresh-button.vue'
import { useToast } from '@/composables/useToast'
import { getDateRangeFromPeriod } from '@/features/usage/composables'
import type { DateRangeParams } from '@/features/usage/types'
import { formatDate, formatNumber, formatTokens } from '@/utils/format'
import { log } from '@/utils/logger'

const LIVE_REFRESH_INTERVAL_MS = 10_000

const timeRange = ref<DateRangeParams>(getDateRangeFromPeriod('last30days'))
const { error: showError } = useToast()

const percentiles = ref<PercentileItem[]>([])
const percentileLoading = ref(false)

const errorDistribution = ref<ErrorDistributionResponse['distribution']>([])
const errorTrend = ref<ErrorDistributionResponse['trend']>([])
const errorLoading = ref(false)

const providerStatus = ref<ProviderStatus[]>([])
const providerLoading = ref(false)

const systemStatus = ref<AdminMonitoringSystemStatus | null>(null)
const resilienceStatus = ref<AdminMonitoringResilienceStatus | null>(null)
const circuitHistory = ref<AdminMonitoringCircuitHistoryItem[]>([])
const gatewayMetrics = ref<GatewayMetricsSummary | null>(null)
const liveLoading = ref(false)
const liveRefreshing = ref(false)
const liveReady = ref(false)
const liveLoadError = ref<string | null>(null)
const liveLastUpdatedAt = ref<string | null>(null)

let percentilesRequestId = 0
let errorsRequestId = 0
let providersRequestId = 0
let liveRequestId = 0
let loadAllPromise: Promise<void> | null = null
let hasPendingLoadAll = false
let loadAllDebounceTimer: ReturnType<typeof setTimeout> | null = null
let liveRefreshTimer: ReturnType<typeof setInterval> | null = null

function buildTimeRangeParams() {
  return {
    start_date: timeRange.value.start_date,
    end_date: timeRange.value.end_date,
    preset: timeRange.value.preset,
    timezone: timeRange.value.timezone,
    tz_offset_minutes: timeRange.value.tz_offset_minutes
  }
}

function formatMetricNumber(value: number | null | undefined): string {
  if (value == null || Number.isNaN(value)) {
    return '-'
  }

  if (!Number.isInteger(value)) {
    return value.toFixed(2)
  }

  return formatNumber(value)
}

async function loadPercentiles() {
  const requestId = ++percentilesRequestId
  percentileLoading.value = true
  try {
    const data = await adminApi.getPercentiles(buildTimeRangeParams())
    if (requestId !== percentilesRequestId) return
    percentiles.value = data
  } catch (error) {
    if (requestId !== percentilesRequestId) return
    percentiles.value = []
    log.error('加载延迟百分位失败', error)
  } finally {
    if (requestId === percentilesRequestId) {
      percentileLoading.value = false
    }
  }
}

async function loadErrors() {
  const requestId = ++errorsRequestId
  errorLoading.value = true
  try {
    const response = await adminApi.getErrorDistribution(buildTimeRangeParams())
    if (requestId !== errorsRequestId) return
    errorDistribution.value = response.distribution
    errorTrend.value = response.trend
  } catch (error) {
    if (requestId !== errorsRequestId) return
    errorDistribution.value = []
    errorTrend.value = []
    log.error('加载错误分布失败', error)
  } finally {
    if (requestId === errorsRequestId) {
      errorLoading.value = false
    }
  }
}

async function loadProviders() {
  const requestId = ++providersRequestId
  providerLoading.value = true
  try {
    const data = await dashboardApi.getProviderStatus()
    if (requestId !== providersRequestId) return
    providerStatus.value = data
  } catch (error) {
    if (requestId !== providersRequestId) return
    providerStatus.value = []
    log.error('加载提供商状态失败', error)
  } finally {
    if (requestId === providersRequestId) {
      providerLoading.value = false
    }
  }
}

async function loadLiveData(options: { silent?: boolean } = {}) {
  const requestId = ++liveRequestId
  const initialLoad = !liveReady.value

  if (initialLoad) {
    liveLoading.value = true
  } else {
    liveRefreshing.value = true
  }

  const results = await Promise.allSettled([
    monitoringApi.getSystemStatus(),
    monitoringApi.getResilienceStatus(),
    monitoringApi.getCircuitHistory(8),
    monitoringApi.getGatewayMetricsSummary(),
  ])

  if (requestId !== liveRequestId) {
    return
  }

  const failedScopes: string[] = []
  let successCount = 0

  const [systemResult, resilienceResult, circuitResult, metricsResult] = results

  if (systemResult.status === 'fulfilled') {
    systemStatus.value = systemResult.value
    successCount += 1
  } else {
    failedScopes.push('系统状态')
    log.error('加载系统状态失败', systemResult.reason)
  }

  if (resilienceResult.status === 'fulfilled') {
    resilienceStatus.value = resilienceResult.value
    successCount += 1
  } else {
    failedScopes.push('韧性状态')
    log.error('加载韧性状态失败', resilienceResult.reason)
  }

  if (circuitResult.status === 'fulfilled') {
    circuitHistory.value = circuitResult.value.items
    successCount += 1
  } else {
    failedScopes.push('熔断历史')
    log.error('加载熔断历史失败', circuitResult.reason)
  }

  if (metricsResult.status === 'fulfilled') {
    gatewayMetrics.value = metricsResult.value
    successCount += 1
  } else {
    failedScopes.push('网关指标')
    log.error('加载网关指标失败', metricsResult.reason)
  }

  liveReady.value = successCount > 0
  if (successCount > 0) {
    liveLastUpdatedAt.value = new Date().toISOString()
  }
  liveLoadError.value = failedScopes.length
    ? `部分实时数据加载失败：${failedScopes.join('、')}`
    : null

  if (failedScopes.length && !options.silent) {
    showError(liveLoadError.value ?? '实时性能数据加载失败')
  }

  if (requestId === liveRequestId) {
    liveLoading.value = false
    liveRefreshing.value = false
  }
}

const errorTrendChartData = computed(() => ({
  labels: errorTrend.value.map(item => item.date),
  datasets: [
    {
      label: '错误数',
      data: errorTrend.value.map(item => item.total),
      borderColor: 'rgb(239, 68, 68)',
      tension: 0.25,
      pointRadius: 2
    }
  ]
}))

const recentErrors = computed(() => resilienceStatus.value?.recent_errors ?? [])
const resilienceRecommendations = computed(() => resilienceStatus.value?.recommendations ?? [])

const healthStatusVariant = computed<'success' | 'warning' | 'destructive' | 'outline'>(() => {
  switch (resilienceStatus.value?.status) {
    case 'healthy':
      return 'success'
    case 'degraded':
      return 'warning'
    case 'critical':
      return 'destructive'
    default:
      return 'outline'
  }
})

const healthStatusText = computed(() => {
  if (!resilienceStatus.value) {
    return '健康状态未知'
  }

  const statusMap: Record<string, string> = {
    healthy: '系统健康',
    degraded: '系统降级',
    critical: '系统告警',
  }

  return `${statusMap[resilienceStatus.value.status] ?? resilienceStatus.value.status} · ${resilienceStatus.value.health_score}/100`
})

const metricsAvailabilityText = computed(() => (
  gatewayMetrics.value ? 'Prometheus 在线' : 'Prometheus 暂不可达'
))

const distributedGateVariant = computed<'warning' | 'outline'>(() => (
  gatewayMetrics.value?.distributed.unavailable ? 'warning' : 'outline'
))

const distributedGateText = computed(() => (
  gatewayMetrics.value?.distributed.unavailable ? '不可用' : '在线'
))

const liveLastUpdatedLabel = computed(() => (
  liveLastUpdatedAt.value ? formatDate(liveLastUpdatedAt.value) : '尚未刷新'
))

const currentActiveStreams = computed(() => (
  gatewayMetrics.value?.tunnel.activeStreams ?? systemStatus.value?.tunnel.active_streams ?? null
))

const currentProxyConnections = computed(() => (
  gatewayMetrics.value?.tunnel.proxyConnections ?? systemStatus.value?.tunnel.proxy_connections ?? null
))

const currentTunnelNodes = computed(() => (
  gatewayMetrics.value?.tunnel.nodes ?? systemStatus.value?.tunnel.nodes ?? null
))

const providerAndKeySummary = computed(() => {
  if (!systemStatus.value) {
    return '-'
  }
  return `${systemStatus.value.providers.active}/${systemStatus.value.providers.total} · ${systemStatus.value.api_keys.active}/${systemStatus.value.api_keys.total}`
})

const fallbackRows = computed(() => {
  const items = gatewayMetrics.value?.fallbacks ?? []
  const maxValue = Math.max(...items.map(item => item.total), 0)

  return items
    .filter(item => item.total > 0)
    .sort((left, right) => right.total - left.total)
    .map(item => ({
      ...item,
      ratio: maxValue > 0 ? item.total / maxValue * 100 : 0,
    }))
})

const liveSummaryCards = computed(() => [
  {
    title: '系统健康',
    value: resilienceStatus.value ? `${resilienceStatus.value.health_score}/100` : '-',
    hint: `${healthStatusText.value} · 开路 ${formatMetricNumber(resilienceStatus.value?.error_statistics.open_circuit_breakers)}`,
    icon: ShieldCheck,
    iconClass: 'text-emerald-500',
  },
  {
    title: '最近 1 小时错误',
    value: formatMetricNumber(systemStatus.value?.recent_errors),
    hint: `24h 总错误 ${formatMetricNumber(resilienceStatus.value?.error_statistics.total_errors)}`,
    icon: AlertTriangle,
    iconClass: 'text-yellow-500',
  },
  {
    title: '当前活跃流',
    value: formatMetricNumber(currentActiveStreams.value),
    hint: `代理连接 ${formatMetricNumber(currentProxyConnections.value)}`,
    icon: Cable,
    iconClass: 'text-sky-500',
  },
  {
    title: '本地 In Flight',
    value: formatMetricNumber(gatewayMetrics.value?.local.inFlight),
    hint: `剩余 permit ${formatMetricNumber(gatewayMetrics.value?.local.availablePermits)}`,
    icon: Activity,
    iconClass: 'text-blue-500',
  },
  {
    title: '分布式 In Flight',
    value: gatewayMetrics.value?.distributed.unavailable
      ? '不可用'
      : formatMetricNumber(gatewayMetrics.value?.distributed.inFlight),
    hint: gatewayMetrics.value?.distributed.unavailable
      ? '检查 Redis gate 状态'
      : `剩余 permit ${formatMetricNumber(gatewayMetrics.value?.distributed.availablePermits)}`,
    icon: Workflow,
    iconClass: 'text-violet-500',
  },
  {
    title: 'Fallback 累计',
    value: formatMetricNumber(gatewayMetrics.value?.fallbackTotal),
    hint: `今日请求 ${formatMetricNumber(systemStatus.value?.today_stats.requests)}`,
    icon: GitBranch,
    iconClass: 'text-rose-500',
  },
])

const isRefreshing = computed(() => (
  liveLoading.value ||
  liveRefreshing.value ||
  percentileLoading.value ||
  errorLoading.value ||
  providerLoading.value
))

async function loadAll() {
  if (loadAllPromise) {
    hasPendingLoadAll = true
    return loadAllPromise
  }

  loadAllPromise = Promise.all([loadPercentiles(), loadErrors(), loadProviders()])
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

async function handleManualRefresh() {
  await Promise.allSettled([loadLiveData(), loadAll()])
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

watch(timeRange, scheduleLoadAll, { deep: true })

onMounted(() => {
  void loadLiveData()
  void loadAll()
  liveRefreshTimer = setInterval(() => {
    void loadLiveData({ silent: true })
  }, LIVE_REFRESH_INTERVAL_MS)
})

onUnmounted(() => {
  if (loadAllDebounceTimer) {
    clearTimeout(loadAllDebounceTimer)
    loadAllDebounceTimer = null
  }

  if (liveRefreshTimer) {
    clearInterval(liveRefreshTimer)
    liveRefreshTimer = null
  }

  hasPendingLoadAll = false
  loadAllPromise = null
  percentilesRequestId += 1
  errorsRequestId += 1
  providersRequestId += 1
  liveRequestId += 1
})
</script>
