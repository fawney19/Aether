<template>
  <div
    class="grid gap-3"
    :class="secondaryVisible ? 'grid-cols-2' : 'grid-cols-1'"
  >
    <!-- 主限额 -->
    <ProviderQuotaProgressRow
      v-if="display?.primary_used_percent !== undefined"
      :label="primaryLabel"
      :used-percent="display?.primary_used_percent || 0"
      :remaining-percent="toCodexRemainingPercent(display?.primary_used_percent)"
      :meter-class="getQuotaRemainingClass(display?.primary_used_percent || 0)"
      :bar-class="getQuotaRemainingBarColor(display?.primary_used_percent || 0)"
      :reset-text="(display?.primary_reset_at || display?.primary_reset_seconds) && shouldStartCodexResetCountdown(display?.primary_used_percent || 0)
        ? getResetCountdownText(
          display?.primary_reset_at,
          display?.primary_reset_seconds,
          display?.updated_at,
          display?.primary_used_percent
        )
        : null"
      :footer-class="getResetCountdownClass(
        display?.primary_reset_at,
        display?.primary_reset_seconds,
        display?.updated_at,
        display?.primary_used_percent
      )"
    />
    <!-- 周限额（月与周并存时主位取月，周在此单独渲染） -->
    <ProviderQuotaProgressRow
      v-if="weeklyVisible"
      :label="legacyT('周限额')"
      :used-percent="display?.weekly_used_percent || 0"
      :remaining-percent="toCodexRemainingPercent(display?.weekly_used_percent)"
      :meter-class="getQuotaRemainingClass(display?.weekly_used_percent || 0)"
      :bar-class="getQuotaRemainingBarColor(display?.weekly_used_percent || 0)"
      :reset-text="(display?.weekly_reset_at || display?.weekly_reset_seconds) && shouldStartCodexResetCountdown(display?.weekly_used_percent || 0)
        ? getResetCountdownText(
          display?.weekly_reset_at,
          display?.weekly_reset_seconds,
          display?.updated_at,
          display?.weekly_used_percent
        )
        : null"
      :footer-class="getResetCountdownClass(
        display?.weekly_reset_at,
        display?.weekly_reset_seconds,
        display?.updated_at,
        display?.weekly_used_percent
      )"
    />
    <!-- 5H限额（Team/Plus/Enterprise 及自定义提供商的 5h 窗口） -->
    <ProviderQuotaProgressRow
      v-if="secondaryVisible"
      :label="legacyT('5H限额')"
      :used-percent="display?.secondary_used_percent || 0"
      :remaining-percent="toCodexRemainingPercent(display?.secondary_used_percent)"
      :meter-class="getQuotaRemainingClass(display?.secondary_used_percent || 0)"
      :bar-class="getQuotaRemainingBarColor(display?.secondary_used_percent || 0)"
      :reset-text="getCodexQuotaResetText(
        display?.secondary_reset_at,
        display?.secondary_reset_seconds,
        display?.updated_at,
        display?.secondary_used_percent
      )"
      :footer-class="getResetCountdownClass(
        display?.secondary_reset_at,
        display?.secondary_reset_seconds,
        display?.updated_at,
        display?.secondary_used_percent
      )"
    />
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from '@/i18n'
import { getCodexResetCountdown } from '@/composables/useCountdownTimer'
import type { CodexUpstreamMetadata } from '@/api/endpoints/types'
import ProviderQuotaProgressRow from './ProviderQuotaProgressRow.vue'

const props = withDefaults(defineProps<{
  display: CodexUpstreamMetadata | null
  primaryLabel: string
  /** Whether the 5h secondary row may render (still requires data). */
  showSecondary?: boolean
  /** Parent countdown tick so reset texts re-render every second. */
  tick?: number
}>(), {
  showSecondary: false,
  tick: 0,
})

const { legacyT } = useI18n()

const secondaryVisible = computed(
  () => props.showSecondary && props.display?.secondary_used_percent !== undefined,
)

// 周窗口与月窗口并存时才走这一行；主位本身就是周时不重复渲染。
const weeklyVisible = computed(
  () => props.display?.weekly_used_percent !== undefined
    && props.display?.primary_code !== 'weekly',
)

function toCodexRemainingPercent(usedPercent: number | null | undefined): number | null {
  const normalizedUsed = Number(usedPercent)
  if (!Number.isFinite(normalizedUsed)) return null
  const clampedUsed = Math.min(Math.max(normalizedUsed, 0), 100)
  return Math.max(100 - clampedUsed, 0)
}

function shouldStartCodexResetCountdown(usedPercent: number | null | undefined): boolean {
  const remainingPercent = toCodexRemainingPercent(usedPercent)
  if (remainingPercent == null) return true
  return remainingPercent < 100
}

function getQuotaRemainingClass(usedPercent: number): string {
  const remaining = 100 - usedPercent
  if (remaining <= 10) return 'text-red-600 dark:text-red-400'
  if (remaining <= 30) return 'text-yellow-600 dark:text-yellow-400'
  return 'text-green-600 dark:text-green-400'
}

function getQuotaRemainingBarColor(usedPercent: number): string {
  const remaining = 100 - usedPercent
  if (remaining <= 10) return 'bg-red-500 dark:bg-red-400'
  if (remaining <= 30) return 'bg-yellow-500 dark:bg-yellow-400'
  return 'bg-green-500 dark:bg-green-400'
}

function getResetCountdownText(
  resetAt: number | null | undefined,
  resetSecs: number | null | undefined,
  updatedAt: number | null | undefined,
  usedPercent: number | null | undefined
): string {
  const status = getCodexResetCountdown(
    resetAt,
    resetSecs,
    updatedAt,
    props.tick,
    toCodexRemainingPercent(usedPercent)
  )
  if (!status) return ''
  return status.isExpired ? legacyT(status.text) : `${legacyT(status.text)} ${legacyT('后重置')}`
}

function getCodexQuotaResetText(
  resetAt: number | null | undefined,
  resetSecs: number | null | undefined,
  updatedAt: number | null | undefined,
  usedPercent: number | null | undefined
): string {
  if (resetAt || resetSecs) {
    return getResetCountdownText(resetAt, resetSecs, updatedAt, usedPercent)
  }
  return legacyT('已重置')
}

function getResetCountdownClass(
  resetAt: number | null | undefined,
  resetSecs: number | null | undefined,
  updatedAt: number | null | undefined,
  usedPercent: number | null | undefined
): string {
  const status = getCodexResetCountdown(
    resetAt,
    resetSecs,
    updatedAt,
    props.tick,
    toCodexRemainingPercent(usedPercent)
  )
  if (!status || status.isExpired) return 'text-muted-foreground/70'
  if (status.isCritical) return 'text-destructive font-medium animate-pulse'
  if (status.isUrgent) return 'text-amber-500 dark:text-amber-400'
  return 'text-muted-foreground/70'
}
</script>
