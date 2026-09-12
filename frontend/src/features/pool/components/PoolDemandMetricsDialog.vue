<template>
  <Dialog
    :model-value="modelValue"
    :no-padding="true"
    size="3xl"
    @update:model-value="emit('update:modelValue', $event)"
  >
    <template #header>
      <div class="border-b border-border px-4 py-4 sm:px-6">
        <div class="flex items-start gap-3">
          <div class="flex-1 min-w-0">
            <h3 class="text-lg font-semibold text-foreground leading-tight">
              自适应热池指标
            </h3>
            <p class="text-xs text-muted-foreground">
              {{ providerName || '当前 Provider' }}
            </p>
          </div>
          <Button
            variant="ghost"
            size="icon"
            class="h-8 w-8 shrink-0"
            title="关闭"
            @click="emit('update:modelValue', false)"
          >
            <X class="h-4 w-4" />
          </Button>
        </div>
      </div>
    </template>

    <div class="max-h-[calc(100dvh-13rem)] space-y-4 overflow-y-auto overscroll-contain pr-1 sm:max-h-[min(72vh,42rem)] sm:pr-2">
      <div class="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
        <div
          v-for="item in summaryCards"
          :key="item.label"
          class="rounded-lg border border-border/60 bg-card/70 px-3 py-3"
        >
          <div class="text-xs text-muted-foreground">
            {{ item.label }}
          </div>
          <div class="mt-2 text-xl font-semibold tabular-nums">
            {{ item.value }}
          </div>
          <div class="mt-1 text-[11px] text-muted-foreground">
            {{ item.hint }}
          </div>
        </div>
      </div>

      <section class="rounded-lg border border-border/60 bg-card/70 p-4">
        <div class="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
          <div>
            <h3 class="text-sm font-semibold">
              最近采样
            </h3>
            <p class="text-xs text-muted-foreground">
              {{ sampleWindowText }}
            </p>
          </div>
          <div class="flex flex-wrap items-center gap-2 text-[11px]">
            <span
              v-for="item in legendItems"
              :key="item.label"
              class="inline-flex items-center gap-1.5 text-muted-foreground"
            >
              <span
                class="h-2 w-2 rounded-full"
                :class="item.dotClass"
              />
              {{ item.label }}
            </span>
          </div>
        </div>

        <div
          v-if="samples.length === 0"
          class="mt-4 flex h-56 items-center justify-center rounded-lg border border-dashed border-border/70 bg-muted/20 text-sm text-muted-foreground"
        >
          暂无采样
        </div>

        <div
          v-else
          class="mt-4 h-56 rounded-lg border border-border/50 bg-background/60 p-3"
        >
          <svg
            class="h-full w-full overflow-visible"
            :viewBox="`0 0 ${chartWidth} ${chartHeight}`"
            preserveAspectRatio="none"
            role="img"
            aria-label="自适应热池趋势"
          >
            <line
              v-for="tick in yTicks"
              :key="tick.y"
              x1="0"
              :x2="chartWidth"
              :y1="tick.y"
              :y2="tick.y"
              class="stroke-border/70"
              stroke-width="1"
            />
            <polyline
              v-if="desiredHotLine"
              :points="desiredHotLine"
              fill="none"
              stroke="rgb(99 102 241)"
              stroke-width="2.4"
              stroke-linecap="round"
              stroke-linejoin="round"
              vector-effect="non-scaling-stroke"
            />
            <polyline
              v-if="hotLine"
              :points="hotLine"
              fill="none"
              stroke="rgb(16 185 129)"
              stroke-width="2.2"
              stroke-linecap="round"
              stroke-linejoin="round"
              vector-effect="non-scaling-stroke"
            />
            <polyline
              v-if="inFlightLine"
              :points="inFlightLine"
              fill="none"
              stroke="rgb(245 158 11)"
              stroke-width="2.2"
              stroke-linecap="round"
              stroke-linejoin="round"
              vector-effect="non-scaling-stroke"
            />
            <polyline
              v-if="emaLine"
              :points="emaLine"
              fill="none"
              stroke="rgb(14 165 233)"
              stroke-width="2.2"
              stroke-linecap="round"
              stroke-linejoin="round"
              vector-effect="non-scaling-stroke"
            />
            <circle
              v-for="burst in burstPoints"
              :key="`${burst.x}-${burst.y}`"
              :cx="burst.x"
              :cy="burst.y"
              r="3"
              fill="rgb(239 68 68)"
              vector-effect="non-scaling-stroke"
            />
          </svg>
        </div>

        <div class="mt-3 flex items-center justify-between gap-3 text-[11px] text-muted-foreground">
          <span>{{ firstSampleTime }}</span>
          <span>峰值 {{ maxChartValueText }}</span>
          <span>{{ lastSampleTime }}</span>
        </div>
      </section>
    </div>
  </Dialog>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { Button, Dialog } from '@/components/ui'
