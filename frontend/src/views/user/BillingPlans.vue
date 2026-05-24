<template>
  <PageContainer>
    <PageHeader
      title="套餐中心"
      description="购买每日额度或会员权益"
    />

    <div class="mt-6 space-y-6">
      <div
        v-if="loading"
        class="py-16"
      >
        <LoadingState message="正在加载套餐..." />
      </div>

      <template v-else>
        <CardSection
          title="当前权益"
          description="只展示仍在有效期内的套餐权益"
        >
          <div
            v-if="activeEntitlements.length"
            class="grid grid-cols-1 gap-3 lg:grid-cols-2"
          >
            <div
              v-for="item in activeEntitlements"
              :key="item.id"
              class="rounded-lg border border-border/60 bg-muted/20 p-4"
            >
              <div class="flex items-start justify-between gap-3">
                <div>
                  <div class="font-medium">
                    {{ planTitle(item.plan_id) }}
                  </div>
                  <div class="mt-1 text-xs text-muted-foreground">
                    {{ formatDate(item.starts_at) }} - {{ formatDate(item.expires_at) }}
                  </div>
                </div>
                <Badge variant="success">
                  生效中
                </Badge>
              </div>
              <div class="mt-3 flex flex-wrap gap-1.5">
                <Badge
                  v-for="label in entitlementLabels(item.entitlements)"
                  :key="label"
                  variant="outline"
                >
                  {{ label }}
                </Badge>
              </div>
            </div>
          </div>
          <EmptyState
            v-else
            title="暂无有效套餐"
            description="购买套餐后，有效权益会显示在这里"
          />
        </CardSection>

        <CardSection
          title="可购买套餐"
          description="支付成功后由回调自动发放权益"
        >
          <div class="grid grid-cols-1 gap-4 xl:grid-cols-3">
            <Card
              v-for="plan in purchaseablePlans"
              :key="plan.id"
              class="flex flex-col p-5"
            >
              <div class="flex items-start justify-between gap-3">
                <div>
                  <h3 class="text-base font-semibold">
                    {{ plan.title }}
                  </h3>
                  <p class="mt-1 min-h-[32px] text-xs text-muted-foreground">
                    {{ plan.description || '标准套餐' }}
                  </p>
                </div>
                <Badge variant="outline">
                  {{ formatDuration(plan.duration_unit, plan.duration_value) }}
                </Badge>
              </div>

              <div class="mt-5">
                <span class="text-3xl font-semibold tabular-nums">
                  {{ Number(plan.price_amount || 0).toFixed(2) }}
                </span>
                <span class="ml-1 text-sm text-muted-foreground">
                  {{ plan.price_currency }}
                </span>
              </div>

              <div class="mt-5 flex flex-wrap gap-1.5">
                <Badge
                  v-for="label in entitlementLabels(plan.entitlements)"
                  :key="label"
                  variant="outline"
                >
                  {{ label }}
                </Badge>
              </div>

              <div
                v-if="replacementNotice(plan)"
                class="mt-4 rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-xs leading-5 text-amber-200"
              >
                {{ replacementNotice(plan) }}
              </div>

              <div class="mt-5 flex-1" />

              <div class="mt-5 space-y-3">
                <Select v-model="selectedChannel">
                  <SelectTrigger>
                    <SelectValue
                      :placeholder="checkoutOptions.length ? '选择支付通道' : '暂无可用支付通道'"
                    />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem
                      v-for="option in checkoutOptions"
                      :key="option.key"
                      :value="option.key"
                    >
                      {{ option.display_name }}
                    </SelectItem>
                  </SelectContent>
                </Select>
                <Button
                  class="w-full"
                  :disabled="
                    checkoutLoadingPlanId === plan.id
                      || checkoutOptions.length === 0
                      || !selectedChannel
                  "
                  @click="checkoutPlan(plan)"
                >
                  <CreditCard class="mr-2 h-4 w-4" />
                  {{ checkoutLoadingPlanId === plan.id ? '创建订单中...' : '购买套餐' }}
                </Button>
              </div>
            </Card>

            <div
              v-if="purchaseablePlans.length === 0"
              class="xl:col-span-3"
            >
              <EmptyState
                title="暂无可购买套餐"
                description="管理员上架套餐后会显示在这里"
              />
            </div>
          </div>
        </CardSection>

        <Card
          v-if="latestCheckout"
          class="p-4"
        >
          <div class="flex flex-col gap-3 md:flex-row md:items-center md:justify-between">
            <div>
              <div class="text-sm font-medium">
                最新订单：<span class="font-mono">{{ latestCheckout.order.order_no }}</span>
              </div>
              <div class="mt-1 text-xs text-muted-foreground">
                应付 {{ latestCheckout.order.pay_amount ?? '-' }} {{ latestCheckout.order.pay_currency || '' }}
              </div>
            </div>
            <Button
              v-if="latestCheckoutActionLabel"
              variant="outline"
              @click="openLatestPayment"
            >
              {{ latestCheckoutActionLabel }}
            </Button>
          </div>
        </Card>
      </template>
    </div>

    <StripePaymentDialog
      v-model:open="stripeDialogOpen"
      :instructions="stripePaymentInstructions"
      title="套餐 Stripe 支付"
      description="完成支付后，套餐权益会由 Stripe Webhook 自动发放。"
      confirm-text="支付套餐"
      @success="handleStripePaymentSuccess"
    />
  </PageContainer>
