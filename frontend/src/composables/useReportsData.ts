import { computed, ref, watch, onMounted, onUnmounted, type Ref } from 'vue'
import {
  analyticsApi,
  type AnalyticsBreakdownRow,
  type AnalyticsFilterOption,
  type AnalyticsScope,
  type AnalyticsSummary,
  type AnalyticsTimeseriesBucket,
} from '@/api/analytics'
import type { DateRangeParams, PeriodValue } from '@/features/usage/types'
import { getDateRangeFromPeriod } from '@/features/usage/composables'
import { createLoader, createRequestGuard, buildTimeRangeParams } from '@/composables/useAnalyticsFilters'
import { fillMissingTimeseriesBuckets } from '@/utils/analyticsTimeseries'
import { getAnalyticsRangeDaysInclusive, resolveAnalyticsAutoGranularity, type ResolvedAnalyticsGranularity } from '@/utils/analyticsGranularity'
import { type DailyUsageBreakdown } from '@/utils/usageBreakdown'

export interface ModelSummary {
  model: string
  requests: number
  tokens: number
  cost: number
  inputTokens: number
  outputTokens: number
  cacheCreationTokens: number
  cacheReadTokens: number
  cacheHitRate: number
  avg_response_time?: number
  avg_first_byte_time?: number
}

export interface ReportBucketStat {
  bucket_start: string
  bucket_end: string
  requests: number
  tokens: number
  cost: number
  avg_response_time: number
  avg_first_byte_time: number
  models_used_count: number
}

function mapBucketToReportStat(bucket: AnalyticsTimeseriesBucket): ReportBucketStat {
  return {
    bucket_start: bucket.bucket_start,
    bucket_end: bucket.bucket_end,
    requests: bucket.requests_total,
    tokens: bucket.total_tokens,
    cost: bucket.total_cost_usd,
    avg_response_time: bucket.avg_response_time_ms / 1000,
    avg_first_byte_time: bucket.avg_first_byte_time_ms / 1000,
    models_used_count: bucket.models_used_count,
  }
}

function mapBucketToBreakdown(bucket: AnalyticsTimeseriesBucket): DailyUsageBreakdown {
  return {
    date: bucket.bucket_start,
    inputTokens: bucket.input_tokens,
    outputTokens: bucket.output_tokens,
    cacheCreationTokens: bucket.cache_creation_input_tokens,
    cacheReadTokens: bucket.cache_read_input_tokens,
    cacheCreationCost: bucket.cache_creation_cost_usd,
    cacheReadCost: bucket.cache_read_cost_usd,
    cacheHitRate: bucket.cache_hit_rate,
    totalCacheCost: bucket.cache_cost_usd,
    totalCacheTokens: bucket.cache_creation_input_tokens + bucket.cache_read_input_tokens,
    totalTrackedTokens: bucket.total_tokens,
    baseTokens: bucket.input_output_total_tokens,
  }
}

function mapBreakdownRowToModelSummary(row: AnalyticsBreakdownRow): ModelSummary {
  const requests = row.requests_total
  const tokens = row.total_tokens
  const cost = row.total_cost_usd
  return {
    model: row.label,
    requests,
    tokens,
    cost,
    inputTokens: row.input_tokens,
    outputTokens: row.output_tokens,
    cacheCreationTokens: row.cache_creation_input_tokens,
    cacheReadTokens: row.cache_read_input_tokens,
    cacheHitRate: row.cache_hit_rate,
    avg_response_time: row.avg_response_time_ms / 1000,
    avg_first_byte_time: row.avg_first_byte_time_ms / 1000,
  }
}

type ReportGranularity = ResolvedAnalyticsGranularity

function resolveReportGranularity(range: DateRangeParams): ReportGranularity {
  return resolveAnalyticsAutoGranularity(range)
}

function areStringArraysEqual(left: string[], right: string[]): boolean {
  if (left.length !== right.length) return false
  return left.every((value, index) => value === right[index])
}

export interface UseReportsDataOptions {
  scope?: AnalyticsScope
  defaultPreset?: PeriodValue
  timeRange?: Ref<DateRangeParams>
  userFilter?: Ref<string[]>
  providerFilter?: Ref<string[]>
  apiKeyFilter?: Ref<string[]>
  loadApiKeyOptions?: boolean
}

