import client from '../client'
import type { ProviderWithEndpointsSummary } from './types'

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
    display_name: string
    description: string
    website: string
    provider_priority: number
    billing_type: 'monthly_quota' | 'pay_as_you_go' | 'free_tier'
    monthly_quota_usd: number
    quota_reset_day: number
    quota_last_reset_at: string  // 周期开始时间
    quota_expires_at: string
    rpm_limit: number | null
    cache_ttl_minutes: number  // 0表示不支持缓存，>0表示支持缓存并设置TTL(分钟)
    max_probe_interval_minutes: number
    is_active: boolean
  }>
): Promise<ProviderWithEndpointsSummary> {
  const response = await client.patch(`/api/admin/providers/${providerId}`, data)
  return response.data
}

/**
 * 创建 Provider
 */
export async function createProvider(data: any): Promise<any> {
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
    display_name: string
  }
  model?: string
}

export async function testModel(data: TestModelRequest): Promise<TestModelResponse> {
  const response = await client.post('/api/admin/provider-query/test-model', data)
  return response.data
}

