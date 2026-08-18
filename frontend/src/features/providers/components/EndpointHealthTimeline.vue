<template>
  <HealthStatusTimeline
    v-if="hasStatusTimeline"
    :timeline="monitor?.timeline"
    :timeline-details="monitor?.timeline_details"
    :time-range-start="monitor?.time_range_start"
    :time-range-end="monitor?.time_range_end"
    :lookback-hours="lookbackHours"
    :fallback-segments="segmentCount ?? GRID_COUNT"
    entity-label="端点"
    :entity-name="monitor?.api_format"
  />
  <div
    v-else
    class="w-full space-y-1"
  >
    <div class="flex items-center gap-px h-6 w-full">
      <TooltipProvider
        v-for="(segment, index) in segments"
        :key="index"
        :delay-duration="100"
      >
        <Tooltip>
          <TooltipTrigger as-child>
            <button
              type="button"
              :title="segment.tooltip"
              class="h-full flex-1 cursor-pointer rounded-sm border-0 p-0 transition-all duration-150 hover:scale-y-110 hover:brightness-110 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-primary"
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
import HealthStatusTimeline from './HealthStatusTimeline.vue'
import { formatTimestamp, formatTimelineTooltip } from './health-monitor-utils'

// 组件同时支持管理员端和用户端的监控数据类型
// - EndpointStatusMonitor: 管理员端，包含 provider_count, key_count 等敏感信息
// - PublicEndpointStatusMonitor: 用户端，不含敏感信息
const props = defineProps<{
  monitor?: EndpointStatusMonitor | PublicEndpointStatusMonitor | null
  segmentCount?: number
  lookbackHours?: number
}>()

// 固定格子数量，将实际事件按时间均匀分布到格子中
const GRID_COUNT = 60

const hasStatusTimeline = computed(() =>
  Array.isArray(props.monitor?.timeline) && (props.monitor?.timeline?.length ?? 0) > 0
)

const segments = computed(() => {
  const gridCount = props.segmentCount ?? GRID_COUNT
  const lookbackHours = props.lookbackHours ?? 6
  const events = props.monitor?.events ?? []
  const nowUtc = Date.now()
  const startTimeUtc = nowUtc - lookbackHours * 60 * 60 * 1000
  const timeRange = lookbackHours * 60 * 60 * 1000
  const timePerGrid = timeRange / gridCount

  // 无数据时显示空白格子
  if (events.length === 0) {
    return Array.from({ length: gridCount }, (_, index) => {
      const cellStartTime = new Date(startTimeUtc + index * timePerGrid)
      const cellEndTime = new Date(startTimeUtc + (index + 1) * timePerGrid)
      return {
        color: 'bg-gray-300 dark:bg-gray-600',
        tooltip: buildSegmentTooltip('unknown', cellStartTime, cellEndTime, [])
      }
    })
  }

  // 计算时间范围：使用 UTC 时间戳避免时区问题
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
        tooltip: buildSegmentTooltip('unknown', cellStartTime, cellEndTime, [])
      })
      continue
    }

    if (cellEvents.length === 1) {
      result.push({
        color: getStatusColor(cellEvents[0].status),
        tooltip: buildSegmentTooltip(
          getTimelineStatusFromEvents(cellEvents),
          cellStartTime,
          cellEndTime,
          cellEvents
        )
      })
      continue
    }

    const successCount = cellEvents.filter(e => e.status === 'success').length
    const failedCount = cellEvents.filter(e => e.status === 'failed').length
    const userErrorCount = cellEvents.filter(e => e.status === 'user_error').length
    const skippedCount = cellEvents.filter(e => e.status === 'skipped').length
    const slaEligibleCount = successCount + failedCount

    let color: string
    if (failedCount > 0) {
      const failRate = failedCount / Math.max(slaEligibleCount, 1)
      color = failRate > 0.5 ? 'bg-red-500' : 'bg-red-400/80'
    } else if (successCount > 0) {
      const successRate = successCount / Math.max(slaEligibleCount, 1)
      color = successRate > 0.7 ? 'bg-green-500/80' : 'bg-green-400/80'
    } else if (skippedCount > 0) {
      color = 'bg-amber-400/80'
    } else if (userErrorCount > 0) {
      color = 'bg-slate-400/80 dark:bg-slate-500/80'
    } else {
      color = 'bg-gray-300 dark:bg-gray-600'
    }

    result.push({
      color,
      tooltip: buildSegmentTooltip(
        getTimelineStatusFromEvents(cellEvents),
        cellStartTime,
        cellEndTime,
        cellEvents
      )
    })
  }

  return result
})

