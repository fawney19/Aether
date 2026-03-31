<template>
  <div class="space-y-6 pb-8">
    <div v-if="loadingInitial" class="py-16">
      <LoadingState message="正在加载钱包数据..." />
    </div>

    <template v-else>
      <div class="grid grid-cols-1 lg:grid-cols-2 gap-6">
        <!-- Left Side: Balances & Stats & History -->
        <div class="lg:col-span-2 space-y-6">
          <div class="grid grid-cols-1 sm:grid-cols-2 gap-6">
            <!-- Balance Card -->
            <Card class="relative overflow-hidden p-6 border-border/40 shadow-sm hover:shadow-md transition-shadow duration-300">
              <div class="relative z-10 space-y-3">
                <div class="flex items-center justify-between text-xs uppercase tracking-wider text-muted-foreground/80 font-medium">
                  <span>可用余额</span>
                  <WalletIcon class="w-4 h-4 text-primary/70" />
                </div>
                <div class="text-3xl font-bold tabular-nums">
                  {{ formatCurrency(walletBalance?.balance) }}
                </div>
                <div class="pt-1 flex flex-col gap-1 text-[13px] text-muted-foreground/80">
                  <div class="flex justify-between items-center"><span class="flex items-center gap-1.5"><div class="w-1.5 h-1.5 rounded-full bg-primary/60"></div> 充值余额</span> <span class="font-medium text-foreground/80">{{ formatCurrency(walletBalance?.wallet?.recharge_balance) }}</span></div>
                  <div class="flex justify-between items-center"><span class="flex items-center gap-1.5"><div class="w-1.5 h-1.5 rounded-full bg-emerald-500/60"></div> 赠款余额</span> <span class="font-medium text-foreground/80">{{ formatCurrency(walletBalance?.wallet?.gift_balance) }}</span></div>
                </div>
              </div>
            </Card>
            
            <!-- Stats Card -->
            <Card class="relative overflow-hidden p-6 border-border/40 shadow-sm hover:shadow-md transition-shadow duration-300">
              <div class="relative z-10 space-y-3">
                <div class="flex items-center justify-between text-xs uppercase tracking-wider text-muted-foreground/80 font-medium">
                  <span>累计充值 / 消费</span>
                  <ActivityIcon class="w-4 h-4 text-emerald-500/70" />
                </div>
                <div class="flex items-baseline gap-2 py-2">
                  <span class="text-2xl font-bold tabular-nums text-foreground">{{ formatCurrency(walletBalance?.wallet?.total_recharged) }}</span>
                  <span class="text-muted-foreground font-light">/</span>
                  <span class="text-xl font-medium tabular-nums text-muted-foreground/80">{{ formatCurrency(walletBalance?.wallet?.total_consumed) }}</span>
                </div>
                <div class="pt-0 flex flex-col gap-1 text-[13px] text-muted-foreground/80">
                  <div class="flex justify-between items-center"><span class="flex items-center gap-1.5"><div class="w-1.5 h-1.5 rounded-full bg-amber-500/60"></div> 累计退款</span> <span class="font-medium text-foreground/80">{{ formatCurrency(walletBalance?.wallet?.total_refunded) }}</span></div>
                  <div class="flex justify-between items-center"><span class="flex items-center gap-1.5"><div class="w-1.5 h-1.5 rounded-full bg-indigo-500/60"></div> 可退款余额</span> <span class="font-medium text-foreground/80">{{ formatCurrency(walletBalance?.wallet?.refundable_balance) }}</span></div>
                </div>
              </div>
            </Card>
          </div>

          <!-- History Tabs -->
          <Card class="overflow-hidden">
            <div class="px-5 pt-5 pb-2">
              <Tabs v-model="activeTab">
                <TabsList class="tabs-button-list grid grid-cols-2 w-full max-w-xl">
                  <TabsTrigger value="transactions">资金流水</TabsTrigger>
                  <TabsTrigger value="orders">充值订单</TabsTrigger>
                </TabsList>

                <TabsContent value="transactions" class="mt-4 space-y-4">
                  <div class="px-5 flex items-center justify-between">
                    <div class="text-sm text-muted-foreground">共 {{ txTotal }} 条</div>
                    <RefreshButton :loading="loadingTransactions" @click="loadTransactions" />
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
                        <TableRow v-if="todayUsage">
                          <TableCell class="text-xs text-muted-foreground">{{ todayUsage.date || '-' }}</TableCell>
                          <TableCell>
                            <div class="space-y-1">
                              <div class="flex items-center gap-2">
                                <Badge variant="outline" class="font-mono border-amber-500/40 text-amber-700 dark:text-amber-300">
                                  {{ dailyUsageCategoryLabel(true) }}
                                </Badge>
                                <span class="inline-flex h-2 w-2 rounded-full bg-emerald-500 animate-pulse" />
                                <span class="text-[11px] text-muted-foreground">Live</span>
                              </div>
                              <div class="text-[11px] text-muted-foreground">{{ todayUsage.timezone || 'UTC' }}</div>
                            </div>
                          </TableCell>
                          <TableCell class="text-rose-600 dark:text-rose-400">-{{ todayUsage.total_cost.toFixed(4) }}</TableCell>
                          <TableCell class="text-xs text-muted-foreground">按日汇总</TableCell>
                          <TableCell class="text-xs text-muted-foreground">
                            {{ todayUsage.total_requests }} 次请求 · {{ formatTokenCount(todayUsage.input_tokens) }} / {{ formatTokenCount(todayUsage.output_tokens) }} tokens
                          </TableCell>
                        </TableRow>
                        <template v-for="item in flowItems" :key="item.type === 'transaction' ? item.data.id : `daily-${item.data.id || item.data.date}`">
                          <TableRow v-if="item.type === 'transaction'">
                            <TableCell class="text-xs text-muted-foreground">{{ formatDateTime(item.data.created_at) }}</TableCell>
                            <TableCell>
                              <div class="space-y-1">
                                <Badge variant="outline" class="font-mono">{{ walletTransactionCategoryLabel(item.data.category) }}</Badge>
                                <div class="text-[11px] text-muted-foreground">{{ walletTransactionReasonLabel(item.data.reason_code) }}</div>
                              </div>
                            </TableCell>
                            <TableCell :class="item.data.amount >= 0 ? 'text-emerald-600 dark:text-emerald-400' : 'text-rose-600 dark:text-rose-400'">
                              {{ item.data.amount >= 0 ? '+' : '' }}{{ item.data.amount.toFixed(4) }}
                            </TableCell>
                            <TableCell class="text-xs tabular-nums">{{ item.data.balance_before.toFixed(4) }} → {{ item.data.balance_after.toFixed(4) }}</TableCell>
                            <TableCell class="text-xs text-muted-foreground">{{ item.data.description || '-' }}</TableCell>
                          </TableRow>
                          <TableRow v-else>
                            <TableCell class="text-xs text-muted-foreground">{{ item.data.date || '-' }}</TableCell>
                            <TableCell>
                              <div class="space-y-1">
                                <Badge variant="outline" class="font-mono border-amber-500/40 text-amber-700 dark:text-amber-300">{{ dailyUsageCategoryLabel(false) }}</Badge>
                                <div class="text-[11px] text-muted-foreground">{{ item.data.timezone || '-' }}</div>
                              </div>
                            </TableCell>
                            <TableCell class="text-rose-600 dark:text-rose-400">-{{ item.data.total_cost.toFixed(4) }}</TableCell>
                            <TableCell class="text-xs text-muted-foreground">按日汇总</TableCell>
                            <TableCell class="text-xs text-muted-foreground">
                              {{ item.data.total_requests }} 次请求 · {{ formatTokenCount(item.data.input_tokens) }} / {{ formatTokenCount(item.data.output_tokens) }} tokens
                            </TableCell>
                          </TableRow>
                        </template>
                        <TableRow v-if="!loadingTransactions && flowItems.length === 0">
                          <TableCell colspan="5" class="py-10">
                            <EmptyState title="暂无资金流水" description="充值、退款或消费后会在这里显示" />
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

                <TabsContent value="orders" class="mt-4 space-y-4">
                  <div class="px-5 flex items-center justify-between">
                    <div class="text-sm text-muted-foreground">共 {{ orderTotal }} 条</div>
                    <RefreshButton :loading="loadingOrders" @click="loadOrders" />
                  </div>
                  <div class="overflow-x-auto">
                    <Table>
                      <TableHeader>
                        <TableRow>
                          <TableHead>订单号</TableHead>
                          <TableHead>到账金额</TableHead>
                          <TableHead>支付方式</TableHead>
                          <TableHead>状态</TableHead>
                          <TableHead>可退金额</TableHead>
                          <TableHead>创建时间</TableHead>
                          <TableHead>最晚支付时间</TableHead>
                          <TableHead class="text-right">操作</TableHead>
                        </TableRow>
                      </TableHeader>
                      <TableBody>
                        <TableRow v-for="order in rechargeOrders" :key="order.id">
                          <TableCell class="font-mono text-xs">{{ order.order_no }}</TableCell>
                          <TableCell class="tabular-nums">
                            <div>{{ formatCurrency(order.amount_usd) }}</div>
                            <div v-if="order.bonus_amount_usd > 0" class="text-[11px] text-emerald-600">
                              赠送 {{ formatCurrency(order.bonus_amount_usd) }} · 合计 {{ formatCurrency(order.total_amount_usd) }}
                            </div>
                            <div v-if="order.pay_amount !== null && order.pay_amount !== undefined" class="text-[11px] text-muted-foreground">
                              实付 {{ formatPaymentCurrency(order.pay_amount) }}
                            </div>
                          </TableCell>
                          <TableCell>{{ paymentMethodLabel(order.payment_method) }}</TableCell>
                          <TableCell>
                            <Badge :variant="paymentStatusBadge(order.status)">{{ paymentStatusLabel(order.status) }}</Badge>
                          </TableCell>
                          <TableCell class="tabular-nums">
                            {{ formatCurrency(order.refundable_amount_usd) }}
                          </TableCell>
                          <TableCell class="text-xs text-muted-foreground">{{ formatDateTime(order.created_at) }}</TableCell>
                          <TableCell class="text-xs whitespace-nowrap" :class="order.status === 'expired' ? 'text-rose-600' : 'text-muted-foreground'">
                            <div>{{ order.expires_at ? formatDateTime(order.expires_at) : '-' }}</div>
                            <div v-if="order.status === 'expired'" class="mt-1 text-[11px] text-rose-600">已超时</div>
                          </TableCell>
                          <TableCell class="text-right">
                            <Button
                              v-if="canContinuePayment(order)"
                              variant="default"
                              size="default"
                              class="h-9 px-4 text-xs"
                              :disabled="continuingPayOrderId === order.id"
                              @click="continuePayOrder(order)"
                            >
                              {{ continuingPayOrderId === order.id ? '打开中...' : '继续支付' }}
                            </Button>
                            <span v-else class="text-xs text-muted-foreground">-</span>
                          </TableCell>
                        </TableRow>
                        <TableRow v-if="!loadingOrders && rechargeOrders.length === 0">
                          <TableCell colspan="8" class="py-10">
                            <EmptyState title="暂无充值订单" description="发起充值后会在这里显示" />
                          </TableCell>
                        </TableRow>
                      </TableBody>
                    </Table>
                  </div>
                  <Pagination :current="orderPage" :total="orderTotal" :page-size="orderPageSize" @update:current="handleOrderPageChange" @update:page-size="handleOrderPageSizeChange" />
                </TabsContent>
              </Tabs>
            </div>
          </Card>
        </div>

        <!-- Right Side: Wallet Actions (Payment Container) -->
        <div class="lg:col-span-1 border-l pl-4 hidden">
        </div>
      </div>
      <!-- End of layout Grid -->
      
      <!-- Sticking the action forms below history since they fit better or floating right -->
      <div v-if="showRechargeCard" class="mt-8 max-w-xl mx-auto space-y-4">
        <Card class="p-5 space-y-4 shadow-sm border-primary/20">
          <div class="flex items-center justify-between">
            <h3 class="text-base font-semibold">发起充值</h3>
            <RefreshButton :loading="loadingOrders" @click="loadOrders" />
          </div>

          <!-- Packages support here -->
          <div class="space-y-6 pb-2">
            <div v-if="rechargePackages.length > 0" class="space-y-3">
              <div class="flex items-center justify-between gap-3">
                <Label class="text-sm font-medium">快捷套餐 (立享赠送)</Label>
                <Button v-if="selectedRechargePackage" variant="ghost" size="sm" class="h-8 px-2 text-xs" @click="clearSelectedPackage">改为自定义</Button>
              </div>
              
              <div class="grid grid-cols-2 gap-3">
                <button
                  v-for="pkg in rechargePackages"
                  :key="pkg.id"
                  class="relative overflow-hidden rounded-xl border p-3 flex flex-col items-center justify-center transition-all duration-200"
                  :class="[
                    selectedPackageId === pkg.id 
                      ? 'border-primary ring-1 ring-primary/20 bg-primary/5 shadow-sm text-primary' 
                      : 'border-border/60 hover:border-primary/50 hover:bg-muted/30 text-muted-foreground hover:text-foreground',
                    !pkg.available ? 'opacity-50 cursor-not-allowed' : ''
                  ]"
                  :disabled="!pkg.available"
                  @click="selectRechargePackage(pkg)"
                >
                  <div class="text-[0.95rem] font-semibold tabular-nums">{{ pkg.name }}</div>
                  <div class="text-[0.7rem] text-emerald-600 font-medium">充{{ formatCurrency(pkg.recharge_amount_usd) }}送{{ formatCurrency(pkg.bonus_amount_usd) }}</div>
                  <div class="text-[0.75rem] text-muted-foreground mt-1">实付 ¥{{ pkg.pay_amount }}</div>
                </button>
              </div>
            </div>

            <div v-if="rechargePackages.length === 0" class="space-y-3">
              <Label class="text-sm font-medium">选择充值金额 (CNY)</Label>
              <div class="grid grid-cols-3 gap-3">
                <button
                  v-for="amount in presetAmounts"
                  :key="amount"
                  class="relative overflow-hidden rounded-xl border p-2.5 flex flex-col items-center justify-center transition-all duration-200"
                  :class="[
                    !isCustomAmount && rechargeForm.amount_usd === amount 
                      ? 'border-primary ring-1 ring-primary/20 bg-primary/5 shadow-sm text-primary' 
                      : 'border-border/60 hover:border-primary/50 hover:bg-muted/30 text-muted-foreground hover:text-foreground'
                  ]"
                  @click="selectPreset(amount)"
                >
                  <div class="text-[1.1rem] font-semibold tabular-nums">¥{{ amount }}</div>
                </button>
                <button
                  class="relative overflow-hidden rounded-xl border p-2.5 flex flex-col items-center justify-center transition-all duration-200 group"
                  :class="[
                    isCustomAmount 
                      ? 'border-primary ring-1 ring-primary/20 bg-primary/5 shadow-sm text-primary' 
                      : 'border-border/60 hover:border-primary/50 hover:bg-muted/30 text-muted-foreground hover:text-foreground'
                  ]"
                  @click="isCustomAmount = true; clearSelectedPackage(); rechargeForm.amount_usd = undefined;"
                >
                  <div class="text-sm font-medium transition-transform group-hover:scale-105">自定义</div>
                </button>
              </div>
            </div>

            <Transition
              enter-active-class="transition-all duration-300 ease-out"
              enter-from-class="opacity-0 -translate-y-2 h-0"
              enter-to-class="opacity-100 translate-y-0 h-16"
              leave-active-class="transition-all duration-200 ease-in"
              leave-from-class="opacity-100 translate-y-0 h-16"
              leave-to-class="opacity-0 -translate-y-2 h-0"
            >
              <div v-show="isCustomAmount || (rechargePackages.length > 0 && !selectedPackageId)" class="space-y-2 pt-2">
                <Label class="text-sm font-medium">自定义金额 (CNY) <span class="text-xs text-muted-foreground font-normal ml-2">无赠送额度</span></Label>
                <div class="relative group">
                  <span class="absolute left-3 top-1/2 -translate-y-1/2 text-muted-foreground/70 font-medium z-10">¥</span>
                  <Input
                    v-model.number="rechargeForm.amount_usd"
                    class="pl-8 text-lg font-medium h-12 transition-shadow focus-visible:ring-primary/30"
                    type="number"
                    min="0.01"
                    step="0.01"
                    placeholder="输入充值金额"
                    @update:model-value="handleRechargeAmountChange"
                  />
                </div>
              </div>
            </Transition>

            <div class="space-y-3 pt-2">
              <Label class="text-sm font-medium">支付方式</Label>
              <div class="space-y-3">
                <button
                  class="w-full flex items-center justify-between p-3 rounded-xl border transition-all"
                  :class="rechargeForm.payment_method === 'alipay' ? 'border-[#1677FF] bg-[#1677FF]/5' : 'border-border/60 hover:border-border'"
                  @click="rechargeForm.payment_method = 'alipay'"
                >
                  <div class="flex items-center gap-3">
                     <svg viewBox="0 0 1024 1024" class="w-6 h-6 text-[#1677FF]"><path fill="currentColor" d="M1024 512C1024 229.23 794.77 0 512 0S0 229.23 0 512s229.23 512 512 512 512-229.23 512-512zm-463.53 175.78H454.2L358.55 572.5c-48.42 27.81-98.39 46.54-142.36 55.43l13.75-58.85c52-11.89 107.41-35.15 158.42-63.7l-45.56-118H187v-54.89h154v-56.96h-165v-54.91h165V161h58.84v59.61h161v54.91h-161v56.96h176v54.89H394.07l29.41 76.22c50.32-34.96 85.39-81.82 108.77-133.58l56.88 27.67c-17.65 37.94-43.08 76.51-75.14 113.1l157 186.9h-66.67l-123.01-149.2-20.84-24.89zM731.33 331h-85.34v78.22h85.34V331zm0 133.69h-85.34v78.22h85.34v-78.22z"></path></svg>
                     <span class="font-medium text-foreground">支付宝 Alipay</span>
                  </div>
                  <div v-show="rechargeForm.payment_method === 'alipay'" class="w-4 h-4 rounded-full border-[4px] border-[#1677FF] bg-white"></div>
                  <div v-show="rechargeForm.payment_method !== 'alipay'" class="w-4 h-4 rounded-full border border-muted-foreground/40 bg-transparent"></div>
                </button>
              </div>
            </div>
          </div>

          <Button
            class="w-full h-12 text-base font-medium transition-all"
            :class="submittingRecharge ? 'opacity-80' : 'hover:shadow-md hover:shadow-primary/25 hover:-translate-y-0.5'"
            :disabled="submittingRecharge || (!selectedPackageId && (!rechargeForm.amount_usd && rechargeForm.amount_usd !== 0))"
            @click="submitRecharge"
          >
            <BanknoteIcon class="w-5 h-5 mr-2" v-if="!submittingRecharge" />
            <Loader2Icon class="w-5 h-5 mr-2 animate-spin" v-else />
            {{ submittingRecharge ? '正在跳转支付...' : '立即充值' }}
          </Button>

          <div
            v-if="latestRecharge"
            class="rounded-xl border border-border/60 bg-muted/30 p-3 space-y-1.5"
          >
            <div class="text-xs text-muted-foreground">最新订单: <span class="font-medium text-foreground">{{ latestRecharge.order.order_no }}</span></div>
            <div class="text-xs text-muted-foreground">
              状态: <Badge :variant="paymentStatusBadge(latestRecharge.order.status)" class="ml-1">{{ paymentStatusLabel(latestRecharge.order.status) }}</Badge>
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
          </div>
        </Card>
      </div>

    </template>
  </div>
