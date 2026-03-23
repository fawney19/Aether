import { ref, watch, onMounted, onUnmounted } from 'vue'
import { analyticsApi, type AnalyticsErrorCategory, type AnalyticsErrorTrendItem, type AnalyticsPercentilePoint, type AnalyticsPerformanceResponse, type AnalyticsProviderHealthItem } from '@/api/analytics'
import { auditApi, type MonitoringSystemStatus } from '@/api/audit'
import { getHealthSummary, type HealthSummary } from '@/api/endpoints'
import { createLoader, createRequestGuard, useInjectedAnalyticsFilters } from '@/composables/useAnalyticsFilters'

const EMPTY_LATENCY: AnalyticsPerformanceResponse['latency'] = {
  response_time_ms: { avg: 0, p50: null, p90: null, p99: null },
  first_byte_time_ms: { avg: 0, p50: null, p90: null, p99: null },
}

export function usePerformanceData() {
  const filters = useInjectedAnalyticsFilters()

  const latency = ref<AnalyticsPerformanceResponse['latency']>(EMPTY_LATENCY)
  const percentiles = ref<AnalyticsPercentilePoint[]>([])
  const errorDistribution = ref<AnalyticsErrorCategory[]>([])
  const errorTrend = ref<AnalyticsErrorTrendItem[]>([])
  const errorTotal = ref(0)
  const errorRate = ref(0)
  const providerStatus = ref<AnalyticsProviderHealthItem[]>([])
  const healthSummary = ref<HealthSummary | null>(null)
  const systemStatus = ref<MonitoringSystemStatus | null>(null)
  const loading = ref(false)
  const loadError = ref(false)
  const hasLoaded = ref(false)
  const guard = createRequestGuard()

  async function load() {
    const requestId = guard.next()
    loading.value = true
    loadError.value = false
    try {
      const [response, summaryResponse, systemResponse] = await Promise.all([
        analyticsApi.getPerformance({
          scope: { kind: 'global' },
          time_range: filters.getTimeRangeParams(),
          filters: {
            statuses: ['completed', 'failed', 'cancelled'],
          },
        }),
        getHealthSummary().catch(() => null),
        auditApi.getSystemStatus().catch(() => null),
      ])

      if (guard.isStale(requestId)) return
      latency.value = response.latency
      percentiles.value = response.percentiles
      errorTotal.value = response.errors.total
      errorRate.value = response.errors.rate
      errorDistribution.value = response.errors.categories
      errorTrend.value = response.errors.trend
      providerStatus.value = response.provider_health
      if (summaryResponse) {
        healthSummary.value = summaryResponse
      }
      if (systemResponse) {
        systemStatus.value = systemResponse
      }
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

  const loader = createLoader(load)

  watch(
    () => [
      filters.getTimeRangeParams(),
    ],
    () => loader.schedule(),
    { deep: true },
  )

  onMounted(() => { void loader.execute() })
  onUnmounted(() => { loader.cleanup(); guard.invalidate() })

  return {
    latency,
    percentiles,
    errorDistribution,
    errorTrend,
    errorTotal,
    errorRate,
    providerStatus,
    healthSummary,
    systemStatus,
    loading,
    loadError,
    hasLoaded,
  }
}
