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
      class="h-[260px]"
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
import LineChart from '@/components/charts/LineChart.vue'
import { LoadingState } from '@/components/common'
import type { PercentileItem } from '@/api/admin'

interface Props {
  title: string
  subtitle?: string
  series: PercentileItem[]
  mode: 'response' | 'ttfb'
  loading?: boolean
}

const props = withDefaults(defineProps<Props>(), {
  subtitle: undefined,
  loading: false
})

const labels = computed(() => props.series.map(item => item.date))

// 毫秒转秒
function msToSeconds(ms: number | null | undefined): number | null {
  if (ms == null) return null
  return ms / 1000
}

const chartData = computed(() => {
  const p50Key = props.mode === 'response' ? 'p50_response_time_ms' : 'p50_first_byte_time_ms'
  const p90Key = props.mode === 'response' ? 'p90_response_time_ms' : 'p90_first_byte_time_ms'
  const p99Key = props.mode === 'response' ? 'p99_response_time_ms' : 'p99_first_byte_time_ms'

  return {
    labels: labels.value,
    datasets: [
      {
        label: 'P50',
        data: props.series.map(item => msToSeconds(item[p50Key])),
        borderColor: 'rgb(59, 130, 246)',
        tension: 0.25,
        pointRadius: 2
      },
      {
        label: 'P90',
        data: props.series.map(item => msToSeconds(item[p90Key])),
        borderColor: 'rgb(234, 179, 8)',
        tension: 0.25,
        pointRadius: 2
      },
      {
        label: 'P99',
        data: props.series.map(item => msToSeconds(item[p99Key])),
        borderColor: 'rgb(239, 68, 68)',
        tension: 0.25,
        pointRadius: 2
      }
    ]
  }
})

const chartOptions = computed(() => ({
  scales: {
    y: {
      ticks: {
        callback: (value: string | number) => `${value}s`
      }
    }
  }
}))
</script>
