export type HealthBadgeVariant =
  | 'default'
  | 'secondary'
  | 'destructive'
  | 'outline'
  | 'success'
  | 'warning'
  | 'dark'

export type HealthMonitorSourceKind = 'endpoint' | 'model' | 'provider'

export interface HealthMonitorDetailSource {
  kind: HealthMonitorSourceKind
  value: string
  title: string
  metaText?: string | null
  totalAttempts: number
  slaEligibleCount?: number | null
  successCount: number
  failedCount: number
  serviceErrorCount?: number | null
  userErrorCount?: number | null
  successRate: number
  avgLatencyMs?: number | null
  avgFirstByteMs?: number | null
  avgTps?: number | null
  timeline?: string[] | null
  timelineDetails?: HealthTimelineTooltipMetrics[] | null
  timeRangeStart?: string | null
  timeRangeEnd?: string | null
}

export interface HealthMonitorDetailTarget {
  source: HealthMonitorDetailSource
  lookbackHours: number
}

export interface HealthMonitorAvailability {
  total_attempts: number
  sla_eligible_count?: number | null
  success_count?: number | null
  failed_count?: number | null
  service_error_count?: number | null
  user_error_count?: number | null
  success_rate: number
}

export interface HealthTimelineTooltipMetrics {
  time_range_start?: string | null
  time_range_end?: string | null
  total_attempts?: number | null
  sla_eligible_count?: number | null
  success_count?: number | null
  failed_count?: number | null
  service_error_count?: number | null
  user_error_count?: number | null
  success_rate?: number | null
  avg_latency_ms?: number | null
  avg_first_byte_ms?: number | null
  avg_tps?: number | null
}

export interface HealthMonitorSectionSummary {
  total: number
  healthy: number
  warning: number
  unhealthy: number
  empty: number
  attempts: number
}

export function summarizeHealthMonitorItems(
  items: HealthMonitorAvailability[]
): HealthMonitorSectionSummary {
  return items.reduce<HealthMonitorSectionSummary>((summary, item) => {
    summary.total += 1
    summary.attempts += item.total_attempts
    if (!hasSlaSamples(item)) {
      summary.empty += 1
    } else if (item.success_rate >= 0.95) {
      summary.healthy += 1
    } else if (item.success_rate >= 0.8) {
      summary.warning += 1
    } else {
      summary.unhealthy += 1
    }
    return summary
  }, createEmptyHealthMonitorSectionSummary())
}

export function createEmptyHealthMonitorSectionSummary(): HealthMonitorSectionSummary {
  return {
    total: 0,
    healthy: 0,
    warning: 0,
    unhealthy: 0,
    empty: 0,
    attempts: 0
  }
}

export function getHealthLabel(
  item: HealthMonitorAvailability,
  emptyLabel = '暂无请求'
) {
  if (!hasSlaSamples(item)) {
    return item.total_attempts > 0 ? '暂无 SLA 样本' : emptyLabel
  }
  if (item.success_rate >= 0.95) return '正常'
  if (item.success_rate >= 0.8) return '波动'
  return '异常'
}

export function getHealthBadgeVariant(
  item: HealthMonitorAvailability
): HealthBadgeVariant {
  if (!hasSlaSamples(item)) return 'outline'
  if (item.success_rate >= 0.95) return 'success'
  if (item.success_rate >= 0.8) return 'warning'
  return 'destructive'
}

export function getSuccessRateClass(rate: number) {
  if (rate >= 0.95) return 'text-green-600 dark:text-green-400'
  if (rate >= 0.8) return 'text-amber-600 dark:text-amber-400'
  return 'text-red-600 dark:text-red-400'
}

export function getAvailabilityClass(item: HealthMonitorAvailability) {
  if (!hasSlaSamples(item)) return ''
  return getSuccessRateClass(item.success_rate)
}

export function formatMs(value?: number | null) {
  if (typeof value !== 'number' || Number.isNaN(value)) return '-'
  const absValue = Math.abs(value)
  if (absValue < 1000) return `${Math.round(value)} ms`
  if (absValue < 60_000) return `${formatDurationNumber(value / 1000)} s`
  return `${formatDurationNumber(value / 60_000)} min`
}

function formatDurationNumber(value: number) {
  return new Intl.NumberFormat('zh-CN', {
    maximumFractionDigits: Math.abs(value) < 10 ? 2 : 1
  }).format(value)
}

export function formatPercent(value: number) {
  if (typeof value !== 'number' || Number.isNaN(value)) return '-'
  return `${(value * 100).toFixed(2)}%`
}

export function formatAvailability(item: HealthMonitorAvailability) {
  if (!hasSlaSamples(item)) return '-'
  return formatPercent(item.success_rate)
}

