<template>
  <div class="w-full space-y-1">
    <div class="flex h-6 w-full items-center gap-px">
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
              :class="getTimelineColor(segment.status)"
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
      <span>{{ startLabel }}</span>
      <span>{{ endLabel }}</span>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip'
import {
  formatTimestamp,
  formatTimelineTooltip,
  getTimelineColor,
  type HealthTimelineTooltipMetrics
} from './health-monitor-utils'

const props = withDefaults(defineProps<{
  timeline?: string[] | null
  timelineDetails?: HealthTimelineTooltipMetrics[] | null
  timeRangeStart?: string | null
  timeRangeEnd?: string | null
  generatedAt?: string | null
  lookbackHours?: number
  fallbackSegments?: number
  entityLabel?: string
  entityName?: string | null
}>(), {
  timeline: null,
  timelineDetails: null,
  timeRangeStart: null,
  timeRangeEnd: null,
  generatedAt: null,
  lookbackHours: 6,
  fallbackSegments: 60,
  entityLabel: '',
  entityName: null
})

const statuses = computed(() => {
  if (Array.isArray(props.timeline) && props.timeline.length > 0) {
    return props.timeline
  }
  return Array.from({ length: props.fallbackSegments }, () => 'unknown')
})

const startMs = computed(() => {
  const explicitStart = props.timeRangeStart
    ? new Date(props.timeRangeStart).getTime()
    : NaN
  if (!Number.isNaN(explicitStart)) return explicitStart
  return endMs.value - props.lookbackHours * 60 * 60 * 1000
})

const endMs = computed(() => {
  const explicitEnd = props.timeRangeEnd
    ? new Date(props.timeRangeEnd).getTime()
    : NaN
  if (!Number.isNaN(explicitEnd)) return explicitEnd
  const generatedAt = props.generatedAt
    ? new Date(props.generatedAt).getTime()
    : NaN
  if (!Number.isNaN(generatedAt)) return generatedAt
  return Date.now()
})

const startLabel = computed(() => formatTimestamp(new Date(startMs.value).toISOString()))
const endLabel = computed(() => formatTimestamp(new Date(endMs.value).toISOString()))

const segments = computed(() => {
  const segmentStatuses = statuses.value
  const safeRange = Math.max(endMs.value - startMs.value, 1)
  const interval = safeRange / segmentStatuses.length

  return segmentStatuses.map((status, index) => {
    const cellStart = new Date(startMs.value + index * interval).toISOString()
    const cellEnd = new Date(startMs.value + (index + 1) * interval).toISOString()
    const detail = props.timelineDetails?.[index] ?? null
    const timeRangeStart = detail?.time_range_start || cellStart
    const timeRangeEnd = detail?.time_range_end || cellEnd
    return {
      status,
      tooltip: buildTooltip(status, timeRangeStart, timeRangeEnd, detail)
    }
  })
})

function buildTooltip(
  status: string,
  cellStart: string,
  cellEnd: string,
  detail: HealthTimelineTooltipMetrics | null
) {
  return formatTimelineTooltip({
    status,
    timeRangeStart: cellStart,
    timeRangeEnd: cellEnd,
    metrics: detail,
    entityLabel: props.entityLabel,
    entityName: props.entityName
  })
}
</script>
