<template>
  <div class="space-y-6 pb-8">
    <div
      v-if="loadingInitial"
      class="py-16"
    >
      <LoadingState message="正在加载钱包数据..." />
    </div>

    <template v-else>
      <div class="grid grid-cols-1 lg:grid-cols-3 gap-4">
        <Card class="p-5 space-y-2">
          <div class="text-xs uppercase tracking-wider text-muted-foreground">
            可用余额
          </div>
          <div class="text-3xl font-bold tabular-nums">
            {{ formatCurrency(walletBalance?.balance) }}
          </div>
          <div class="text-xs text-muted-foreground">
            充值余额: {{ formatCurrency(walletBalance?.wallet?.recharge_balance) }} · 赠款余额: {{ formatCurrency(walletBalance?.wallet?.gift_balance) }}
          </div>
        </Card>

        <Card class="p-5 space-y-2">
          <div class="text-xs uppercase tracking-wider text-muted-foreground">
            累计充值 / 消费
          </div>
          <div class="text-lg font-semibold tabular-nums">
            {{ formatCurrency(walletBalance?.wallet?.total_recharged) }}
            <span class="text-muted-foreground font-normal mx-1">/</span>
            {{ formatCurrency(walletBalance?.wallet?.total_consumed) }}
          </div>
          <div class="text-xs text-muted-foreground">
            累计退款: {{ formatCurrency(walletBalance?.wallet?.total_refunded) }} · 可退款余额: {{ formatCurrency(walletBalance?.wallet?.refundable_balance) }}
          </div>
        </Card>

        <Card class="p-5 space-y-2">
          <div class="text-xs uppercase tracking-wider text-muted-foreground">
            钱包状态
          </div>
          <div class="flex items-center gap-2">
            <Badge :variant="walletStatusBadge(walletBalance?.wallet?.status)">
              {{ walletStatusLabel(walletBalance?.wallet?.status) }}
            </Badge>
          </div>
          <div
            v-if="walletBalance?.unlimited"
            class="text-xs text-amber-600 dark:text-amber-400"
          >
            当前账号处于无限制模式，余额仅用于账务统计。
          </div>
          <div class="text-xs text-muted-foreground">
            待处理退款: {{ walletBalance?.pending_refund_count || 0 }}
          </div>
        </Card>
      </div>

      <!-- TODO(wallet): 充值/退款用户主动操作入口暂未启用，待支付链路联调完成后再开放 -->
      <div
        v-if="ENABLE_WALLET_ACTION_FORMS"
        class="grid grid-cols-1 xl:grid-cols-2 gap-4"
      >
        <Card class="p-5 space-y-4">
          <div class="flex items-center justify-between">
            <h3 class="text-base font-semibold">
              发起充值
            </h3>
            <RefreshButton
              :loading="loadingOrders"
              @click="loadOrders"
            />
          </div>

          <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
            <div class="space-y-1.5">
              <Label>充值金额 (USD)</Label>
              <Input
                v-model.number="rechargeForm.amount_usd"
                type="number"
                min="0.01"
                step="0.01"
                placeholder="10"
              />
            </div>

            <div class="space-y-1.5">
              <Label>支付方式</Label>
              <Select v-model="rechargeForm.payment_method">
                <SelectTrigger>
                  <SelectValue placeholder="选择支付方式" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="alipay">
                    支付宝
                  </SelectItem>
                  <SelectItem value="wechat">
                    微信支付
                  </SelectItem>
                </SelectContent>
              </Select>
            </div>
          </div>

          <Button
            class="w-full"
            :disabled="submittingRecharge"
            @click="submitRecharge"
          >
            {{ submittingRecharge ? '创建订单中...' : '创建充值订单' }}
          </Button>

          <div
            v-if="latestRecharge"
            class="rounded-xl border border-border/60 bg-muted/30 p-3 space-y-1.5"
          >
            <div class="text-xs text-muted-foreground">
              最新订单: <span class="font-medium text-foreground">{{ latestRecharge.order.order_no }}</span>
            </div>
            <div class="text-xs text-muted-foreground">
              状态:
              <Badge
                :variant="paymentStatusBadge(latestRecharge.order.status)"
                class="ml-1"
              >
                {{ paymentStatusLabel(latestRecharge.order.status) }}
              </Badge>
            </div>
            <a
              v-if="latestRecharge.payment_instructions?.payment_url"
              class="inline-flex text-xs text-primary hover:underline"
              :href="String(latestRecharge.payment_instructions.payment_url)"
              target="_blank"
              rel="noopener noreferrer"
            >
              打开支付链接
            </a>
            <div
              v-if="latestRecharge.payment_instructions?.qr_code"
              class="text-xs text-muted-foreground break-all"
            >
              二维码标识: {{ latestRecharge.payment_instructions.qr_code }}
            </div>
          </div>
        </Card>

        <Card class="p-5 space-y-4">
          <div class="flex items-center justify-between">
            <h3 class="text-base font-semibold">
              申请退款
            </h3>
            <RefreshButton
              :loading="loadingRefunds"
              @click="loadRefunds"
            />
          </div>

          <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
            <div class="space-y-1.5">
              <Label>退款金额 (USD)</Label>
              <Input
                v-model.number="refundForm.amount_usd"
                type="number"
                min="0.01"
                step="0.01"
                placeholder="5"
              />
            </div>

            <div class="space-y-1.5">
              <Label>退款模式</Label>
              <Select v-model="refundForm.refund_mode">
                <SelectTrigger>
                  <SelectValue placeholder="选择退款模式" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="original_channel">
                    原路退回
                  </SelectItem>
                  <SelectItem value="offline_payout">
                    线下打款
                  </SelectItem>
                </SelectContent>
              </Select>
            </div>
          </div>

          <div class="space-y-1.5">
            <Label>关联充值订单（可选）</Label>
            <Select v-model="refundForm.payment_order_id">
              <SelectTrigger>
                <SelectValue placeholder="不指定订单，直接从钱包余额退款" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="__none__">
                  不指定
                </SelectItem>
                <SelectItem
                  v-for="order in refundableOrders"
                  :key="order.id"
                  :value="order.id"
                >
                  {{ order.order_no }} (可退 {{ formatCurrency(order.refundable_amount_usd) }})
                </SelectItem>
              </SelectContent>
            </Select>
          </div>

          <div class="space-y-1.5">
            <Label>退款原因（可选）</Label>
            <Textarea
              v-model="refundForm.reason"
              placeholder="填写退款原因，便于审核"
              rows="3"
            />
          </div>

          <div class="rounded-xl border border-border/60 bg-muted/20 p-3 text-xs text-muted-foreground">
            仅充值余额可退款，赠款余额不可退款。
          </div>

          <Button
            class="w-full"
            variant="outline"
            :disabled="submittingRefund"
            @click="submitRefund"
          >
            {{ submittingRefund ? '提交中...' : '提交退款申请' }}
          </Button>
        </Card>
      </div>

      <Card class="overflow-hidden">
        <div class="px-5 pt-5 pb-2">
          <Tabs v-model="activeTab">
            <TabsList class="tabs-button-list grid grid-cols-3 w-full max-w-xl">
              <TabsTrigger value="transactions">
                资金流水
              </TabsTrigger>
              <TabsTrigger value="orders">
                充值订单
              </TabsTrigger>
              <TabsTrigger value="refunds">
                退款记录
              </TabsTrigger>
            </TabsList>

            <TabsContent
              value="transactions"
              class="mt-4 space-y-4"
            >
              <div class="px-5 flex items-center justify-between">
                <div class="text-sm text-muted-foreground">
                  共 {{ txTotal }} 条
                </div>
                <RefreshButton
                  :loading="loadingTransactions"
                  @click="loadTransactions"
                />
              </div>
              <div class="overflow-x-auto">
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead>时间</TableHead>
                      <TableHead>类型</TableHead>
                      <TableHead>变动</TableHead>
                      <TableHead>余额变化</TableHead>
                      <TableHead>说明</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    <TableRow
                      v-for="tx in transactions"
                      :key="tx.id"
                    >
                      <TableCell class="text-xs text-muted-foreground">
                        {{ formatDateTime(tx.created_at) }}
                      </TableCell>
                      <TableCell>
                        <div class="space-y-1">
                          <Badge
                            variant="outline"
                            class="font-mono"
                          >
                            {{ walletTransactionCategoryLabel(tx.category) }}
                          </Badge>
                          <div class="text-[11px] text-muted-foreground">
                            {{ walletTransactionReasonLabel(tx.reason_code) }}
                          </div>
                        </div>
                      </TableCell>
                      <TableCell
                        :class="tx.amount >= 0 ? 'text-emerald-600 dark:text-emerald-400' : 'text-rose-600 dark:text-rose-400'"
                      >
                        {{ tx.amount >= 0 ? '+' : '' }}{{ tx.amount.toFixed(4) }}
                      </TableCell>
                      <TableCell class="text-xs tabular-nums">
                        {{ tx.balance_before.toFixed(4) }} → {{ tx.balance_after.toFixed(4) }}
                      </TableCell>
                      <TableCell class="text-xs text-muted-foreground">
                        {{ tx.description || '-' }}
                      </TableCell>
                    </TableRow>
                    <TableRow v-if="!loadingTransactions && transactions.length === 0">
                      <TableCell
                        colspan="5"
                        class="py-10"
                      >
                        <EmptyState
                          title="暂无资金流水"
                          description="充值或退款后会在这里显示"
                        />
                      </TableCell>
                    </TableRow>
                  </TableBody>
                </Table>
              </div>
              <Pagination
                :current="txPage"
                :total="txTotal"
                :page-size="txPageSize"
                @update:current="handleTxPageChange"
                @update:page-size="handleTxPageSizeChange"
              />
            </TabsContent>

            <TabsContent
              value="orders"
              class="mt-4 space-y-4"
            >
              <div class="px-5 flex items-center justify-between">
                <div class="text-sm text-muted-foreground">
                  共 {{ orderTotal }} 条
                </div>
                <RefreshButton
                  :loading="loadingOrders"
                  @click="loadOrders"
                />
              </div>
              <div class="overflow-x-auto">
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead>订单号</TableHead>
                      <TableHead>金额</TableHead>
                      <TableHead>支付方式</TableHead>
                      <TableHead>状态</TableHead>
                      <TableHead>可退金额</TableHead>
                      <TableHead>创建时间</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    <TableRow
                      v-for="order in rechargeOrders"
                      :key="order.id"
                    >
                      <TableCell class="font-mono text-xs">
                        {{ order.order_no }}
                      </TableCell>
                      <TableCell class="tabular-nums">
                        {{ formatCurrency(order.amount_usd) }}
                      </TableCell>
                      <TableCell>{{ paymentMethodLabel(order.payment_method) }}</TableCell>
                      <TableCell>
                        <Badge :variant="paymentStatusBadge(order.status)">
                          {{ paymentStatusLabel(order.status) }}
                        </Badge>
                      </TableCell>
                      <TableCell class="tabular-nums">
                        {{ formatCurrency(order.refundable_amount_usd) }}
                      </TableCell>
                      <TableCell class="text-xs text-muted-foreground">
                        {{ formatDateTime(order.created_at) }}
                      </TableCell>
                    </TableRow>
                    <TableRow v-if="!loadingOrders && rechargeOrders.length === 0">
                      <TableCell
                        colspan="6"
                        class="py-10"
                      >
                        <EmptyState
                          title="暂无充值订单"
                          description="发起充值后会在这里显示"
                        />
                      </TableCell>
                    </TableRow>
                  </TableBody>
                </Table>
              </div>
              <Pagination
                :current="orderPage"
                :total="orderTotal"
                :page-size="orderPageSize"
                @update:current="handleOrderPageChange"
                @update:page-size="handleOrderPageSizeChange"
              />
            </TabsContent>

            <TabsContent
              value="refunds"
              class="mt-4 space-y-4"
            >
              <div class="px-5 flex items-center justify-between">
                <div class="text-sm text-muted-foreground">
                  共 {{ refundTotal }} 条
                </div>
                <RefreshButton
                  :loading="loadingRefunds"
                  @click="loadRefunds"
                />
              </div>
              <div class="overflow-x-auto">
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead>退款单号</TableHead>
                      <TableHead>金额</TableHead>
                      <TableHead>模式</TableHead>
                      <TableHead>状态</TableHead>
                      <TableHead>原因</TableHead>
                      <TableHead>申请时间</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    <TableRow
                      v-for="refund in refunds"
                      :key="refund.id"
                    >
                      <TableCell class="font-mono text-xs">
                        {{ refund.refund_no }}
                      </TableCell>
                      <TableCell class="tabular-nums">
                        {{ formatCurrency(refund.amount_usd) }}
                      </TableCell>
                      <TableCell>{{ refundModeLabel(refund.refund_mode) }}</TableCell>
                      <TableCell>
                        <Badge :variant="refundStatusBadge(refund.status)">
                          {{ refundStatusLabel(refund.status) }}
                        </Badge>
                      </TableCell>
                      <TableCell class="text-xs text-muted-foreground max-w-[220px] truncate">
                        {{ refund.reason || refund.failure_reason || '-' }}
                      </TableCell>
                      <TableCell class="text-xs text-muted-foreground">
                        {{ formatDateTime(refund.created_at) }}
                      </TableCell>
                    </TableRow>
                    <TableRow v-if="!loadingRefunds && refunds.length === 0">
                      <TableCell
                        colspan="6"
                        class="py-10"
                      >
                        <EmptyState
                          title="暂无退款记录"
                          description="提交退款申请后会在这里显示"
                        />
                      </TableCell>
                    </TableRow>
                  </TableBody>
                </Table>
              </div>
              <Pagination
                :current="refundPage"
                :total="refundTotal"
                :page-size="refundPageSize"
                @update:current="handleRefundPageChange"
                @update:page-size="handleRefundPageSizeChange"
              />
            </TabsContent>
          </Tabs>
        </div>
      </Card>
    </template>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import {
  Badge,
  Button,
  Card,
  Input,
  Label,
  Pagination,
  RefreshButton,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
  Textarea,
} from '@/components/ui'
import { EmptyState, LoadingState } from '@/components/common'
import {
  walletApi,
  type PaymentOrder,
  type RefundRequest,
  type WalletBalanceResponse,
  type WalletTransaction,
} from '@/api/wallet'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'
import { log } from '@/utils/logger'
import {
  formatWalletCurrency as formatCurrency,
  paymentMethodLabel,
  paymentStatusBadge,
  paymentStatusLabel,
  refundModeLabel,
  refundStatusBadge,
  refundStatusLabel,
  walletStatusBadge,
  walletStatusLabel,
  walletTransactionCategoryLabel,
  walletTransactionReasonLabel,
} from '@/utils/walletDisplay'

