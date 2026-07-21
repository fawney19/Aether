/**
 * Models.dev API 服务
 * 通过后端代理获取 models.dev 数据（解决跨域问题）
 */

import api from './client'
import {
  resolveModelsDevTieredPricing,
  type ModelsDevCost,
} from './models-dev-pricing'
import type { TieredPricingConfig } from './endpoints/types'

export type { ModelsDevCost, ModelsDevCostTier, ModelsDevTokenCost } from './models-dev-pricing'

// 缓存配置
const CACHE_KEY = 'models_dev_cache'
const CACHE_DURATION = 15 * 60 * 1000 // 15 分钟

// Models.dev API 数据结构
export interface ModelsDevLimit {
  context?: number
  output?: number
}

export interface ModelsDevModel {
  id: string
  name: string
  family?: string
  reasoning?: boolean
  tool_call?: boolean
  structured_output?: boolean
  temperature?: boolean
  attachment?: boolean
  knowledge?: string
  release_date?: string
  last_updated?: string
  input?: string[] // 输入模态: text, image, audio, video, pdf
  output?: string[] // 输出模态: text, image, audio
  modalities?: {
    input?: string[]
    output?: string[]
  }
  open_weights?: boolean
  cost?: ModelsDevCost
  experimental?: {
    modes?: Record<string, {
      cost?: ModelsDevCost
      provider?: {
        body?: Record<string, unknown>
        headers?: Record<string, string>
      }
    }>
  }
  limit?: ModelsDevLimit
  deprecated?: boolean
}

export interface ModelsDevProvider {
  id: string
  env?: string[]
  npm?: string
  api?: string
  name: string
  doc?: string
  models: Record<string, ModelsDevModel>
  official?: boolean // 是否为官方提供商
}

export type ModelsDevData = Record<string, ModelsDevProvider>

// 扁平化的模型列表项（用于搜索和选择）
export interface ModelsDevModelItem {
  providerId: string
  providerName: string
  modelId: string
  modelName: string
  family?: string
  inputPrice?: number
  outputPrice?: number
  tieredPricing?: TieredPricingConfig
  contextLimit?: number
  outputLimit?: number
  supportsVision?: boolean
  supportsToolCall?: boolean
  supportsReasoning?: boolean
  supportsStructuredOutput?: boolean
  supportsTemperature?: boolean
  supportsAttachment?: boolean
  supportsEmbedding?: boolean
  openWeights?: boolean
  deprecated?: boolean
  official?: boolean
  // 用于 display_metadata 的额外字段
  knowledgeCutoff?: string
  releaseDate?: string
  inputModalities?: string[]
  outputModalities?: string[]
}

interface CacheData {
  timestamp: number
  data: ModelsDevData
}

// 内存缓存
let memoryCache: CacheData | null = null

function hasOfficialFlag(data: ModelsDevData): boolean {
  return Object.values(data).some(provider => typeof provider?.official === 'boolean')
}

/**
 * 获取 models.dev 数据（带缓存）
 */
export async function getModelsDevData(): Promise<ModelsDevData> {
  // 1. 检查内存缓存
  if (memoryCache && Date.now() - memoryCache.timestamp < CACHE_DURATION) {
    // 兼容旧缓存：没有 official 字段时丢弃，强制刷新一次
    if (hasOfficialFlag(memoryCache.data)) {
      return memoryCache.data
    }
    memoryCache = null
  }

  // 2. 检查 localStorage 缓存
  try {
    const cached = localStorage.getItem(CACHE_KEY)
    if (cached) {
      const cacheData: CacheData = JSON.parse(cached)
      if (Date.now() - cacheData.timestamp < CACHE_DURATION) {
        // 兼容旧缓存：没有 official 字段时丢弃，强制刷新一次
        if (hasOfficialFlag(cacheData.data)) {
          memoryCache = cacheData
          return cacheData.data
        }
        localStorage.removeItem(CACHE_KEY)
      }
    }
  } catch {
    // 缓存解析失败，忽略
  }

  // 3. 从后端代理获取新数据
  const response = await api.get<ModelsDevData>('/api/admin/models/external')
  const data = response.data

  // 4. 更新缓存
  const cacheData: CacheData = {
    timestamp: Date.now(),
    data,
  }
  memoryCache = cacheData
  try {
    localStorage.setItem(CACHE_KEY, JSON.stringify(cacheData))
  } catch {
    // localStorage 写入失败，忽略
  }

  return data
}

// 模型列表缓存（避免重复转换）
let modelsListCache: ModelsDevModelItem[] | null = null
let modelsListCacheTimestamp: number | null = null

/**
 * 获取扁平化的模型列表
 * 数据只加载一次，通过参数过滤官方/全部
 */
