<template>
  <Card
    variant="default"
    class="overflow-hidden"
  >
    <HealthMonitorHeader
      v-model:lookback-hours="lookbackHours"
      :title="title"
      description="仅展示活跃提供商，点击详情查看该提供商关联的端点与模型健康"
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
        v-else-if="visibleProviders.length === 0"
        class="flex flex-col items-center justify-center py-12 text-muted-foreground"
      >
        <Server class="w-12 h-12 mb-3 opacity-30" />
        <p>暂无提供商健康数据</p>
        <p class="text-xs mt-1">
          当前没有提供商产生请求记录
        </p>
      </div>

      <div
        v-else
        class="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-4"
      >
        <div
          v-for="provider in visibleProviders"
          :key="provider.provider_id"
          class="relative overflow-hidden rounded-xl border border-border/60 bg-card/60 p-4 transition-colors hover:border-primary/50"
        >
          <div class="absolute inset-x-0 top-0 h-px bg-gradient-to-r from-transparent via-primary/40 to-transparent" />
          <div class="flex items-start justify-between gap-3">
            <div class="flex min-w-0 items-center gap-3">
              <div class="flex h-11 w-11 flex-shrink-0 items-center justify-center rounded-xl border border-border/60 bg-muted/50">
                <Server class="h-5 w-5 text-muted-foreground" />
              </div>
              <div class="min-w-0">
                <h4 class="truncate text-sm font-semibold">
                  {{ provider.provider_name }}
                </h4>
              </div>
            </div>
            <Badge
              :variant="getHealthBadgeVariant(provider)"
              class="shrink-0"
            >
              {{ getHealthLabel(provider) }}
            </Badge>
          </div>

          <HealthMetricGrid
            class="mt-4"
            :avg-latency-ms="provider.avg_latency_ms"
            :avg-first-byte-ms="provider.avg_first_byte_ms"
            :avg-tps="provider.avg_tps"
            :total-attempts="provider.total_attempts"
            :success-rate="provider.success_rate"
          />

          <div class="mt-4 flex items-center justify-between gap-3 text-[11px] uppercase tracking-wide text-muted-foreground">
            <span>History (60pts)</span>
            <span class="truncate normal-case tracking-normal">
              {{ getProviderMetaText(provider) }}
            </span>
          </div>

          <HealthStatusTimeline
            class="mt-2"
            :timeline="provider.timeline"
            :timeline-details="provider.timeline_details"
            :time-range-start="provider.time_range_start"
            :time-range-end="provider.time_range_end"
            :generated-at="generatedAt"
            :lookback-hours="parseInt(lookbackHours)"
            entity-label="提供商"
            :entity-name="provider.provider_name"
          />

          <div class="mt-4 flex justify-end">
            <Button
              variant="outline"
              size="sm"
              @click="openDetails(provider)"
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
import { Loader2, Server } from 'lucide-vue-next'
import Card from '@/components/ui/card.vue'
import Badge from '@/components/ui/badge.vue'
import Button from '@/components/ui/button.vue'
import HealthMetricGrid from './HealthMetricGrid.vue'
import HealthMonitorHeader from './HealthMonitorHeader.vue'
import HealthStatusTimeline from './HealthStatusTimeline.vue'
import { getProviderStatusMonitor } from '@/api/endpoints/health'
import type { ProviderStatusMonitor } from '@/api/endpoints/types'
import type { HealthMonitorDetailTarget, HealthMonitorSectionSummary } from './health-monitor-utils'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'
import {
  formatCompactNumber,
  getHealthBadgeVariant,
  getHealthLabel,
  summarizeHealthMonitorItems
} from './health-monitor-utils'

withDefaults(defineProps<{
  title?: string
}>(), {
  title: '提供商健康监控'
})

const emit = defineEmits<{
  viewDetails: [target: HealthMonitorDetailTarget]
  summaryUpdated: [summary: HealthMonitorSectionSummary]
}>()

const { error: showError } = useToast()

const loading = ref(false)
const loadingMonitors = ref(false)
const providers = ref<ProviderStatusMonitor[]>([])
const generatedAt = ref<string | null>(null)
const lookbackHours = ref('6')
const visibleProviders = computed(() => providers.value.filter(provider => provider.total_attempts > 0))

async function loadMonitors() {
  loadingMonitors.value = true
  try {
    const data = await getProviderStatusMonitor({
      lookback_hours: parseInt(lookbackHours.value),
      provider_limit: 50,
      per_provider_model_limit: 12,
      per_model_limit: 100
    })
    providers.value = data.providers || []
    generatedAt.value = data.generated_at || null
    emitSummary()
  } catch (err: unknown) {
    showError(parseApiError(err, '加载提供商健康监控数据失败'), '错误')
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

function getProviderMetaText(provider: ProviderStatusMonitor) {
  const attempts = `${formatCompactNumber(provider.total_attempts)} 次请求`
  return `${provider.model_count} 个模型 / ${attempts}`
}

function openDetails(provider: ProviderStatusMonitor) {
  emit('viewDetails', {
    lookbackHours: parseInt(lookbackHours.value),
    source: {
      kind: 'provider',
      value: provider.provider_name,
      title: provider.provider_name,
      metaText: null,
      totalAttempts: provider.total_attempts,
      successCount: provider.success_count,
      failedCount: provider.failed_count,
      successRate: provider.success_rate,
      avgLatencyMs: provider.avg_latency_ms,
      avgFirstByteMs: provider.avg_first_byte_ms,
      avgTps: provider.avg_tps,
      timeline: provider.timeline || null,
      timelineDetails: provider.timeline_details || null,
      timeRangeStart: provider.time_range_start || null,
      timeRangeEnd: provider.time_range_end || null
    }
  })
}

function emitSummary() {
  emit('summaryUpdated', summarizeHealthMonitorItems(visibleProviders.value))
}

watch(lookbackHours, () => {
  loadMonitors()
})

onMounted(() => {
  refreshData()
})
</script>
