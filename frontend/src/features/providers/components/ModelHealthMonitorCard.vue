<template>
  <Card
    variant="default"
    class="overflow-hidden"
  >
    <div class="px-6 py-3.5 border-b border-border/60">
      <div class="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
        <div>
          <h3 class="text-base font-semibold">
            {{ title }}
          </h3>
          <p class="mt-1 text-xs text-muted-foreground">
            基于真实请求统计模型可用率、响应延迟与首包延迟
          </p>
        </div>
        <div class="flex items-center gap-3">
          <Label class="text-xs text-muted-foreground">回溯时间：</Label>
          <Select v-model="lookbackHours">
            <SelectTrigger class="w-28 h-8 text-xs border-border/60">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="1">
                1 小时
              </SelectItem>
              <SelectItem value="6">
                6 小时
              </SelectItem>
              <SelectItem value="12">
                12 小时
              </SelectItem>
              <SelectItem value="24">
                24 小时
              </SelectItem>
              <SelectItem value="48">
                48 小时
              </SelectItem>
            </SelectContent>
          </Select>
          <RefreshButton
            :loading="loading"
            @click="refreshData"
          />
        </div>
      </div>
    </div>

    <div class="p-6">
      <div
        v-if="loadingMonitors"
        class="flex items-center justify-center py-12"
      >
        <Loader2 class="w-6 h-6 animate-spin text-muted-foreground" />
        <span class="ml-2 text-muted-foreground">加载中...</span>
      </div>

      <div
        v-else-if="monitors.length === 0"
        class="flex flex-col items-center justify-center py-12 text-muted-foreground"
      >
        <Bot class="w-12 h-12 mb-3 opacity-30" />
        <p>暂无模型健康监控数据</p>
        <p class="text-xs mt-1">
          模型尚未产生请求记录
        </p>
      </div>

      <div
        v-else
        class="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-4"
      >
        <div
          v-for="monitor in monitors"
          :key="monitor.model"
          class="relative overflow-hidden rounded-xl border border-border/60 bg-card/60 p-4 transition-colors hover:border-primary/50"
        >
          <div class="absolute inset-x-0 top-0 h-px bg-gradient-to-r from-transparent via-primary/40 to-transparent" />
          <div class="flex items-start justify-between gap-3">
            <div class="flex min-w-0 items-center gap-3">
              <div class="flex h-11 w-11 flex-shrink-0 items-center justify-center rounded-xl border border-border/60 bg-muted/50">
                <Bot class="h-5 w-5 text-muted-foreground" />
              </div>
              <h4 class="min-w-0 truncate text-sm font-semibold">
                {{ monitor.display_name || monitor.model }}
              </h4>
            </div>
            <Badge
              :variant="getHealthBadgeVariant(monitor)"
              class="shrink-0"
            >
              {{ getHealthLabel(monitor) }}
            </Badge>
          </div>

          <div class="mt-4 grid grid-cols-3 gap-2">
            <div class="rounded-lg border border-border/40 bg-muted/20 px-3 py-2">
              <div class="flex items-center gap-1.5 text-[11px] text-muted-foreground">
                <Gauge class="h-3.5 w-3.5" />
                延迟
              </div>
              <div class="mt-1 text-sm font-semibold tabular-nums">
                {{ formatMs(monitor.avg_latency_ms) }}
              </div>
            </div>
            <div class="rounded-lg border border-border/40 bg-muted/20 px-3 py-2">
              <div class="flex items-center gap-1.5 text-[11px] text-muted-foreground">
                <Radio class="h-3.5 w-3.5" />
                Ping
              </div>
              <div class="mt-1 text-sm font-semibold tabular-nums">
                {{ formatMs(monitor.avg_first_byte_ms) }}
              </div>
            </div>
            <div class="rounded-lg border border-border/40 bg-muted/20 px-3 py-2">
              <div class="flex items-center gap-1.5 text-[11px] text-muted-foreground">
                <Activity class="h-3.5 w-3.5" />
                可用率
              </div>
              <div
                class="mt-1 text-sm font-semibold tabular-nums"
                :class="getSuccessRateClass(monitor.success_rate)"
              >
                {{ formatPercent(monitor.success_rate) }}
              </div>
            </div>
          </div>

          <div class="mt-4 flex items-center justify-between gap-3 text-[11px] uppercase tracking-wide text-muted-foreground">
            <span>History (60pts)</span>
            <span class="truncate normal-case tracking-normal">
              {{ getModelMetaText(monitor) }}
            </span>
          </div>

          <TooltipProvider :delay-duration="100">
            <div class="mt-2 flex h-7 w-full items-center gap-px">
              <Tooltip
                v-for="(segment, index) in timelineSegments(monitor)"
                :key="`${monitor.model}-${index}`"
              >
                <TooltipTrigger as-child>
                  <div
                    class="h-full flex-1 rounded-[2px] transition-all duration-150 hover:scale-y-110 hover:brightness-110"
                    :class="getTimelineColor(segment)"
                  />
                </TooltipTrigger>
                <TooltipContent
                  side="top"
                  :side-offset="8"
                  class="max-w-xs"
                >
                  <div class="text-xs whitespace-pre-line">
                    {{ buildTimelineTooltip(monitor, segment, index) }}
                  </div>
                </TooltipContent>
              </Tooltip>
            </div>
          </TooltipProvider>

          <div class="mt-2 flex items-center justify-between text-[10px] text-muted-foreground">
            <span>{{ formatTimestamp(monitor.time_range_start) }}</span>
            <span>{{ formatTimestamp(monitor.time_range_end || generatedAt) }}</span>
          </div>
        </div>
      </div>
    </div>
  </Card>
