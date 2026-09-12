/**
 * Public Models API - 普通用户可访问的模型列表
 */

import client from './client'
import type { TieredPricingConfig } from './endpoints/types'

export interface PublicGlobalModel {
  id: string
  name: string
  display_name: string | null
  is_active: boolean
  // 阶梯计费配置
  default_tiered_pricing: TieredPricingConfig | null
  default_price_per_request: number | null  // 按次计费价格
  // Key 能力支持
  supported_capabilities: string[] | null
  supports_embedding?: boolean | null
  // 模型配置（JSON）
  config: Record<string, unknown> | null
  // 调用次数
  usage_count: number
}

export interface PublicGlobalModelListResponse {
  models: PublicGlobalModel[]
  total: number
}

/**
 * 获取公开的 GlobalModel 列表（普通用户可访问）
 */
export async function getPublicGlobalModels(params?: {
  skip?: number
  limit?: number
  is_active?: boolean
  search?: string
}): Promise<PublicGlobalModelListResponse> {
  const response = await client.get<PublicGlobalModelListResponse>('/api/public/global-models', { params })
  return response.data
}
