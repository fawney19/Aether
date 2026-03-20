<template>
  <Card
    variant="default"
    class="overflow-hidden"
  >
    <!-- 标题和筛选器 -->
    <div class="px-6 py-3.5 border-b border-border/60">
      <div class="flex items-center justify-between gap-4">
        <h3 class="text-base font-semibold">
          {{ title }}
        </h3>
        <div class="flex items-center gap-3">
          <template v-if="showLookbackControl">
            <Label class="text-xs text-muted-foreground">回溯时间：</Label>
            <Select
              v-model="localLookbackHours"
            >
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
          </template>
          <RefreshButton
            :loading="loading"
            @click="refreshData"
          />
        </div>
      </div>
    </div>

    <!-- 内容区域 -->
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
        <Activity class="w-12 h-12 mb-3 opacity-30" />
        <p>暂无健康监控数据</p>
        <p class="text-xs mt-1">
          端点尚未产生请求记录
        </p>
      </div>

      <div
        v-else
        class="space-y-3"
      >
        <div class="grid gap-3 md:grid-cols-3">
          <div class="rounded-lg border border-border/60 bg-muted/20 p-4">
            <p class="text-xs text-muted-foreground">
              {{ props.isAdmin ? '活跃端点' : '监控格式' }}
            </p>
            <p class="mt-2 text-2xl font-semibold">
              {{ props.isAdmin ? endpointSummary.activeEndpoints : endpointSummary.activeFormats }}
            </p>
            <p class="mt-1 text-xs text-muted-foreground">
              {{ props.isAdmin ? `共 ${endpointSummary.totalEndpoints} 个端点` : `共 ${endpointSummary.activeFormats} 个格式` }}
            </p>
          </div>

          <div class="rounded-lg border border-border/60 bg-muted/20 p-4">
            <p class="text-xs text-muted-foreground">
              平均健康度
            </p>
            <p
              class="mt-2 text-2xl font-semibold"
              :class="getHealthScoreTextClass(endpointSummary.averageHealthScore)"
            >
              {{ formatHealthScore(endpointSummary.averageHealthScore) }}
            </p>
            <p class="mt-1 text-xs text-muted-foreground">
              基于当前回溯窗口内的时间线状态计算
            </p>
          </div>

          <div class="rounded-lg border border-border/60 bg-muted/20 p-4">
            <p class="text-xs text-muted-foreground">
              {{ props.isAdmin ? '异常端点' : '总请求数' }}
            </p>
            <p class="mt-2 text-2xl font-semibold">
              {{ props.isAdmin ? endpointSummary.unhealthyEndpoints : endpointSummary.totalAttempts }}
            </p>
            <p class="mt-1 text-xs text-muted-foreground">
              {{ props.isAdmin ? '健康分数偏低或已退化' : '当前窗口内统计到的请求次数' }}
            </p>
          </div>
        </div>

        <div
          v-for="monitor in monitors"
          :key="monitor.api_format"
          class="border border-border/60 rounded-lg p-4 hover:border-primary/50 transition-colors"
        >
          <!-- 响应式布局：窄屏上下两行，宽屏左右结构 -->
          <div class="flex flex-col sm:flex-row sm:gap-6 sm:items-center">
            <!-- 第一行/左侧：信息区域 -->
            <div class="sm:w-52 flex-shrink-0 space-y-1.5 mb-3 sm:mb-0">
              <!-- API 格式标签和成功率 -->
              <div class="flex items-center gap-2 flex-wrap">
                <Badge
                  variant="outline"
                  class="font-mono text-xs whitespace-nowrap"
                >
                  {{ formatApiFormat(monitor.api_format) }}
                </Badge>
                <Badge
                  v-if="monitor.total_attempts > 0"
                  :variant="getSuccessRateVariant(monitor.success_rate)"
                  class="text-xs whitespace-nowrap"
                >
                  {{ (monitor.success_rate * 100).toFixed(0) }}%
                </Badge>
                <!-- 提供商信息（仅管理员可见）- 窄屏时显示在同一行 -->
                <span
                  v-if="showProviderInfo && 'provider_count' in monitor"
                  class="text-xs text-muted-foreground sm:hidden"
                >
                  {{ monitor.provider_count }} 个提供商 / {{ monitor.key_count }} 个密钥
                </span>
              </div>

              <!-- 提供商信息（仅管理员可见）- 宽屏时显示在下方 -->
              <div
                v-if="showProviderInfo && 'provider_count' in monitor"
                class="text-xs text-muted-foreground hidden sm:block"
              >
                {{ monitor.provider_count }} 个提供商 / {{ monitor.key_count }} 个密钥
              </div>
            </div>

            <!-- 第二行/右侧：时间线区域 -->
            <div class="flex-1 min-w-0 sm:flex sm:justify-end">
              <div class="w-full sm:max-w-5xl space-y-2">
                <div class="flex items-center justify-between">
                  <span class="text-xs text-muted-foreground">健康度</span>
                  <span
                    class="text-sm font-semibold"
                    :class="getHealthScoreTextClass(getMonitorHealthScore(monitor))"
                  >
                    {{ formatHealthScore(getMonitorHealthScore(monitor)) }}
                  </span>
                </div>
                <EndpointHealthTimeline
                  :monitor="monitor"
                  :lookback-hours="resolvedLookbackHours"
                />
                <div class="space-y-1 text-[11px] text-muted-foreground">
                  <p
                    v-for="line in getEndpointInsightLines(monitor)"
                    :key="`${monitor.api_format}-${line}`"
                  >
                    {{ line }}
                  </p>
                </div>
              </div>
            </div>
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
import Label from '@/components/ui/label.vue'
import Select from '@/components/ui/select.vue'
import SelectTrigger from '@/components/ui/select-trigger.vue'
import SelectValue from '@/components/ui/select-value.vue'
import SelectContent from '@/components/ui/select-content.vue'
import SelectItem from '@/components/ui/select-item.vue'
import RefreshButton from '@/components/ui/refresh-button.vue'
import EndpointHealthTimeline from './EndpointHealthTimeline.vue'
import { getEndpointStatusMonitor, getHealthSummary, getPublicEndpointStatusMonitor } from '@/api/endpoints/health'
import { getModelCatalog } from '@/api/endpoints/models'
import type {
  EndpointStatusMonitor,
  HealthSummary,
  ModelCatalogItem,
  ProviderWithEndpointsSummary,
  PublicEndpointStatusMonitor,
} from '@/api/endpoints/types'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'
import { formatApiFormat } from '@/api/endpoints/types/api-format'
import {
  fetchAllProviderSummaries,
  formatHealthScore,
  getHealthTextClass,
} from '@/features/providers/utils/healthMonitorUtils'

