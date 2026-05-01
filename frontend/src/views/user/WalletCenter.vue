<template>
  <div class="space-y-6 pb-8">
    <div
      v-if="loadingInitial"
      class="py-16"
    >
      <LoadingState message="正在加载钱包数据..." />
    </div>

    <template v-else>
      <div class="space-y-2.5 sm:hidden">
        <Card class="overflow-hidden border border-border/60 bg-[radial-gradient(circle_at_top_left,hsl(var(--primary)/0.12),transparent_58%),linear-gradient(180deg,hsl(var(--background)),hsl(var(--muted)/0.34))] p-3.5">
          <div class="flex items-start justify-between gap-3">
            <div class="min-w-0">
              <div class="text-[11px] uppercase tracking-[0.24em] text-muted-foreground">
                可用余额
              </div>
              <div class="mt-1.5 text-[1.85rem] font-semibold tracking-tight tabular-nums text-foreground">
                {{ formatCurrency(walletBalance?.balance) }}
              </div>
              <div class="mt-1.5 flex flex-wrap items-center gap-1.5">
                <Badge :variant="walletStatusBadge(walletBalance?.wallet?.status)">
                  {{ walletStatusLabel(walletBalance?.wallet?.status) }}
                </Badge>
                <span
                  v-if="walletBalance?.unlimited"
                  class="text-[11px] text-amber-600 dark:text-amber-400"
                >
                  无限制模式
                </span>
              </div>
            </div>

            <Button
              v-if="ENABLE_WALLET_RECHARGE_FORM"
              type="button"
              size="sm"
              variant="outline"
              class="h-8 rounded-full px-3 text-xs"
              @click="showRechargeDialog = true"
            >
              充值
            </Button>
          </div>

          <div class="mt-3 grid grid-cols-2 gap-2">
            <div class="rounded-xl border border-border/50 bg-background/88 p-2.5">
              <div class="text-[11px] text-muted-foreground">
                充值余额
              </div>
              <div class="mt-0.5 text-[15px] font-semibold tabular-nums text-foreground">
                {{ formatCurrency(walletBalance?.wallet?.recharge_balance) }}
              </div>
            </div>
            <div class="rounded-xl border border-border/50 bg-background/88 p-2.5">
              <div class="text-[11px] text-muted-foreground">
                赠款余额
              </div>
              <div class="mt-0.5 text-[15px] font-semibold tabular-nums text-foreground">
                {{ formatCurrency(walletBalance?.wallet?.gift_balance) }}
              </div>
            </div>
            <div class="rounded-xl border border-border/50 bg-background/88 p-2.5">
              <div class="text-[11px] text-muted-foreground">
                累计充值
              </div>
              <div class="mt-0.5 text-[15px] font-semibold tabular-nums text-foreground">
                {{ formatCurrency(walletBalance?.wallet?.total_recharged) }}
              </div>
            </div>
            <div class="rounded-xl border border-border/50 bg-background/88 p-2.5">
              <div class="text-[11px] text-muted-foreground">
                累计消费
              </div>
              <div class="mt-0.5 text-[15px] font-semibold tabular-nums text-foreground">
                {{ formatCurrency(walletBalance?.wallet?.total_consumed) }}
              </div>
            </div>
          </div>
        </Card>

        <Card class="border border-border/60 bg-card/95 p-3.5">
          <div class="grid grid-cols-2 gap-2">
            <div class="rounded-xl border border-border/50 bg-muted/18 p-2.5">
              <div class="text-[11px] text-muted-foreground">
                累计退款
              </div>
              <div class="mt-0.5 text-[15px] font-semibold tabular-nums text-foreground">
                {{ formatCurrency(walletBalance?.wallet?.total_refunded) }}
              </div>
            </div>
            <div class="rounded-xl border border-border/50 bg-muted/18 p-2.5">
              <div class="text-[11px] text-muted-foreground">
                可退款余额
              </div>
              <div class="mt-0.5 text-[15px] font-semibold tabular-nums text-foreground">
                {{ formatCurrency(walletBalance?.wallet?.refundable_balance) }}
              </div>
            </div>
            <div class="rounded-xl border border-border/50 bg-muted/18 p-2.5">
              <div class="text-[11px] text-muted-foreground">
                待处理退款
              </div>
              <div class="mt-0.5 text-[15px] font-semibold text-foreground">
                {{ walletBalance?.pending_refund_count || 0 }}
              </div>
            </div>
            <div class="rounded-xl border border-border/50 bg-muted/18 p-2.5">
              <div class="text-[11px] text-muted-foreground">
                钱包状态
              </div>
              <div class="mt-1">
                <Badge :variant="walletStatusBadge(walletBalance?.wallet?.status)">
                  {{ walletStatusLabel(walletBalance?.wallet?.status) }}
                </Badge>
              </div>
            </div>
          </div>
        </Card>
      </div>

      <div class="hidden grid-cols-1 gap-4 sm:grid lg:grid-cols-3">
        <Card class="p-5 space-y-2">
          <div class="flex items-start justify-between gap-3">
            <div class="text-xs uppercase tracking-wider text-muted-foreground">
              可用余额
            </div>
            <Button
              v-if="ENABLE_WALLET_RECHARGE_FORM"
              type="button"
              size="sm"
              variant="outline"
              class="h-8 rounded-full px-3 text-xs"
              @click="showRechargeDialog = true"
            >
              充值
            </Button>
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

      <Card
        v-if="latestRecharge"
        class="overflow-hidden border border-border/60 bg-gradient-to-r from-muted/25 via-background to-muted/10"
      >
        <div class="flex flex-col gap-3 px-4 py-3 xl:flex-row xl:items-center xl:justify-between">
          <div class="min-w-0 flex-1">
            <div class="flex flex-wrap items-center gap-x-3 gap-y-2 xl:flex-nowrap">
              <div class="flex shrink-0 flex-wrap items-center gap-2">
                <Badge
                  :variant="paymentStatusBadge(latestRecharge.order.status)"
                  class="h-6 shrink-0 whitespace-nowrap px-2.5 text-[11px]"
                >
                  {{ latestRecharge.order.status === 'pending_approval' ? '待审核订单' : '待支付订单' }}
                </Badge>
                <Badge
                  variant="outline"
                  class="h-6 shrink-0 whitespace-nowrap px-2.5 text-[11px]"
                >
                  钱包充值
                </Badge>
                <Badge
                  variant="outline"
                  class="h-6 shrink-0 whitespace-nowrap px-2.5 text-[11px]"
                >
                  {{ paymentMethodLabel(latestRecharge.order.payment_method) }}
                </Badge>
              </div>

              <div class="min-w-0 flex flex-1 flex-wrap items-center gap-x-3 gap-y-1">
                <div class="min-w-0 truncate text-sm font-semibold text-foreground">
                  钱包充值 · {{ formatCurrency(latestRecharge.order.amount_usd) }}
                </div>
                <span class="shrink-0 text-xs text-muted-foreground">
                  {{ pendingRechargeDateLabel(latestRecharge.order) }}
                </span>
                <span
                  class="min-w-0 truncate text-[11px] text-muted-foreground"
                  :title="latestRecharge.order.order_no"
                >
                  订单号 {{ compactOrderNo(latestRecharge.order.order_no) }}
                </span>
              </div>
            </div>
          </div>

          <div class="flex shrink-0 items-center gap-2">
            <a
              v-if="rechargePaymentUrl(latestRecharge)"
              :href="rechargePaymentUrl(latestRecharge)"
              target="_blank"
              rel="noopener noreferrer"
              class="inline-flex h-8 items-center justify-center rounded-md border border-border bg-background px-3 text-xs font-medium text-foreground transition hover:bg-muted"
            >
              立即支付
            </a>
            <Button
              variant="outline"
              size="sm"
              class="h-8 px-3"
              @click="activeTab = 'orders'"
            >
              查看订单
            </Button>
            <Button
              v-if="canCancelRechargeOrder(latestRecharge.order)"
              variant="outline"
              size="sm"
              class="h-8 px-3"
              :disabled="cancelingRechargeOrderId === latestRecharge.order.id"
              @click="cancelRechargeOrder(latestRecharge.order)"
            >
              {{ cancelingRechargeOrderId === latestRecharge.order.id ? '取消中...' : '取消订单' }}
            </Button>
          </div>
        </div>
      </Card>

      <div
        v-if="ENABLE_WALLET_REFUND_FORM"
        class="max-w-xl"
      >
        <Card
          v-if="ENABLE_WALLET_REFUND_FORM"
          class="p-5 space-y-4"
        >
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
              <div class="flex items-center justify-between px-4 sm:px-5">
                <div class="text-sm text-muted-foreground">
                  共 {{ txTotal }} 条
                </div>
                <RefreshButton
                  :loading="loadingTransactions"
                  @click="loadTransactions"
                />
              </div>

              <div class="space-y-2.5 px-4 sm:hidden">
                <div
                  v-if="todayUsage"
                  class="rounded-2xl border border-border/60 bg-card/95 p-3.5 shadow-[0_16px_34px_-30px_hsl(var(--foreground))]"
                >
                  <div class="flex items-start justify-between gap-3">
                    <div class="min-w-0">
                      <div class="flex flex-wrap items-center gap-2">
                        <Badge
                          variant="outline"
                          class="h-6 whitespace-nowrap px-2.5 py-0 text-[11px]"
                        >
                          {{ dailyUsageCategoryLabel(true) }}
                        </Badge>
                        <span class="inline-flex h-2 w-2 rounded-full bg-emerald-500 animate-pulse" />
                        <span class="text-[11px] text-muted-foreground">
                          实时
                        </span>
                      </div>
                      <div class="mt-2.5 text-sm font-semibold text-foreground">
                        {{ todayUsage.date || '-' }}
                      </div>
                      <div class="mt-1 text-[11px] text-muted-foreground">
                        {{ todayUsage.timezone || 'UTC' }}
                      </div>
                    </div>

                    <div class="text-right">
                      <div class="text-[11px] text-muted-foreground">
                        今日消费
                      </div>
                      <div class="mt-0.5 text-base font-semibold tabular-nums text-rose-600 dark:text-rose-400">
                        -{{ todayUsage.total_cost.toFixed(4) }}
                      </div>
                    </div>
                  </div>

                  <div class="mt-3 grid grid-cols-2 gap-2 text-xs">
                    <div class="rounded-xl border border-border/40 bg-muted/18 p-2.5">
                      <div class="text-muted-foreground">
                        请求次数
                      </div>
                      <div class="mt-1 font-semibold text-foreground">
                        {{ todayUsage.total_requests }}
                      </div>
                    </div>
                    <div class="rounded-xl border border-border/40 bg-muted/18 p-2.5">
                      <div class="text-muted-foreground">
                        Tokens
                      </div>
                      <div class="mt-1 font-semibold text-foreground">
                        {{ formatTokenCount(todayUsage.input_tokens + todayUsage.output_tokens) }}
                      </div>
                    </div>
                  </div>
                </div>

                <div
                  v-for="item in flowItems"
                  :key="item.type === 'transaction' ? item.data.id : `daily-mobile-${item.data.id || item.data.date}`"
                >
                  <div
                    v-if="item.type === 'transaction'"
                    class="rounded-2xl border border-border/60 bg-card/95 p-3.5 shadow-[0_16px_34px_-30px_hsl(var(--foreground))]"
                  >
                    <div class="flex items-start justify-between gap-3">
                      <div class="min-w-0">
                        <div class="flex flex-wrap items-center gap-2">
                          <Badge
                            variant="outline"
                            class="h-6 whitespace-nowrap px-2.5 py-0 text-[11px]"
                          >
                            {{ walletTransactionCategoryLabel(item.data.category) }}
                          </Badge>
                          <span
                            class="max-w-[11rem] truncate text-[11px] text-muted-foreground"
                            :title="walletTransactionReasonLabel(item.data.reason_code)"
                          >
                            {{ walletTransactionReasonLabel(item.data.reason_code) }}
                          </span>
                        </div>
                        <div class="mt-2.5 text-sm font-medium text-foreground">
                          {{ formatDateLabel(item.data.created_at) }}
                        </div>
                        <div class="mt-1 text-[11px] text-muted-foreground">
                          {{ formatTimeLabel(item.data.created_at) }}
                        </div>
                      </div>

                      <div class="text-right">
                        <div class="text-[11px] text-muted-foreground">
                          变动
                        </div>
                        <div
                          class="mt-0.5 text-base font-semibold tabular-nums"
                          :class="item.data.amount >= 0 ? 'text-emerald-600 dark:text-emerald-400' : 'text-rose-600 dark:text-rose-400'"
                        >
                          {{ item.data.amount >= 0 ? '+' : '' }}{{ item.data.amount.toFixed(4) }}
                        </div>
                      </div>
                    </div>

                    <div class="mt-3 grid grid-cols-2 gap-2 text-xs">
                      <div class="rounded-xl border border-border/40 bg-muted/18 p-2.5">
                        <div class="text-muted-foreground">
                          总余额
                        </div>
                        <div class="mt-1 font-medium tabular-nums text-foreground">
                          {{ formatCurrency(item.data.balance_before, { decimals: 4 }) }} → {{ formatCurrency(item.data.balance_after, { decimals: 4 }) }}
                        </div>
                      </div>
                      <div class="rounded-xl border border-border/40 bg-muted/18 p-2.5">
                        <div class="text-muted-foreground">
                          充值余额
                        </div>
                        <div class="mt-1 font-medium tabular-nums text-foreground">
                          {{ formatCurrency(item.data.recharge_balance_before, { decimals: 4 }) }} → {{ formatCurrency(item.data.recharge_balance_after, { decimals: 4 }) }}
                        </div>
                      </div>
                      <div class="col-span-2 rounded-xl border border-border/40 bg-muted/18 p-2.5">
                        <div class="text-muted-foreground">
                          赠款余额
                        </div>
                        <div class="mt-1 font-medium tabular-nums text-foreground">
                          {{ formatCurrency(item.data.gift_balance_before, { decimals: 4 }) }} → {{ formatCurrency(item.data.gift_balance_after, { decimals: 4 }) }}
                        </div>
                      </div>
                    </div>

                    <div
                      v-if="item.data.description"
                      class="mt-2.5 rounded-xl border border-border/40 bg-background/85 px-3 py-2 text-xs leading-5 text-muted-foreground"
                    >
                      {{ item.data.description }}
                    </div>
                  </div>

                  <div
                    v-else
                    class="rounded-2xl border border-border/60 bg-card/95 p-3.5 shadow-[0_16px_34px_-30px_hsl(var(--foreground))]"
                  >
                    <div class="flex items-start justify-between gap-3">
                      <div class="min-w-0">
                        <div class="flex flex-wrap items-center gap-2">
                        <Badge
                          variant="outline"
                          class="h-6 whitespace-nowrap px-2.5 py-0 text-[11px]"
                        >
                          {{ dailyUsageCategoryLabel(false) }}
                          </Badge>
                          <span class="text-[11px] text-muted-foreground">
                            {{ item.data.timezone || '-' }}
                          </span>
                        </div>
                        <div class="mt-2.5 text-sm font-medium text-foreground">
                          {{ item.data.date || '-' }}
                        </div>
                        <div class="mt-1 text-[11px] text-muted-foreground">
                          按日汇总
                        </div>
                      </div>

                      <div class="text-right">
                        <div class="text-[11px] text-muted-foreground">
                          消费
                        </div>
                        <div class="mt-0.5 text-base font-semibold tabular-nums text-rose-600 dark:text-rose-400">
                          -{{ item.data.total_cost.toFixed(4) }}
                        </div>
                      </div>
                    </div>

                    <div class="mt-3 grid grid-cols-2 gap-2 text-xs">
                      <div class="rounded-xl border border-border/40 bg-muted/18 p-2.5">
                        <div class="text-muted-foreground">
                          请求次数
                        </div>
                        <div class="mt-1 font-semibold text-foreground">
                          {{ item.data.total_requests }}
                        </div>
                      </div>
                      <div class="rounded-xl border border-border/40 bg-muted/18 p-2.5">
                        <div class="text-muted-foreground">
                          Tokens
                        </div>
                        <div class="mt-1 font-semibold text-foreground">
                          {{ formatTokenCount(item.data.input_tokens + item.data.output_tokens) }}
                        </div>
                      </div>
                    </div>
                  </div>
                </div>

                <EmptyState
                  v-if="!loadingTransactions && !todayUsage && flowItems.length === 0"
                  title="暂无资金流水"
                  description="充值、退款或消费后会在这里显示"
                />
              </div>

              <div class="hidden overflow-hidden rounded-2xl border border-border/60 bg-background sm:block">
                <div class="overflow-x-auto">
                  <Table class="min-w-[980px] table-fixed">
                    <TableHeader>
                      <TableRow>
                        <TableHead class="w-[16%] whitespace-nowrap">时间</TableHead>
                        <TableHead class="w-[18%] whitespace-nowrap">类型</TableHead>
                        <TableHead class="w-[12%] whitespace-nowrap">变动</TableHead>
                        <TableHead class="w-[22%] whitespace-nowrap">余额变化</TableHead>
                        <TableHead class="w-[32%] whitespace-nowrap">说明</TableHead>
                      </TableRow>
                    </TableHeader>
                    <TableBody>
                      <TableRow
                        v-if="todayUsage"
                        class="border-b border-border/40"
                      >
                        <TableCell class="py-4 align-top text-sm text-muted-foreground">
                          <div class="whitespace-nowrap">
                            {{ todayUsage.date || '-' }}
                          </div>
                          <div class="mt-1 whitespace-nowrap text-xs text-muted-foreground">
                            实时
                          </div>
                        </TableCell>
                        <TableCell class="py-5 align-top">
                          <div class="space-y-1">
                            <div class="flex items-center gap-2">
                              <Badge
                                variant="outline"
                                class="h-8 whitespace-nowrap border-amber-500/40 px-3 py-0 text-amber-700 dark:text-amber-300"
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
                        <TableCell class="py-4 align-top text-sm font-medium tabular-nums text-rose-600 dark:text-rose-400">
                          -{{ todayUsage.total_cost.toFixed(4) }}
                        </TableCell>
                        <TableCell class="py-4 align-top text-xs tabular-nums text-muted-foreground">
                          按日汇总
                        </TableCell>
                        <TableCell
                          class="py-4 align-top text-xs text-muted-foreground"
                          :title="`${todayUsage.total_requests} 次请求 · ${formatTokenCount(todayUsage.input_tokens)} / ${formatTokenCount(todayUsage.output_tokens)} tokens`"
                        >
                          {{ todayUsage.total_requests }} 次请求 · {{ formatTokenCount(todayUsage.input_tokens) }} / {{ formatTokenCount(todayUsage.output_tokens) }} tokens
                        </TableCell>
                      </TableRow>
                      <template
                        v-for="item in flowItems"
                        :key="item.type === 'transaction' ? item.data.id : `daily-${item.data.id || item.data.date}`"
                      >
                        <TableRow
                          v-if="item.type === 'transaction'"
                          class="border-b border-border/40 last:border-b-0"
                        >
                          <TableCell class="py-4 align-top text-sm text-muted-foreground">
                            <div class="whitespace-nowrap">
                              {{ formatDateLabel(item.data.created_at) }}
                            </div>
                            <div class="mt-1 whitespace-nowrap text-xs text-muted-foreground">
                              {{ formatTimeLabel(item.data.created_at) }}
                            </div>
                          </TableCell>
                          <TableCell class="py-4 align-top">
                            <div class="space-y-1">
                              <Badge
                                variant="outline"
                                class="h-8 whitespace-nowrap px-3 py-0"
                              >
                                {{ walletTransactionCategoryLabel(item.data.category) }}
                              </Badge>
                              <div
                                class="truncate text-[11px] text-muted-foreground"
                                :title="walletTransactionReasonLabel(item.data.reason_code)"
                              >
                                {{ walletTransactionReasonLabel(item.data.reason_code) }}
                              </div>
                            </div>
                          </TableCell>
                          <TableCell
                            class="py-4 align-top text-sm font-medium tabular-nums"
                            :class="item.data.amount >= 0 ? 'text-emerald-600 dark:text-emerald-400' : 'text-rose-600 dark:text-rose-400'"
                          >
                            {{ item.data.amount >= 0 ? '+' : '' }}{{ item.data.amount.toFixed(4) }}
                          </TableCell>
                          <TableCell class="py-4 align-top text-xs tabular-nums text-muted-foreground">
                            <div class="whitespace-nowrap">
                              {{ item.data.balance_before.toFixed(4) }} → {{ item.data.balance_after.toFixed(4) }}
                            </div>
                          </TableCell>
                          <TableCell
                            class="py-4 align-top text-xs text-muted-foreground"
                            :title="item.data.description || '-'"
                          >
                            {{ item.data.description || '-' }}
                          </TableCell>
                        </TableRow>
                        <TableRow
                          v-else
                          class="border-b border-border/40 last:border-b-0"
                        >
                          <TableCell class="py-4 align-top text-sm text-muted-foreground">
                            <div class="whitespace-nowrap">
                              {{ item.data.date || '-' }}
                            </div>
                            <div class="mt-1 whitespace-nowrap text-xs text-muted-foreground">
                              汇总
                            </div>
                          </TableCell>
                          <TableCell class="py-4 align-top">
                            <div class="space-y-1">
                              <Badge
                                variant="outline"
                                class="h-8 whitespace-nowrap border-amber-500/40 px-3 py-0 text-amber-700 dark:text-amber-300"
                              >
                                {{ dailyUsageCategoryLabel(false) }}
                              </Badge>
                              <div class="text-[11px] text-muted-foreground">
                                {{ item.data.timezone || '-' }}
                              </div>
                            </div>
                          </TableCell>
                          <TableCell class="py-4 align-top text-sm font-medium tabular-nums text-rose-600 dark:text-rose-400">
                            -{{ item.data.total_cost.toFixed(4) }}
                          </TableCell>
                          <TableCell class="py-4 align-top text-xs tabular-nums text-muted-foreground">
                            按日汇总
                          </TableCell>
                          <TableCell
                            class="py-4 align-top text-xs text-muted-foreground"
                            :title="`${item.data.total_requests} 次请求 · ${formatTokenCount(item.data.input_tokens)} / ${formatTokenCount(item.data.output_tokens)} tokens`"
                          >
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
              <div class="flex items-center justify-between px-4 sm:px-5">
                <div class="text-sm text-muted-foreground">
                  共 {{ orderTotal }} 条
                </div>
                <RefreshButton
                  :loading="loadingOrders"
                  @click="loadOrders"
                />
              </div>

              <div class="space-y-2.5 px-4 sm:hidden">
                <div
                  v-for="order in rechargeOrders"
                  :key="order.id"
                  class="rounded-2xl border border-border/60 bg-card/95 p-3.5 shadow-[0_16px_34px_-30px_hsl(var(--foreground))]"
                >
                  <div class="flex items-start justify-between gap-3">
                    <div class="min-w-0">
                      <div class="flex flex-wrap items-center gap-2">
                        <Badge
                          :variant="paymentStatusBadge(order.status)"
                          class="h-6 whitespace-nowrap px-2.5 py-0 text-[11px]"
                        >
                          {{ paymentStatusLabel(order.status) }}
                        </Badge>
                        <Badge
                          variant="outline"
                          class="h-6 whitespace-nowrap px-2.5 py-0 text-[11px]"
                        >
                          {{ paymentMethodLabel(order.payment_method) }}
                        </Badge>
                      </div>
                      <div
                        class="mt-2.5 truncate font-mono text-xs text-foreground"
                        :title="order.order_no"
                      >
                        {{ compactOrderNo(order.order_no) }}
                      </div>
                      <div class="mt-1 text-[11px] text-muted-foreground">
                        钱包充值
                      </div>
                    </div>

                    <div class="text-right">
                      <div class="text-[11px] text-muted-foreground">
                        金额
                      </div>
                      <div class="mt-0.5 text-base font-semibold tabular-nums text-foreground">
                        {{ formatCurrency(order.amount_usd) }}
                      </div>
                    </div>
                  </div>

                    <div class="mt-3 grid grid-cols-2 gap-2 text-xs">
                      <div class="rounded-xl border border-border/40 bg-muted/18 p-2.5">
                        <div class="text-muted-foreground">
                          可退金额
                        </div>
                        <div class="mt-0.5 font-semibold tabular-nums text-foreground">
                          {{ formatCurrency(order.refundable_amount_usd) }}
                        </div>
                        <div
                          v-if="!canCancelRechargeOrder(order) && !(order.status === 'pending' && rechargeOrderPaymentUrl(order))"
                          class="mt-1 text-[11px] text-muted-foreground"
                        >
                          {{ rechargeOrderActionText(order) }}
                        </div>
                      </div>
                      <div class="rounded-xl border border-border/40 bg-muted/18 p-2.5">
                        <div class="text-muted-foreground">
                          创建时间
                        </div>
                      <div class="mt-0.5 font-medium text-foreground">
                        {{ formatDateLabel(order.created_at) }}
                      </div>
                      <div class="mt-0.5 text-[11px] text-muted-foreground">
                        {{ formatTimeLabel(order.created_at) }}
                      </div>
                    </div>
                  </div>

                  <div
                    v-if="(order.status === 'pending' && rechargeOrderPaymentUrl(order)) || canCancelRechargeOrder(order)"
                    class="mt-3 flex flex-wrap gap-2"
                  >
                    <a
                      v-if="order.status === 'pending' && rechargeOrderPaymentUrl(order)"
                      :href="rechargeOrderPaymentUrl(order)"
                      target="_blank"
                      rel="noopener noreferrer"
                      class="inline-flex h-8 items-center justify-center rounded-md border border-border bg-background px-3 text-xs font-medium text-foreground transition hover:bg-muted"
                    >
                      去支付
                    </a>
                    <Button
                      v-if="canCancelRechargeOrder(order)"
                      size="sm"
                      variant="outline"
                      class="h-8 px-3"
                      :disabled="cancelingRechargeOrderId === order.id"
                      @click="cancelRechargeOrder(order)"
                    >
                      {{ cancelingRechargeOrderId === order.id ? '取消中...' : '取消订单' }}
                    </Button>
                  </div>
                </div>

                <EmptyState
                  v-if="!loadingOrders && rechargeOrders.length === 0"
                  title="暂无充值订单"
                  description="发起充值后会在这里显示"
                />
              </div>

              <div class="hidden overflow-hidden rounded-2xl border border-border/60 bg-background sm:block">
                <div class="overflow-x-auto">
                  <Table class="w-full table-fixed">
                    <TableHeader>
                      <TableRow>
                        <TableHead class="w-[24%] whitespace-nowrap">订单号</TableHead>
                        <TableHead class="w-[12%] whitespace-nowrap">金额</TableHead>
                        <TableHead class="w-[14%] whitespace-nowrap">支付方式</TableHead>
                        <TableHead class="w-[12%] whitespace-nowrap">状态</TableHead>
                        <TableHead class="w-[12%] whitespace-nowrap">可退金额</TableHead>
                        <TableHead class="w-[14%] whitespace-nowrap">创建时间</TableHead>
                        <TableHead class="w-[12%] whitespace-nowrap text-right">操作</TableHead>
                      </TableRow>
                    </TableHeader>
                    <TableBody>
                      <TableRow
                        v-for="order in rechargeOrders"
                        :key="order.id"
                        class="border-b border-border/40 last:border-b-0"
                      >
                        <TableCell class="py-4 align-top">
                          <div
                            class="max-w-full truncate font-mono text-xs text-foreground"
                            :title="order.order_no"
                          >
                            {{ compactOrderNo(order.order_no) }}
                          </div>
                          <div class="mt-1 truncate text-xs text-muted-foreground">
                            钱包充值
                          </div>
                        </TableCell>
                        <TableCell class="py-5 align-top whitespace-nowrap text-sm font-medium tabular-nums">
                          {{ formatCurrency(order.amount_usd) }}
                        </TableCell>
                        <TableCell class="py-5 align-top">
                          <Badge
                            variant="outline"
                            class="h-8 whitespace-nowrap px-3 py-0"
                          >
                            {{ paymentMethodLabel(order.payment_method) }}
                          </Badge>
                        </TableCell>
                        <TableCell class="py-5 align-top">
                          <Badge
                            :variant="paymentStatusBadge(order.status)"
                            class="h-8 whitespace-nowrap px-3 py-0"
                          >
                            {{ paymentStatusLabel(order.status) }}
                          </Badge>
                        </TableCell>
                        <TableCell class="py-5 align-top whitespace-nowrap text-sm font-medium tabular-nums">
                          {{ formatCurrency(order.refundable_amount_usd) }}
                        </TableCell>
                        <TableCell class="py-5 align-top text-sm text-muted-foreground">
                          <div class="whitespace-nowrap">
                            {{ formatDateLabel(order.created_at) }}
                          </div>
                          <div class="mt-1 whitespace-nowrap text-xs text-muted-foreground">
                            {{ formatTimeLabel(order.created_at) }}
                          </div>
                        </TableCell>
                        <TableCell class="py-5 align-top text-right">
                          <div class="flex justify-end gap-1.5 whitespace-nowrap">
                            <a
                              v-if="order.status === 'pending' && rechargeOrderPaymentUrl(order)"
                              :href="rechargeOrderPaymentUrl(order)"
                              target="_blank"
                              rel="noopener noreferrer"
                              class="inline-flex h-8 items-center justify-center rounded-md border border-border bg-background px-3 text-xs font-medium text-foreground transition hover:bg-muted"
                            >
                              去支付
                            </a>
                            <Button
                              v-if="canCancelRechargeOrder(order)"
                              size="sm"
                              variant="outline"
                              class="h-8 px-3"
                              :disabled="cancelingRechargeOrderId === order.id"
                              @click="cancelRechargeOrder(order)"
                            >
                              {{ cancelingRechargeOrderId === order.id ? '取消中...' : '取消订单' }}
                            </Button>
                            <span
                              v-if="!canCancelRechargeOrder(order) && !(order.status === 'pending' && rechargeOrderPaymentUrl(order))"
                              class="inline-block max-w-full truncate text-xs text-muted-foreground"
                              :title="rechargeOrderActionText(order)"
                            >
                              {{ rechargeOrderActionText(order) }}
                            </span>
                          </div>
                        </TableCell>
                      </TableRow>
                      <TableRow v-if="!loadingOrders && rechargeOrders.length === 0">
                        <TableCell
                          colspan="7"
                          class="py-12"
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
              <div class="flex items-center justify-between px-4 sm:px-5">
                <div class="text-sm text-muted-foreground">
                  共 {{ refundTotal }} 条
                </div>
                <RefreshButton
                  :loading="loadingRefunds"
                  @click="loadRefunds"
                />
              </div>

              <div class="space-y-3 px-4 sm:hidden">
                <div
                  v-for="refund in refunds"
                  :key="refund.id"
                  class="rounded-2xl border border-border/60 bg-card/95 p-4 shadow-[0_16px_34px_-30px_hsl(var(--foreground))]"
                >
                  <div class="flex items-start justify-between gap-3">
                    <div class="min-w-0">
                      <div class="flex flex-wrap items-center gap-2">
                        <Badge
                          :variant="refundStatusBadge(refund.status)"
                          class="h-6 whitespace-nowrap px-2.5 py-0 text-[11px]"
                        >
                          {{ refundStatusLabel(refund.status) }}
                        </Badge>
                        <Badge
                          variant="outline"
                          class="h-6 whitespace-nowrap px-2.5 py-0 text-[11px]"
                        >
                          {{ refundModeLabel(refund.refund_mode) }}
                        </Badge>
                      </div>
                      <div
                        class="mt-3 truncate font-mono text-xs text-foreground"
                        :title="refund.refund_no"
                      >
                        {{ compactOrderNo(refund.refund_no) }}
                      </div>
                      <div class="mt-1 text-[11px] text-muted-foreground">
                        退款申请
                      </div>
                    </div>

                    <div class="text-right">
                      <div class="text-[11px] text-muted-foreground">
                        退款金额
                      </div>
                      <div class="mt-1 text-lg font-semibold tabular-nums text-foreground">
                        {{ formatCurrency(refund.amount_usd) }}
                      </div>
                    </div>
                  </div>

                  <div class="mt-4 grid grid-cols-2 gap-2 text-xs">
                    <div class="rounded-xl border border-border/40 bg-muted/18 p-3">
                      <div class="text-muted-foreground">
                        申请时间
                      </div>
                      <div class="mt-1 font-medium text-foreground">
                        {{ formatDateLabel(refund.created_at) }}
                      </div>
                      <div class="mt-0.5 text-[11px] text-muted-foreground">
                        {{ formatTimeLabel(refund.created_at) }}
                      </div>
                    </div>
                    <div class="rounded-xl border border-border/40 bg-muted/18 p-3">
                      <div class="text-muted-foreground">
                        最新进度
                      </div>
                      <div class="mt-1 font-medium text-foreground">
                        {{ refundProgressText(refund) }}
                      </div>
                      <div class="mt-0.5 text-[11px] text-muted-foreground">
                        {{ refundProgressTime(refund) }}
                      </div>
                    </div>
                  </div>

                  <div class="mt-3 rounded-xl border border-border/40 bg-background/85 px-3 py-2.5 text-xs leading-5 text-muted-foreground">
                    {{ refund.reason || refund.failure_reason || '未填写备注' }}
                  </div>
                </div>

                <EmptyState
                  v-if="!loadingRefunds && refunds.length === 0"
                  title="暂无退款记录"
                  description="提交退款申请后会在这里显示"
                />
              </div>

              <div class="hidden overflow-hidden rounded-2xl border border-border/60 bg-background sm:block">
                <div class="overflow-x-auto">
                  <Table class="min-w-[920px] table-fixed">
                    <TableHeader>
                      <TableRow>
                        <TableHead class="w-[24%] whitespace-nowrap">退款单号</TableHead>
                        <TableHead class="w-[12%] whitespace-nowrap">金额</TableHead>
                        <TableHead class="w-[14%] whitespace-nowrap">模式</TableHead>
                        <TableHead class="w-[12%] whitespace-nowrap">状态</TableHead>
                        <TableHead class="w-[22%] whitespace-nowrap">原因</TableHead>
                        <TableHead class="w-[16%] whitespace-nowrap">申请时间</TableHead>
                      </TableRow>
                    </TableHeader>
                    <TableBody>
                      <TableRow
                        v-for="refund in refunds"
                        :key="refund.id"
                        class="border-b border-border/40 last:border-b-0"
                      >
                        <TableCell class="py-5 align-top">
                          <div
                            class="max-w-full truncate font-mono text-xs text-foreground"
                            :title="refund.refund_no"
                          >
                            {{ compactOrderNo(refund.refund_no) }}
                          </div>
                          <div class="mt-1 truncate text-xs text-muted-foreground">
                            退款申请
                          </div>
                        </TableCell>
                        <TableCell class="py-5 align-top whitespace-nowrap text-sm font-medium tabular-nums">
                          {{ formatCurrency(refund.amount_usd) }}
                        </TableCell>
                        <TableCell class="py-5 align-top">
                          <Badge
                            variant="outline"
                            class="h-8 whitespace-nowrap px-3 py-0"
                          >
                            {{ refundModeLabel(refund.refund_mode) }}
                          </Badge>
                        </TableCell>
                        <TableCell class="py-5 align-top">
                          <Badge
                            :variant="refundStatusBadge(refund.status)"
                            class="h-8 whitespace-nowrap px-3 py-0"
                          >
                            {{ refundStatusLabel(refund.status) }}
                          </Badge>
                        </TableCell>
                        <TableCell
                          class="py-5 align-top text-xs text-muted-foreground"
                          :title="refund.reason || refund.failure_reason || '-'"
                        >
                          <div class="truncate">
                            {{ refund.reason || refund.failure_reason || '-' }}
                          </div>
                        </TableCell>
                        <TableCell class="py-5 align-top text-sm text-muted-foreground">
                          <div class="whitespace-nowrap">
                            {{ formatDateLabel(refund.created_at) }}
                          </div>
                          <div class="mt-1 whitespace-nowrap text-xs text-muted-foreground">
                            {{ formatTimeLabel(refund.created_at) }}
                          </div>
                        </TableCell>
                      </TableRow>
                      <TableRow v-if="!loadingRefunds && refunds.length === 0">
                        <TableCell
                          colspan="6"
                          class="py-12"
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

      <Dialog
        v-if="ENABLE_WALLET_RECHARGE_FORM"
        v-model="showRechargeDialog"
        size="md"
      >
        <template #header>
          <div class="border-b border-border px-6 py-4">
            <div class="flex items-center justify-between gap-3">
              <div>
                <h3 class="text-lg font-semibold">
                  钱包充值
                </h3>
                <p class="text-xs text-muted-foreground">
                  选择支付方式并创建充值订单。
                </p>
              </div>
              <RefreshButton
                :loading="loadingOrders"
                @click="loadOrders"
              />
            </div>
          </div>
        </template>

        <div class="space-y-4">
          <div class="rounded-2xl border border-border/60 bg-muted/20 p-4 text-sm">
            <div class="flex items-center justify-between gap-4">
              <span class="text-muted-foreground">当前可用余额</span>
              <span class="font-semibold text-foreground">
                {{ formatCurrency(walletBalance?.balance) }}
              </span>
            </div>
            <div class="mt-3 flex items-center justify-between gap-4">
              <span class="text-muted-foreground">充值余额</span>
              <span class="font-medium text-foreground">
                {{ formatCurrency(walletBalance?.wallet?.recharge_balance) }}
              </span>
            </div>
            <div class="mt-3 flex items-center justify-between gap-4">
              <span class="text-muted-foreground">赠款余额</span>
              <span class="font-medium text-foreground">
                {{ formatCurrency(walletBalance?.wallet?.gift_balance) }}
              </span>
            </div>
          </div>

          <div class="space-y-3">
            <Label>充值金额 (USD)</Label>
            <div class="grid grid-cols-3 gap-2 sm:grid-cols-5">
              <Button
                v-for="amount in quickRechargeAmounts"
                :key="amount"
                type="button"
                :variant="Number(rechargeForm.amount_usd) === amount ? 'secondary' : 'outline'"
                class="h-10"
                @click="rechargeForm.amount_usd = amount"
              >
                ${{ amount }}
              </Button>
            </div>

            <Input
              v-model.number="rechargeForm.amount_usd"
              type="number"
              min="0.01"
              step="0.01"
              placeholder="自定义金额"
            />
          </div>

          <div class="space-y-1.5">
            <Label>支付方式</Label>
            <div class="grid grid-cols-3 gap-2">
              <Button
                type="button"
                :variant="rechargeForm.payment_method === 'alipay' ? 'secondary' : 'outline'"
                @click="rechargeForm.payment_method = 'alipay'"
              >
                支付宝
              </Button>
              <Button
                type="button"
                :variant="rechargeForm.payment_method === 'wechat' ? 'secondary' : 'outline'"
                @click="rechargeForm.payment_method = 'wechat'"
              >
                微信支付
              </Button>
              <Button
                type="button"
                :variant="rechargeForm.payment_method === 'manual_review' ? 'secondary' : 'outline'"
                @click="rechargeForm.payment_method = 'manual_review'"
              >
                人工充值
              </Button>
            </div>
          </div>

          <div class="rounded-xl border border-border/60 bg-muted/20 p-3 text-xs leading-6 text-muted-foreground">
            {{
              rechargeForm.payment_method === 'manual_review'
                ? '人工充值会先创建待审核订单，待管理员确认到账后再入账钱包余额。'
                : '创建订单后可继续完成支付，到账后会自动计入充值余额。'
            }}
          </div>
        </div>

        <template #footer>
          <Button
            variant="ghost"
            :disabled="submittingRecharge"
            @click="showRechargeDialog = false"
          >
            关闭
          </Button>
          <Button
            :disabled="submittingRecharge"
            @click="submitRecharge"
          >
            {{ submittingRecharge ? '创建订单中...' : '确认充值' }}
          </Button>
        </template>
      </Dialog>
    </template>
  </div>
