<template>
  <div class="space-y-5">
    <div class="space-y-3">
      <div class="flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between">
        <div>
          <h3 class="text-sm font-medium">
            系统资源监控
          </h3>
          <p class="mt-1 text-[11px] text-muted-foreground">
            6 项核心资源的实时压力指数，越满表示越紧张
          </p>
        </div>
        <div
          v-if="systemStatusTimestamp"
          class="inline-flex h-7 items-center rounded-lg border border-border/70 bg-background px-2.5 text-[10px] text-muted-foreground"
        >
          {{ systemStatusTimestamp }}
        </div>
      </div>

      <div class="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
        <Card
          v-for="item in systemMetricCards"
          :key="item.key"
          class="min-h-[148px] border-border/60 bg-card p-3.5"
          :class="{ 'opacity-60 transition-opacity': isRefreshing }"
        >
          <div class="flex items-start justify-between gap-2.5">
            <div class="min-w-0">
              <div class="text-[10px] font-medium uppercase tracking-[0.14em] text-muted-foreground/85">
                {{ item.label }}
              </div>
              <div class="mt-1.5 text-xl font-semibold tracking-tight text-foreground">
                {{ item.value }}
              </div>
              <div class="mt-1 line-clamp-2 text-[10px] text-muted-foreground">
                {{ item.detail }}
              </div>
            </div>

            <div
              class="flex h-8 w-8 shrink-0 items-center justify-center rounded-xl border"
              :class="item.iconClass"
            >
              <component :is="item.icon" class="h-3.5 w-3.5" />
            </div>
          </div>

          <div class="mt-2.5 space-y-2">
            <div class="flex items-center justify-between gap-2">
              <span class="min-w-0 truncate text-[10px] text-muted-foreground">
                {{ item.meta }}
              </span>
              <span
                class="inline-flex shrink-0 rounded-full border px-2 py-0.5 text-[10px] font-medium"
                :class="item.badgeClass"
              >
                {{ item.badge }}
              </span>
            </div>

            <div class="h-1.5 overflow-hidden rounded-full bg-muted/50">
              <div
                class="h-full rounded-full transition-all"
                :class="item.progressClass"
                :style="{ width: item.progressWidth }"
              />
            </div>

            <div class="flex items-center justify-between gap-2 text-[10px] text-muted-foreground">
              <span>{{ item.caption }}</span>
              <span class="tabular-nums">{{ item.captionValue }}</span>
            </div>
          </div>
        </Card>
      </div>
    </div>

    <div class="space-y-3">
      <div class="flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between">
        <div>
          <h3 class="text-sm font-medium">
            运行概览
          </h3>
          <p class="mt-1 text-[11px] text-muted-foreground">
            汇总健康状态、今日负载与当前风险信号
          </p>
        </div>
        <div
          v-if="isRefreshing"
          class="inline-flex h-7 items-center rounded-lg border border-border/70 bg-background px-2.5 text-[10px] text-muted-foreground"
        >
          更新中
        </div>
      </div>

      <div class="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
        <Card
          v-for="item in statusCards"
          :key="item.key"
          class="min-h-[138px] border-border/60 bg-card p-4"
          :class="{ 'opacity-60 transition-opacity': isRefreshing }"
        >
          <div class="flex items-start justify-between gap-3">
            <div class="min-w-0">
              <div class="text-[10px] font-medium uppercase tracking-[0.14em] text-muted-foreground/85">
                {{ item.label }}
              </div>
              <div class="mt-2 text-2xl font-semibold tracking-tight text-foreground">
                {{ item.value }}
              </div>
              <div class="mt-1.5 text-[11px] leading-relaxed text-muted-foreground">
                {{ item.detail }}
              </div>
            </div>

            <div
              class="flex h-10 w-10 shrink-0 items-center justify-center rounded-2xl border"
              :class="item.iconClass"
            >
              <component :is="item.icon" class="h-4 w-4" />
            </div>
          </div>

          <div class="mt-3 flex items-center justify-between gap-3 border-t border-border/50 pt-3">
            <span class="min-w-0 truncate text-[11px] text-muted-foreground">
              {{ item.caption }}
            </span>
            <span
              class="inline-flex rounded-full border px-2 py-0.5 text-[10px] font-medium"
              :class="item.badgeClass"
            >
              {{ item.badge }}
            </span>
          </div>
        </Card>
      </div>
    </div>

    <div class="grid gap-4 lg:grid-cols-2">
      <Card class="overflow-hidden border-border/60 bg-card p-4">
        <div :class="{ 'opacity-60 transition-opacity': isRefreshing }">
          <PercentileChart
            title="响应延迟百分位"
            subtitle="观察 P50 / P90 / P99 响应耗时的分布变化"
            :series="percentiles"
            mode="response"
            :loading="isInitialLoading"
            :unavailable="performanceUnavailable"
            unavailable-text="性能数据暂不可用"
          />
        </div>
      </Card>

      <Card class="overflow-hidden border-border/60 bg-card p-4">
        <div :class="{ 'opacity-60 transition-opacity': isRefreshing }">
          <PercentileChart
            title="首字节延迟百分位"
            subtitle="聚焦首包返回速度，判断链路是否出现抖动"
            :series="percentiles"
            mode="ttfb"
            :loading="isInitialLoading"
            :unavailable="performanceUnavailable"
            unavailable-text="性能数据暂不可用"
          />
        </div>
      </Card>

      <Card class="overflow-hidden border-border/60 bg-card p-4">
        <div :class="{ 'opacity-60 transition-opacity': isRefreshing }">
          <ErrorDistributionChart
            title="错误分布"
            subtitle="按错误类别观察主要失败来源"
            :distribution="errorDistribution"
            :loading="isInitialLoading"
            :unavailable="performanceUnavailable"
            unavailable-text="性能数据暂不可用"
          />
        </div>
      </Card>

      <Card class="overflow-hidden border-border/60 bg-card p-4">
        <div class="space-y-4" :class="{ 'opacity-60 transition-opacity': isRefreshing }">
          <div class="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
            <div>
              <h3 class="text-sm font-semibold">
                错误趋势
              </h3>
              <p class="mt-1 text-[11px] text-muted-foreground">
                观察错误量在当前时间范围内的波动与峰值
              </p>
            </div>

            <div
              v-if="!performanceUnavailable"
              class="flex flex-wrap gap-2"
            >
              <div class="rounded-full border border-rose-500/20 bg-rose-500/8 px-2.5 py-1 text-[10px] font-medium text-rose-700 dark:text-rose-300">
                总错误 {{ errorTotalCount }}
              </div>
              <div
                v-if="errorPeakLabel"
                class="rounded-full border border-border/70 bg-muted/20 px-2.5 py-1 text-[10px] font-medium text-muted-foreground"
              >
                峰值 {{ errorPeakLabel }}
              </div>
            </div>
          </div>

          <div
            v-if="isInitialLoading"
            class="p-6"
          >
            <LoadingState />
          </div>
          <div
            v-else-if="performanceUnavailable"
            class="flex h-[272px] items-center justify-center rounded-2xl border border-dashed border-border/70 bg-muted/10 text-xs text-muted-foreground"
          >
            性能数据暂不可用
          </div>
          <div
            v-else-if="errorTrendChartData"
            class="rounded-2xl border border-border/60 bg-background p-3"
          >
            <LineChart
              :data="errorTrendChartData"
              :options="errorTrendChartOptions"
              :height="248"
            />
          </div>
          <div
            v-else
            class="flex h-[272px] items-center justify-center rounded-2xl border border-dashed border-border/70 bg-muted/10 text-xs text-muted-foreground"
          >
            暂无错误趋势数据
          </div>
        </div>
      </Card>
    </div>

    <div>
      <div class="mb-3 flex items-center justify-between">
        <h3 class="text-sm font-medium">
          提供商健康度
        </h3>
        <div
          v-if="isRefreshing"
          class="inline-flex h-7 items-center rounded-lg border border-border/70 bg-background px-2.5 text-[10px] text-muted-foreground"
        >
          更新中
        </div>
      </div>
      <div
        v-if="performanceUnavailable"
        class="text-xs text-muted-foreground"
      >
        性能数据暂不可用
      </div>
      <div
        v-else-if="providerStatus.length === 0 && !loading"
        class="text-xs text-muted-foreground"
      >
        暂无提供商状态数据
      </div>
      <div class="grid grid-cols-1 gap-2.5 sm:grid-cols-2 lg:grid-cols-5">
        <Card
          v-for="provider in providerStatus"
          :key="provider.provider_name"
          class="space-y-2 border-border/60 bg-card p-3"
          :class="{ 'opacity-60 transition-opacity': isRefreshing }"
        >
          <div class="flex items-start justify-between gap-2">
            <div class="min-w-0">
              <div class="truncate text-xs font-semibold" :title="provider.provider_name">
                {{ provider.provider_name }}
              </div>
              <div class="mt-1 text-[10px] text-muted-foreground tabular-nums">
                {{ provider.requests_total?.toLocaleString() ?? 0 }} 请求
              </div>
            </div>
            <Badge
              :variant="provider.error_rate > 5 ? 'destructive' : provider.success_rate >= 98 ? 'success' : 'warning'"
              class="h-5 px-1.5 text-[9px]"
            >
              {{ provider.error_rate > 5 ? '异常' : provider.success_rate >= 98 ? '稳定' : '波动' }}
            </Badge>
          </div>
          <div class="grid grid-cols-2 gap-x-3 gap-y-2 text-[10px] text-muted-foreground">
            <div>
              成功率
              <div class="mt-0.5 font-semibold text-foreground tabular-nums">
                {{ provider.success_rate.toFixed(1) }}%
              </div>
            </div>
            <div>
              错误率
              <div class="mt-0.5 font-semibold text-foreground tabular-nums">
                {{ provider.error_rate.toFixed(1) }}%
              </div>
            </div>
            <div>
              响应
              <div class="mt-0.5 font-semibold text-foreground tabular-nums">
                {{ formatLatency(provider.avg_response_time_ms) }}
              </div>
            </div>
            <div>
              TTFB
              <div class="mt-0.5 font-semibold text-foreground tabular-nums">
                {{ formatLatency(provider.avg_first_byte_time_ms) }}
              </div>
            </div>
          </div>
        </Card>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, type Component } from 'vue'
