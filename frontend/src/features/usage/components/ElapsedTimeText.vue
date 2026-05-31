<template>
  <span class="tabular-nums">{{ displayText }}</span>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { useActiveElapsedDisplayNowMs } from '../composables/useActiveElapsedDisplayClock'

const props = withDefaults(defineProps<{
  createdAt?: string | null
  status?: string | null
  responseTimeMs?: number | null
  displayNowMs?: number | null
  precision?: number
}>(), {
  createdAt: null,
  status: null,
  responseTimeMs: null,
  displayNowMs: null,
  precision: 2,
})

const precision = computed(() => Math.max(0, props.precision))
const isActive = computed(() => props.status === 'pending' || props.status === 'streaming')
const injectedDisplayNowMs = useActiveElapsedDisplayNowMs()

function parseCreatedAtMs(value: string | null | undefined): number {
  if (!value) return Number.NaN
  // 后端有时返回无时区时间，按 UTC 解析，和列表时间显示逻辑保持一致
  const normalized = /(?:Z|[+-]\d{2}:\d{2})$/i.test(value) ? value : `${value}Z`
  return new Date(normalized).getTime()
}

const displayText = computed(() => {
  if (!isActive.value) {
    if (props.responseTimeMs == null) return '-'
    return `${(props.responseTimeMs / 1000).toFixed(precision.value)}s`
  }

  if (!props.createdAt) return '-'

  const createdAtMs = parseCreatedAtMs(props.createdAt)
  if (Number.isNaN(createdAtMs)) return '-'

  // 活跃请求里的 response_time_ms 可能只是首字或中间值；终态才使用后端最终耗时。
  const injectedNowMs = injectedDisplayNowMs?.value
  const nowMs = typeof props.displayNowMs === 'number' && Number.isFinite(props.displayNowMs)
    ? props.displayNowMs
    : typeof injectedNowMs === 'number' && Number.isFinite(injectedNowMs)
      ? injectedNowMs
    : Date.now()
  const elapsedMs = Math.max(0, nowMs - createdAtMs)
  return `${(elapsedMs / 1000).toFixed(precision.value)}s`
})
</script>
