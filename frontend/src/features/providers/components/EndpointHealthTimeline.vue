<template>
  <div class="w-full space-y-1">
    <!-- 时间线 -->
    <div class="flex items-center gap-px h-6 w-full">
      <TooltipProvider
        v-for="(segment, index) in segments"
        :key="index"
        :delay-duration="100"
      >
        <Tooltip>
          <TooltipTrigger as-child>
            <div
              class="flex-1 h-full rounded-sm transition-all duration-150 cursor-pointer hover:scale-y-110 hover:brightness-110"
              :class="segment.color"
            />
          </TooltipTrigger>
          <TooltipContent
            side="top"
            :side-offset="8"
            class="max-w-xs"
          >
            <div class="text-xs whitespace-pre-line">
              {{ segment.tooltip }}
            </div>
          </TooltipContent>
        </Tooltip>
      </TooltipProvider>
    </div>
    <!-- 时间标签 -->
    <div class="flex items-center justify-between text-[10px] text-muted-foreground">
      <span>{{ earliestTime }}</span>
      <span>{{ latestTime }}</span>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import type { EndpointStatusMonitor, EndpointHealthEvent, PublicEndpointStatusMonitor, PublicHealthEvent } from '@/api/endpoints'
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip'
import type { HealthTimelineMonitorLike } from '@/features/providers/utils/healthMonitorUtils'

// 组件同时支持管理员端和用户端的监控数据类型
// - EndpointStatusMonitor: 管理员端，包含 provider_count, key_count 等敏感信息
// - PublicEndpointStatusMonitor: 用户端，不含敏感信息
const props = defineProps<{
  monitor?: EndpointStatusMonitor | PublicEndpointStatusMonitor | HealthTimelineMonitorLike | null
  segmentCount?: number
  lookbackHours?: number
}>()

// 固定格子数量，将实际事件按时间均匀分布到格子中
const GRID_COUNT = 100

const segments = computed(() => {
  const gridCount = props.segmentCount ?? GRID_COUNT
  const lookbackHours = props.lookbackHours ?? 6
  const usageTimeline = Array.isArray(props.monitor?.timeline)
    ? props.monitor?.timeline ?? []
    : []

  if (usageTimeline.length > 0) {
    return buildUsageTimelineSegments(
      usageTimeline,
      props.monitor?.time_range_start ?? null,
      props.monitor?.time_range_end ?? null,
      lookbackHours
    )
  }

  const events = props.monitor?.events ?? []

  // 无数据时显示空白格子
  if (events.length === 0) {
    return Array.from({ length: gridCount }, () => ({
      color: 'bg-gray-300 dark:bg-gray-600',
      tooltip: '暂无请求记录'
    }))
  }

  // 计算时间范围：使用 UTC 时间戳避免时区问题
  const nowUtc = Date.now()
  const startTimeUtc = nowUtc - lookbackHours * 60 * 60 * 1000
  const timeRange = lookbackHours * 60 * 60 * 1000
  const timePerGrid = timeRange / gridCount

  const gridEvents: Array<Array<EndpointHealthEvent | PublicHealthEvent>> = Array.from({ length: gridCount }, () => [])

  for (const event of events) {
    const eventTime = new Date(event.timestamp).getTime()
    const gridIndex = Math.floor((eventTime - startTimeUtc) / timePerGrid)
    if (gridIndex >= 0 && gridIndex < gridCount) {
      gridEvents[gridIndex].push(event)
    }
  }

  const result: Array<{ color: string; tooltip: string }> = []

  for (let i = 0; i < gridCount; i++) {
    const cellEvents = gridEvents[i]
    const cellStartTime = new Date(startTimeUtc + i * timePerGrid)
    const cellEndTime = new Date(startTimeUtc + (i + 1) * timePerGrid)

    if (cellEvents.length === 0) {
      result.push({
        color: 'bg-gray-300 dark:bg-gray-600',
        tooltip: `${formatTimestamp(cellStartTime.toISOString())} - ${formatTimestamp(cellEndTime.toISOString())}\n暂无请求记录`
      })
      continue
    }

    if (cellEvents.length === 1) {
      result.push({
        color: getStatusColor(cellEvents[0].status),
        tooltip: buildTooltip(cellEvents[0])
      })
      continue
    }

    const successCount = cellEvents.filter(e => e.status === 'success').length
    const failedCount = cellEvents.filter(e => e.status === 'failed').length
    const skippedCount = cellEvents.filter(e => e.status === 'skipped').length
    const total = cellEvents.length

    let color: string
    if (failedCount > 0) {
      const failRate = failedCount / total
      color = failRate > 0.5 ? 'bg-red-500' : 'bg-red-400/80'
    } else if (successCount > 0) {
      const successRate = successCount / total
      color = successRate > 0.7 ? 'bg-green-500/80' : 'bg-green-400/80'
    } else if (skippedCount > 0) {
      color = 'bg-amber-400/80'
    } else {
      color = 'bg-gray-300 dark:bg-gray-600'
    }

    const firstTime = formatTimestamp(cellEvents[0]?.timestamp)
    const lastTime = formatTimestamp(cellEvents[cellEvents.length - 1]?.timestamp)
    const tooltip = `${firstTime} - ${lastTime}\n共 ${total} 次请求\n成功: ${successCount}, 失败: ${failedCount}, 跳过: ${skippedCount}`

    result.push({ color, tooltip })
  }

  return result
})

