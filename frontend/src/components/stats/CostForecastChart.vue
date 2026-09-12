<template>
  <div class="space-y-3">
    <div class="flex items-center justify-between">
      <h3 class="text-sm font-semibold">
        {{ title }}
      </h3>
      <span
        v-if="subtitle"
        class="text-xs text-muted-foreground"
      >{{ subtitle }}</span>
    </div>
    <div
      v-if="loading"
      class="p-6"
    >
      <LoadingState />
    </div>
    <div
      v-else
      class="h-[280px]"
    >
      <LineChart
        :data="chartData"
        :options="chartOptions"
      />
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from '@/i18n'
import LineChart from '@/components/charts/LineChart.vue'
import { LoadingState } from '@/components/common'
import { formatCurrency } from '@/utils/format'

interface Props {
  title: string
  subtitle?: string
  history: Array<{ date: string; total_cost: number }>
  forecast: Array<{ date: string; total_cost: number }>
  loading?: boolean
}

const props = withDefaults(defineProps<Props>(), {
  subtitle: undefined,
  loading: false
})
const { t } = useI18n()

const labels = computed(() => [
  ...props.history.map(item => item.date),
  ...props.forecast.map(item => item.date)
])

const chartData = computed(() => {
  const historyValues = props.history.map(item => item.total_cost)
  const forecastValues = props.forecast.map(item => item.total_cost)
  return {
    labels: labels.value,
    datasets: [
      {
        label: t('chart.actualCost'),
        data: historyValues.concat(new Array(forecastValues.length).fill(null)),
        borderColor: 'rgb(59, 130, 246)',
        backgroundColor: 'rgba(59, 130, 246, 0.15)',
        tension: 0.25,
        pointRadius: 2
      },
      {
        label: t('chart.forecastCost'),
        data: new Array(historyValues.length).fill(null).concat(forecastValues),
        borderColor: 'rgb(234, 179, 8)',
        backgroundColor: 'rgba(234, 179, 8, 0.15)',
        borderDash: [6, 4],
        tension: 0.25,
        pointRadius: 2
      }
    ]
  }
})

const chartOptions = computed(() => ({
  plugins: {
    tooltip: {
      callbacks: {
        label: (context: { parsed?: { y?: number }; dataset: { label?: string } }) => {
          const value = context.parsed?.y ?? 0
          return `${context.dataset.label}: ${formatCurrency(value)}`
        }
      }
    }
  }
}))
</script>
