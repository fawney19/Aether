<template>
  <Card
    variant="default"
    class="overflow-hidden"
  >
    <HealthMonitorHeader
      v-model:lookback-hours="lookbackHours"
      :title="title"
      description="基于真实请求统计端点 SLA 可用率；用户错误单列且不影响服务健康"
      :loading="loading"
      @refresh="refreshData"
    />

    <div class="p-6">
      <div
        v-if="loadingMonitors"
        class="flex items-center justify-center py-12"
      >
        <Loader2 class="w-6 h-6 animate-spin text-muted-foreground" />
        <span class="ml-2 text-muted-foreground">加载中...</span>
      </div>

      <div
        v-else-if="visibleMonitors.length === 0"
        class="flex flex-col items-center justify-center py-12 text-muted-foreground"
      >
        <Activity class="w-12 h-12 mb-3 opacity-30" />
        <p>暂无健康监控数据</p>
        <p class="text-xs mt-1">
          端点尚未产生请求记录
        </p>
      </div>

      <div
        v-else
        class="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-4"
      >
        <div
          v-for="(monitor, index) in visibleMonitors"
          :key="`${monitor.api_format}-${index}`"
          class="relative overflow-hidden rounded-xl border border-border/60 bg-card/60 p-4 transition-colors hover:border-primary/50"
        >
          <div
            class="absolute inset-x-0 top-0 h-px bg-gradient-to-r from-transparent via-primary/40 to-transparent"
          />
          <div class="flex items-start justify-between gap-3">
            <div class="flex min-w-0 items-center gap-3">
              <div
                class="flex h-11 w-11 flex-shrink-0 items-center justify-center rounded-xl border border-border/60 bg-muted/50"
              >
                <Activity class="h-5 w-5 text-muted-foreground" />
              </div>
              <div class="min-w-0">
                <h4 class="truncate text-sm font-semibold">
                  {{ formatApiFormat(monitor.api_format) }}
                </h4>
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

          <div
            class="mt-4 flex items-center justify-between gap-3 text-[11px] uppercase tracking-wide text-muted-foreground"
          >
            <span>History (60pts)</span>
            <span class="truncate normal-case tracking-normal">
              {{ getEndpointMetaText(monitor) }}
            </span>
          </div>

          <div class="mt-2">
            <EndpointHealthTimeline
              :monitor="monitor"
              :lookback-hours="parseInt(lookbackHours)"
            />
          </div>

          <div class="mt-4 flex justify-end">
            <Button
              variant="outline"
              size="sm"
              @click="openDetails(monitor)"
            >
              查看详情
            </Button>
          </div>
        </div>
      </div>
    </div>
  </Card>
</template>

<script setup lang="ts">
import { computed, ref, onMounted, watch } from 'vue'
import { Activity, Loader2 } from 'lucide-vue-next'
import Card from '@/components/ui/card.vue'
import Badge from '@/components/ui/badge.vue'
import Button from '@/components/ui/button.vue'
import HealthMetricGrid from './HealthMetricGrid.vue'
import HealthMonitorHeader from './HealthMonitorHeader.vue'
import EndpointHealthTimeline from './EndpointHealthTimeline.vue'
import { getEndpointStatusMonitor, getPublicEndpointStatusMonitor } from '@/api/endpoints/health'
import type { EndpointStatusMonitor, PublicEndpointStatusMonitor } from '@/api/endpoints/types'
import type { HealthMonitorDetailTarget, HealthMonitorSectionSummary } from './health-monitor-utils'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'
import { formatApiFormat } from '@/api/endpoints/types/api-format'
import {
  formatCompactNumber,
  getHealthBadgeVariant,
  getHealthLabel,
  summarizeHealthMonitorItems
} from './health-monitor-utils'

type EndpointMonitor = EndpointStatusMonitor | PublicEndpointStatusMonitor

