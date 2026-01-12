/**
 * 上游模型缓存 - 共享缓存，避免重复请求
 */
import { ref } from 'vue'
import { adminApi } from '@/api/admin'
import { parseUpstreamModelError } from '@/utils/errorParser'
import type { UpstreamModel } from '@/api/endpoints/types'

// 扩展类型，包含可能的额外字段
export type { UpstreamModel }

interface CacheEntry {
  models: UpstreamModel[]
  timestamp: number
}

type FetchResult = { models: UpstreamModel[]; error?: string }

// 全局缓存（模块级别，所有组件共享）
const cache = new Map<string, CacheEntry>()
const CACHE_TTL = 5 * 60 * 1000 // 5分钟

// 进行中的请求（用于去重并发请求）
const pendingRequests = new Map<string, Promise<FetchResult>>()

// 请求状态
const loadingMap = ref<Map<string, boolean>>(new Map())

export function useUpstreamModelsCache() {
  /**
   * 获取上游模型列表
   * @param providerId 提供商ID
   * @param forceRefresh 是否强制刷新
   * @returns 模型列表或 null（如果请求失败）
   */
  async function fetchModels(
    providerId: string,
    forceRefresh = false
  ): Promise<FetchResult> {
    // 检查缓存
    if (!forceRefresh) {
      const cached = cache.get(providerId)
      if (cached && Date.now() - cached.timestamp < CACHE_TTL) {
        return { models: cached.models }
      }
    }

    // 检查是否有进行中的请求（非强制刷新时复用）
    if (!forceRefresh && pendingRequests.has(providerId)) {
      return pendingRequests.get(providerId)!
    }

    // 创建新请求
    const requestPromise = (async (): Promise<FetchResult> => {
      try {
        loadingMap.value.set(providerId, true)
        const response = await adminApi.queryProviderModels(providerId)

        if (response.success && response.data?.models) {
          // 存入缓存
          cache.set(providerId, {
            models: response.data.models,
            timestamp: Date.now()
          })
          return { models: response.data.models }
        } else {
          // 使用友好的错误解析
          const rawError = response.data?.error || '获取上游模型失败'
          return { models: [], error: parseUpstreamModelError(rawError) }
        }
      } catch (err: any) {
        // 使用友好的错误解析
        const rawError = err.response?.data?.detail || err.message || '获取上游模型失败'
        return { models: [], error: parseUpstreamModelError(rawError) }
      } finally {
        loadingMap.value.set(providerId, false)
        pendingRequests.delete(providerId)
      }
    })()

    pendingRequests.set(providerId, requestPromise)
    return requestPromise
  }

  /**
   * 获取缓存的模型（不发起请求）
   */
  function getCachedModels(providerId: string): UpstreamModel[] | null {
    const cached = cache.get(providerId)
    if (cached && Date.now() - cached.timestamp < CACHE_TTL) {
      return cached.models
    }
    return null
  }

  /**
   * 清除指定提供商的缓存
   */
  function clearCache(providerId: string) {
    cache.delete(providerId)
  }

  /**
   * 检查是否正在加载
   */
  function isLoading(providerId: string): boolean {
    return loadingMap.value.get(providerId) || false
  }

  return {
    fetchModels,
    getCachedModels,
    clearCache,
    isLoading,
    loadingMap
  }
}
