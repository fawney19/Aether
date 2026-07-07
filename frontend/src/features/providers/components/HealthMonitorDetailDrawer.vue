<template>
  <Dialog
    v-model:open="isOpen"
    :title="dialogTitle"
    :description="dialogDescription"
    max-width="6xl"
    no-padding
  >
    <template #header-actions>
      <Button
        variant="outline"
        size="sm"
        @click="isOpen = false"
      >
        关闭
      </Button>
    </template>

    <div class="max-h-[78vh] overflow-y-auto px-4 py-4 sm:px-6">
      <div
        v-if="sourceMonitor"
        class="space-y-6"
      >
        <section>
          <div class="mb-3 flex items-center justify-between gap-3">
            <div>
              <h4 class="text-sm font-semibold">
                当前健康
              </h4>
              <p class="text-xs text-muted-foreground">
                当前卡片在相同回溯窗口内的健康概况
              </p>
            </div>
          </div>
          <HealthRelatedMonitorCard
            :monitor="sourceMonitor"
            :lookback-hours="target?.lookbackHours || 6"
            :show-detail-button="false"
          />
        </section>

        <div
          v-if="loading"
          class="flex items-center justify-center rounded-xl border border-dashed border-border/60 py-12 text-muted-foreground"
        >
          <Loader2 class="h-5 w-5 animate-spin" />
          <span class="ml-2 text-sm">加载关联健康...</span>
        </div>

        <div
          v-else-if="errorMessage"
          class="rounded-xl border border-destructive/30 bg-destructive/5 p-4 text-sm text-destructive"
        >
          <div class="flex items-start gap-2">
            <AlertTriangle class="mt-0.5 h-4 w-4 flex-shrink-0" />
            <div class="min-w-0">
              <p class="font-medium">
                关联健康加载失败
              </p>
              <p class="mt-1 text-xs">
                {{ errorMessage }}
              </p>
            </div>
          </div>
          <Button
            class="mt-3"
            variant="outline"
            size="sm"
            @click="loadRelated"
          >
            重试
          </Button>
        </div>

        <template v-else>
          <section
            v-for="section in relatedSections"
            :key="section.key"
            class="space-y-3"
          >
            <div>
              <h4 class="text-sm font-semibold">
                {{ section.title }}
              </h4>
              <p class="text-xs text-muted-foreground">
                {{ section.description }}
              </p>
            </div>
            <div class="grid grid-cols-1 gap-4 md:grid-cols-2">
              <HealthRelatedMonitorCard
                v-for="monitor in section.items"
                :key="`${monitor.kind}-${monitor.key}`"
                :monitor="monitor"
                :lookback-hours="target?.lookbackHours || 6"
                :generated-at="related?.generated_at || null"
                @view-details="openRelatedDetails"
              />
            </div>
          </section>

          <div
            v-if="relatedSections.length === 0"
            class="rounded-xl border border-dashed border-border/60 py-10 text-center text-sm text-muted-foreground"
          >
            当前时间范围内暂无关联健康数据
          </div>
        </template>
      </div>
    </div>
  </Dialog>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { AlertTriangle, Loader2 } from 'lucide-vue-next'
import Dialog from '@/components/ui/dialog/Dialog.vue'
import Button from '@/components/ui/button.vue'
import {
  getHealthRelatedMonitor,
  getPublicHealthRelatedMonitor
} from '@/api/endpoints/health'
import type {
  HealthRelatedMonitor,
  HealthRelatedMonitorResponse
} from '@/api/endpoints/types'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'
import HealthRelatedMonitorCard from './HealthRelatedMonitorCard.vue'
import type {
  HealthMonitorDetailSource,
  HealthMonitorDetailTarget
} from './health-monitor-utils'

const props = defineProps<{
  open: boolean
  target: HealthMonitorDetailTarget | null
  isAdmin: boolean
}>()

const emit = defineEmits<{
  'update:open': [value: boolean]
  viewDetails: [target: HealthMonitorDetailTarget]
}>()

const { error: showError } = useToast()

const related = ref<HealthRelatedMonitorResponse | null>(null)
const loading = ref(false)
const errorMessage = ref<string | null>(null)
let requestSeq = 0

const isOpen = computed({
  get: () => props.open,
  set: value => emit('update:open', value)
})

const dialogTitle = computed(() => {
  if (!props.target) return '健康详情'
  return `${props.target.source.title} 详情`
})