const props = withDefaults(defineProps<{
  title?: string
  isAdmin?: boolean
  showProviderInfo?: boolean
}>(), {
  title: '健康监控',
  isAdmin: false,
  showProviderInfo: false
})

const emit = defineEmits<{
  viewDetails: [target: HealthMonitorDetailTarget]
  summaryUpdated: [summary: HealthMonitorSectionSummary]
}>()

const { error: showError } = useToast()

const loading = ref(false)
const loadingMonitors = ref(false)
const monitors = ref<EndpointMonitor[]>([])
const lookbackHours = ref('6')
const visibleMonitors = computed(() => monitors.value.filter(monitor => monitor.total_attempts > 0))

async function loadMonitors() {
  loadingMonitors.value = true
  try {
    const params = {
      lookback_hours: parseInt(lookbackHours.value),
      per_format_limit: 100
    }

    if (props.isAdmin) {
      const data = await getEndpointStatusMonitor(params)
      monitors.value = data.formats || []
    } else {
      const data = await getPublicEndpointStatusMonitor(params)
      monitors.value = data.formats || []
    }
    emitSummary()
  } catch (err: unknown) {
    showError(parseApiError(err, '加载健康监控数据失败'), '错误')
  } finally {
    loadingMonitors.value = false
  }
}

async function refreshData() {
  loading.value = true
  try {
    await loadMonitors()
  } finally {
    loading.value = false
  }
}

function getEndpointMetaText(monitor: EndpointMonitor) {
  const attempts = `${formatCompactNumber(monitor.total_attempts)} 次请求`
  if (props.showProviderInfo && hasProviderInfo(monitor)) {
    return `${monitor.provider_count} 个提供商 / ${monitor.key_count} 个密钥 / ${attempts}`
  }
  if (hasApiPath(monitor)) {
    return `${monitor.api_path} / ${attempts}`
  }
  return attempts
}

function openDetails(monitor: EndpointMonitor) {
  emit('viewDetails', {
    lookbackHours: parseInt(lookbackHours.value),
    source: {
      kind: 'endpoint',
      value: monitor.api_format,
      title: formatApiFormat(monitor.api_format),
      metaText: getEndpointDetailMetaText(monitor),
      totalAttempts: monitor.total_attempts,
      slaEligibleCount: monitor.sla_eligible_count,
      successCount: monitor.success_count,
      failedCount: monitor.failed_count,
      serviceErrorCount: monitor.service_error_count ?? monitor.failed_count,
      userErrorCount: monitor.user_error_count,
      successRate: monitor.success_rate,
      avgLatencyMs: monitor.avg_latency_ms,
      avgFirstByteMs: monitor.avg_first_byte_ms,
      avgTps: monitor.avg_tps,
      timeline: monitor.timeline || null,
      timelineDetails: monitor.timeline_details || null,
      timeRangeStart: monitor.time_range_start || null,
      timeRangeEnd: monitor.time_range_end || null
    }
  })
}

function getEndpointDetailMetaText(monitor: EndpointMonitor) {
  if (props.showProviderInfo && hasProviderInfo(monitor)) {
    return `${monitor.provider_count} 个提供商 / ${monitor.key_count} 个密钥`
  }
  if (hasApiPath(monitor)) {
    return monitor.api_path
  }
  return null
}

function emitSummary() {
  emit('summaryUpdated', summarizeHealthMonitorItems(visibleMonitors.value))
}

function hasProviderInfo(monitor: EndpointMonitor): monitor is EndpointStatusMonitor {
  return 'provider_count' in monitor && typeof monitor.provider_count === 'number'
}

function hasApiPath(monitor: EndpointMonitor): monitor is PublicEndpointStatusMonitor {
  return 'api_path' in monitor && typeof monitor.api_path === 'string' && monitor.api_path.length > 0
}

watch(lookbackHours, () => {
  loadMonitors()
})

onMounted(() => {
  refreshData()
})
</script>