import type { ChartData, ChartOptions } from 'chart.js'
import { Activity, AlertTriangle, Cpu, Database, Gauge, HardDrive, Key } from 'lucide-vue-next'
import type { MonitoringMetricStatus } from '@/api/audit'
import { Badge, Card } from '@/components/ui'
import { LoadingState } from '@/components/common'
import { PercentileChart, ErrorDistributionChart } from '@/components/stats'
import LineChart from '@/components/charts/LineChart.vue'
import { usePerformanceData } from '@/composables/analytics'
import { formatCurrency, formatTokens } from '@/utils/format'

const {
  latency,
  percentiles,
  errorDistribution,
  errorTrend,
  errorTotal: errorTotalCount,
  errorRate,
  providerStatus,
  healthSummary,
  systemStatus,
  loading,
  loadError,
  hasLoaded,
} = usePerformanceData()

const isInitialLoading = computed(() => loading.value && !hasLoaded.value)
const isRefreshing = computed(() => loading.value && hasLoaded.value)
const performanceUnavailable = computed(() => loadError.value && !hasLoaded.value)
const errorPeakPoint = computed(() => (
  errorTrend.value.slice().sort((left, right) => right.total - left.total)[0] ?? null
))
const errorPeakLabel = computed(() => (
  errorPeakPoint.value && errorPeakPoint.value.total > 0
    ? `${errorPeakPoint.value.date.slice(5)} · ${errorPeakPoint.value.total}`
    : null
))

