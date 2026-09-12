import client from '../client'
import { buildCacheKey, cachedRequest, dedupedRequest } from '@/utils/cache'
import type {
  GlobalModelCreate,
  GlobalModelUpdate,
  GlobalModelResponse,
  GlobalModelWithStats,
  GlobalModelListResponse,
  ModelCatalogProviderDetail,
  ModelRoutingPreviewResponse,
} from './types'

// 重新导出路由相关类型供外部使用
export type {
  GlobalModelResponse,
  RoutingKeyInfo,
  RoutingEndpointInfo,
  RoutingModelMapping,
  RoutingProviderInfo,
  ModelRoutingPreviewResponse,
} from './types'

/**
 * 获取 GlobalModel 列表
 */
interface GlobalModelListOptions {
  cacheTtlMs?: number
}

export async function getGlobalModels(params?: {
  skip?: number
  limit?: number
  is_active?: boolean
  search?: string
}, options: GlobalModelListOptions = {}): Promise<GlobalModelListResponse> {
  const cacheTtlMs = options.cacheTtlMs ?? 0
  const key = buildCacheKey('global-models:list', params as Record<string, unknown> | undefined)
  return cachedRequest(
    key,
    async () => {
      const response = await client.get<GlobalModelListResponse>('/api/admin/models/global', { params })
      return response.data
    },
    cacheTtlMs,
  )
}

/**
 * 获取单个 GlobalModel 详情
 */
export async function getGlobalModel(id: string): Promise<GlobalModelWithStats> {
  return dedupedRequest(`global-models:detail:${id}`, async () => {
    const response = await client.get<GlobalModelWithStats>(`/api/admin/models/global/${id}`)
    return response.data
  })
}

/**
 * 创建 GlobalModel
 */
export async function createGlobalModel(data: GlobalModelCreate): Promise<GlobalModelResponse> {
  const response = await client.post<GlobalModelResponse>('/api/admin/models/global', data)
  return response.data
}

/**
 * 更新 GlobalModel
 */
export async function updateGlobalModel(
  id: string,
  data: GlobalModelUpdate
): Promise<GlobalModelResponse> {
  const response = await client.patch<GlobalModelResponse>(`/api/admin/models/global/${id}`, data)
  return response.data
}

/**
 * 删除 GlobalModel
 */
export async function deleteGlobalModel(
  id: string,
  force: boolean = false
): Promise<void> {
  await client.delete(`/api/admin/models/global/${id}`, { params: { force } })
}

/**
 * 批量删除 GlobalModel
 */
export async function batchDeleteGlobalModels(
  ids: string[]
): Promise<{ success_count: number; failed: Array<{ id: string; error: string }> }> {
  const response = await client.post<{ success_count: number; failed: Array<{ id: string; error: string }> }>('/api/admin/models/global/batch-delete', { ids })
  return response.data
}

/**
 * 批量为 GlobalModel 添加关联提供商
 */
export async function batchAssignToProviders(
  globalModelId: string,
  data: {
    provider_ids: string[]
    create_models: boolean
  }
): Promise<{
  success: Array<{
    provider_id: string
    provider_name: string
    model_id?: string
  }>
  errors: Array<{
    provider_id: string
    error: string
  }>
}> {
  const response = await client.post<{
  success: Array<{
    provider_id: string
    provider_name: string
    model_id?: string
  }>
  errors: Array<{
    provider_id: string
    error: string
  }>
}>(
    `/api/admin/models/global/${globalModelId}/assign-to-providers`,
    data
  )
  return response.data
}

/**
 * 获取 GlobalModel 的所有关联提供商（包括非活跃的）
 */
export async function getGlobalModelProviders(globalModelId: string): Promise<{
  providers: ModelCatalogProviderDetail[]
  total: number
}> {
  return dedupedRequest(`global-models:providers:${globalModelId}`, async () => {
    const response = await client.get<{ providers: ModelCatalogProviderDetail[]; total: number }>(
      `/api/admin/models/global/${globalModelId}/providers`
    )
    return response.data
  })
}

/**
 * 获取 GlobalModel 的请求链路预览
 */
export async function getGlobalModelRoutingPreview(
  globalModelId: string
): Promise<ModelRoutingPreviewResponse> {
  const response = await client.get<ModelRoutingPreviewResponse>(
    `/api/admin/models/global/${globalModelId}/routing`
  )
  return response.data
}
