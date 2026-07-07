<template>
  <div
    v-if="cycle"
    :class="cycleContainerClass"
    :data-testid="variant === 'desktop' ? 'pool-stats-cycle-groups' : undefined"
  >
    <div
      :class="cycleGridClass"
      :data-testid="variant === 'desktop' ? 'pool-stats-cycle-grid' : 'pool-mobile-stats-cycle-grid'"
    >
      <span aria-hidden="true" />
      <span
        :class="cycleGroupLabelClass"
        :data-testid="variant === 'desktop' ? 'pool-stats-cycle-group-5h' : 'pool-mobile-stats-cycle-group-5h'"
      >{{ fiveHourGroupLabel }}</span>
      <span class="text-center text-muted-foreground/50">|</span>
      <span
        :class="cycleGroupLabelClass"
        :data-testid="variant === 'desktop' ? 'pool-stats-cycle-group-weekly' : 'pool-mobile-stats-cycle-group-weekly'"
      >{{ weeklyGroupLabel }}</span>

      <template
        v-for="row in cycleRows"
        :key="`${row.key}-${variant}-cycle-row`"
      >
        <span class="text-muted-foreground truncate">{{ row.label }}</span>
        <span
          :class="[cycleValueClass, row.fiveH.missing ? 'text-muted-foreground/80' : '']"
          :data-testid="variant === 'desktop' ? `pool-stats-5h-${row.key}` : undefined"
          :title="row.fiveH.value"
        >{{ row.fiveH.value }}</span>
        <span class="text-center text-muted-foreground/50">|</span>
        <span
          :class="[cycleValueClass, row.weekly.missing ? 'text-muted-foreground/80' : '']"
          :data-testid="variant === 'desktop' ? `pool-stats-weekly-${row.key}` : undefined"
          :title="row.weekly.value"
        >{{ row.weekly.value }}</span>
      </template>
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
import { useI18n } from '@/i18n'
import type { PoolStatsMetric } from '@/features/pool/utils/poolStatsDisplay'

export interface PoolKeyCycleStatsRow {
  key: PoolStatsMetric['key']
  label: string
  fiveH: PoolStatsMetric
  weekly: PoolStatsMetric
}

const props = withDefaults(defineProps<{
  cycle: boolean
  cycleRows: PoolKeyCycleStatsRow[]
  accountMetrics: PoolStatsMetric[]
  variant?: 'desktop' | 'mobile'
  fiveHourGroupLabel?: string
  weeklyGroupLabel?: string
}>(), {
  variant: 'desktop',
  fiveHourGroupLabel: '5H',
  weeklyGroupLabel: undefined,
})

const { legacyT } = useI18n()

const weeklyGroupLabel = computed(() => props.weeklyGroupLabel ?? legacyT('周'))

const cycleContainerClass = computed(() => props.variant === 'desktop'
  ? 'mx-auto w-[188px] text-[10px] leading-4'
  : ''
)

const cycleGridClass = computed(() => [
  'grid min-h-16 w-[188px] grid-cols-[38px_64px_10px_64px] items-center gap-x-1',
  props.variant === 'mobile' ? 'text-left' : '',
].filter(Boolean).join(' '))

const cycleGroupLabelClass = computed(() => props.variant === 'desktop'
  ? 'text-center text-[9px] font-semibold text-muted-foreground/80'
  : 'text-center text-[10px] font-semibold text-foreground'
)

const cycleValueClass = computed(() => [
  'min-w-0 truncate text-center text-foreground/90',
  props.variant === 'desktop' ? 'tabular-nums' : 'font-medium tabular-nums',
].join(' '))

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