function hasSlaSamples(item: HealthMonitorAvailability): boolean {
  if (typeof item.sla_eligible_count === 'number' && Number.isFinite(item.sla_eligible_count)) {
    return item.sla_eligible_count > 0
  }
  return item.total_attempts > 0
}

export function formatTps(value?: number | null) {
  if (typeof value !== 'number' || Number.isNaN(value)) return '-'
  return `${new Intl.NumberFormat('zh-CN', {
    maximumFractionDigits: value < 10 ? 2 : value < 100 ? 1 : 0
  }).format(value)} tps`
}

export function formatFullTimestamp(timestamp?: string | null) {
  if (!timestamp) return '未知时间'
  const date = new Date(timestamp)
  if (Number.isNaN(date.getTime())) return '未知时间'
  const pad = (value: number) => value.toString().padStart(2, '0')
  return [
    `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`,
    `${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`
  ].join(' ')
}

export function formatTimelineTooltip(input: {
  status: string
  timeRangeStart: string
  timeRangeEnd: string
  metrics?: HealthTimelineTooltipMetrics | null
  entityLabel?: string
  entityName?: string | null
}) {
  const metrics = input.metrics
  const lines = [
    `总请求/SLA样本/成功/服务错误/用户错误/SLA可用率/状态：${formatTimelineRequestBreakdown(metrics, input.status)}`,
    `平均耗时/TTFB/速度：${formatTimelineAverageMetrics(metrics)}`,
    `时间范围：${formatFullTimestamp(input.timeRangeStart)} - ${formatFullTimestamp(input.timeRangeEnd)}`
  ]
  if (input.entityLabel && input.entityName) {
    lines.push(`${input.entityLabel}：${input.entityName}`)
  }
  return lines.join('\n')
}

function formatTimelineAverageMetrics(metrics?: HealthTimelineTooltipMetrics | null) {
  return [
    formatMs(metrics?.avg_latency_ms),
    formatMs(metrics?.avg_first_byte_ms),
    formatTps(metrics?.avg_tps)
  ].join('/')
}

function formatTimelineRequestBreakdown(
  metrics: HealthTimelineTooltipMetrics | null | undefined,
  status: string
) {
  if (!metrics) return '-'
  const total = formatTimelineCount(metrics.total_attempts)
  const eligible = formatTimelineCount(metrics.sla_eligible_count)
  const success = formatTimelineCount(metrics.success_count)
  const serviceError = formatTimelineCount(
    metrics.service_error_count ?? metrics.failed_count,
  )
  const userError = formatTimelineCount(metrics.user_error_count)
  const availability = formatTimelineMetricAvailability(metrics)
  return `${total}/${eligible}/${success}/${serviceError}/${userError}/${availability}/${getTimelineLabel(status)}`
}

function formatTimelineCount(value?: number | null) {
  if (typeof value !== 'number' || Number.isNaN(value)) return '-'
  return `${new Intl.NumberFormat('zh-CN').format(value)} 次`
}

function formatTimelineMetricAvailability(metrics?: HealthTimelineTooltipMetrics | null) {
  if (!metrics) return '-'
  if (typeof metrics.sla_eligible_count === 'number' &&
    Number.isFinite(metrics.sla_eligible_count) &&
    metrics.sla_eligible_count <= 0) return '-'
  if (typeof metrics.total_attempts === 'number' && metrics.total_attempts <= 0) return '-'
  if (typeof metrics.success_rate !== 'number' || Number.isNaN(metrics.success_rate)) return '-'
  return formatPercent(metrics.success_rate)
}

export function formatCompactNumber(value: number) {
  return new Intl.NumberFormat('zh-CN', {
    notation: 'compact',
    maximumFractionDigits: 1
  }).format(value)
}

export function formatTimestamp(timestamp?: string | null) {
  if (!timestamp) return '未知时间'
  const date = new Date(timestamp)
  if (Number.isNaN(date.getTime())) return '未知时间'
  return date.toLocaleString('zh-CN', {
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit'
  })
}

export function getTimelineColor(status: string) {
  switch (status) {
    case 'healthy':
      return 'bg-green-500/80 dark:bg-green-400/90'
    case 'warning':
      return 'bg-amber-400/80 dark:bg-amber-300/80'
    case 'unhealthy':
      return 'bg-red-500/80 dark:bg-red-400/90'
    case 'user_error':
      return 'bg-slate-400/80 dark:bg-slate-500/80'
    default:
      return 'bg-gray-300 dark:bg-gray-600'
  }
}

export function getTimelineLabel(status: string) {
  switch (status) {
    case 'healthy':
      return '健康'
    case 'warning':
      return '波动'
    case 'unhealthy':
      return '异常'
    case 'user_error':
      return '用户错误'
    default:
      return '无请求'
  }
}