function formatLatency(value: number) {
  if (!value || value <= 0) return '-'
  if (value < 1000) return `${Math.round(value)}ms`
  return `${(value / 1000).toFixed(2)}s`
}

function formatCount(value: number | null | undefined, ready: boolean) {
  return ready && value != null ? value.toLocaleString() : '--'
}

function formatRatio(active: number | null | undefined, total: number | null | undefined, ready: boolean) {
  return ready && active != null && total != null ? `${active}/${total}` : '--'
}

type StatusTone = 'success' | 'warning' | 'danger' | 'info' | 'neutral'

interface StatusCardItem {
  key: string
  label: string
  value: string
  detail: string
  caption: string
  badge: string
  badgeClass: string
  iconClass: string
  icon: Component
}

interface SystemMetricCardItem {
  key: string
  label: string
  value: string
  detail: string
  meta: string
  caption: string
  captionValue: string
  badge: string
  badgeClass: string
  iconClass: string
  progressClass: string
  progressWidth: string
  icon: Component
}

function toneStyle(tone: StatusTone) {
  switch (tone) {
    case 'success':
      return {
        badgeClass: 'border-emerald-500/20 bg-emerald-500/8 text-emerald-700 dark:text-emerald-300',
        iconClass: 'border-emerald-500/20 bg-emerald-500/8 text-emerald-700 dark:text-emerald-300',
      }
    case 'warning':
      return {
        badgeClass: 'border-amber-500/20 bg-amber-500/8 text-amber-700 dark:text-amber-300',
        iconClass: 'border-amber-500/20 bg-amber-500/8 text-amber-700 dark:text-amber-300',
      }
    case 'danger':
      return {
        badgeClass: 'border-rose-500/20 bg-rose-500/8 text-rose-700 dark:text-rose-300',
        iconClass: 'border-rose-500/20 bg-rose-500/8 text-rose-700 dark:text-rose-300',
      }
    case 'info':
      return {
        badgeClass: 'border-sky-500/20 bg-sky-500/8 text-sky-700 dark:text-sky-300',
        iconClass: 'border-sky-500/20 bg-sky-500/8 text-sky-700 dark:text-sky-300',
      }
    default:
      return {
        badgeClass: 'border-border/70 bg-muted/20 text-muted-foreground',
        iconClass: 'border-border/70 bg-muted/20 text-muted-foreground',
      }
  }
}

