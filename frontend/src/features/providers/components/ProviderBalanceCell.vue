<template>
  <!-- 余额正在加载中 -->
  <div
    v-if="provider.ops_configured && isBalanceLoading(provider.id)"
    class="flex items-center gap-1.5 text-xs text-muted-foreground"
  >
    <Loader2 class="h-3 w-3 animate-spin" />
    <span>{{ legacyT('加载中...') }}</span>
  </div>
  <!-- 显示从上游 API 查询的余额 -->
  <div
    v-else-if="provider.ops_configured && getProviderBalance(provider.id)"
    class="flex items-center gap-2 text-xs"
  >
    <!-- 余额文字：balance + points 分开显示，或普通余额 -->
    <template
      v-for="(bd, idx) in [getProviderBalanceBreakdown(provider.id)]"
      :key="idx"
    >
      <div
        v-if="bd"
        class="min-w-[4.5rem] tabular-nums leading-tight"
      >
        <div class="font-semibold text-foreground/90">
          ${{ bd.balance.toFixed(2) }}
        </div>
        <div class="text-muted-foreground/70 text-[10px]">
          ${{ bd.points.toFixed(2) }}
        </div>
      </div>
      <span
        v-else
        class="font-semibold text-foreground/90 min-w-[4.5rem] tabular-nums"
      >
        {{ formatBalanceDisplay(getProviderBalance(provider.id)) }}
      </span>
    </template>
    <!-- 窗口限额 + 签到状态 + Cookie 失效警告 -->
    <div
      v-if="getProviderBalanceExtra(provider.id, provider.ops_architecture_id).length > 0 || getProviderCheckin(provider.id) || getProviderCookieExpired(provider.id)"
      class="text-muted-foreground/70 space-y-0.5"
    >
      <!-- 限额（进度条 + 倒计时，每行一个） -->
      <template
        v-for="item in getProviderBalanceExtra(provider.id, provider.ops_architecture_id)"
        :key="item.label"
      >
        <div
          :title="item.tooltip"
          class="flex items-center gap-1"
        >
          <span class="text-[10px] text-muted-foreground/60 w-4">{{ item.label }}</span>
          <div class="w-12 h-1.5 bg-border rounded-full overflow-hidden">
            <div
              class="h-full rounded-full"
              :class="[
                item.percent !== undefined && item.percent >= 50 ? 'bg-green-500' :
                item.percent !== undefined && item.percent >= 20 ? 'bg-amber-500' : 'bg-red-500'
              ]"
              :style="{ width: `${item.percent ?? 0}%` }"
            />
          </div>
          <span class="text-[10px] text-muted-foreground/50 w-7 text-right tabular-nums">{{ item.value }}</span>
          <span
            v-if="item.resetsAt"
            class="text-[10px] text-muted-foreground/40 w-14 text-right tabular-nums"
          >{{ formatResetCountdown(item.resetsAt) }}</span>
        </div>
      </template>
      <!-- Cookie 失效警告 -->
      <div
        v-if="getProviderCookieExpired(provider.id)"
        class="flex items-center gap-1"
      >
        <span
          class="text-[10px] text-amber-600 dark:text-amber-500"
          :title="getProviderCookieExpired(provider.id)?.message"
        >{{ legacyT('签到 Cookie 已失效') }}</span>
      </div>
      <!-- 签到状态 -->
      <div
        v-else-if="getProviderCheckin(provider.id)"
        class="flex items-center gap-1.5"
      >
        <span
          v-if="getProviderCheckin(provider.id)?.success !== false"
          class="text-[10px] text-muted-foreground/60"
          :title="getProviderCheckin(provider.id)?.message"
        >{{ legacyT('已签到') }}</span>
        <span
          v-else
          class="text-[10px] text-destructive/70"
          :title="getProviderCheckin(provider.id)?.message"
        >{{ legacyT('签到失败') }}</span>
      </div>
    </div>
  </div>
  <!-- 余额查询失败时显示错误 -->
  <div
    v-else-if="provider.ops_configured && getProviderBalanceError(provider.id)"
    class="text-xs text-destructive/80"
    :title="getProviderBalanceError(provider.id)?.message"
  >
    {{ getProviderBalanceError(provider.id)?.message }}
  </div>
  <!-- 显示本地配置的月度配额 -->
  <div
    v-else-if="provider.billing_type === 'monthly_quota'"
    class="space-y-0.5 text-xs"
  >
    <Badge
      variant="outline"
      class="text-[10px] font-normal border-border/50"
    >
      {{ formatBillingType(provider.billing_type) }}
    </Badge>
    <div class="text-muted-foreground/70 pt-0.5">
      <span
        class="font-semibold"
        :class="getQuotaUsedColorClass(provider)"
      >${{ (provider.monthly_used_usd ?? 0).toFixed(2) }}</span> / <span class="font-medium">${{ (provider.monthly_quota_usd ?? 0).toFixed(2) }}</span>
    </div>
  </div>
  <span
    v-else
    class="text-xs text-muted-foreground/50"
  >-</span>
</template>

<script setup lang="ts">
import { Loader2 } from 'lucide-vue-next'
import Badge from '@/components/ui/badge.vue'
import type { ProviderWithEndpointsSummary } from '@/api/endpoints'
import { formatBillingType } from '@/utils/format'
import type { BalanceExtraItem } from '@/features/providers/auth-templates'
import { useI18n } from '@/i18n'

defineProps<{
  provider: ProviderWithEndpointsSummary
  isBalanceLoading: (providerId: string) => boolean
  getProviderBalance: (providerId: string) => { available: number | null; currency: string } | null
  getProviderBalanceBreakdown: (providerId: string) => { balance: number; points: number; currency: string } | null
  getProviderBalanceError: (providerId: string) => { status: string; message: string } | null
  getProviderCheckin: (providerId: string) => { success: boolean | null; message: string } | null
  getProviderCookieExpired: (providerId: string) => { expired: boolean; message: string } | null
  getProviderBalanceExtra: (providerId: string, architectureId?: string) => BalanceExtraItem[]
  formatBalanceDisplay: (balance: { available: number | null; currency: string } | null) => string
  formatResetCountdown: (resetsAt: number) => string
  getQuotaUsedColorClass: (provider: ProviderWithEndpointsSummary) => string
}>()

const { legacyT } = useI18n()
</script>