</template>

<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { CreditCard } from 'lucide-vue-next'
import {
  billingApi,
  type BillingDurationUnit,
  type BillingEntitlement,
  type BillingCheckoutResponse,
  type BillingPlan,
  type UserPlanEntitlement,
} from '@/api/billing'
import { walletApi, type WalletRechargeOption } from '@/api/wallet'
import {
  Badge,
  Button,
  Card,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui'
import { EmptyState, LoadingState, StripePaymentDialog } from '@/components/common'
import { CardSection, PageContainer, PageHeader } from '@/components/layout'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'
import { log } from '@/utils/logger'
import {
  getPaymentInstructionString,
  getStripePaymentInstructions,
  type PaymentInstructionMap,
} from '@/utils/paymentInstructions'

const { success, error: showError } = useToast()

const loading = ref(true)
const plans = ref<BillingPlan[]>([])
const entitlements = ref<UserPlanEntitlement[]>([])
const rechargeOptions = ref<WalletRechargeOption[]>([])
const selectedChannel = ref('')
const checkoutLoadingPlanId = ref<string | null>(null)
const latestCheckout = ref<BillingCheckoutResponse | null>(null)
const stripeDialogOpen = ref(false)
const stripePaymentInstructions = ref<PaymentInstructionMap | null>(null)

const checkoutOptions = computed(() =>
  rechargeOptions.value
    .filter((option) => Boolean(paymentOptionProvider(option)) && Boolean(option.payment_channel))
    .map((option, index) => ({
      ...option,
      key: [
        paymentOptionProvider(option),
        option.payment_method,
        option.payment_channel || '',
        index,
      ].join(':'),
    }))
)

const selectedCheckoutOption = computed(() => {
  if (checkoutOptions.value.length === 0) return null
  return checkoutOptions.value.find(option => option.key === selectedChannel.value)
    || checkoutOptions.value[0]
})

const activeEntitlements = computed(() =>
  entitlements.value.filter((item) =>
    item.active !== false
    && item.status === 'active'
    && hasPackageEntitlement(item.entitlements)
  )
)

const purchaseablePlans = computed(() =>
  plans.value.filter((plan) => hasPackageEntitlement(plan.entitlements))
)

const latestPaymentUrl = computed(() => {
  const value = latestCheckout.value?.payment_instructions?.payment_url
  return typeof value === 'string' && value ? value : ''
})

const latestCheckoutActionLabel = computed(() => {
  const instructions = latestCheckout.value?.payment_instructions
  if (getStripePaymentInstructions(instructions)) return '打开 Stripe 支付'
  if (latestPaymentUrl.value) return '打开支付链接'
  return ''
})

watch(checkoutOptions, (options) => {
  const keys = options.map(option => option.key)
  if (!keys.includes(selectedChannel.value)) {
    selectedChannel.value = keys[0] || ''
  }
}, { immediate: true })

onMounted(async () => {
  await Promise.all([
    loadPlans(),
    loadEntitlements(),
    loadRechargeOptions(),
  ])
  loading.value = false
})

async function loadPlans() {
  try {
    const response = await billingApi.listPlans()
    plans.value = response.items
  } catch (err) {
    log.error('加载套餐失败:', err)
    showError(parseApiError(err, '加载套餐失败'))
  }
}

async function loadEntitlements() {
  try {
    const response = await billingApi.listEntitlements()
    entitlements.value = response.items
  } catch (err) {
    log.error('加载套餐权益失败:', err)
    showError(parseApiError(err, '加载套餐权益失败'))
  }
}

async function loadRechargeOptions() {
  try {
    const response = await walletApi.listRechargeOptions()
    rechargeOptions.value = response.items
    if (!selectedChannel.value && checkoutOptions.value.length > 0) {
      selectedChannel.value = checkoutOptions.value[0].key
    }
  } catch (err) {
    log.error('加载支付通道失败:', err)
    showError(parseApiError(err, '加载支付通道失败'))
  }
}

async function checkoutPlan(plan: BillingPlan) {
  if (hasMatchingActivePlan(plan)) {
    const confirmed = window.confirm('购买成功后，同类旧套餐会自动失效。确定继续购买吗？')
    if (!confirmed) return
  }
  const option = selectedCheckoutOption.value
  if (!option) {
    showError('请选择支付方式')
    return
  }
  const provider = paymentOptionProvider(option)
  if (!provider) {
    showError('支付方式配置无效')
    return
  }
  checkoutLoadingPlanId.value = plan.id
  try {
    const response = await billingApi.checkout(plan.id, {
      payment_method: option.payment_method || provider,
      payment_provider: provider,
      payment_channel: option.payment_channel,
    })
    latestCheckout.value = response
    success('套餐订单已创建')
    submitPaymentInstructions(response.payment_instructions)
  } catch (err) {
    log.error('创建套餐订单失败:', err)
    showError(parseApiError(err, '创建套餐订单失败'))
  } finally {
    checkoutLoadingPlanId.value = null
  }
}

function openLatestPayment() {
  submitPaymentInstructions(latestCheckout.value?.payment_instructions || { payment_url: latestPaymentUrl.value })
}

function submitPaymentInstructions(instructions: Record<string, unknown> | null | undefined) {
  if (!instructions) return
  const stripeInstructions = getStripePaymentInstructions(instructions)
  if (stripeInstructions) {
    stripePaymentInstructions.value = instructions
    stripeDialogOpen.value = true
    return
  }
  const paymentUrl = getPaymentInstructionString(instructions, 'payment_url')
  if (!paymentUrl) return
  const paymentParams = instructions.payment_params
  if (paymentParams && typeof paymentParams === 'object' && !Array.isArray(paymentParams)) {
    submitPaymentForm(paymentUrl, paymentParams as Record<string, unknown>)
    return
  }
  const opened = window.open(paymentUrl, '_blank', 'noopener,noreferrer')
  if (!opened) {
    window.location.href = paymentUrl
  }
}

function submitPaymentForm(url: string, params: Record<string, unknown>) {
  const form = document.createElement('form')
  form.action = url
  form.method = 'POST'
  if (!isSafariBrowser()) {
    form.target = '_blank'
  }
  Object.entries(params).forEach(([key, value]) => {
    if (value === null || value === undefined) return
    const input = document.createElement('input')
    input.type = 'hidden'
    input.name = key
    input.value = String(value)
    form.appendChild(input)
  })
  document.body.appendChild(form)
  form.submit()
  document.body.removeChild(form)
}

function isSafariBrowser(): boolean {
  return navigator.userAgent.includes('Safari') && !navigator.userAgent.includes('Chrome')
}

async function handleStripePaymentSuccess() {
  success('支付已完成，正在刷新套餐状态')
  await Promise.all([loadEntitlements(), loadPlans()])
}

function paymentOptionProvider(option: WalletRechargeOption): string {
  return (option.payment_provider || option.provider || option.payment_method || '').trim()
}

function planTitle(planId: string): string {
  return plans.value.find((plan) => plan.id === planId)?.title || planId
}

function hasMatchingActivePlan(plan: BillingPlan): boolean {
  const replacesDailyQuota = hasDailyQuotaEntitlement(plan.entitlements)
  const replacesMembership = hasMembershipEntitlement(plan.entitlements)
  if (!replacesDailyQuota && !replacesMembership) return false
  return activeEntitlements.value.some((item) =>
    (replacesDailyQuota && hasDailyQuotaEntitlement(item.entitlements))
    || (replacesMembership && hasMembershipEntitlement(item.entitlements))
  )
}

function replacementNotice(plan: BillingPlan): string {
  const labels = replacementClassLabels(plan.entitlements)
  if (labels.length === 0) return ''
  if (hasMatchingActivePlan(plan)) {
    return `你已有有效${labels.join('和')}，购买成功后旧同类套餐会自动失效。`
  }
  return `若已有有效${labels.join('和')}，购买成功后旧同类套餐会自动失效。`
}

function entitlementLabels(items: BillingEntitlement[]): string[] {
  return (items || []).map((item) => {
    if (item.type === 'wallet_credit') {
      return `附赠余额 $${Number(item.amount_usd || 0).toFixed(2)}`
    }
    if (item.type === 'daily_quota') {
      return `每日 $${Number(item.daily_quota_usd || 0).toFixed(2)}`
    }
    if (item.type === 'membership_group') {
      return `会员组 ${item.grant_user_groups.join(', ')}`
    }
    return item.type
  })
}

function hasPackageEntitlement(items: BillingEntitlement[] | undefined): boolean {
  return (items || []).some((item) =>
    item.type === 'daily_quota' || item.type === 'membership_group'
  )
}

function hasDailyQuotaEntitlement(items: BillingEntitlement[] | undefined): boolean {
  return (items || []).some((item) => item.type === 'daily_quota')
}

function hasMembershipEntitlement(items: BillingEntitlement[] | undefined): boolean {
  return (items || []).some((item) => item.type === 'membership_group')
}

function replacementClassLabels(items: BillingEntitlement[] | undefined): string[] {
  const labels: string[] = []
  if (hasDailyQuotaEntitlement(items)) labels.push('每日额度套餐')
  if (hasMembershipEntitlement(items)) labels.push('会员权益包')
  return labels
}

function formatDuration(unit: BillingDurationUnit, value: number): string {
  const labels: Record<BillingDurationUnit, string> = {
    day: '天',
    month: '个月',
    year: '年',
    custom: '自定义周期',
  }
  return unit === 'custom' ? `${value} ${labels[unit]}` : `${value}${labels[unit]}`
}

function formatDate(value: string | null | undefined): string {
  if (!value) return '-'
  return new Date(value).toLocaleDateString('zh-CN')
}
</script>
