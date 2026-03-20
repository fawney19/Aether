<template>
  <Card
    variant="default"
    class="overflow-hidden"
  >
    <div class="px-6 py-3.5 border-b border-border/60">
      <div class="flex items-center justify-between gap-4">
        <div class="space-y-1">
          <h3 class="text-base font-semibold">
            模型监控
          </h3>
          <p class="text-xs text-muted-foreground">
            基于端点监控时间刻度聚合出的模型视图
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
        v-else-if="models.length === 0"
        class="flex flex-col items-center justify-center py-12 text-muted-foreground"
      >
        <Activity class="w-12 h-12 mb-3 opacity-30" />
        <p>暂无模型监控数据</p>
        <p class="text-xs mt-1">
          尚未配置可用模型
        </p>
      </div>

      <div
        v-else
        class="space-y-4"
      >
        <div class="grid gap-3 md:grid-cols-3">
          <div class="rounded-lg border border-border/60 bg-muted/20 p-4">
            <p class="text-xs text-muted-foreground">
              活跃模型
            </p>
            <p class="mt-2 text-2xl font-semibold">
              {{ summary.totalModels }}
            </p>
            <p class="mt-1 text-xs text-muted-foreground">
              当前模型目录中的可用模型
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
              基于承载该模型的活跃提供商计算
            </p>
          </div>

          <div class="rounded-lg border border-border/60 bg-muted/20 p-4">
            <p class="text-xs text-muted-foreground">
              覆盖风险模型
            </p>
            <p class="mt-2 text-2xl font-semibold">
              {{ summary.riskyModels }}
            </p>
            <p class="mt-1 text-xs text-muted-foreground">
              无活跃提供商或存在健康短板
            </p>
          </div>
        </div>

        <div class="space-y-3">
          <div
            v-for="model in models"
            :key="model.id"
            class="rounded-lg border border-border/60 p-4 transition-colors hover:border-primary/50"
          >
            <div class="flex flex-col sm:flex-row sm:gap-6 sm:items-center">
              <div class="sm:w-52 flex-shrink-0 space-y-2 mb-3 sm:mb-0">
                <div class="flex items-center gap-2 flex-wrap">
                  <span
                    class="text-sm font-semibold"
                    :title="model.name"
                  >
                    {{ model.displayName }}
                  </span>
                  <Badge :variant="getHealthBadgeVariant(model.healthScore)">
                    {{ getHealthLabel(model.healthScore) }}
                  </Badge>
                </div>

                <div class="flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground">
                  <span>活跃提供商 {{ model.activeProviders }}/{{ model.totalProviders }}</span>
                  <span>风险提供商 {{ model.riskyProviders }}</span>
                  <span>冗余级别 {{ getCoverageLabel(model.activeProviders) }}</span>
                </div>

                <div
                  v-if="model.providerNames.length > 0"
                  class="flex flex-wrap items-center gap-1.5"
                >
                  <Badge
                    v-for="providerName in model.providerNames.slice(0, 4)"
                    :key="`${model.id}-${providerName}`"
                    variant="outline"
                    class="text-[11px]"
                  >
                    {{ providerName }}
                  </Badge>
                  <span
                    v-if="model.providerNames.length > 4"
                    class="text-xs text-muted-foreground"
                  >
                    +{{ model.providerNames.length - 4 }}
                  </span>
                </div>
              </div>

              <div class="flex-1 min-w-0 sm:flex sm:justify-end">
                <div class="w-full sm:max-w-5xl space-y-2">
                  <div class="flex items-center justify-between">
                    <span class="text-xs text-muted-foreground">健康度</span>
                    <span
                      class="text-sm font-semibold"
                      :class="getHealthTextClass(model.healthScore)"
                    >
                      {{ formatHealthScore(model.healthScore) }}
                    </span>
                  </div>
                  <EndpointHealthTimeline
                    :monitor="model.timelineMonitor"
                    :lookback-hours="resolvedLookbackHours"
                  />
                  <p class="text-[11px] text-muted-foreground">
                    {{ getModelSummary(model) }}
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
import { getModelCatalog } from '@/api/endpoints/models'
import type { EndpointStatusMonitor, ModelCatalogItem, ProviderWithEndpointsSummary } from '@/api/endpoints/types'
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

interface ModelHealthItem {
  id: string
  name: string
  displayName: string
  healthScore: number
  totalProviders: number
  activeProviders: number
  riskyProviders: number
  providerNames: string[]
  timelineMonitor: HealthTimelineMonitorLike
}

const { error: showError } = useToast()

