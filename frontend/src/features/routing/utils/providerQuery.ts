import type { ProviderSummaryQuery } from '@/api/endpoints/providers'

import {
  DEFAULT_ROUTING_POLICY_MODEL,
  type RoutingPriorityMode,
} from './routingPolicy'

const PROVIDER_SUMMARY_PAGE_SIZE = 9999

/**
 * 构造路由排序编辑器的提供商查询参数。
 * 按模型配置时必须使用 GlobalModel ID 过滤，避免把模型名称误当成 ID；统一调度和
 * Key 排序仍需要完整的提供商集合。
 */
export function buildRoutingProviderSummaryQuery(
  model: string | undefined,
  modelId: string | undefined,
  priorityMode: RoutingPriorityMode,
): ProviderSummaryQuery | null {
  const query: ProviderSummaryQuery = {
    page: 1,
    page_size: PROVIDER_SUMMARY_PAGE_SIZE,
  }
  const isModelScoped = (model?.trim() || DEFAULT_ROUTING_POLICY_MODEL)
    !== DEFAULT_ROUTING_POLICY_MODEL

  if (!isModelScoped || priorityMode !== 'provider') {
    return query
  }

  const normalizedModelId = modelId?.trim()
  // 模型 ID 尚未由父组件解析出来时，不能回退到全量列表，否则会短暂显示错误的提供商。
  if (!normalizedModelId) return null

  return {
    ...query,
    model_id: normalizedModelId,
  }
}
