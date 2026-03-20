<template>
  <Card
    variant="default"
    class="overflow-hidden"
  >
    <div class="px-6 py-3.5 border-b border-border/60">
      <div class="flex items-center justify-between gap-4">
        <div class="space-y-1">
          <h3 class="text-base font-semibold">
            提供商监控
          </h3>
          <p class="text-xs text-muted-foreground">
            基于端点监控时间刻度聚合出的提供商视图
          </p>
        </div>
        <RefreshButton
          :loading="loading"
          @click="refreshData"
        />
      </div>
    </div>

    <div class="p-6">
      <div
        v-if="loadingData"
        class="flex items-center justify-center py-12"
      >
        <Loader2 class="w-6 h-6 animate-spin text-muted-foreground" />
        <span class="ml-2 text-muted-foreground">加载中...</span>
      </div>

      <div
        v-else-if="providers.length === 0"
        class="flex flex-col items-center justify-center py-12 text-muted-foreground"
      >
        <Activity class="w-12 h-12 mb-3 opacity-30" />
        <p>暂无提供商监控数据</p>
        <p class="text-xs mt-1">
          尚未配置提供商或端点
        </p>
      </div>

      <div
        v-else
        class="space-y-4"
      >
        <div class="grid gap-3 md:grid-cols-3">
          <div class="rounded-lg border border-border/60 bg-muted/20 p-4">
            <p class="text-xs text-muted-foreground">
              活跃提供商
            </p>
            <p class="mt-2 text-2xl font-semibold">
              {{ summary.activeProviders }}
            </p>
            <p class="mt-1 text-xs text-muted-foreground">
              共 {{ summary.totalProviders }} 个提供商
            </p>
          </div>

          <div class="rounded-lg border border-border/60 bg-muted/20 p-4">
            <p class="text-xs text-muted-foreground">
              平均健康度
            </p>
            <p
              class="mt-2 text-2xl font-semibold"
              :class="getHealthTextClass(summary.averageHealthScore)"
            >
              {{ formatHealthScore(summary.averageHealthScore) }}
            </p>
            <p class="mt-1 text-xs text-muted-foreground">
              仅统计启用中的提供商
            </p>
          </div>

          <div class="rounded-lg border border-border/60 bg-muted/20 p-4">
            <p class="text-xs text-muted-foreground">
              风险提供商
            </p>
            <p class="mt-2 text-2xl font-semibold">
              {{ summary.riskyProviders }}
            </p>
            <p class="mt-1 text-xs text-muted-foreground">
              存在异常端点或健康度偏低
            </p>
          </div>
        </div>

        <div class="space-y-3">
          <div
            v-for="provider in providers"
            :key="provider.id"
            class="rounded-lg border border-border/60 p-4 transition-colors hover:border-primary/50"
          >
            <div class="flex flex-col sm:flex-row sm:gap-6 sm:items-center">
              <div class="sm:w-52 flex-shrink-0 space-y-2 mb-3 sm:mb-0">
                <div class="flex items-center gap-2 flex-wrap">
                  <span class="text-sm font-semibold">
                    {{ provider.name }}
                  </span>
                  <Badge :variant="provider.isActive ? 'outline' : 'secondary'">
                    {{ provider.isActive ? '启用中' : '已停用' }}
                  </Badge>
                  <Badge :variant="getHealthBadgeVariant(provider.healthScore)">
                    {{ getHealthLabel(provider.healthScore) }}
                  </Badge>
                </div>

                <div class="flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground">
                  <span>端点 {{ provider.activeEndpoints }}/{{ provider.totalEndpoints }}</span>
                  <span>模型 {{ provider.activeModels }}/{{ provider.totalModels }}</span>
                  <span>异常端点 {{ provider.unhealthyEndpoints }}</span>
                  <span>密钥 {{ provider.activeKeys }}/{{ provider.totalKeys }}</span>
                </div>

                <div
                  v-if="provider.apiFormats.length > 0"
                  class="flex flex-wrap items-center gap-1.5"
                >
                  <Badge
                    v-for="format in provider.apiFormats.slice(0, 4)"
                    :key="`${provider.id}-${format}`"
                    variant="outline"
                    class="font-mono text-[11px]"
                  >
                    {{ formatApiFormat(format) }}
                  </Badge>
                  <span
                    v-if="provider.apiFormats.length > 4"
                    class="text-xs text-muted-foreground"
                  >
                    +{{ provider.apiFormats.length - 4 }}
                  </span>
                </div>
              </div>

              <div class="flex-1 min-w-0 sm:flex sm:justify-end">
                <div class="w-full sm:max-w-5xl space-y-2">
                  <div class="flex items-center justify-between">
                    <span class="text-xs text-muted-foreground">健康度</span>
                    <span
                      class="text-sm font-semibold"
                      :class="getHealthTextClass(provider.healthScore)"
                    >
                      {{ formatHealthScore(provider.healthScore) }}
                    </span>
                  </div>
                  <EndpointHealthTimeline
                    :monitor="provider.timelineMonitor"
                    :lookback-hours="resolvedLookbackHours"
                  />
                  <p class="text-[11px] text-muted-foreground">
                    {{ getProviderSummary(provider) }}
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
import { computed, onMounted, ref, watch } from 'vue'
import { Activity, Loader2 } from 'lucide-vue-next'
import Card from '@/components/ui/card.vue'
import Badge from '@/components/ui/badge.vue'
import RefreshButton from '@/components/ui/refresh-button.vue'
import EndpointHealthTimeline from '@/features/providers/components/EndpointHealthTimeline.vue'
import { getEndpointStatusMonitor } from '@/api/endpoints/health'
import type { EndpointStatusMonitor, ProviderWithEndpointsSummary } from '@/api/endpoints/types'
import { formatApiFormat } from '@/api/endpoints/types/api-format'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'
import {
  type HealthTimelineMonitorLike,
  fetchAllProviderSummaries,
  formatHealthScore,
  getHealthBadgeVariant,
  getHealthLabel,
  getHealthTextClass,
  mergeTimelineMonitors,
} from '@/features/providers/utils/healthMonitorUtils'

