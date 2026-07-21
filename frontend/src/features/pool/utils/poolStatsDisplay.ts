import type { QuotaWindowUsageSnapshot } from '@/api/endpoints/types/statusSnapshot'
import type { PoolManagementStatsMode } from '@/features/pool/utils/poolManagementState'
import { getCodexQuotaWindowPresentation } from '@/utils/codexQuotaWindow'
import { formatCompactNumber } from '@/utils/format'

export type PoolStatsMetricKey = 'request_count' | 'total_tokens' | 'total_cost_usd'
export type PoolStatsDisplayKind = 'account_total' | 'codex_cycle'
export type PoolCodexCycleWindowCode = string

interface PoolStatsQuotaWindowInput {
  code?: string | null
  label?: string | null
  scope?: string | null
  window_minutes?: number | null
  unit?: string | null
  usage?: QuotaWindowUsageSnapshot | null
}

export interface PoolStatsKeyInput {
  request_count?: number | null
  total_tokens?: number | null
  total_cost_usd?: number | string | null
  status_snapshot?: {
    quota?: {
      windows?: Array<PoolStatsQuotaWindowInput | null> | null
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

export function isGlmCodingPlanProviderType(providerType: string | null | undefined): boolean {
  return String(providerType || '').trim().toLowerCase() === 'glm_coding_plan'
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

function getQuotaWindowByCodes(
  key: PoolStatsKeyInput,
  codes: readonly string[],
): PoolStatsQuotaWindowInput | null {
  const windows = key.status_snapshot?.quota?.windows
  if (!Array.isArray(windows)) return null

  const normalizedCodes = new Set(codes.map(normalizeWindowCode))
  return windows.find(item => normalizedCodes.has(normalizeWindowCode(item?.code))) ?? null
}

function getCodexCycleStatsGroups(key: PoolStatsKeyInput): PoolCodexCycleStatsGroup[] {
  const windows = key.status_snapshot?.quota?.windows
  if (!Array.isArray(windows)) return []

  const seenCodes = new Set<string>()
  return windows
    .map((window) => {
      if (!window) return null
      const code = normalizeWindowCode(window.code)
      const scope = String(window.scope || 'account').trim().toLowerCase()
      if (!code || scope !== 'account' || code.startsWith('spark_') || seenCodes.has(code)) {
        return null
      }
      const presentation = getCodexQuotaWindowPresentation({
        code,
        label: window.label,
        scope,
        window_minutes: window.window_minutes,
      })
      if (!presentation) return null
      seenCodes.add(code)
      return {
        code,
        label: presentation.label,
        sortOrder: presentation.sortOrder,
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

function buildOfficialUsageMetrics(usage: QuotaWindowUsageSnapshot | null): PoolStatsMetric[] {
  return [
    createMetric('request_count', '请求', formatCycleInteger(usage?.request_count)),
    createMetric('total_tokens', 'Token', formatCycleTokenCount(usage?.total_tokens)),
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
): PoolCodexCycleStatsDisplay {
  return {
    kind: 'codex_cycle',
    groups: getCodexCycleStatsGroups(key),
  }
}

export function buildGlmCodingPlanStatsDisplay(
  key: PoolStatsKeyInput,
): PoolCodexCycleStatsDisplay {
  const fiveHWindow = getQuotaWindowByCodes(key, ['tokens_5h', '5h'])
  const weeklyWindow = getQuotaWindowByCodes(key, ['tokens_weekly', 'weekly'])

  return {
    kind: 'codex_cycle',
    groups: [
      {
        code: '5h',
        label: '5h',
        metrics: buildOfficialUsageMetrics(fiveHWindow?.usage ?? null),
      },
      {
        code: 'weekly',
        label: '周',
        metrics: buildOfficialUsageMetrics(weeklyWindow?.usage ?? null),
      },
    ],
  }
}

export function buildPoolStatsDisplay(
  key: PoolStatsKeyInput,
  providerType: string | null | undefined,
  mode: PoolManagementStatsMode,
): PoolStatsDisplay {
  if (isGlmCodingPlanProviderType(providerType)) {
    return buildGlmCodingPlanStatsDisplay(key)
  }

  if (isCodexProviderType(providerType) && mode === 'current_cycle') {
    return buildCodexCycleStatsDisplay(key)
  }

  return buildAccountTotalStatsDisplay(key)
}
