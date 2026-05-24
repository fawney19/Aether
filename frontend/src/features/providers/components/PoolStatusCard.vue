<template>
  <Card class="overflow-hidden">
    <!-- Header -->
    <div class="p-4 border-b border-border/60">
      <div class="flex items-center justify-between">
        <div class="flex flex-wrap items-center gap-2">
          <h3 class="text-sm font-semibold">
            号池状态
          </h3>
          <Badge
            v-if="poolStatus"
            variant="secondary"
            class="text-xs"
          >
            {{ poolStatus.total_keys }} 个密钥
          </Badge>
          <Badge
            v-if="poolStatus && poolStatus.total_sticky_sessions > 0"
            variant="outline"
            class="text-xs"
          >
            {{ poolStatus.total_sticky_sessions }} 个粘性会话
          </Badge>
          <Badge
            v-if="poolStatus && poolStatus.provider_desired_hot > 0"
            variant="outline"
            class="text-xs"
          >
            热池 {{ poolStatus.provider_hot_count }} / {{ poolStatus.provider_desired_hot }}
          </Badge>
          <Badge
            v-if="poolStatus && poolStatus.provider_in_flight > 0"
            variant="outline"
            class="text-xs"
          >
            in-flight {{ poolStatus.provider_in_flight }}
          </Badge>
          <Badge
            v-if="poolStatus && poolStatus.provider_desired_hot > 0"
            variant="outline"
            class="text-xs"
          >
            EMA {{ formatEmaHeat(poolStatus.provider_ema_in_flight) }}
          </Badge>
          <Badge
            v-if="poolStatus?.provider_burst_pending"
            variant="secondary"
            class="text-xs"
          >
            补热中
          </Badge>
        </div>
        <RefreshButton
          :loading="refreshing"
          title="刷新号池状态"
          @click="refresh"
        />
      </div>
    </div>

    <!-- Loading -->
    <div
      v-if="initialLoading"
      class="flex items-center justify-center py-8"
    >
      <div class="animate-spin rounded-full h-6 w-6 border-b-2 border-primary" />
    </div>

    <!-- Pool not enabled -->
    <div
      v-else-if="poolStatus && !poolStatus.pool_enabled"
      class="p-6 text-center text-muted-foreground"
    >
      <p class="text-sm">
        号池未启用
      </p>
      <p class="text-xs mt-1">
        请在提供商编辑中配置号池参数
      </p>
    </div>

    <!-- Key list -->
    <div
      v-else-if="poolStatus && poolStatus.keys.length > 0"
      class="divide-y divide-border/40"
    >
      <div
        v-for="key in poolStatus.keys"
        :key="key.key_id"
        class="px-4 py-3 hover:bg-muted/30 transition-colors"
        :class="{ 'opacity-40': !key.is_active }"
      >
        <!-- Row 1: name + cooldown + actions -->
        <div class="flex items-center justify-between gap-2">
          <div class="flex items-center gap-2 min-w-0">
            <span class="text-sm font-medium truncate">{{ key.key_name || '未命名' }}</span>
            <Badge
              v-if="key.cooldown_reason"
              variant="destructive"
              class="text-[10px] px-1.5 py-0 shrink-0"
            >
              {{ formatCooldownReason(key.cooldown_reason) }}
            </Badge>
            <span
              v-if="key.cooldown_ttl_seconds"
              class="text-[10px] text-destructive tabular-nums shrink-0"
            >
              {{ formatTTL(key.cooldown_ttl_seconds) }}
            </span>
          </div>
          <div class="flex items-center gap-0.5 shrink-0">
            <Button
              v-if="key.cooldown_reason"
              variant="ghost"
              size="icon"
              class="h-7 w-7 text-muted-foreground hover:text-green-600"
              title="清除冷却"
              :disabled="actionLoading === key.key_id"
              @click="handleClearCooldown(key.key_id)"
            >
              <RefreshCw
                class="w-3.5 h-3.5"
                :class="{ 'animate-spin': actionLoading === key.key_id }"
              />
            </Button>
            <Button
              v-if="key.cost_limit != null && key.cost_window_usage > 0"
              variant="ghost"
              size="icon"
              class="h-7 w-7 text-muted-foreground hover:text-foreground"
              title="重置成本窗口"
              :disabled="actionLoading === key.key_id"
              @click="handleResetCost(key.key_id)"
            >
              <RotateCcw class="w-3.5 h-3.5" />
            </Button>
          </div>
        </div>

        <!-- Row 2: cost + sticky + lru -->
        <div class="flex items-center gap-3 mt-1.5 text-[11px] text-muted-foreground">
          <!-- Cost with limit -->
          <div
            v-if="key.cost_limit != null"
            class="flex items-center gap-1.5 flex-1 min-w-0"
          >
            <span class="shrink-0">成本</span>
            <div class="flex-1 h-1.5 bg-border rounded-full overflow-hidden max-w-[120px]">
              <div
                class="h-full transition-all duration-300 rounded-full"
                :class="getCostBarColor(key.cost_window_usage, key.cost_limit)"
                :style="{
                  width: `${Math.min((key.cost_window_usage / key.cost_limit) * 100, 100)}%`,
                }"
              />
            </div>
            <span class="tabular-nums shrink-0">
              {{ formatTokens(key.cost_window_usage) }} / {{ formatTokens(key.cost_limit) }}
            </span>
          </div>
          <!-- Cost without limit -->
          <div
            v-else-if="key.cost_window_usage > 0"
            class="flex items-center gap-1"
          >
            <span>成本</span>
            <span class="tabular-nums">{{ formatTokens(key.cost_window_usage) }}</span>
          </div>

          <span
            v-if="(key.cost_limit != null || key.cost_window_usage > 0) && key.sticky_sessions > 0"
            class="text-muted-foreground/40"
          >|</span>

          <!-- Sticky sessions -->
          <span v-if="key.sticky_sessions > 0">
            {{ key.sticky_sessions }} 粘性会话
          </span>

          <!-- LRU score -->
          <template v-if="key.lru_score != null">
            <span class="text-muted-foreground/40">|</span>
            <span>LRU {{ formatLruScore(key.lru_score) }}</span>
          </template>
        </div>
      </div>
    </div>

    <!-- Empty -->
    <div
      v-else
      class="p-6 text-center text-muted-foreground"
    >
      <p class="text-sm">
        暂无密钥数据
      </p>
    </div>
  </Card>
