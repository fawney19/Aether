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
            仅展示活跃提供商，展开后查看该提供商下的模型健康明细
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
        v-else-if="providers.length === 0"
        class="flex flex-col items-center justify-center py-12 text-muted-foreground"
      >
        <Server class="w-12 h-12 mb-3 opacity-30" />
        <p>暂无活跃提供商健康数据</p>
        <p class="text-xs mt-1">
          当前没有活跃提供商或尚未产生请求记录
        </p>
      </div>

      <div
        v-else
        class="space-y-3"
      >
        <Collapsible
          v-for="provider in providers"
          :key="provider.provider_id"
          v-model:open="expandedProviders[provider.provider_id]"
          class="overflow-hidden rounded-xl border border-border/60 bg-card/60"
        >
          <CollapsibleTrigger as-child>
            <button
              type="button"
              class="flex w-full flex-col gap-4 p-4 text-left transition-colors hover:bg-muted/30 lg:flex-row lg:items-center lg:justify-between"
            >
              <div class="flex min-w-0 items-start gap-3">
                <div class="flex h-11 w-11 flex-shrink-0 items-center justify-center rounded-xl border border-border/60 bg-muted/50">
                  <Server class="h-5 w-5 text-muted-foreground" />
                </div>
                <div class="min-w-0">
                  <div class="flex min-w-0 flex-wrap items-center gap-2">
                    <ChevronRight
                      class="h-4 w-4 text-muted-foreground transition-transform"
                      :class="{ 'rotate-90': expandedProviders[provider.provider_id] }"
                    />
                    <h4 class="truncate text-sm font-semibold">
                      {{ provider.provider_name }}
                    </h4>
                    <Badge
                      variant="outline"
                      class="font-mono text-[11px]"
                    >
                      {{ provider.provider_type || 'custom' }}
                    </Badge>
                    <Badge :variant="getHealthBadgeVariant(provider)">
                      {{ getHealthLabel(provider) }}
                    </Badge>
                  </div>
                  <p class="mt-1 text-xs text-muted-foreground">
                    {{ getProviderMetaText(provider) }}
                  </p>
                </div>
              </div>

              <div class="grid w-full grid-cols-3 gap-2 lg:max-w-xl">
                <MetricBox
                  label="延迟"
                  :value="formatMs(provider.avg_latency_ms)"
                />
                <MetricBox
                  label="Ping"
                  :value="formatMs(provider.avg_first_byte_ms)"
                />
                <MetricBox
                  label="可用率"
                  :value="formatPercent(provider.success_rate)"
                  :value-class="getSuccessRateClass(provider.success_rate)"
                />
              </div>
            </button>
          </CollapsibleTrigger>

          <CollapsibleContent class="border-t border-border/50 px-4 pb-4 pt-4">
            <div
              v-if="provider.models.length === 0"
              class="rounded-lg border border-dashed border-border/60 py-8 text-center text-sm text-muted-foreground"
            >
              该提供商在当前时间范围内暂无模型请求
            </div>
            <div
              v-else
              class="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-4"
            >
              <div
                v-for="model in provider.models"
                :key="`${provider.provider_id}-${model.model}`"
                class="relative overflow-hidden rounded-xl border border-border/60 bg-card/80 p-4 transition-colors hover:border-primary/50"
              >
                <div class="absolute inset-x-0 top-0 h-px bg-gradient-to-r from-transparent via-primary/40 to-transparent" />
                <div class="flex items-start justify-between gap-3">
                  <div class="flex min-w-0 items-center gap-3">
                    <div class="flex h-11 w-11 flex-shrink-0 items-center justify-center rounded-xl border border-border/60 bg-muted/50">
                      <Bot class="h-5 w-5 text-muted-foreground" />
                    </div>
                    <h4 class="min-w-0 truncate text-sm font-semibold">
                      {{ model.display_name || model.model }}
                    </h4>
                  </div>
                  <Badge
                    :variant="getHealthBadgeVariant(model)"
                    class="shrink-0"
                  >
                    {{ getHealthLabel(model) }}
                  </Badge>
                </div>

                <div class="mt-4 grid grid-cols-3 gap-2">
                  <MetricBox
                    label="延迟"
                    :value="formatMs(model.avg_latency_ms)"
                  />
                  <MetricBox
                    label="Ping"
                    :value="formatMs(model.avg_first_byte_ms)"
                  />
                  <MetricBox
                    label="可用率"
                    :value="formatPercent(model.success_rate)"
                    :value-class="getSuccessRateClass(model.success_rate)"
                  />
                </div>

                <div class="mt-4 flex items-center justify-between gap-3 text-[11px] uppercase tracking-wide text-muted-foreground">
                  <span>History (60pts)</span>
                  <span class="truncate normal-case tracking-normal">
                    {{ formatCompactNumber(model.total_attempts) }} 次请求
                  </span>
                </div>

                <TooltipProvider :delay-duration="100">
                  <div class="mt-2 flex h-7 w-full items-center gap-px">
                    <Tooltip
                      v-for="(segment, index) in timelineSegments(model)"
                      :key="`${provider.provider_id}-${model.model}-${index}`"
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
                          {{ buildTimelineTooltip(model, segment, index) }}
                        </div>
                      </TooltipContent>
                    </Tooltip>
                  </div>
                </TooltipProvider>

                <div class="mt-2 flex items-center justify-between text-[10px] text-muted-foreground">
                  <span>{{ formatTimestamp(model.time_range_start) }}</span>
                  <span>{{ formatTimestamp(model.time_range_end || generatedAt) }}</span>
                </div>
              </div>
            </div>
          </CollapsibleContent>
        </Collapsible>
      </div>
    </div>
  </Card>
