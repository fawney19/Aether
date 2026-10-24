import type { CodexUpstreamMetadata, QuotaStatusSnapshot, QuotaWindowSnapshot } from '@/api/endpoints/types'
import type { ProviderCustomQuota } from '@/api/providerFinance'

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

// ===== 共享投影：status_snapshot → codex 展示字段 =====

function findWindowByCode(
  quota: QuotaStatusSnapshot | null | undefined,
  code: string,
): QuotaWindowSnapshot | null {
  const windows = quota?.windows
  if (!Array.isArray(windows)) return null
  return windows.find(window => String(window?.code || '').trim().toLowerCase() === code.trim().toLowerCase()) ?? null
}

function windowUsedPercent(window: QuotaWindowSnapshot | null | undefined): number | undefined {
  if (!window) return undefined
  if (typeof window.used_ratio === 'number') {
    return Math.max(Math.min(window.used_ratio * 100, 100), 0)
  }
  if (typeof window.remaining_ratio === 'number') {
    return Math.max(Math.min((1 - window.remaining_ratio) * 100, 100), 0)
  }
  return undefined
}

function windowResetAt(window: QuotaWindowSnapshot | null | undefined): number | undefined {
  return typeof window?.reset_at === 'number' && window.reset_at > 0 ? window.reset_at : undefined
}

function windowResetSeconds(window: QuotaWindowSnapshot | null | undefined): number | undefined {
  return typeof window?.reset_seconds === 'number' && window.reset_seconds >= 0
    ? window.reset_seconds
    : undefined
}

function snapshotUpdatedAt(quota: QuotaStatusSnapshot | null | undefined): number | undefined {
  const updatedAt = quota?.updated_at ?? quota?.observed_at
  return typeof updatedAt === 'number' ? updatedAt : undefined
}

/** Project a persisted quota snapshot onto the codex display fields used by
 * the drawer and the quota-query test panel, so both render identically. */
export function getCodexQuotaDisplayFromSnapshot(
  quota: QuotaStatusSnapshot | null | undefined,
): CodexUpstreamMetadata | null {
  if (!quota) return null

  const display: CodexUpstreamMetadata = {}
  const updatedAt = snapshotUpdatedAt(quota)
  if (updatedAt !== undefined) display.updated_at = updatedAt
  if (quota.plan_type) display.plan_type = quota.plan_type

  const primaryWindow = getCodexPrimaryQuotaWindow(quota.windows)
  const primaryUsedPercent = windowUsedPercent(primaryWindow)
  if (primaryUsedPercent !== undefined) display.primary_used_percent = primaryUsedPercent
  const primaryResetAt = windowResetAt(primaryWindow)
  if (primaryResetAt !== undefined) display.primary_reset_at = primaryResetAt
  const primaryResetSeconds = windowResetSeconds(primaryWindow)
  if (primaryResetSeconds !== undefined) display.primary_reset_seconds = primaryResetSeconds
  if (typeof primaryWindow?.window_minutes === 'number') {
    display.primary_window_minutes = primaryWindow.window_minutes
  }
  if (primaryWindow?.code) display.primary_code = String(primaryWindow.code)

  // When both monthly and weekly windows exist the primary slot takes the
  // monthly window; project the weekly window separately so it still renders
  // (custom providers commonly expose 月 / 周 / 5h at the same time).
  const weeklyWindow = findWindowByCode(quota, 'weekly')
  const weeklyUsedPercent = windowUsedPercent(weeklyWindow)
  if (weeklyUsedPercent !== undefined) display.weekly_used_percent = weeklyUsedPercent
  const weeklyResetAt = windowResetAt(weeklyWindow)
  if (weeklyResetAt !== undefined) display.weekly_reset_at = weeklyResetAt
  const weeklyResetSeconds = windowResetSeconds(weeklyWindow)
  if (weeklyResetSeconds !== undefined) display.weekly_reset_seconds = weeklyResetSeconds

  const secondaryWindow = findWindowByCode(quota, '5h')
  const secondaryUsedPercent = windowUsedPercent(secondaryWindow)
  if (secondaryUsedPercent !== undefined) display.secondary_used_percent = secondaryUsedPercent
  const secondaryResetAt = windowResetAt(secondaryWindow)
  if (secondaryResetAt !== undefined) display.secondary_reset_at = secondaryResetAt
  const secondaryResetSeconds = windowResetSeconds(secondaryWindow)
  if (secondaryResetSeconds !== undefined) display.secondary_reset_seconds = secondaryResetSeconds
  if (typeof secondaryWindow?.window_minutes === 'number') {
    display.secondary_window_minutes = secondaryWindow.window_minutes
  }

  const sparkPrimaryWindow = findWindowByCode(quota, 'spark_5h')
  const sparkPrimaryUsedPercent = windowUsedPercent(sparkPrimaryWindow)
  if (sparkPrimaryUsedPercent !== undefined) display.spark_primary_used_percent = sparkPrimaryUsedPercent
  const sparkPrimaryResetAt = windowResetAt(sparkPrimaryWindow)
  if (sparkPrimaryResetAt !== undefined) display.spark_primary_reset_at = sparkPrimaryResetAt
  const sparkPrimaryResetSeconds = windowResetSeconds(sparkPrimaryWindow)
  if (sparkPrimaryResetSeconds !== undefined) display.spark_primary_reset_seconds = sparkPrimaryResetSeconds
  if (typeof sparkPrimaryWindow?.window_minutes === 'number') {
    display.spark_primary_window_minutes = sparkPrimaryWindow.window_minutes
  }

  const sparkSecondaryWindow = findWindowByCode(quota, 'spark_weekly')
  const sparkSecondaryUsedPercent = windowUsedPercent(sparkSecondaryWindow)
  if (sparkSecondaryUsedPercent !== undefined) display.spark_secondary_used_percent = sparkSecondaryUsedPercent
  const sparkSecondaryResetAt = windowResetAt(sparkSecondaryWindow)
  if (sparkSecondaryResetAt !== undefined) display.spark_secondary_reset_at = sparkSecondaryResetAt
  const sparkSecondaryResetSeconds = windowResetSeconds(sparkSecondaryWindow)
  if (sparkSecondaryResetSeconds !== undefined) display.spark_secondary_reset_seconds = sparkSecondaryResetSeconds
  if (typeof sparkSecondaryWindow?.window_minutes === 'number') {
    display.spark_secondary_window_minutes = sparkSecondaryWindow.window_minutes
  }
  if (quota.reset_credits) display.reset_credits = quota.reset_credits

  return Object.keys(display).length > 0 ? display : null
}

