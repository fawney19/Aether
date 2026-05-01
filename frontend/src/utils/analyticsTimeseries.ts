import type { AnalyticsGranularity, AnalyticsSummary, AnalyticsTimeRange, AnalyticsTimeseriesBucket } from '@/api/analytics'

type DailyPreset = 'today' | 'last7days' | 'last30days' | 'last180days' | 'last1year'

const ZERO_ANALYTICS_SUMMARY: AnalyticsSummary = {
  requests_total: 0,
  requests_success: 0,
  requests_error: 0,
  requests_stream: 0,
  success_rate: 0,
  input_tokens: 0,
  output_tokens: 0,
  input_output_total_tokens: 0,
  cache_creation_input_tokens: 0,
  cache_read_input_tokens: 0,
  input_context_tokens: 0,
  total_tokens: 0,
  cache_hit_rate: 0,
  input_cost_usd: 0,
  output_cost_usd: 0,
  cache_creation_cost_usd: 0,
  cache_read_cost_usd: 0,
  cache_cost_usd: 0,
  request_cost_usd: 0,
  total_cost_usd: 0,
  actual_total_cost_usd: 0,
  actual_cache_cost_usd: 0,
  avg_response_time_ms: 0,
  avg_first_byte_time_ms: 0,
  format_conversion_count: 0,
  models_used_count: 0,
}

function parseDateKey(dateKey: string | undefined): Date | null {
  if (!dateKey) return null
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(dateKey.trim())
  if (!match) return null
  const [, year, month, day] = match
  return new Date(Date.UTC(Number(year), Number(month) - 1, Number(day)))
}

function formatDateKey(date: Date): string {
  return date.toISOString().slice(0, 10)
}

function formatLocalDateKey(date: Date): string {
  const year = date.getFullYear()
  const month = String(date.getMonth() + 1).padStart(2, '0')
  const day = String(date.getDate()).padStart(2, '0')
  return `${year}-${month}-${day}`
}

function formatHourKey(dateKey: string, hour: number): string {
  return `${dateKey}T${String(hour).padStart(2, '0')}`
}

function getStartOfWeekDateKey(date: Date): string {
  const weekday = date.getUTCDay()
  const diff = weekday === 0 ? -6 : 1 - weekday
  const result = new Date(date)
  result.setUTCDate(result.getUTCDate() + diff)
  return formatDateKey(result)
}

function getStartOfMonthDateKey(date: Date): string {
  const result = new Date(date)
  result.setUTCDate(1)
  return formatDateKey(result)
}

function getNextDateKey(dateKey: string): string {
  const date = parseDateKey(dateKey)
  if (!date) return dateKey
  date.setUTCDate(date.getUTCDate() + 1)
  return formatDateKey(date)
}

function getNextWeekKey(dateKey: string): string {
  const date = parseDateKey(dateKey)
  if (!date) return dateKey
  date.setUTCDate(date.getUTCDate() + 7)
  return formatDateKey(date)
}

function getNextMonthKey(dateKey: string): string {
  const date = parseDateKey(dateKey)
  if (!date) return dateKey
  date.setUTCMonth(date.getUTCMonth() + 1, 1)
  return formatDateKey(date)
}

function resolvePresetDateRange(preset?: string): { start_date: string; end_date: string } | null {
  const now = new Date()
  let startDate: Date
  const endDate = new Date(now)

  switch (preset as DailyPreset | undefined) {
    case 'today':
      startDate = new Date(now.getFullYear(), now.getMonth(), now.getDate())
      break
    case 'last7days':
      startDate = new Date(now.getFullYear(), now.getMonth(), now.getDate() - 6)
      break
    case 'last30days':
      startDate = new Date(now.getFullYear(), now.getMonth(), now.getDate() - 29)
      break
    case 'last180days':
      startDate = new Date(now.getFullYear(), now.getMonth(), now.getDate() - 179)
      break
    case 'last1year':
      startDate = new Date(now.getFullYear() - 1, now.getMonth(), now.getDate())
      break
    default:
      return null
  }

  return {
    start_date: formatLocalDateKey(startDate),
    end_date: formatLocalDateKey(endDate),
  }
}

function resolveDateBounds(
  timeRange: AnalyticsTimeRange,
  fallbackBuckets: AnalyticsTimeseriesBucket[],
): { startDate: Date | null; endDate: Date | null } {
  const sortedBuckets = sortBuckets(fallbackBuckets)
  const fallbackStart = sortedBuckets[0]?.bucket_start.slice(0, 10)
  const fallbackEnd = sortedBuckets[sortedBuckets.length - 1]?.bucket_start.slice(0, 10)
  const presetRange = resolvePresetDateRange(timeRange.preset)
  const startDate = parseDateKey(timeRange.start_date ?? presetRange?.start_date ?? fallbackStart)
  const endDate = parseDateKey(timeRange.end_date ?? presetRange?.end_date ?? fallbackEnd)
  return { startDate, endDate }
}