const { success, error: showError } = useToast()

// TODO(wallet): 充值和退款前台入口尚未正式启用；联调完成后改为 true 即可恢复显示。
const ENABLE_WALLET_ACTION_FORMS = false

const loadingInitial = ref(true)
const loadingTransactions = ref(false)
const loadingOrders = ref(false)
const loadingRefunds = ref(false)
const submittingRecharge = ref(false)
const submittingRefund = ref(false)

const walletBalance = ref<WalletBalanceResponse | null>(null)
const latestRecharge = ref<{ order: PaymentOrder; payment_instructions: Record<string, unknown> } | null>(null)

const transactions = ref<WalletTransaction[]>([])
const txTotal = ref(0)
const txPage = ref(1)
const txPageSize = ref(20)

const rechargeOrders = ref<PaymentOrder[]>([])
const orderTotal = ref(0)
const orderPage = ref(1)
const orderPageSize = ref(20)

const refunds = ref<RefundRequest[]>([])
const refundTotal = ref(0)
const refundPage = ref(1)
const refundPageSize = ref(20)

const activeTab = ref('transactions')

const rechargeForm = reactive({
  amount_usd: 10,
  payment_method: 'alipay',
})

const refundForm = reactive({
  amount_usd: 0,
  payment_order_id: '__none__',
  refund_mode: 'offline_payout',
  reason: '',
})