const loading = ref(false)
const loadingData = ref(false)
const rawModels = ref<ModelHealthItem[]>([])
const resolvedLookbackHours = computed(() => props.lookbackHours ?? 6)

const models = computed(() =>
  [...rawModels.value].sort((a, b) => {
    if (a.healthScore !== b.healthScore) return a.healthScore - b.healthScore
    if (a.activeProviders !== b.activeProviders) return a.activeProviders - b.activeProviders
    return a.displayName.localeCompare(b.displayName, 'zh-CN')
  })
)

const summary = computed(() => {
  const averageHealthScore = models.value.length > 0
    ? models.value.reduce((total, model) => total + model.healthScore, 0) / models.value.length
    : 0
  const riskyModels = models.value.filter(
    model => model.activeProviders === 0 || model.riskyProviders > 0 || model.healthScore < 0.5
  ).length

  return {
    totalModels: models.value.length,
    averageHealthScore,
    riskyModels,
  }
})

async function loadModels() {
  loadingData.value = true
  try {
    const [providers, endpointResponse, catalog] = await Promise.all([
      fetchAllProviderSummaries(),
      getEndpointStatusMonitor({
        lookback_hours: resolvedLookbackHours.value,
        per_format_limit: 100,
      }),
      getModelCatalog(),
    ])

    rawModels.value = buildModelHealthItems(catalog.models, providers, endpointResponse.formats || [])
  } catch (err: unknown) {
    showError(parseApiError(err, '加载模型监控数据失败'), '错误')
  } finally {
    loadingData.value = false
  }
}

async function refreshData() {
  loading.value = true
  try {
    await loadModels()
  } finally {
    loading.value = false
  }
}

function buildModelHealthItems(
  catalogModels: ModelCatalogItem[],
  providers: ProviderWithEndpointsSummary[],
  endpointMonitors: EndpointStatusMonitor[]
): ModelHealthItem[] {
  const providerMap = new Map(providers.map(provider => [provider.id, provider]))
  const providerTimelineMap = new Map(
    providers.map(provider => {
      const matchedMonitors = provider.api_formats
        .map(format => endpointMonitors.find(monitor => monitor.api_format === format))
        .filter((monitor): monitor is EndpointStatusMonitor => Boolean(monitor))

      return [
        provider.id,
        mergeTimelineMonitors(matchedMonitors, resolvedLookbackHours.value),
      ] as const
    })
  )

  return catalogModels.map(model => {
    const uniqueProviders = [...new Map(
      model.providers.map(provider => [provider.provider_id, provider])
    ).values()]

    const activeProviders = uniqueProviders
      .map(provider => providerMap.get(provider.provider_id))
      .filter((provider): provider is ProviderWithEndpointsSummary => Boolean(provider?.is_active))

    const healthScores = activeProviders.map(provider => provider.avg_health_score)
    const healthScore = healthScores.length > 0
      ? healthScores.reduce((total, score) => total + score, 0) / healthScores.length
      : 0
    const riskyProviders = activeProviders.filter(
      provider => provider.avg_health_score < 0.5 || provider.unhealthy_endpoints > 0
    ).length
    const timelineMonitor = mergeTimelineMonitors(
      activeProviders
        .map(provider => providerTimelineMap.get(provider.id))
        .filter((monitor): monitor is HealthTimelineMonitorLike => Boolean(monitor)),
      resolvedLookbackHours.value
    )

    return {
      id: model.global_model_name,
      name: model.global_model_name,
      displayName: model.display_name,
      healthScore,
      totalProviders: uniqueProviders.length,
      activeProviders: activeProviders.length,
      riskyProviders,
      providerNames: activeProviders.map(provider => provider.name),
      timelineMonitor,
    }
  })
}

function getCoverageLabel(activeProviders: number): string {
  if (activeProviders >= 3) return '高'
  if (activeProviders >= 2) return '中'
  if (activeProviders >= 1) return '低'
  return '无'
}

function getModelSummary(model: ModelHealthItem): string {
  if (model.activeProviders === 0) {
    return '暂无活跃提供商可承载该模型'
  }
  if (model.riskyProviders > 0) {
    return `${model.riskyProviders} 个提供商健康偏低，建议检查冗余与路由`
  }
  if (model.activeProviders === 1) {
    return '当前仅有单提供商承载，建议补充冗余'
  }
  return `${model.activeProviders} 个活跃提供商正在共同承载该模型`
}

onMounted(() => {
  refreshData()
})

watch(() => props.lookbackHours, (value, oldValue) => {
  if (value === oldValue) return
  refreshData()
})
</script>
