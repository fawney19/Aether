<template>
  <Card class="p-4">
    <div class="flex items-center justify-between mb-3">
      <p class="text-sm font-semibold">
        {{ title }}
      </p>
      <div
        v-if="displayLegendItems.length > 0"
        class="flex items-center gap-2 flex-wrap justify-end text-[11px]"
      >
        <div
          v-for="item in displayLegendItems"
          :key="item.id"
          class="flex items-center gap-1"
        >
          <div
            class="w-2.5 h-2.5 rounded-full"
            :style="{ backgroundColor: item.color }"
          />
          <span class="text-muted-foreground">{{ item.name }}</span>
        </div>
        <span
          v-if="hiddenLegendCount > 0"
          class="text-muted-foreground"
        >
          +{{ hiddenLegendCount }} 更多
        </span>
      </div>
    </div>
    <div
      v-if="loading"
      class="h-[160px] flex items-center justify-center"
    >
      <div class="text-sm text-muted-foreground">
        Loading...
      </div>
    </div>
    <div
      v-else-if="hasData"
      class="h-[160px]"
    >
      <ScatterChart
        :data="chartData"
        :options="chartOptions"
      />
    </div>
    <div
      v-else
      class="h-[160px] flex items-center justify-center text-sm text-muted-foreground"
    >
      暂无请求间隔数据
    </div>
  </Card>
</template>

<script setup lang="ts">
import type { TimeScatterChartData } from '@/components/charts/types'
import { computed, ref, onMounted, onBeforeUnmount, watch } from 'vue'
import Card from '@/components/ui/card.vue'
import ScatterChart from '@/components/charts/ScatterChart.vue'
import { cacheAnalysisApi, type IntervalTimelineResponse } from '@/api/cache'
import { meApi } from '@/api/me'
import type { ChartOptions } from 'chart.js'
import { log } from '@/utils/logger'

const props = withDefaults(defineProps<{
  title: string
  isAdmin: boolean
  hours?: number
  refreshIntervalMs?: number
}>(), {
  hours: 24,  // 默认当天
  refreshIntervalMs: 30000
})

const loading = ref(false)
const timelineData = ref<IntervalTimelineResponse | null>(null)
const primaryColor = ref('201, 100, 66')  // 默认主题色
let loadRequestId = 0
let refreshTimer: ReturnType<typeof setTimeout> | null = null
const isPageVisible = ref(typeof document === 'undefined' ? true : !document.hidden)

const ADMIN_TIMELINE_LIMIT = 1500
const USER_TIMELINE_LIMIT = 1200

// 获取主题色
function getPrimaryColor(): string {
  if (typeof window === 'undefined') return '201, 100, 66'
  // CSS 变量定义在 body 上，不是 documentElement
  const body = document.body
  const style = getComputedStyle(body)
  const rgb = style.getPropertyValue('--color-primary-rgb').trim()
  return rgb || '201, 100, 66'
}

onMounted(() => {
  primaryColor.value = getPrimaryColor()
  if (typeof document !== 'undefined') {
    document.addEventListener('visibilitychange', handleVisibilityChange)
  }
  void loadData()
  scheduleNextRefresh()
})

// 预定义的颜色列表（用于区分不同用户/模型）
const COLORS = [
  'rgba(59, 130, 246, 0.7)',   // blue
  'rgba(236, 72, 153, 0.7)',   // pink
  'rgba(34, 197, 94, 0.7)',    // green
  'rgba(249, 115, 22, 0.7)',   // orange
  'rgba(168, 85, 247, 0.7)',   // purple
  'rgba(234, 179, 8, 0.7)',    // yellow
  'rgba(14, 165, 233, 0.7)',   // sky
  'rgba(239, 68, 68, 0.7)',    // red
  'rgba(20, 184, 166, 0.7)',   // teal
  'rgba(99, 102, 241, 0.7)',   // indigo
]

const hasData = computed(() =>
  timelineData.value && timelineData.value.points && timelineData.value.points.length > 0
)

