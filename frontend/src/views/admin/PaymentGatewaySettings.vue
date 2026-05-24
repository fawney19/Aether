<template>
  <PageContainer>
    <PageHeader
      title="支付配置"
      description="配置易支付、支付宝官方、微信支付官方和 Stripe"
    >
      <template #actions>
        <div class="flex items-center gap-2">
          <Button
            variant="outline"
            size="sm"
            :disabled="testing || loading"
            @click="testGateway"
          >
            <PlugZap class="mr-2 h-4 w-4" />
            {{ testing ? '测试中...' : '测试配置' }}
          </Button>
          <Button
            size="sm"
            :disabled="saving || loading"
            @click="saveConfig"
          >
            <Save class="mr-2 h-4 w-4" />
            {{ saving ? '保存中...' : '保存' }}
          </Button>
        </div>
      </template>
    </PageHeader>

    <div class="mt-6 space-y-6">
      <Card class="p-4">
        <div class="grid grid-cols-2 gap-2 md:grid-cols-4">
          <Button
            v-for="provider in providers"
            :key="provider.key"
            :variant="activeProvider === provider.key ? 'default' : 'outline'"
            size="sm"
            @click="selectProvider(provider.key)"
          >
            {{ provider.label }}
          </Button>
        </div>
      </Card>

      <div
        v-if="loading"
        class="py-16"
      >
        <LoadingState message="正在加载支付配置..." />
      </div>

      <template v-else>
        <div class="grid grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-5">
          <Card class="p-5">
            <div class="text-xs uppercase tracking-wider text-muted-foreground">
              网关状态
            </div>
            <div class="mt-3 flex items-center gap-3">
              <Badge :variant="form.enabled ? 'success' : 'secondary'">
                {{ form.enabled ? '已启用' : '未启用' }}
              </Badge>
              <Switch v-model="form.enabled" />
            </div>
          </Card>
          <Card class="p-5">
            <div class="text-xs uppercase tracking-wider text-muted-foreground">
              退款能力
            </div>
            <div class="mt-3 flex items-center gap-3">
              <Badge :variant="form.refund_enabled ? 'success' : 'secondary'">
                {{ form.refund_enabled ? '允许退款' : '关闭退款' }}
              </Badge>
              <Switch
                :model-value="form.refund_enabled"
                @update:model-value="setRefundEnabled"
              />
            </div>
          </Card>
          <Card class="p-5">
            <div class="text-xs uppercase tracking-wider text-muted-foreground">
              用户退款
            </div>
            <div class="mt-3 flex items-center gap-3">
              <Badge :variant="form.allow_user_refund && form.refund_enabled ? 'success' : 'secondary'">
                {{ form.allow_user_refund && form.refund_enabled ? '允许用户退款' : '关闭用户退款' }}
              </Badge>
              <Switch
                :model-value="form.allow_user_refund"
                :disabled="!form.refund_enabled"
                @update:model-value="setAllowUserRefund"
              />
            </div>
          </Card>
          <Card class="p-5">
            <div class="text-xs uppercase tracking-wider text-muted-foreground">
              密钥状态
            </div>
            <div class="mt-3">
              <Badge :variant="hasSecret ? 'success' : 'warning'">
                {{ hasSecret ? '已保存' : '未设置' }}
              </Badge>
            </div>
          </Card>
          <Card class="p-5">
            <div class="text-xs uppercase tracking-wider text-muted-foreground">
              汇率
            </div>
            <div class="mt-2 text-2xl font-semibold tabular-nums">
              1 USD = {{ Number(form.usd_exchange_rate || 0).toFixed(4) }} {{ form.pay_currency }}
            </div>
          </Card>
        </div>

        <CardSection>
          <template #header>
            <div class="flex items-center justify-between">
              <div>
                <div class="flex items-center gap-2">
                  <h3 class="text-lg font-medium leading-6 text-foreground">
                    {{ activeProviderMeta.label }}商户
                  </h3>
                  <div
                    v-if="activeProvider === 'alipay'"
                    ref="paymentHelpRef"
                    class="relative inline-flex"
                  >
                    <button
                      type="button"
                      class="inline-flex h-6 w-6 cursor-pointer list-none items-center justify-center rounded-full border border-border/70 bg-background/60 text-muted-foreground transition hover:border-primary/60 hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 [&::-webkit-details-marker]:hidden"
                      title="支付宝支付模式说明"
                      aria-label="支付宝支付模式说明"
                      :aria-expanded="paymentHelpOpen === 'alipay'"
                      aria-controls="alipay-payment-help"
                      @click.stop="togglePaymentHelp('alipay')"
                    >
                      <CircleHelp class="h-3.5 w-3.5" />
                    </button>
                    <div
                      v-if="paymentHelpOpen === 'alipay'"
                      id="alipay-payment-help"
                      class="absolute left-0 top-full z-[240] mt-2 w-[320px] max-w-[calc(100vw-2rem)] overflow-hidden rounded-xl border border-border/60 bg-card/95 p-0 text-card-foreground shadow-xl shadow-black/5 backdrop-blur supports-[backdrop-filter]:bg-card/90"
                      role="dialog"
                      aria-label="支付宝支付模式说明"
                    >
                      <div class="space-y-4 p-4 text-xs leading-6">
                        <p class="font-medium text-foreground">
                          桌面优先扫码单，失败再走收银台；移动优先手机网站支付。
                        </p>

                        <div class="border-t border-border/60 pt-3">
                          <h4 class="mb-1 font-semibold text-foreground">
                            当面付 / 扫码支付
                          </h4>
                          <p>开通：需开通当面付或扫码支付能力。</p>
                          <p>
                            调用：桌面端下单时优先调用 <code class="rounded bg-muted px-1 py-0.5 text-foreground">alipay.trade.precreate</code>，前台直接渲染二维码。
                          </p>
                          <p>降级：接口不可用或返回失败时，自动降级到电脑网站支付。</p>
                        </div>

                        <div class="border-t border-border/60 pt-3">
                          <h4 class="mb-1 font-semibold text-foreground">
                            电脑网站支付
                          </h4>
                          <p>开通：需开通电脑网站支付。</p>
                          <p>
                            调用：桌面端当面付不可用时调用 <code class="rounded bg-muted px-1 py-0.5 text-foreground">alipay.trade.page.pay</code>，并继续以返回链接渲染成二维码。
                          </p>
                          <p>降级：同时保留打开收银台入口，用户可手动重新拉起支付页。</p>
                        </div>

                        <div class="border-t border-border/60 pt-3">
                          <h4 class="mb-1 font-semibold text-foreground">
                            手机网站支付
                          </h4>
                          <p>开通：需开通手机网站支付。</p>
                          <p>
                            调用：移动端优先调用 <code class="rounded bg-muted px-1 py-0.5 text-foreground">alipay.trade.wap.pay</code>，跳转支付宝收银台。
                          </p>
                          <p>降级：未开通或返回异常时，前端自动改走扫码支付并提示未开通移动支付。</p>
                        </div>
                      </div>
                    </div>
                  </div>
                  <div
                    v-else-if="activeProvider === 'wxpay'"
                    ref="paymentHelpRef"
                    class="relative inline-flex"
                  >
                    <button
                      type="button"
                      class="inline-flex h-6 w-6 cursor-pointer list-none items-center justify-center rounded-full border border-border/70 bg-background/60 text-muted-foreground transition hover:border-primary/60 hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 [&::-webkit-details-marker]:hidden"
                      title="微信支付模式说明"
                      aria-label="微信支付模式说明"
                      :aria-expanded="paymentHelpOpen === 'wxpay'"
                      aria-controls="wxpay-payment-help"
                      @click.stop="togglePaymentHelp('wxpay')"
                    >
                      <CircleHelp class="h-3.5 w-3.5" />
                    </button>
                    <div
                      v-if="paymentHelpOpen === 'wxpay'"
                      id="wxpay-payment-help"
                      class="absolute left-0 top-full z-[240] mt-2 w-[340px] max-w-[calc(100vw-2rem)] overflow-hidden rounded-xl border border-border/60 bg-card/95 p-0 text-card-foreground shadow-xl shadow-black/5 backdrop-blur supports-[backdrop-filter]:bg-card/90"
                      role="dialog"
                      aria-label="微信支付模式说明"
                    >
                      <div class="space-y-4 p-4 text-xs leading-6">
                        <p class="font-medium text-foreground">
                          桌面优先 Native 扫码，移动端微信浏览器优先 JSAPI，非微信浏览器兜底 H5。
                        </p>

                        <div class="border-t border-border/60 pt-3">
                          <h4 class="mb-1 font-semibold text-foreground">
                            Native / 扫码支付
                          </h4>
                          <p>开通：需开通 Native 或扫码支付能力。</p>
                          <p>
                            调用：桌面端默认调用 Native，前台会渲染二维码内容。
                          </p>
                          <p>降级：移动端无法走 JSAPI 或 H5 时，也会自动回退到这里。</p>
                        </div>

                        <div class="border-t border-border/60 pt-3">
                          <h4 class="mb-1 font-semibold text-foreground">
                            JSAPI / 公众号支付
                          </h4>
                          <p>开通：需开通公众号支付，并保证当前浏览器在微信内且能拿到 OpenID。</p>
                          <p>
                            调用：微信内浏览器完成授权后调用 JSAPI，直接拉起微信收银台。
                          </p>
                          <p>降级：未配置或拉起失败时，自动改走扫码支付。</p>
                        </div>

                        <div class="border-t border-border/60 pt-3">
                          <h4 class="mb-1 font-semibold text-foreground">
                            H5 支付
                          </h4>
                          <p>开通：需开通 H5 支付。</p>
                          <p>
                            调用：移动端非微信浏览器且客户端 IP 可用时调用 H5，跳转微信收银台。
                          </p>
                          <p>降级：未开通 H5 或下单失败时，自动改走扫码支付。</p>
                        </div>

                        <p class="border-t border-border/60 pt-3">
                          当前表单默认共用一个 App ID，适合同主体下统一配置网页、移动和公众号场景。
                        </p>
                      </div>
                    </div>
                  </div>
                </div>
                <p class="mt-1 text-sm text-muted-foreground">
                  {{ activeProviderMeta.description }}
                </p>
              </div>
            </div>
          </template>

          <div class="grid grid-cols-1 gap-5 md:grid-cols-2">
            <div
              v-if="activeProvider === 'epay'"
              class="space-y-1.5"
            >
              <Label for="gateway-endpoint">易支付接口地址</Label>
              <Input
                id="gateway-endpoint"
                v-model="form.endpoint_url"
                placeholder="https://pay.example.com/submit.php"
              />
            </div>

            <div
              v-if="activeProvider !== 'stripe'"
              class="space-y-1.5"
            >
              <Label for="gateway-callback-base">回调站点根地址</Label>
              <Input
                id="gateway-callback-base"
                v-model="form.callback_base_url"
                :placeholder="defaultCallbackBaseUrl || 'https://aether.example.com'"
              />
            </div>

            <div
              v-for="field in visibleFields"
              :key="field.key"
              class="space-y-1.5"
            >
              <Label :for="`gateway-field-${field.key}`">
                {{ field.label }}
                <span
                  v-if="field.secret && hasSecretKey(field.key)"
                  class="text-xs font-normal text-muted-foreground"
                >
                  （留空保持不变）
                </span>
              </Label>
              <Input
                :id="`gateway-field-${field.key}`"
                v-model="fieldValues[field.key]"
                :masked="field.secret"
                :placeholder="field.placeholder"
                autocomplete="off"
              />
            </div>
          </div>
        </CardSection>

        <CardSection
          title="计费参数"
          description="用户充值按美元金额下单，实际收款金额按这里的币种、汇率和通道手续费率计算"
        >
          <div class="grid grid-cols-1 gap-5 md:grid-cols-3">
            <div class="space-y-1.5">
              <Label for="gateway-currency">支付币种</Label>
              <Input
                id="gateway-currency"
                v-model="form.pay_currency"
                maxlength="16"
                placeholder="CNY"
              />
            </div>
            <div class="space-y-1.5">
              <Label for="gateway-rate">USD 汇率</Label>
              <Input
                id="gateway-rate"
                v-model.number="form.usd_exchange_rate"
                type="number"
                min="0.0001"
                step="0.0001"
              />
            </div>
            <div class="space-y-1.5">
              <Label for="gateway-min">最低充值金额 (USD)</Label>
              <Input
                id="gateway-min"
                v-model.number="form.min_recharge_usd"
                type="number"
                min="0.01"
                step="0.01"
              />
            </div>
          </div>
        </CardSection>

        <CardSection
          title="支付通道"
          :description="activeProvider === 'epay' ? '通道值会传给易支付 type 字段' : '通道值决定用户侧展示和后端创建订单模式'"
        >
          <template #actions>
            <Button
              variant="outline"
              size="sm"
              @click="addChannel"
            >
              <Plus class="mr-2 h-4 w-4" />
              添加通道
            </Button>
          </template>

          <div class="space-y-3">
            <div
              v-for="(channel, index) in form.channels"
              :key="index"
              class="grid grid-cols-1 gap-3 rounded-lg border border-border/60 bg-muted/20 p-3 md:grid-cols-[1fr_1fr_160px_auto]"
            >
              <div class="space-y-1.5">
                <Label :for="`gateway-channel-${index}`">通道值</Label>
                <Input
                  :id="`gateway-channel-${index}`"
                  v-model="channel.channel"
                  placeholder="alipay"
                />
              </div>
              <div class="space-y-1.5">
                <Label :for="`gateway-channel-name-${index}`">显示名称</Label>
                <Input
                  :id="`gateway-channel-name-${index}`"
                  v-model="channel.display_name"
                  placeholder="支付宝"
                />
              </div>
              <div class="space-y-1.5">
                <Label :for="`gateway-channel-fee-${index}`">手续费率 (%)</Label>
                <Input
                  :id="`gateway-channel-fee-${index}`"
                  v-model.number="channel.fee_rate"
                  type="number"
                  min="0"
                  step="0.01"
                  placeholder="0"
                />
              </div>
              <div class="flex items-end">
                <Button
                  variant="ghost"
                  size="icon"
                  title="移除通道"
                  :disabled="form.channels.length <= 1"
                  @click="removeChannel(index)"
                >
                  <Trash2 class="h-4 w-4" />
                </Button>
              </div>
            </div>
          </div>
        </CardSection>

        <p
          v-if="updatedAtText"
          class="text-xs text-muted-foreground"
        >
          最后更新：{{ updatedAtText }}
        </p>
      </template>
    </div>
  </PageContainer>
