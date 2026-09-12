import client from '../client'
import type { ProviderEndpoint, ProxyConfig, HeaderRule, BodyRule, FormatAcceptanceConfig } from './types'

export interface ProviderEndpointRules {
  header_rules: HeaderRule[]
  body_rules: BodyRule[]
  response_header_rules: HeaderRule[]
}

export async function revealEndpointRules(endpointId: string, signal?: AbortSignal): Promise<ProviderEndpointRules> {
  const response = await client.get<ProviderEndpointRules>(
    `/api/admin/endpoints/${encodeURIComponent(endpointId)}/rules/reveal`,
    { signal },
  )
  return response.data
}

/**
 * 获取指定 Provider 的所有 Endpoints
 */
export async function getProviderEndpoints(providerId: string): Promise<ProviderEndpoint[]> {
  const response = await client.get<ProviderEndpoint[]>(`/api/admin/endpoints/providers/${providerId}/endpoints`)
  return response.data
}

/**
 * 获取 Endpoint 详情
 */
export async function getEndpoint(endpointId: string): Promise<ProviderEndpoint> {
  const response = await client.get<ProviderEndpoint>(`/api/admin/endpoints/${endpointId}`)
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
    config?: Record<string, unknown>
    proxy?: ProxyConfig | null
    format_acceptance_config?: FormatAcceptanceConfig | null
  }
): Promise<ProviderEndpoint> {
  const response = await client.post<ProviderEndpoint>(`/api/admin/endpoints/providers/${providerId}/endpoints`, data)
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
    config: Record<string, unknown> | null
    proxy: ProxyConfig | null
    format_acceptance_config: FormatAcceptanceConfig | null
  }>
): Promise<ProviderEndpoint> {
  const response = await client.put<ProviderEndpoint>(`/api/admin/endpoints/${endpointId}`, data)
  return response.data
}

/**
 * 删除 Endpoint
 */
export async function deleteEndpoint(endpointId: string): Promise<{ message: string; affected_keys_count: number }> {
  const response = await client.delete<{ message: string; affected_keys_count: number }>(`/api/admin/endpoints/${endpointId}`)
  return response.data
}

/**
 * 获取指定 API 格式的默认请求体规则
 */
export async function getDefaultBodyRules(apiFormat: string, providerType?: string): Promise<{ api_format: string; body_rules: BodyRule[] }> {
  const params: Record<string, string> = {}
  if (providerType) params.provider_type = providerType
  const response = await client.get<{ api_format: string; body_rules: BodyRule[] }>(`/api/admin/endpoints/defaults/${apiFormat}/body-rules`, { params })
  return response.data
}