</template>

<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { RefreshCw, RotateCcw } from 'lucide-vue-next'

import { getPoolStatus, clearPoolCooldown, resetPoolCost } from '@/api/endpoints/pool'
import type { PoolStatusResponse } from '@/api/endpoints/pool'
import { parseApiError } from '@/utils/errorParser'
import { formatTokens } from '@/utils/format'
import { useToast } from '@/composables/useToast'

import Card from '@/components/ui/card.vue'
import Badge from '@/components/ui/badge.vue'
import Button from '@/components/ui/button.vue'
import RefreshButton from '@/components/ui/refresh-button.vue'

const props = defineProps<{
  providerId: string
  poolEnabled: boolean
}>()

const { error: showError, success } = useToast()

const poolStatus = ref<PoolStatusResponse | null>(null)
const initialLoading = ref(true)
const refreshing = ref(false)
const actionLoading = ref<string | null>(null)

async function loadPoolStatus() {
  try {
    poolStatus.value = await getPoolStatus(props.providerId)
  } catch (err) {
    showError(parseApiError(err))
  }
}

async function refresh() {
  refreshing.value = true
  try {
    await loadPoolStatus()
  } finally {
    refreshing.value = false
  }
}

async function handleClearCooldown(keyId: string) {
  actionLoading.value = keyId
  try {
    const res = await clearPoolCooldown(props.providerId, keyId)
    success(res.message)
    await loadPoolStatus()
  } catch (err) {
    showError(parseApiError(err))
  } finally {
    actionLoading.value = null
  }
}

async function handleResetCost(keyId: string) {
  actionLoading.value = keyId
  try {
    const res = await resetPoolCost(props.providerId, keyId)
    success(res.message)
    await loadPoolStatus()
  } catch (err) {
    showError(parseApiError(err))
  } finally {
    actionLoading.value = null
  }
}

const COOLDOWN_REASON_MAP: Record<string, string> = {
  rate_limited_429: '429 限流',
  forbidden_403: '403 禁止',
  overloaded_529: '529 过载',
  auth_failed_401: '401 认证失败',
  payment_required_402: '402 欠费',
  server_error_500: '500 错误',
}

function formatCooldownReason(reason: string): string {
  return COOLDOWN_REASON_MAP[reason] || reason
}

function formatTTL(seconds: number): string {
  if (seconds <= 0) return ''
  const m = Math.floor(seconds / 60)
  const s = seconds % 60
  return m > 0 ? `${m}m ${s}s` : `${s}s`
}

function formatEmaHeat(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return '0.0'
  return value.toFixed(1)
}

function getCostBarColor(usage: number, limit: number): string {
  const ratio = usage / limit
  if (ratio >= 0.9) return 'bg-red-500'
  if (ratio >= 0.7) return 'bg-yellow-500'
  return 'bg-green-500'
}

function formatLruScore(score: number): string {
  const now = Date.now() / 1000
  const diff = now - score
  if (diff < 60) return '刚刚'
  if (diff < 3600) return `${Math.floor(diff / 60)}m 前`
  if (diff < 86400) return `${Math.floor(diff / 3600)}h 前`
  return `${Math.floor(diff / 86400)}d 前`
}

onMounted(async () => {
  await loadPoolStatus()
  initialLoading.value = false
})
</script>
