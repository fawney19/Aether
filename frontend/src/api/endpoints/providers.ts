import client from '../client'
import type { ProviderWithEndpointsSummary, ProxyConfig } from './types'

/**
 * 获取 Providers 摘要（包含 Endpoints 统计）
 */
export async function getProvidersSummary(): Promise<ProviderWithEndpointsSummary[]> {
  const response = await client.get('/api/admin/providers/summary')
  return response.data
}

/**
 * 获取单个 Provider 的详细信息
 */
export async function getProvider(providerId: string): Promise<ProviderWithEndpointsSummary> {
  const response = await client.get(`/api/admin/providers/${providerId}/summary`)
  return response.data
}

/**
 * 更新 Provider 基础配置
 */
export async function updateProvider(
  providerId: string,
  data: Partial<{
    name: string
    provider_type: 'custom' | 'claude_code' | 'codex' | 'gemini_cli' | 'antigravity'
    description: string | null
    website: string
    provider_priority: number
    billing_type: 'monthly_quota' | 'pay_as_you_go' | 'free_tier'
    monthly_quota_usd: number
    quota_reset_day: number
    quota_last_reset_at: string  // 周期开始时间
    quota_expires_at: string
    rpm_limit: number | null
    // 请求配置（从 Endpoint 迁移）
    max_retries: number
    proxy: ProxyConfig | null
    cache_ttl_minutes: number  // 0表示不支持缓存，>0表示支持缓存并设置TTL(分钟)
    max_probe_interval_minutes: number
    enable_format_conversion: boolean  // 是否允许格式转换（提供商级别开关）
    is_active: boolean
  }>
): Promise<ProviderWithEndpointsSummary> {
  const response = await client.patch(`/api/admin/providers/${providerId}`, data)
  return response.data
}

/**
 * 创建 Provider
 */
export async function createProvider(
  data: {
    name: string
    provider_type?: 'custom' | 'claude_code' | 'codex' | 'gemini_cli' | 'antigravity'
    description?: string
    website?: string
    billing_type?: 'monthly_quota' | 'pay_as_you_go' | 'free_tier'
    monthly_quota_usd?: number
    quota_reset_day?: number
    quota_last_reset_at?: string
    quota_expires_at?: string
    provider_priority?: number
    keep_priority_on_conversion?: boolean
    is_active?: boolean
    max_retries?: number
    stream_first_byte_timeout?: number | null
    request_timeout?: number | null
    proxy?: ProxyConfig | null
  }
): Promise<{ id: string; name: string; message?: string }> {
  const response = await client.post('/api/admin/providers/', data)
  return response.data
}

/**
 * 删除 Provider
 */
export async function deleteProvider(providerId: string): Promise<{ message: string }> {
  const response = await client.delete(`/api/admin/providers/${providerId}`)
  return response.data
}

/**
 * 测试模型连接性
 */
export interface TestModelRequest {
  provider_id: string
  model_name: string
  api_key_id?: string
  endpoint_id?: string
  message?: string
  api_format?: string
}

export interface TestModelResponse {
  success: boolean
  error?: string
  data?: {
    response?: {
      status_code?: number
      error?: string | { message?: string }
      choices?: Array<{ message?: { content?: string } }>
    }
    content_preview?: string
  }
  provider?: {
    id: string
    name: string
  }
  model?: string
}

export async function testModel(data: TestModelRequest): Promise<TestModelResponse> {
  const response = await client.post('/api/admin/provider-query/test-model', data)
  return response.data
}

/**
 * 映射预览相关类型
 */
export interface MappingMatchedModel {
  allowed_model: string
  mapping_pattern: string
}

export interface MappingMatchingGlobalModel {
  global_model_id: string
  global_model_name: string
  display_name: string
  is_active: boolean
  matched_models: MappingMatchedModel[]
}

export interface MappingMatchingKey {
  key_id: string
  key_name: string
  masked_key: string
  is_active: boolean
  allowed_models: string[]
  matching_global_models: MappingMatchingGlobalModel[]
}

export interface ProviderMappingPreviewResponse {
  provider_id: string
  provider_name: string
  keys: MappingMatchingKey[]
  total_keys: number
  total_matches: number
  // 截断提示
  truncated: boolean
  truncated_keys: number
  truncated_models: number
}

/**
 * 获取 Provider 映射预览
 */
export async function getProviderMappingPreview(
  providerId: string
): Promise<ProviderMappingPreviewResponse> {
  const response = await client.get(`/api/admin/providers/${providerId}/mapping-preview`)
  return response.data
}
