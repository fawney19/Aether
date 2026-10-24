<template>
  <section
    class="space-y-4"
    data-provider-accounts
  >
    <div class="flex flex-wrap items-center justify-between gap-2">
      <h2 class="text-sm font-semibold">
        {{ t('账户余额与套餐', 'Balances and subscriptions') }}
      </h2>
      <span class="text-xs text-muted-foreground">{{ t('当前账户状态', 'Current account status') }}</span>
    </div>
    <div
      v-if="accounts.length"
      class="grid grid-cols-1 gap-4 xl:grid-cols-2"
    >
      <Card
        v-for="account in orderedAccounts"
        :key="account.provider_id"
        class="min-w-0 space-y-4 p-4 sm:p-5"
      >
        <div class="flex items-start justify-between gap-3">
          <div class="min-w-0">
            <h3 class="break-words text-sm font-semibold">
              {{ account.provider_name }}
            </h3>
            <p class="mt-1 text-xs text-muted-foreground">
              {{ billingLabel(account.billing_type) }}<span v-if="!account.is_active"> · {{ t('已停用', 'Disabled') }}</span>
            </p>
          </div>
          <span
            v-if="attention(account)"
            class="shrink-0 rounded-full bg-amber-500/10 px-2 py-1 text-xs text-amber-700 dark:text-amber-400"
          >{{ attention(account) }}</span>
        </div>
        <div class="grid grid-cols-2 gap-4">
          <div>
            <p class="text-xs text-muted-foreground">
              {{ t('上游余额', 'Upstream balance') }}
            </p>
            <p class="mt-2 text-xl font-semibold tabular-nums">
              {{ amount(account.balance?.available, account.balance?.currency) }}
            </p>
            <p class="mt-1 text-xs text-muted-foreground">
              {{ account.balance?.observed_at ? `${t('更新于', 'Updated')} ${timestamp(account.balance.observed_at, timezone)}` : t('暂无余额快照', 'No balance snapshot') }}
            </p>
            <p
              v-if="account.balance && account.balance.status !== 'success'"
              class="mt-1 text-xs text-amber-700 dark:text-amber-400"
            >
              {{ t('上次查询未成功', 'Last query unsuccessful') }}
            </p>
          </div>
          <div>
            <p class="text-xs text-muted-foreground">
              {{ t('配置套餐剩余额度', 'Configured plan remaining') }}
            </p>
            <p class="mt-2 text-xl font-semibold tabular-nums">
              {{ amount(account.quota?.remaining, account.quota?.currency) }}
            </p>
            <p class="mt-1 text-xs text-muted-foreground">
              {{ account.quota ? `${t('周期总额度', 'Period quota')} ${amount(account.quota.limit, account.quota.currency)}` : t('未配置套餐额度', 'No configured plan quota') }}
            </p>
          </div>
        </div>
        <div
          v-if="account.quota"
          class="space-y-2"
        >
          <div class="flex flex-wrap justify-between gap-2 text-xs text-muted-foreground">
            <span>{{ t('已使用', 'Used') }} {{ amount(account.quota.used, account.quota.currency) }}</span>
            <span>{{ account.quota.expires_at ? `${t('到期', 'Expires')} ${timestamp(account.quota.expires_at, timezone)}` : t('未设置到期时间', 'No expiry configured') }}</span>
          </div>
          <div class="h-1.5 overflow-hidden rounded-full bg-muted">
            <div
              class="h-full rounded-full"
              :class="quotaRatio(account) >= 90 ? 'bg-amber-500' : 'bg-primary/65'"
              :style="{ width: `${quotaRatio(account)}%` }"
            />
          </div>
        </div>
        <div
          v-if="account.balance?.plan_name || account.balance?.subscriptions.length"
          class="space-y-3 border-t pt-3"
        >
          <p
            v-if="account.balance.plan_name"
            class="text-xs font-medium"
          >
            {{ t('上游套餐', 'Upstream plan') }} · {{ account.balance.plan_name }}
          </p>
          <div
            v-for="(plan, index) in account.balance.subscriptions"
            :key="index"
            class="space-y-1 text-xs"
          >
            <div class="flex flex-wrap justify-between gap-2">
              <span class="font-medium">{{ plan.group_name || t('订阅套餐', 'Subscription') }}</span>
              <span class="text-muted-foreground">{{ plan.expires_at ? timestamp(plan.expires_at, timezone) : t('未提供到期时间', 'No expiry reported') }}</span>
            </div>
            <p class="text-muted-foreground">
              {{ planUsage(plan) }}
            </p>
          </div>
        </div>
        <div
          v-if="account.custom_quota"
          class="space-y-1.5 border-t pt-3 text-xs"
        >
          <div class="flex flex-wrap items-center justify-between gap-2">
            <p class="font-medium">
              {{ t('自定义额度查询', 'Custom quota query') }}
              <span v-if="account.custom_quota.plan_name"> · {{ account.custom_quota.plan_name }}</span>
            </p>
            <span
              v-if="account.custom_quota.is_valid === false"
              class="text-destructive"
            >{{ account.custom_quota.invalid_message || t('套餐已失效', 'Plan invalid') }}</span>
          </div>
          <div class="flex flex-wrap gap-x-4 gap-y-1 text-muted-foreground">
            <span class="tabular-nums">
              {{ t('已用', 'Used') }} {{ amount(account.custom_quota.used, account.custom_quota.unit ?? undefined) }}
              /
              {{ t('总额', 'Total') }} {{ amount(account.custom_quota.total, account.custom_quota.unit ?? undefined) }}
            </span>
            <span class="tabular-nums">
              {{ t('剩余', 'Remaining') }} {{ amount(account.custom_quota.remaining, account.custom_quota.unit ?? undefined) }}
            </span>
          </div>
          <div
            v-for="period in account.custom_quota.periods ?? []"
            :key="period.key"
            class="space-y-1"
          >
            <div class="flex flex-wrap justify-between gap-2">
              <span class="font-medium">{{ periodLabel(period.key) }}</span>
              <span class="tabular-nums text-muted-foreground">
                {{ amount(period.used, account.custom_quota.unit ?? undefined) }} / {{ amount(period.limit, account.custom_quota.unit ?? undefined) }}
                <template v-if="period.reset_at"> · {{ t('重置', 'Reset') }} {{ resetDisplay(period.reset_at) }}</template>
              </span>
            </div>
            <div class="h-1.5 overflow-hidden rounded-full bg-muted">
              <div
                class="h-full rounded-full"
                :class="periodPercent(period) >= 90 ? 'bg-amber-500' : 'bg-primary/65'"
                :style="{ width: `${periodPercent(period)}%` }"
              />
            </div>
          </div>
          <div
            v-if="hasTotalWindow(account.custom_quota)"
            class="h-1.5 overflow-hidden rounded-full bg-muted"
          >
            <div
              class="h-full rounded-full"
              :class="customQuotaPercent(account.custom_quota) >= 90 ? 'bg-amber-500' : 'bg-primary/65'"
              :style="{ width: `${customQuotaPercent(account.custom_quota)}%` }"
            />
          </div>
          <p
            v-if="account.custom_quota.extra"
            class="text-muted-foreground"
          >
            {{ account.custom_quota.extra }}
          </p>
        </div>
      </Card>
    </div>
    <Card
      v-else
      class="p-10 text-center text-sm text-muted-foreground"
    >
      {{ t('暂无提供商账户', 'No provider accounts') }}
    </Card>
  </section>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { Card } from '@/components/ui'
