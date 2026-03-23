import { inject, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import type { DateRangeParams, PeriodValue } from '@/features/usage/types'
import { getDateRangeFromPeriod } from '@/features/usage/composables'

/**
 * 构建时间范围 API 参数（从各页面提取的公共逻辑）
 */
export function buildTimeRangeParams(timeRange: DateRangeParams) {
  return {
    start_date: timeRange.start_date,
    end_date: timeRange.end_date,
    preset: timeRange.preset,
    timezone: timeRange.timezone,
    tz_offset_minutes: timeRange.tz_offset_minutes,
    granularity: timeRange.granularity && timeRange.granularity !== 'auto'
      ? timeRange.granularity
      : 'day',
  }
}

/**
 * 通用防抖加载器工厂
 * 封装 120ms debounce + in-flight promise 去重 + requestId 过期检查
 */
export function createLoader(loadFn: () => Promise<void>) {
  let loadPromise: Promise<void> | null = null
  let hasPending = false
  let debounceTimer: ReturnType<typeof setTimeout> | null = null

  function schedule() {
    if (debounceTimer) clearTimeout(debounceTimer)
    debounceTimer = setTimeout(() => {
      debounceTimer = null
      void execute()
    }, 120)
  }

  async function execute() {
    if (loadPromise) {
      hasPending = true
      return loadPromise
    }
    loadPromise = loadFn()
      .finally(() => {
        loadPromise = null
        if (hasPending) {
          hasPending = false
          void execute()
        }
      })
    return loadPromise
  }

  function cleanup() {
    if (debounceTimer) {
      clearTimeout(debounceTimer)
      debounceTimer = null
    }
    hasPending = false
    loadPromise = null
  }

  return { schedule, execute, cleanup }
}

/**
 * 请求 ID 守卫：用于确保异步回调中的数据不被过期请求覆盖
 */
export function createRequestGuard() {
  let id = 0
  function next() { return ++id }
  function isStale(requestId: number) { return requestId !== id }
  function isCurrent(requestId: number) { return requestId === id }
  function invalidate() { id++ }
  return { next, isStale, isCurrent, invalidate }
}

export interface AnalyticsFiltersOptions {
  defaultPreset?: PeriodValue
  syncToUrl?: boolean
}

/**
 * 统一的分析页面筛选状态
 * - timeRange: 时间范围
 * - modelFilter / providerFilter / apiKeyFilter / userFilter: 多维度筛选
 * - activeTab: 当前 Tab
 * - URL query 双向同步
 */
export function useAnalyticsFilters(options: AnalyticsFiltersOptions = {}) {
  const { defaultPreset = 'last30days', syncToUrl = true } = options
  const route = useRoute()
  const router = useRouter()

  // 从 URL query 初始化，无则用默认值
  const initialPreset = (syncToUrl && route.query.preset as PeriodValue) || defaultPreset
  const initialTab = (syncToUrl && route.query.tab as string) || 'detail'

  const timeRange = ref<DateRangeParams>(getDateRangeFromPeriod(initialPreset))
  const activeTab = ref(initialTab)
  const modelFilter = ref<string[]>([])
  const providerFilter = ref<string[]>([])
  const apiKeyFilter = ref<string[]>([])
  const userFilter = ref<string[]>([])

  // URL 同步：仅同步 preset 和 tab（各类多选筛选太冗长不入 URL）
  if (syncToUrl) {
    watch([() => timeRange.value.preset, activeTab], ([preset, tab]) => {
      const query: Record<string, string> = {}
      if (preset && preset !== defaultPreset) query.preset = preset
      if (tab && tab !== 'detail') query.tab = tab
      // 保留其他 query 参数
      const currentQuery = { ...route.query }
      delete currentQuery.preset
      delete currentQuery.tab
      router.replace({ query: { ...currentQuery, ...query } })
    }, { flush: 'post' })
  }

  function getTimeRangeParams() {
    return buildTimeRangeParams(timeRange.value)
  }

  return {
    timeRange,
    activeTab,
    modelFilter,
    providerFilter,
    apiKeyFilter,
    userFilter,
    getTimeRangeParams,
  }
}

export type AnalyticsFilters = ReturnType<typeof useAnalyticsFilters>

export function useInjectedAnalyticsFilters(): AnalyticsFilters {
  const filters = inject<AnalyticsFilters>('analyticsFilters', null)
  if (!filters) {
    throw new Error('analyticsFilters provider is missing')
  }
  return filters
}