const dialogDescription = computed(() => {
  if (!props.target) return '查看关联健康维度'
  const dimensions = props.isAdmin ? '关联端点、模型与提供商健康' : '关联端点与模型健康'
  return `${props.target.lookbackHours} 小时内的${dimensions}`
})

const sourceMonitor = computed<HealthRelatedMonitor | null>(() => {
  const source = props.target?.source
  if (!source) return null
  return {
    kind: source.kind,
    key: source.value,
    display_name: source.title,
    meta_text: source.metaText || null,
    total_attempts: source.totalAttempts,
    success_count: source.successCount,
    failed_count: source.failedCount,
    success_rate: source.successRate,
    avg_latency_ms: source.avgLatencyMs,
    avg_first_byte_ms: source.avgFirstByteMs,
    avg_tps: source.avgTps,
    timeline: source.timeline || undefined,
    timeline_details: source.timelineDetails || undefined,
    time_range_start: source.timeRangeStart || null,
    time_range_end: source.timeRangeEnd || null
  }
})

const relatedSections = computed(() => {
  const data = related.value
  const targetKind = props.target?.source.kind
  if (!data || !targetKind) return []

  const sectionMap = {
    endpoints: {
      key: 'endpoints',
      title: '关联端点健康',
      description: '当前维度下实际请求涉及的端点健康',
      items: data.related_endpoints || []
    },
    models: {
      key: 'models',
      title: '关联模型健康',
      description: '当前维度下实际请求涉及的模型健康',
      items: data.related_models || []
    },
    providers: {
      key: 'providers',
      title: '关联提供商健康',
      description: '当前维度下实际请求涉及的提供商健康',
      items: props.isAdmin ? (data.related_providers || []) : []
    }
  }

  const orderByKind = {
    endpoint: ['providers', 'models'],
    provider: ['endpoints', 'models'],
    model: ['providers', 'endpoints']
  } as const

  return orderByKind[targetKind]
    .map(key => sectionMap[key])
    .filter(section => section.items.length > 0)
})

async function loadRelated() {
  const target = props.target
  if (!props.open || !target) return

  const seq = ++requestSeq
  loading.value = true
  errorMessage.value = null
  try {
    const params = {
      dimension: target.source.kind,
      value: target.source.value,
      lookback_hours: target.lookbackHours,
      related_limit: 8,
      per_item_limit: 100
    }
    if (!props.isAdmin && target.source.kind === 'provider') {
      throw new Error('公开健康监控不支持 provider 详情')
    }
    const data = props.isAdmin
      ? await getHealthRelatedMonitor(params)
      : await getPublicHealthRelatedMonitor({
          ...params,
          dimension: target.source.kind
        })
    if (seq === requestSeq) {
      related.value = data
    }
  } catch (err: unknown) {
    if (seq === requestSeq) {
      const message = parseApiError(err, '加载关联健康失败')
      errorMessage.value = message
      showError(message, '错误')
    }
  } finally {
    if (seq === requestSeq) {
      loading.value = false
    }
  }
}

function openRelatedDetails(monitor: HealthRelatedMonitor) {
  if (!props.isAdmin && monitor.kind === 'provider') return
  emit('viewDetails', {
    lookbackHours: props.target?.lookbackHours || 6,
    source: buildSourceFromRelatedMonitor(monitor)
  })
}

function buildSourceFromRelatedMonitor(monitor: HealthRelatedMonitor): HealthMonitorDetailSource {
  return {
    kind: monitor.kind,
    value: monitor.key,
    title: monitor.display_name || monitor.key,
    metaText: buildRelatedMetaText(monitor),
    totalAttempts: monitor.total_attempts,
    successCount: monitor.success_count,
    failedCount: monitor.failed_count,
    successRate: monitor.success_rate,
    avgLatencyMs: monitor.avg_latency_ms,
    avgFirstByteMs: monitor.avg_first_byte_ms,
    avgTps: monitor.avg_tps,
    timeline: monitor.timeline || null,
    timelineDetails: monitor.timeline_details || null,
    timeRangeStart: monitor.time_range_start || null,
    timeRangeEnd: monitor.time_range_end || null
  }
}

function buildRelatedMetaText(monitor: HealthRelatedMonitor) {
  return monitor.meta_text || null
}

watch([
  () => props.open,
  () => props.target?.source.kind,
  () => props.target?.source.value,
  () => props.target?.lookbackHours,
  () => props.isAdmin
], ([open]) => {
  if (open) {
    loadRelated()
  }
}, { immediate: true })
</script>
