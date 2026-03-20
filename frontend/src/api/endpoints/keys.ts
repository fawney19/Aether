import client from '../client'
import type { EndpointAPIKey, AllowedModels } from './types'

// Re-export types for convenience
export type { EndpointAPIKey, AllowedModels }

export interface GroupedFormatKey {
  id: string
  provider_id: string
  name: string
  auth_type?: string
  api_key_masked: string
  internal_priority: number
  global_priority_by_format: Record<string, number> | null
  format_priority: number | null
  rate_multipliers: Record<string, number> | null
  is_active: boolean
  provider_active: boolean
  pool_enabled: boolean
  circuit_breaker_open: boolean
  provider_name: string
  api_format: string
  api_formats: string[]
  capabilities: string[]
  health_score: number | null
  success_rate: number | null
  avg_response_time_ms: number | null
  request_count: number
}

function toNumberOrNull(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

/**
 * 能力定义类型
 */
export interface CapabilityDefinition {
  name: string
  display_name: string
  description: string
  match_mode: 'exclusive' | 'compatible'
  config_mode?: 'user_configurable' | 'auto_detect' | 'request_param'
  short_name?: string
}

/**
 * 模型支持的能力响应类型
 */
export interface ModelCapabilitiesResponse {
  model: string
  global_model_id?: string
  global_model_name?: string
  supported_capabilities: string[]
  capability_details: CapabilityDefinition[]
  error?: string
}

/**
 * 获取所有能力定义
 */
export async function getAllCapabilities(): Promise<CapabilityDefinition[]> {
  const response = await client.get('/api/capabilities')
  return response.data.capabilities
}

/**
 * 获取用户可配置的能力列表
 */
export async function getUserConfigurableCapabilities(): Promise<CapabilityDefinition[]> {
  const response = await client.get('/api/capabilities/user-configurable')
  return response.data.capabilities
}

/**
 * 获取指定模型支持的能力列表
 */
export async function getModelCapabilities(modelName: string): Promise<ModelCapabilitiesResponse> {
  const response = await client.get(`/api/capabilities/model/${encodeURIComponent(modelName)}`)
  return response.data
}

/**
 * 获取完整的 API Key（用于查看和复制）
 */
export interface RevealKeyResult {
  auth_type: 'api_key' | 'service_account' | 'oauth'
  api_key?: string
  refresh_token?: string
  auth_config?: string | Record<string, unknown>
}

export async function revealEndpointKey(keyId: string): Promise<RevealKeyResult> {
  const response = await client.get(`/api/admin/endpoints/keys/${keyId}/reveal`)
  return response.data
}

/**
 * 导出 OAuth Key 凭据（扁平 JSON，用于跨实例迁移）
 */
export async function exportKey(keyId: string): Promise<Record<string, unknown>> {
  const response = await client.get(`/api/admin/endpoints/keys/${keyId}/export`)
  return response.data
}

/**
 * 删除 Key
 */
export async function deleteEndpointKey(keyId: string): Promise<{ message: string }> {
  const response = await client.delete(`/api/admin/endpoints/keys/${keyId}`)
  return response.data
}

/**
 * 批量删除 Keys
 */
export interface BatchDeleteKeysResult {
  success_count: number
  failed_count: number
  failed: Array<{ id: string; error: string }>
}

export async function batchDeleteEndpointKeys(ids: string[]): Promise<BatchDeleteKeysResult> {
  const response = await client.post('/api/admin/endpoints/keys/batch-delete', { ids })
  return response.data
}


// ========== Provider 级别的 Keys API ==========


/**
 * 获取 Provider 的所有 Keys
 */
export async function getProviderKeys(providerId: string): Promise<EndpointAPIKey[]> {
  // 后端默认 limit=100，这里主动分页拉取，避免账号数 >100 时前端被截断
  const pageSize = 1000
  let skip = 0
  const allKeys: EndpointAPIKey[] = []

  while (true) {
    const response = await client.get(`/api/admin/endpoints/providers/${providerId}/keys`, {
      params: { skip, limit: pageSize },
    })

    const batch = Array.isArray(response.data) ? (response.data as EndpointAPIKey[]) : []
    allKeys.push(...batch)

    if (batch.length < pageSize) break
    skip += pageSize
  }

  return allKeys
}

/**
 * 获取按 API 格式分组的 Key 列表
 */
export async function getKeysGroupedByFormat(): Promise<Record<string, GroupedFormatKey[]>> {
  const response = await client.get('/api/admin/endpoints/keys/grouped-by-format')
  const grouped = response.data as Record<string, Array<Record<string, unknown>>>
  const result: Record<string, GroupedFormatKey[]> = {}

  for (const [apiFormat, keys] of Object.entries(grouped || {})) {
    if (!Array.isArray(keys)) continue

    result[apiFormat] = keys.map((key) => ({
      id: String(key.id || ''),
      provider_id: String(key.provider_id || ''),
      name: String(key.name || 'Unnamed Key'),
      auth_type: typeof key.auth_type === 'string' ? key.auth_type : undefined,
      api_key_masked: String(key.api_key_masked || '***'),
      internal_priority: toNumberOrNull(key.internal_priority) ?? 0,
      global_priority_by_format:
        key.global_priority_by_format && typeof key.global_priority_by_format === 'object'
          ? (key.global_priority_by_format as Record<string, number>)
          : null,
      format_priority: toNumberOrNull(key.format_priority),
      rate_multipliers:
        key.rate_multipliers && typeof key.rate_multipliers === 'object'
          ? (key.rate_multipliers as Record<string, number>)
          : null,
      is_active: key.is_active !== false,
      provider_active: key.provider_active !== false,
      pool_enabled: key.pool_enabled === true,
      circuit_breaker_open: key.circuit_breaker_open === true,
      provider_name: String(key.provider_name || 'Unknown Provider'),
      api_format: typeof key.api_format === 'string' ? key.api_format : apiFormat,
      api_formats: Array.isArray(key.api_formats) ? key.api_formats.map(item => String(item)) : [apiFormat],
      capabilities: Array.isArray(key.capabilities) ? key.capabilities.map(item => String(item)) : [],
      health_score: toNumberOrNull(key.health_score),
      success_rate: toNumberOrNull(key.success_rate),
      avg_response_time_ms: toNumberOrNull(key.avg_response_time_ms),
      request_count: toNumberOrNull(key.request_count) ?? 0,
    }))
  }

  return result
}

/**
 * 为 Provider 添加 Key
 */
export async function addProviderKey(
  providerId: string,
  data: {
    api_formats: string[]  // 支持的 API 格式列表（必填）
    api_key: string
    auth_type?: 'api_key' | 'service_account' | 'oauth'  // 认证类型
    auth_config?: Record<string, unknown>  // 认证配置（Vertex AI Service Account JSON）
    name: string
    rate_multipliers?: Record<string, number> | null  // 按 API 格式的成本倍率
    internal_priority?: number
    rpm_limit?: number | null  // RPM 限制（留空=自适应模式）
    cache_ttl_minutes?: number
    max_probe_interval_minutes?: number
    allowed_models?: AllowedModels
    capabilities?: Record<string, boolean>
    note?: string
    auto_fetch_models?: boolean  // 是否启用自动获取模型
    model_include_patterns?: string[]  // 模型包含规则
    model_exclude_patterns?: string[]  // 模型排除规则
  }
): Promise<EndpointAPIKey> {
  const response = await client.post(`/api/admin/endpoints/providers/${providerId}/keys`, data)
  return response.data
}

/**
 * 更新 Key
 */
export async function updateProviderKey(
  keyId: string,
  data: Partial<{
    api_formats: string[]  // 支持的 API 格式列表
    api_key: string
    auth_type: 'api_key' | 'service_account' | 'oauth'  // 认证类型
    auth_config: Record<string, unknown>  // 认证配置（Vertex AI Service Account JSON）
    name: string
    rate_multipliers: Record<string, number> | null  // 按 API 格式的成本倍率
    internal_priority: number
    global_priority_by_format: Record<string, number> | null  // 按 API 格式的全局优先级
    rpm_limit: number | null  // RPM 限制（留空=自适应模式）
    cache_ttl_minutes: number
    max_probe_interval_minutes: number
    allowed_models: AllowedModels
    locked_models: string[]  // 被锁定的模型列表
    capabilities: Record<string, boolean> | null
    is_active: boolean
    note: string
    auto_fetch_models: boolean  // 是否启用自动获取模型
    model_include_patterns: string[]  // 模型包含规则
    model_exclude_patterns: string[]  // 模型排除规则
    proxy: import('./types').ProxyConfig | null  // Key 级别代理配置
  }>
): Promise<EndpointAPIKey> {
  const response = await client.put(`/api/admin/endpoints/keys/${keyId}`, data)
  return response.data
}

/**
 * 清除 Key 的 OAuth 失效标记
 */
export async function clearOAuthInvalid(keyId: string): Promise<{ message: string }> {
  const response = await client.post(`/api/admin/endpoints/keys/${keyId}/clear-oauth-invalid`)
  return response.data
}

/**
 * 刷新 Provider 的所有 Key 限额信息（Codex / Antigravity）
 */
export interface RefreshQuotaResult {
  success: number
  failed: number
  total: number
  results: Array<{
    key_id: string
    key_name: string
    status: 'success' | 'no_metadata' | 'error'
    // Codex: 额度字段为扁平结构；Antigravity: 返回 { antigravity: { quota_by_model: ... } }
    metadata?: Record<string, unknown>
    message?: string
    status_code?: number
  }>
}

export async function refreshProviderQuota(
  providerId: string,
  keyIds?: string[],
): Promise<RefreshQuotaResult> {
  const body = keyIds && keyIds.length > 0 ? { key_ids: keyIds } : undefined
  const response = await client.post(
    `/api/admin/endpoints/providers/${providerId}/refresh-quota`,
    body,
    { timeout: 5 * 60 * 1000 },
  )
  return response.data
}

/**
 * 批量导入 OAuth 凭据（通用）
 * 支持的 Provider 类型：Codex、Antigravity、GeminiCli、ClaudeCode、Kiro
 */
export interface BatchImportResultItem {
  index: number
  status: 'success' | 'error'
  key_id?: string
  key_name?: string
  auth_method?: string
  error?: string
}

export interface BatchImportResult {
  total: number
  success: number
  failed: number
  results: BatchImportResultItem[]
}

export async function batchImportOAuth(
  providerId: string,
  credentials: string,
  proxyNodeId?: string
): Promise<BatchImportResult> {
  const response = await client.post(`/api/admin/provider-oauth/providers/${providerId}/batch-import`, {
    credentials,
    proxy_node_id: proxyNodeId || undefined,
  })
  return response.data
}