</template>

<script setup lang="ts">
import { Wallet as WalletIcon, Activity as ActivityIcon, ShieldCheck as ShieldCheckIcon, Banknote as BanknoteIcon, Loader2 as Loader2Icon } from 'lucide-vue-next'
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
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
  type DailyUsageRecord,
  type FlowItem,
  type PaymentOrder,
  type RechargePackage,
  type RefundRequest,
  type WalletBalanceResponse,
  type WalletRechargeSettingsResponse,
} from '@/api/wallet'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'
import { log } from '@/utils/logger'
import {
  dailyUsageCategoryLabel,
  formatPaymentCurrency,
  formatTokenCount,
  formatWalletCurrency as formatCurrency,
  paymentMethodLabel,
  paymentStatusBadge,
  paymentStatusLabel,
  walletStatusBadge,
  walletStatusLabel,
  walletTransactionCategoryLabel,
  walletTransactionReasonLabel,
} from '@/utils/walletDisplay'

const route = useRoute()
const router = useRouter()
const { success, error: showError } = useToast()

const loadingInitial = ref(true)
const loadingTransactions = ref(false)
const loadingOrders = ref(false)
const submittingRecharge = ref(false)
const submittingRefund = ref(false)
const continuingPayOrderId = ref<string | null>(null)