import type { ProviderAccount, ProviderAccountSubscription, ProviderCustomQuota, ProviderCustomQuotaPeriod } from '@/api/providerFinance'
import { money, timestamp } from '../format'
import { useOverviewI18n } from '../i18n'
const props = defineProps<{ accounts: ProviderAccount[]; timezone: string; observedAt?: string }>()
const { t } = useOverviewI18n()
function amount(value: string | number | null | undefined, currency: string | null = 'USD') { return value == null || !currency || !/^[a-z]{3}$/i.test(currency) ? '-' : money({ value: String(value), currency, basis: 'provider_account', status: 'known' }) }
function billingLabel(value: string | null) {
  return value === 'monthly_quota' ? t('周期套餐', 'Period plan') : value === 'free_tier' ? t('免费额度', 'Free tier') : value === 'pay_as_you_go' ? t('按量付费', 'Pay as you go') : t('未设置计费方式', 'Billing type not configured')
}
function quotaRatio(account: ProviderAccount) {
  return account.quota && Number(account.quota.limit) > 0 ? Math.min(100, Math.max(0, Number(account.quota.used) / Number(account.quota.limit) * 100)) : 0
}
function attention(account: ProviderAccount) {
  if (!account.is_active) return ''
  const now = props.observedAt ? Date.parse(props.observedAt) : Date.now()
  const end = account.quota?.expires_at ? Date.parse(account.quota.expires_at) : NaN
  if (Number.isFinite(end) && end <= now) return t('套餐已到期', 'Plan expired')
  if (Number.isFinite(end) && end - now <= 7 * 86_400_000) return t('7 天内到期', 'Expires within 7 days')
  if (account.quota?.remaining != null && Number(account.quota.remaining) <= 0) return t('套餐额度用尽', 'Plan quota exhausted')
  if (quotaRatio(account) >= 90) return t('套餐剩余不足 10%', 'Under 10% quota remaining')
  return ''
}
const orderedAccounts = computed(() => [...props.accounts].sort((a, b) => Number(Boolean(attention(b))) - Number(Boolean(attention(a))) || a.provider_name.localeCompare(b.provider_name)))
function planUsage(plan: ProviderAccountSubscription) {
  return [
    [t('日额度', 'Daily'), plan.daily_used_usd, plan.daily_limit_usd],
    [t('周额度', 'Weekly'), plan.weekly_used_usd, plan.weekly_limit_usd],
    [t('月额度', 'Monthly'), plan.monthly_used_usd, plan.monthly_limit_usd],
  ].filter(([, , limit]) => limit != null).map(([label, used, limit]) => `${label} ${amount(used)} / ${amount(limit)}`).join(' · ') || t('暂无额度明细', 'Quota details unavailable')
}
function customQuotaPercent(quota: ProviderCustomQuota) {
  const used = Number(quota.used)
  const total = Number(quota.total)
  return Number.isFinite(used) && Number.isFinite(total) && total > 0 ? Math.min(100, Math.max(0, (used / total) * 100)) : 0
}
function hasTotalWindow(quota: ProviderCustomQuota) {
  return quota.used != null && quota.total != null && Number(quota.total) > 0
}
function periodLabel(key: string) {
  return key === '5h' ? t('5 小时', '5 hours') : key === 'week' ? t('周', 'Weekly') : key === 'month' ? t('月', 'Monthly') : key
}
function periodPercent(period: ProviderCustomQuotaPeriod) {
  const used = Number(period.used)
  const limit = Number(period.limit)
  return Number.isFinite(used) && Number.isFinite(limit) && limit > 0 ? Math.min(100, Math.max(0, (used / limit) * 100)) : 0
}
function resetDisplay(raw: string) {
  return /^\d{4}-\d{2}-\d{2}T/.test(raw) ? timestamp(raw, props.timezone) : raw
}
</script>
