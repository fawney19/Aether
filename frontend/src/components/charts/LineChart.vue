<template>
  <div class="w-full h-full">
    <canvas ref="chartRef" />
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted, onUnmounted, watch, nextTick } from 'vue'
import { useI18n } from '@/i18n'
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
  type ChartData,
  type ChartOptions
} from 'chart.js'

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
  Legend
)

interface Props {
  data: ChartData<'line'>
  options?: ChartOptions<'line'>
  height?: number
}

const chartRef = ref<HTMLCanvasElement>()
const { locale } = useI18n()
let chart: ChartJS<'line'> | null = null

function buildChartOptions(): ChartOptions<'line'> {
  return {
    ...defaultOptions,
    locale: locale.value,
    ...props.options
  }
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
    data: props.data,
    options: buildChartOptions()
  })
}

function updateChart() {
  if (chart) {
    chart.data = props.data
    chart.update('none') // 禁用动画以提高性能
  }
}

onMounted(async () => {
  await nextTick()
  createChart()
})

onUnmounted(() => {
  if (chart) {
    chart.destroy()
    chart = null
  }
})

// 监听引用变化，避免深监听触发整图重算
watch(() => props.data, updateChart)
watch([() => props.options, locale], () => {
  if (chart) {
    chart.options = buildChartOptions()
    chart.update('none')
  }
})
</script>