function getStatusColor(status: string) {
  switch (status) {
    case 'success':
      return 'bg-green-500/80 dark:bg-green-400/90'
    case 'failed':
      return 'bg-red-500/80 dark:bg-red-400/90'
    case 'skipped':
      return 'bg-amber-400/80 dark:bg-amber-300/80'
    case 'started':
      return 'bg-blue-400/80 dark:bg-blue-300/80'
    default:
      return 'bg-muted/50 dark:bg-muted/20'
  }
}

function buildTooltip(event: EndpointHealthEvent | PublicHealthEvent) {
  const time = formatTimestamp(event.timestamp)
  const statusText = getStatusText(event.status)
  const latency = event.latency_ms ? ` • ${event.latency_ms}ms` : ''
  const code = event.status_code ? ` • ${event.status_code}` : ''
  const error = event.error_type ? ` • ${event.error_type}` : ''
  return `${time} ${statusText}${latency}${code}${error}`
}

function getStatusText(status: string) {
  switch (status) {
    case 'success':
      return '成功'
    case 'failed':
      return '失败'
    case 'skipped':
      return '跳过'
    case 'started':
      return '执行中'
    default:
      return '未知'
  }
}

function formatTimestamp(timestamp?: string | null) {
  if (!timestamp) return '未知时间'
  const date = new Date(timestamp)
  return date.toLocaleString('zh-CN', {
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit'
  })
}

// 计算时间范围显示
const earliestTime = computed(() => {
  const explicitStart =
    (props.monitor as (EndpointStatusMonitor | PublicEndpointStatusMonitor | HealthTimelineMonitorLike | null))?.time_range_start
  if (explicitStart) return formatTimestamp(explicitStart)
  const lookbackHours = props.lookbackHours ?? 6
  const startTime = new Date(Date.now() - lookbackHours * 60 * 60 * 1000)
  return formatTimestamp(startTime.toISOString())
})

const latestTime = computed(() => {
  const explicitEnd =
    (props.monitor as (EndpointStatusMonitor | PublicEndpointStatusMonitor | HealthTimelineMonitorLike | null))?.time_range_end
  if (explicitEnd) return formatTimestamp(explicitEnd)
  return formatTimestamp(new Date().toISOString())
})

function buildUsageTimelineSegments(
  statuses: string[],
  timeRangeStart: string | null,
  timeRangeEnd: string | null,
  lookbackHours: number
) {
  const gridCount = statuses.length
  const endTime = timeRangeEnd ? new Date(timeRangeEnd).getTime() : Date.now()
  const startTime = timeRangeStart
    ? new Date(timeRangeStart).getTime()
    : endTime - lookbackHours * 60 * 60 * 1000
  const safeRange = Math.max(endTime - startTime, 1)
  const interval = safeRange / gridCount

  return statuses.map((status, index) => {
    const cellStart = new Date(startTime + index * interval)
    const cellEnd = new Date(startTime + (index + 1) * interval)
    return {
      color: getHealthTimelineColor(status),
      tooltip: `${formatTimestamp(cellStart.toISOString())} - ${formatTimestamp(
        cellEnd.toISOString()
      )}\n状态：${getHealthTimelineLabel(status)}`
    }
  })
}

function getHealthTimelineColor(status: string) {
  switch (status) {
    case 'healthy':
      return 'bg-green-500/80 dark:bg-green-400/90'
    case 'warning':
      return 'bg-amber-400/80 dark:bg-amber-300/80'
    case 'unhealthy':
      return 'bg-red-500/80 dark:bg-red-400/90'
    default:
      return 'bg-gray-300 dark:bg-gray-600'
  }
}

function getHealthTimelineLabel(status: string) {
  switch (status) {
    case 'healthy':
      return '健康'
    case 'warning':
      return '警告'
    case 'unhealthy':
      return '异常'
    default:
      return '未知'
  }
}
</script>
