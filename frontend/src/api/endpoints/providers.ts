import client from '../client'
import { buildCacheKey, cachedRequest, dedupedRequest } from '@/utils/cache'
import type {
  ClaudeCodeAdvancedConfig,
  FailoverRulesConfig,
  PoolAdvancedConfig,
  ProviderConfig,
  ProviderWithEndpointsSummary,
  ProxyConfig,
} from './types'
import {
  normalizeChatPiiRedactionProviderConfig as normalizeChatPiiRedactionProvider,
  normalizePoolAdvancedConfig as normalizePoolAdvanced,
} from './types'

interface ProviderRequestOptions {
  timeout?: number
}

interface ProviderReadOptions {
  timeout?: number
  cacheTtlMs?: number
}

/**
 * 获取 Providers 摘要（分页）
 */
export interface ProviderSummaryQuery {
  page?: number
  page_size?: number
  search?: string
  status?: string
  api_format?: string
  model_id?: string
}

export interface ProviderSummaryPageResponse {
  total: number
  page: number
  page_size: number
  items: ProviderWithEndpointsSummary[]
}

function normalizeProviderSummary(
  provider: ProviderWithEndpointsSummary,
): ProviderWithEndpointsSummary {
  return {
    ...provider,
    chat_pii_redaction: normalizeChatPiiRedactionProvider(provider.chat_pii_redaction),
    pool_advanced: normalizePoolAdvanced(provider.pool_advanced),
  }
}

export async function getProvidersSummary(
  params: ProviderSummaryQuery = {},
  options: ProviderReadOptions = {},
): Promise<ProviderSummaryPageResponse> {
  const cacheTtlMs = options.cacheTtlMs ?? 0
  const cacheKey = buildCacheKey('providers:summary', params as Record<string, unknown>)
  return cachedRequest(
    cacheKey,
    async () => {
      const response = await client.get<ProviderSummaryPageResponse>(
        '/api/admin/providers/summary',
        {
          params,
          timeout: options.timeout,
        },
      )
      return {
        ...response.data,
        items: response.data.items.map(normalizeProviderSummary),
      }
    },
    cacheTtlMs,
  )
}

/**
 * 获取单个 Provider 的详细信息
 */
export async function getProvider(providerId: string): Promise<ProviderWithEndpointsSummary> {
  const response = await client.get<ProviderWithEndpointsSummary>(`/api/admin/providers/${providerId}/summary`)
  return normalizeProviderSummary(response.data)
}

/**
 * 更新 Provider 基础配置
 */
export async function updateProvider(
  providerId: string,
  data: Partial<{
    name: string
    provider_type: 'custom' | 'vertex_ai' | 'claude_code' | 'codex' | 'chatgpt_web' | 'gemini_cli' | 'antigravity' | 'kiro'
    description: string | null
    website: string
    provider_priority: number
    keep_priority_on_conversion: boolean
    billing_type: 'monthly_quota' | 'pay_as_you_go' | 'free_tier'
    monthly_quota_usd: number
    quota_reset_day: number
    quota_last_reset_at: string  // 周期开始时间
    quota_expires_at: string
    rpm_limit: number | null
    // 请求配置（从 Endpoint 迁移）
    max_retries: number
    proxy: ProxyConfig | null
    cache_ttl_minutes: number  // 0表示不支持缓存，>0表示支持缓存并设置TTL(分钟)
    max_probe_interval_minutes: number
    enable_format_conversion: boolean  // 是否允许格式转换（提供商级别开关）
    is_active: boolean
    claude_code_advanced: ClaudeCodeAdvancedConfig | null
    pool_advanced: PoolAdvancedConfig | null
    failover_rules: FailoverRulesConfig | null
    config: ProviderConfig | null
  }>,
  requestOptions?: ProviderRequestOptions,
): Promise<ProviderWithEndpointsSummary> {
  const response = await client.patch(`/api/admin/providers/${providerId}`, data, requestOptions)
  return normalizeProviderSummary(response.data)
}

/**
 * 创建 Provider
 */
export async function createProvider(
  data: {
    name: string
    provider_type?: 'custom' | 'vertex_ai' | 'claude_code' | 'codex' | 'chatgpt_web' | 'gemini_cli' | 'antigravity' | 'kiro'
    description?: string
    website?: string
    billing_type?: 'monthly_quota' | 'pay_as_you_go' | 'free_tier'
    monthly_quota_usd?: number
    quota_reset_day?: number
    quota_last_reset_at?: string
    quota_expires_at?: string
    provider_priority?: number
    keep_priority_on_conversion?: boolean
    is_active?: boolean
    max_retries?: number
    stream_first_byte_timeout?: number | null
    request_timeout?: number | null
    proxy?: ProxyConfig | null
    claude_code_advanced?: ClaudeCodeAdvancedConfig | null
    pool_advanced?: PoolAdvancedConfig | null
    failover_rules?: FailoverRulesConfig | null
    config?: ProviderConfig | null
  }
): Promise<{ id: string; name: string; message?: string }> {
  const response = await client.post('/api/admin/providers/', data)
  return response.data
}

