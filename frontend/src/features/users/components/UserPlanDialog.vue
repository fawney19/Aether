<template>
  <Dialog
    :model-value="open"
    size="xl"
    @update:model-value="(value) => !value && $emit('close')"
  >
    <template #header>
      <div class="border-b border-border px-6 py-4">
        <div class="flex items-center gap-3">
          <div class="flex h-9 w-9 flex-shrink-0 items-center justify-center rounded-lg bg-kraft/10">
            <PackageCheck class="h-5 w-5 text-kraft" />
          </div>
          <div class="min-w-0 flex-1">
            <h3 class="text-lg font-semibold leading-tight text-foreground">
              {{ legacyT('用户套餐') }}
            </h3>
            <p class="text-xs text-muted-foreground">
              {{ userName || '-' }} · {{ legacyT('查看当前套餐并手动发放') }}
            </p>
          </div>
        </div>
      </div>
    </template>

    <div class="max-h-[64vh] space-y-4 overflow-y-auto">
      <div class="rounded-lg border border-amber-500/20 bg-amber-500/10 px-3 py-2.5 text-xs text-amber-100/90">
        {{ legacyT('后台发放会立即生效；如果新套餐包含每日额度或会员权益，用户已有的同类旧套餐会自动失效。') }}
      </div>

      <section class="space-y-2.5">
        <div class="flex items-center justify-between gap-3">
          <h4 class="text-sm font-semibold text-foreground">
            {{ legacyT('当前有效套餐') }}
          </h4>
          <Button
            variant="ghost"
            size="sm"
            class="h-7 px-2 text-[11px]"
            :disabled="loadingEntitlements || !userId"
            @click="userId && $emit('refresh-entitlements', userId)"
          >
            {{ loadingEntitlements ? legacyT('加载中...') : legacyT('刷新') }}
          </Button>
        </div>

        <div
          v-if="loadingEntitlements"
          class="rounded-lg border border-dashed border-border/60 bg-muted/20 px-4 py-8 text-center text-sm text-muted-foreground"
        >
          {{ legacyT('正在加载用户套餐...') }}
        </div>
        <div
          v-else-if="entitlements.length === 0"
          class="rounded-lg border border-dashed border-border/60 bg-muted/20 px-4 py-8 text-center text-sm text-muted-foreground"
        >
          {{ legacyT('当前没有有效套餐') }}
        </div>
        <div
          v-else
          class="space-y-2.5"
        >
          <div
            v-for="item in entitlements"
            :key="item.id"
            class="rounded-lg border border-border bg-card/80 p-3"
          >
            <div class="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
              <div class="min-w-0 flex-1">
                <div class="flex flex-wrap items-center gap-2">
                  <span class="font-medium text-foreground">
                    {{ item.plan_title || item.plan?.title || item.plan_id }}
                  </span>
                  <Badge
                    :variant="item.active ? 'success' : 'secondary'"
                    class="h-5 px-1.5 py-0 text-[10px]"
                  >
                    {{ item.active ? legacyT('生效中') : item.status }}
                  </Badge>
                </div>
                <div class="mt-2 flex flex-wrap gap-1.5">
                  <Badge
                    v-for="label in entitlementLabels(item.entitlements)"
                    :key="label"
                    variant="outline"
                    class="h-5 px-1.5 py-0 text-[10px]"
                  >
                    {{ label }}
                  </Badge>
                </div>
              </div>
              <div class="text-left text-[11px] text-muted-foreground sm:text-right">
                <div>{{ legacyT('开始：') }}{{ formatDateTime(item.starts_at) }}</div>
                <div>{{ legacyT('到期：') }}{{ formatDateTime(item.expires_at) }}</div>
              </div>
            </div>
          </div>
        </div>
      </section>

      <section class="space-y-3 rounded-lg border border-border bg-card/70 p-4">
        <div class="space-y-1">
          <h4 class="text-sm font-semibold text-foreground">
            {{ legacyT('发放套餐') }}
          </h4>
          <p class="text-xs text-muted-foreground">
            {{ legacyT('仅发放套餐权益，不产生用户付款；同类旧套餐会按现有规则自动替换。') }}
          </p>
        </div>

        <Select
          :model-value="selectedPlanId"
          @update:model-value="$emit('update:selectedPlanId', $event)"
        >
          <SelectTrigger
            class="h-9 rounded-md bg-muted/50 px-3"
            :disabled="loadingPlans || plans.length === 0"
          >
            <SelectValue :placeholder="loadingPlans ? legacyT('加载套餐中...') : legacyT('选择要发放的套餐')" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem
              v-for="plan in plans"
              :key="plan.id"
              :value="plan.id"
            >
              <div class="flex min-w-0 items-center gap-2">
                <span class="truncate">{{ plan.title }}</span>
                <span class="shrink-0 text-xs text-muted-foreground">
                  {{ formatPlanPrice(plan) }} · {{ formatPlanDuration(plan) }}
                </span>
                <span
                  v-if="!plan.enabled"
                  class="shrink-0 text-[10px] text-amber-400"
                >
                  {{ legacyT('已下架') }}
                </span>
              </div>
            </SelectItem>
          </SelectContent>
        </Select>

        <Textarea
          :model-value="grantReason"
          class="min-h-[60px] resize-y rounded-md bg-muted/50 text-sm"
          maxlength="512"
          :placeholder="legacyT('备注（可选，例如：人工补偿、活动赠送）')"
          @update:model-value="$emit('update:grantReason', $event)"
        />

        <div class="flex justify-end">
          <Button
            size="sm"
            :disabled="granting || !userId || !selectedPlanId"
            @click="$emit('grant')"
          >
            {{ granting ? legacyT('发放中...') : legacyT('发放套餐') }}
          </Button>
        </div>
      </section>
    </div>

    <template #footer>
      <Button
        variant="outline"
        class="h-10 px-5"
        @click="$emit('close')"
      >
        {{ legacyT('关闭') }}
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { PackageCheck } from 'lucide-vue-next'
import {
  Badge,
  Button,
  Dialog,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Textarea,
} from '@/components/ui'
import { useI18n } from '@/i18n'
import type { AdminUserPlanEntitlement } from '@/api/users'
import type { BillingEntitlement, BillingPlan } from '@/api/billing'

defineProps<{
  open: boolean
  userId?: string | null
  userName?: string
  entitlements: AdminUserPlanEntitlement[]
  plans: BillingPlan[]
  selectedPlanId: string
  grantReason: string
  loadingEntitlements: boolean
  loadingPlans: boolean
  granting: boolean
  formatDateTime: (value?: string | null) => string
  formatPlanPrice: (plan: BillingPlan) => string
  formatPlanDuration: (plan: BillingPlan) => string
  entitlementLabels: (items: BillingEntitlement[] | undefined) => string[]
}>()

defineEmits<{
  close: []
  'update:selectedPlanId': [value: string]
  'update:grantReason': [value: string]
  'refresh-entitlements': [userId: string]
  grant: []
}>()

const { legacyT } = useI18n()
</script>
