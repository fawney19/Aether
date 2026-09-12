import client from '../client'
import type {
  Model,
  ModelCreate,
  ModelUpdate,
  ModelCatalogResponse,
  ProviderAvailableSourceModelsResponse,
  ImportFromUpstreamResponse,
} from './types'

/**
 * 获取 Provider 的所有模型
 */
export async function getProviderModels(
  providerId: string,
  params?: {
    is_active?: boolean
    skip?: number
    limit?: number
  }
): Promise<Model[]> {
  const response = await client.get<Model[]>(`/api/admin/providers/${providerId}/models`, { params })
  return response.data
}

/**
 * 创建模型
 */
export async function createModel(
  providerId: string,
  data: ModelCreate
): Promise<Model> {
  const response = await client.post<Model>(`/api/admin/providers/${providerId}/models`, data)
  return response.data
}

/**
 * 获取模型详情
 */
export async function getModel(
  providerId: string,
  modelId: string
): Promise<Model> {
  const response = await client.get<Model>(`/api/admin/providers/${providerId}/models/${modelId}`)
  return response.data
}

/**
 * 更新模型
 */
export async function updateModel(
  providerId: string,
  modelId: string,
  data: ModelUpdate
): Promise<Model> {
  const response = await client.patch<Model>(`/api/admin/providers/${providerId}/models/${modelId}`, data)
  return response.data
}

/**
 * 删除模型
 */
export async function deleteModel(
  providerId: string,
  modelId: string
): Promise<{ message: string }> {
  const response = await client.delete<{ message: string }>(`/api/admin/providers/${providerId}/models/${modelId}`)
  return response.data
}

/**
 * 批量创建模型
 */
export async function batchCreateModels(
  providerId: string,
  modelsData: ModelCreate[]
): Promise<Model[]> {
  const response = await client.post<Model[]>(`/api/admin/providers/${providerId}/models/batch`, modelsData)
  return response.data
}

/**
 * 获取统一模型目录
 */
export async function getModelCatalog(): Promise<ModelCatalogResponse> {
  const response = await client.get<ModelCatalogResponse>('/api/admin/models/catalog')
  return response.data
}

/**
 * 获取 Provider 支持的统一模型列表
 */
export async function getProviderAvailableSourceModels(
  providerId: string
): Promise<ProviderAvailableSourceModelsResponse> {
  const response = await client.get<ProviderAvailableSourceModelsResponse>(`/api/admin/providers/${providerId}/available-source-models`)
  return response.data
}

/**
 * 批量为 Provider 关联 GlobalModels
 */
export async function batchAssignModelsToProvider(
  providerId: string,
  globalModelIds: string[]
): Promise<{
  success: Array<{
    global_model_id: string
    global_model_name: string
    model_id: string
  }>
  errors: Array<{
    global_model_id: string
    error: string
  }>
}> {
  const response = await client.post<{
  success: Array<{
    global_model_id: string
    global_model_name: string
    model_id: string
  }>
  errors: Array<{
    global_model_id: string
    error: string
  }>
}>(
    `/api/admin/providers/${providerId}/assign-global-models`,
    { global_model_ids: globalModelIds }
  )
  return response.data
}

/**
 * 从上游提供商导入模型
 * @param providerId 提供商 ID
 * @param modelIds 模型 ID 列表
 * @param options 可选配置
 * @param options.tiered_pricing 阶梯计费配置
 * @param options.price_per_request 按次计费价格
 */
export async function importModelsFromUpstream(
  providerId: string,
  modelIds: string[],
  options?: {
    tiered_pricing?: object
    price_per_request?: number
  }
): Promise<ImportFromUpstreamResponse> {
  const response = await client.post<ImportFromUpstreamResponse>(
    `/api/admin/providers/${providerId}/import-from-upstream`,
    {
      model_ids: modelIds,
      ...options
    }
  )
  return response.data
}
