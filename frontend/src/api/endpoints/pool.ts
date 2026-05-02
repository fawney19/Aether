import client from '../client'
import { buildCacheKey, cachedRequest } from '@/utils/cache'
import type {
  AllowedModels,
  OAuthOrganizationInfo,
  ProxyConfig,
  UpstreamMetadata,
} from './types/provider'
import type { ProviderKeyStatusSnapshot } from './types/statusSnapshot'

const POOL_BATCH_ACTION_TIMEOUT_MS = 5 * 60 * 1000

export interface PoolKeyStatus {
  key_id: string
  key_name: string
  is_active: boolean
  cooldown_reason: string | null
  cooldown_ttl_seconds: number | null
  cost_window_usage: number
  cost_limit: number | null
  sticky_sessions: number
  lru_score: number | null
}

export interface PoolStatusResponse {
  provider_id: string
  provider_name: string
  pool_enabled: boolean
  total_keys: number
  total_sticky_sessions: number
  keys: PoolKeyStatus[]
}

/**
 * 获取 Provider 的号池状态
 */
export async function getPoolStatus(providerId: string): Promise<PoolStatusResponse> {
  const response = await client.get<PoolStatusResponse>(`/api/admin/providers/${providerId}/pool-status`)
  return response.data
}

/**
 * 清除指定 Key 的号池冷却状态
 */
export async function clearPoolCooldown(
  providerId: string,
  keyId: string,
): Promise<{ message: string }> {
  const response = await client.post<{ message: string }>(
    `/api/admin/providers/${providerId}/pool/clear-cooldown/${keyId}`,
  )
  return response.data
}

/**
 * 重置指定 Key 的号池成本窗口
 */
export async function resetPoolCost(
  providerId: string,
  keyId: string,
): Promise<{ message: string }> {
  const response = await client.post<{ message: string }>(
    `/api/admin/providers/${providerId}/pool/reset-cost/${keyId}`,
  )
  return response.data
}

// ---------------------------------------------------------------------------
// Pool management API (standalone page)
// ---------------------------------------------------------------------------

export interface PoolOverviewItem {
  provider_id: string
  provider_name: string
  provider_type: string
  total_keys: number
  active_keys: number
  cooldown_count: number
  pool_enabled: boolean
}

export interface PoolOverviewResponse {
  items: PoolOverviewItem[]
}

export interface PoolPresetModeMeta {
  value: string
  label: string
}

export interface PoolPresetMeta {
  name: string
  label: string
  description: string
  providers: string[]
  modes?: PoolPresetModeMeta[] | null
  default_mode?: string | null
  mutex_group?: string | null
  evidence_hint?: string | null
}

export interface PoolKeyDetail {
  key_id: string
  key_name: string
  is_active: boolean
  auth_type: string
  auth_type_by_format?: Record<string, 'api_key' | 'bearer'> | null
  allow_auth_channel_mismatch_formats?: string[] | null
  credential_kind?: 'raw_secret' | 'oauth_session' | 'service_account' | string | null
  runtime_auth_kind?: 'api_key' | 'bearer' | 'service_account' | 'mixed' | 'unknown' | string | null
  oauth_managed?: boolean
  can_refresh_oauth?: boolean
  can_export_oauth?: boolean
  can_edit_oauth?: boolean
  oauth_expires_at?: number | null
  oauth_invalid_at?: number | null  // 兼容字段；优先使用 status_snapshot.oauth
  oauth_invalid_reason?: string | null  // 兼容字段；优先使用 status_snapshot.oauth
  oauth_plan_type?: string | null
  oauth_account_id?: string | null
  oauth_account_user_id?: string | null
  oauth_account_name?: string | null
  oauth_organizations?: OAuthOrganizationInfo[] | null
  oauth_temporary?: boolean | null
  account_status_code?: string | null  // 兼容字段；优先使用 status_snapshot.account
  account_status_label?: string | null  // 兼容字段；优先使用 status_snapshot.account
  account_status_reason?: string | null  // 兼容字段；优先使用 status_snapshot.account
  account_status_blocked?: boolean  // 兼容字段；优先使用 status_snapshot.account
  account_status_recoverable?: boolean  // 兼容字段；优先使用 status_snapshot.account
  account_status_source?: string | null  // 兼容字段；优先使用 status_snapshot.account
  status_snapshot?: ProviderKeyStatusSnapshot | null
  upstream_metadata?: UpstreamMetadata | null
  quota_updated_at?: number | null
  health_score?: number
  circuit_breaker_open?: boolean
  api_formats?: string[]
  rate_multipliers?: Record<string, number> | null
  internal_priority?: number
  rpm_limit?: number | null
  cache_ttl_minutes?: number
  max_probe_interval_minutes?: number
  note?: string | null
  allowed_models?: AllowedModels
  capabilities?: Record<string, boolean> | null
  auto_fetch_models?: boolean
  locked_models?: string[] | null
  model_include_patterns?: string[] | null
  model_exclude_patterns?: string[] | null
  proxy?: ProxyConfig | null
  account_quota: string | null  // compatibility only; UI should prefer status_snapshot.quota
  cooldown_reason: string | null
  cooldown_ttl_seconds: number | null
  cost_window_usage: number
  cost_limit: number | null
  request_count: number
  total_tokens: number
  total_cost_usd: string
  sticky_sessions: number
  lru_score: number | null
  created_at: string | null
  imported_at?: string | null
  last_used_at: string | null
  scheduling_status?: 'available' | 'degraded' | 'blocked'
  scheduling_reason?:
    | 'available'
    | 'manual_disabled'
    | 'cooldown'
    | 'circuit_open'
    | 'cost_exhausted'
    | 'cost_soft'
    | 'cost'
    | 'health_low'
    | 'health_degraded'
    | 'health'
    | string
  scheduling_label?: string
  scheduling_reasons?: PoolSchedulingReason[]
}