function toneProgressClass(tone: StatusTone) {
  switch (tone) {
    case 'success':
      return 'bg-emerald-500/80'
    case 'warning':
      return 'bg-amber-500/80'
    case 'danger':
      return 'bg-rose-500/80'
    case 'info':
      return 'bg-sky-500/80'
    default:
      return 'bg-muted-foreground/40'
  }
}

function healthTone(unhealthy: number, total: number): StatusTone {
  if (total <= 0) return 'neutral'
  if (unhealthy <= 0) return 'success'
  if (unhealthy / total < 0.25) return 'warning'
  return 'danger'
}

function availabilityTone(active: number, total: number): StatusTone {
  if (total <= 0) return 'neutral'
  if (active === total) return 'success'
  if (active > 0) return 'warning'
  return 'danger'
}

function latencyTone(avgMs: number): StatusTone {
  if (!avgMs || avgMs <= 0) return 'neutral'
  if (avgMs < 1500) return 'success'
  if (avgMs < 3000) return 'warning'
  return 'danger'
}

function monitoringTone(status: MonitoringMetricStatus | undefined): StatusTone {
  switch (status) {
    case 'ok':
      return 'success'
    case 'warning':
    case 'degraded':
      return 'warning'
    case 'danger':
    case 'error':
      return 'danger'
    default:
      return 'neutral'
  }
}

function formatPercent(value: number | null | undefined) {
  return value == null ? '--' : `${value.toFixed(1)}%`
}

function formatCompactCount(value: number | null | undefined) {
  return value == null ? '--' : value.toLocaleString()
}

function formatBytes(value: number | null | undefined) {
  if (value == null) return '--'
  if (value === 0) return '0 B'

  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  let current = value
  let unitIndex = 0

  while (current >= 1024 && unitIndex < units.length - 1) {
    current /= 1024
    unitIndex += 1
  }

  const digits = current >= 100 ? 0 : current >= 10 ? 1 : 2
  return `${current.toFixed(digits)} ${units[unitIndex]}`
}

function formatTimeLabel(timestamp: string | null | undefined) {
  if (!timestamp) return null
  return `更新 ${new Date(timestamp).toLocaleTimeString('zh-CN', {
    hour: '2-digit',
    minute: '2-digit',
  })}`
}

function clampPercent(value: number | null | undefined) {
  if (value == null) return null
  return Math.max(0, Math.min(100, value))
}

function progressWidth(value: number | null | undefined) {
  const safeValue = clampPercent(value)
  const width = safeValue == null ? 12 : Math.max(6, safeValue)
  return `${width}%`
}

function remainingToPressurePercent(value: number | null | undefined) {
  const safeValue = clampPercent(value)
  return safeValue == null ? null : Math.max(0, 100 - safeValue)
}

function formatPressureValue(value: number | null | undefined) {
  return formatPercent(clampPercent(value))
}