</template>

<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue'
import {
  Badge,
  Button,
  Card,
  Dialog,
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
  type RefundRequest,
  type WalletBalanceResponse,
} from '@/api/wallet'
import { useConfirm } from '@/composables/useConfirm'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'
import { log } from '@/utils/logger'
import {
  dailyUsageCategoryLabel,
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

const { success, error: showError } = useToast()
const { confirm } = useConfirm()

const ENABLE_WALLET_RECHARGE_FORM = true
const ENABLE_WALLET_REFUND_FORM = false

const loadingInitial = ref(true)
const loadingTransactions = ref(false)
const loadingOrders = ref(false)
const loadingRefunds = ref(false)
const submittingRecharge = ref(false)
const submittingRefund = ref(false)
const cancelingRechargeOrderId = ref<string | null>(null)

const walletBalance = ref<WalletBalanceResponse | null>(null)
const latestRecharge = ref<{ order: PaymentOrder; payment_instructions: Record<string, unknown> } | null>(null)

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
const showRechargeDialog = ref(false)
let todayCostPollTimer: ReturnType<typeof setInterval> | null = null
const quickRechargeAmounts = [10, 25, 50, 100, 200]

const rechargeForm = reactive({
  amount_usd: 10,
  payment_method: 'alipay' as 'alipay' | 'wechat' | 'manual_review',
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
  document.addEventListener('visibilitychange', handleVisibilityChange)
  try {
    await Promise.all([
      loadBalance(),
      loadTransactions(),
      loadTodayCost(),
      loadOrders(),
      loadRefunds(),
    ])
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

async function loadOrders() {
  loadingOrders.value = true
  try {
    const offset = (orderPage.value - 1) * orderPageSize.value
    const resp = await walletApi.listRechargeOrders({ limit: orderPageSize.value, offset })
    rechargeOrders.value = resp.items
    orderTotal.value = resp.total
    syncLatestRechargeFromOrders()
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
    success(rechargeForm.payment_method === 'manual_review' ? '人工充值申请已提交' : '充值订单创建成功')
    await Promise.all([loadOrders(), loadBalance()])
    showRechargeDialog.value = false
  } catch (error) {
    log.error('创建充值订单失败:', error)
    showError(parseApiError(error, '创建充值订单失败'))
  } finally {
    submittingRecharge.value = false
  }
}

async function cancelRechargeOrder(order: PaymentOrder) {
  const confirmed = await confirm({
    title: '取消充值订单',
    message: `确认取消订单 ${compactOrderNo(order.order_no)} 吗？取消后需要重新创建充值订单。`,
    confirmText: '确认取消',
    variant: 'warning',
  })
  if (!confirmed) return

  cancelingRechargeOrderId.value = order.id
  try {
    await walletApi.cancelRechargeOrder(order.id)
    success('充值订单已取消')
    await Promise.all([loadOrders(), loadBalance()])
  } catch (error) {
    log.error('取消充值订单失败:', error)
    showError(parseApiError(error, '取消充值订单失败'))
  } finally {
    cancelingRechargeOrderId.value = null
  }
}

function syncLatestRechargeFromOrders() {
  const actionableOrder = rechargeOrders.value.find((order) =>
    ['pending', 'pending_approval', 'paid'].includes(order.status)
  )
  if (!actionableOrder) {
    if (latestRecharge.value && !['pending', 'pending_approval', 'paid'].includes(latestRecharge.value.order.status)) {
      latestRecharge.value = null
    }
    return
  }
  if (latestRecharge.value?.order.id === actionableOrder.id) {
    latestRecharge.value = {
      order: actionableOrder,
      payment_instructions: latestRecharge.value.payment_instructions,
    }
    return
  }
  latestRecharge.value = {
    order: actionableOrder,
    payment_instructions:
      actionableOrder.gateway_response && typeof actionableOrder.gateway_response === 'object'
        ? actionableOrder.gateway_response
        : {},
  }
}

function rechargePaymentUrl(
  item: { order: PaymentOrder; payment_instructions: Record<string, unknown> } | null
): string | null {
  const raw = item?.payment_instructions?.payment_url
  return typeof raw === 'string' && raw ? raw : null
}

function rechargeOrderPaymentUrl(order: PaymentOrder): string | null {
  const raw = order.gateway_response?.payment_url
  return typeof raw === 'string' && raw ? raw : null
}

function canCancelRechargeOrder(order: PaymentOrder): boolean {
  return order.status === 'pending' || order.status === 'pending_approval'
}

function rechargeOrderActionText(order: PaymentOrder): string {
  if (order.status === 'pending_approval') return '审核中'
  if (order.status === 'credited') return '已到账'
  if (order.status === 'paid') return '支付中'
  if (order.status === 'expired') return '已过期'
  if (order.status === 'failed') return '支付失败'
  if (order.status === 'pending') {
    if (order.expires_at) return `截止 ${formatTimeLabel(order.expires_at)}`
    return '待支付'
  }
  return paymentStatusLabel(order.status)
}

function pendingRechargeDateLabel(order: PaymentOrder): string {
  if (order.expires_at) return `截止 ${formatDateTime(order.expires_at)}`
  return `创建 ${formatDateTime(order.created_at)}`
}

function compactOrderNo(orderNo: string | null | undefined): string {
  if (!orderNo) return '-'
  if (orderNo.length <= 26) return orderNo
  return `${orderNo.slice(0, 18)}...${orderNo.slice(-8)}`
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

function formatDateLabel(value: string | null | undefined): string {
  if (!value) return '-'
  return new Date(value).toLocaleDateString('zh-CN', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
  })
}

function formatTimeLabel(value: string | null | undefined): string {
  if (!value) return '-'
  return new Date(value).toLocaleTimeString('zh-CN', {
    hour: '2-digit',
    minute: '2-digit',
  })
}

function refundProgressText(refund: RefundRequest): string {
  if (refund.completed_at) return '已完成'
  if (refund.processed_at) return '已处理'
  if (refund.status === 'processing') return '处理中'
  if (refund.status === 'pending_approval') return '待审批'
  return '最近更新'
}

function refundProgressTime(refund: RefundRequest): string {
  if (refund.completed_at) return formatDateTime(refund.completed_at)
  if (refund.processed_at) return formatDateTime(refund.processed_at)
  return formatDateTime(refund.updated_at)
}
</script>
