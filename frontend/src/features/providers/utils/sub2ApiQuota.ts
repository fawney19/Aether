import type {
  Sub2ApiQuotaWindow,
  Sub2ApiRemoteQuotaGroup,
} from '@/api/providerOps'

export const sub2ApiQuotaWindows: ReadonlyArray<{
  key: Sub2ApiQuotaWindow
  label: string
}> = [
  { key: 'daily', label: '日' },
  { key: 'weekly', label: '周' },
  { key: 'monthly', label: '月' },
]

function finiteNonNegative(value: unknown): number {
  const parsed = Number(value)
  return Number.isFinite(parsed) && parsed >= 0 ? parsed : 0
}

export function sub2ApiWindowValues(
  group: Sub2ApiRemoteQuotaGroup,
  window: Sub2ApiQuotaWindow,
): { used: number; limit: number } {
  switch (window) {
    case 'daily':
      return {
        used: finiteNonNegative(group.daily_used_usd),
        limit: finiteNonNegative(group.daily_limit_usd),
      }
    case 'weekly':
      return {
        used: finiteNonNegative(group.weekly_used_usd),
        limit: finiteNonNegative(group.weekly_limit_usd),
      }
    case 'monthly':
      return {
        used: finiteNonNegative(group.monthly_used_usd),
        limit: finiteNonNegative(group.monthly_limit_usd),
      }
  }
}

function formatMoney(value: number): string {
  return `$${value.toLocaleString('en-US', { maximumFractionDigits: 2 })}`
}

export function formatSub2ApiWindow(
  group: Sub2ApiRemoteQuotaGroup,
  window: Sub2ApiQuotaWindow,
): string {
  const { used, limit } = sub2ApiWindowValues(group, window)
  return limit <= 0 ? '不限' : `${formatMoney(used)} / ${formatMoney(limit)}`
}

export function localSyncWindowText(window: Sub2ApiQuotaWindow | null): string {
  const label = sub2ApiQuotaWindows.find((item) => item.key === window)?.label
  return label ? `${label}额度` : '不限额'
}

export function formatSub2ApiGroupOption(group: Sub2ApiRemoteQuotaGroup): string {
  const name = group.group_name.trim() || '未命名套餐'
  return `${name} · ${localSyncWindowText(group.local_sync_window)}`
}
