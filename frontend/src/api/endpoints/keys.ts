import client from '../client'
import type { EndpointAPIKey, AllowedModels } from './types'

// Re-export types for convenience
export type { EndpointAPIKey, AllowedModels }

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
  auth_type: 'api_key' | 'vertex_ai' | 'oauth'
  api_key?: string
  auth_config?: string | Record<string, any>
}

export async function revealEndpointKey(keyId: string): Promise<RevealKeyResult> {
  const response = await client.get(`/api/admin/endpoints/keys/${keyId}/reveal`)
  return response.data
}

/**
 * 删除 Key
 */
export async function deleteEndpointKey(keyId: string): Promise<{ message: string }> {
  const response = await client.delete(`/api/admin/endpoints/keys/${keyId}`)
  return response.data
}


// ========== Provider 级别的 Keys API ==========


/**
 * 获取 Provider 的所有 Keys
 */
export async function getProviderKeys(providerId: string): Promise<EndpointAPIKey[]> {
  const response = await client.get(`/api/admin/endpoints/providers/${providerId}/keys`)
  return response.data
}

/**
 * 为 Provider 添加 Key
 */
export async function addProviderKey(
  providerId: string,
  data: {
    api_formats: string[]  // 支持的 API 格式列表（必填）
    api_key: string
    auth_type?: 'api_key' | 'vertex_ai' | 'oauth'  // 认证类型
    auth_config?: Record<string, any>  // 认证配置（Vertex AI Service Account JSON）
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
    auth_type: 'api_key' | 'vertex_ai' | 'oauth'  // 认证类型
    auth_config: Record<string, any>  // 认证配置（Vertex AI Service Account JSON）
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
  }>
): Promise<EndpointAPIKey> {
  const response = await client.put(`/api/admin/endpoints/keys/${keyId}`, data)
  return response.data
}

/**
 * 刷新 Provider 的所有 Key 限额信息（Codex）
 */
export interface RefreshQuotaResult {
  success: number
  failed: number
  total: number
  results: Array<{
    key_id: string
    key_name: string
    status: 'success' | 'no_metadata' | 'error'
    metadata?: {
      plan_type?: string
      primary_used_percent?: number
      primary_reset_seconds?: number
      secondary_used_percent?: number
      secondary_reset_seconds?: number
    }
    message?: string
    status_code?: number
  }>
}

export async function refreshProviderQuota(providerId: string): Promise<RefreshQuotaResult> {
  const response = await client.post(`/api/admin/endpoints/providers/${providerId}/refresh-quota`)
  return response.data
}
