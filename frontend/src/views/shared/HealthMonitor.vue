<template>
  <div class="space-y-6 pb-8">
    <Card
      variant="default"
      class="overflow-hidden"
    >
      <div class="relative overflow-hidden border-b border-border/60 px-6 py-5">
        <div class="absolute inset-0 bg-gradient-to-br from-primary/10 via-transparent to-muted/30" />
        <div class="relative flex flex-col gap-4 lg:flex-row lg:items-start lg:justify-between">
          <div class="max-w-3xl">
            <p class="text-xs font-medium uppercase tracking-[0.22em] text-primary/80">
              Health Dashboard
            </p>
            <h2 class="mt-2 text-2xl font-semibold tracking-tight">
              健康监控
            </h2>
            <p class="mt-2 text-sm leading-6 text-muted-foreground">
              统一查看端点、模型{{ isAdminPage ? '、提供商' : '' }}健康状态，先从概览判断风险，再进入具体视角排查关联健康。
            </p>
          </div>
        </div>
      </div>

      <div class="space-y-5 p-6">
        <div class="grid grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-4">
          <div
            v-for="card in overviewCards"
            :key="card.label"
            class="rounded-xl border border-border/60 bg-card/70 p-4"
          >
            <div class="flex items-start justify-between gap-3">
              <div>
                <p class="text-xs text-muted-foreground">
                  {{ card.label }}
                </p>
                <div
                  class="mt-2 text-2xl font-semibold tabular-nums"
                  :class="card.valueClass"
                >
                  {{ card.value }}
                </div>
              </div>
              <div class="flex h-10 w-10 flex-shrink-0 items-center justify-center rounded-xl border border-border/60 bg-muted/40">
                <component
                  :is="card.icon"
                  class="h-5 w-5 text-muted-foreground"
                />
              </div>
            </div>
            <p class="mt-3 text-xs text-muted-foreground">
              {{ card.description }}
            </p>
          </div>
        </div>

        <div>
          <div class="mb-3 flex flex-col gap-1 sm:flex-row sm:items-end sm:justify-between">
            <div>
              <h3 class="text-sm font-semibold">
                综合概览
              </h3>
              <p class="text-xs text-muted-foreground">
                点击视角卡片快速跳转到对应健康列表
              </p>
            </div>
          </div>

          <div
            class="grid grid-cols-1 gap-3"
            :class="isAdminPage ? 'lg:grid-cols-3' : 'lg:grid-cols-2'"
          >
            <button
              v-for="section in sectionCards"
              :key="section.key"
              type="button"
              class="group rounded-xl border border-border/60 bg-muted/20 p-4 text-left transition-colors hover:border-primary/50 hover:bg-primary/5"
              @click="scrollToSection(section.id)"
            >
              <div class="flex items-start justify-between gap-3">
                <div class="flex min-w-0 items-center gap-3">
                  <div class="flex h-11 w-11 flex-shrink-0 items-center justify-center rounded-xl border border-border/60 bg-card/70 transition-colors group-hover:border-primary/40">
                    <component
                      :is="section.icon"
                      class="h-5 w-5 text-muted-foreground"
                    />
                  </div>
                  <div class="min-w-0">
                    <h4 class="truncate text-sm font-semibold">
                      {{ section.title }}
                    </h4>
                    <p class="mt-1 text-xs text-muted-foreground">
                      {{ section.description }}
                    </p>
                  </div>
                </div>
                <Badge
                  :variant="section.badgeVariant"
                  class="shrink-0"
                >
                  {{ section.badgeLabel }}
                </Badge>
              </div>
              <div class="mt-4 grid grid-cols-3 gap-2 text-xs">
                <div class="rounded-lg border border-border/40 bg-card/50 px-3 py-2">
                  <p class="text-muted-foreground">
                    总数
                  </p>
                  <p class="mt-1 font-semibold tabular-nums">
                    {{ section.summary.total }}
                  </p>
                </div>
                <div class="rounded-lg border border-border/40 bg-card/50 px-3 py-2">
                  <p class="text-muted-foreground">
                    异常
                  </p>
                  <p class="mt-1 font-semibold tabular-nums text-red-600 dark:text-red-400">
                    {{ section.summary.unhealthy }}
                  </p>
                </div>
                <div class="rounded-lg border border-border/40 bg-card/50 px-3 py-2">
                  <p class="text-muted-foreground">
                    波动
                  </p>
                  <p class="mt-1 font-semibold tabular-nums text-amber-600 dark:text-amber-400">
                    {{ section.summary.warning }}
                  </p>
                </div>
              </div>
            </button>
          </div>
        </div>
      </div>
    </Card>

    <section
      id="health-endpoints"
      class="scroll-mt-6"
    >
      <HealthMonitorCard
        title="端点健康监控"
        :is-admin="isAdminPage"
        :show-provider-info="isAdminPage"
        @view-details="openHealthDetails"
        @summary-updated="updateSummary('endpoint', $event)"
      />
    </section>

    <section
      id="health-models"
      class="scroll-mt-6"
    >
      <ModelHealthMonitorCard
        title="模型健康监控"
        :is-admin="isAdminPage"
        :show-provider-info="isAdminPage"
        @view-details="openHealthDetails"
        @summary-updated="updateSummary('model', $event)"
      />
    </section>

    <section
      v-if="isAdminPage"
      id="health-providers"
      class="scroll-mt-6"
    >
      <ProviderHealthMonitorCard
        title="提供商健康监控"
        @view-details="openHealthDetails"
        @summary-updated="updateSummary('provider', $event)"
      />
    </section>

    <HealthMonitorDetailDrawer
      v-model:open="detailOpen"
      :target="detailTarget"
      :is-admin="isAdminPage"
      @view-details="openHealthDetails"
    />
  </div>