function latencyPressureValue(latencyMs: number | null | undefined, status: MonitoringMetricStatus | undefined) {
  if (latencyMs == null) {
    return statusPressureValue(status)
  }

  if (latencyMs >= 200) return 100
  if (latencyMs >= 120) return 88
  if (latencyMs >= 80) return 72
  if (latencyMs >= 40) return 34
  if (latencyMs >= 20) return 24
  return 16
}

function statusPressureValue(status: MonitoringMetricStatus | undefined) {
  switch (status) {
    case 'ok':
      return 16
    case 'warning':
      return 72
    case 'degraded':
      return 82
    case 'danger':
      return 92
    case 'error':
      return 100
    default:
      return null
  }
}

const PRESSURE_CAPTION = '压力指数'

function pressureMetric(value: number | null | undefined) {
  return clampPercent(value)
}

function storagePressureMetric(value: number | null | undefined) {
  return remainingToPressurePercent(value)
}

function fallbackPressureWidth() {
  return progressWidth(null)
}

function fallbackPressureCaptionValue() {
  return '--'
}

const statusCards = computed<StatusCardItem[]>(() => {
  if (performanceUnavailable.value) {
    return [
      { key: 'endpoint-health', label: 'Endpoint 健康', icon: Database },
      { key: 'key-health', label: 'Key 健康', icon: Key },
      { key: 'provider-status', label: 'Provider 在线', icon: Cpu },
      { key: 'today-traffic', label: '今日流量', icon: Activity },
      { key: 'recent-alerts', label: '近 1h 告警', icon: AlertTriangle },
      { key: 'latency', label: '平均延迟', icon: Gauge },
    ].map(item => ({
      ...item,
      value: '不可用',
      detail: '性能接口未返回结果',
      caption: '请稍后重试',
      badge: '不可用',
      ...toneStyle('neutral'),
    }))
  }

  const healthReady = healthSummary.value !== null
  const systemReady = systemStatus.value !== null

  const endpointTotal = healthSummary.value?.endpoints.total ?? 0
  const endpointActive = healthSummary.value?.endpoints.active ?? 0
  const endpointUnhealthy = healthSummary.value?.endpoints.unhealthy ?? 0
  const endpointTone = toneStyle(healthTone(endpointUnhealthy, endpointTotal))

  const keyTotal = healthSummary.value?.keys.total ?? systemStatus.value?.api_keys.total ?? 0
  const keyActive = healthSummary.value?.keys.active ?? systemStatus.value?.api_keys.active ?? 0
  const keyUnhealthy = healthSummary.value?.keys.unhealthy ?? Math.max(keyTotal - keyActive, 0)
  const keyCircuitOpen = healthSummary.value?.keys.circuit_open ?? 0
  const keyTone = toneStyle(healthTone(keyUnhealthy, keyTotal))

  const providerTotal = systemStatus.value?.providers.total ?? 0
  const providerActive = systemStatus.value?.providers.active ?? 0
  const userActive = systemStatus.value?.users.active ?? 0
  const userTotal = systemStatus.value?.users.total ?? 0
  const providerTone = toneStyle(availabilityTone(providerActive, providerTotal))

  const todayRequests = systemStatus.value?.today_stats.requests ?? 0
  const todayTokens = systemStatus.value?.today_stats.tokens ?? 0
  const todayCost = systemStatus.value?.today_stats.cost_usd ?? 0
  const throughputTone = toneStyle(todayRequests > 0 ? 'info' : 'neutral')

  const recentAlerts = systemStatus.value?.recent_errors ?? 0
  const alertsTone = toneStyle(recentAlerts <= 0 && errorRate.value < 2 ? 'success' : recentAlerts < 5 && errorRate.value < 5 ? 'warning' : 'danger')

  const responseAvg = latency.value.response_time_ms.avg ?? 0
  const ttfbAvg = latency.value.first_byte_time_ms.avg ?? 0
  const latencyStyle = toneStyle(latencyTone(responseAvg))

  return [
    {
      key: 'endpoint-health',
      label: 'Endpoint 健康',
      value: formatRatio(endpointActive, endpointTotal, healthReady),
      detail: healthReady ? `异常端点 ${endpointUnhealthy}` : '健康摘要载入中',
      caption: healthReady ? '当前可用端点覆盖' : '等待健康摘要返回',
      badge: healthReady ? (endpointUnhealthy > 0 ? '关注中' : '稳定') : '载入中',
      ...endpointTone,
      icon: Database,
    },
    {
      key: 'key-health',
      label: 'Key 健康',
      value: formatRatio(keyActive, keyTotal, healthReady || systemReady),
      detail: healthReady ? `异常 Key ${keyUnhealthy} · 熔断 ${keyCircuitOpen}` : '健康摘要载入中',
      caption: healthReady ? '密钥池可用状态' : '等待健康摘要返回',
      badge: healthReady ? (keyUnhealthy > 0 || keyCircuitOpen > 0 ? '波动' : '健康') : '载入中',
      ...keyTone,
      icon: Key,
    },
    {
      key: 'provider-status',
      label: 'Provider 在线',
      value: formatRatio(providerActive, providerTotal, systemReady),
      detail: systemReady ? `活跃用户 ${userActive}/${userTotal}` : '系统状态载入中',
      caption: systemReady ? '提供商接入状态' : '等待系统状态返回',
      badge: systemReady ? (providerActive === providerTotal ? '在线' : '需关注') : '载入中',
      ...providerTone,
      icon: Cpu,
    },
    {
      key: 'today-traffic',
      label: '今日流量',
      value: formatCount(todayRequests, systemReady),
      detail: systemReady ? `Tokens ${formatTokens(todayTokens)}` : '系统状态载入中',
      caption: systemReady ? `成本 ${formatCurrency(todayCost)}` : '等待系统状态返回',
      badge: systemReady ? (todayRequests > 0 ? '活跃' : '空闲') : '载入中',
      ...throughputTone,
      icon: Activity,
    },
    {
      key: 'recent-alerts',
      label: '近 1h 告警',
      value: formatCount(recentAlerts, systemReady),
      detail: `当前范围失败 ${errorTotalCount.value.toLocaleString()}`,
      caption: `错误率 ${errorRate.value.toFixed(1)}%`,
      badge: systemReady
        ? recentAlerts <= 0 && errorRate.value < 2 ? '低风险' : recentAlerts < 5 && errorRate.value < 5 ? '需关注' : '偏高'
        : '载入中',
      ...alertsTone,
      icon: AlertTriangle,
    },
    {
      key: 'latency-baseline',
      label: '延迟基线',
      value: formatLatency(responseAvg),
      detail: `TTFB ${formatLatency(ttfbAvg)}`,
      caption: `P50 ${formatLatency(latency.value.response_time_ms.p50 ?? 0)}`,
      badge: responseAvg > 0 ? (responseAvg < 1500 ? '顺畅' : responseAvg < 3000 ? '一般' : '偏慢') : '无数据',
      ...latencyStyle,
      icon: Gauge,
    },
  ]
})