</template>

<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, reactive, ref } from 'vue'
import { CircleHelp, PlugZap, Plus, Save, Trash2 } from 'lucide-vue-next'
import { epayGatewayApi, type EpayChannelConfig, type PaymentGatewayProvider } from '@/api/billing'
import {
  Badge,
  Button,
  Card,
  Input,
  Label,
  Switch,
} from '@/components/ui'
import { LoadingState } from '@/components/common'
import { CardSection, PageContainer, PageHeader } from '@/components/layout'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'
import { log } from '@/utils/logger'

type ProviderField = {
  key: string
  label: string
  secret?: boolean
  placeholder?: string
}

const { success, error: showError } = useToast()

const providers: Array<{
  key: PaymentGatewayProvider
  label: string
  description: string
  fields: ProviderField[]
  defaultChannels: EpayChannelConfig[]
}> = [
  {
    key: 'epay',
    label: '易支付',
    description: '密钥留空会保留原密钥；回调地址留空时后端会使用当前 API 访问地址',
    fields: [
      { key: 'merchant_id', label: '商户 ID', placeholder: '1000' },
      { key: 'merchant_key', label: '商户密钥', secret: true, placeholder: '请输入商户密钥' },
    ],
    defaultChannels: [
      { channel: 'alipay', display_name: '支付宝', fee_rate: 0 },
      { channel: 'wxpay', display_name: '微信支付', fee_rate: 0 },
    ],
  },
  {
    key: 'alipay',
    label: '支付宝官方',
    description: '用于支付宝官方当面付、手机网站支付或电脑网站支付',
    fields: [
      { key: 'app_id', label: 'App ID', placeholder: '202100...' },
      { key: 'payment_mode', label: '支付模式', placeholder: 'precreate / page / wap' },
      { key: 'private_key', label: '应用私钥', secret: true, placeholder: 'PKCS#1 或 PKCS#8 私钥' },
      { key: 'alipay_public_key', label: '支付宝公钥', secret: true, placeholder: '支付宝开放平台公钥' },
    ],
    defaultChannels: [{ channel: 'alipay', display_name: '支付宝官方', fee_rate: 0 }],
  },
  {
    key: 'wxpay',
    label: '微信支付官方',
    description: '用于微信支付 Native/H5，JSAPI 还需要后续接入 OpenID 获取流程',
    fields: [
      { key: 'app_id', label: 'App ID', placeholder: 'wx...' },
      { key: 'mch_id', label: '商户号', placeholder: '1900000000' },
      { key: 'cert_serial', label: '商户证书序列号', placeholder: '证书序列号' },
      { key: 'public_key_id', label: '微信支付公钥 ID', placeholder: 'PUB_KEY_ID_...' },
      { key: 'private_key', label: '商户 API 私钥', secret: true, placeholder: 'BEGIN PRIVATE KEY' },
      { key: 'api_v3_key', label: 'API v3 密钥', secret: true, placeholder: '32 位 API v3 key' },
      { key: 'public_key', label: '微信支付公钥', secret: true, placeholder: 'BEGIN PUBLIC KEY' },
    ],
    defaultChannels: [
      { channel: 'native', display_name: '微信 Native', fee_rate: 0 },
      { channel: 'h5', display_name: '微信 H5', fee_rate: 0 },
    ],
  },
  {
    key: 'stripe',
    label: 'Stripe',
    description: '用于 Stripe PaymentIntent；Webhook Secret 用于回调验签',
    fields: [
      { key: 'publishable_key', label: 'Publishable Key', placeholder: 'pk_live_...' },
      { key: 'secret_key', label: 'Secret Key', secret: true, placeholder: 'sk_live_...' },
      { key: 'webhook_secret', label: 'Webhook Secret', secret: true, placeholder: 'whsec_...' },
    ],
    defaultChannels: [
      { channel: 'card', display_name: 'Card', fee_rate: 0 },
      { channel: 'alipay', display_name: 'Alipay', fee_rate: 0 },
      { channel: 'wechat_pay', display_name: 'WeChat Pay', fee_rate: 0 },
      { channel: 'link', display_name: 'Link', fee_rate: 0 },
    ],
  },
]