export async function getModelsDevList(officialOnly: boolean = true): Promise<ModelsDevModelItem[]> {
  const data = await getModelsDevData()
  const currentTimestamp = memoryCache?.timestamp ?? 0

  // 如果缓存为空或数据已刷新，构建一次
  if (!modelsListCache || modelsListCacheTimestamp !== currentTimestamp) {
    const items: ModelsDevModelItem[] = []

    for (const [providerId, provider] of Object.entries(data)) {
      if (!provider.models) continue

      for (const [modelId, model] of Object.entries(provider.models)) {
        const inputModalities = model.modalities?.input ?? model.input
        const outputModalities = model.modalities?.output ?? model.output
        const tieredPricing = resolveModelsDevTieredPricing(
          providerId,
          modelId,
          model.cost,
          model.experimental?.modes,
        )
        const basePricingTier = tieredPricing?.tiers[0]
        items.push({
          providerId,
          providerName: provider.name,
          modelId,
          modelName: model.name || modelId,
          family: model.family,
          inputPrice: basePricingTier?.input_price_per_1m ?? model.cost?.input,
          outputPrice: basePricingTier?.output_price_per_1m ?? model.cost?.output,
          tieredPricing: tieredPricing ?? undefined,
          contextLimit: model.limit?.context,
          outputLimit: model.limit?.output,
          supportsVision: inputModalities?.includes('image'),
          supportsToolCall: model.tool_call,
          supportsReasoning: model.reasoning,
          supportsStructuredOutput: model.structured_output,
          supportsTemperature: model.temperature,
          supportsAttachment: model.attachment,
          supportsEmbedding: model.id.toLowerCase().includes('embedding')
            || model.name.toLowerCase().includes('embedding')
            || model.family?.toLowerCase().includes('embedding') === true,
          openWeights: model.open_weights,
          deprecated: model.deprecated,
          official: provider.official,
          // display_metadata 相关字段
          knowledgeCutoff: model.knowledge,
          releaseDate: model.release_date,
          inputModalities,
          outputModalities,
        })
      }
    }

    // 按 provider 名称排序，provider 中的模型按 release_date 从近到远排序
    items.sort((a, b) => {
      const providerCompare = a.providerName.localeCompare(b.providerName)
      if (providerCompare !== 0) return providerCompare
      
      // 模型按 release_date 从近到远排序（没有日期的排到最后）
      const aDate = a.releaseDate ? new Date(a.releaseDate).getTime() : 0
      const bDate = b.releaseDate ? new Date(b.releaseDate).getTime() : 0
      if (aDate !== bDate) return bDate - aDate // 降序：新的在前
      
      // 日期相同或都没有日期时，按模型名称排序
      return a.modelName.localeCompare(b.modelName)
    })

    modelsListCache = items
    modelsListCacheTimestamp = currentTimestamp
  }

  // 根据参数过滤
  if (officialOnly) {
    return modelsListCache.filter(m => m.official)
  }
  return modelsListCache
}

/**
 * 搜索模型
 * 搜索时包含所有提供商（包括第三方）
 */
export async function searchModelsDevModels(
  query: string,
  options?: {
    limit?: number
    excludeDeprecated?: boolean
  }
): Promise<ModelsDevModelItem[]> {
  // 搜索时包含全部提供商
  const allModels = await getModelsDevList(false)
  const { limit = 50, excludeDeprecated = true } = options || {}

  const queryLower = query.toLowerCase()

  const filtered = allModels.filter((model) => {
    if (excludeDeprecated && model.deprecated) return false

    // 搜索模型 ID、名称、provider 名称、family
    return (
      model.modelId.toLowerCase().includes(queryLower) ||
      model.modelName.toLowerCase().includes(queryLower) ||
      model.providerName.toLowerCase().includes(queryLower) ||
      model.family?.toLowerCase().includes(queryLower)
    )
  })

  // 排序：精确匹配优先
  filtered.sort((a, b) => {
    const aExact =
      a.modelId.toLowerCase() === queryLower ||
      a.modelName.toLowerCase() === queryLower
    const bExact =
      b.modelId.toLowerCase() === queryLower ||
      b.modelName.toLowerCase() === queryLower
    if (aExact && !bExact) return -1
    if (!aExact && bExact) return 1
    return 0
  })

  return filtered.slice(0, limit)
}

/**
 * 获取特定模型详情
 */
export async function getModelsDevModel(
  providerId: string,
  modelId: string
): Promise<ModelsDevModel | null> {
  const data = await getModelsDevData()
  return data[providerId]?.models?.[modelId] || null
}

/**
 * 获取 provider logo URL
 */
export function getProviderLogoUrl(providerId: string): string {
  return `https://models.dev/logos/${providerId}.svg`
}

/**
 * 清除缓存
 */
export function clearModelsDevCache(): void {
  memoryCache = null
  modelsListCache = null
  modelsListCacheTimestamp = null
  try {
    localStorage.removeItem(CACHE_KEY)
  } catch {
    // 忽略错误
  }
}