function sortBuckets(buckets: AnalyticsTimeseriesBucket[]): AnalyticsTimeseriesBucket[] {
  return buckets.slice().sort((left, right) => left.bucket_start.localeCompare(right.bucket_start))
}

function normalizeGranularity(granularity?: string): AnalyticsGranularity {
  if (granularity === 'hour' || granularity === 'week' || granularity === 'month') {
    return granularity
  }
  return 'day'
}

function resolveTodayDateKey(): string {
  return formatLocalDateKey(new Date())
}

export function getDateKeysInRange(
  timeRange: AnalyticsTimeRange,
  fallbackBuckets: AnalyticsTimeseriesBucket[] = [],
): string[] {
  const sortedBuckets = sortBuckets(fallbackBuckets)
  const { startDate, endDate } = resolveDateBounds(timeRange, sortedBuckets)

  if (!startDate || !endDate || startDate.getTime() > endDate.getTime()) {
    return sortedBuckets.map(bucket => bucket.bucket_start.slice(0, 10))
  }

  const dates: string[] = []
  const cursor = new Date(startDate)

  while (cursor.getTime() <= endDate.getTime()) {
    dates.push(formatDateKey(cursor))
    cursor.setUTCDate(cursor.getUTCDate() + 1)
  }

  return dates
}

export function getHourKeysInRange(
  timeRange: AnalyticsTimeRange,
  fallbackBuckets: AnalyticsTimeseriesBucket[] = [],
): string[] {
  const sortedBuckets = sortBuckets(fallbackBuckets)
  const { startDate, endDate } = resolveDateBounds(timeRange, sortedBuckets)

  if (!startDate || !endDate || startDate.getTime() !== endDate.getTime()) {
    return sortedBuckets.map(bucket => bucket.bucket_start.slice(0, 13))
  }

  const dateKey = formatDateKey(startDate)
  const isToday = dateKey === resolveTodayDateKey()
  const maxHour = isToday ? new Date().getHours() : 23

  return Array.from({ length: maxHour + 1 }, (_, hour) => formatHourKey(dateKey, hour))
}

export function getWeekKeysInRange(
  timeRange: AnalyticsTimeRange,
  fallbackBuckets: AnalyticsTimeseriesBucket[] = [],
): string[] {
  const sortedBuckets = sortBuckets(fallbackBuckets)
  const { startDate, endDate } = resolveDateBounds(timeRange, sortedBuckets)

  if (!startDate || !endDate || startDate.getTime() > endDate.getTime()) {
    return sortedBuckets.map(bucket => bucket.bucket_start.slice(0, 10))
  }

  const keys: string[] = []
  let cursorKey = getStartOfWeekDateKey(startDate)
  const endKey = getStartOfWeekDateKey(endDate)

  while (cursorKey <= endKey) {
    keys.push(cursorKey)
    cursorKey = getNextWeekKey(cursorKey)
  }

  return keys
}

export function getMonthKeysInRange(
  timeRange: AnalyticsTimeRange,
  fallbackBuckets: AnalyticsTimeseriesBucket[] = [],
): string[] {
  const sortedBuckets = sortBuckets(fallbackBuckets)
  const { startDate, endDate } = resolveDateBounds(timeRange, sortedBuckets)

  if (!startDate || !endDate || startDate.getTime() > endDate.getTime()) {
    return sortedBuckets.map(bucket => bucket.bucket_start.slice(0, 10))
  }

  const keys: string[] = []
  let cursorKey = getStartOfMonthDateKey(startDate)
  const endKey = getStartOfMonthDateKey(endDate)

  while (cursorKey <= endKey) {
    keys.push(cursorKey)
    cursorKey = getNextMonthKey(cursorKey)
  }

  return keys
}

export function createZeroDailyBucket(dateKey: string): AnalyticsTimeseriesBucket {
  return {
    ...ZERO_ANALYTICS_SUMMARY,
    bucket_start: `${dateKey}T00:00:00`,
    bucket_end: `${getNextDateKey(dateKey)}T00:00:00`,
  }
}