const refundableOrders = computed(() =>
  rechargeOrders.value.filter(o => (o.refundable_amount_usd || 0) > 0)
)

onMounted(async () => {
  try {
    await Promise.all([
      loadBalance(),
      loadTransactions(),
      loadOrders(),
      loadRefunds(),
    ])
  } finally {
    loadingInitial.value = false
  }
})

async function loadBalance() {
  walletBalance.value = await walletApi.getBalance()
}

async function loadTransactions() {
  loadingTransactions.value = true
  try {
    const offset = (txPage.value - 1) * txPageSize.value
    const resp = await walletApi.getTransactions({ limit: txPageSize.value, offset })
    transactions.value = resp.items
    txTotal.value = resp.total
  } catch (error) {
    log.error('加载钱包流水失败:', error)
    showError(parseApiError(error, '加载钱包流水失败'))
  } finally {
    loadingTransactions.value = false
  }
}

async function loadOrders() {
  loadingOrders.value = true
  try {
    const offset = (orderPage.value - 1) * orderPageSize.value
    const resp = await walletApi.listRechargeOrders({ limit: orderPageSize.value, offset })
    rechargeOrders.value = resp.items
    orderTotal.value = resp.total
  } catch (error) {
    log.error('加载充值订单失败:', error)
    showError(parseApiError(error, '加载充值订单失败'))
  } finally {
    loadingOrders.value = false
  }
}

