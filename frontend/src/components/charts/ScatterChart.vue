<template>
  <div class="w-full h-full relative">
    <canvas ref="chartRef" />
    <div
      v-if="crosshairStats"
      class="absolute top-2 right-2 bg-gray-800/90 text-gray-100 px-3 py-2 rounded-lg text-sm shadow-lg border border-gray-600"
    >
      <div class="font-medium text-yellow-400">
        Y = {{ crosshairStats.yValue.toFixed(1) }} 分钟
      </div>
      <!-- 单个 dataset 时显示简单统计 -->
      <div
        v-if="crosshairStats.datasets.length === 1"
        class="mt-1"
      >
        <span class="text-green-400">{{ crosshairStats.datasets[0].belowCount }}</span> / {{ crosshairStats.datasets[0].totalCount }} 点在横线以下
        <span class="ml-2 text-blue-400">({{ crosshairStats.datasets[0].belowPercent.toFixed(1) }}%)</span>
      </div>
      <!-- 多个 dataset 时按模型分别显示 -->
      <div
        v-else
        class="mt-1 space-y-0.5"
      >
        <div
          v-for="ds in crosshairStats.datasets"
          :key="ds.label"
          class="flex items-center gap-2"
        >
          <div
            class="w-2 h-2 rounded-full flex-shrink-0"
            :style="{ backgroundColor: ds.color }"
          />
          <span class="text-gray-300 truncate max-w-[80px]">{{ ds.label }}:</span>
          <span class="text-green-400">{{ ds.belowCount }}</span>/<span class="text-gray-400">{{ ds.totalCount }}</span>
          <span class="text-blue-400">({{ ds.belowPercent.toFixed(0) }}%)</span>
        </div>
        <!-- 总计 -->
        <div class="flex items-center gap-2 pt-1 border-t border-gray-600 mt-1">
          <span class="text-gray-300">总计:</span>
          <span class="text-green-400">{{ crosshairStats.totalBelowCount }}</span>/<span class="text-gray-400">{{ crosshairStats.totalCount }}</span>
          <span class="text-blue-400">({{ crosshairStats.totalBelowPercent.toFixed(1) }}%)</span>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted, onUnmounted, watch, nextTick, computed } from 'vue'
import {
  Chart as ChartJS,
  LinearScale,
  PointElement,
  ScatterController,
  TimeScale,
  Title,
  Tooltip,
  Legend,
  type ChartData,
  type ChartOptions,
  type Plugin,
  type Scale
} from 'chart.js'
import 'chartjs-adapter-date-fns'

const props = withDefaults(defineProps<Props>(), {
  height: 300,
  options: undefined
})

ChartJS.register(
  LinearScale,
  PointElement,
  ScatterController,
  TimeScale,
  Title,
  Tooltip,
  Legend
)

interface Props {
  data: ChartData<'scatter'>
  options?: ChartOptions<'scatter'>
  height?: number
}

interface DatasetStats {
  label: string
  color: string
  belowCount: number
  totalCount: number
  belowPercent: number
}

interface CrosshairStats {
  yValue: number
  datasets: DatasetStats[]
  totalBelowCount: number
  totalCount: number
  totalBelowPercent: number
}

const chartRef = ref<HTMLCanvasElement>()
let chart: ChartJS<'scatter'> | null = null

const crosshairY = ref<number | null>(null)

const crosshairStats = computed<CrosshairStats | null>(() => {
  if (crosshairY.value === null || !props.data.datasets) return null

  const datasetStats: DatasetStats[] = []
  let totalBelowCount = 0
  let totalCount = 0

  for (const dataset of props.data.datasets) {
    if (!dataset.data) continue

    let belowCount = 0
    let dsTotal = 0

    for (const point of dataset.data) {
      const p = point as { x: string; y: number }
      if (typeof p.y === 'number') {
        dsTotal++
        totalCount++
        if (p.y <= crosshairY.value) {
          belowCount++
          totalBelowCount++
        }
      }
    }

    if (dsTotal > 0) {
      datasetStats.push({
        label: dataset.label || 'Unknown',
        color: (dataset.backgroundColor as string) || 'rgba(59, 130, 246, 0.7)',
        belowCount,
        totalCount: dsTotal,
        belowPercent: (belowCount / dsTotal) * 100
      })
    }
  }

  if (totalCount === 0) return null

  return {
    yValue: crosshairY.value,
    datasets: datasetStats,
    totalBelowCount,
    totalCount,
    totalBelowPercent: (totalBelowCount / totalCount) * 100
  }
})

