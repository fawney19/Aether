import { getProvidersSummary } from '@/api/endpoints/providers'
import type { ProviderWithEndpointsSummary } from '@/api/endpoints/types'

const HEALTHY_SCORE_THRESHOLD = 0.8
const WARNING_SCORE_THRESHOLD = 0.5
const DEFAULT_TIMELINE_SEGMENT_COUNT = 100

export interface HealthTimelineMonitorLike {
  timeline?: string[]
  time_range_start?: string | null
  time_range_end?: string | null
}

export async function fetchAllProviderSummaries(
  pageSize: number = 200
): Promise<ProviderWithEndpointsSummary[]> {
  const items: ProviderWithEndpointsSummary[] = []
  let page = 1

  while (true) {
    const response = await getProvidersSummary({ page, page_size: pageSize })
    items.push(...response.items)

    if (items.length >= response.total || response.items.length === 0) {
      break
    }

    page += 1
  }

  return items
}

export function clampHealthScore(score: number | null | undefined): number {
  if (score === null || score === undefined || Number.isNaN(score)) {
    return 0
  }

  return Math.min(1, Math.max(0, score))
}

export function formatHealthScore(score: number | null | undefined): string {
  return `${Math.round(clampHealthScore(score) * 100)}%`
}

export function getHealthLabel(score: number | null | undefined): string {
  const value = clampHealthScore(score)

  if (value >= HEALTHY_SCORE_THRESHOLD) return '健康'
  if (value >= WARNING_SCORE_THRESHOLD) return '警告'
  return '异常'
}

export function getHealthBadgeVariant(
  score: number | null | undefined
): 'default' | 'secondary' | 'destructive' | 'outline' {
  const value = clampHealthScore(score)

  if (value >= HEALTHY_SCORE_THRESHOLD) return 'default'
  if (value >= WARNING_SCORE_THRESHOLD) return 'secondary'
  return 'destructive'
}

export function getHealthBarClass(score: number | null | undefined): string {
  const value = clampHealthScore(score)

  if (value >= HEALTHY_SCORE_THRESHOLD) return 'bg-emerald-500'
  if (value >= WARNING_SCORE_THRESHOLD) return 'bg-amber-500'
  return 'bg-red-500'
}

export function getHealthTextClass(score: number | null | undefined): string {
  const value = clampHealthScore(score)

  if (value >= HEALTHY_SCORE_THRESHOLD) return 'text-emerald-600 dark:text-emerald-400'
  if (value >= WARNING_SCORE_THRESHOLD) return 'text-amber-600 dark:text-amber-400'
  return 'text-red-600 dark:text-red-400'
}

export function buildFallbackTimelineMonitor(
  lookbackHours: number,
  segmentCount: number = DEFAULT_TIMELINE_SEGMENT_COUNT
): HealthTimelineMonitorLike {
  const end = Date.now()
  const start = end - lookbackHours * 60 * 60 * 1000

  return {
    timeline: Array.from({ length: segmentCount }, () => 'unknown'),
    time_range_start: new Date(start).toISOString(),
    time_range_end: new Date(end).toISOString(),
  }
}

export function mergeTimelineMonitors(
  monitors: HealthTimelineMonitorLike[],
  lookbackHours: number
): HealthTimelineMonitorLike {
  if (monitors.length === 0) {
    return buildFallbackTimelineMonitor(lookbackHours)
  }

  const maxSegments = Math.max(
    ...monitors.map(monitor => monitor.timeline?.length || 0),
    DEFAULT_TIMELINE_SEGMENT_COUNT
  )

  const timeline = Array.from({ length: maxSegments }, (_, index) => {
    const statuses = monitors.map(monitor => monitor.timeline?.[index] ?? 'unknown')
    return mergeTimelineStatus(statuses)
  })

  const startTimestamps = monitors
    .map(monitor => monitor.time_range_start)
    .filter((value): value is string => Boolean(value))
    .map(value => new Date(value).getTime())
    .filter(value => Number.isFinite(value))

  const endTimestamps = monitors
    .map(monitor => monitor.time_range_end)
    .filter((value): value is string => Boolean(value))
    .map(value => new Date(value).getTime())
    .filter(value => Number.isFinite(value))

  const fallback = buildFallbackTimelineMonitor(lookbackHours, maxSegments)

  return {
    timeline,
    time_range_start: startTimestamps.length > 0
      ? new Date(Math.min(...startTimestamps)).toISOString()
      : fallback.time_range_start,
    time_range_end: endTimestamps.length > 0
      ? new Date(Math.max(...endTimestamps)).toISOString()
      : fallback.time_range_end,
  }
}

function mergeTimelineStatus(statuses: string[]): string {
  if (statuses.some(status => status === 'unhealthy')) return 'unhealthy'
  if (statuses.some(status => status === 'warning')) return 'warning'
  if (statuses.some(status => status === 'healthy')) return 'healthy'
  return 'unknown'
}
