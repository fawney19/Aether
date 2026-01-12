import client from '../client'
import type {
  HealthStatus,
  HealthSummary,
  EndpointStatusMonitorResponse,
  PublicEndpointStatusMonitorResponse
} from './types'

/**
 * 获取健康状态摘要
 */
export async function getHealthSummary(): Promise<HealthSummary> {
  const response = await client.get('/api/admin/endpoints/health/summary')
  return response.data
}

/**
 * 获取 Endpoint 健康状态
 */
export async function getEndpointHealth(endpointId: string): Promise<HealthStatus> {
  const response = await client.get(`/api/admin/endpoints/health/endpoint/${endpointId}`)
  return response.data
}

/**
 * 获取 Key 健康状态
 */
export async function getKeyHealth(keyId: string): Promise<HealthStatus> {
  const response = await client.get(`/api/admin/endpoints/health/key/${keyId}`)
  return response.data
}

/**
 * 恢复Key健康状态（一键恢复：重置健康度 + 关闭熔断器 + 取消自动禁用）
 * @param keyId Key ID
 * @param apiFormat 可选，指定 API 格式（如 CLAUDE、OPENAI），不指定则恢复所有格式
 */
export async function recoverKeyHealth(keyId: string, apiFormat?: string): Promise<{
  message: string
  details: {
    api_format?: string
    health_score: number
    circuit_breaker_open: boolean
    is_active: boolean
  }
}> {
  const response = await client.patch(`/api/admin/endpoints/health/keys/${keyId}`, null, {
    params: apiFormat ? { api_format: apiFormat } : undefined
  })
  return response.data
}

/**
 * 批量恢复所有熔断的Key健康状态
 */
export async function recoverAllKeysHealth(): Promise<{
  message: string
  recovered_count: number
  recovered_keys: Array<{
    key_id: string
    key_name: string
    endpoint_id: string
  }>
}> {
  const response = await client.patch('/api/admin/endpoints/health/keys')
  return response.data
}

/**
 * 获取按 API 格式聚合的健康监控时间线（管理员版，含 provider/key 数量）
 */
export async function getEndpointStatusMonitor(params?: {
  lookback_hours?: number
  per_format_limit?: number
}): Promise<EndpointStatusMonitorResponse> {
  const response = await client.get('/api/admin/endpoints/health/api-formats', {
    params
  })
  return response.data
}

/**
 * 获取按 API 格式聚合的健康监控时间线（公开版，不含敏感信息）
 */
export async function getPublicEndpointStatusMonitor(params?: {
  lookback_hours?: number
  per_format_limit?: number
}): Promise<PublicEndpointStatusMonitorResponse> {
  const response = await client.get('/api/public/health/api-formats', {
    params
  })
  return response.data
}