/**
 * 删除 Provider
 */
export interface ProviderDeleteSubmitResponse {
  task_id: string
  status: string
  message: string
}

export interface ProviderDeleteTaskResponse {
  task_id: string
  provider_id: string
  status: string
  stage: string
  total_keys: number
  deleted_keys: number
  total_endpoints: number
  deleted_endpoints: number
  message: string
}

export async function deleteProvider(providerId: string): Promise<ProviderDeleteSubmitResponse> {
  const response = await client.delete<ProviderDeleteSubmitResponse>(`/api/admin/providers/${providerId}`)
  return response.data
}

export async function getProviderDeleteTask(
  providerId: string,
  taskId: string,
): Promise<ProviderDeleteTaskResponse> {
  const response = await client.get<ProviderDeleteTaskResponse>(
    `/api/admin/providers/${providerId}/delete-task/${taskId}`,
  )
  return response.data
}

/**
 * 测试模型连接性
 */
export interface TestModelRequest {
  provider_id: string
  model_name: string
  api_key_id?: string
  endpoint_id?: string
  message?: string
  api_format?: string
  request_headers?: Record<string, unknown>
  request_body?: Record<string, unknown>
  request_id?: string
  concurrency?: number
}

export interface TestModelResponse {
  success: boolean
  error?: string
  data?: {
    response?: {
      status_code?: number
      error?: string | { message?: string }
      choices?: Array<{ message?: { content?: string } }>
    }
    content_preview?: string
  }
  provider?: {
    id: string
    name: string
  }
  model?: string
}

export async function testModel(
  data: TestModelRequest,
  options: { signal?: AbortSignal } = {},
): Promise<TestModelResponse> {
  const response = await client.post('/api/admin/provider-query/test-model', data, {
    timeout: 10 * 60 * 1000,
    signal: options.signal,
  })
  return response.data
}

/**
 * 带故障转移的模型测试
 */
export interface TestModelFailoverRequest {
  provider_id: string
  mode: 'global' | 'direct'
  model_name: string
  failover_models?: string[]
  api_format?: string
  endpoint_id?: string
  message?: string
  request_headers?: Record<string, unknown>
  request_body?: Record<string, unknown>
  request_id?: string
  concurrency?: number
}

export interface TestAttemptDetail {
  candidate_index: number
  retry_index?: number
  endpoint_api_format: string
  endpoint_base_url: string
  key_name: string | null
  key_id: string
  auth_type: string
  effective_model?: string | null
  status: 'success' | 'failed' | 'skipped' | 'cancelled' | 'pending' | 'streaming' | 'stream_interrupted' | 'available' | 'unused'
  skip_reason?: string | null
  error_message?: string | null
  status_code?: number | null
  latency_ms?: number | null
  request_url?: string | null
  request_headers?: Record<string, unknown> | null
  request_body?: unknown
  response_headers?: Record<string, unknown> | null
  response_body?: unknown
}

export interface TestModelFailoverResponse {
  success: boolean
  model: string
  provider: { id: string; name: string }
  attempts: TestAttemptDetail[]
  total_candidates: number
  total_attempts: number
  data?: Record<string, unknown> | null
  error?: string | null
}

export async function testModelFailover(
  data: TestModelFailoverRequest,
  options: { signal?: AbortSignal } = {}
): Promise<TestModelFailoverResponse> {
  const normalizedModelName = typeof data.model_name === 'string' ? data.model_name.trim() : ''
  const failoverModels = Array.isArray(data.failover_models) && data.failover_models.length > 0
    ? data.failover_models
    : (normalizedModelName ? [normalizedModelName] : undefined)
  const response = await client.post('/api/admin/provider-query/test-model-failover', {
    ...data,
    ...(failoverModels ? { failover_models: failoverModels } : {}),
  }, {
    timeout: 10 * 60 * 1000,
    signal: options.signal,
  })
  return response.data
}

/**
 * 映射预览相关类型
 */
export interface MappingMatchedModel {
  allowed_model: string
  mapping_pattern: string
}

export interface MappingMatchingGlobalModel {
  global_model_id: string
  global_model_name: string
  display_name: string
  is_active: boolean
  matched_models: MappingMatchedModel[]
}

export interface MappingMatchingKey {
  key_id: string
  key_name: string
  masked_key: string
  is_active: boolean
  allowed_models: string[]
  matching_global_models: MappingMatchingGlobalModel[]
}

export interface ProviderMappingPreviewResponse {
  provider_id: string
  provider_name: string
  keys: MappingMatchingKey[]
  total_keys: number
  total_matches: number
  // 截断提示
  truncated: boolean
  truncated_keys: number
  truncated_models: number
}

/**
 * 获取 Provider 映射预览
 */
export async function getProviderMappingPreview(
  providerId: string
): Promise<ProviderMappingPreviewResponse> {
  return dedupedRequest(`providers:mapping-preview:${providerId}`, async () => {
    const response = await client.get<ProviderMappingPreviewResponse>(`/api/admin/providers/${providerId}/mapping-preview`)
    return response.data
  })
}