export function createZeroHourlyBucket(hourKey: string): AnalyticsTimeseriesBucket {
  const dateKey = hourKey.slice(0, 10)
  const hour = Number(hourKey.slice(11, 13))
  const nextDate = new Date(Date.UTC(
    Number(dateKey.slice(0, 4)),
    Number(dateKey.slice(5, 7)) - 1,
    Number(dateKey.slice(8, 10)),
    hour,
  ))
  nextDate.setUTCHours(nextDate.getUTCHours() + 1)
  const nextDateKey = formatDateKey(nextDate)
  const nextHour = String(nextDate.getUTCHours()).padStart(2, '0')

  return {
    ...ZERO_ANALYTICS_SUMMARY,
    bucket_start: `${hourKey}:00:00`,
    bucket_end: `${nextDateKey}T${nextHour}:00:00`,
  }
}

export function createZeroWeeklyBucket(dateKey: string): AnalyticsTimeseriesBucket {
  return {
    ...ZERO_ANALYTICS_SUMMARY,
    bucket_start: `${dateKey}T00:00:00`,
    bucket_end: `${getNextWeekKey(dateKey)}T00:00:00`,
  }
}

export function createZeroMonthlyBucket(dateKey: string): AnalyticsTimeseriesBucket {
  return {
    ...ZERO_ANALYTICS_SUMMARY,
    bucket_start: `${dateKey}T00:00:00`,
    bucket_end: `${getNextMonthKey(dateKey)}T00:00:00`,
  }
}

export function fillMissingDailyTimeseriesBuckets(
  buckets: AnalyticsTimeseriesBucket[],
  timeRange: AnalyticsTimeRange,
): AnalyticsTimeseriesBucket[] {
  const sortedBuckets = sortBuckets(buckets)
  const dateKeys = getDateKeysInRange(timeRange, sortedBuckets)

  if (!dateKeys.length) {
    return sortedBuckets
  }

  const bucketByDate = new Map(
    sortedBuckets.map(bucket => [bucket.bucket_start.slice(0, 10), bucket]),
  )

  return dateKeys.map(dateKey => bucketByDate.get(dateKey) ?? createZeroDailyBucket(dateKey))
}

export function fillMissingWeeklyTimeseriesBuckets(
  buckets: AnalyticsTimeseriesBucket[],
  timeRange: AnalyticsTimeRange,
): AnalyticsTimeseriesBucket[] {
  const sortedBuckets = sortBuckets(buckets)
  const weekKeys = getWeekKeysInRange(timeRange, sortedBuckets)

  if (!weekKeys.length) {
    return sortedBuckets
  }

  const bucketByWeek = new Map(
    sortedBuckets.map(bucket => [bucket.bucket_start.slice(0, 10), bucket]),
  )

  return weekKeys.map(weekKey => bucketByWeek.get(weekKey) ?? createZeroWeeklyBucket(weekKey))
}

export function fillMissingMonthlyTimeseriesBuckets(
  buckets: AnalyticsTimeseriesBucket[],
  timeRange: AnalyticsTimeRange,
): AnalyticsTimeseriesBucket[] {
  const sortedBuckets = sortBuckets(buckets)
  const monthKeys = getMonthKeysInRange(timeRange, sortedBuckets)

  if (!monthKeys.length) {
    return sortedBuckets
  }

  const bucketByMonth = new Map(
    sortedBuckets.map(bucket => [bucket.bucket_start.slice(0, 10), bucket]),
  )

  return monthKeys.map(monthKey => bucketByMonth.get(monthKey) ?? createZeroMonthlyBucket(monthKey))
}

export function fillMissingTimeseriesBuckets(
  buckets: AnalyticsTimeseriesBucket[],
  timeRange: AnalyticsTimeRange,
): AnalyticsTimeseriesBucket[] {
  const granularity = normalizeGranularity(timeRange.granularity)
  const sortedBuckets = sortBuckets(buckets)

  if (granularity === 'week') {
    return fillMissingWeeklyTimeseriesBuckets(sortedBuckets, timeRange)
  }

  if (granularity === 'month') {
    return fillMissingMonthlyTimeseriesBuckets(sortedBuckets, timeRange)
  }

  if (granularity !== 'hour') {
    return fillMissingDailyTimeseriesBuckets(sortedBuckets, timeRange)
  }

  const hourKeys = getHourKeysInRange(timeRange, sortedBuckets)

  if (!hourKeys.length) {
    return sortedBuckets
  }

  const bucketByHour = new Map(
    sortedBuckets.map(bucket => [bucket.bucket_start.slice(0, 13), bucket]),
  )

  return hourKeys.map(hourKey => bucketByHour.get(hourKey) ?? createZeroHourlyBucket(hourKey))
}
