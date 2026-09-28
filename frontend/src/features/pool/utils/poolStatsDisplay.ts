import type { QuotaWindowUsageSnapshot } from '@/api/endpoints/types/statusSnapshot'
import type { PoolManagementStatsMode } from '@/features/pool/utils/poolManagementState'
import { getCodexQuotaWindowPresentation } from '@/utils/codexQuotaWindow'
import { formatCompactNumber } from '@/utils/format'

export type PoolStatsMetricKey = 'request_count' | 'total_tokens' | 'total_cost_usd'
export type PoolStatsDisplayKind = 'account_total' | 'codex_cycle'
export type PoolCodexCycleWindowCode = string

export interface PoolStatsKeyInput {
  request_count?: number | null
  total_tokens?: number | null
  total_cost_usd?: number | string | null
  status_snapshot?: {
    quota?: {
      windows?: Array<{
        code?: string | null
        label?: string | null
        scope?: string | null
        quota_group_label?: string | null
        bucket_id?: string | null
        window_minutes?: number | null
        usage?: QuotaWindowUsageSnapshot | null
      } | null> | null
    } | null
  } | null
}

export interface PoolStatsMetric {
  key: PoolStatsMetricKey
  label: string
  value: string
  missing: boolean
  numericValue?: number | null
}

export interface PoolAccountTotalStatsDisplay {
  kind: 'account_total'
  metrics: PoolStatsMetric[]
}

export interface PoolCodexCycleStatsGroup {
  code: PoolCodexCycleWindowCode
  label: string
  /** Model family whose quota this window tracks (Antigravity quota groups). */
  section?: string
  metrics: PoolStatsMetric[]
}

export interface PoolCodexCycleStatsDisplay {
  kind: 'codex_cycle'
  groups: PoolCodexCycleStatsGroup[]
}

export type PoolStatsDisplay = PoolAccountTotalStatsDisplay | PoolCodexCycleStatsDisplay

const MISSING_STAT_VALUE = '—'

export function isCodexProviderType(providerType: string | null | undefined): boolean {
  return String(providerType || '').trim().toLowerCase() === 'codex'
}

const CYCLE_STATS_PROVIDER_TYPES = new Set(['codex', 'xai', 'antigravity'])

export function isCycleStatsProviderType(providerType: string | null | undefined): boolean {
  return CYCLE_STATS_PROVIDER_TYPES.has(String(providerType || '').trim().toLowerCase())
}

export function formatPoolStatInteger(value: number | null | undefined): string {
  const n = Number(value ?? 0)
  if (!Number.isFinite(n) || n <= 0) return '0'
  return Math.round(n).toLocaleString('en-US')
}

export function formatPoolTokenCount(value: number | null | undefined): string {
  const n = Number(value ?? 0)
  if (!Number.isFinite(n) || n <= 0) return '0'
  return formatCompactNumber(Math.round(n), { fractionDigits: 1 })
}

export function formatPoolStatUsd(value: number | string | null | undefined): string {
  const n = Number(value ?? 0)
  if (!Number.isFinite(n) || n <= 0) return '$0.00'
  if (n < 0.01) return `$${n.toFixed(4)}`
  if (n < 1) return `$${n.toFixed(3)}`
  if (n < 1000) return `$${n.toFixed(2)}`
  return `$${n.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`
}

function formatCycleInteger(value: number | null | undefined): string | null {
  if (value == null) return null
  const n = Number(value)
  if (!Number.isFinite(n)) return null
  if (n <= 0) return '0'
  return Math.round(n).toLocaleString('en-US')
}

function formatCycleTokenCount(value: number | null | undefined): string | null {
  if (value == null) return null
  const n = Number(value)
  if (!Number.isFinite(n)) return null
  return formatPoolTokenCount(n)
}

function formatCycleUsd(value: number | string | null | undefined): string | null {
  if (value == null) return null
  const n = Number(value)
  if (!Number.isFinite(n)) return null
  if (n <= 0) return '0'
  return formatPoolStatUsd(value)
}

function createMetric(
  key: PoolStatsMetricKey,
  label: string,
  value: string | null,
  numericValue?: number | null,
): PoolStatsMetric {
  return {
    key,
    label,
    value: value ?? MISSING_STAT_VALUE,
    missing: value == null,
    numericValue: numericValue ?? null,
  }
}

function normalizeWindowCode(value: unknown): string {
  return String(value || '').trim().toLowerCase()
}

// Antigravity quota groups: "group:<index>:<bucket_id>".
function quotaGroupPrefix(window: { bucket_id?: string | null, quota_group_label?: string | null }): string {
  const bucketId = String(window.bucket_id || '').trim().toLowerCase()
  if (bucketId.startsWith('gemini')) return 'Gemini'
  if (bucketId.startsWith('3p')) return 'Claude/GPT'
  return String(window.quota_group_label || '').trim()
}

