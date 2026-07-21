import { ref, computed, type Ref } from 'vue'
import { usageApi } from '@/api/usage'
import { meApi } from '@/api/me'
import type {
  UsageStatsState,
  ModelStatsItem,
  ProviderStatsItem,
  ApiFormatStatsItem,
  UsageRecord,
  DateRangeParams,
  EnhancedModelStatsItem
} from '../types'
import { createDefaultStats } from '../types'
import { log } from '@/utils/logger'
import { getErrorStatus } from '@/types/api-error'
import { isUsageProviderVisible, normalizeUsageProviderStats } from '../utils/providerStats'
import {
  mergeUsageRecordErrorMessage,
  mergeUsageRecordFirstByteTimeMs,
  mergeUsageRecordResponseTiming,
  parseUsageTimestampMs,
} from '../utils/recordSync'

export interface UseUsageDataOptions {
  isAdminPage: Ref<boolean>
}

export interface LoadStatsOptions {
  force?: boolean
  preserveOnFailure?: boolean
}

export interface PaginationParams {
  page: number
  pageSize: number
}

export interface FilterParams {
  search?: string
  user_id?: string
  model?: string
  provider?: string
  api_format?: string
  status?: string
  client_family?: string
  hideUnknownRecords?: boolean
}

export function useUsageData(options: UseUsageDataOptions) {
  const { isAdminPage } = options

  // 加载状态
  const isLoadingStats = ref(true)
  const isLoadingRecords = ref(false)
  const loading = computed(() => isLoadingStats.value || isLoadingRecords.value)

  // 统计数据
  const stats = ref<UsageStatsState>(createDefaultStats())
  const modelStats = ref<ModelStatsItem[]>([])
  const providerStats = ref<ProviderStatsItem[]>([])
  const apiFormatStats = ref<ApiFormatStatsItem[]>([])

  // 记录数据 - 只存储当前页
  const currentRecords = ref<UsageRecord[]>([])
  const totalRecords = ref(0)

  // 当前的日期范围（用于分页请求）
  const currentDateRange = ref<DateRangeParams | undefined>(undefined)
  let loadStatsRequestId = 0
  let loadRecordsRequestId = 0
  let currentAdminRecordTotalState: { key: string; exact: boolean } | null = null
  const adminRecordExactTotalCache = new Map<string, number>()

  // 可用的筛选选项（从统计数据获取，而不是从记录中）
  const availableModels = ref<string[]>([])
  const availableProviders = ref<string[]>([])

  // 增强的模型统计（包含效率分析）
  const enhancedModelStats = computed<EnhancedModelStatsItem[]>(() => {
    return modelStats.value.map(model => ({
      ...model,
      costPerToken: model.total_tokens > 0
        ? `$${(model.total_cost / model.total_tokens * 1000000).toFixed(2)}/M`
        : '-'
    }))
  })

  function buildAdminRecordTotalKey(params: Record<string, unknown>): string {
    return JSON.stringify(
      Object.entries(params)
        .filter(([key, value]) =>
          !['limit', 'offset', 'include_total', 'total_only'].includes(key) &&
          value !== undefined &&
          value !== null &&
          value !== ''
        )
        .sort(([left], [right]) => left.localeCompare(right))
    )
  }

  function applyAdminRecordTotal(
    totalKey: string,
    total: number,
    totalIsEstimated: boolean
  ) {
    const normalizedTotal = Number.isFinite(total) && total > 0 ? total : 0

    if (!totalIsEstimated) {
      totalRecords.value = normalizedTotal
      currentAdminRecordTotalState = { key: totalKey, exact: true }
      adminRecordExactTotalCache.set(totalKey, normalizedTotal)
      return
    }

    const cachedExactTotal = adminRecordExactTotalCache.get(totalKey)
    if (cachedExactTotal !== undefined) {
      totalRecords.value = cachedExactTotal
      currentAdminRecordTotalState = { key: totalKey, exact: true }
      return
    }

    const previousTotal = currentAdminRecordTotalState?.key === totalKey
      ? totalRecords.value
      : 0
    totalRecords.value = Math.max(previousTotal, normalizedTotal)
    currentAdminRecordTotalState = { key: totalKey, exact: false }
  }

  // 加载统计数据（不加载记录）
  async function loadStats(dateRange?: DateRangeParams, options: LoadStatsOptions = {}): Promise<boolean> {
    const requestId = ++loadStatsRequestId
    isLoadingStats.value = true
    currentDateRange.value = dateRange

    try {
      if (isAdminPage.value) {
        // 管理员页面顺序加载统计数据，避免刷新使用记录时瞬时打满后端 worker。
        if (!options.preserveOnFailure) {
          stats.value = createDefaultStats()
          modelStats.value = []
          providerStats.value = []
          apiFormatStats.value = []
          availableModels.value = []
          availableProviders.value = []
        }

        let hadFailure = false
        const requestOptions = options.force ? { skipCache: true } : undefined
        const markFailure = (error: unknown) => {
          hadFailure = true
          if (getErrorStatus(error) !== 403) {
            log.error('加载统计数据失败:', error)
          }
        }

        try {
          const statsData = await usageApi.getUsageStats(dateRange, requestOptions)
          if (requestId !== loadStatsRequestId) {
            return true
          }

          // statsData may contain additional fields not declared in UsageStats
          const statsRaw = statsData as Record<string, unknown>
          stats.value = {
            total_requests: statsData.total_requests || 0,
            total_tokens: statsData.total_tokens || 0,
            total_cost: statsData.total_cost || 0,
            total_actual_cost: statsData.total_actual_cost,
            avg_response_time: statsData.avg_response_time || 0,
            error_count: typeof statsRaw.error_count === 'number' ? statsRaw.error_count : undefined,
            error_rate: typeof statsRaw.error_rate === 'number' ? statsRaw.error_rate : undefined,
            cache_stats: statsRaw.cache_stats as UsageStatsState['cache_stats'],
            period_start: '',
            period_end: '',
          }
        } catch (error) {
          if (requestId !== loadStatsRequestId) {
            return true
          }
          markFailure(error)
        }

        try {
          const modelData = await usageApi.getUsageByModel(dateRange, requestOptions)
          if (requestId !== loadStatsRequestId) {
            return true
          }

          modelStats.value = modelData.map(item => {
            const raw = item as Record<string, unknown>
            return {
              model: item.model,
              request_count: item.request_count || 0,
              total_tokens: item.total_tokens || 0,
              effective_input_tokens: typeof raw.effective_input_tokens === 'number' ? raw.effective_input_tokens : 0,
              total_input_context: typeof raw.total_input_context === 'number' ? raw.total_input_context : 0,
              output_tokens: typeof raw.output_tokens === 'number' ? raw.output_tokens : 0,
              cache_read_tokens: typeof raw.cache_read_tokens === 'number' ? raw.cache_read_tokens : 0,
              cache_creation_tokens: typeof raw.cache_creation_tokens === 'number' ? raw.cache_creation_tokens : 0,
              cache_hit_rate: typeof raw.cache_hit_rate === 'number' ? raw.cache_hit_rate : 0,
              total_cost: item.total_cost || 0,
              actual_cost: typeof raw.actual_cost === 'number' ? raw.actual_cost : undefined
            }
          })

          availableModels.value = modelData.map(item => item.model).filter(Boolean).sort()
        } catch (error) {
          if (requestId !== loadStatsRequestId) {
            return true
          }
          markFailure(error)
        }

        try {
          const providerData = await usageApi.getUsageByProvider(dateRange, requestOptions)
          if (requestId !== loadStatsRequestId) {
            return true
          }

          providerStats.value = normalizeUsageProviderStats(providerData)
          availableProviders.value = providerStats.value.map(item => item.provider).sort()
        } catch (error) {
          if (requestId !== loadStatsRequestId) {
            return true
          }
          markFailure(error)
        }

        try {
          const apiFormatData = await usageApi.getUsageByApiFormat(dateRange, requestOptions)
          if (requestId !== loadStatsRequestId) {
            return true
          }

          apiFormatStats.value = apiFormatData.map(item => ({
            api_format: item.api_format,
            request_count: item.request_count || 0,
            total_tokens: item.total_tokens || 0,
            effective_input_tokens: item.effective_input_tokens || 0,
            total_input_context: item.total_input_context || 0,
            output_tokens: item.output_tokens || 0,
            cache_read_tokens: item.cache_read_tokens || 0,
            cache_creation_tokens: item.cache_creation_tokens || 0,
            cache_hit_rate: item.cache_hit_rate || 0,
            total_cost: item.total_cost || 0,
            actual_cost: item.actual_cost,
            avgResponseTime: item.avg_response_time_ms > 0
              ? `${(item.avg_response_time_ms / 1000).toFixed(2)}s`
              : '-'
          }))
        } catch (error) {
          if (requestId !== loadStatsRequestId) {
            return true
          }
          markFailure(error)
        }

        return hadFailure
      }

      // 用户页面
      const userData = await meApi.getUsage(dateRange)
      if (requestId !== loadStatsRequestId) {
        return false
      }

      stats.value = {
        total_requests: userData.total_requests || 0,
        total_tokens: userData.total_tokens || 0,
        total_cost: userData.total_cost || 0,
        total_actual_cost: userData.total_actual_cost,
        avg_response_time: userData.avg_response_time || 0,
        period_start: '',
        period_end: '',
      }

      modelStats.value = (userData.summary_by_model || []).map((item) => ({
        model: item.model,
        request_count: item.requests || 0,
        total_tokens: item.total_tokens || 0,
        effective_input_tokens: item.effective_input_tokens || 0,
        total_input_context: item.total_input_context || 0,
        output_tokens: item.output_tokens || 0,
        cache_read_tokens: item.cache_read_tokens || 0,
        cache_creation_tokens: item.cache_creation_tokens || 0,
        cache_hit_rate: item.cache_hit_rate || 0,
        total_cost: item.total_cost_usd || 0,
        actual_cost: item.actual_total_cost_usd
      }))

      providerStats.value = (userData.summary_by_provider || [])
        .filter((item) => isUsageProviderVisible(item.provider))
        .map((item) => ({
          provider: item.provider,
          requests: item.requests || 0,
          totalTokens: item.total_tokens || 0,
          effectiveInputTokens: item.effective_input_tokens || 0,
          totalInputContext: item.total_input_context || 0,
          outputTokens: item.output_tokens || 0,
          cacheReadTokens: item.cache_read_tokens || 0,
          cacheCreationTokens: item.cache_creation_tokens || 0,
          cacheHitRate: item.cache_hit_rate || 0,
          totalCost: item.total_cost_usd || 0,
          successRate: item.success_rate || 0,
          avgResponseTime: (item.avg_response_time_ms ?? 0) > 0
            ? `${((item.avg_response_time_ms ?? 0) / 1000).toFixed(2)}s`
            : '-'
        }))

      // 用户页面：记录直接从 userData 获取（数量较少）
      // 使用 mergeRecordStatus 保护已有的活跃状态，避免轮询更新被覆盖
      const nextRecords = (userData.records || []) as UsageRecord[]
      currentRecords.value = mergeRecordStatus(currentRecords.value, nextRecords)
      totalRecords.value = userData.pagination?.total ?? currentRecords.value.length

      // 从记录中提取筛选选项
      const models = new Set<string>()
      const providers = new Set<string>()
      currentRecords.value.forEach(record => {
        if (record.model) models.add(record.model)
        if (isUsageProviderVisible(record.provider)) providers.add(record.provider)
      })
      availableModels.value = Array.from(models).sort()
      availableProviders.value = Array.from(providers).sort()

      // API 格式统计直接使用后端聚合数据
      apiFormatStats.value = (userData.summary_by_api_format || []).map(item => ({
        api_format: item.api_format,
        request_count: item.request_count || 0,
        total_tokens: item.total_tokens || 0,
        effective_input_tokens: item.effective_input_tokens || 0,
        total_input_context: item.total_input_context || 0,
        output_tokens: item.output_tokens || 0,
        cache_read_tokens: item.cache_read_tokens || 0,
        cache_creation_tokens: item.cache_creation_tokens || 0,
        cache_hit_rate: item.cache_hit_rate || 0,
        total_cost: item.total_cost_usd || 0,
        avgResponseTime: (item.avg_response_time_ms ?? 0) > 0
          ? `${((item.avg_response_time_ms ?? 0) / 1000).toFixed(2)}s`
          : '-'
      }))

      return false
    } catch (error: unknown) {
      if (requestId !== loadStatsRequestId) {
        return true
      }
      if (getErrorStatus(error) !== 403) {
        log.error('加载统计数据失败:', error)
      }
      if (!isAdminPage.value) {
        stats.value = createDefaultStats()
        modelStats.value = []
        // 用户页的 records 依赖 stats 一起加载；管理员页的 records 是独立分页，不应被统计失败清空。
        currentRecords.value = []
        totalRecords.value = 0
      }
      return true
    } finally {
      if (requestId === loadStatsRequestId) {
        isLoadingStats.value = false
      }
    }
  }

  // 加载记录（真正的后端分页）
  async function loadRecords(
    pagination: PaginationParams,
    filters?: FilterParams,
    dateRange?: DateRangeParams
  ): Promise<void> {
    const requestId = ++loadRecordsRequestId
    isLoadingRecords.value = true

    try {
      const offset = (pagination.page - 1) * pagination.pageSize
      const effectiveDateRange = dateRange ?? currentDateRange.value
      if (dateRange) {
        currentDateRange.value = dateRange
      }

      // 构建请求参数
      const params: Record<string, unknown> = {
        limit: pagination.pageSize,
        offset,
        ...effectiveDateRange
      }

      // 添加筛选条件
      if (filters?.search?.trim()) {
        params.search = filters.search.trim()
      }

      if (isAdminPage.value) {
        // 管理员页面：使用管理员 API
        if (filters?.user_id) {
          params.user_id = filters.user_id
        }
        if (filters?.model) {
          params.model = filters.model
        }
        if (filters?.provider) {
          params.provider = filters.provider
        }
        if (filters?.api_format) {
          params.api_format = filters.api_format
        }
        if (filters?.status) {
          params.status = filters.status
        }
        if (filters?.client_family) {
          params.client_family = filters.client_family
        }
        if (filters?.hideUnknownRecords) {
          params.hide_unknown = true
        }

        const response = await usageApi.getAllUsageRecords({
          ...params,
          include_total: false,
        })
        if (requestId !== loadRecordsRequestId) {
          return
        }
        const nextRecords = (response.records || []) as UsageRecord[]
        currentRecords.value = mergeRecordStatus(currentRecords.value, nextRecords)
        const totalKey = buildAdminRecordTotalKey(params)
        applyAdminRecordTotal(totalKey, response.total ?? 0, response.total_is_estimated === true)
        if (response.total_is_estimated === true) {
          void refreshAdminRecordTotal(params, requestId, totalKey)
        }
      } else {
        // 用户页面：使用用户 API
        const userData = await meApi.getUsage(params)
        if (requestId !== loadRecordsRequestId) {
          return
        }
        const nextRecords = (userData.records || []) as UsageRecord[]
        currentRecords.value = mergeRecordStatus(currentRecords.value, nextRecords)
        totalRecords.value = userData.pagination?.total || currentRecords.value.length
      }
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

  async function refreshAdminRecordTotal(
    params: Record<string, unknown>,
    requestId: number,
    totalKey: string
  ): Promise<void> {
    try {
      const total = await usageApi.getAllUsageRecordTotal(params)
      if (requestId === loadRecordsRequestId) {
        adminRecordExactTotalCache.set(totalKey, total)
        currentAdminRecordTotalState = { key: totalKey, exact: true }
        totalRecords.value = total
      }
    } catch (error) {
      log.warn('加载使用记录总数失败:', error)
    }
  }

  function mergeSparseRecordMetric(
    existingValue: number | null | undefined,
    nextValue: number | null | undefined
  ): number | null | undefined {
    const existingIsPositive = typeof existingValue === 'number' && Number.isFinite(existingValue) && existingValue > 0
    const nextIsPositive = typeof nextValue === 'number' && Number.isFinite(nextValue) && nextValue > 0

    if (nextIsPositive) {
      return nextValue
    }
    if (existingIsPositive) {
      return existingValue
    }
    return existingValue ?? nextValue
  }

  function mergeBooleanTrueWins(
    existingValue: boolean | null | undefined,
    nextValue: boolean | null | undefined
  ): boolean | undefined {
    if (existingValue === true || nextValue === true) {
      return true
    }
    if (typeof nextValue === 'boolean') {
      return nextValue
    }
    if (typeof existingValue === 'boolean') {
      return existingValue
    }
    return undefined
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
      const existingUpdatedAtMs = parseUsageTimestampMs(existing.updated_at)
      const nextUpdatedAtMs = parseUsageTimestampMs(record.updated_at)
      const nextStatusSnapshotIsStale = existingUpdatedAtMs != null &&
        nextUpdatedAtMs != null &&
        nextUpdatedAtMs < existingUpdatedAtMs
      const sameRankTerminalTransition = currentRank === 2 && nextRank === 2
      const statusProgressed = hasNextStatus && !nextStatusSnapshotIsStale && (
        !hasExistingStatus ||
        nextRank > currentRank ||
        (nextRank === currentRank && (
          existing.status === record.status || sameRankTerminalTransition
        ))
      )
      const mergedStatus = statusProgressed ? record.status : existing.status

      // 确定是否需要保护 provider（避免 pending/unknown/unknow 覆盖已有的正确值）
      const isPendingProvider = !isUsageProviderVisible(record.provider)
      const hasValidExistingProvider = isUsageProviderVisible(existing.provider)
      const protectProvider = isPendingProvider && hasValidExistingProvider

      const recordUpstreamIsStream = typeof record.upstream_is_stream === 'boolean'
        ? record.upstream_is_stream
        : typeof record.is_stream === 'boolean'
          ? record.is_stream
          : undefined
      const existingUpstreamIsStream = typeof existing.upstream_is_stream === 'boolean'
        ? existing.upstream_is_stream
        : typeof existing.is_stream === 'boolean'
          ? existing.is_stream
          : undefined
      const upstreamIsStream = mergeBooleanTrueWins(existingUpstreamIsStream, recordUpstreamIsStream) ?? false

      const recordClientRequestedStream = typeof record.client_requested_stream === 'boolean'
        ? record.client_requested_stream
        : typeof record.client_is_stream === 'boolean'
          ? record.client_is_stream
          : undefined
      const existingClientRequestedStream = typeof existing.client_requested_stream === 'boolean'
        ? existing.client_requested_stream
        : typeof existing.client_is_stream === 'boolean'
          ? existing.client_is_stream
          : undefined
      const clientRequestedStream = mergeBooleanTrueWins(existingClientRequestedStream, recordClientRequestedStream)

      const recordClientIsStream = typeof record.client_is_stream === 'boolean'
        ? record.client_is_stream
        : typeof record.client_requested_stream === 'boolean'
          ? record.client_requested_stream
          : undefined
      const existingClientIsStream = typeof existing.client_is_stream === 'boolean'
        ? existing.client_is_stream
        : typeof existing.client_requested_stream === 'boolean'
          ? existing.client_requested_stream
          : undefined
      const clientIsStream = mergeBooleanTrueWins(existingClientIsStream, recordClientIsStream) ?? clientRequestedStream
      const nextTimingIsAuthoritative = statusProgressed &&
        (record.status === 'completed' || record.status === 'failed' || record.status === 'cancelled')
      const responseTiming = mergeUsageRecordResponseTiming(
        {
          response_time_ms: existing.response_time_ms,
          response_time_updated_at: existing.response_time_updated_at,
        },
        {
          response_time_ms: record.response_time_ms,
          response_time_updated_at: record.response_time_updated_at,
        },
        { preferNext: nextTimingIsAuthoritative },
      )

      return {
        ...record,
        // 保留详情抽屉/活跃轮询已经拿到的完整指标，避免列表刷新用 0 或空值回退。
        status: mergedStatus,
        provider: statusProgressed
          ? (protectProvider ? existing.provider : (record.provider || existing.provider))
          : existing.provider,
        input_tokens: mergeSparseRecordMetric(existing.input_tokens, record.input_tokens) ?? record.input_tokens,
        effective_input_tokens: mergeSparseRecordMetric(existing.effective_input_tokens, record.effective_input_tokens) ?? record.effective_input_tokens,
        output_tokens: mergeSparseRecordMetric(existing.output_tokens, record.output_tokens) ?? record.output_tokens,
        reasoning_tokens: mergeSparseRecordMetric(existing.reasoning_tokens, record.reasoning_tokens) ?? record.reasoning_tokens,
        total_tokens: mergeSparseRecordMetric(existing.total_tokens, record.total_tokens) ?? record.total_tokens,
        cache_creation_input_tokens: mergeSparseRecordMetric(existing.cache_creation_input_tokens, record.cache_creation_input_tokens) ?? record.cache_creation_input_tokens,
        cache_creation_ephemeral_5m_input_tokens:
          mergeSparseRecordMetric(
            existing.cache_creation_ephemeral_5m_input_tokens,
            record.cache_creation_ephemeral_5m_input_tokens
          ) ?? record.cache_creation_ephemeral_5m_input_tokens,
        cache_creation_ephemeral_1h_input_tokens:
          mergeSparseRecordMetric(
            existing.cache_creation_ephemeral_1h_input_tokens,
            record.cache_creation_ephemeral_1h_input_tokens
          ) ?? record.cache_creation_ephemeral_1h_input_tokens,
        cache_read_input_tokens: mergeSparseRecordMetric(existing.cache_read_input_tokens, record.cache_read_input_tokens) ?? record.cache_read_input_tokens,
        cost: mergeSparseRecordMetric(existing.cost, record.cost) ?? record.cost,
        actual_cost: mergeSparseRecordMetric(existing.actual_cost, record.actual_cost) ?? record.actual_cost,
        response_time_ms: responseTiming.response_time_ms,
        first_byte_time_ms: mergeUsageRecordFirstByteTimeMs(
          existing.first_byte_time_ms,
          record.first_byte_time_ms,
        ),
        updated_at: statusProgressed
          ? (record.updated_at ?? existing.updated_at)
          : existing.updated_at,
        response_time_updated_at: responseTiming.response_time_updated_at,
        // Status, code and error are one lifecycle snapshot. An accepted full
        // list snapshot may clear an earlier candidate's 400/Cyber failure;
        // a rejected stale status snapshot must not mutate either field.
        status_code: statusProgressed
          ? (record.status_code ?? undefined)
          : existing.status_code,
        error_message: statusProgressed
          ? mergeUsageRecordErrorMessage(
            existing.error_message,
            record.error_message,
            { authoritative: true },
          )
          : existing.error_message,
        image_progress: statusProgressed
          ? (record.image_progress ?? existing.image_progress)
          : existing.image_progress,
        is_stream: upstreamIsStream,
        upstream_is_stream: upstreamIsStream,
        client_requested_stream: clientRequestedStream,
        client_is_stream: clientIsStream,
        api_format: record.api_format || existing.api_format,
        endpoint_api_format: record.endpoint_api_format || existing.endpoint_api_format,
        has_format_conversion: record.has_format_conversion ?? existing.has_format_conversion,
        has_fallback: existing.has_fallback === true || record.has_fallback === true,
        has_retry: existing.has_retry === true || record.has_retry === true,
        api_key_name: record.api_key_name || existing.api_key_name,
        provider_key_name: statusProgressed
          ? (record.provider_key_name || existing.provider_key_name)
          : existing.provider_key_name,
        rate_multiplier: statusProgressed
          ? (record.rate_multiplier ?? existing.rate_multiplier)
          : existing.rate_multiplier,
        // Full list snapshots describe the final provider candidate. Missing/null means the
        // final request did not map the model and must clear an earlier candidate's arrow.
        target_model: statusProgressed
          ? (typeof record.target_model === 'string' && record.target_model.trim()
              ? record.target_model
              : null)
          : existing.target_model,
        // Request type is client-request identity, not a provider-candidate fact. Preserve a
        // known compact operation when a later sparse snapshot omits it.
        request_type:
          typeof record.request_type === 'string' && record.request_type.trim()
            ? record.request_type
            : existing.request_type,
        requested_reasoning_effort:
          typeof record.requested_reasoning_effort === 'string'
            && record.requested_reasoning_effort.trim()
            ? record.requested_reasoning_effort
            : existing.requested_reasoning_effort,
        // Provider reasoning belongs to the final candidate just like service_tier; do not
        // retain a previous candidate's `max` when the final request has no reasoning field.
        reasoning_effort: statusProgressed
          ? (typeof record.reasoning_effort === 'string' && record.reasoning_effort.trim()
              ? record.reasoning_effort
              : null)
          : existing.reasoning_effort,
        // The list response is the authoritative snapshot of the final provider request. Do not
        // carry a tier forward when this response has no tier; doing so can leave a stale Fast
        // badge after the final upstream request falls back to Standard.
        service_tier: statusProgressed
          ? (typeof record.service_tier === 'string' && record.service_tier.trim()
              ? record.service_tier
              : null)
          : existing.service_tier,
        actual_service_tier: statusProgressed
          ? (typeof record.actual_service_tier === 'string' && record.actual_service_tier.trim()
              ? record.actual_service_tier
              : null)
          : existing.actual_service_tier
      }
    })
  }

  // 刷新所有数据
  async function refreshData(dateRange?: DateRangeParams) {
    await loadStats(dateRange)
  }

  return {
    // 状态
    loading,
    isLoadingStats,
    isLoadingRecords,
    stats,
    modelStats,
    providerStats,
    apiFormatStats,
    currentRecords,
    totalRecords,

    // 筛选选项
    availableModels,
    availableProviders,

    // 计算属性
    enhancedModelStats,

    // 方法
    loadStats,
    loadRecords,
    refreshData
  }
}
