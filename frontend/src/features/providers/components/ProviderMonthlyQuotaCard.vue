<template>
  <Card
    v-if="remoteQuotaEnabled || remoteQuotaGroup || quota > 0"
    class="p-4"
    data-testid="provider-monthly-quota-card"
  >
    <div class="space-y-3">
      <div class="flex flex-wrap items-center justify-between gap-2">
        <div class="min-w-0">
          <h3 class="text-sm font-semibold">
            {{ legacyT('订阅配额') }}
          </h3>
          <p
            v-if="remoteQuotaGroup"
            class="mt-0.5 truncate text-xs text-muted-foreground"
          >
            {{ remoteQuotaGroup.group_name || legacyT('未命名套餐') }}
          </p>
        </div>
        <Badge
          v-if="!remoteQuotaEnabled && !remoteQuotaGroup"
          variant="secondary"
          class="text-xs"
          data-testid="provider-monthly-quota-percent"
        >
          {{ usedPercent.toFixed(1) }}%
        </Badge>
      </div>

      <template v-if="remoteQuotaGroup">
        <div
          v-if="hasLocalEffectiveQuota"
          class="rounded-lg border border-indigo-500/20 bg-indigo-500/5 p-3"
          data-testid="provider-remote-quota-local-effective"
        >
          <div class="flex flex-wrap items-center justify-between gap-2">
            <span class="text-xs font-medium text-muted-foreground">
              {{ t('providers.quota.localEffective') }}
            </span>
            <Badge
              variant="secondary"
              class="px-1.5 py-0 text-[10px]"
            >
              {{ usedPercent.toFixed(1) }}%
            </Badge>
          </div>
          <div class="relative mt-2 h-1.5 w-full overflow-hidden rounded-full bg-border">
            <div
              class="absolute inset-y-0 left-0 transition-all duration-300"
              :class="barClass"
              :style="{ width: `${cappedUsedPercent}%` }"
            />
          </div>
          <div class="mt-2 flex flex-wrap items-center justify-between gap-x-3 gap-y-1 text-xs">
            <span class="font-semibold tabular-nums">
              ${{ used.toFixed(2) }} / ${{ quota.toFixed(2) }}
            </span>
            <span
              v-if="resetIntervalText"
              class="text-muted-foreground"
            >
              {{ resetIntervalText }}
            </span>
          </div>
          <div
            v-if="remoteConfirmedUsedUsd !== null"
            class="mt-2 flex flex-wrap gap-x-4 gap-y-1 border-t border-border/60 pt-2 text-[11px] text-muted-foreground"
          >
            <span data-testid="provider-remote-quota-confirmed-used">
              {{ t('providers.quota.remoteConfirmed') }}:
              <span class="font-medium tabular-nums text-foreground/80">{{ formatMoney(remoteConfirmedUsedUsd) }}</span>
            </span>
            <span
              v-if="pendingLocalUsedUsd >= 0.005"
              data-testid="provider-remote-quota-pending-local"
            >
              {{ t('providers.quota.syncLocalDelta') }}:
              <span class="font-medium tabular-nums text-foreground/80">{{ formatMoney(pendingLocalUsedUsd) }}</span>
            </span>
          </div>
        </div>

        <p class="text-xs font-medium text-muted-foreground">
          {{ t('providers.quota.upstreamWindows') }}
        </p>
        <div class="grid grid-cols-1 gap-2 sm:grid-cols-3">
          <div
            v-for="window in sub2ApiQuotaWindows"
            :key="window.key"
            class="rounded-lg border border-border/70 bg-muted/20 p-3"
            :data-testid="`provider-remote-quota-${window.key}`"
          >
            <div class="flex items-center justify-between gap-2">
              <span class="text-xs font-medium text-muted-foreground">
                {{ remoteWindowLabel(window.key) }}
              </span>
              <Badge
                v-if="remoteQuotaApplied && remoteQuotaGroup.local_sync_window === window.key"
                variant="secondary"
                class="px-1.5 py-0 text-[10px]"
              >
                {{ t('providers.quota.mappedLocally') }}
              </Badge>
            </div>
            <p class="mt-2 text-sm font-semibold tabular-nums">
              {{ remoteWindowAmount(window.key) }}
            </p>
            <template v-if="remoteWindowValues(window.key).limit > 0">
              <div class="relative mt-2 h-1.5 w-full overflow-hidden rounded-full bg-border">
                <div
                  class="absolute inset-y-0 left-0 transition-all duration-300"
                  :class="remoteWindowBarClass(window.key)"
                  :style="{ width: `${remoteWindowPercent(window.key)}%` }"
                />
              </div>
              <p class="mt-1 text-right text-[10px] tabular-nums text-muted-foreground">
                {{ remoteWindowRawPercent(window.key).toFixed(1) }}%
              </p>
            </template>
          </div>
        </div>

        <div
          v-if="remoteQuotaApplied"
          class="flex flex-wrap items-center justify-between gap-x-3 gap-y-1 text-xs text-muted-foreground"
        >
          <span data-testid="provider-remote-quota-local-window">
            <template v-if="remoteQuotaGroup.local_sync_window">
              {{ remoteWindowLabel(remoteQuotaGroup.local_sync_window) }}
              <template v-if="resetIntervalText"> · {{ resetIntervalText }}</template>
            </template>
            <template v-else>
              {{ t('providers.quota.allUnlimited') }}
            </template>
          </span>
          <span v-if="remoteQuotaExpiresText">
            {{ remoteQuotaExpiresText }}
          </span>
        </div>
        <p
          v-else
          data-testid="provider-remote-quota-sync-warning"
          class="text-xs text-amber-600 dark:text-amber-400"
        >
          {{ remoteQuotaGroup.sync_message || t('providers.quota.syncNotApplied') }}
        </p>
      </template>

      <div
        v-else-if="remoteQuotaEnabled"
        data-testid="provider-remote-quota-fallback"
        class="rounded-lg border border-border/70 bg-muted/20 p-3 text-sm"
      >
        <p
          v-if="remoteQuotaFallbackState === 'exhausted'"
          class="font-medium text-red-600 dark:text-red-400"
        >
          {{ t('providers.quota.exhausted') }}
        </p>
        <p
          v-else-if="remoteQuotaFallbackState === 'limited'"
          class="font-semibold tabular-nums"
        >
          {{ t('providers.quota.retainedLocal') }}: ${{ used.toFixed(2) }} / ${{ quota.toFixed(2) }}
        </p>
        <p
          v-else
          class="text-muted-foreground"
        >
          {{ t('providers.quota.awaitingSync') }}
        </p>
      </div>

      <template v-else>
        <div class="relative h-2 w-full overflow-hidden rounded-full bg-border">
          <div
            class="absolute left-0 top-0 h-full transition-all duration-300"
            :class="barClass"
            :style="{ width: `${cappedUsedPercent}%` }"
          />
        </div>
        <div class="flex items-center justify-between text-xs">
          <span
            class="font-semibold"
            data-testid="provider-monthly-quota-amount"
          >
            ${{ used.toFixed(2) }} / ${{ quota.toFixed(2) }}
          </span>
          <span
            v-if="resetIntervalText"
            class="text-muted-foreground"
            data-testid="provider-monthly-quota-reset"
          >
            {{ resetIntervalText }}
          </span>
        </div>
      </template>
    </div>
  </Card>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import Badge from '@/components/ui/badge.vue'
