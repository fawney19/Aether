import type { DateRangeParams } from '@/features/usage/types'

export type AnalyticsGranularityOption = NonNullable<DateRangeParams['granularity']>
export type ResolvedAnalyticsGranularity = Exclude<AnalyticsGranularityOption, 'auto'>

export const analyticsGranularityTabs: Array<{ value: AnalyticsGranularityOption; label: string }> = [
  { value: 'auto', label: '自动' },
  { value: 'hour', label: '小时' },
  { value: 'day', label: '天' },
  { value: 'week', label: '周' },
  { value: 'month', label: '月' },
]

export function parseAnalyticsDateKey(dateKey?: string): Date | null {
  if (!dateKey) return null
  return new Date(`${dateKey}T00:00:00`)
}

export function resolveAnalyticsRangeBounds(range: DateRangeParams): { startDate: Date | null; endDate: Date | null } {
  const today = new Date()

  switch (range.preset) {
    case 'today':
      return {
        startDate: new Date(today.getFullYear(), today.getMonth(), today.getDate()),
        endDate: new Date(today.getFullYear(), today.getMonth(), today.getDate()),
      }
    case 'last7days':
      return {
        startDate: new Date(today.getFullYear(), today.getMonth(), today.getDate() - 6),
        endDate: new Date(today.getFullYear(), today.getMonth(), today.getDate()),
      }
    case 'last30days':
      return {
        startDate: new Date(today.getFullYear(), today.getMonth(), today.getDate() - 29),
        endDate: new Date(today.getFullYear(), today.getMonth(), today.getDate()),
      }
    case 'last180days':
      return {
        startDate: new Date(today.getFullYear(), today.getMonth(), today.getDate() - 179),
        endDate: new Date(today.getFullYear(), today.getMonth(), today.getDate()),
      }
    case 'last1year':
      return {
        startDate: new Date(today.getFullYear() - 1, today.getMonth(), today.getDate()),
        endDate: new Date(today.getFullYear(), today.getMonth(), today.getDate()),
      }
    default:
      return {
        startDate: parseAnalyticsDateKey(range.start_date),
        endDate: parseAnalyticsDateKey(range.end_date),
      }
  }
}

export function getAnalyticsRangeDaysInclusive(range: DateRangeParams): number | null {
  const { startDate, endDate } = resolveAnalyticsRangeBounds(range)
  if (!startDate || !endDate) return null
  const diff = endDate.getTime() - startDate.getTime()
  if (diff < 0) return null
  return Math.floor(diff / (24 * 60 * 60 * 1000)) + 1
}

export function isAnalyticsSingleDayRange(range: DateRangeParams): boolean {
  return getAnalyticsRangeDaysInclusive(range) === 1
}

export function resolveAnalyticsAutoGranularity(range: DateRangeParams): ResolvedAnalyticsGranularity {
  if (range.granularity && range.granularity !== 'auto') {
    if (range.granularity === 'hour') {
      return isAnalyticsSingleDayRange(range) ? 'hour' : 'day'
    }
    return range.granularity
  }

  const rangeDays = getAnalyticsRangeDaysInclusive(range)
  if (rangeDays == null) return 'day'
  if (rangeDays === 1) return 'hour'
  if (rangeDays > 180) return 'month'
  if (rangeDays > 31) return 'week'
  return 'day'
}

export function formatAnalyticsTrendDescription(granularity: ResolvedAnalyticsGranularity, metricsLabel: string): string {
  if (granularity === 'hour') {
    return `按小时观察今天截至当前时刻的${metricsLabel}`
  }
  if (granularity === 'week') {
    return `按周观察${metricsLabel}`
  }
  if (granularity === 'month') {
    return `按月观察${metricsLabel}`
  }
  return `按日观察${metricsLabel}`
}

export function formatAnalyticsTrendBucketLabel(
  bucketStart: string,
  granularity: ResolvedAnalyticsGranularity,
): string {
  if (granularity === 'hour') {
    return `${bucketStart.slice(11, 13)}:00`
  }
  if (granularity === 'month') {
    return bucketStart.slice(0, 7)
  }
  return bucketStart.slice(5, 10)
}

function formatAnalyticsDateRangeLabel(start: Date, end: Date): string {
  const startLabel = `${String(start.getMonth() + 1).padStart(2, '0')}/${String(start.getDate()).padStart(2, '0')}`
  const endLabel = `${String(end.getMonth() + 1).padStart(2, '0')}/${String(end.getDate()).padStart(2, '0')}`
  return `${startLabel} - ${endLabel}`
}

export function formatAnalyticsTrendTooltipTitle(
  bucketStart: string,
  granularity: ResolvedAnalyticsGranularity,
): string {
  if (granularity === 'hour') {
    return `${bucketStart.slice(0, 10)} ${bucketStart.slice(11, 13)}:00`
  }

  if (granularity === 'month') {
    return bucketStart.slice(0, 7)
  }

  if (granularity === 'week') {
    const start = parseAnalyticsDateKey(bucketStart.slice(0, 10))
    if (!start) return bucketStart.slice(0, 10)
    const end = new Date(start)
    end.setDate(end.getDate() + 6)
    return formatAnalyticsDateRangeLabel(start, end)
  }

  return bucketStart.slice(0, 10)
}