const systemStatusTimestamp = computed(() => formatTimeLabel(systemStatus.value?.timestamp))

const systemMetricCards = computed<SystemMetricCardItem[]>(() => {
  const metrics = systemStatus.value?.system_metrics

  if (!metrics) {
    const detail = systemStatus.value ? 'system_metrics 未返回' : '系统状态载入中'
    const tone = toneStyle('neutral')
    const progressClass = toneProgressClass('neutral')
    return [
      { key: 'cpu', label: 'CPU', icon: Cpu },
      { key: 'memory', label: '内存', icon: HardDrive },
      { key: 'redis-latency', label: 'Redis', icon: Activity },
      { key: 'redis-memory', label: 'Redis 内存', icon: HardDrive },
      { key: 'postgres-pool', label: 'PostgreSQL', icon: Database },
      { key: 'postgres-storage', label: 'PostgreSQL 空间', icon: HardDrive },
    ].map(item => ({
      ...item,
      value: '--',
      detail,
      meta: '等待监控数据',
      caption: PRESSURE_CAPTION,
      captionValue: fallbackPressureCaptionValue(),
      badge: '未知',
      progressWidth: fallbackPressureWidth(),
      progressClass,
      ...tone,
    }))
  }

  const cpuTone = monitoringTone(metrics.cpu.status)
  const memoryTone = monitoringTone(metrics.memory.status)
  const redisTone = monitoringTone(metrics.redis.status)
  const redisMemoryTone = monitoringTone(metrics.redis.memory_status ?? metrics.redis.status)
  const postgresTone = monitoringTone(metrics.postgres.status)
  const postgresStorageTone = monitoringTone(metrics.postgres.storage_status ?? metrics.postgres.status)
  const redisMemoryCeiling = metrics.redis.memory_ceiling_bytes ?? metrics.redis.maxmemory_bytes ?? null
  const redisMemorySource = metrics.redis.memory_source ?? 'unknown'
  const redisMemorySourceLabel = redisMemorySource === 'configured'
    ? '手动容量'
    : redisMemorySource === 'maxmemory'
        ? '上限'
      : redisMemorySource === 'system'
        ? '估算上限'
        : '未配置容量'
  const cpuPressure = pressureMetric(metrics.cpu.usage_percent ?? metrics.cpu.load_percent)
  const memoryPressure = pressureMetric(metrics.memory.used_percent)
  const redisHealthPressure = latencyPressureValue(metrics.redis.latency_ms, metrics.redis.status)
  const redisMemoryPressure = pressureMetric(metrics.redis.memory_percent ?? statusPressureValue(metrics.redis.memory_status))
  const postgresPoolPressure = pressureMetric(
    metrics.postgres.usage_percent ?? metrics.postgres.pool_usage_percent ?? metrics.postgres.server_usage_percent,
  )
  const postgresStoragePressure = storagePressureMetric(metrics.postgres.storage_free_percent)

  return [
    {
      key: 'cpu',
      label: 'CPU',
      value: formatPercent(metrics.cpu.usage_percent ?? metrics.cpu.load_percent),
      detail: metrics.cpu.usage_percent != null
        ? `实时占用 · Load ${formatPercent(metrics.cpu.load_percent)}`
        : (metrics.cpu.message ?? 'CPU 监控数据不可用'),
      meta: metrics.cpu.core_count > 0 ? `${metrics.cpu.core_count} 核` : '核心数未知',
      caption: PRESSURE_CAPTION,
      captionValue: formatPressureValue(cpuPressure),
      badge: metrics.cpu.label,
      progressWidth: progressWidth(cpuPressure),
      progressClass: toneProgressClass(cpuTone),
      ...toneStyle(cpuTone),
      icon: Cpu,
    },
    {
      key: 'memory',
      label: '内存',
      value: formatPercent(metrics.memory.used_percent),
      detail: metrics.memory.total_bytes != null
        ? `${formatBytes(metrics.memory.used_bytes)} / ${formatBytes(metrics.memory.total_bytes)}`
        : (metrics.memory.message ?? '内存监控数据不可用'),
      meta: metrics.memory.available_bytes != null ? `可用 ${formatBytes(metrics.memory.available_bytes)}` : '等待系统指标',
      caption: PRESSURE_CAPTION,
      captionValue: formatPressureValue(memoryPressure),
      badge: metrics.memory.label,
      progressWidth: progressWidth(memoryPressure),
      progressClass: toneProgressClass(memoryTone),
      ...toneStyle(memoryTone),
      icon: HardDrive,
    },
    {
      key: 'redis-latency',
      label: 'Redis',
      value: metrics.redis.latency_ms != null ? formatLatency(metrics.redis.latency_ms) : metrics.redis.label,
      detail: metrics.redis.message ?? '缓存链路实时探测',
      meta: metrics.redis.latency_ms != null ? 'PING 延迟' : '连接状态',
      caption: PRESSURE_CAPTION,
      captionValue: formatPressureValue(redisHealthPressure),
      badge: metrics.redis.label,
      progressWidth: progressWidth(redisHealthPressure),
      progressClass: toneProgressClass(redisTone),
      ...toneStyle(redisTone),
      icon: Activity,
    },
    {
      key: 'redis-memory',
      label: 'Redis 内存',
      value: formatBytes(metrics.redis.used_memory_bytes),
      detail: metrics.redis.peak_memory_bytes != null
        ? `峰值 ${formatBytes(metrics.redis.peak_memory_bytes)}`
        : (redisMemorySource === 'unknown' ? '可在系统设置中手动填写 Redis 总内存' : '等待 Redis 内存指标'),
      meta: redisMemoryCeiling != null
        ? `${redisMemorySourceLabel} ${formatBytes(redisMemoryCeiling)}`
        : '未提供内存上限',
      caption: PRESSURE_CAPTION,
      captionValue: formatPressureValue(redisMemoryPressure),
      badge: metrics.redis.memory_label ?? metrics.redis.label,
      progressWidth: progressWidth(redisMemoryPressure),
      progressClass: toneProgressClass(redisMemoryTone),
      ...toneStyle(redisMemoryTone),
      icon: HardDrive,
    },
    {
      key: 'postgres-pool',
      label: 'PostgreSQL',
      value: formatPercent(metrics.postgres.usage_percent),
      detail: metrics.postgres.message
        ?? (metrics.postgres.server_connections != null && metrics.postgres.server_max_connections != null
          ? `服务端 ${formatCompactCount(metrics.postgres.server_connections)} / ${formatCompactCount(metrics.postgres.server_max_connections)} 连接`
          : `${formatCompactCount(metrics.postgres.checked_out)} / ${formatCompactCount(metrics.postgres.max_capacity)} 连接占用`),
      meta: metrics.postgres.server_connections != null && metrics.postgres.server_max_connections != null
        ? `应用池 ${formatCompactCount(metrics.postgres.checked_out)} / ${formatCompactCount(metrics.postgres.max_capacity)}`
        : `池 ${formatCompactCount(metrics.postgres.pool_size)} · 溢出 ${formatCompactCount(metrics.postgres.overflow)}`,
      caption: PRESSURE_CAPTION,
      captionValue: formatPressureValue(postgresPoolPressure),
      badge: metrics.postgres.label,
      progressWidth: progressWidth(postgresPoolPressure),
      progressClass: toneProgressClass(postgresTone),
      ...toneStyle(postgresTone),
      icon: Database,
    },
    {
      key: 'postgres-storage',
      label: 'PostgreSQL 空间',
      value: formatBytes(metrics.postgres.storage_free_bytes),
      detail: metrics.postgres.database_size_bytes != null
        ? `库体积 ${formatBytes(metrics.postgres.database_size_bytes)}`
        : (metrics.postgres.storage_message ?? '等待 PostgreSQL 空间指标'),
      meta: metrics.postgres.storage_total_bytes != null
        ? `总容量 ${formatBytes(metrics.postgres.storage_total_bytes)}`
        : (metrics.postgres.storage_message ?? '数据目录总量未知'),
      caption: PRESSURE_CAPTION,
      captionValue: formatPressureValue(postgresStoragePressure),
      badge: metrics.postgres.storage_label ?? metrics.postgres.label,
      progressWidth: progressWidth(postgresStoragePressure),
      progressClass: toneProgressClass(postgresStorageTone),
      ...toneStyle(postgresStorageTone),
      icon: HardDrive,
    },
  ]
})

