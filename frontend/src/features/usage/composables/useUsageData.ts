import { ref, type Ref } from 'vue'
import { analyticsApi, type AnalyticsBaseRequest, type AnalyticsFilterOption, type AnalyticsRecord, type AnalyticsScope } from '@/api/analytics'
import { buildTimeRangeParams } from '@/composables/useAnalyticsFilters'
import type {
  UsageRecord,
  DateRangeParams,
} from '../types'
import { log } from '@/utils/logger'
import { getErrorStatus } from '@/types/api-error'

export interface UseUsageDataOptions {
  isAdminPage: Ref<boolean>
}

export interface PaginationParams {
  page: number
  pageSize: number
}

export interface FilterParams {
  search?: string
  user_id?: string
  api_key_id?: string
  model?: string
  provider?: string
  api_format?: string
  status?: string
}

type FilterOptionDimension = 'user' | 'api_key' | 'model' | 'provider' | 'api_format' | 'status'

export function useUsageData(options: UseUsageDataOptions) {
  const { isAdminPage } = options

  const isLoadingRecords = ref(false)

  // 记录数据：前后端统一按当前页维护
  const currentRecords = ref<UsageRecord[]>([])
  const totalRecords = ref(0)

  // 当前的日期范围（用于分页请求）
  const currentDateRange = ref<DateRangeParams | undefined>(undefined)
  let loadStatsRequestId = 0
  let loadRecordsRequestId = 0

  // 可用的筛选选项（从统计数据获取，而不是从记录中）
  const availableUsers = ref<AnalyticsFilterOption[]>([])
  const availableModels = ref<AnalyticsFilterOption[]>([])
  const availableProviders = ref<AnalyticsFilterOption[]>([])
  const availableApiKeys = ref<AnalyticsFilterOption[]>([])
  const availableApiFormats = ref<AnalyticsFilterOption[]>([])
  const availableStatuses = ref<AnalyticsFilterOption[]>([])

  function mapAnalyticsRecord(record: AnalyticsRecord): UsageRecord {
    return {
      id: record.id,
      user_id: record.user_id || undefined,
      username: record.username || undefined,
      api_key: {
        id: record.api_key_id,
        name: record.api_key_name,
        display: null,
      },
      provider: record.provider_name,
      api_key_name: record.provider_api_key_name || undefined,
      rate_multiplier: record.rate_multiplier,
      model: record.model,
      target_model: record.target_model,
      api_format: record.api_format || undefined,
      endpoint_api_format: undefined,
      has_format_conversion: record.has_format_conversion ?? undefined,
      input_tokens: record.input_tokens,
      output_tokens: record.output_tokens,
      cache_creation_input_tokens: record.cache_creation_input_tokens,
      cache_read_input_tokens: record.cache_read_input_tokens,
      total_tokens: record.total_tokens,
      cost: record.total_cost_usd,
      actual_cost: record.actual_total_cost_usd,
      response_time_ms: record.response_time_ms ?? undefined,
      first_byte_time_ms: record.first_byte_time_ms ?? undefined,
      is_stream: record.is_stream,
      status_code: record.status_code ?? undefined,
      error_message: record.error_message ?? undefined,
      status: record.status as UsageRecord['status'],
      created_at: record.created_at || '',
      has_fallback: record.has_fallback ?? undefined,
      has_retry: record.has_retry ?? undefined,
    }
  }

  function buildScope(): AnalyticsScope {
    if (isAdminPage.value) {
      return { kind: 'global' }
    }
    return { kind: 'me' }
  }

  function buildAnalyticsFilters(
    filters?: FilterParams,
    excludeDimension?: FilterOptionDimension,
  ): NonNullable<AnalyticsBaseRequest['filters']> {
    return {
      user_ids: filters?.user_id && excludeDimension !== 'user' ? [filters.user_id] : [],
      models: filters?.model && excludeDimension !== 'model' ? [filters.model] : [],
      provider_names: filters?.provider && excludeDimension !== 'provider' ? [filters.provider] : [],
      api_key_ids: filters?.api_key_id && excludeDimension !== 'api_key' ? [filters.api_key_id] : [],
      api_formats: filters?.api_format && excludeDimension !== 'api_format' ? [filters.api_format] : [],
      statuses: filters?.status && excludeDimension !== 'status' ? [filters.status] : [],
    }
  }

  // 加载统计数据（不加载记录）
  async function loadStats(dateRange?: DateRangeParams, filters?: FilterParams) {
    const requestId = ++loadStatsRequestId
    currentDateRange.value = dateRange

    try {
      const response = await analyticsApi.getFilterOptions({
        scope: buildScope(),
        time_range: buildTimeRangeParams(dateRange || currentDateRange.value || {}),
        filters: buildAnalyticsFilters(filters),
      })

      if (requestId !== loadStatsRequestId) {
        return
      }

      availableUsers.value = response.users ?? []
      availableModels.value = response.models ?? []
      availableProviders.value = response.providers ?? []
      availableApiKeys.value = response.api_keys ?? []
      availableApiFormats.value = response.api_formats ?? []
      availableStatuses.value = response.statuses ?? []
    } catch (error: unknown) {
      if (requestId !== loadStatsRequestId) {
        return
      }
      if (getErrorStatus(error) !== 403) {
        log.error('加载统计数据失败:', error)
      }
      availableUsers.value = []
      availableModels.value = []
      availableProviders.value = []
      currentRecords.value = []
      availableApiKeys.value = []
      availableApiFormats.value = []
      availableStatuses.value = []
    }
  }

  // 加载记录：前后端统一由后端分页和筛选
  async function loadRecords(
    pagination: PaginationParams,
    filters?: FilterParams
  ): Promise<void> {
    const requestId = ++loadRecordsRequestId
    isLoadingRecords.value = true

    try {
      const offset = (pagination.page - 1) * pagination.pageSize

      const response = await analyticsApi.getRecords({
        scope: buildScope(),
        time_range: buildTimeRangeParams(currentDateRange.value || {}),
        filters: buildAnalyticsFilters(filters),
        search: {
          text: filters?.search || null,
        },
        pagination: {
          limit: pagination.pageSize,
          offset,
        },
      })
      if (requestId !== loadRecordsRequestId) {
        return
      }
      const nextRecords = (response.records || []).map(mapAnalyticsRecord)
      currentRecords.value = mergeRecordStatus(currentRecords.value, nextRecords)
      totalRecords.value = response.total || 0
    } catch (error) {
      if (requestId !== loadRecordsRequestId) {
        return
      }
      log.error('加载记录失败:', error)
      currentRecords.value = []
      totalRecords.value = 0
    } finally {
      if (requestId === loadRecordsRequestId) {
        isLoadingRecords.value = false
      }
    }
  }

  function mergeRecordStatus(
    current: UsageRecord[],
    next: UsageRecord[]
  ): UsageRecord[] {
    if (!current.length) return next
    const statusPriority: Record<string, number> = {
      pending: 0,
      streaming: 1,
      completed: 2,
      failed: 2,
      cancelled: 2
    }
    const currentById = new Map<string, UsageRecord>(
      current.map(record => [record.id, record])
    )
    return next.map(record => {
      const existing = currentById.get(record.id)
      if (!existing) return record

      // 确定是否需要保护 status（避免刷新把已知状态覆盖为 undefined 或回退）
      const hasExistingStatus = typeof existing.status === 'string' && existing.status.length > 0
      const hasNextStatus = typeof record.status === 'string' && record.status.length > 0
      const currentRank = hasExistingStatus ? (statusPriority[existing.status] ?? -1) : -1
      const nextRank = hasNextStatus ? (statusPriority[record.status] ?? -1) : -1
      const statusProgressed = hasNextStatus && (
        !hasExistingStatus ||
        nextRank > currentRank ||
        (nextRank === currentRank && existing.status === record.status)
      )
      const mergedStatus = statusProgressed ? record.status : existing.status
      const protectStatus = mergedStatus !== record.status

      // 确定是否需要保护 provider（避免 pending/unknown 覆盖已有的正确值）
      const isPendingProvider = !record.provider || record.provider === 'pending' || record.provider === 'unknown'
      const hasValidExistingProvider = existing.provider && existing.provider !== 'pending' && existing.provider !== 'unknown'
      const protectProvider = isPendingProvider && hasValidExistingProvider

      // 如果需要保护状态，说明本地数据比后端更新，应该保留本地的所有实时更新字段
      if (protectStatus) {
        return {
          ...record,
          // 保留本地的状态和所有通过轮询更新的字段
          status: mergedStatus,
          provider: protectProvider ? existing.provider : (record.provider || existing.provider),
          input_tokens: existing.input_tokens || record.input_tokens,
          output_tokens: existing.output_tokens || record.output_tokens,
          cache_creation_input_tokens: existing.cache_creation_input_tokens ?? record.cache_creation_input_tokens,
          cache_read_input_tokens: existing.cache_read_input_tokens ?? record.cache_read_input_tokens,
          cost: existing.cost || record.cost,
          actual_cost: existing.actual_cost ?? record.actual_cost,
          response_time_ms: existing.response_time_ms ?? record.response_time_ms,
          first_byte_time_ms: existing.first_byte_time_ms ?? record.first_byte_time_ms,
          api_format: existing.api_format || record.api_format,
          endpoint_api_format: existing.endpoint_api_format || record.endpoint_api_format,
          has_format_conversion: existing.has_format_conversion ?? record.has_format_conversion,
          api_key_name: existing.api_key_name || record.api_key_name,
          rate_multiplier: existing.rate_multiplier ?? record.rate_multiplier,
          target_model: existing.target_model || record.target_model
        }
      }

      // 只需要保护 provider
      if (protectProvider) {
        return {
          ...record,
          provider: existing.provider
        }
      }

      return record
    })
  }

  return {
    isLoadingRecords,
    currentRecords,
    totalRecords,

    availableUsers,
    availableModels,
    availableProviders,
    availableApiKeys,
    availableApiFormats,
    availableStatuses,

    loadStats,
    loadRecords,
  }
}
