<template>
  <div class="grid w-full grid-cols-2 gap-2 sm:grid-cols-3">
    <div
      v-for="metric in metrics"
      :key="metric.label"
      class="rounded-lg border border-border/40 bg-muted/20 px-3 py-2"
    >
      <div class="text-[11px] leading-tight text-muted-foreground">
        {{ metric.label }}
      </div>
      <div
        class="mt-1 text-sm font-semibold tabular-nums"
        :class="metric.valueClass"
      >
        {{ metric.value }}
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import {
  formatAvailability,
  formatMs,
  formatTps,
  getAvailabilityClass
} from './health-monitor-utils'

const props = defineProps<{
  avgLatencyMs?: number | null
  avgFirstByteMs?: number | null
  avgTps?: number | null
  totalAttempts: number
  slaEligibleCount?: number | null
  serviceErrorCount?: number | null
  userErrorCount?: number | null
  successRate: number
}>()

const availabilityItem = computed(() => ({
  total_attempts: props.totalAttempts,
  sla_eligible_count: props.slaEligibleCount,
  service_error_count: props.serviceErrorCount,
  user_error_count: props.userErrorCount,
  success_rate: props.successRate
}))

const metrics = computed(() => [
  {
    label: 'SLA 可用率',
    value: formatAvailability(availabilityItem.value),
    valueClass: getAvailabilityClass(availabilityItem.value)
  },
  {
    label: '服务错误',
    value: formatCount(props.serviceErrorCount),
    valueClass: (props.serviceErrorCount ?? 0) > 0 ? 'text-red-600 dark:text-red-400' : ''
  },
  {
    label: '用户错误',
    value: formatCount(props.userErrorCount),
    valueClass: 'text-muted-foreground'
  },
  {
    label: '平均耗时',
    value: formatMs(props.avgLatencyMs),
    valueClass: ''
  },
  {
    label: '平均TTFB',
    value: formatMs(props.avgFirstByteMs),
    valueClass: ''
  },
  {
    label: '平均速度',
    value: formatTps(props.avgTps),
    valueClass: ''
  }
])

function formatCount(value?: number | null): string {
  return typeof value === 'number' && Number.isFinite(value)
    ? value.toLocaleString('zh-CN')
    : '-'
}
</script>
