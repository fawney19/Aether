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
      <!-- 支付开发测试进行中 -->
      <div
        v-if="showRechargeCard"
        class="grid grid-cols-1 gap-4"
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

          <div
            v-if="rechargePackages.length > 0"
            class="space-y-2"
          >
            <div class="flex items-center justify-between gap-3">
              <div class="text-sm font-medium">
                快捷套餐
              </div>
              <Button
                v-if="selectedRechargePackage"
                variant="ghost"
                size="sm"
                class="h-8 px-2 text-xs"
                @click="clearSelectedPackage"
              >
                改为自定义
              </Button>
            </div>
            <div class="grid grid-cols-1 md:grid-cols-3 gap-3">
              <button
                v-for="pkg in rechargePackages"
                :key="pkg.id"
                type="button"
                class="rounded-2xl border p-4 text-left transition"
                :class="selectedPackageId === pkg.id
                  ? 'border-primary bg-primary/5 shadow-sm'
                  : 'border-border/60 hover:border-primary/40 hover:bg-muted/20'"
                :disabled="!pkg.available"
                :aria-pressed="selectedPackageId === pkg.id"
                @click="selectRechargePackage(pkg)"
              >
                <div class="flex items-start justify-between gap-2">
                  <div>
                    <div class="font-medium">
                      {{ pkg.name }}
                    </div>
                    <div class="text-xs text-muted-foreground mt-1">
                      {{ pkg.description || '固定面额套餐' }}
                    </div>
                  </div>
                  <Badge :variant="selectedPackageId === pkg.id ? 'success' : pkg.available ? 'outline' : 'warning'">
                    {{ selectedPackageId === pkg.id ? '已选中' : pkg.available ? '可用' : '不可售' }}
                  </Badge>
                </div>
                <div class="mt-3 text-sm">
                  充 <span class="font-semibold">{{ formatCurrency(pkg.recharge_amount_usd) }}</span>
                  <span class="text-muted-foreground mx-1">送</span>
                  <span class="font-semibold text-emerald-600">{{ formatCurrency(pkg.bonus_amount_usd) }}</span>
                </div>
                <div class="mt-2 text-xs text-muted-foreground">
                  实付 {{ formatPaymentCurrency(pkg.pay_amount) }} · 合计到账 {{ formatCurrency(pkg.total_amount_usd) }}
                </div>
                <div
                  v-if="!pkg.available && pkg.availability_message"
                  class="mt-2 text-xs text-amber-600"
                >
                  {{ pkg.availability_message }}
                </div>
                <div
                  v-else-if="selectedPackageId === pkg.id"
                  class="mt-2 text-xs text-primary"
                >
                  再次点击可取消选择
                </div>
              </button>
            </div>
          </div>

          <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
            <div class="space-y-1.5">
              <Label>自定义充值金额 (CNY)</Label>
              <Input
                :model-value="rechargeForm.amount_usd"
                type="number"
                :min="rechargeSettings?.min_amount || 0.01"
                step="0.01"
                placeholder="10"
                :disabled="Boolean(selectedRechargePackage)"
                @update:model-value="handleRechargeAmountChange"
              />
              <p class="text-xs text-muted-foreground">
                单笔范围 {{ formatPaymentCurrency(rechargeSettings?.min_amount) }} - {{ formatPaymentCurrency(rechargeSettings?.max_amount) }}
              </p>
              <p
                v-if="selectedRechargePackage"
                class="text-xs text-primary"
              >
                当前已选择套餐，若需手动输入金额，请先取消套餐选择。
              </p>
            </div>

            <div class="space-y-1.5">
              <Label>支付方式</Label>
              <Select v-model="rechargeForm.payment_method">
                <SelectTrigger>
                  <SelectValue placeholder="选择支付方式" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem
                    v-for="method in availableRechargeMethods"
                    :key="method"
                    :value="method"
                  >
                    {{ paymentMethodLabel(method) }}
                  </SelectItem>
                </SelectContent>
              </Select>
              <p class="text-xs text-muted-foreground">
                当前比例 1 CNY = {{ rechargeSettings?.credit_ratio || 1 }} $余额，订单 {{ rechargeSettings?.expire_minutes || 15 }} 分钟过期
              </p>
            </div>
          </div>

          <div
            v-if="selectedRechargePackage"
            class="rounded-xl border border-primary/20 bg-primary/5 p-3 text-sm"
          >
            已选择套餐
            <span class="font-medium">{{ selectedRechargePackage.name }}</span>
            ，实付 {{ formatPaymentCurrency(selectedRechargePackage.pay_amount) }}，到账 {{ formatCurrency(selectedRechargePackage.recharge_amount_usd) }}，赠送 {{ formatCurrency(selectedRechargePackage.bonus_amount_usd) }}。
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
              实付金额: <span class="font-medium text-foreground">{{ formatPaymentCurrency(latestRecharge.order.pay_amount ?? latestRecharge.order.amount_usd) }}</span>
              · 到账金额: <span class="font-medium text-foreground">{{ formatCurrency(latestRecharge.order.amount_usd) }}</span>
              <span v-if="latestRecharge.order.bonus_amount_usd > 0">
                · 赠送: <span class="font-medium text-emerald-600">{{ formatCurrency(latestRecharge.order.bonus_amount_usd) }}</span>
              </span>
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
              @click.prevent="openPaymentWindow(String(latestRecharge.payment_instructions.payment_url))"
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
        <!-- TODO: 暂时屏蔽退款入口 -->
        <Card v-if="false" class="p-5 space-y-4">
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
              <Label>退款金额 (CNY)</Label>
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
                    <TableRow v-if="todayUsage">
                      <TableCell class="text-xs text-muted-foreground">
                        {{ todayUsage.date || '-' }}
                      </TableCell>
                      <TableCell>
                        <div class="space-y-1">
                          <div class="flex items-center gap-2">
                            <Badge
                              variant="outline"
                              class="font-mono border-amber-500/40 text-amber-700 dark:text-amber-300"
                            >
                              {{ dailyUsageCategoryLabel(true) }}
                            </Badge>
                            <span class="inline-flex h-2 w-2 rounded-full bg-emerald-500 animate-pulse" />
                            <span class="text-[11px] text-muted-foreground">
                              Live
                            </span>
                          </div>
                          <div class="text-[11px] text-muted-foreground">
                            {{ todayUsage.timezone || 'UTC' }}
                          </div>
                        </div>
                      </TableCell>
                      <TableCell class="text-rose-600 dark:text-rose-400">
                        -{{ todayUsage.total_cost.toFixed(4) }}
                      </TableCell>
                      <TableCell class="text-xs text-muted-foreground">
                        按日汇总
                      </TableCell>
                      <TableCell class="text-xs text-muted-foreground">
                        {{ todayUsage.total_requests }} 次请求 · {{ formatTokenCount(todayUsage.input_tokens) }} / {{ formatTokenCount(todayUsage.output_tokens) }} tokens
                      </TableCell>
                    </TableRow>
                    <template
                      v-for="item in flowItems"
                      :key="item.type === 'transaction' ? item.data.id : `daily-${item.data.id || item.data.date}`"
                    >
                      <TableRow v-if="item.type === 'transaction'">
                        <TableCell class="text-xs text-muted-foreground">
                          {{ formatDateTime(item.data.created_at) }}
                        </TableCell>
                        <TableCell>
                          <div class="space-y-1">
                            <Badge
                              variant="outline"
                              class="font-mono"
                            >
                              {{ walletTransactionCategoryLabel(item.data.category) }}
                            </Badge>
                            <div class="text-[11px] text-muted-foreground">
                              {{ walletTransactionReasonLabel(item.data.reason_code) }}
                            </div>
                          </div>
                        </TableCell>
                        <TableCell
                          :class="item.data.amount >= 0 ? 'text-emerald-600 dark:text-emerald-400' : 'text-rose-600 dark:text-rose-400'"
                        >
                          {{ item.data.amount >= 0 ? '+' : '' }}{{ item.data.amount.toFixed(4) }}
                        </TableCell>
                        <TableCell class="text-xs tabular-nums">
                          {{ item.data.balance_before.toFixed(4) }} → {{ item.data.balance_after.toFixed(4) }}
                        </TableCell>
                        <TableCell class="text-xs text-muted-foreground">
                          {{ item.data.description || '-' }}
                        </TableCell>
                      </TableRow>
                      <TableRow v-else>
                        <TableCell class="text-xs text-muted-foreground">
                          {{ item.data.date || '-' }}
                        </TableCell>
                        <TableCell>
                          <div class="space-y-1">
                            <Badge
                              variant="outline"
                              class="font-mono border-amber-500/40 text-amber-700 dark:text-amber-300"
                            >
                              {{ dailyUsageCategoryLabel(false) }}
                            </Badge>
                            <div class="text-[11px] text-muted-foreground">
                              {{ item.data.timezone || '-' }}
                            </div>
                          </div>
                        </TableCell>
                        <TableCell class="text-rose-600 dark:text-rose-400">
                          -{{ item.data.total_cost.toFixed(4) }}
                        </TableCell>
                        <TableCell class="text-xs text-muted-foreground">
                          按日汇总
                        </TableCell>
                        <TableCell class="text-xs text-muted-foreground">
                          {{ item.data.total_requests }} 次请求 · {{ formatTokenCount(item.data.input_tokens) }} / {{ formatTokenCount(item.data.output_tokens) }} tokens
                        </TableCell>
                      </TableRow>
                    </template>
                    <TableRow v-if="!loadingTransactions && flowItems.length === 0">
                      <TableCell
                        colspan="5"
                        class="py-10"
                      >
                        <EmptyState
                          title="暂无资金流水"
                          description="充值、退款或消费后会在这里显示"
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
                      <TableHead>到账金额</TableHead>
                      <TableHead>支付方式</TableHead>
                      <TableHead>状态</TableHead>
                      <TableHead>可退金额</TableHead>
                      <TableHead>支付时间</TableHead>
                      <TableHead>创建时间</TableHead>
                      <TableHead>最晚支付时间</TableHead>
                      <TableHead class="text-right">操作</TableHead>
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
                        <div>{{ formatCurrency(order.amount_usd) }}</div>
                        <div
                          v-if="order.bonus_amount_usd > 0"
                          class="text-[11px] text-emerald-600"
                        >
                          赠送 {{ formatCurrency(order.bonus_amount_usd) }} · 合计 {{ formatCurrency(order.total_amount_usd) }}
                        </div>
                        <div
                          v-if="order.pay_amount !== null && order.pay_amount !== undefined"
                          class="text-[11px] text-muted-foreground"
                        >
                          实付 {{ formatPaymentCurrency(order.pay_amount) }}
                        </div>
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
                      <TableCell
                        class="text-xs whitespace-nowrap"
                        :class="order.paid_at ? 'text-emerald-600' : 'text-muted-foreground'"
                      >
                        {{ order.paid_at ? formatDateTime(order.paid_at) : '-' }}
                      </TableCell>
                      <TableCell class="text-xs text-muted-foreground">
                        {{ formatDateTime(order.created_at) }}
                      </TableCell>
                      <TableCell
                        class="text-xs whitespace-nowrap"
                        :class="order.status === 'expired' ? 'text-rose-600' : 'text-muted-foreground'"
                      >
                        <div>{{ order.expires_at ? formatDateTime(order.expires_at) : '-' }}</div>
                        <div
                          v-if="order.status === 'expired'"
                          class="mt-1 text-[11px] text-rose-600"
                        >
                          已超时
                        </div>
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
                        <span
                          v-else
                          class="text-xs text-muted-foreground"
                        >
                          -
                        </span>
                      </TableCell>
                    </TableRow>
                    <TableRow v-if="!loadingOrders && rechargeOrders.length === 0">
                      <TableCell
                        colspan="9"
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
  refundModeLabel,
  refundStatusBadge,
  refundStatusLabel,
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
const loadingRefunds = ref(false)
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

