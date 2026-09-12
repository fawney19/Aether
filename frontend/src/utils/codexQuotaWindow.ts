import type { QuotaWindowSnapshot } from '@/api/endpoints/types'

const MINUTES_PER_HOUR = 60
const MINUTES_PER_DAY = 24 * MINUTES_PER_HOUR
const MINUTES_PER_WEEK = 7 * MINUTES_PER_DAY
const MIN_MONTH_MINUTES = 28 * MINUTES_PER_DAY
const MAX_MONTH_MINUTES = 31 * MINUTES_PER_DAY

export interface CodexQuotaWindowPresentation {
  label: string
  sortOrder: number
}

function formatCodexQuotaPeriod(windowMinutes: number): string {
  if (windowMinutes === 5 * MINUTES_PER_HOUR) return '5H'
  if (windowMinutes === MINUTES_PER_WEEK) return '周'
  if (windowMinutes >= MIN_MONTH_MINUTES && windowMinutes <= MAX_MONTH_MINUTES) return '月'

  if (windowMinutes % MINUTES_PER_WEEK === 0) {
    return `${windowMinutes / MINUTES_PER_WEEK}周`
  }
  if (windowMinutes % MINUTES_PER_DAY === 0) {
    return `${windowMinutes / MINUTES_PER_DAY}天`
  }
  if (windowMinutes % MINUTES_PER_HOUR === 0) {
    return `${windowMinutes / MINUTES_PER_HOUR}H`
  }
  return `${windowMinutes}分钟`
}

function getLegacyCodexQuotaPeriod(code: string, label: string): string | null {
  if (code === '5h') return '5H'
  if (code === 'weekly') return '周'
  if (code === 'monthly') return '月'
  return label || null
}

export function getCodexQuotaWindowPresentation(
  window: QuotaWindowSnapshot,
): CodexQuotaWindowPresentation | null {
  const code = String(window.code || '').trim().toLowerCase()
  const isSpark = code.startsWith('spark_')
  const baseCode = isSpark ? code.slice('spark_'.length) : code
  const rawLabel = String(window.label || '').trim()
  const modelLabel = String(window.scope || '').trim().toLowerCase() === 'model' && !isSpark
    ? [rawLabel, window.model, window.quota_group_label, window.quota_group, window.code]
      .map(value => String(value || '').trim())
      .find(Boolean)
    : undefined
  const legacyLabel = isSpark ? rawLabel.replace(/^Spark\s*/i, '') : rawLabel || modelLabel || ''
  const hasExplicitWindowMinutes = window.window_minutes != null
  const windowMinutes = Number(window.window_minutes)

  if (hasExplicitWindowMinutes && (!Number.isFinite(windowMinutes) || windowMinutes <= 0)) {
    return null
  }

  const period = hasExplicitWindowMinutes
    ? formatCodexQuotaPeriod(windowMinutes)
    : getLegacyCodexQuotaPeriod(baseCode, legacyLabel)
  if (!period) return null

  let label = period
  if (isSpark) {
    label = `Spark${period}`
  } else if (modelLabel && modelLabel !== period) {
    label = `${modelLabel} ${period}`
  }

  const fallbackOrder = baseCode === '5h' ? 300 : baseCode === 'weekly' ? 10_080 : 1_000_000
  return {
    label,
    sortOrder: (isSpark ? 10_000_000 : 0) + (hasExplicitWindowMinutes ? windowMinutes : fallbackOrder),
  }
}

export function getCodexQuotaWindowLimitLabel(window: QuotaWindowSnapshot): string | null {
  const presentation = getCodexQuotaWindowPresentation(window)
  return presentation ? `${presentation.label}限额` : null
}

export function getCodexPrimaryQuotaWindow(
  windows: QuotaWindowSnapshot[] | null | undefined,
): QuotaWindowSnapshot | null {
  if (!Array.isArray(windows)) return null

  for (const code of ['monthly', 'weekly']) {
    const window = windows.find(candidate => String(candidate.code || '').trim().toLowerCase() === code)
    if (window && getCodexQuotaWindowPresentation(window)) return window
  }
  return null
}