import Card from '@/components/ui/card.vue'
import { useI18n } from '@/i18n'
import type { Sub2ApiQuotaWindow, Sub2ApiRemoteQuotaGroup } from '@/api/providerOps'
import {
  sub2ApiQuotaWindows,
  sub2ApiWindowValues,
} from '@/features/providers/utils/sub2ApiQuota'

const props = withDefaults(defineProps<{
  used?: number | null
  quota?: number | null
  resetIntervalDays?: number | null
  billingType?: string | null
  remoteQuotaEnabled?: boolean
  remoteQuotaGroup?: Sub2ApiRemoteQuotaGroup | null
}>(), {
  used: 0,
  quota: 0,
  resetIntervalDays: null,
  billingType: null,
  remoteQuotaEnabled: false,
  remoteQuotaGroup: null,
})

const { legacyT, locale, t } = useI18n()

const used = computed(() => Number.isFinite(Number(props.used)) ? Number(props.used) : 0)
const quota = computed(() => Number.isFinite(Number(props.quota)) ? Number(props.quota) : 0)
const normalizedResetIntervalDays = computed(() => {
  const value = Number(props.resetIntervalDays)
  return Number.isInteger(value) && value > 0 ? value : null
})
const resetIntervalText = computed(() => {
  const days = normalizedResetIntervalDays.value
  if (days === null) return ''
  if (days === 1) return t('providers.quota.reset.daily')
  return t('providers.quota.reset.intervalDays', { days })
})
const usedPercent = computed(() => quota.value > 0 ? (used.value / quota.value) * 100 : 0)
const cappedUsedPercent = computed(() => Math.min(Math.max(usedPercent.value, 0), 100))
const barClass = computed(() => ratioBarClass(quota.value > 0 ? used.value / quota.value : 0))
const remoteQuotaApplied = computed(() => {
  const status = props.remoteQuotaGroup?.sync_status
  return status === undefined || status === 'applied'
})
const hasLocalEffectiveQuota = computed(() => (
  props.billingType === 'monthly_quota' && quota.value > 0
))
const remoteConfirmedUsedUsd = computed<number | null>(() => {
  const group = props.remoteQuotaGroup
  const window = group?.local_sync_window
  if (!remoteQuotaApplied.value || !window || !group) return null
  return group.remote_confirmed_used_usd ?? remoteWindowValues(window).used
})
const pendingLocalUsedUsd = computed(() => {
  if (remoteConfirmedUsedUsd.value === null) return 0
  return Math.max(used.value - remoteConfirmedUsedUsd.value, 0)
})
const remoteQuotaFallbackState = computed<'limited' | 'exhausted' | 'pending'>(() => {
  if (props.billingType !== 'monthly_quota') return 'pending'
  return quota.value > 0 ? 'limited' : 'exhausted'
})
const remoteQuotaExpiresText = computed(() => {
  const expiresAt = props.remoteQuotaGroup?.expires_at_unix_secs
  if (!expiresAt) return ''
  const date = new Date(expiresAt * 1000).toLocaleDateString(locale.value)
  return t('providers.quota.expires', { date })
})

