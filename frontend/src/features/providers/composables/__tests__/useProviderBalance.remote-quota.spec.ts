import { describe, expect, it } from 'vitest'
import type { ActionResultResponse, RemoteQuotaSyncStatus } from '@/api/providerOps'
import { parseProviderRemoteQuotaGroup } from '../useProviderBalance'

function resultWithSubscription(
  subscription: Record<string, unknown>,
  status: RemoteQuotaSyncStatus = 'applied',
  message?: string,
  syncExtra: Record<string, unknown> = {},
): ActionResultResponse {
  return {
    status: 'success',
    action_type: 'query_balance',
    data: {
      total_available: 8.5,
      currency: 'USD',
      extra: {
        remote_quota_sync: {
          status,
          message,
          subscription,
          ...syncExtra,
        },
      },
    },
    message: null,
    executed_at: '2030-01-30T00:00:00Z',
    response_time_ms: 10,
    cache_ttl_seconds: 86400,
  }
}

describe('provider balance remote quota snapshot', () => {
  it('extracts all daily, weekly, and monthly windows from cached balance data', () => {
    const parsed = parseProviderRemoteQuotaGroup(resultWithSubscription({
      group_id: '42',
      group_name: 'Pro',
      subscription_id: '9',
      daily_limit_usd: 10,
      daily_used_usd: 2,
      weekly_limit_usd: 50,
      weekly_used_usd: 8,
      monthly_limit_usd: 0,
      monthly_used_usd: 0,
      local_sync_window: 'daily',
      expires_at_unix_secs: 1_896_134_400,
    }, 'applied', undefined, {
      local: {
        billing_type: 'monthly_quota',
        monthly_quota_usd: 10,
        monthly_used_usd: 2.5,
        remote_confirmed_used_usd: 2.25,
      },
    }))

    expect(parsed).toMatchObject({
      group_id: '42',
      daily_limit_usd: 10,
      daily_used_usd: 2,
      weekly_limit_usd: 50,
      weekly_used_usd: 8,
      monthly_limit_usd: 0,
      local_sync_window: 'daily',
      sync_status: 'applied',
      sync_message: null,
      sync_executed_at: '2030-01-30T00:00:00Z',
      local_billing_type: 'monthly_quota',
      local_monthly_quota_usd: 10,
      local_monthly_used_usd: 2.5,
      remote_confirmed_used_usd: 2.25,
    })
  })

  it('keeps failed apply state attached to a valid remote subscription snapshot', () => {
    const result = resultWithSubscription({
      group_id: '42',
      group_name: 'Pro',
      subscription_id: '9',
      daily_limit_usd: 10,
      daily_used_usd: 2,
      weekly_limit_usd: 50,
      weekly_used_usd: 8,
      monthly_limit_usd: 0,
      monthly_used_usd: 0,
      local_sync_window: 'daily',
      expires_at_unix_secs: null,
    }, 'failed_keep_local', 'progress 数据缺失')

    expect(parseProviderRemoteQuotaGroup(result)).toMatchObject({
      group_id: '42',
      sync_status: 'failed_keep_local',
      sync_message: 'progress 数据缺失',
    })
  })

  it('rejects incomplete cached snapshots instead of showing misleading zeros', () => {
    const parsed = parseProviderRemoteQuotaGroup(resultWithSubscription({
      group_id: '42',
      subscription_id: '9',
      daily_limit_usd: 10,
    }))

    expect(parsed).toBeNull()
  })
})
