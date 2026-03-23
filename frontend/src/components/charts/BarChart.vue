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
  CategoryScale,
  LinearScale,
  BarElement,
  BarController,
  Title,
  Tooltip,
  Legend,
  type ChartData,
  type ChartOptions
} from 'chart.js'
import { observeChartThemeChanges, resolveChartTheme } from '@/utils/chartTheme'

const props = withDefaults(defineProps<Props>(), {
  height: 300,
  stacked: true,
  options: undefined
})

ChartJS.register(
  CategoryScale,
  LinearScale,
  BarElement,
  BarController,
  Title,
  Tooltip,
  Legend
)

interface Props {
  data: ChartData<'bar'>
  options?: ChartOptions<'bar'>
  height?: number
  stacked?: boolean
}

const chartRef = ref<HTMLCanvasElement>()
let chart: ChartJS<'bar'> | null = null
let stopThemeObserver: (() => void) | null = null

const defaultOptions: ChartOptions<'bar'> = {
  responsive: true,
  maintainAspectRatio: false,
  interaction: {
    mode: 'index',
    intersect: false
  },
  scales: {
    x: {
      stacked: true,
      grid: {
        color: 'rgba(156, 163, 175, 0.1)'
      },
      ticks: {
        color: 'rgb(107, 114, 128)'
      }
    },
    y: {
      stacked: true,
      grid: {
        color: 'rgba(156, 163, 175, 0.1)'
      },
      ticks: {
        color: 'rgb(107, 114, 128)'
      }
    }
  },
  plugins: {
    legend: {
      position: 'top',
      labels: {
        color: 'rgb(107, 114, 128)',
        usePointStyle: true,
        padding: 16
      }
    },
    tooltip: {
      backgroundColor: 'rgb(31, 41, 55)',
      titleColor: 'rgb(243, 244, 246)',
      bodyColor: 'rgb(243, 244, 246)',
      borderColor: 'rgb(75, 85, 99)',
      borderWidth: 1
    }
  }
}

function buildChartOptions(): ChartOptions<'bar'> {
  const stackedOptions = props.stacked ? {
    scales: {
      x: { ...defaultOptions.scales?.x, stacked: true },
      y: { ...defaultOptions.scales?.y, stacked: true }
    }
  } : {
    scales: {
      x: { ...defaultOptions.scales?.x, stacked: false },
      y: { ...defaultOptions.scales?.y, stacked: false }
    }
  }

  return resolveChartTheme({
    ...defaultOptions,
    ...stackedOptions,
    ...props.options
  })
}

function createChart() {
  if (!chartRef.value) return

  chart = new ChartJS(chartRef.value, {
    type: 'bar',
    data: resolveChartTheme(props.data),
    options: buildChartOptions()
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
    chart.options = buildChartOptions()
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
    chart.options = buildChartOptions()
    chart.update('none')
  }
}, { deep: true })
</script>
