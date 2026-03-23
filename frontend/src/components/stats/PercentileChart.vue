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
        v-if="summaryMetrics.length > 0 && !unavailable"
        class="flex flex-wrap gap-2"
      >
        <div
          v-for="metric in summaryMetrics"
          :key="metric.label"
          class="rounded-full border px-2.5 py-1 text-[10px] font-medium"
          :class="metric.className"
        >
          {{ metric.label }} {{ metric.value }}
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
      暂无延迟数据
    </div>
    <div
      v-else
      class="rounded-2xl border border-border/60 bg-background p-3"
    >
      <LineChart
        :data="chartData"
        :options="chartOptions"
        :height="248"
      />
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import type { ChartData, ChartOptions } from 'chart.js'
import LineChart from '@/components/charts/LineChart.vue'
import { LoadingState } from '@/components/common'

interface PercentileItem {
  date: string
  p50_response_time_ms?: number | null
  p90_response_time_ms?: number | null
  p99_response_time_ms?: number | null
  p50_first_byte_time_ms?: number | null
  p90_first_byte_time_ms?: number | null
  p99_first_byte_time_ms?: number | null
}

interface Props {
  title: string
  subtitle?: string
  series: PercentileItem[]
  mode: 'response' | 'ttfb'
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

function msToSeconds(ms: number | null | undefined): number | null {
  if (ms == null) return null
  return Number((ms / 1000).toFixed(3))
}

function getPercentileValue(item: PercentileItem, percentile: '50' | '90' | '99') {
  if (props.mode === 'response') {
    if (percentile === '50') return msToSeconds(item.p50_response_time_ms)
    if (percentile === '90') return msToSeconds(item.p90_response_time_ms)
    return msToSeconds(item.p99_response_time_ms)
  }

  if (percentile === '50') return msToSeconds(item.p50_first_byte_time_ms)
  if (percentile === '90') return msToSeconds(item.p90_first_byte_time_ms)
  return msToSeconds(item.p99_first_byte_time_ms)
}

function getLatestMetricValue(percentile: '50' | '90' | '99') {
  for (let index = props.series.length - 1; index >= 0; index -= 1) {
    const value = getPercentileValue(props.series[index], percentile)
    if (value != null) return value
  }
  return null
}

function formatSeconds(value: number | null) {
  if (value == null) return '--'
  if (value < 1) return `${Math.round(value * 1000)}ms`
  return `${value.toFixed(2)}s`
}

const labels = computed(() => props.series.map(item => item.date?.slice(5) ?? ''))
const hasData = computed(() => props.series.some(item => (
  getPercentileValue(item, '50') != null
  || getPercentileValue(item, '90') != null
  || getPercentileValue(item, '99') != null
)))

const summaryMetrics = computed(() => ([
  {
    label: 'P50',
    value: formatSeconds(getLatestMetricValue('50')),
    className: 'border-sky-500/20 bg-sky-500/8 text-sky-700 dark:text-sky-300',
  },
  {
    label: 'P90',
    value: formatSeconds(getLatestMetricValue('90')),
    className: 'border-amber-500/20 bg-amber-500/8 text-amber-700 dark:text-amber-300',
  },
  {
    label: 'P99',
    value: formatSeconds(getLatestMetricValue('99')),
    className: 'border-rose-500/20 bg-rose-500/8 text-rose-700 dark:text-rose-300',
  },
]))

const chartData = computed<ChartData<'line'>>(() => ({
  labels: labels.value,
  datasets: [
    {
      label: 'P50',
      data: props.series.map(item => getPercentileValue(item, '50')),
      borderColor: 'rgb(70, 136, 240)',
      backgroundColor: 'rgba(70, 136, 240, 0.10)',
      borderWidth: 2.4,
      fill: false,
      tension: 0.32,
      pointRadius: 0,
      pointHoverRadius: 4,
      pointHitRadius: 14,
    },
    {
      label: 'P90',
      data: props.series.map(item => getPercentileValue(item, '90')),
      borderColor: 'rgb(223, 166, 55)',
      backgroundColor: 'rgba(223, 166, 55, 0.08)',
      borderWidth: 2.1,
      fill: false,
      tension: 0.3,
      pointRadius: 0,
      pointHoverRadius: 4,
      pointHitRadius: 14,
    },
    {
      label: 'P99',
      data: props.series.map(item => getPercentileValue(item, '99')),
      borderColor: 'rgb(214, 92, 111)',
      backgroundColor: 'rgba(214, 92, 111, 0.08)',
      borderWidth: 2,
      borderDash: [6, 4],
      fill: false,
      tension: 0.28,
      pointRadius: 0,
      pointHoverRadius: 4,
      pointHitRadius: 14,
    },
  ],
}))

const chartOptions = computed<ChartOptions<'line'>>(() => ({
  responsive: true,
  maintainAspectRatio: false,
  interaction: {
    mode: 'index',
    intersect: false,
  },
  plugins: {
    legend: {
      position: 'bottom',
      labels: {
        usePointStyle: true,
        boxWidth: 10,
        padding: 14,
      },
    },
    tooltip: {
      callbacks: {
        title: items => props.series[items[0]?.dataIndex ?? -1]?.date ?? '',
        label: context => `${context.dataset.label}: ${formatSeconds(Number(context.raw ?? 0))}`,
      },
    },
  },
  scales: {
    x: {
      grid: {
        color: 'rgba(148, 163, 184, 0.06)',
      },
      ticks: {
        color: 'rgb(100, 116, 139)',
        maxRotation: 0,
      },
    },
    y: {
      beginAtZero: true,
      grid: {
        color: 'rgba(148, 163, 184, 0.07)',
      },
      ticks: {
        color: 'rgb(100, 116, 139)',
        callback: value => formatSeconds(Number(value)),
      },
    },
  },
}))
</script>
