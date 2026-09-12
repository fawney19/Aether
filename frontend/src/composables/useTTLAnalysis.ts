import type { TimeScatterChartData } from '@/components/charts/types'
/**
 * TTL 分析 composable
 * 封装缓存亲和性 TTL 分析相关的状态和逻辑
 */
import { ref, computed, watch } from 'vue'
import { useToast } from '@/composables/useToast'
import {
  cacheAnalysisApi,
  type TTLAnalysisResponse,
  type CacheHitAnalysisResponse,
  type IntervalTimelineResponse
} from '@/api/cache'
import { log } from '@/utils/logger'

// 时间范围选项
export const ANALYSIS_HOURS_OPTIONS = [
  { value: '12', label: '12 小时' },
  { value: '24', label: '24 小时' },
  { value: '72', label: '3 天' },
  { value: '168', label: '7 天' },
  { value: '336', label: '14 天' },
  { value: '720', label: '30 天' }
] as const

// 间隔颜色配置
export const INTERVAL_COLORS = {
  short: 'rgba(34, 197, 94, 0.6)',    // green: 0-5 分钟
  medium: 'rgba(59, 130, 246, 0.6)',  // blue: 5-15 分钟
  normal: 'rgba(168, 85, 247, 0.6)',  // purple: 15-30 分钟
  long: 'rgba(249, 115, 22, 0.6)',    // orange: 30-60 分钟
  veryLong: 'rgba(239, 68, 68, 0.6)'  // red: >60 分钟
} as const

/**
 * 根据间隔时间获取对应的颜色
 */
export function getIntervalColor(interval: number): string {
  if (interval <= 5) return INTERVAL_COLORS.short
  if (interval <= 15) return INTERVAL_COLORS.medium
  if (interval <= 30) return INTERVAL_COLORS.normal
  if (interval <= 60) return INTERVAL_COLORS.long
  return INTERVAL_COLORS.veryLong
}

/**
 * 获取 TTL 推荐的 Badge 样式
 */
export function getTTLBadgeVariant(ttl: number): 'default' | 'secondary' | 'outline' | 'destructive' {
  if (ttl <= 5) return 'default'
  if (ttl <= 15) return 'secondary'
  if (ttl <= 30) return 'outline'
  return 'destructive'
}

/**
 * 获取使用频率标签
 */
export function getFrequencyLabel(ttl: number): string {
  if (ttl <= 5) return '高频'
  if (ttl <= 15) return '中高频'
  if (ttl <= 30) return '中频'
  return '低频'
}

/**
 * 获取使用频率样式类名
 */
export function getFrequencyClass(ttl: number): string {
  if (ttl <= 5) return 'text-success font-medium'
  if (ttl <= 15) return 'text-blue-500 font-medium'
  if (ttl <= 30) return 'text-muted-foreground'
  return 'text-destructive'
}

export function useTTLAnalysis() {
  const { error: showError, info: showInfo } = useToast()

  // 状态
  const ttlAnalysis = ref<TTLAnalysisResponse | null>(null)
  const hitAnalysis = ref<CacheHitAnalysisResponse | null>(null)
  const ttlAnalysisLoading = ref(false)
  const hitAnalysisLoading = ref(false)
  const analysisHours = ref('24')

  // 用户散点图展开状态
  const expandedUserId = ref<string | null>(null)
  const userTimelineData = ref<IntervalTimelineResponse | null>(null)
  const userTimelineLoading = ref(false)

  // 计算属性：是否正在加载
  const isLoading = computed(() => ttlAnalysisLoading.value || hitAnalysisLoading.value)

  // 获取 TTL 分析数据
  async function fetchTTLAnalysis() {
    ttlAnalysisLoading.value = true
    try {
      const hours = parseInt(analysisHours.value)
      const result = await cacheAnalysisApi.analyzeTTL({ hours })
      ttlAnalysis.value = result

      if (result.total_users_analyzed === 0) {
        const periodText = hours >= 24 ? `${hours / 24} 天` : `${hours} 小时`
        showInfo(`未找到符合条件的数据（最近 ${periodText}）`)
      }
    } catch (error) {
      showError('获取 TTL 分析失败')
      log.error('获取 TTL 分析失败', error)
    } finally {
      ttlAnalysisLoading.value = false
    }
  }

  // 获取缓存命中分析数据
  async function fetchHitAnalysis() {
    hitAnalysisLoading.value = true
    try {
      hitAnalysis.value = await cacheAnalysisApi.analyzeHit({
        hours: parseInt(analysisHours.value)
      })
    } catch (error) {
      showError('获取缓存命中分析失败')
      log.error('获取缓存命中分析失败', error)
    } finally {
      hitAnalysisLoading.value = false
    }
  }

  // 获取指定用户的时间线数据
  async function fetchUserTimeline(userId: string) {
    userTimelineLoading.value = true
    try {
      userTimelineData.value = await cacheAnalysisApi.getIntervalTimeline({
        hours: parseInt(analysisHours.value),
        limit: 2000,
        user_id: userId
      })
    } catch (error) {
      showError('获取用户时间线数据失败')
      log.error('获取用户时间线数据失败', error)
    } finally {
      userTimelineLoading.value = false
    }
  }

  // 切换用户行展开状态
  async function toggleUserExpand(userId: string) {
    if (expandedUserId.value === userId) {
      expandedUserId.value = null
      userTimelineData.value = null
    } else {
      expandedUserId.value = userId
      await fetchUserTimeline(userId)
    }
  }

  // 刷新所有分析数据
  async function refreshAnalysis() {
    expandedUserId.value = null
    userTimelineData.value = null
    await Promise.all([fetchTTLAnalysis(), fetchHitAnalysis()])
  }

  // 用户时间线散点图数据
  const userTimelineChartData = computed<TimeScatterChartData>(() => {
    if (!userTimelineData.value || userTimelineData.value.points.length === 0) {
      return { datasets: [] }
    }

    const points = userTimelineData.value.points

    return {
      datasets: [{
        label: '请求间隔',
        data: points.map(p => ({ x: p.x, y: p.y })),
        backgroundColor: points.map(p => getIntervalColor(p.y)),
        borderColor: points.map(p => getIntervalColor(p.y).replace('0.6', '1')),
        pointRadius: 3,
        pointHoverRadius: 5
      }]
    }
  })

  // 监听时间范围变化
  watch(analysisHours, () => {
    refreshAnalysis()
  })

  return {
    // 状态
    ttlAnalysis,
    hitAnalysis,
    ttlAnalysisLoading,
    hitAnalysisLoading,
    analysisHours,
    expandedUserId,
    userTimelineData,
    userTimelineLoading,
    isLoading,
    userTimelineChartData,

    // 方法
    fetchTTLAnalysis,
    fetchHitAnalysis,
    fetchUserTimeline,
    toggleUserExpand,
    refreshAnalysis
  }
}
