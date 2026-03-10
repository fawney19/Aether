import client from '../client'
import { dedupedRequest } from '@/utils/cache'
import type { AllowedModels, ProxyConfig } from './types/provider'

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
  oauth_expires_at?: number | null
  oauth_invalid_at?: number | null
  oauth_invalid_reason?: string | null
  oauth_plan_type?: string | null
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
  account_quota: string | null
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
}

export interface PoolBatchAction {
  key_ids: string[]
  action:
    | 'enable'
    | 'disable'
    | 'delete'
    | 'clear_cooldown'
    | 'reset_cost'
    | 'regenerate_fingerprint'
    | 'clear_proxy'
    | 'set_proxy'
  payload?: Record<string, unknown> | null
}

export async function getPoolOverview(): Promise<PoolOverviewResponse> {
  return dedupedRequest('pool:overview', async () => {
    const response = await client.get<PoolOverviewResponse>('/api/admin/pool/overview')
    return response.data
  })
}

export async function getPoolSchedulingPresets(): Promise<PoolPresetMeta[]> {
  return dedupedRequest('pool:scheduling-presets', async () => {
    const response = await client.get<PoolPresetMeta[]>('/api/admin/pool/scheduling-presets')
    return response.data
  })
}

export async function listPoolKeys(
  providerId: string,
  params: PoolKeysQuery = {},
): Promise<PoolKeysPageResponse> {
  const key = `pool:keys:${providerId}|${params.page ?? ''}|${params.page_size ?? ''}|${params.search ?? ''}|${params.status ?? ''}`
  return dedupedRequest(key, async () => {
    const response = await client.get<PoolKeysPageResponse>(`/api/admin/pool/${providerId}/keys`, { params })
    return response.data
  })
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