const props = withDefaults(defineProps<{
  title?: string
  isAdmin?: boolean
  showProviderInfo?: boolean
  lookbackHours?: number
  showLookbackControl?: boolean
}>(), {
  title: '端点监控',
  isAdmin: false,
  showProviderInfo: false,
  lookbackHours: undefined,
  showLookbackControl: true,
})

const { error: showError } = useToast()

const loading = ref(false)
const loadingMonitors = ref(false)
const monitors = ref<(EndpointStatusMonitor | PublicEndpointStatusMonitor)[]>([])
const healthSummary = ref<HealthSummary | null>(null)
const providerSummaries = ref<ProviderWithEndpointsSummary[]>([])
const modelCatalogItems = ref<ModelCatalogItem[]>([])
const localLookbackHours = ref(String(props.lookbackHours ?? 6))
const resolvedLookbackHours = computed(() => props.lookbackHours ?? parseInt(localLookbackHours.value))

async function loadMonitors() {
  loadingMonitors.value = true
  try {
    const params = {
      lookback_hours: resolvedLookbackHours.value,
      per_format_limit: 100
    }

    if (props.isAdmin) {
      const data = await getEndpointStatusMonitor(params)
      monitors.value = data.formats || []
    } else {
      const data = await getPublicEndpointStatusMonitor(params)
      monitors.value = data.formats || []
    }
  } catch (err: unknown) {
    showError(parseApiError(err, '加载健康监控数据失败'), '错误')
  } finally {
    loadingMonitors.value = false
  }
}

async function loadHealthSummary() {
  if (!props.isAdmin) return

  try {
    healthSummary.value = await getHealthSummary()
  } catch (err: unknown) {
    showError(parseApiError(err, '加载端点摘要失败'), '错误')
  }
}

async function loadEndpointSupportContext() {
  if (!props.isAdmin) return

  try {
    const [providers, catalog] = await Promise.all([
      fetchAllProviderSummaries(),
      getModelCatalog(),
    ])

    providerSummaries.value = providers
    modelCatalogItems.value = catalog.models || []
  } catch (err: unknown) {
    showError(parseApiError(err, '加载端点支撑信息失败'), '错误')
  }
}

async function refreshData() {
  loading.value = true
  try {
    await Promise.all([
      loadMonitors(),
      loadHealthSummary(),
      loadEndpointSupportContext(),
    ])
  } finally {
    loading.value = false
  }
}

function getSuccessRateVariant(rate: number): 'default' | 'secondary' | 'destructive' | 'outline' {
  if (rate >= 0.95) return 'default'
  if (rate >= 0.8) return 'secondary'
  return 'destructive'
}

function getHealthScoreTextClass(score: number): string {
  return getHealthTextClass(score)
}

