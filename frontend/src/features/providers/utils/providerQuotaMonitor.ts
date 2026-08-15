import type { ProviderWithEndpointsSummary } from '@/api/endpoints/types/provider'
import type { Sub2ApiRemoteQuotaGroup } from '@/api/providerOps'

export type ProviderQuotaMonitor =
  | {
      source: 'remote_subscription'
      state: 'limited'
      limitUsd: number
      usedUsd: number
      remainingUsd: number
    }
  | {
      source: 'remote_subscription'
      state: 'unlimited' | 'pending' | 'exhausted'
    }

function finiteNonNegative(value: number | undefined): number | null {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0 ? value : null
}

function remoteGroupIsUnlimited(group: Sub2ApiRemoteQuotaGroup | null): boolean {
  if (!group) return false
  return group.daily_limit_usd <= 0
    && group.weekly_limit_usd <= 0
    && group.monthly_limit_usd <= 0
}

function appliedLocalSnapshotIsCurrent(
  provider: ProviderWithEndpointsSummary,
  group: Sub2ApiRemoteQuotaGroup | null,
): boolean {
  if (group?.sync_status !== 'applied') return false
  const syncTime = Date.parse(group.sync_executed_at ?? '')
  const providerTime = Date.parse(provider.updated_at)
  return !Number.isFinite(syncTime)
    || !Number.isFinite(providerTime)
    || syncTime >= providerTime
}

export function providerQuotaMonitor(
  provider: ProviderWithEndpointsSummary,
  remoteGroup: Sub2ApiRemoteQuotaGroup | null = null,
): ProviderQuotaMonitor | null {
  const remoteSubscription = provider.ops_remote_quota_enabled === true
  if (!remoteSubscription) return null

  const appliedLocal = appliedLocalSnapshotIsCurrent(provider, remoteGroup)
    ? remoteGroup
    : null
  const billingType = appliedLocal?.local_billing_type ?? provider.billing_type
  if (billingType === 'monthly_quota') {
    const limitUsd = finiteNonNegative(
      appliedLocal?.local_monthly_quota_usd ?? provider.monthly_quota_usd,
    )
    const usedUsd = finiteNonNegative(
      appliedLocal?.local_monthly_used_usd ?? provider.monthly_used_usd,
    )
    if (limitUsd !== null && usedUsd !== null) {
      if (limitUsd === 0 || usedUsd >= limitUsd) {
        return { source: 'remote_subscription', state: 'exhausted' }
      }
      return {
        source: 'remote_subscription',
        state: 'limited',
        limitUsd,
        usedUsd,
        remainingUsd: limitUsd - usedUsd,
      }
    }
  }

  return remoteGroupIsUnlimited(remoteGroup)
    ? { source: 'remote_subscription', state: 'unlimited' }
    : { source: 'remote_subscription', state: 'pending' }
}