// 旧版插件（保留作为参考但不再使用，使用下方的 crosshairPluginWithTransform 代替）
// const crosshairPlugin: Plugin<'scatter'> = { ... }

// 自定义非线性 Y 轴转换函数
// 0-10 分钟占据 70% 的空间，10-120 分钟占据 30% 的空间
const BREAKPOINT = 10  // 分界点：10 分钟
const LOWER_RATIO = 0.7  // 0-10 分钟占 70% 空间

// 将实际值转换为显示值（用于绘图）
function toDisplayValue(realValue: number): number {
  if (realValue <= BREAKPOINT) {
    // 0-10 分钟线性映射到 0-70
    return realValue * (LOWER_RATIO * 100 / BREAKPOINT)
  } else {
    // 10-120 分钟映射到 70-100
    const upperRange = 120 - BREAKPOINT
    const displayUpperRange = (1 - LOWER_RATIO) * 100
    return LOWER_RATIO * 100 + ((realValue - BREAKPOINT) / upperRange) * displayUpperRange
  }
}

// 将显示值转换回实际值（用于读取鼠标位置）
function toRealValue(displayValue: number): number {
  const breakpointDisplay = LOWER_RATIO * 100
  if (displayValue <= breakpointDisplay) {
    return displayValue / (LOWER_RATIO * 100 / BREAKPOINT)
  } else {
    const upperRange = 120 - BREAKPOINT
    const displayUpperRange = (1 - LOWER_RATIO) * 100
    return BREAKPOINT + ((displayValue - breakpointDisplay) / displayUpperRange) * upperRange
  }
}

// 转换数据点的 Y 值
function transformData(data: ChartData<'scatter'>): ChartData<'scatter'> {
  return {
    ...data,
    datasets: data.datasets.map(dataset => ({
      ...dataset,
      data: (dataset.data as Array<{ x: string; y: number }>).map(point => ({
        ...point,
        y: toDisplayValue(point.y),
        _originalY: point.y  // 保存原始值用于 tooltip
      }))
    }))
  }
}