const activeProvider = ref<PaymentGatewayProvider>('epay')
const loading = ref(true)
const saving = ref(false)
const testing = ref(false)
const hasSecret = ref(false)
const hasSecretKeys = ref<string[]>([])
const updatedAt = ref<number | null>(null)
const fieldValues = reactive<Record<string, string>>({})
const paymentHelpOpen = ref<PaymentGatewayProvider | null>(null)
const paymentHelpRef = ref<HTMLElement | null>(null)

const form = reactive({
  enabled: false,
  endpoint_url: '',
  callback_base_url: '',
  pay_currency: 'CNY',
  usd_exchange_rate: 7.2,
  min_recharge_usd: 1,
  refund_enabled: false,
  allow_user_refund: false,
  channels: providers[0].defaultChannels.map((item) => ({ ...item })) as EpayChannelConfig[],
})

const activeProviderMeta = computed(() =>
  providers.find(provider => provider.key === activeProvider.value) || providers[0]
)

const visibleFields = computed(() => activeProviderMeta.value.fields)

const updatedAtText = computed(() => {
  if (!updatedAt.value) return ''
  return new Date(updatedAt.value * 1000).toLocaleString('zh-CN')
})

const defaultCallbackBaseUrl = computed(() => {
  if (typeof window === 'undefined') return ''
  return window.location.origin
})

