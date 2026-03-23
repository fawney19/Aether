<template>
  <div class="space-y-4">
    <div class="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
      <div>
        <h3 class="text-sm font-semibold">
          {{ title }}
        </h3>
        <p
          v-if="subtitle"
          class="mt-1 text-[11px] text-muted-foreground"
        >
          {{ subtitle }}
        </p>
      </div>

      <div
        v-if="!unavailable"
        class="flex flex-wrap gap-2"
      >
        <div class="rounded-full border border-rose-500/20 bg-rose-500/8 px-2.5 py-1 text-[10px] font-medium text-rose-700 dark:text-rose-300">
          总错误 {{ totalErrors }}
        </div>
        <div
          v-if="topCategory"
          class="rounded-full border border-border/70 bg-muted/20 px-2.5 py-1 text-[10px] font-medium text-muted-foreground"
        >
          TOP {{ topCategory.label }} · {{ topCategory.count }}
        </div>
      </div>
    </div>

    <div
      v-if="loading"
      class="p-6"
    >
      <LoadingState />
    </div>
    <div
      v-else-if="unavailable"
      class="flex h-[272px] items-center justify-center rounded-2xl border border-dashed border-border/70 bg-muted/10 text-xs text-muted-foreground"
    >
      {{ unavailableText }}
    </div>
    <div
      v-else-if="!hasData"
      class="flex h-[272px] items-center justify-center rounded-2xl border border-dashed border-border/70 bg-muted/10 text-xs text-muted-foreground"
    >
      暂无错误分布数据
    </div>
    <div
      v-else
      class="rounded-2xl border border-border/60 bg-background p-3"
    >
      <div class="mx-auto w-full max-w-[320px]">
        <DoughnutChart
          :data="chartData"
          :options="chartOptions"
          :show-legend="true"
          :height="248"
        />
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import type { ChartData, ChartOptions } from 'chart.js'
import DoughnutChart from '@/components/charts/DoughnutChart.vue'
import { LoadingState } from '@/components/common'

interface ErrorDistributionItem {
  category: string
  label: string
  count: number
}

interface Props {
  title: string
  subtitle?: string
  distribution: ErrorDistributionItem[]
  loading?: boolean
  unavailable?: boolean
  unavailableText?: string
}

const props = withDefaults(defineProps<Props>(), {
  subtitle: undefined,
  loading: false,
  unavailable: false,
  unavailableText: '数据暂不可用',
})

const chartColors = [
  'rgba(226, 85, 118, 0.88)',
  'rgba(84, 135, 237, 0.86)',
  'rgba(225, 165, 52, 0.84)',
  'rgba(68, 187, 132, 0.82)',
  'rgba(148, 163, 184, 0.8)',
  'rgba(145, 118, 214, 0.82)',
]

const totalErrors = computed(() => props.distribution.reduce((sum, item) => sum + item.count, 0))
const hasData = computed(() => totalErrors.value > 0)
const distributionWithShare = computed(() => props.distribution.map(item => ({
  ...item,
  label: item.label?.trim() || item.category,
  share: totalErrors.value > 0 ? `${((item.count / totalErrors.value) * 100).toFixed(1)}%` : '0.0%',
})))
const topCategory = computed(() => (
  distributionWithShare.value.slice().sort((left, right) => right.count - left.count)[0] ?? null
))

const chartData = computed<ChartData<'doughnut'>>(() => ({
  labels: distributionWithShare.value.map(item => item.label),
  datasets: [
    {
      data: distributionWithShare.value.map(item => item.count),
      backgroundColor: distributionWithShare.value.map((_, index) => chartColors[index % chartColors.length]),
      borderColor: 'rgba(255,255,255,0.92)',
      borderWidth: 2,
      hoverOffset: 6,
    },
  ],
}))

const chartOptions = computed<ChartOptions<'doughnut'>>(() => ({
  cutout: '68%',
  plugins: {
    legend: {
      position: 'bottom',
      align: 'center',
      labels: {
        usePointStyle: true,
        pointStyle: 'circle',
        boxWidth: 10,
        padding: 14,
      },
    },
    tooltip: {
      callbacks: {
        label: (context) => {
          const value = Number(context.raw ?? 0)
          const percentage = totalErrors.value > 0 ? ((value / totalErrors.value) * 100).toFixed(1) : '0.0'
          return `${context.label ?? '错误'}: ${value} (${percentage}%)`
        },
      },
    },
  },
}))
</script>