function quotaGroupSortBase(code: string): number {
  const index = Number(code.split(':')[1])
  return Number.isInteger(index) && index >= 0 ? (index + 1) * 1_000_000 : 100_000_000
}

function getCodexCycleStatsGroups(
  key: PoolStatsKeyInput,
  providerType?: string | null,
): PoolCodexCycleStatsGroup[] {
  const windows = key.status_snapshot?.quota?.windows
  if (!Array.isArray(windows)) return []

  // Non-codex providers only get cycle stats on the windows the backend
  // annotated with a cycle length (xAI "usage", Antigravity quota groups).
  const requireWindowMinutes = providerType != null && !isCodexProviderType(providerType)
  const seenCodes = new Set<string>()
  return windows
    .map((window) => {
      if (!window) return null
      const code = normalizeWindowCode(window.code)
      const scope = String(window.scope || 'account').trim().toLowerCase()
      const isQuotaGroup = scope === 'quota_group'
      if (
        !code
        || (scope !== 'account' && !isQuotaGroup)
        || code.startsWith('spark_')
        || seenCodes.has(code)
        || (requireWindowMinutes && window.window_minutes == null)
      ) {
        return null
      }
      const presentation = getCodexQuotaWindowPresentation({
        code,
        label: isQuotaGroup ? null : window.label,
        scope,
        window_minutes: window.window_minutes,
      })
      if (!presentation) return null
      seenCodes.add(code)
      const prefix = isQuotaGroup ? quotaGroupPrefix(window) : ''
      return {
        code,
        label: prefix ? `${prefix} ${presentation.label}` : presentation.label,
        ...(prefix ? { section: prefix } : {}),
        sortOrder: (isQuotaGroup ? quotaGroupSortBase(code) : 0) + presentation.sortOrder,
        metrics: buildCycleMetrics(window.usage ?? null),
      }
    })
    .filter((group): group is PoolCodexCycleStatsGroup & { sortOrder: number } => group != null)
    .sort((left, right) => left.sortOrder - right.sortOrder)
    .map(({ sortOrder: _sortOrder, ...group }) => group)
}


function buildAccountTotalMetrics(key: PoolStatsKeyInput): PoolStatsMetric[] {
  return [
    createMetric('request_count', '请求', formatPoolStatInteger(key.request_count)),
    createMetric('total_tokens', 'Token', formatPoolTokenCount(key.total_tokens)),
    createMetric('total_cost_usd', '费用', formatPoolStatUsd(key.total_cost_usd)),
  ]
}

function buildCycleMetrics(usage: QuotaWindowUsageSnapshot | null): PoolStatsMetric[] {
  const requestCount = usage?.request_count == null ? null : Number(usage.request_count)
  const totalTokens = usage?.total_tokens == null ? null : Number(usage.total_tokens)
  const totalCostUsd = usage?.total_cost_usd == null ? null : Number(usage.total_cost_usd)
  return [
    createMetric(
      'request_count',
      '请求',
      formatCycleInteger(usage?.request_count),
      Number.isFinite(requestCount) ? Math.max(requestCount ?? 0, 0) : null,
    ),
    createMetric(
      'total_tokens',
      'Token',
      formatCycleTokenCount(usage?.total_tokens),
      Number.isFinite(totalTokens) ? Math.max(totalTokens ?? 0, 0) : null,
    ),
    createMetric(
      'total_cost_usd',
      '费用',
      formatCycleUsd(usage?.total_cost_usd),
      Number.isFinite(totalCostUsd) ? Math.max(totalCostUsd ?? 0, 0) : null,
    ),
  ]
}

export function buildAccountTotalStatsDisplay(
  key: PoolStatsKeyInput,
): PoolAccountTotalStatsDisplay {
  return {
    kind: 'account_total',
    metrics: buildAccountTotalMetrics(key),
  }
}

export function buildCodexCycleStatsDisplay(
  key: PoolStatsKeyInput,
  providerType?: string | null,
): PoolCodexCycleStatsDisplay {
  return {
    kind: 'codex_cycle',
    groups: getCodexCycleStatsGroups(key, providerType),
  }
}

export function buildPoolStatsDisplay(
  key: PoolStatsKeyInput,
  providerType: string | null | undefined,
  mode: PoolManagementStatsMode,
): PoolStatsDisplay {
  if (isCycleStatsProviderType(providerType) && mode === 'current_cycle') {
    const display = buildCodexCycleStatsDisplay(key, providerType)
    // Keys without any cycle window (e.g. never refreshed) keep account totals.
    if (isCodexProviderType(providerType) || display.groups.length > 0) return display
  }

  return buildAccountTotalStatsDisplay(key)
}