function getMonitorHealthScore(monitor: EndpointStatusMonitor | PublicEndpointStatusMonitor): number {
  const timeline = Array.isArray(monitor.timeline) ? monitor.timeline : []
  if (timeline.length === 0) {
    return Number.isFinite(monitor.success_rate) ? monitor.success_rate : 0
  }

  let total = 0
  let count = 0

  for (const status of timeline) {
    if (status === 'healthy') {
      total += 1
      count += 1
      continue
    }
    if (status === 'warning') {
      total += 0.7
      count += 1
      continue
    }
    if (status === 'unhealthy') {
      total += 0.3
      count += 1
    }
  }

  if (count === 0) {
    return Number.isFinite(monitor.success_rate) ? monitor.success_rate : 0
  }

  return total / count
}

const activeEndpointCountByFormat = computed(() => {
  const countMap = new Map<string, number>()

  for (const provider of providerSummaries.value) {
    if (!provider.is_active) continue

    for (const endpoint of provider.endpoint_health_details || []) {
      if (!endpoint.is_active) continue
      countMap.set(endpoint.api_format, (countMap.get(endpoint.api_format) || 0) + 1)
    }
  }

  return countMap
})

const activeProviderCountByFormat = computed(() => {
  const providerMap = new Map<string, Set<string>>()

  for (const provider of providerSummaries.value) {
    if (!provider.is_active) continue

    for (const endpoint of provider.endpoint_health_details || []) {
      if (!endpoint.is_active) continue
      if (!providerMap.has(endpoint.api_format)) {
        providerMap.set(endpoint.api_format, new Set())
      }
      providerMap.get(endpoint.api_format)?.add(provider.id)
    }
  }

  return providerMap
})

const activeModelCountByFormat = computed(() => {
  const providerMap = new Map(providerSummaries.value.map(provider => [provider.id, provider]))
  const modelMap = new Map<string, Set<string>>()

  for (const model of modelCatalogItems.value) {
    const matchedFormats = new Set<string>()

    for (const provider of model.providers) {
      const summary = providerMap.get(provider.provider_id)
      if (!summary?.is_active) continue

      for (const endpoint of summary.endpoint_health_details || []) {
        if (!endpoint.is_active) continue
        matchedFormats.add(endpoint.api_format)
      }
    }

    for (const format of matchedFormats) {
      if (!modelMap.has(format)) {
        modelMap.set(format, new Set())
      }
      modelMap.get(format)?.add(model.global_model_name)
    }
  }

  return modelMap
})

function getEndpointInsightLines(
  monitor: EndpointStatusMonitor | PublicEndpointStatusMonitor
): string[] {
  if (!props.isAdmin) {
    if (monitor.total_attempts <= 0) {
      return ['当前窗口暂无请求记录']
    }
    return [`当前窗口共记录 ${monitor.total_attempts} 次请求，成功率 ${(monitor.success_rate * 100).toFixed(0)}%`]
  }

  const format = monitor.api_format
  const activeEndpoints = activeEndpointCountByFormat.value.get(format) || 0
  const activeProviders = activeProviderCountByFormat.value.get(format)?.size || 0
  const activeModels = activeModelCountByFormat.value.get(format)?.size || 0
  const lines: string[] = []

  if (activeModels > 0 && activeEndpoints > 0) {
    lines.push(`${activeModels} 个活跃模型正在支撑 ${activeEndpoints} 个端点`)
  }

  if (activeProviders > 1) {
    lines.push(`${activeProviders} 个活跃提供商正在共同承载该端点格式`)
  } else if (activeProviders === 1) {
    lines.push('1 个活跃提供商正在承载该端点格式')
  } else if ('key_count' in monitor && monitor.key_count > 0) {
    lines.push(`${monitor.key_count} 个密钥正在支撑当前端点流量`)
  }

  if (lines.length === 0) {
    if (monitor.total_attempts > 0) {
      lines.push(`当前窗口共记录 ${monitor.total_attempts} 次请求，成功率 ${(monitor.success_rate * 100).toFixed(0)}%`)
    } else {
      lines.push('当前窗口暂无请求记录')
    }
  }

  return lines
}

const endpointSummary = computed(() => {
  const averageHealthScore = monitors.value.length > 0
    ? monitors.value.reduce((sum, monitor) => sum + getMonitorHealthScore(monitor), 0) / monitors.value.length
    : 0
  const totalAttempts = monitors.value.reduce((sum, monitor) => sum + monitor.total_attempts, 0)

  return {
    totalEndpoints: healthSummary.value?.endpoints.total ?? monitors.value.length,
    activeEndpoints: healthSummary.value?.endpoints.active ?? monitors.value.length,
    unhealthyEndpoints: healthSummary.value?.endpoints.unhealthy ?? monitors.value.filter(
      monitor => getMonitorHealthScore(monitor) < 0.5
    ).length,
    activeFormats: monitors.value.length,
    averageHealthScore,
    totalAttempts,
  }
})

watch(localLookbackHours, () => {
  if (props.showLookbackControl) {
    loadMonitors()
  }
})

watch(() => props.lookbackHours, (value, oldValue) => {
  if (value === undefined || value === oldValue) return
  localLookbackHours.value = String(value)
  loadMonitors()
})

onMounted(() => {
  refreshData()
})
</script>