// 判断是否有多个分组（管理员按用户分组，普通用户按模型分组）
const hasMultipleGroups = computed(() => {
  if (props.isAdmin) {
    // 管理员视图：按用户分组
    return timelineData.value?.users && Object.keys(timelineData.value.users).length > 1
  } else {
    // 用户视图：按模型分组
    return timelineData.value?.models && timelineData.value.models.length > 1
  }
})

// 图例最多显示数量
const MAX_LEGEND_ITEMS = 6

// 图例项（管理员显示用户，普通用户显示模型）
const legendItems = computed(() => {
  if (props.isAdmin && timelineData.value?.users) {
    // 管理员视图：显示用户图例
    const users = Object.entries(timelineData.value.users)
    if (users.length <= 1) return []
    return users.map(([userId, username], index) => ({
      id: userId,
      name: username || userId.slice(0, 8),
      color: COLORS[index % COLORS.length]
    }))
  } else if (timelineData.value?.models && timelineData.value.models.length > 1) {
    // 用户视图：显示模型图例
    return timelineData.value.models.map((model, index) => ({
      id: model,
      name: formatModelName(model),
      color: COLORS[index % COLORS.length]
    }))
  }
  return []
})

// 显示的图例项（限制数量）
const displayLegendItems = computed(() => {
  return legendItems.value.slice(0, MAX_LEGEND_ITEMS)
})

// 隐藏的图例数量
const hiddenLegendCount = computed(() => {
  return Math.max(0, legendItems.value.length - MAX_LEGEND_ITEMS)
})

// 格式化模型名称，使其更简洁
function formatModelName(model: string): string {
  // 常见的简化规则
  if (model.includes('claude')) {
    // claude-3-5-sonnet-20241022 -> Claude 3.5 Sonnet
    const match = model.match(/claude-(\d+)-(\d+)?-?(\w+)?/i)
    if (match) {
      const major = match[1]
      const minor = match[2]
      const variant = match[3]
      let name = `Claude ${major}`
      if (minor) name += `.${minor}`
      if (variant) name += ` ${variant.charAt(0).toUpperCase() + variant.slice(1)}`
      return name
    }
  }
  // 其他模型保持原样但截断
  return model.length > 20 ? `${model.slice(0, 17)  }...` : model
}

// 构建图表数据
const chartData = computed<TimeScatterChartData>(() => {
  if (!timelineData.value?.points) {
    return { datasets: [] }
  }

  const points = timelineData.value.points

  // 管理员视图且有多个用户：按用户分组
  if (props.isAdmin && timelineData.value.users && Object.keys(timelineData.value.users).length > 1) {
    const userIds = Object.keys(timelineData.value.users)
    const userColorMap: Record<string, string> = {}
    userIds.forEach((userId, index) => {
      userColorMap[userId] = COLORS[index % COLORS.length]
    })

    // 按用户分组数据
    const groupedData: Record<string, Array<{ x: string; y: number }>> = {}
    for (const point of points) {
      const userId = point.user_id || 'unknown'
      if (!groupedData[userId]) {
        groupedData[userId] = []
      }
      groupedData[userId].push({ x: point.x, y: point.y })
    }

    // 创建每个用户的 dataset
    const datasets = Object.entries(groupedData).map(([userId, data]) => ({
      label: timelineData.value?.users?.[userId] || userId.slice(0, 8),
      data,
      backgroundColor: userColorMap[userId] || 'rgba(59, 130, 246, 0.6)',
      borderColor: userColorMap[userId] || 'rgba(59, 130, 246, 0.8)',
      pointRadius: 3,
      pointHoverRadius: 5,
    }))

    return { datasets }
  }

  // 用户视图且有多个模型：按模型分组
  if (!props.isAdmin && timelineData.value.models && timelineData.value.models.length > 1) {
    const models = timelineData.value.models
    const modelColorMap: Record<string, string> = {}
    models.forEach((model, index) => {
      modelColorMap[model] = COLORS[index % COLORS.length]
    })

    // 按模型分组数据
    const groupedData: Record<string, Array<{ x: string; y: number }>> = {}
    for (const point of points) {
      const model = point.model || 'unknown'
      if (!groupedData[model]) {
        groupedData[model] = []
      }
      groupedData[model].push({ x: point.x, y: point.y })
    }

    // 创建每个模型的 dataset
    const datasets = Object.entries(groupedData).map(([model, data]) => ({
      label: formatModelName(model),
      data,
      backgroundColor: modelColorMap[model] || 'rgba(59, 130, 246, 0.6)',
      borderColor: modelColorMap[model] || 'rgba(59, 130, 246, 0.8)',
      pointRadius: 3,
      pointHoverRadius: 5,
    }))

    return { datasets }
  }

  // 单用户或单模型：使用主题色
  return {
    datasets: [{
      label: '请求间隔',
      data: points.map(p => ({ x: p.x, y: p.y })),
      backgroundColor: `rgba(${primaryColor.value}, 0.6)`,
      borderColor: `rgba(${primaryColor.value}, 0.8)`,
      pointRadius: 3,
      pointHoverRadius: 5,
    }]
  }
})