</template>

<script setup lang="ts">
import { computed, ref, type Component } from 'vue'
import { useRoute } from 'vue-router'
import { Activity, Bot, Gauge, Server, Zap } from 'lucide-vue-next'
import Card from '@/components/ui/card.vue'
import Badge from '@/components/ui/badge.vue'
import HealthMonitorCard from '@/features/providers/components/HealthMonitorCard.vue'
import ModelHealthMonitorCard from '@/features/providers/components/ModelHealthMonitorCard.vue'
import ProviderHealthMonitorCard from '@/features/providers/components/ProviderHealthMonitorCard.vue'
import HealthMonitorDetailDrawer from '@/features/providers/components/HealthMonitorDetailDrawer.vue'
import {
  createEmptyHealthMonitorSectionSummary,
  formatCompactNumber,
  type HealthBadgeVariant,
  type HealthMonitorDetailTarget,
  type HealthMonitorSectionSummary,
  type HealthMonitorSourceKind
} from '@/features/providers/components/health-monitor-utils'

const route = useRoute()
const isAdminPage = computed(() => route.path.startsWith('/admin'))
const detailOpen = ref(false)
const detailTarget = ref<HealthMonitorDetailTarget | null>(null)
const sectionSummaries = ref<Partial<Record<HealthMonitorSourceKind, HealthMonitorSectionSummary>>>({})

const expectedSectionKeys = computed<HealthMonitorSourceKind[]>(() => (
  isAdminPage.value ? ['endpoint', 'model', 'provider'] : ['endpoint', 'model']
))

const loadedSectionCount = computed(() => (
  expectedSectionKeys.value.filter(key => sectionSummaries.value[key]).length
))

const allSectionsLoaded = computed(() => loadedSectionCount.value >= expectedSectionKeys.value.length)

const combinedSummary = computed(() => {
  const summary = createEmptyHealthMonitorSectionSummary()
  for (const key of expectedSectionKeys.value) {
    const section = getSummary(key)
    summary.total += section.total
    summary.healthy += section.healthy
    summary.warning += section.warning
    summary.unhealthy += section.unhealthy
    summary.empty += section.empty
    summary.attempts += section.attempts
  }
  return summary
})

const overallLabel = computed(() => getStatusLabel(combinedSummary.value, allSectionsLoaded.value))