import { X } from 'lucide-vue-next'

export interface PoolDemandMetricSample {
  providerId: string
  sampledAt: number
  hotCount: number
  desiredHot: number
  inFlight: number
  emaInFlight: number
  burstPending: boolean
}

const props = defineProps<{
  modelValue: boolean
  providerName?: string | null
  samples: PoolDemandMetricSample[]
}>()

const emit = defineEmits<{
  'update:modelValue': [value: boolean]
}>()

const chartWidth = 640
const chartHeight = 220
const yTickRatios = [0, 0.25, 0.5, 0.75, 1]

const latest = computed(() => props.samples[props.samples.length - 1] ?? null)

const maxChartValue = computed(() => {
  const maxValue = props.samples.reduce((max, sample) => {
    return Math.max(
      max,
      sample.hotCount,
      sample.desiredHot,
      sample.inFlight,
      sample.emaInFlight,
    )
  }, 1)
  return Math.max(1, Math.ceil(maxValue))
})

const yTicks = computed(() => {
  return yTickRatios.map(ratio => ({
    y: chartHeight * ratio,
  }))
})

function formatMetric(value: number, fractionDigits = 0): string {
  if (!Number.isFinite(value) || value <= 0) {
    return fractionDigits > 0 ? '0.0' : '0'
  }
  return value.toFixed(fractionDigits)
}

function formatSampleTime(timestamp: number | undefined): string {
  if (!timestamp) return '--:--:--'
  const date = new Date(timestamp)
  const pad = (value: number) => String(value).padStart(2, '0')
  return `${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`
}

function buildLine(valueOf: (sample: PoolDemandMetricSample) => number): string {
  if (props.samples.length === 0) return ''
  const maxValue = maxChartValue.value
  const widthStep = props.samples.length > 1
    ? chartWidth / (props.samples.length - 1)
    : 0
  return props.samples
    .map((sample, index) => {
      const x = props.samples.length > 1 ? index * widthStep : chartWidth / 2
      const normalized = Math.max(0, Math.min(valueOf(sample), maxValue))
      const y = chartHeight - ((normalized / maxValue) * chartHeight)
      return `${x.toFixed(2)},${y.toFixed(2)}`
    })
    .join(' ')
}

const hotLine = computed(() => buildLine(sample => sample.hotCount))
const desiredHotLine = computed(() => buildLine(sample => sample.desiredHot))
const inFlightLine = computed(() => buildLine(sample => sample.inFlight))
const emaLine = computed(() => buildLine(sample => sample.emaInFlight))

const burstPoints = computed(() => {
  if (props.samples.length === 0) return []
  const widthStep = props.samples.length > 1
    ? chartWidth / (props.samples.length - 1)
    : 0
  const maxValue = maxChartValue.value
  return props.samples
    .map((sample, index) => {
      if (!sample.burstPending) return null
      const x = props.samples.length > 1 ? index * widthStep : chartWidth / 2
      const normalized = Math.max(0, Math.min(sample.desiredHot, maxValue))
      const y = chartHeight - ((normalized / maxValue) * chartHeight)
      return { x, y }
    })
    .filter((point): point is { x: number, y: number } => point !== null)
})

const summaryCards = computed(() => {
  const sample = latest.value
  return [
    {
      label: '热池',
      value: sample ? `${sample.hotCount} / ${sample.desiredHot}` : '0 / 0',
      hint: '当前 / 目标',
    },
    {
      label: 'in-flight',
      value: sample ? formatMetric(sample.inFlight) : '0',
      hint: '正在执行',
    },
    {
      label: 'EMA',
      value: sample ? formatMetric(sample.emaInFlight, 1) : '0.0',
      hint: '平滑热度',
    },
    {
      label: 'Burst',
      value: sample?.burstPending ? '补热中' : '空闲',
      hint: '异步补位',
    },
  ]
})

const legendItems = [
  { label: '目标', dotClass: 'bg-indigo-500' },
  { label: '热池', dotClass: 'bg-emerald-500' },
  { label: 'in-flight', dotClass: 'bg-amber-500' },
  { label: 'EMA', dotClass: 'bg-sky-500' },
  { label: 'Burst', dotClass: 'bg-red-500' },
]

const sampleWindowText = computed(() => {
  const count = props.samples.length
  if (count === 0) return '等待下一次采样'
  return `最近 ${count} 个采样点`
})

const firstSampleTime = computed(() => formatSampleTime(props.samples[0]?.sampledAt))
const lastSampleTime = computed(() => formatSampleTime(latest.value?.sampledAt))
const maxChartValueText = computed(() => formatMetric(maxChartValue.value))
</script>
