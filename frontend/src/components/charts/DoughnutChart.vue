<template>
  <div class="w-full h-full">
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
}

const props = withDefaults(defineProps<Props>(), {
  height: 300,
  options: undefined
})

const chartRef = ref<HTMLCanvasElement>()
let chart: ChartJS<'doughnut'> | null = null

const defaultOptions: ChartOptions<'doughnut'> = {
  responsive: true,
  maintainAspectRatio: false,
  cutout: '60%',
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

function createChart() {
  if (!chartRef.value) return

  chart = new ChartJS(chartRef.value, {
    type: 'doughnut',
    data: props.data,
    options: {
      ...defaultOptions,
      ...props.options
    }
  })
}

function updateChart() {
  if (chart) {
    chart.data = props.data
    chart.update('none')
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

watch(() => props.data, updateChart, { deep: true })
watch(() => props.options, () => {
  if (chart) {
    chart.options = { ...defaultOptions, ...props.options }
    chart.update()
  }
}, { deep: true })
</script>