const overviewCards = computed(() => {
  const endpointSummary = getSummary('endpoint')
  const modelSummary = getSummary('model')
  const providerSummary = getSummary('provider')
  const cards = [
    {
      label: '总体状态',
      value: overallLabel.value,
      description: allSectionsLoaded.value
        ? `${combinedSummary.value.total} 项健康对象 / ${formatCompactNumber(combinedSummary.value.attempts)} 次请求`
        : `${loadedSectionCount.value}/${expectedSectionKeys.value.length} 个视角已加载`,
      icon: Gauge,
      valueClass: getStatusValueClass(combinedSummary.value, allSectionsLoaded.value)
    },
    {
      label: '异常端点',
      value: endpointSummary.unhealthy,
      description: `${endpointSummary.warning} 个波动 / ${formatCompactNumber(endpointSummary.attempts)} 次请求`,
      icon: Activity,
      valueClass: endpointSummary.unhealthy > 0 ? 'text-red-600 dark:text-red-400' : ''
    },
    {
      label: '异常模型',
      value: modelSummary.unhealthy,
      description: `${modelSummary.warning} 个波动 / ${formatCompactNumber(modelSummary.attempts)} 次请求`,
      icon: Bot,
      valueClass: modelSummary.unhealthy > 0 ? 'text-red-600 dark:text-red-400' : ''
    }
  ]

  if (isAdminPage.value) {
    cards.push({
      label: '异常提供商',
      value: providerSummary.unhealthy,
      description: `${providerSummary.warning} 个波动 / ${providerSummary.empty} 个暂无请求`,
      icon: Server,
      valueClass: providerSummary.unhealthy > 0 ? 'text-red-600 dark:text-red-400' : ''
    })
  } else {
    cards.push({
      label: '请求总量',
      value: formatCompactNumber(combinedSummary.value.attempts),
      description: '当前回溯窗口内参与健康统计的请求',
      icon: Zap,
      valueClass: ''
    })
  }

  return cards
})

const sectionCards = computed(() => {
  const sections = [
    buildSectionCard('endpoint', 'health-endpoints', '端点视角', '按 API 入口定位入口层健康', Activity),
    buildSectionCard('model', 'health-models', '模型视角', '按模型聚合查看跨提供商健康', Bot)
  ]

  if (isAdminPage.value) {
    sections.push(buildSectionCard('provider', 'health-providers', '提供商视角', '按提供商定位供应商侧波动', Server))
  }

  return sections
})

function openHealthDetails(target: HealthMonitorDetailTarget) {
  detailTarget.value = target
  detailOpen.value = true
}

function updateSummary(kind: HealthMonitorSourceKind, summary: HealthMonitorSectionSummary) {
  sectionSummaries.value = {
    ...sectionSummaries.value,
    [kind]: summary
  }
}

function getSummary(kind: HealthMonitorSourceKind) {
  return sectionSummaries.value[kind] || createEmptyHealthMonitorSectionSummary()
}

function buildSectionCard(
  key: HealthMonitorSourceKind,
  id: string,
  title: string,
  description: string,
  icon: Component
) {
  const summary = getSummary(key)
  const loaded = Boolean(sectionSummaries.value[key])
  return {
    key,
    id,
    title,
    description,
    icon,
    summary,
    badgeLabel: getStatusLabel(summary, loaded),
    badgeVariant: getStatusBadgeVariant(summary, loaded)
  }
}

function getStatusLabel(summary: HealthMonitorSectionSummary, loaded: boolean) {
  if (!loaded) return '加载中'
  if (summary.total === 0) return '暂无数据'
  if (summary.unhealthy > 0) return '异常'
  if (summary.warning > 0) return '波动'
  if (summary.empty > 0) return '部分暂无请求'
  return '正常'
}

function getStatusBadgeVariant(
  summary: HealthMonitorSectionSummary,
  loaded: boolean
): HealthBadgeVariant {
  if (!loaded || summary.total === 0) return 'outline'
  if (summary.unhealthy > 0) return 'destructive'
  if (summary.warning > 0 || summary.empty > 0) return 'warning'
  return 'success'
}

function getStatusValueClass(summary: HealthMonitorSectionSummary, loaded: boolean) {
  if (!loaded || summary.total === 0) return ''
  if (summary.unhealthy > 0) return 'text-red-600 dark:text-red-400'
  if (summary.warning > 0 || summary.empty > 0) return 'text-amber-600 dark:text-amber-400'
  return 'text-green-600 dark:text-green-400'
}

function scrollToSection(id: string) {
  document.getElementById(id)?.scrollIntoView({
    behavior: 'smooth',
    block: 'start'
  })
}
</script>
