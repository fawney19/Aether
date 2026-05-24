import type { QuotaWindowUsageSnapshot } from '@/api/endpoints/types/statusSnapshot'
import type { PoolManagementStatsMode } from '@/features/pool/utils/poolManagementState'
import { formatCompactNumber } from '@/utils/format'

export type PoolStatsMetricKey = 'request_count' | 'total_tokens' | 'total_cost_usd'
export type PoolStatsDisplayKind = 'account_total' | 'codex_cycle'
export type PoolCodexCycleWindowCode = '5h' | 'weekly'

export interface PoolStatsKeyInput {
  request_count?: number | null
  total_tokens?: number | null
  total_cost_usd?: number | string | null
  status_snapshot?: {
    quota?: {
      windows?: Array<{
        code?: string | null
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
const CODEX_CYCLE_WINDOWS: Array<{ code: PoolCodexCycleWindowCode, label: string }> = [
  { code: '5h', label: '5H' },
  { code: 'weekly', label: '周' },
]

export function isCodexProviderType(providerType: string | null | undefined): boolean {
  return String(providerType || '').trim().toLowerCase() === 'codex'
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
): PoolStatsMetric {
  return {
    key,
    label,
    value: value ?? MISSING_STAT_VALUE,
    missing: value == null,
  }
}

function normalizeWindowCode(value: unknown): string {
  return String(value || '').trim().toLowerCase()
}

function getQuotaWindowUsage(
  key: PoolStatsKeyInput,
  code: PoolCodexCycleWindowCode,
): QuotaWindowUsageSnapshot | null {
  const windows = key.status_snapshot?.quota?.windows
  if (!Array.isArray(windows)) return null

  const window = windows.find(item => normalizeWindowCode(item?.code) === code)
  return window?.usage ?? null
}

function buildAccountTotalMetrics(key: PoolStatsKeyInput): PoolStatsMetric[] {
  return [
    createMetric('request_count', '请求', formatPoolStatInteger(key.request_count)),
    createMetric('total_tokens', 'Token', formatPoolTokenCount(key.total_tokens)),
    createMetric('total_cost_usd', '费用', formatPoolStatUsd(key.total_cost_usd)),
  ]
}

function buildCycleMetrics(usage: QuotaWindowUsageSnapshot | null): PoolStatsMetric[] {
  return [
    createMetric('request_count', '请求', formatCycleInteger(usage?.request_count)),
    createMetric('total_tokens', 'Token', formatCycleTokenCount(usage?.total_tokens)),
    createMetric('total_cost_usd', '费用', formatCycleUsd(usage?.total_cost_usd)),
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
    groups: CODEX_CYCLE_WINDOWS.map(window => ({
      ...window,
      metrics: buildCycleMetrics(getQuotaWindowUsage(key, window.code)),
    })),
  }
}

export function buildPoolStatsDisplay(
  key: PoolStatsKeyInput,
  providerType: string | null | undefined,
  mode: PoolManagementStatsMode,
): PoolStatsDisplay {
  if (isCodexProviderType(providerType) && mode === 'current_cycle') {
    return buildCodexCycleStatsDisplay(key)
  }

  return buildAccountTotalStatsDisplay(key)
}