</template>

<script setup lang="ts">
import { ref, onMounted, watch } from 'vue'
import { Activity, Bot, Gauge, Loader2, Radio } from 'lucide-vue-next'
import Card from '@/components/ui/card.vue'
import Badge from '@/components/ui/badge.vue'
import Label from '@/components/ui/label.vue'
import Select from '@/components/ui/select.vue'
import SelectTrigger from '@/components/ui/select-trigger.vue'
import SelectValue from '@/components/ui/select-value.vue'
import SelectContent from '@/components/ui/select-content.vue'
import SelectItem from '@/components/ui/select-item.vue'
import RefreshButton from '@/components/ui/refresh-button.vue'
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip'
import { getModelStatusMonitor, getPublicModelStatusMonitor } from '@/api/endpoints/health'
import type { ModelStatusMonitor } from '@/api/endpoints/types'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'

const props = withDefaults(defineProps<{
  title?: string
  isAdmin?: boolean
  showProviderInfo?: boolean
}>(), {
  title: '模型健康监控',
  isAdmin: false,
  showProviderInfo: false
})

const { error: showError } = useToast()

const loading = ref(false)
const loadingMonitors = ref(false)
const monitors = ref<ModelStatusMonitor[]>([])
const generatedAt = ref<string | null>(null)
const lookbackHours = ref('6')

