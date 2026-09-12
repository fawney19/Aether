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
  BarElement,
  BarController,
  Title,
  Tooltip,
  Legend,
  type ChartData,
  type ChartOptions
} from 'chart.js'

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
const { locale } = useI18n()
let chart: ChartJS<'bar'> | null = null

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

  return {
    ...defaultOptions,
    ...stackedOptions,
    locale: locale.value,
    ...props.options
  }
}

function createChart() {
  if (!chartRef.value) return

  chart = new ChartJS(chartRef.value, {
    type: 'bar',
    data: props.data,
    options: buildChartOptions()
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
watch([() => props.options, () => props.stacked, locale], () => {
  if (chart) {
    chart.options = buildChartOptions()
    chart.update()
  }
}, { deep: true })
</script>