const walletBalance = ref<WalletBalanceResponse | null>(null)
const rechargeSettings = ref<WalletRechargeSettingsResponse | null>(null)
const latestRecharge = ref<{ order: PaymentOrder; payment_instructions: Record<string, unknown> } | null>(null)
const selectedPackageId = ref<string | null>(null)

const flowItems = ref<FlowItem[]>([])
const todayUsage = ref<DailyUsageRecord | null>(null)
const txTotal = ref(0)
const txPage = ref(1)
const txPageSize = ref(20)

const rechargeOrders = ref<PaymentOrder[]>([])
const orderTotal = ref(0)
const orderPage = ref(1)
const orderPageSize = ref(20)

const activeTab = ref('transactions')
let todayCostPollTimer: ReturnType<typeof setInterval> | null = null

const presetAmounts = [10, 50, 100, 200, 500]
const isCustomAmount = ref(false)

function selectPreset(amount: number) {
  isCustomAmount.value = false
  clearSelectedPackage()
  rechargeForm.amount_usd = amount
}

const rechargeForm = reactive<{
  amount_usd: number | undefined;
  payment_method: string;
}>({
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

const availableRechargeMethods = computed(() => rechargeSettings.value?.enabled_payment_methods || [])
const rechargePackages = computed(() => rechargeSettings.value?.packages || [])
const selectedRechargePackage = computed<RechargePackage | null>(() =>
  rechargePackages.value.find(pkg => pkg.id === selectedPackageId.value) || null
)
const showRechargeCard = computed(() => {
  const settings = rechargeSettings.value
  return Boolean(settings?.recharge_enabled && settings.enabled_payment_methods.length > 0)
})

const PAYMENT_RETURN_QUERY_KEYS = new Set([
  'charset', 'out_trade_no', 'trade_no', 'transaction_id', 'merchant_trade_no', 'total_amount', 'cash_fee', 'payment_status', 'result', 'return_code', 'return_msg', 'sign', 'auth_app_id', 'appid', 'version', 'app_id', 'sign_type', 'seller_id', 'timestamp', 'nonce_str', 'method',
])

onMounted(async () => {
  document.addEventListener('visibilitychange', handleVisibilityChange)
  try {
    await Promise.all([
      loadRechargeSettings(),
      loadBalance(),
      loadTransactions(),
      loadTodayCost(),
      loadOrders(),
    ])
    await handlePaymentGatewayReturn()
    syncTodayCostPolling()
  } finally {
    loadingInitial.value = false
  }
})

onBeforeUnmount(() => {
  stopTodayCostPolling()
  document.removeEventListener('visibilitychange', handleVisibilityChange)
})

watch(activeTab, () => {
  syncTodayCostPolling()
})

async function loadBalance() {
  walletBalance.value = await walletApi.getBalance()
}

async function loadRechargeSettings() {
  try {
    rechargeSettings.value = await walletApi.getRechargeSettings()
    const methods = rechargeSettings.value.enabled_payment_methods
    if (!methods.includes(rechargeForm.payment_method)) {
      rechargeForm.payment_method = methods[0] || 'alipay'
    }
    if (
      selectedPackageId.value
      && !rechargeSettings.value.packages.some(pkg => pkg.id === selectedPackageId.value && pkg.available)
    ) {
      selectedPackageId.value = null
    }
  } catch (error) {
    log.error('加载充值配置失败:', error)
    rechargeSettings.value = null
  }
}

async function loadTransactions() {
  loadingTransactions.value = true
  try {
    const offset = (txPage.value - 1) * txPageSize.value
    const resp = await walletApi.getFlow({ limit: txPageSize.value, offset })
    flowItems.value = resp.items
    txTotal.value = resp.total
    todayUsage.value = resp.today_entry
  } catch (error) {
    log.error('加载钱包流水失败:', error)
    showError(parseApiError(error, '加载钱包流水失败'))
  } finally {
    loadingTransactions.value = false
  }
}

async function loadTodayCost() {
  try {
    todayUsage.value = await walletApi.getTodayCost()
  } catch (error) {
    log.error('加载今日消费失败:', error)
  }
}

function syncTodayCostPolling() {
  if (activeTab.value === 'transactions' && !document.hidden) {
    startTodayCostPolling()
  } else {
    stopTodayCostPolling()
  }
}

function startTodayCostPolling() {
  if (todayCostPollTimer) return
  todayCostPollTimer = setInterval(() => { void loadTodayCost() }, 20_000)
}

function stopTodayCostPolling() {
  if (!todayCostPollTimer) return
  clearInterval(todayCostPollTimer)
  todayCostPollTimer = null
}

function handleVisibilityChange() {
  syncTodayCostPolling()
}

function getSingleQueryValue(value: unknown): string | null {
  if (typeof value === 'string') return value
  if (Array.isArray(value) && typeof value[0] === 'string') return value[0]
  return null
}

function hasPaymentGatewayReturnQuery(): boolean {
  const orderNo = getSingleQueryValue(route.query.out_trade_no)
  const tradeNo = getSingleQueryValue(route.query.trade_no)
  const transactionId = getSingleQueryValue(route.query.transaction_id)
  const result = getSingleQueryValue(route.query.result)
  const returnCode = getSingleQueryValue(route.query.return_code)
  const paymentStatus = getSingleQueryValue(route.query.payment_status)
  const method = getSingleQueryValue(route.query.method)

  return Boolean((orderNo && (tradeNo || transactionId)) || result || returnCode || paymentStatus || method)
}

async function clearPaymentGatewayReturnQuery() {
  const nextQuery: Record<string, string | string[]> = {}
  let removed = false

  for (const [key, value] of Object.entries(route.query)) {
    if (PAYMENT_RETURN_QUERY_KEYS.has(key)) {
      removed = true
      continue
    }
    if (typeof value === 'string') {
      nextQuery[key] = value
      continue
    }
    if (Array.isArray(value)) {
      nextQuery[key] = value.filter((item): item is string => typeof item === 'string')
    }
  }

  if (!removed) return

  await router.replace({
    path: route.path,
    query: nextQuery,
    hash: route.hash,
  })
}

async function handlePaymentGatewayReturn() {
  if (!hasPaymentGatewayReturnQuery()) return

  const orderNo = getSingleQueryValue(route.query.out_trade_no)
  activeTab.value = 'orders'

  try {
    await Promise.all([loadOrders(), loadBalance()])

    const matchedOrder = orderNo
      ? rechargeOrders.value.find(order => order.order_no === orderNo) ?? null
      : null

    if (matchedOrder) {
      latestRecharge.value = {
        order: matchedOrder,
        payment_instructions: matchedOrder.gateway_response ?? {},
      }

      if (matchedOrder.status === 'credited' || matchedOrder.status === 'paid') {
        success(`订单 ${matchedOrder.order_no} 已更新为${paymentStatusLabel(matchedOrder.status)}`)
      } else {
        // ...
      }
    }
  } catch (error) {
    log.error('处理支付回跳失败:', error)
  } finally {
    await clearPaymentGatewayReturnQuery()
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

async function submitRecharge() {
  if (!showRechargeCard.value) {
    showError('充值功能暂未开放')
    return
  }
  if (!rechargeForm.payment_method) {
    showError('请选择支付方式')
    return
  }
  const selectedPackage = selectedRechargePackage.value
  if (rechargeSettings.value) {
    if (!rechargeSettings.value.enabled_payment_methods.includes(rechargeForm.payment_method)) {
      showError('当前支付方式暂未开放')
      return
    }
    if (selectedPackage) {
      if (!selectedPackage.available) {
        showError(selectedPackage.availability_message || '当前套餐暂不可购买')
        return
      }
    } else {
      if (!rechargeForm.amount_usd || rechargeForm.amount_usd <= 0) {
        showError('请输入有效的充值金额')
        return
      }
      if (rechargeForm.amount_usd < rechargeSettings.value.min_amount) {
        showError(`单笔充值金额不能低于 ${formatPaymentCurrency(rechargeSettings.value.min_amount)}`)
        return
      }
      if (rechargeForm.amount_usd > rechargeSettings.value.max_amount) {
        showError(`单笔充值金额不能高于 ${formatPaymentCurrency(rechargeSettings.value.max_amount)}`)
        return
      }
    }
  } else if (!selectedPackage && (!rechargeForm.amount_usd || rechargeForm.amount_usd <= 0)) {
    showError('请输入有效的充值金额')
    return
  }

  submittingRecharge.value = true
  try {
    const isMobile = /Android|webOS|iPhone|iPad|iPod|BlackBerry|IEMobile|Opera Mini/i.test(navigator.userAgent)
    latestRecharge.value = await walletApi.createRechargeOrder({
      amount_usd: selectedPackage ? undefined : rechargeForm.amount_usd,
      package_id: selectedPackage?.id,
      payment_method: rechargeForm.payment_method,
      client_type: isMobile ? 'h5' : 'pc'
    })
    success('充值订单创建成功')
    await Promise.all([loadOrders(), loadBalance()])
    activeTab.value = 'orders'
    
    // 自动跳转到支付页面
    if (latestRecharge.value?.payment_instructions?.payment_url) {
      success('正在为您跳转到支付页面...')
      openPaymentWindow(String(latestRecharge.value.payment_instructions.payment_url))
    }
  } catch (error) {
    log.error('创建充值订单失败:', error)
    showError(parseApiError(error, '创建充值订单失败'))
  } finally {
    submittingRecharge.value = false
  }
}

function openPaymentWindow(url: string) {
  // Directly set window.location.href to support mobile auto redirect gracefully instead of opening new tabs that could be blocked by pop-ups
  window.location.href = url
}

function handleRechargeAmountChange(value: string | number) {
  selectedPackageId.value = null
  const nextValue = typeof value === 'number' ? value : Number(value)
  rechargeForm.amount_usd = Number.isFinite(nextValue) ? nextValue : 0
}

function selectRechargePackage(pkg: RechargePackage) {
  if (!pkg.available) {
    return
  }
  if (selectedPackageId.value === pkg.id) {
    clearSelectedPackage()
    return
  }
  selectedPackageId.value = pkg.id
  isCustomAmount.value = false
  rechargeForm.amount_usd = pkg.pay_amount
}

function clearSelectedPackage() {
  selectedPackageId.value = null
}

function canContinuePayment(order: PaymentOrder) {
  return order.status === 'pending'
}

async function continuePayOrder(order: PaymentOrder) {
  continuingPayOrderId.value = order.id
  try {
    const resp = await walletApi.getRechargeOrder(order.id)
    if (resp.order.status !== 'pending') {
      await loadOrders()
      showError(`当前订单状态为${paymentStatusLabel(resp.order.status)}，无法继续支付`)
      return
    }

    const paymentUrl = resp.order.gateway_response?.payment_url

    if (!paymentUrl || typeof paymentUrl !== 'string') {
      showError('当前订单暂无可用支付链接，请重新创建订单')
      return
    }

    latestRecharge.value = {
      order: resp.order,
      payment_instructions: resp.order.gateway_response ?? {},
    }
    openPaymentWindow(paymentUrl)
  } catch (error) {
    log.error('继续支付失败:', error)
    showError(parseApiError(error, '继续支付失败'))
  } finally {
    continuingPayOrderId.value = null
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
