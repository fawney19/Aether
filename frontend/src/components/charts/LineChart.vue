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
  PointElement,
  LineElement,
  LineController,
  Title,
  Tooltip,
  Legend,
  Filler,
  type ChartData,
  type ChartOptions
} from 'chart.js'
import { observeChartThemeChanges, resolveChartTheme } from '@/utils/chartTheme'

const props = withDefaults(defineProps<Props>(), {
  height: 300,
  options: undefined
})

// 注册 Chart.js 组件
ChartJS.register(
  CategoryScale,
  LinearScale,
  PointElement,
  LineElement,
  LineController,
  Title,
  Tooltip,
  Legend,
  Filler
)

interface Props {
  data: ChartData<'line'>
  options?: ChartOptions<'line'>
  height?: number
}

const chartRef = ref<HTMLCanvasElement>()
let chart: ChartJS<'line'> | null = null
let stopThemeObserver: (() => void) | null = null

function buildChartOptions(): ChartOptions<'line'> {
  return resolveChartTheme({
    ...defaultOptions,
    ...props.options
  })
}

function buildChartData(): ChartData<'line'> {
  return resolveChartTheme(props.data)
}

const defaultOptions: ChartOptions<'line'> = {
  responsive: true,
  maintainAspectRatio: false,
  scales: {
    x: {
      grid: {
        color: 'rgba(156, 163, 175, 0.1)' // gray-400 with opacity
      },
      ticks: {
        color: 'rgb(107, 114, 128)' // gray-500
      }
    },
    y: {
      grid: {
        color: 'rgba(156, 163, 175, 0.1)' // gray-400 with opacity
      },
      ticks: {
        color: 'rgb(107, 114, 128)' // gray-500
      }
    }
  },
  plugins: {
    legend: {
      labels: {
        color: 'rgb(107, 114, 128)' // gray-500
      }
    },
    tooltip: {
      backgroundColor: 'rgb(31, 41, 55)', // gray-800
      titleColor: 'rgb(243, 244, 246)', // gray-100
      bodyColor: 'rgb(243, 244, 246)', // gray-100
      borderColor: 'rgb(75, 85, 99)', // gray-600
      borderWidth: 1
    }
  }
}

function createChart() {
  if (!chartRef.value) return

  chart = new ChartJS(chartRef.value, {
    type: 'line',
    data: buildChartData(),
    options: buildChartOptions()
  })
}

function updateChart() {
  if (chart) {
    chart.data = buildChartData()
    chart.update('none') // 禁用动画以提高性能
  }
}

onMounted(async () => {
  await nextTick()
  createChart()
  stopThemeObserver = observeChartThemeChanges(() => {
    if (!chart) return
    chart.data = buildChartData()
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

// 监听引用变化，避免深监听触发整图重算
watch(() => props.data, updateChart)
watch(() => props.options, () => {
  if (chart) {
    chart.options = buildChartOptions()
    chart.update('none')
  }
})
</script>
