import client from '../client'

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
  const response = await client.get(`/api/admin/providers/${providerId}/pool-status`)
  return response.data
}

/**
 * 清除指定 Key 的号池冷却状态
 */
export async function clearPoolCooldown(
  providerId: string,
  keyId: string,
): Promise<{ message: string }> {
  const response = await client.post(
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
  const response = await client.post(
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

export interface PoolKeyDetail {
  key_id: string
  key_name: string
  is_active: boolean
  auth_type: string
  oauth_plan_type: string | null
  account_quota: string | null
  cooldown_reason: string | null
  cooldown_ttl_seconds: number | null
  cost_window_usage: number
  cost_limit: number | null
  sticky_sessions: number
  lru_score: number | null
  created_at: string | null
  last_used_at: string | null
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

export interface PoolKeyImportItem {
  name: string
  api_key: string
  auth_type?: string
}

export interface BatchImportResponse {
  imported: number
  skipped: number
  errors: { index: number; reason: string }[]
}

export interface PoolBatchAction {
  key_ids: string[]
  action: 'enable' | 'disable' | 'delete' | 'clear_cooldown' | 'reset_cost'
}

export async function getPoolOverview(): Promise<PoolOverviewResponse> {
  const response = await client.get('/api/admin/pool/overview')
  return response.data
}

export async function listPoolKeys(
  providerId: string,
  params: PoolKeysQuery = {},
): Promise<PoolKeysPageResponse> {
  const response = await client.get(`/api/admin/pool/${providerId}/keys`, { params })
  return response.data
}

export async function batchImportPoolKeys(
  providerId: string,
  keys: PoolKeyImportItem[],
): Promise<BatchImportResponse> {
  const response = await client.post(`/api/admin/pool/${providerId}/keys/batch-import`, { keys })
  return response.data
}

export async function batchActionPoolKeys(
  providerId: string,
  body: PoolBatchAction,
): Promise<{ affected: number; message: string }> {
  const response = await client.post(`/api/admin/pool/${providerId}/keys/batch-action`, body)
  return response.data
}