export function useReportsData(options: UseReportsDataOptions = {}) {
  const {
    scope = { kind: 'me' as const },
    defaultPreset = 'today',
    timeRange: externalTimeRange,
    userFilter: externalUserFilter,
    providerFilter: externalProviderFilter,
    apiKeyFilter: externalApiKeyFilter,
    loadApiKeyOptions = scope.kind === 'me',
  } = options

  const timeRange = externalTimeRange ?? ref<DateRangeParams>({
    ...getDateRangeFromPeriod(defaultPreset),
    granularity: 'auto',
  })
  const userFilter = externalUserFilter ?? ref<string[]>([])
  const providerFilter = externalProviderFilter ?? ref<string[]>([])
  const apiKeyFilter = externalApiKeyFilter ?? ref<string[]>([])
  const apiKeyOptions = ref<AnalyticsFilterOption[]>([])

  const summary = ref<AnalyticsSummary | null>(null)
  const bucketStats = ref<ReportBucketStat[]>([])
  const bucketBreakdowns = ref<Record<string, DailyUsageBreakdown>>({})
  const modelSummary = ref<ModelSummary[]>([])
  const loading = ref(false)
  const loadError = ref(false)
  const hasLoaded = ref(false)
  const guard = createRequestGuard()
  const apiKeysGuard = createRequestGuard()
  let suppressNextApiKeyFilterLoad = false
  const resolvedGranularity = computed(() => resolveReportGranularity(timeRange.value))
  const timeseriesTimeRange = computed(() => ({
    ...buildTimeRangeParams(timeRange.value),
    granularity: resolvedGranularity.value,
  }))

  watch(
    () => [timeRange.value.preset, timeRange.value.start_date, timeRange.value.end_date],
    () => {
      if (timeRange.value.granularity === 'hour' && getAnalyticsRangeDaysInclusive(timeRange.value) !== 1) {
        timeRange.value = {
          ...timeRange.value,
          granularity: 'auto',
        }
      }
    },
  )

  async function load() {
    const requestId = guard.next()
    loading.value = true
    loadError.value = false
    try {
      const params = buildTimeRangeParams(timeRange.value)
      const timeseriesParams = timeseriesTimeRange.value
      const basePayload = {
        scope,
        time_range: params,
        filters: {
          user_ids: userFilter.value,
          provider_names: providerFilter.value,
          api_key_ids: apiKeyFilter.value,
        },
      }

      const [overviewData, timeseriesData, modelBreakdownData] = await Promise.all([
        analyticsApi.getOverview(basePayload),
        analyticsApi.getTimeseries({
          ...basePayload,
          time_range: timeseriesParams,
        }),
        analyticsApi.getBreakdown({
          ...basePayload,
          dimension: 'model',
          limit: 50,
        }),
      ])

      if (guard.isStale(requestId)) return

      const filledBuckets = fillMissingTimeseriesBuckets(timeseriesData.buckets, timeseriesParams)

      summary.value = overviewData.summary
      bucketStats.value = filledBuckets.map(mapBucketToReportStat)
      bucketBreakdowns.value = Object.fromEntries(
        filledBuckets.map(bucket => {
          const breakdown = mapBucketToBreakdown(bucket)
          return [breakdown.date, breakdown]
        }),
      )
      modelSummary.value = modelBreakdownData.rows.map(mapBreakdownRowToModelSummary)
      hasLoaded.value = true
    } catch {
      if (guard.isStale(requestId)) return
      loadError.value = true
    } finally {
      if (guard.isCurrent(requestId)) {
        loading.value = false
      }
    }
  }

  async function loadApiKeys() {
    if (!loadApiKeyOptions) return
    const requestId = apiKeysGuard.next()
    const filterOptions = await analyticsApi.getFilterOptions({
      scope,
      time_range: buildTimeRangeParams(timeRange.value),
      filters: {},
    }).catch(() => null)
    if (apiKeysGuard.isStale(requestId)) return

    const nextOptions = filterOptions?.api_keys ?? []

    apiKeyOptions.value = nextOptions
    const validOptionValues = new Set(nextOptions.map(option => option.value))
    const nextFilter = apiKeyFilter.value.filter(value => validOptionValues.has(value))
    if (!areStringArraysEqual(nextFilter, apiKeyFilter.value)) {
      suppressNextApiKeyFilterLoad = true
      apiKeyFilter.value = nextFilter
    }
  }

  const loader = createLoader(load)
  const apiKeysLoader = createLoader(loadApiKeys)

  watch(
    timeRange,
    () => {
      void (async () => {
        if (loadApiKeyOptions) {
          await apiKeysLoader.execute()
        }
        loader.schedule()
      })()
    },
    { deep: true },
  )

  watch(
    [userFilter, providerFilter, apiKeyFilter],
    () => {
      if (suppressNextApiKeyFilterLoad) {
        suppressNextApiKeyFilterLoad = false
        return
      }
      loader.schedule()
    },
    { deep: true },
  )

  onMounted(() => {
    void (async () => {
      if (loadApiKeyOptions) {
        await apiKeysLoader.execute()
      }
      await loader.execute()
    })()
  })
  onUnmounted(() => {
    loader.cleanup()
    apiKeysLoader.cleanup()
    guard.invalidate()
    apiKeysGuard.invalidate()
  })

  return {
    timeRange,
    apiKeyFilter,
    apiKeyOptions,
    summary,
    bucketStats,
    bucketBreakdowns,
    modelSummary,
    loading,
    loadError,
    hasLoaded,
    resolvedGranularity,
  }
}