async function loadRefunds() {
  loadingRefunds.value = true
  try {
    const offset = (refundPage.value - 1) * refundPageSize.value
    const resp = await walletApi.listRefunds({ limit: refundPageSize.value, offset })
    refunds.value = resp.items
    refundTotal.value = resp.total
  } catch (error) {
    log.error('加载退款记录失败:', error)
    showError(parseApiError(error, '加载退款记录失败'))
  } finally {
    loadingRefunds.value = false
  }
}

async function submitRecharge() {
  if (!rechargeForm.amount_usd || rechargeForm.amount_usd <= 0) {
    showError('请输入有效的充值金额')
    return
  }

  submittingRecharge.value = true
  try {
    latestRecharge.value = await walletApi.createRechargeOrder({
      amount_usd: rechargeForm.amount_usd,
      payment_method: rechargeForm.payment_method,
    })
    success('充值订单创建成功')
    await Promise.all([loadOrders(), loadBalance()])
    activeTab.value = 'orders'
  } catch (error) {
    log.error('创建充值订单失败:', error)
    showError(parseApiError(error, '创建充值订单失败'))
  } finally {
    submittingRecharge.value = false
  }
}

async function submitRefund() {
  if (!refundForm.amount_usd || refundForm.amount_usd <= 0) {
    showError('请输入有效的退款金额')
    return
  }
  const refundableBalance =
    walletBalance.value?.wallet?.refundable_balance ?? walletBalance.value?.refundable_balance ?? null
  if (refundableBalance !== null && refundForm.amount_usd > refundableBalance) {
    showError(`退款金额超过可退款余额（当前可退 ${formatCurrency(refundableBalance)}）`)
    return
  }

  submittingRefund.value = true
  try {
    await walletApi.createRefund({
      amount_usd: refundForm.amount_usd,
      payment_order_id:
        refundForm.payment_order_id && refundForm.payment_order_id !== '__none__'
          ? refundForm.payment_order_id
          : undefined,
      refund_mode: refundForm.refund_mode || undefined,
      reason: refundForm.reason || undefined,
      idempotency_key: `web_refund_${buildRefundIdempotencyKey()}`,
    })
    success('退款申请已提交')
    refundForm.amount_usd = 0
    refundForm.payment_order_id = '__none__'
    refundForm.reason = ''
    await Promise.all([loadRefunds(), loadBalance(), loadOrders(), loadTransactions()])
    activeTab.value = 'refunds'
  } catch (error) {
    log.error('提交退款申请失败:', error)
    showError(parseApiError(error, '提交退款申请失败'))
  } finally {
    submittingRefund.value = false
  }
}

function buildRefundIdempotencyKey(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID().replaceAll('-', '')
  }
  return `${Date.now()}_${Math.random().toString(16).slice(2, 10)}`
}

function handleTxPageChange(page: number) {
  txPage.value = page
  void loadTransactions()
}

function handleTxPageSizeChange(size: number) {
  txPageSize.value = size
  txPage.value = 1
  void loadTransactions()
}

function handleOrderPageChange(page: number) {
  orderPage.value = page
  void loadOrders()
}

function handleOrderPageSizeChange(size: number) {
  orderPageSize.value = size
  orderPage.value = 1
  void loadOrders()
}

function handleRefundPageChange(page: number) {
  refundPage.value = page
  void loadRefunds()
}

function handleRefundPageSizeChange(size: number) {
  refundPageSize.value = size
  refundPage.value = 1
  void loadRefunds()
}

function formatDateTime(value: string | null | undefined): string {
  if (!value) return '-'
  return new Date(value).toLocaleString('zh-CN', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  })
}
</script>
