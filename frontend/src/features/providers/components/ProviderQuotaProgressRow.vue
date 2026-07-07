<template>
  <div data-testid="provider-quota-progress-row">
    <div class="flex items-center justify-between text-[10px] mb-0.5">
      <span
        class="text-muted-foreground truncate mr-2 min-w-0 flex-1"
        :title="title || label"
      >
        {{ label }}
      </span>
      <span
        :class="meterClass"
        data-testid="provider-quota-progress-meter"
      >
        {{ normalizedRemainingPercent.toFixed(1) }}%
      </span>
    </div>
    <div class="relative w-full h-1.5 bg-border rounded-full overflow-hidden">
      <div
        class="absolute left-0 top-0 h-full transition-all duration-300"
        :class="barClass"
        :style="{ width: `${normalizedRemainingPercent}%` }"
        data-testid="provider-quota-progress-bar"
      />
    </div>
    <slot name="footer">
      <div
        v-if="resetText"
        class="text-[9px] text-muted-foreground/70 mt-0.5"
        :class="footerClass"
        data-testid="provider-quota-progress-reset"
      >
        {{ resetText }}
      </div>
    </slot>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue'

const props = withDefaults(defineProps<{
  label: string
  usedPercent?: number | null
  remainingPercent?: number | null
  title?: string | null
  meterClass?: string
  barClass?: string
  footerClass?: string
  resetText?: string | null
}>(), {
  usedPercent: null,
  remainingPercent: null,
  title: null,
  meterClass: '',
  barClass: '',
  footerClass: '',
  resetText: null,
})

function normalizePercent(value: number | null | undefined): number | null {
  const numeric = Number(value)
  if (!Number.isFinite(numeric)) return null
  return Math.min(Math.max(numeric, 0), 100)
}

const normalizedRemainingPercent = computed(() => {
  const remaining = normalizePercent(props.remainingPercent)
  if (remaining !== null) return remaining

  const used = normalizePercent(props.usedPercent)
  if (used !== null) return Math.max(100 - used, 0)

  return 0
})
</script>
