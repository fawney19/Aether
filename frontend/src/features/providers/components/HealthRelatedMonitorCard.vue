<template>
  <div class="relative overflow-hidden rounded-xl border border-border/60 bg-card/60 p-4 transition-colors hover:border-primary/50">
    <div class="absolute inset-x-0 top-0 h-px bg-gradient-to-r from-transparent via-primary/40 to-transparent" />

    <div class="flex items-start justify-between gap-3">
      <div class="flex min-w-0 items-center gap-3">
        <div class="flex h-11 w-11 flex-shrink-0 items-center justify-center rounded-xl border border-border/60 bg-muted/50">
          <component
            :is="iconComponent"
            class="h-5 w-5 text-muted-foreground"
          />
        </div>
        <div class="min-w-0">
          <h4 class="truncate text-sm font-semibold">
            {{ monitor.display_name || monitor.key }}
          </h4>
          <p
            v-if="monitor.meta_text"
            class="mt-1 truncate text-xs text-muted-foreground"
          >
            {{ monitor.meta_text }}
          </p>
        </div>
      </div>
      <Badge
        :variant="getHealthBadgeVariant(monitor)"
        class="shrink-0"
      >
        {{ getHealthLabel(monitor) }}
      </Badge>
    </div>

    <HealthMetricGrid
      class="mt-4"
      :avg-latency-ms="monitor.avg_latency_ms"
      :avg-first-byte-ms="monitor.avg_first_byte_ms"
      :avg-tps="monitor.avg_tps"
      :total-attempts="monitor.total_attempts"
      :sla-eligible-count="monitor.sla_eligible_count"
      :service-error-count="monitor.service_error_count ?? monitor.failed_count"
      :user-error-count="monitor.user_error_count"
      :success-rate="monitor.success_rate"
    />

    <div class="mt-4 flex items-center justify-between gap-3 text-[11px] uppercase tracking-wide text-muted-foreground">
      <span>History (60pts)</span>
      <span class="truncate normal-case tracking-normal">
        {{ metaText }}
      </span>
    </div>

    <HealthStatusTimeline
      class="mt-2"
      :timeline="monitor.timeline"
      :timeline-details="monitor.timeline_details"
      :time-range-start="monitor.time_range_start"
      :time-range-end="monitor.time_range_end"
      :generated-at="generatedAt"
      :lookback-hours="lookbackHours"
      :entity-label="entityLabel"
      :entity-name="monitor.key"
    />

    <div
      v-if="showDetailButton"
      class="mt-4 flex justify-end"
    >
      <Button
        variant="outline"
        size="sm"
        @click="$emit('viewDetails', monitor)"
      >
        查看详情
      </Button>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { Activity, Bot, Server } from 'lucide-vue-next'
import Badge from '@/components/ui/badge.vue'
import Button from '@/components/ui/button.vue'
import type { HealthRelatedMonitor } from '@/api/endpoints/types'
import HealthMetricGrid from './HealthMetricGrid.vue'
import HealthStatusTimeline from './HealthStatusTimeline.vue'
import {
  formatCompactNumber,
  getHealthBadgeVariant,
  getHealthLabel
} from './health-monitor-utils'

const props = withDefaults(defineProps<{
  monitor: HealthRelatedMonitor
  lookbackHours: number
  generatedAt?: string | null
  showDetailButton?: boolean
}>(), {
  generatedAt: null,
  showDetailButton: true
})

defineEmits<{
  viewDetails: [monitor: HealthRelatedMonitor]
}>()

const iconComponent = computed(() => {
  switch (props.monitor.kind) {
    case 'model':
      return Bot
    case 'provider':
      return Server
    default:
      return Activity
  }
})

const entityLabel = computed(() => {
  switch (props.monitor.kind) {
    case 'model':
      return '模型'
    case 'provider':
      return '提供商'
    default:
      return '端点'
  }
})

const metaText = computed(() => {
  const attempts = `${formatCompactNumber(props.monitor.total_attempts)} 次请求`
  if (props.monitor.meta_text?.includes('次请求')) return props.monitor.meta_text
  return props.monitor.meta_text
    ? `${props.monitor.meta_text} / ${attempts}`
    : attempts
})
</script>