</template>

<script setup lang="ts">
import { defineComponent, h, ref, onMounted, watch } from 'vue'
import { Bot, ChevronRight, Loader2, Server } from 'lucide-vue-next'
import Card from '@/components/ui/card.vue'
import Badge from '@/components/ui/badge.vue'
import Label from '@/components/ui/label.vue'
import Select from '@/components/ui/select.vue'
import SelectTrigger from '@/components/ui/select-trigger.vue'
import SelectValue from '@/components/ui/select-value.vue'
import SelectContent from '@/components/ui/select-content.vue'
import SelectItem from '@/components/ui/select-item.vue'
import RefreshButton from '@/components/ui/refresh-button.vue'
import Collapsible from '@/components/ui/collapsible.vue'
import CollapsibleTrigger from '@/components/ui/collapsible-trigger.vue'
import CollapsibleContent from '@/components/ui/collapsible-content.vue'
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip'
import { getProviderStatusMonitor } from '@/api/endpoints/health'
import type { ModelStatusMonitor, ProviderStatusMonitor } from '@/api/endpoints/types'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'

type HealthMonitorItem = Pick<ProviderStatusMonitor | ModelStatusMonitor, 'total_attempts' | 'success_rate'>

const MetricBox = defineComponent({
  props: {
    label: { type: String, required: true },
    value: { type: String, required: true },
    valueClass: { type: String, default: '' }
  },
  setup(props) {
    return () => h('div', { class: 'rounded-lg border border-border/40 bg-muted/20 px-3 py-2' }, [
      h('div', { class: 'text-[11px] text-muted-foreground' }, props.label),
      h('div', { class: ['mt-1 text-sm font-semibold tabular-nums', props.valueClass] }, props.value)
    ])
  }
})

const props = withDefaults(defineProps<{
  title?: string
}>(), {
  title: '提供商健康监控'
})

const { error: showError } = useToast()

const loading = ref(false)
const loadingMonitors = ref(false)
const providers = ref<ProviderStatusMonitor[]>([])
const generatedAt = ref<string | null>(null)
const lookbackHours = ref('6')
const expandedProviders = ref<Record<string, boolean>>({})

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
    ensureExpandedProviderState()
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

function ensureExpandedProviderState() {
  const next = { ...expandedProviders.value }
  for (const provider of providers.value) {
    if (!(provider.provider_id in next)) {
      next[provider.provider_id] = false
    }
  }
  expandedProviders.value = next
}

function getHealthLabel(item: HealthMonitorItem) {
  if (item.total_attempts <= 0) return '暂无请求'
  if (item.success_rate >= 0.95) return '正常'
  if (item.success_rate >= 0.8) return '波动'
  return '异常'
}

function getHealthBadgeVariant(item: HealthMonitorItem): 'default' | 'secondary' | 'destructive' | 'outline' | 'success' | 'warning' | 'dark' {
  if (item.total_attempts <= 0) return 'outline'
  if (item.success_rate >= 0.95) return 'success'
  if (item.success_rate >= 0.8) return 'warning'
  return 'destructive'
}

function getSuccessRateClass(rate: number) {
  if (rate >= 0.95) return 'text-green-600 dark:text-green-400'
  if (rate >= 0.8) return 'text-amber-600 dark:text-amber-400'
  return 'text-red-600 dark:text-red-400'
}

function getProviderMetaText(provider: ProviderStatusMonitor) {
  const attempts = `${formatCompactNumber(provider.total_attempts)} 次请求`
  return `${provider.model_count} 个模型 / ${attempts}`
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

function timelineSegments(item: ModelStatusMonitor | ProviderStatusMonitor) {
  const timeline = Array.isArray(item.timeline) ? item.timeline : []
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

function buildTimelineTooltip(model: ModelStatusMonitor, status: string, index: number) {
  const segmentCount = timelineSegments(model).length
  const startMs = model.time_range_start ? new Date(model.time_range_start).getTime() : Date.now() - parseInt(lookbackHours.value) * 60 * 60 * 1000
  const endMs = model.time_range_end ? new Date(model.time_range_end).getTime() : Date.now()
  const interval = Math.max(endMs - startMs, 1) / segmentCount
  const cellStart = new Date(startMs + index * interval).toISOString()
  const cellEnd = new Date(startMs + (index + 1) * interval).toISOString()
  return `${formatTimestamp(cellStart)} - ${formatTimestamp(cellEnd)}\n模型：${model.model}\n状态：${getTimelineLabel(status)}`
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