const props = withDefaults(defineProps<{
  lookbackHours?: number
}>(), {
  lookbackHours: 6,
})

interface ProviderHealthItem {
  id: string
  name: string
  isActive: boolean
  healthScore: number
  totalEndpoints: number
  activeEndpoints: number
  totalModels: number
  activeModels: number
  totalKeys: number
  activeKeys: number
  unhealthyEndpoints: number
  apiFormats: string[]
  timelineMonitor: HealthTimelineMonitorLike
}

const { error: showError } = useToast()

const loading = ref(false)
const loadingData = ref(false)
const rawProviders = ref<ProviderWithEndpointsSummary[]>([])
const endpointMonitors = ref<EndpointStatusMonitor[]>([])
const resolvedLookbackHours = computed(() => props.lookbackHours ?? 6)

const providers = computed<ProviderHealthItem[]>(() =>
  rawProviders.value
    .map(provider => {
      const matchedMonitors = provider.api_formats
        .map(format => endpointMonitors.value.find(monitor => monitor.api_format === format))
        .filter((monitor): monitor is EndpointStatusMonitor => Boolean(monitor))

      return {
        id: provider.id,
        name: provider.name,
        isActive: provider.is_active,
        healthScore: provider.avg_health_score,
        totalEndpoints: provider.total_endpoints,
        activeEndpoints: provider.active_endpoints,
        totalModels: provider.total_models,
        activeModels: provider.active_models,
        totalKeys: provider.total_keys,
        activeKeys: provider.active_keys,
        unhealthyEndpoints: provider.unhealthy_endpoints,
        apiFormats: [...new Set(provider.api_formats)],
        timelineMonitor: mergeTimelineMonitors(matchedMonitors, resolvedLookbackHours.value),
      }
    })
    .sort((a, b) => {
      if (a.isActive !== b.isActive) return a.isActive ? -1 : 1
      if (a.healthScore !== b.healthScore) return a.healthScore - b.healthScore
      if (a.unhealthyEndpoints !== b.unhealthyEndpoints) return b.unhealthyEndpoints - a.unhealthyEndpoints
      return a.name.localeCompare(b.name, 'zh-CN')
    })
)

const summary = computed(() => {
  const activeProviders = providers.value.filter(provider => provider.isActive)
  const averageHealthScore = activeProviders.length > 0
    ? activeProviders.reduce((total, provider) => total + provider.healthScore, 0) / activeProviders.length
    : 0
  const riskyProviders = activeProviders.filter(
    provider => provider.healthScore < 0.5 || provider.unhealthyEndpoints > 0
  ).length

  return {
    totalProviders: providers.value.length,
    activeProviders: activeProviders.length,
    averageHealthScore,
    riskyProviders,
  }
})

async function loadProviders() {
  loadingData.value = true
  try {
    const [providersResponse, endpointResponse] = await Promise.all([
      fetchAllProviderSummaries(),
      getEndpointStatusMonitor({
        lookback_hours: resolvedLookbackHours.value,
        per_format_limit: 100,
      }),
    ])

    rawProviders.value = providersResponse
    endpointMonitors.value = endpointResponse.formats || []
  } catch (err: unknown) {
    showError(parseApiError(err, '加载提供商监控数据失败'), '错误')
  } finally {
    loadingData.value = false
  }
}

async function refreshData() {
  loading.value = true
  try {
    await loadProviders()
  } finally {
    loading.value = false
  }
}

function getProviderSummary(provider: ProviderHealthItem): string {
  if (!provider.isActive) {
    return '该提供商已停用，不参与当前调度'
  }
  if (provider.totalEndpoints === 0) {
    return '尚未配置端点，暂无可观测链路'
  }
  if (provider.unhealthyEndpoints > 0) {
    return `${provider.unhealthyEndpoints} 个端点处于异常区间，建议优先排查`
  }
  if (provider.activeModels === 0) {
    return '暂无活跃模型，建议检查模型关联或启用状态'
  }
  return `${provider.activeModels} 个活跃模型正在支撑 ${provider.activeEndpoints} 个端点`
}

onMounted(() => {
  refreshData()
})

watch(() => props.lookbackHours, (value, oldValue) => {
  if (value === oldValue) return
  refreshData()
})
</script>