function remoteWindowLabel(window: Sub2ApiQuotaWindow): string {
  switch (window) {
    case 'daily': return t('providers.quota.window.daily')
    case 'weekly': return t('providers.quota.window.weekly')
    case 'monthly': return t('providers.quota.window.monthly')
  }
}

function remoteWindowValues(window: Sub2ApiQuotaWindow): { used: number; limit: number } {
  if (!props.remoteQuotaGroup) return { used: 0, limit: 0 }
  return sub2ApiWindowValues(props.remoteQuotaGroup, window)
}

function formatMoney(value: number): string {
  return `$${value.toLocaleString('en-US', { maximumFractionDigits: 2 })}`
}

function remoteWindowAmount(window: Sub2ApiQuotaWindow): string {
  const { used: windowUsed, limit } = remoteWindowValues(window)
  return limit <= 0
    ? t('providers.quota.unlimited')
    : `${formatMoney(windowUsed)} / ${formatMoney(limit)}`
}

function remoteWindowRawPercent(window: Sub2ApiQuotaWindow): number {
  const { used: windowUsed, limit } = remoteWindowValues(window)
  return limit > 0 ? (windowUsed / limit) * 100 : 0
}

function remoteWindowPercent(window: Sub2ApiQuotaWindow): number {
  return Math.min(Math.max(remoteWindowRawPercent(window), 0), 100)
}

function remoteWindowBarClass(window: Sub2ApiQuotaWindow): string {
  return ratioBarClass(remoteWindowRawPercent(window) / 100)
}

function ratioBarClass(ratio: number): string {
  if (ratio >= 0.9) return 'bg-red-500'
  if (ratio >= 0.7) return 'bg-yellow-500'
  return 'bg-green-500'
}
</script>