const errorTrendChartData = computed<ChartData<'line'> | null>(() => {
  if (!errorTrend.value.length) return null
  return {
    labels: errorTrend.value.map(d => d.date?.slice(5) ?? ''),
    datasets: [{
      label: '错误数',
      data: errorTrend.value.map(d => d.total),
      borderColor: 'rgb(226, 85, 118)',
      backgroundColor: 'rgba(226, 85, 118, 0.16)',
      fill: true,
      tension: 0.34,
      pointRadius: 0,
      pointHoverRadius: 4,
      pointHitRadius: 14,
      borderWidth: 2.4,
    }],
  }
})

const errorTrendChartOptions = computed<ChartOptions<'line'>>(() => ({
  responsive: true,
  maintainAspectRatio: false,
  interaction: {
    mode: 'index',
    intersect: false,
  },
  plugins: {
    legend: {
      display: false,
    },
    tooltip: {
      callbacks: {
        title: items => errorTrend.value[items[0]?.dataIndex ?? -1]?.date ?? '',
        label: context => `错误数: ${Number(context.raw ?? 0).toLocaleString()}`,
      },
    },
  },
  scales: {
    x: {
      grid: {
        color: 'rgba(148, 163, 184, 0.06)',
      },
      ticks: {
        color: 'rgb(100, 116, 139)',
        maxRotation: 0,
      },
    },
    y: {
      beginAtZero: true,
      grid: {
        color: 'rgba(148, 163, 184, 0.07)',
      },
      ticks: {
        color: 'rgb(100, 116, 139)',
        precision: 0,
      },
    },
  },
}))
</script>