const defaultOptions: ChartOptions<'scatter'> = {
  responsive: true,
  maintainAspectRatio: false,
  interaction: {
    mode: 'nearest',
    intersect: true
  },
  scales: {
    x: {
      type: 'time',
      time: {
        unit: 'hour',
        displayFormats: {
          hour: 'MM-dd HH:mm'
        }
      },
      grid: {
        color: 'rgba(156, 163, 175, 0.1)'
      },
      ticks: {
        color: 'rgb(107, 114, 128)',
        maxRotation: 45
      },
      title: {
        display: true,
        text: '时间',
        color: 'rgb(107, 114, 128)'
      }
    },
    y: {
      type: 'linear',
      min: 0,
      max: 100,  // 显示值范围 0-100
      grid: {
        color: 'rgba(156, 163, 175, 0.1)'
      },
      ticks: {
        color: 'rgb(107, 114, 128)',
        // 自定义刻度值：在实际值 0, 2, 5, 10, 30, 60, 120 处显示
        callback(this: Scale, tickValue: string | number) {
          const displayVal = Number(tickValue)
          // 只在特定的显示位置显示刻度
          const targetTicks = [0, 2, 5, 10, 30, 60, 120]
          for (const target of targetTicks) {
            const targetDisplay = toDisplayValue(target)
            if (Math.abs(displayVal - targetDisplay) < 1) {
              return `${target}`
            }
          }
          return ''
        },
        stepSize: 5,  // 显示值的步长
        autoSkip: false
      },
      title: {
        display: true,
        text: '间隔 (分钟)',
        color: 'rgb(107, 114, 128)'
      },
      afterBuildTicks(scale: Scale) {
        // 在特定实际值处设置刻度
        const targetTicks = [0, 2, 5, 10, 30, 60, 120]
        scale.ticks = targetTicks.map(val => ({
          value: toDisplayValue(val),
          label: `${val}`
        }))
      }
    }
  },
  plugins: {
    legend: {
      display: false
    },
    tooltip: {
      backgroundColor: 'rgb(31, 41, 55)',
      titleColor: 'rgb(243, 244, 246)',
      bodyColor: 'rgb(243, 244, 246)',
      borderColor: 'rgb(75, 85, 99)',
      borderWidth: 1,
      callbacks: {
        label: (context) => {
          const point = context.raw as { x: string; y: number; _originalY?: number }
          const realY = point._originalY ?? toRealValue(point.y)
          return `间隔: ${realY.toFixed(1)} 分钟`
        }
      }
    }
  },
  onHover: (event, _elements, chartInstance) => {
    const canvas = chartInstance.canvas
    if (!canvas) return

    const rect = canvas.getBoundingClientRect()
    const mouseY = (event.native as MouseEvent)?.clientY

    if (mouseY === undefined) {
      crosshairY.value = null
      return
    }

    const { chartArea, scales } = chartInstance
    const yScale = scales.y

    if (!chartArea || !yScale) return

    const relativeY = mouseY - rect.top

    if (relativeY < chartArea.top || relativeY > chartArea.bottom) {
      crosshairY.value = null
    } else {
      const displayValue = yScale.getValueForPixel(relativeY)
      // 转换回实际值
      crosshairY.value = displayValue !== undefined ? toRealValue(displayValue) : null
    }

    chartInstance.draw()
  }
}

// 修改 crosshairPlugin 使用显示值
const crosshairPluginWithTransform: Plugin<'scatter'> = {
  id: 'crosshairLine',
  afterDraw: (chartInstance) => {
    if (crosshairY.value === null) return

    const { ctx, chartArea, scales } = chartInstance
    const yScale = scales.y
    if (!yScale || !chartArea) return

    // 将实际值转换为显示值再获取像素位置
    const displayValue = toDisplayValue(crosshairY.value)
    const yPixel = yScale.getPixelForValue(displayValue)

    if (yPixel < chartArea.top || yPixel > chartArea.bottom) return

    ctx.save()
    ctx.beginPath()
    ctx.moveTo(chartArea.left, yPixel)
    ctx.lineTo(chartArea.right, yPixel)
    ctx.strokeStyle = 'rgba(250, 204, 21, 0.8)'
    ctx.lineWidth = 2
    ctx.setLineDash([6, 4])
    ctx.stroke()
    ctx.restore()
  }
}

function handleMouseLeave() {
  crosshairY.value = null
  if (chart) {
    chart.draw()
  }
}

function createChart() {
  if (!chartRef.value) return

  // 转换数据
  const transformedData = transformData(props.data)

  chart = new ChartJS(chartRef.value, {
    type: 'scatter',
    data: transformedData,
    options: {
      ...defaultOptions,
      ...props.options
    },
    plugins: [crosshairPluginWithTransform]
  })

  chartRef.value.addEventListener('mouseleave', handleMouseLeave)
}

function updateChart() {
  if (chart) {
    chart.data = transformData(props.data)
    chart.update('none')
  }
}

onMounted(async () => {
  await nextTick()
  createChart()
})

onUnmounted(() => {
  if (chartRef.value) {
    chartRef.value.removeEventListener('mouseleave', handleMouseLeave)
  }
  if (chart) {
    chart.destroy()
    chart = null
  }
})

watch(() => props.data, updateChart, { deep: true })
watch(() => props.options, () => {
  if (chart) {
    chart.options = {
      ...defaultOptions,
      ...props.options
    }
    chart.update()
  }
}, { deep: true })
</script>