const refunds = ref<RefundRequest[]>([])
const refundTotal = ref(0)
const refundPage = ref(1)
const refundPageSize = ref(20)

const activeTab = ref('transactions')
let todayCostPollTimer: ReturnType<typeof setInterval> | null = null

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
  'charset',
  'out_trade_no',
  'trade_no',
  'transaction_id',
  'merchant_trade_no',
  'total_amount',
  'cash_fee',
  'payment_status',
  'result',
  'return_code',
  'return_msg',
  'sign',
  'auth_app_id',
  'appid',
  'version',
  'app_id',
  'sign_type',
  'seller_id',
  'timestamp',
  'nonce_str',
  'method',
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
      loadRefunds(),
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
      rechargeForm.payment_method = methods[0] || ''
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
  todayCostPollTimer = setInterval(() => {
    void loadTodayCost()
  }, 20_000)
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

  return Boolean(
    (orderNo && (tradeNo || transactionId))
      || result
      || returnCode
      || paymentStatus
      || method,
  )
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
        info(`订单 ${matchedOrder.order_no} 当前状态：${paymentStatusLabel(matchedOrder.status)}`)
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
    latestRecharge.value = await walletApi.createRechargeOrder({
      amount_usd: selectedPackage ? undefined : rechargeForm.amount_usd,
      package_id: selectedPackage?.id,
      payment_method: rechargeForm.payment_method,
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
  window.open(url, '_blank', 'noopener,noreferrer')
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
    await Promise.all([loadRefunds(), loadBalance(), loadOrders(), loadTransactions(), loadTodayCost()])
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
    return crypto.randomUUID().replace(/-/g, '')
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