const chartOptions = computed<ChartOptions<'scatter'>>(() => ({
  plugins: {
    legend: {
      display: false  // 使用自定义图例
    },
    tooltip: {
      callbacks: {
        label: (context) => {
          const point = context.raw as { x: string; y: number; _originalY?: number }
          const realY = point._originalY ?? point.y
          const datasetLabel = context.dataset.label || ''
          if (hasMultipleGroups.value) {
            return `${datasetLabel}: ${realY.toFixed(1)} 分钟`
          }
          return `间隔: ${realY.toFixed(1)} 分钟`
        }
      }
    }
  }
}))

async function loadData() {
  const requestId = ++loadRequestId
  loading.value = true
  try {
    const limit = props.isAdmin ? ADMIN_TIMELINE_LIMIT : USER_TIMELINE_LIMIT
    if (props.isAdmin) {
      // 管理员：获取所有用户数据（按比例采样）
      const data = await cacheAnalysisApi.getIntervalTimeline({
        hours: props.hours,
        include_user_info: true,
        limit,
      })
      if (requestId !== loadRequestId) return
      timelineData.value = data
    } else {
      // 普通用户：获取自己的数据
      const data = await meApi.getIntervalTimeline({
        hours: props.hours,
        limit,
      })
      if (requestId !== loadRequestId) return
      timelineData.value = data
    }
  } catch (error) {
    if (requestId !== loadRequestId) return
    log.error('加载请求间隔时间线失败:', error)
    timelineData.value = null
  } finally {
    if (requestId === loadRequestId) {
      loading.value = false
    }
  }
}

function stopRefreshTimer() {
  if (refreshTimer) {
    clearTimeout(refreshTimer)
    refreshTimer = null
  }
}

function scheduleNextRefresh() {
  if (refreshTimer) return
  if (!isPageVisible.value) return
  if (!props.refreshIntervalMs || props.refreshIntervalMs <= 0) return
  refreshTimer = setTimeout(async () => {
    refreshTimer = null
    await loadData()
    scheduleNextRefresh()
  }, props.refreshIntervalMs)
}

function handleVisibilityChange() {
  isPageVisible.value = !document.hidden
  if (!isPageVisible.value) {
    stopRefreshTimer()
    return
  }
  if (!props.refreshIntervalMs || props.refreshIntervalMs <= 0) {
    return
  }
  void loadData()
  scheduleNextRefresh()
}

watch([() => props.hours, () => props.isAdmin], () => {
  void loadData()
  stopRefreshTimer()
  scheduleNextRefresh()
})

onBeforeUnmount(() => {
  loadRequestId++
  if (typeof document !== 'undefined') {
    document.removeEventListener('visibilitychange', handleVisibilityChange)
  }
  stopRefreshTimer()
})
</script>
