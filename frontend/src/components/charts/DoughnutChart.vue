<template>
  <div
    class="w-full"
    :style="{ height: `${props.height}px` }"
  >
    <canvas ref="chartRef" />
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted, onUnmounted, watch, nextTick } from 'vue'
import {
  Chart as ChartJS,
  ArcElement,
  DoughnutController,
  Title,
  Tooltip,
  Legend,
  type ChartData,
  type ChartOptions
} from 'chart.js'
import { observeChartThemeChanges, resolveChartTheme } from '@/utils/chartTheme'

const props = withDefaults(defineProps<Props>(), {
  height: 300,
  options: undefined,
  showLegend: true,
})

ChartJS.register(
  ArcElement,
  DoughnutController,
  Title,
  Tooltip,
  Legend
)

interface Props {
  data: ChartData<'doughnut'>
  options?: ChartOptions<'doughnut'>
  height?: number
  showLegend?: boolean
}

const chartRef = ref<HTMLCanvasElement>()
let chart: ChartJS<'doughnut'> | null = null
let stopThemeObserver: (() => void) | null = null

const defaultOptions: ChartOptions<'doughnut'> = {
  responsive: true,
  maintainAspectRatio: false,
  cutout: '60%',
  events: ['mousemove', 'mouseout', 'click', 'touchstart', 'touchmove'],
  interaction: {
    mode: 'nearest',
    intersect: true,
  },
  plugins: {
    legend: {
      position: 'right',
      labels: {
        color: 'rgb(107, 114, 128)',
        usePointStyle: true,
        padding: 16,
        font: { size: 11 }
      }
    },
    tooltip: {
      backgroundColor: 'rgb(31, 41, 55)',
      titleColor: 'rgb(243, 244, 246)',
      bodyColor: 'rgb(243, 244, 246)',
      borderColor: 'rgb(75, 85, 99)',
      borderWidth: 1,
      callbacks: {
        label: (context) => {
          const value = context.raw as number
          const total = (context.dataset.data as number[]).reduce((a, b) => a + b, 0)
          const percentage = total > 0 ? ((value / total) * 100).toFixed(1) : '0'
          return `${context.label}: $${value.toFixed(4)} (${percentage}%)`
        }
      }
    }
  }
}

function resolvedOptions(): ChartOptions<'doughnut'> {
  const incomingOptions = props.options ?? {}
  const incomingPlugins = (incomingOptions.plugins ?? {}) as Record<string, unknown>
  const incomingLegend = (incomingPlugins.legend ?? {}) as Record<string, unknown>
  const incomingTooltip = (incomingPlugins.tooltip ?? {}) as Record<string, unknown>
  const defaultPlugins = (defaultOptions.plugins ?? {}) as Record<string, unknown>
  const defaultLegend = (defaultPlugins.legend ?? {}) as Record<string, unknown>
  const defaultTooltip = (defaultPlugins.tooltip ?? {}) as Record<string, unknown>

  return resolveChartTheme({
    ...defaultOptions,
    ...incomingOptions,
    interaction: {
      ...defaultOptions.interaction,
      ...incomingOptions.interaction,
    },
    plugins: {
      ...defaultPlugins,
      ...incomingPlugins,
      legend: {
        ...defaultLegend,
        ...incomingLegend,
        display: incomingLegend.display ?? props.showLegend,
        labels: {
          ...(defaultLegend.labels as Record<string, unknown> | undefined),
          ...(incomingLegend.labels as Record<string, unknown> | undefined),
        },
      },
      tooltip: {
        ...defaultTooltip,
        ...incomingTooltip,
        callbacks: {
          ...(defaultTooltip.callbacks as Record<string, unknown> | undefined),
          ...(incomingTooltip.callbacks as Record<string, unknown> | undefined),
        },
      },
    },
  })
}

function createChart() {
  if (!chartRef.value) return

  chart = new ChartJS(chartRef.value, {
    type: 'doughnut',
    data: resolveChartTheme(props.data),
    options: resolvedOptions(),
  })
}

function updateChart() {
  if (chart) {
    chart.data = resolveChartTheme(props.data)
    chart.update('none')
  }
}

onMounted(async () => {
  await nextTick()
  createChart()
  stopThemeObserver = observeChartThemeChanges(() => {
    if (!chart) return
    chart.data = resolveChartTheme(props.data)
    chart.options = resolvedOptions()
    chart.update('none')
  })
})

onUnmounted(() => {
  stopThemeObserver?.()
  stopThemeObserver = null
  if (chart) {
    chart.destroy()
    chart = null
  }
})

watch(() => props.data, updateChart, { deep: true })
watch(() => props.options, () => {
  if (chart) {
    chart.options = resolvedOptions()
    chart.update('none')
  }
}, { deep: true })

watch(() => props.showLegend, () => {
  if (chart) {
    chart.options = resolvedOptions()
    chart.update('none')
  }
})
</script>