onMounted(() => {
  document.addEventListener('pointerdown', handlePaymentHelpOutsidePointerDown, true)
  void loadConfig()
})

onBeforeUnmount(() => {
  document.removeEventListener('pointerdown', handlePaymentHelpOutsidePointerDown, true)
})

async function selectProvider(provider: PaymentGatewayProvider) {
  if (activeProvider.value === provider) return
  closePaymentHelp()
  activeProvider.value = provider
  await loadConfig()
}

function togglePaymentHelp(provider: PaymentGatewayProvider) {
  paymentHelpOpen.value = paymentHelpOpen.value === provider ? null : provider
}

function closePaymentHelp() {
  paymentHelpOpen.value = null
}

function handlePaymentHelpOutsidePointerDown(event: PointerEvent) {
  const target = event.target
  if (!(target instanceof Node)) return
  if (paymentHelpRef.value?.contains(target)) return
  closePaymentHelp()
}

async function loadConfig() {
  loading.value = true
  try {
    const config = await epayGatewayApi.get(activeProvider.value)
    form.enabled = config.enabled
    form.endpoint_url = config.endpoint_url || ''
    form.callback_base_url = config.callback_base_url || ''
    form.pay_currency = config.pay_currency || 'CNY'
    form.usd_exchange_rate = Number(config.usd_exchange_rate || 7.2)
    form.min_recharge_usd = Number(config.min_recharge_usd || 1)
    form.refund_enabled = Boolean(config.refund_enabled)
    form.allow_user_refund = form.refund_enabled && Boolean(config.allow_user_refund)
    form.channels = config.channels?.length
      ? config.channels.map((item) => {
        const feeRate = Number(item.fee_rate ?? 0)
        return { ...item, fee_rate: Number.isFinite(feeRate) && feeRate >= 0 ? feeRate : 0 }
      })
      : activeProviderMeta.value.defaultChannels.map((item) => ({ ...item }))
    hasSecret.value = config.has_secret
    hasSecretKeys.value = config.has_secret_keys || []
    updatedAt.value = config.updated_at ?? null

    resetFieldValues()
    const savedConfig = config.config || {}
    for (const field of visibleFields.value) {
      if (field.key === 'merchant_id') {
        fieldValues[field.key] = config.merchant_id || ''
      } else {
        const value = savedConfig[field.key]
        fieldValues[field.key] = typeof value === 'string' ? value : ''
      }
    }
  } catch (err) {
    log.error('加载支付配置失败:', err)
    showError(parseApiError(err, '加载支付配置失败'))
  } finally {
    loading.value = false
  }
}

