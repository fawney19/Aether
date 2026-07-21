<template>
  <div
    v-if="cycle && cycleMetricRows.length > 0"
    :class="cycleContainerClass"
    :data-testid="variant === 'desktop' ? 'pool-stats-cycle-text' : 'pool-mobile-stats-cycle-text'"
  >
    <div
      v-for="row in cycleMetricRows"
      :key="`${row.key}-${variant}-cycle-row`"
      class="flex items-baseline justify-between gap-3"
      :title="`${row.label} ${row.valueText}`"
    >
      <span class="shrink-0 text-muted-foreground">
        {{ row.label }}
      </span>
      <span
        class="grid w-[112px] shrink-0 grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)] items-baseline gap-x-1 font-medium text-foreground"
        :data-testid="variant === 'desktop' ? `pool-stats-cycle-${row.key}` : undefined"
      >
        <span class="min-w-0 truncate text-right">{{ row.hasComparison ? row.smallValue : '-' }}</span>
        <span
          class="w-1.5 text-center text-muted-foreground/60"
          data-cycle-stat-part="divider"
          aria-hidden="true"
        >/</span>
        <span class="min-w-0 truncate text-left">{{ row.largeValue }}</span>
      </span>
    </div>
  </div>

  <div
    v-else-if="cycle"
    :class="cycleContainerClass"
    :data-testid="variant === 'desktop' ? 'pool-stats-cycle-empty' : 'pool-mobile-stats-cycle-empty'"
  >
    <div class="flex min-h-16 items-center justify-center text-muted-foreground">
      —
    </div>
  </div>

  <div
    v-else
    :class="accountContainerClass"
    :data-testid="variant === 'desktop' ? 'pool-stats-account-total' : undefined"
  >
    <div
      class="invisible h-4"
      aria-hidden="true"
    >
      -
    </div>
    <div
      v-for="metric in accountMetrics"
      :key="`${metric.key}-${variant}-account-total`"
      :class="accountMetricRowClass"
    >
      <span class="text-muted-foreground truncate">{{ metric.label }}</span>
      <span
        :class="accountValueClass"
        :title="metric.value"
      >
        {{ metric.value }}
      </span>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import type {
  PoolCodexCycleStatsGroup,
  PoolStatsMetric,
  PoolStatsMetricKey,
} from '@/features/pool/utils/poolStatsDisplay'

const props = withDefaults(defineProps<{
  cycle: boolean
  cycleGroups: PoolCodexCycleStatsGroup[]
  accountMetrics: PoolStatsMetric[]
  variant?: 'desktop' | 'mobile'
}>(), {
  variant: 'desktop',
})

const DEFAULT_CYCLE_METRIC_KEYS: PoolStatsMetricKey[] = ['request_count', 'total_tokens', 'total_cost_usd']
const CYCLE_METRIC_LABELS: Record<PoolStatsMetricKey, string> = {
  request_count: '请求',
  total_tokens: 'Token',
  total_cost_usd: '费用',
}

function missingMetric(key: PoolStatsMetricKey): PoolStatsMetric {
  return {
    key,
    label: CYCLE_METRIC_LABELS[key],
    value: '-',
    missing: true,
    numericValue: null,
  }
}

function metricForGroup(
  group: PoolCodexCycleStatsGroup | undefined,
  key: PoolStatsMetricKey,
): PoolStatsMetric {
  return group?.metrics.find(metric => metric.key === key) ?? missingMetric(key)
}

const cycleMetricKeys = computed(() => {
  const keys: PoolStatsMetricKey[] = []
  for (const group of props.cycleGroups) {
    for (const metric of group.metrics) {
      if (!keys.includes(metric.key)) keys.push(metric.key)
    }
  }
  return keys.length > 0 ? keys : DEFAULT_CYCLE_METRIC_KEYS
})

const cycleMetricRows = computed(() => {
  const smallGroup = props.cycleGroups.length > 1 ? props.cycleGroups[0] : undefined
  const largeGroup = props.cycleGroups.at(-1)
  if (!largeGroup) return []

  return cycleMetricKeys.value.map((key) => {
    const smallMetric = metricForGroup(smallGroup, key)
    const largeMetric = metricForGroup(largeGroup, key)
    const hasComparison = Boolean(smallGroup)
    return {
      key,
      label: CYCLE_METRIC_LABELS[key],
      hasComparison,
      smallValue: smallMetric.value,
      largeValue: largeMetric.value,
      valueText: hasComparison ? `${smallMetric.value}/${largeMetric.value}` : largeMetric.value,
    }
  })
})

const cycleContainerClass = computed(() => [
  'w-full space-y-1 text-[11px] leading-4 tabular-nums',
  props.variant === 'desktop' ? 'mx-auto max-w-[168px]' : 'py-0.5',
].filter(Boolean).join(' '))

const accountContainerClass = computed(() => props.variant === 'desktop'
  ? 'grid min-h-16 w-[188px] grid-rows-4 gap-0 mx-auto text-[10px] leading-4'
  : ''
)

const accountMetricRowClass = computed(() => props.variant === 'desktop'
  ? 'grid grid-cols-[64px_124px] items-center'
  : 'grid h-4 w-[188px] grid-cols-[64px_124px] items-center text-left'
)

const accountValueClass = computed(() => [
  'min-w-0 truncate text-center text-foreground/90',
  props.variant === 'desktop' ? 'tabular-nums' : 'font-medium',
].join(' '))
</script>
