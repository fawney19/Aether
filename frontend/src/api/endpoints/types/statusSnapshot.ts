export interface OAuthStatusSnapshot {
  code: 'none' | 'valid' | 'expiring' | 'expired' | 'invalid' | 'reauth_required' | 'check_failed'
  label?: string | null
  reason?: string | null
  expires_at?: number | null
  invalid_at?: number | null
  source?: string | null
  requires_reauth?: boolean
  usable_until_expiry?: boolean
  expiring_soon?: boolean
}

export interface AccountStatusSnapshot {
  code: string
  label?: string | null
  reason?: string | null
  blocked: boolean
  source?: string | null
  recoverable?: boolean
}

export interface QuotaWindowUsageSnapshot {
  request_count?: number | null
  total_tokens?: number | null
  total_cost_usd?: number | string | null
}

export interface QuotaWindowSnapshot {
  code: string
  label?: string | null
  scope?: 'account' | 'workspace' | 'model' | string
  unit?: 'percent' | 'count' | 'usd' | 'tokens' | string
  model?: string | null
  used_ratio?: number | null
  remaining_ratio?: number | null
  used_value?: number | null
  remaining_value?: number | null
  limit_value?: number | null
  reset_at?: number | null
  reset_seconds?: number | null
  window_minutes?: number | null
  is_exhausted?: boolean | null
  usage?: QuotaWindowUsageSnapshot | null
}

export interface QuotaCreditsSnapshot {
  has_credits?: boolean | null
  balance?: number | null
  unlimited?: boolean | null
}

export interface QuotaStatusSnapshot {
  version?: number | null
  provider_type?: string | null
  code: 'unknown' | 'ok' | 'exhausted' | 'cooldown' | 'forbidden' | 'banned' | string
  label?: string | null
  reason?: string | null
  freshness?: 'fresh' | 'stale' | 'unknown' | 'error' | string | null
  source?: string | null
  observed_at?: number | null
  exhausted: boolean
  usage_ratio?: number | null
  updated_at?: number | null
  reset_at?: number | null
  reset_seconds?: number | null
  plan_type?: string | null
  pool_tier?: string | null
  credits?: QuotaCreditsSnapshot | null
  allowed_models_count?: number | null
  rate_limit?: Record<string, unknown> | null
  windows?: QuotaWindowSnapshot[] | null
}

export interface ProviderKeyStatusSnapshot {
  oauth: OAuthStatusSnapshot
  account: AccountStatusSnapshot
  quota: QuotaStatusSnapshot
}
