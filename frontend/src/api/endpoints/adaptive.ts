import client from '../client'
import type { AdaptiveStatsResponse } from './types'

/**
 * 启用/禁用 Key 的自适应模式
 */
export async function toggleAdaptiveMode(
  keyId: string,
  data: {
    enabled: boolean
    fixed_limit?: number
  }
): Promise<{
  message: string
  key_id: string
  is_adaptive: boolean
  rpm_limit: number | null
  effective_limit: number | null
}> {
  const response = await client.patch(`/api/admin/adaptive/keys/${keyId}/mode`, data)
  return response.data
}

/**
 * 设置 Key 的固定 RPM 限制
 */
export async function setRpmLimit(
  keyId: string,
  limit: number
): Promise<{
  message: string
  key_id: string
  is_adaptive: boolean
  rpm_limit: number
  previous_mode: string
}> {
  const response = await client.patch(`/api/admin/adaptive/keys/${keyId}/limit`, null, {
    params: { limit }
  })
  return response.data
}

/**
 * 获取 Key 的自适应统计
 */
export async function getAdaptiveStats(keyId: string): Promise<AdaptiveStatsResponse> {
  const response = await client.get(`/api/admin/adaptive/keys/${keyId}/stats`)
  return response.data
}

/**
 * 重置 Key 的学习状态
 */
export async function resetAdaptiveLearning(keyId: string): Promise<{ message: string; key_id: string }> {
  const response = await client.delete(`/api/admin/adaptive/keys/${keyId}/learning`)
  return response.data
}
