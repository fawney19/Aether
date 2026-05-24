<template>
  <Dialog
    v-model:open="dialogOpen"
    max-width="2xl"
    :persistent="initializing || submitting"
    :close-on-backdrop="!(initializing || submitting)"
  >
    <template #header>
      <DialogHeader>
        <div class="flex items-start justify-between gap-3">
          <div class="min-w-0">
            <DialogTitle>
              {{ title }}
            </DialogTitle>
            <DialogDescription>
              {{ description }}
            </DialogDescription>
          </div>
          <Badge
            variant="outline"
            class="shrink-0"
          >
            Stripe
          </Badge>
        </div>
      </DialogHeader>
    </template>

    <div class="space-y-4">
      <div class="rounded-xl border border-border/60 bg-muted/20 p-4">
        <div class="grid grid-cols-1 gap-3 sm:grid-cols-2">
          <div class="space-y-1">
            <div class="text-xs text-muted-foreground">
              PaymentIntent
            </div>
            <div class="break-all font-mono text-sm text-foreground">
              {{ stripeIntentId || stripeGatewayOrderId || '-' }}
            </div>
          </div>
          <div class="space-y-1">
            <div class="text-xs text-muted-foreground">
              支付方式
            </div>
            <div class="text-sm text-foreground">
              {{ stripeDisplayName }}
            </div>
          </div>
          <div class="space-y-1">
            <div class="text-xs text-muted-foreground">
              应付金额
            </div>
            <div class="text-sm text-foreground">
              {{ stripeAmountLabel }}
            </div>
          </div>
          <div class="space-y-1">
            <div class="text-xs text-muted-foreground">
              支付通道
            </div>
            <div class="flex flex-wrap gap-1.5">
              <Badge
                v-for="method in stripePaymentMethodTypes"
                :key="method"
                variant="outline"
              >
                {{ paymentMethodTypeLabel(method) }}
              </Badge>
              <span
                v-if="stripePaymentMethodTypes.length === 0"
                class="text-sm text-muted-foreground"
              >
                -
              </span>
            </div>
          </div>
        </div>
      </div>

      <div class="relative rounded-xl border border-border/60 bg-background p-3">
        <div
          ref="paymentElementRoot"
          class="min-h-[360px]"
          :class="{ 'pointer-events-none opacity-20': initializing }"
        />
        <div
          v-if="initializing"
          class="absolute inset-3 flex items-center justify-center gap-2 rounded-lg bg-background/80 text-sm text-muted-foreground backdrop-blur-sm"
        >
          <Loader2 class="h-4 w-4 animate-spin" />
          正在加载 Stripe 支付组件...
        </div>
      </div>

      <div
        v-if="errorMessage"
        class="rounded-xl border border-rose-500/30 bg-rose-500/10 px-4 py-3 text-sm text-rose-700 dark:text-rose-300"
      >
        {{ errorMessage }}
      </div>
    </div>

    <template #footer>
      <Button
        variant="outline"
        :disabled="initializing || submitting"
        @click="closeDialog"
      >
        关闭
      </Button>
      <Button
        :disabled="!canSubmit"
        @click="submitPayment"
      >
        <Loader2
          v-if="submitting"
          class="mr-2 h-4 w-4 animate-spin"
        />
        {{ submitting ? '支付中...' : confirmText }}
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue'
import { loadStripe, type Stripe, type StripeElements, type StripePaymentElement } from '@stripe/stripe-js'
import { Loader2 } from 'lucide-vue-next'
import { Badge, Button, Dialog, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui'
import {
  getStripePaymentInstructions,
  type PaymentInstructionMap,
} from '@/utils/paymentInstructions'
import { paymentMethodLabel } from '@/utils/walletDisplay'

interface Props {
  open: boolean
  instructions: PaymentInstructionMap | null
  title?: string
  description?: string
  confirmText?: string
  returnUrl?: string
}

const props = withDefaults(defineProps<Props>(), {
  title: 'Stripe 支付',
  description: '请在弹窗内完成支付，成功后系统会自动入账。',
  confirmText: '确认支付',
  returnUrl: '',
})

const emit = defineEmits<{
  'update:open': [value: boolean]
  success: [payload: { intentId: string; status?: string | null }]
}>()

const dialogOpen = computed({
  get: () => props.open,
  set: value => emit('update:open', value),
})

const paymentElementRoot = ref<HTMLDivElement | null>(null)
const stripeInstance = ref<Stripe | null>(null)
const elementsInstance = ref<StripeElements | null>(null)
const paymentElement = ref<StripePaymentElement | null>(null)
const initializing = ref(false)
const submitting = ref(false)
const errorMessage = ref('')
const mountedSignature = ref('')
let mountSequence = 0

const stripeInstructions = computed(() => getStripePaymentInstructions(props.instructions))
const stripeIntentId = computed(() => stripeInstructions.value?.intentId || '')
const stripeGatewayOrderId = computed(() => stripeInstructions.value?.gatewayOrderId || '')
const stripeDisplayName = computed(() => stripeInstructions.value?.displayName || 'Stripe')
const stripePaymentMethodTypes = computed(() => stripeInstructions.value?.paymentMethodTypes || [])
const stripeAmountLabel = computed(() => {
  const amount = stripeInstructions.value?.payAmount
  if (typeof amount !== 'number' || !Number.isFinite(amount)) {
    return '-'
  }
  const currency = stripeInstructions.value?.payCurrency || ''
  return `${amount.toFixed(2)}${currency ? ` ${currency}` : ''}`
})

const canSubmit = computed(() =>
  Boolean(
    stripeInstance.value
    && elementsInstance.value
    && stripeInstructions.value
    && !initializing.value
    && !submitting.value
  )
)

const stripeLoaderCache = new Map<string, Promise<Stripe | null>>()

watch(
  [
    () => props.open,
    () => stripeInstructions.value?.clientSecret || '',
    () => stripeInstructions.value?.publishableKey || '',
  ],
  async ([open]) => {
    if (!open) {
      cleanupStripe()
      return
    }
    await initializeStripe()
  },
  { immediate: true }
)

onBeforeUnmount(() => {
  cleanupStripe()
})

async function initializeStripe() {
  const instructions = stripeInstructions.value
  if (!props.open) return
  if (!instructions) {
    errorMessage.value = '缺少 Stripe 支付参数'
    return
  }

  const signature = [
    instructions.publishableKey,
    instructions.clientSecret,
    instructions.intentId,
    instructions.paymentChannel,
  ].join('::')

  if (mountedSignature.value === signature && stripeInstance.value && elementsInstance.value && paymentElement.value) {
    return
  }

  cleanupStripe()
  if (!props.open) return

  initializing.value = true
  errorMessage.value = ''
  const sequence = ++mountSequence

  try {
    await nextTick()
    if (!paymentElementRoot.value) {
      throw new Error('支付容器未准备好')
    }

    const stripe = await loadStripeCached(instructions.publishableKey)
    if (!stripe) {
      throw new Error('Stripe 初始化失败')
    }
    if (sequence !== mountSequence) return

    const elements = stripe.elements({
      clientSecret: instructions.clientSecret,
    })
    const element = elements.create('payment', {
      layout: 'tabs',
    })
    element.mount(paymentElementRoot.value)

    stripeInstance.value = stripe
    elementsInstance.value = elements
    paymentElement.value = element
    mountedSignature.value = signature
  } catch (error) {
    if (sequence !== mountSequence) return
    errorMessage.value = formatStripeError(error)
  } finally {
    if (sequence === mountSequence) {
      initializing.value = false
    }
  }
}

async function submitPayment() {
  const instructions = stripeInstructions.value
  if (!instructions || !stripeInstance.value || !elementsInstance.value) {
    errorMessage.value = 'Stripe 支付组件未就绪'
    return
  }

  submitting.value = true
  errorMessage.value = ''

  try {
    const submitResult = await elementsInstance.value.submit()
    if (submitResult.error) {
      errorMessage.value = submitResult.error.message || '请检查支付信息'
      return
    }

    const { error, paymentIntent } = await stripeInstance.value.confirmPayment({
      elements: elementsInstance.value,
      confirmParams: {
        return_url: props.returnUrl || buildReturnUrl(),
      },
      redirect: 'if_required',
    })

    if (error) {
      errorMessage.value = error.message || '支付失败'
      return
    }

    const intentId = paymentIntent?.id || instructions.intentId || instructions.gatewayOrderId || ''
    if (paymentIntent?.status === 'succeeded' || paymentIntent?.status === 'processing') {
      emit('success', {
        intentId,
        status: paymentIntent.status,
      })
      emit('update:open', false)
      return
    }

    errorMessage.value = paymentIntent?.status
      ? `当前支付状态: ${paymentIntent.status}`
      : '支付已提交，请稍后刷新订单状态'
  } catch (error) {
    errorMessage.value = formatStripeError(error)
  } finally {
    submitting.value = false
  }
}

function closeDialog() {
  emit('update:open', false)
}

function cleanupStripe() {
  mountSequence += 1
  initializing.value = false
  submitting.value = false
  errorMessage.value = ''
  mountedSignature.value = ''

  try {
    paymentElement.value?.unmount()
  } catch {
    // ignore
  }

  paymentElement.value = null
  elementsInstance.value = null
  stripeInstance.value = null

  if (paymentElementRoot.value) {
    paymentElementRoot.value.innerHTML = ''
  }
}

async function loadStripeCached(publishableKey: string): Promise<Stripe | null> {
  if (!stripeLoaderCache.has(publishableKey)) {
    stripeLoaderCache.set(publishableKey, loadStripe(publishableKey))
  }
  return stripeLoaderCache.get(publishableKey) || null
}

function formatStripeError(error: unknown): string {
  if (error instanceof Error && error.message) {
    return error.message
  }
  if (typeof error === 'string' && error.trim()) {
    return error.trim()
  }
  if (typeof error === 'object' && error && 'message' in error) {
    const message = (error as { message?: unknown }).message
    if (typeof message === 'string' && message.trim()) {
      return message.trim()
    }
  }
  return 'Stripe 支付处理失败'
}

function paymentMethodTypeLabel(method: string): string {
  const labels: Record<string, string> = {
    card: '银行卡/信用卡',
    alipay: '支付宝',
    wechat_pay: '微信支付',
    link: 'Link',
    us_bank_account: '美国银行账户',
  }
  return labels[method] || paymentMethodLabel(method) || method
}

function buildReturnUrl(): string {
  if (typeof window === 'undefined') return ''
  const url = new URL(window.location.href)
  url.hash = ''
  url.searchParams.set('stripe_return', '1')
  return url.toString()
}
</script>