export interface PoolSchedulingReason {
  code: string
  label: string
  blocking: boolean
  source: 'manual' | 'pool' | 'health' | 'policy' | string
  ttl_seconds?: number | null
  detail?: string | null
}

export interface PoolKeysPageResponse {
  total: number
  page: number
  page_size: number
  keys: PoolKeyDetail[]
}

export interface PoolKeysQuery {
  page?: number
  page_size?: number
  search?: string
  status?: 'all' | 'active' | 'cooldown' | 'inactive'
  quick_selectors?: string[]
  search_scope?: 'name' | 'full'
  sort_by?: 'imported_at' | 'last_used_at'
  sort_order?: 'asc' | 'desc'
}

export interface PoolKeySelectionRequest {
  search?: string
  quick_selectors?: string[]
}

export interface PoolKeySelectionItem {
  key_id: string
  key_name: string
  auth_type: string
  auth_type_by_format?: Record<string, 'api_key' | 'bearer'> | null
  allow_auth_channel_mismatch_formats?: string[] | null
  credential_kind?: 'raw_secret' | 'oauth_session' | 'service_account' | string | null
  runtime_auth_kind?: 'api_key' | 'bearer' | 'service_account' | 'mixed' | 'unknown' | string | null
  oauth_managed?: boolean
  can_refresh_oauth?: boolean
  can_export_oauth?: boolean
  can_edit_oauth?: boolean
}

export interface PoolKeySelectionResponse {
  total: number
  items: PoolKeySelectionItem[]
}

export interface PoolBatchAction {
  key_ids: string[]
  action:
    | 'enable'
    | 'disable'
    | 'delete'
    | 'clear_proxy'
    | 'set_proxy'
  payload?: Record<string, unknown> | null
}

interface PoolReadOptions {
  cacheTtlMs?: number
}

export async function getPoolOverview(
  options: PoolReadOptions = {},
): Promise<PoolOverviewResponse> {
  const cacheTtlMs = options.cacheTtlMs ?? 0
  return cachedRequest(
    'pool:overview',
    async () => {
      const response = await client.get<PoolOverviewResponse>('/api/admin/pool/overview')
      return response.data
    },
    cacheTtlMs,
  )
}

export async function getPoolSchedulingPresets(
  options: PoolReadOptions = {},
): Promise<PoolPresetMeta[]> {
  const cacheTtlMs = options.cacheTtlMs ?? 0
  return cachedRequest(
    'pool:scheduling-presets',
    async () => {
      const response = await client.get<PoolPresetMeta[]>('/api/admin/pool/scheduling-presets')
      return response.data
    },
    cacheTtlMs,
  )
}

export async function listPoolKeys(
  providerId: string,
  params: PoolKeysQuery = {},
  options: PoolReadOptions = {},
): Promise<PoolKeysPageResponse> {
  const normalizedParams = {
    ...params,
    quick_selectors: params.quick_selectors?.length ? params.quick_selectors.join(',') : undefined,
  }
  const cacheKey = buildCacheKey(
    `pool:keys:${providerId}`,
    normalizedParams as Record<string, unknown>,
  )
  return cachedRequest(
    cacheKey,
    async () => {
      const response = await client.get<PoolKeysPageResponse>(`/api/admin/pool/${providerId}/keys`, { params: normalizedParams })
      return response.data
    },
    options.cacheTtlMs ?? 0,
  )
}

export async function resolvePoolKeySelection(
  providerId: string,
  body: PoolKeySelectionRequest,
): Promise<PoolKeySelectionResponse> {
  const response = await client.post<PoolKeySelectionResponse>(
    `/api/admin/pool/${providerId}/keys/resolve-selection`,
    body,
    { timeout: POOL_BATCH_ACTION_TIMEOUT_MS },
  )
  return response.data
}

export async function batchActionPoolKeys(
  providerId: string,
  body: PoolBatchAction,
): Promise<{ affected: number; message: string; task_id?: string }> {
  const response = await client.post(
    `/api/admin/pool/${providerId}/keys/batch-action`,
    body,
    { timeout: POOL_BATCH_ACTION_TIMEOUT_MS },
  )
  return response.data
}

export interface BatchDeleteTaskStatus {
  task_id: string
  status: 'pending' | 'running' | 'completed' | 'failed'
  total: number
  deleted: number
  message: string
}

export async function getPoolBatchDeleteTask(
  providerId: string,
  taskId: string,
): Promise<BatchDeleteTaskStatus> {
  const response = await client.get<BatchDeleteTaskStatus>(
    `/api/admin/pool/${providerId}/keys/batch-delete-task/${taskId}`,
  )
  return response.data
}

export async function cleanupBannedPoolKeys(
  providerId: string,
): Promise<{ affected: number; message: string }> {
  const response = await client.post(
    `/api/admin/pool/${providerId}/keys/cleanup-banned`,
    undefined,
    { timeout: POOL_BATCH_ACTION_TIMEOUT_MS },
  )
  return response.data
}