// ===== 自定义额度查询：extractor 结果 → 共享快照 =====

/** Custom period keys share the codex window vocabulary so the existing
 * display derives the 周 / 5H / 月 labels itself. */
function normalizeCustomPeriodCode(key: string): string {
  const code = key.trim().toLowerCase()
  if (code === 'week' || code === 'weekly') return 'weekly'
  if (code === 'month' || code === 'monthly') return 'monthly'
  return code
}

/** RFC3339 reset times become epoch seconds; free display text is dropped,
 * mirroring the gateway's snapshot projection. */
function parseCustomResetAtEpoch(raw: unknown): number | null {
  if (typeof raw !== 'string') return null
  const text = raw.trim()
  if (!/^\d{4}-\d{2}-\d{2}T/.test(text)) return null
  const millis = Date.parse(text)
  return Number.isNaN(millis) ? null : Math.floor(millis / 1000)
}

/** Mirror of the gateway's `build_custom_quota_snapshot`: project the custom
 * extractor result onto the shared quota snapshot, so the test panel renders
 * through the exact same display the persisted key snapshots use. */
export function buildCustomQuotaSnapshotFromExtractorData(
  data: ProviderCustomQuota | null | undefined,
  nowSecs?: number,
): QuotaStatusSnapshot | null {
  const periods = data?.periods
  if (!Array.isArray(periods) || periods.length === 0) return null

  const windows: QuotaWindowSnapshot[] = []
  for (const period of periods.slice(0, 8)) {
    const key = String(period?.key ?? '').trim()
    if (!key) continue
    const window: QuotaWindowSnapshot = {
      code: normalizeCustomPeriodCode(key),
      label: key,
      scope: 'account',
    }
    const used = Number(period?.used)
    const limit = Number(period?.limit)
    if (Number.isFinite(used) && Number.isFinite(limit) && limit > 0) {
      window.used_ratio = Math.min(Math.max(used / limit, 0), 1)
    }
    const resetAt = parseCustomResetAtEpoch(period?.reset_at)
    if (resetAt !== null) window.reset_at = resetAt
    windows.push(window)
  }
  if (windows.length === 0) return null

  return {
    provider_type: 'custom',
    code: data?.is_valid === false ? 'exhausted' : 'ok',
    exhausted: data?.is_valid === false,
    label: data?.plan_name ?? null,
    reason: data?.invalid_message ?? null,
    updated_at: nowSecs ?? Math.floor(Date.now() / 1000),
    windows,
  }
}

/** Primary-window limit label for custom providers (月限额 / 周限额 / …),
 * derived from the real window instead of a hardcoded 周. */
export function getCustomQuotaPrimaryLimitLabel(
  windows: QuotaWindowSnapshot[] | null | undefined,
): string | null {
  const primaryWindow = getCodexPrimaryQuotaWindow(windows)
  return primaryWindow ? getCodexQuotaWindowLimitLabel(primaryWindow) : null
}
