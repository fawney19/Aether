import { computed, onMounted, onUnmounted, ref, watch, type Ref } from 'vue'
import { analyticsApi, type AnalyticsFilterOption, type AnalyticsLeaderboardItem, type AnalyticsTimeseriesBucket } from '@/api/analytics'
import { buildTimeRangeParams, createLoader, createRequestGuard, useInjectedAnalyticsFilters } from '@/composables/useAnalyticsFilters'
import { fillMissingTimeseriesBuckets } from '@/utils/analyticsTimeseries'
import { isAnalyticsSingleDayRange, resolveAnalyticsAutoGranularity, type AnalyticsGranularityOption } from '@/utils/analyticsGranularity'
import { mapAnalyticsLeaderboardItem, type LeaderboardTableRow } from './mappers'

export type LeaderboardDimension = 'users' | 'api_keys'

export interface LeaderboardSummary {
  total_requests: number
  total_tokens: number
  total_cost: number
  total_actual_cost: number
  avg_response_time: number
}

function dimensionToEntity(dimension: LeaderboardDimension): 'user' | 'api_key' {
  return dimension === 'users' ? 'user' : 'api_key'
}

export function useLeaderboardData(dimension: Ref<LeaderboardDimension>) {
  const filters = useInjectedAnalyticsFilters()

  const leaderboardItems = ref<AnalyticsLeaderboardItem[]>([])
  const latestUserOptions = ref<AnalyticsFilterOption[]>([])
  const latestApiKeyOptions = ref<AnalyticsFilterOption[]>([])
  const leaderboardLoading = ref(false)
  const leaderboardError = ref(false)
  const leaderboardHasLoaded = ref(false)
  const selectedId = ref<string>('')

  const selectedSummary = ref<LeaderboardSummary | null>(null)
  const selectedTrendBuckets = ref<AnalyticsTimeseriesBucket[]>([])
  const panelLoading = ref(false)
  const panelError = ref(false)
  const panelHasLoaded = ref(false)

  const leaderboardGuard = createRequestGuard()
  const filterOptionsGuard = createRequestGuard()
  const panelGuard = createRequestGuard()

  const entityOptions = computed(() => (
    dimension.value === 'users' ? latestUserOptions.value : latestApiKeyOptions.value
  ))
  const entityLabelMap = computed(() => (
    new Map(entityOptions.value.map(option => [option.value, option.label]))
  ))

  const leaderboard = computed<LeaderboardTableRow[]>(() => (
    leaderboardItems.value.map(item => ({
      ...mapAnalyticsLeaderboardItem(item),
      name: entityLabelMap.value.get(item.id) ?? item.label,
    }))
  ))

  const selectedItem = computed(() => (
    leaderboard.value.find(item => item.id === selectedId.value) ?? null
  ))

  const selectedLabel = computed(() => (
    selectedItem.value?.name ?? entityLabelMap.value.get(selectedId.value) ?? null
  ))

  const resolvedTrendGranularity = computed(() => resolveAnalyticsAutoGranularity(filters.timeRange.value))
  const baseTimeRangePayload = computed(() => ({
    start_date: filters.timeRange.value.start_date,
    end_date: filters.timeRange.value.end_date,
    preset: filters.timeRange.value.preset,
    timezone: filters.timeRange.value.timezone,
    tz_offset_minutes: filters.timeRange.value.tz_offset_minutes,
  }))
  const trendTimeRangePayload = computed(() => ({
    ...baseTimeRangePayload.value,
    granularity: resolvedTrendGranularity.value,
  }))

  const currentTrendGranularitySelection = computed<AnalyticsGranularityOption>({
    get: () => filters.timeRange.value.granularity || 'auto',
    set: (value) => {
      if (value === 'hour' && !isAnalyticsSingleDayRange(filters.timeRange.value)) {
        value = 'auto'
      }
      filters.timeRange.value = {
        ...filters.timeRange.value,
        granularity: value,
      }
    },
  })

  function resetPanelData(invalidate = false) {
    if (invalidate) panelGuard.invalidate()
    panelLoading.value = false
    panelError.value = false
    selectedSummary.value = null
    selectedTrendBuckets.value = []
  }

  async function loadFilterOptions() {
    const requestId = filterOptionsGuard.next()
    const response = await analyticsApi.getFilterOptions({
      scope: { kind: 'global' },
      time_range: baseTimeRangePayload.value,
      filters: {},
    }).catch(() => null)

    if (filterOptionsGuard.isStale(requestId) || !response) return
    latestUserOptions.value = response.users ?? []
    latestApiKeyOptions.value = response.api_keys ?? []
  }

  async function loadLeaderboard() {
    const requestId = leaderboardGuard.next()
    leaderboardLoading.value = true
    leaderboardError.value = false
    try {
      const response = await analyticsApi.getLeaderboard({
        scope: { kind: 'global' },
        time_range: baseTimeRangePayload.value,
        filters: {
          statuses: ['completed', 'failed', 'cancelled'],
        },
        entity: dimensionToEntity(dimension.value),
        metric: 'total_cost_usd',
        limit: 20,
      })

      if (leaderboardGuard.isStale(requestId)) return
      leaderboardItems.value = response.items

      const nextSelectedId = response.items.some(item => item.id === selectedId.value)
        ? selectedId.value
        : (response.items[0]?.id ?? '')

      if (selectedId.value !== nextSelectedId) {
        selectedId.value = nextSelectedId
      }
      leaderboardHasLoaded.value = true
    } catch {
      if (leaderboardGuard.isStale(requestId)) return
      leaderboardError.value = true
    } finally {
      if (leaderboardGuard.isCurrent(requestId)) {
        leaderboardLoading.value = false
      }
    }
  }

  async function loadPanel() {
    if (!selectedId.value) {
      resetPanelData(true)
      return
    }

    const requestId = panelGuard.next()
    panelLoading.value = true
    panelError.value = false
    try {
      const scope = dimension.value === 'users'
        ? { kind: 'user' as const, user_id: selectedId.value }
        : { kind: 'api_key' as const, api_key_id: selectedId.value }

      const basePayload = {
        scope,
        time_range: baseTimeRangePayload.value,
        filters: {
          statuses: ['completed', 'failed', 'cancelled'],
        },
      }

      const [overview, timeseries] = await Promise.all([
        analyticsApi.getOverview(basePayload),
        analyticsApi.getTimeseries({
          ...basePayload,
          time_range: trendTimeRangePayload.value,
        }),
      ])

      if (panelGuard.isStale(requestId)) return
      selectedSummary.value = {
        total_requests: overview.summary.requests_total,
        total_tokens: overview.summary.total_tokens,
        total_cost: overview.summary.total_cost_usd,
        total_actual_cost: overview.summary.actual_total_cost_usd,
        avg_response_time: overview.summary.avg_response_time_ms / 1000,
      }
      selectedTrendBuckets.value = fillMissingTimeseriesBuckets(
        timeseries.buckets,
        trendTimeRangePayload.value,
      )
      panelHasLoaded.value = true
    } catch {
      if (panelGuard.isStale(requestId)) return
      panelError.value = true
    } finally {
      if (panelGuard.isCurrent(requestId)) {
        panelLoading.value = false
      }
    }
  }

  const filterOptionsLoader = createLoader(loadFilterOptions)
  const leaderboardLoader = createLoader(loadLeaderboard)
  const panelLoader = createLoader(loadPanel)

  watch(
    () => [filters.timeRange.value.preset, filters.timeRange.value.start_date, filters.timeRange.value.end_date],
    () => {
      if (filters.timeRange.value.granularity === 'hour' && !isAnalyticsSingleDayRange(filters.timeRange.value)) {
        filters.timeRange.value = {
          ...filters.timeRange.value,
          granularity: 'auto',
        }
      }
    },
  )

  watch(
    () => [baseTimeRangePayload.value, dimension.value],
    () => {
      filterOptionsLoader.schedule()
      leaderboardLoader.schedule()
    },
    { deep: true },
  )

  watch(
    () => [
      selectedId.value,
      dimension.value,
      baseTimeRangePayload.value,
      trendTimeRangePayload.value,
    ],
    () => panelLoader.schedule(),
    { deep: true },
  )

  onMounted(() => {
    void (async () => {
      await filterOptionsLoader.execute()
      await leaderboardLoader.execute()
      await panelLoader.execute()
    })()
  })

  onUnmounted(() => {
    filterOptionsLoader.cleanup()
    leaderboardLoader.cleanup()
    panelLoader.cleanup()
    filterOptionsGuard.invalidate()
    leaderboardGuard.invalidate()
    panelGuard.invalidate()
  })

  return {
    leaderboard,
    leaderboardLoading,
    leaderboardError,
    leaderboardHasLoaded,
    selectedId,
    selectedItem,
    selectedLabel,
    selectedSummary,
    selectedTrendBuckets,
    panelLoading,
    panelError,
    panelHasLoaded,
    resolvedTrendGranularity,
    currentTrendGranularitySelection,
    reloadLeaderboard: leaderboardLoader.schedule,
  }
}