async function loadMonitors() {
  loadingMonitors.value = true
  try {
    const params = {
      lookback_hours: parseInt(lookbackHours.value),
      model_limit: 12,
      per_model_limit: 100
    }

    const data = props.isAdmin
      ? await getModelStatusMonitor(params)
      : await getPublicModelStatusMonitor(params)
    monitors.value = data.models || []
    generatedAt.value = data.generated_at || null
  } catch (err: unknown) {
    showError(parseApiError(err, '加载模型健康监控数据失败'), '错误')
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

function getHealthLabel(monitor: ModelStatusMonitor) {
  if (monitor.total_attempts <= 0) return '未知'
  if (monitor.success_rate >= 0.95) return '正常'
  if (monitor.success_rate >= 0.8) return '波动'
  return '异常'
}

function getHealthBadgeVariant(monitor: ModelStatusMonitor): 'default' | 'secondary' | 'destructive' | 'outline' | 'success' | 'warning' | 'dark' {
  if (monitor.total_attempts <= 0) return 'outline'
  if (monitor.success_rate >= 0.95) return 'success'
  if (monitor.success_rate >= 0.8) return 'warning'
  return 'destructive'
}

function getSuccessRateClass(rate: number) {
  if (rate >= 0.95) return 'text-green-600 dark:text-green-400'
  if (rate >= 0.8) return 'text-amber-600 dark:text-amber-400'
  return 'text-red-600 dark:text-red-400'
}

function getModelMetaText(monitor: ModelStatusMonitor) {
  const attempts = `${formatCompactNumber(monitor.total_attempts)} 次请求`
  if (props.showProviderInfo && typeof monitor.provider_count === 'number') {
    return `${monitor.provider_count} 个提供商 / ${attempts}`
  }
  return attempts
}

function formatMs(value?: number | null) {
  if (typeof value !== 'number' || Number.isNaN(value)) return '-'
  const absValue = Math.abs(value)
  if (absValue < 1000) return `${Math.round(value)} ms`
  if (absValue < 60_000) return `${formatDurationNumber(value / 1000)} s`
  return `${formatDurationNumber(value / 60_000)} min`
}

function formatDurationNumber(value: number) {
  return new Intl.NumberFormat('zh-CN', {
    maximumFractionDigits: Math.abs(value) < 10 ? 2 : 1
  }).format(value)
}

function formatPercent(value: number) {
  if (typeof value !== 'number' || Number.isNaN(value)) return '-'
  return `${(value * 100).toFixed(2)}%`
}

function formatCompactNumber(value: number) {
  return new Intl.NumberFormat('zh-CN', { notation: 'compact', maximumFractionDigits: 1 }).format(value)
}

function timelineSegments(monitor: ModelStatusMonitor) {
  const timeline = Array.isArray(monitor.timeline) ? monitor.timeline : []
  if (timeline.length > 0) return timeline
  return Array.from({ length: 60 }, () => 'unknown')
}

function getTimelineColor(status: string) {
  switch (status) {
    case 'healthy':
      return 'bg-green-500/85 dark:bg-green-400/90'
    case 'warning':
      return 'bg-amber-400/85 dark:bg-amber-300/85'
    case 'unhealthy':
      return 'bg-red-500/85 dark:bg-red-400/90'
    default:
      return 'bg-gray-300 dark:bg-gray-600'
  }
}

function getTimelineLabel(status: string) {
  switch (status) {
    case 'healthy':
      return '健康'
    case 'warning':
      return '波动'
    case 'unhealthy':
      return '异常'
    default:
      return '无请求'
  }
}

function buildTimelineTooltip(monitor: ModelStatusMonitor, status: string, index: number) {
  const segmentCount = timelineSegments(monitor).length
  const startMs = monitor.time_range_start ? new Date(monitor.time_range_start).getTime() : Date.now() - parseInt(lookbackHours.value) * 60 * 60 * 1000
  const endMs = monitor.time_range_end ? new Date(monitor.time_range_end).getTime() : Date.now()
  const interval = Math.max(endMs - startMs, 1) / segmentCount
  const cellStart = new Date(startMs + index * interval).toISOString()
  const cellEnd = new Date(startMs + (index + 1) * interval).toISOString()
  return `${formatTimestamp(cellStart)} - ${formatTimestamp(cellEnd)}\n模型：${monitor.model}\n状态：${getTimelineLabel(status)}`
}

function formatTimestamp(timestamp?: string | null) {
  if (!timestamp) return '未知时间'
  const date = new Date(timestamp)
  if (Number.isNaN(date.getTime())) return '未知时间'
  return date.toLocaleString('zh-CN', {
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit'
  })
}

watch(lookbackHours, () => {
  loadMonitors()
})

onMounted(() => {
  refreshData()
})
</script>