function getStatusColor(status: string) {
  switch (status) {
    case 'success':
      return 'bg-green-500/80 dark:bg-green-400/90'
    case 'failed':
      return 'bg-red-500/80 dark:bg-red-400/90'
    case 'user_error':
      return 'bg-slate-400/80 dark:bg-slate-500/80'
    case 'skipped':
      return 'bg-amber-400/80 dark:bg-amber-300/80'
    case 'started':
      return 'bg-blue-400/80 dark:bg-blue-300/80'
    default:
      return 'bg-muted/50 dark:bg-muted/20'
  }
}

// 计算时间范围显示
const earliestTime = computed(() => {
  const explicitStart =
    (props.monitor as (EndpointStatusMonitor | PublicEndpointStatusMonitor | null))?.time_range_start
  if (explicitStart) return formatTimestamp(explicitStart)
  const lookbackHours = props.lookbackHours ?? 6
  const startTime = new Date(Date.now() - lookbackHours * 60 * 60 * 1000)
  return formatTimestamp(startTime.toISOString())
})

const latestTime = computed(() => {
  const explicitEnd =
    (props.monitor as (EndpointStatusMonitor | PublicEndpointStatusMonitor | null))?.time_range_end
  if (explicitEnd) return formatTimestamp(explicitEnd)
  return formatTimestamp(new Date().toISOString())
})

function buildSegmentTooltip(
  status: string,
  cellStartTime: Date,
  cellEndTime: Date,
  cellEvents: Array<EndpointHealthEvent | PublicHealthEvent>
) {
  const successCount = cellEvents.filter(event => event.status === 'success').length
  const failedCount = cellEvents.filter(event => event.status === 'failed').length
  const userErrorCount = cellEvents.filter(event => event.status === 'user_error').length
  const completedCount = successCount + failedCount
  const latencyValues = cellEvents
    .map(event => event.latency_ms)
    .filter((value): value is number => typeof value === 'number' && !Number.isNaN(value))
  const avgLatencyMs = latencyValues.length > 0
    ? latencyValues.reduce((sum, value) => sum + value, 0) / latencyValues.length
    : null

  return formatTimelineTooltip({
    status,
    timeRangeStart: cellStartTime.toISOString(),
    timeRangeEnd: cellEndTime.toISOString(),
    metrics: {
      total_attempts: cellEvents.length,
      sla_eligible_count: completedCount,
      success_count: successCount,
      failed_count: failedCount,
      service_error_count: failedCount,
      user_error_count: userErrorCount,
      success_rate: completedCount > 0 ? successCount / completedCount : null,
      avg_latency_ms: avgLatencyMs,
      avg_first_byte_ms: null,
      avg_tps: null
    },
    entityLabel: '端点',
    entityName: props.monitor?.api_format
  })
}

function getTimelineStatusFromEvents(
  cellEvents: Array<EndpointHealthEvent | PublicHealthEvent>
) {
  const successCount = cellEvents.filter(event => event.status === 'success').length
  const failedCount = cellEvents.filter(event => event.status === 'failed').length
  const userErrorCount = cellEvents.filter(event => event.status === 'user_error').length
  const completedCount = successCount + failedCount
  if (completedCount === 0) return userErrorCount > 0 ? 'user_error' : 'unknown'
  const successRate = successCount / completedCount
  if (successRate >= 0.95) return 'healthy'
  if (successRate >= 0.7) return 'warning'
  return 'unhealthy'
}

</script>