function resetFieldValues() {
  for (const key of Object.keys(fieldValues)) {
    delete fieldValues[key]
  }
  for (const field of visibleFields.value) {
    fieldValues[field.key] = ''
  }
}

function hasSecretKey(key: string): boolean {
  if (activeProvider.value === 'epay' && key === 'merchant_key') return hasSecret.value
  return hasSecretKeys.value.includes(key)
}

function setRefundEnabled(value: boolean) {
  form.refund_enabled = value
  if (!value) form.allow_user_refund = false
}

function setAllowUserRefund(value: boolean) {
  form.allow_user_refund = form.refund_enabled && value
}

function normalizeChannels(): EpayChannelConfig[] {
  return form.channels
    .map((item) => {
      const feeRate = Number(item.fee_rate ?? 0)
      return {
        channel: item.channel.trim(),
        display_name: item.display_name.trim(),
        fee_rate: Number.isFinite(feeRate) && feeRate >= 0 ? feeRate : 0,
      }
    })
    .filter((item) => item.channel && item.display_name)
}

function validateForm(): string | null {
  if (activeProvider.value === 'epay' && !form.endpoint_url.trim()) return '请输入易支付接口地址'
  if (!form.pay_currency.trim()) return '请输入支付币种'
  if (!Number.isFinite(Number(form.usd_exchange_rate)) || Number(form.usd_exchange_rate) <= 0) {
    return 'USD 汇率必须大于 0'
  }
  if (!Number.isFinite(Number(form.min_recharge_usd)) || Number(form.min_recharge_usd) <= 0) {
    return '最低充值金额必须大于 0'
  }
  const channels = normalizeChannels()
  if (channels.length === 0) return '至少需要一个支付通道'
  for (const [index, channel] of form.channels.entries()) {
    if (!channel.channel.trim() || !channel.display_name.trim()) continue
    const feeRate = Number(channel.fee_rate ?? 0)
    if (!Number.isFinite(feeRate) || feeRate < 0) {
      return `第 ${index + 1} 个通道手续费率必须大于等于 0`
    }
  }
  for (const field of visibleFields.value) {
    const value = fieldValues[field.key]?.trim() || ''
    if (!field.secret && !value) return `请输入${field.label}`
    if (field.secret && !hasSecretKey(field.key) && !value) return `首次配置需要填写${field.label}`
  }
  return null
}

