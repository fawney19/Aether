import type { OverviewDashboardChartMetrics, OverviewDashboardCharts, OverviewRange } from '@/api/overview'
import type { DateRangeParams } from '@/features/usage/types'
import { getI18nLocale } from '@/i18n'
import { browserTimezone, zonedInput, zonedInstant } from '../query'
import { amountValue } from './amount'

export const chartColors = ['#3b82f6', '#10b981', '#f59e0b', '#8b5cf6', '#ef4444', '#06b6d4', '#94a3b8']
type ModelRow = Pick<OverviewDashboardCharts['models'][number], 'id' | 'label' | 'bucket_start' | 'billable_amount'>

export function chartDate(value: string, timezone: string): string {
  return new Intl.DateTimeFormat(getI18nLocale(), { timeZone: timezone, month: 'short', day: 'numeric' }).format(new Date(value))
}

function knownSum(rows: Pick<OverviewDashboardChartMetrics, 'billable_amount'>[]): number | null {
  if (!rows.length) return 0
  const values = rows.map(row => amountValue(row.billable_amount)).filter((value): value is number => value !== null)
  return values.length ? values.reduce((sum, value) => sum + value, 0) : null
}

export function modelDatasets(data: { series: Pick<OverviewDashboardCharts['series'][number], 'bucket_start' | 'billable_amount'>[]; models: ModelRow[] }, unknownLabel: string, otherLabel: string) {
  const groups = new Map<string | null, ModelRow[]>()
  for (const row of data.models) {
    const rows = groups.get(row.id)
    if (rows) rows.push(row)
    else groups.set(row.id, [row])
  }
  const sorted = [...groups].sort((a, b) => (knownSum(b[1]) ?? -1) - (knownSum(a[1]) ?? -1))
  const visible = sorted.slice(0, 6).map(([id, rows]) => ({ id, label: rows[0]?.label || unknownLabel, rows }))
  if (sorted.length > 6) visible.push({ id: '__other__', label: otherLabel, rows: sorted.slice(6).flatMap(([, rows]) => rows) })
  return visible.map((group, index) => ({
    label: group.label,
    data: data.series.map(day => {
      const rows = group.rows.filter(row => Date.parse(row.bucket_start) === Date.parse(day.bucket_start))
      const incomplete = amountValue(day.billable_amount) === null
        || day.billable_amount.status === 'known_subtotal' || day.billable_amount.status === 'estimated_subtotal'
      return !rows.length && incomplete ? null : knownSum(rows)
    }),
    backgroundColor: chartColors[index % chartColors.length],
    borderRadius: 2, stack: 'models', barPercentage: 0.6, categoryPercentage: 0.7,
  }))
}

export function providerSlices(rows: Pick<OverviewDashboardCharts['providers'][number], 'label' | 'billable_amount'>[], unknownLabel: string, otherLabel: string) {
  const known = rows.map(row => ({ label: row.label || unknownLabel, value: amountValue(row.billable_amount) }))
    .filter((row): row is { label: string; value: number } => row.value !== null && row.value > 0)
    .sort((a, b) => b.value - a.value)
  const visible = known.slice(0, 6)
  if (known.length > 6) visible.push({ label: otherLabel, value: known.slice(6).reduce((sum, row) => sum + row.value, 0) })
  return visible
}

export function dayRange(bucket: string, range: OverviewRange): OverviewRange {
  const date = zonedInput(bucket, range.timezone).slice(0, 10)
  const nextDate = new Date(Date.parse(`${date}T00:00:00Z`) + 86_400_000).toISOString().slice(0, 10)
  const from = localDayStart(date, range.timezone)
  const to = localDayStart(nextDate, range.timezone)
  return {
    from: new Date(Math.max(Date.parse(from), Date.parse(range.from))).toISOString(),
    to: new Date(Math.min(Date.parse(to), Date.parse(range.to))).toISOString(),
    timezone: range.timezone,
  }
}

/** The dashboard picker uses inclusive calendar dates, not rolling 24-hour periods. */
export function dashboardChartRange(params: DateRangeParams, now = new Date()): OverviewRange {
  const timezone = params.timezone || browserTimezone()
  if (params.from && params.to) {
    if (!Number.isFinite(Date.parse(params.from)) || Date.parse(params.from) >= Date.parse(params.to)) {
      throw new Error('Invalid dashboard time range')
    }
    return { from: new Date(params.from).toISOString(), to: new Date(params.to).toISOString(), timezone }
  }
  const shiftDate = (date: string, days: number) => new Date(Date.parse(`${date}T00:00:00Z`) + days * 86_400_000).toISOString().slice(0, 10)
  const today = zonedInput(now, timezone).slice(0, 10)
  const days = params.preset === 'last90days' ? 90 : params.preset === 'last30days' ? 30
    : params.preset === 'today' || params.preset === 'yesterday' ? 1 : 7
  const endDate = params.end_date || shiftDate(today, params.preset === 'yesterday' ? -1 : 0)
  const startDate = params.start_date || shiftDate(endDate, 1 - days)
  if (startDate > endDate) throw new Error('Invalid dashboard time range')
  return { from: localDayStart(startDate, timezone), to: localDayStart(shiftDate(endDate, 1), timezone), timezone }
}

function localDayStart(date: string, timezone: string): string {
  const midnight = Date.parse(`${date}T00:00:00Z`)
  // Some timezone transitions skip midnight; start at the first existing local minute.
  for (let minute = 0; minute <= 1440; minute++) {
    const wall = new Date(midnight + minute * 60_000).toISOString().slice(0, 16)
    const instant = zonedInstant(wall, timezone)
    if (instant) return instant
  }
  throw new Error('Unable to resolve local day boundary')
}
