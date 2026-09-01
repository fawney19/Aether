import { describe, expect, it } from 'vitest'

import type { ProviderWithEndpointsSummary } from '@/api/endpoints/types/provider'
import { providerQuotaMonitor } from '@/features/providers/utils/providerQuotaMonitor'

function provider(overrides: Partial<ProviderWithEndpointsSummary>): ProviderWithEndpointsSummary {
  return {
    id: 'provider-1',
    name: 'Provider',
    provider_priority: 0,
    keep_priority_on_conversion: false,
    enable_format_conversion: false,
    is_active: true,
    total_endpoints: 1,
    active_endpoints: 1,
    total_keys: 1,
    active_keys: 1,
    total_models: 1,
    active_models: 1,
    global_model_ids: [],
    avg_health_score: 1,
    unhealthy_endpoints: 0,
    api_formats: ['openai:chat'],
    endpoint_health_details: [],
    ops_configured: true,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    ...overrides,
  }
}

describe('providerQuotaMonitor', () => {
  it('shows the locally enforced subscription quota instead of account balance', () => {
    expect(providerQuotaMonitor(provider({
      ops_remote_quota_enabled: true,
      billing_type: 'monthly_quota',
      monthly_quota_usd: 35,
      monthly_used_usd: 5,
    }))).toEqual({
      source: 'remote_subscription',
      state: 'limited',
      limitUsd: 35,
      usedUsd: 5,
      remainingUsd: 30,
    })
  })

  it('prefers the applied local snapshot over a stale provider-list row', () => {
    expect(providerQuotaMonitor(provider({
      ops_remote_quota_enabled: true,
      billing_type: 'monthly_quota',
      monthly_quota_usd: 100,
      monthly_used_usd: 25,
    }), {
      group_id: '3',
      group_name: 'Daily',
      subscription_id: '9',
      daily_limit_usd: 10,
      daily_used_usd: 2,
      weekly_limit_usd: 50,
      weekly_used_usd: 8,
      monthly_limit_usd: 100,
      monthly_used_usd: 25,
      local_sync_window: 'daily',
      expires_at_unix_secs: null,
      sync_status: 'applied',
      sync_executed_at: '2026-01-02T00:00:00Z',
      local_billing_type: 'monthly_quota',
      local_monthly_quota_usd: 10,
      local_monthly_used_usd: 2.5,
    })).toEqual({
      source: 'remote_subscription',
      state: 'limited',
      limitUsd: 10,
      usedUsd: 2.5,
      remainingUsd: 7.5,
    })
  })

  it('keeps a provider row that is newer than the cached applied snapshot', () => {
    expect(providerQuotaMonitor(provider({
      ops_remote_quota_enabled: true,
      billing_type: 'monthly_quota',
      monthly_quota_usd: 10,
      monthly_used_usd: 4,
      updated_at: '2026-01-03T00:00:00Z',
    }), {
      group_id: '3',
      group_name: 'Daily',
      subscription_id: '9',
      daily_limit_usd: 10,
      daily_used_usd: 2,
      weekly_limit_usd: 50,
      weekly_used_usd: 8,
      monthly_limit_usd: 100,
      monthly_used_usd: 25,
      local_sync_window: 'daily',
      expires_at_unix_secs: null,
      sync_status: 'applied',
      sync_executed_at: '2026-01-02T00:00:00Z',
      local_billing_type: 'monthly_quota',
      local_monthly_quota_usd: 10,
      local_monthly_used_usd: 2.5,
    })).toEqual({
      source: 'remote_subscription',
      state: 'limited',
      limitUsd: 10,
      usedUsd: 4,
      remainingUsd: 6,
    })
  })

  it('does not mistake an unsynced subscription for wallet balance', () => {
    expect(providerQuotaMonitor(provider({
      ops_remote_quota_enabled: true,
      billing_type: 'pay_as_you_go',
    }))).toEqual({ source: 'remote_subscription', state: 'pending' })
  })

  it('recognizes an unlimited subscription from its cached snapshot', () => {
    expect(providerQuotaMonitor(provider({
      ops_remote_quota_enabled: true,
      billing_type: 'pay_as_you_go',
    }), {
      group_id: '3',
      group_name: 'Unlimited',
      subscription_id: '9',
      daily_limit_usd: 0,
      daily_used_usd: 0,
      weekly_limit_usd: 0,
      weekly_used_usd: 0,
      monthly_limit_usd: 0,
      monthly_used_usd: 0,
      local_sync_window: null,
      expires_at_unix_secs: null,
    })).toEqual({ source: 'remote_subscription', state: 'unlimited' })
  })

  it('leaves non-remote providers on the existing balance and local quota path', () => {
    expect(providerQuotaMonitor(provider({
      billing_type: 'monthly_quota',
      monthly_quota_usd: 10,
      monthly_used_usd: 10,
    }))).toBeNull()
  })
})