async function saveConfig() {
  const validationError = validateForm()
  if (validationError) {
    showError(validationError)
    return
  }

  saving.value = true
  try {
    const configFields: Record<string, unknown> = {}
    const secrets: Record<string, string> = {}
    for (const field of visibleFields.value) {
      const value = fieldValues[field.key]?.trim() || ''
      if (field.secret) {
        if (value) secrets[field.key] = value
      } else if (field.key !== 'merchant_id') {
        configFields[field.key] = value
      }
    }
    const payload = {
      enabled: form.enabled,
      endpoint_url: form.endpoint_url.trim(),
      callback_base_url: form.callback_base_url.trim() || null,
      merchant_id: fieldValues.merchant_id?.trim() || '',
      pay_currency: form.pay_currency.trim().toUpperCase(),
      usd_exchange_rate: Number(form.usd_exchange_rate),
      min_recharge_usd: Number(form.min_recharge_usd),
      channels: normalizeChannels(),
      refund_enabled: form.refund_enabled,
      allow_user_refund: form.refund_enabled && form.allow_user_refund,
      config: configFields,
      secrets,
      ...(activeProvider.value === 'epay' && fieldValues.merchant_key?.trim()
        ? { merchant_key: fieldValues.merchant_key.trim() }
        : {}),
    }
    const config = await epayGatewayApi.update(payload, activeProvider.value)
    hasSecret.value = config.has_secret
    hasSecretKeys.value = config.has_secret_keys || []
    updatedAt.value = config.updated_at ?? null
    form.refund_enabled = Boolean(config.refund_enabled)
    form.allow_user_refund = form.refund_enabled && Boolean(config.allow_user_refund)
    fieldValues.merchant_key = ''
    for (const field of visibleFields.value) {
      if (field.secret) fieldValues[field.key] = ''
    }
    success('支付配置已保存')
  } catch (err) {
    log.error('保存支付配置失败:', err)
    showError(parseApiError(err, '保存支付配置失败'))
  } finally {
    saving.value = false
  }
}

async function testGateway() {
  testing.value = true
  try {
    await epayGatewayApi.test(activeProvider.value)
    success('支付配置可用')
  } catch (err) {
    log.error('测试支付配置失败:', err)
    showError(parseApiError(err, '测试支付配置失败'))
  } finally {
    testing.value = false
  }
}

function addChannel() {
  form.channels.push({ channel: '', display_name: '', fee_rate: 0 })
}

function removeChannel(index: number) {
  if (form.channels.length <= 1) return
  form.channels.splice(index, 1)
}
</script>
