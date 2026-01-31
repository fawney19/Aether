import client from '../client'
import type { ProviderEndpoint, ProxyConfig, HeaderRule, BodyRule, FormatAcceptanceConfig } from './types'

/**
 * 获取指定 Provider 的所有 Endpoints
 */
export async function getProviderEndpoints(providerId: string): Promise<ProviderEndpoint[]> {
  const response = await client.get(`/api/admin/endpoints/providers/${providerId}/endpoints`)
  return response.data
}

/**
 * 获取 Endpoint 详情
 */
export async function getEndpoint(endpointId: string): Promise<ProviderEndpoint> {
  const response = await client.get(`/api/admin/endpoints/${endpointId}`)
  return response.data
}

/**
 * 为 Provider 创建新的 Endpoint
 */
export async function createEndpoint(
  providerId: string,
  data: {
    provider_id: string
    api_format: string
    base_url: string
    custom_path?: string
    header_rules?: HeaderRule[]
    body_rules?: BodyRule[]
    max_retries?: number
    is_active?: boolean
    config?: Record<string, any>
    proxy?: ProxyConfig | null
    format_acceptance_config?: FormatAcceptanceConfig | null
  }
): Promise<ProviderEndpoint> {
  const response = await client.post(`/api/admin/endpoints/providers/${providerId}/endpoints`, data)
  return response.data
}

/**
 * 更新 Endpoint
 */
export async function updateEndpoint(
  endpointId: string,
  data: Partial<{
    base_url: string
    custom_path: string | null
    header_rules: HeaderRule[] | null
    body_rules: BodyRule[] | null
    max_retries: number
    is_active: boolean
    config: Record<string, any>
    proxy: ProxyConfig | null
    format_acceptance_config: FormatAcceptanceConfig | null
  }>
): Promise<ProviderEndpoint> {
  const response = await client.put(`/api/admin/endpoints/${endpointId}`, data)
  return response.data
}

/**
 * 删除 Endpoint
 */
export async function deleteEndpoint(endpointId: string): Promise<{ message: string; affected_keys_count: number }> {
  const response = await client.delete(`/api/admin/endpoints/${endpointId}`)
  return response.data
}
