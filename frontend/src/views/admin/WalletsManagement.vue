<template>
  <div class="space-y-6 pb-8">
    <Card class="overflow-hidden">
      <div class="border-b border-border/60 px-4 py-4 sm:px-5">
        <div class="flex flex-col gap-3 lg:flex-row lg:items-center lg:justify-between">
          <div>
            <h3 class="text-base font-semibold">
              钱包管理
            </h3>
            <p class="text-xs text-muted-foreground mt-1">
              统一管理资金流水、充值审批、退款审批、充值订单与支付回调
            </p>
          </div>
        </div>
      </div>

      <div class="px-4 py-4 sm:px-5 sm:py-5">
        <Tabs v-model="activeTab">
          <TabsList class="tabs-button-list grid w-full grid-cols-2 gap-1 sm:max-w-[940px] sm:grid-cols-5">
            <TabsTrigger
              value="ledger"
              class="text-xs sm:text-sm"
            >
              资金流水
            </TabsTrigger>
            <TabsTrigger
              value="orders"
              class="text-xs sm:text-sm"
            >
              充值订单
            </TabsTrigger>
            <TabsTrigger
              value="reviews"
              class="text-xs sm:text-sm"
            >
              充值审批
            </TabsTrigger>
            <TabsTrigger
              value="refunds"
              class="text-xs sm:text-sm"
            >
              退款审批
            </TabsTrigger>
            <TabsTrigger
              value="callbacks"
              class="text-xs sm:text-sm"
            >
              回调日志
            </TabsTrigger>
          </TabsList>

          <TabsContent
            value="ledger"
            class="mt-5 space-y-4"
          >
            <div class="flex flex-col gap-3 lg:flex-row lg:items-center lg:justify-between">
              <div class="grid grid-cols-1 gap-2 sm:flex sm:flex-wrap sm:items-center">
                <Select v-model="ledgerCategoryFilter">
                  <SelectTrigger class="w-full sm:w-[170px]">
                    <SelectValue placeholder="一级分类" />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="all">
                      全部分类
                    </SelectItem>
                    <SelectItem value="recharge">
                      充值
                    </SelectItem>
                    <SelectItem value="gift">
                      赠款
                    </SelectItem>
                    <SelectItem value="adjust">
                      调账
                    </SelectItem>
                    <SelectItem value="refund">
                      退款
                    </SelectItem>
                  </SelectContent>
                </Select>

                <Select v-model="ledgerReasonFilter">
                  <SelectTrigger class="w-full sm:w-[180px]">
                    <SelectValue placeholder="二级分类" />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="all">
                      全部二级
                    </SelectItem>
                    <SelectItem
                      v-for="option in ledgerReasonOptions"
                      :key="option.value"
                      :value="option.value"
                    >
                      {{ option.label }}
                    </SelectItem>
                  </SelectContent>
                </Select>

                <Select v-model="ledgerOwnerFilter">
                  <SelectTrigger class="w-full sm:w-[170px]">
                    <SelectValue placeholder="归属类型" />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="all">
                      全部归属
                    </SelectItem>
                    <SelectItem value="user">
                      用户钱包
                    </SelectItem>
                    <SelectItem value="api_key">
                      独立密钥钱包
                    </SelectItem>
                  </SelectContent>
                </Select>
              </div>

              <div class="flex items-center justify-between gap-3">
                <div class="text-sm text-muted-foreground">
                  共 {{ ledgerTotal }} 条
                </div>
                <RefreshButton
                  :loading="loadingLedger"
                  @click="loadLedger"
                />
              </div>
            </div>

            <div class="space-y-2.5 sm:hidden">
              <div
                v-for="tx in ledgerItems"
                :key="tx.id"
                class="rounded-2xl border border-border/60 bg-card/95 p-3.5 shadow-[0_16px_34px_-30px_hsl(var(--foreground))]"
              >
                <div class="flex items-start justify-between gap-3">
                  <div class="min-w-0">
                    <div
                      class="truncate text-sm font-semibold text-foreground"
                      :title="ownerDisplayName(tx.owner_name, tx.owner_type)"
                    >
                      {{ ownerDisplayName(tx.owner_name, tx.owner_type) }}
                    </div>
                    <div
                      class="mt-1 truncate text-[11px] text-muted-foreground"
                      :title="ledgerOwnerMetaLine(tx)"
                    >
                      {{ ledgerOwnerMetaLine(tx) }}
                    </div>
                  </div>
                  <div class="shrink-0 text-right">
                    <div
                      class="text-sm font-semibold tabular-nums"
                      :class="tx.amount >= 0 ? 'text-emerald-600 dark:text-emerald-400' : 'text-rose-600 dark:text-rose-400'"
                    >
                      {{ tx.amount >= 0 ? '+' : '' }}{{ tx.amount.toFixed(4) }}
                    </div>
                    <div class="mt-1 text-[11px] text-muted-foreground">
                      {{ formatDateLabel(tx.created_at) }}
                    </div>
                    <div class="text-[11px] text-muted-foreground">
                      {{ formatTimeLabel(tx.created_at) }}
                    </div>
                  </div>
                </div>

                <div class="mt-2 flex flex-wrap gap-1.5">
                  <Badge
                    variant="outline"
                    class="h-6 whitespace-nowrap px-2 py-0 text-[11px]"
                  >
                    {{ walletTransactionCategoryLabel(tx.category) }}
                  </Badge>
                  <Badge
                    variant="outline"
                    class="h-6 max-w-full px-2 py-0 text-[11px] text-muted-foreground"
                  >
                    <span class="truncate">{{ walletTransactionReasonLabel(tx.reason_code) }}</span>
                  </Badge>
                </div>

                <div class="mt-3 grid gap-2">
                  <div class="rounded-xl border border-border/50 bg-muted/[0.08] p-2.5">
                    <div class="text-[11px] text-muted-foreground">
                      余额变化
                    </div>
                    <div
                      class="mt-1 truncate text-sm font-medium text-foreground tabular-nums"
                      :title="`${tx.balance_before.toFixed(4)}→${tx.balance_after.toFixed(4)}`"
                    >
                      {{ tx.balance_before.toFixed(4) }}→{{ tx.balance_after.toFixed(4) }}
                    </div>
                    <div
                      v-if="tx.recharge_balance_before !== null && tx.recharge_balance_before !== undefined && tx.gift_balance_before !== null && tx.gift_balance_before !== undefined"
                      class="mt-1 truncate text-[11px] text-muted-foreground tabular-nums"
                      :title="`充${Number(tx.recharge_balance_before).toFixed(4)}→${Number(tx.recharge_balance_after ?? 0).toFixed(4)} · 赠${Number(tx.gift_balance_before).toFixed(4)}→${Number(tx.gift_balance_after ?? 0).toFixed(4)}`"
                    >
                      充{{ Number(tx.recharge_balance_before).toFixed(4) }}→{{ Number(tx.recharge_balance_after ?? 0).toFixed(4) }}
                      · 赠{{ Number(tx.gift_balance_before).toFixed(4) }}→{{ Number(tx.gift_balance_after ?? 0).toFixed(4) }}
                    </div>
                  </div>

                  <div class="rounded-xl border border-border/50 bg-muted/[0.08] p-2.5">
                    <div class="text-[11px] text-muted-foreground">
                      说明
                    </div>
                    <div
                      class="mt-1 text-sm text-foreground"
                      :title="tx.description || '-'"
                    >
                      {{ tx.description || '-' }}
                    </div>
                  </div>
                </div>

                <div class="mt-3">
                  <Button
                    variant="outline"
                    size="sm"
                    class="h-8 w-full text-xs"
                    @click="openLedgerDrawer(tx)"
                  >
                    查看详情
                  </Button>
                </div>
              </div>

              <EmptyState
                v-if="!loadingLedger && ledgerItems.length === 0"
                size="sm"
                title="暂无资金流水"
                description="当前筛选条件下没有资金动作记录"
              />
            </div>

            <div class="hidden overflow-hidden rounded-2xl border border-border/60 bg-background sm:block">
              <div class="overflow-x-auto">
                <Table class="w-full table-auto">
                  <TableHeader>
                    <TableRow>
                      <TableHead class="h-10 px-2.5 py-2 whitespace-nowrap">时间</TableHead>
                      <TableHead class="h-10 px-2.5 py-2 whitespace-nowrap">归属</TableHead>
                      <TableHead class="h-10 px-2 py-2 whitespace-nowrap text-center">类型</TableHead>
                      <TableHead class="h-10 px-2 py-2 whitespace-nowrap">金额</TableHead>
                      <TableHead class="h-10 px-2.5 py-2 whitespace-nowrap">余额变化</TableHead>
                      <TableHead class="h-10 px-2.5 py-2 whitespace-nowrap">说明</TableHead>
                      <TableHead class="h-10 px-2.5 py-2 whitespace-nowrap">操作</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    <TableRow
                      v-for="tx in ledgerItems"
                      :key="tx.id"
                      class="border-b border-border/40 last:border-b-0"
                    >
                      <TableCell class="px-2.5 py-2.5 align-top text-sm text-muted-foreground">
                        <div class="whitespace-nowrap">
                          {{ formatDateLabel(tx.created_at) }}
                        </div>
                        <div class="mt-1 whitespace-nowrap text-xs text-muted-foreground">
                          {{ formatTimeLabel(tx.created_at) }}
                        </div>
                      </TableCell>
                      <TableCell class="px-2.5 py-2.5 align-top">
                        <div
                          class="max-w-[140px] truncate font-medium"
                          :title="ownerDisplayName(tx.owner_name, tx.owner_type)"
                        >
                          {{ ownerDisplayName(tx.owner_name, tx.owner_type) }}
                        </div>
                        <div
                          class="mt-1 max-w-[140px] truncate text-xs text-muted-foreground"
                          :title="ledgerOwnerMetaLine(tx)"
                        >
                          {{ ledgerOwnerMetaLine(tx) }}
                        </div>
                      </TableCell>
                      <TableCell class="px-2 py-2.5 align-top text-center">
                        <div class="flex flex-col items-center gap-1">
                          <Badge
                            variant="outline"
                            class="h-8 whitespace-nowrap px-3 py-0"
                          >
                            {{ walletTransactionCategoryLabel(tx.category) }}
                          </Badge>
                          <div
                            class="max-w-full truncate text-xs text-muted-foreground"
                            :title="walletTransactionReasonLabel(tx.reason_code)"
                          >
                            {{ walletTransactionReasonLabel(tx.reason_code) }}
                          </div>
                        </div>
                      </TableCell>
                      <TableCell
                        class="px-2 py-2.5 align-top whitespace-nowrap text-sm font-medium tabular-nums"
                        :class="tx.amount >= 0 ? 'text-emerald-600 dark:text-emerald-400' : 'text-rose-600 dark:text-rose-400'"
                      >
                        {{ tx.amount >= 0 ? '+' : '' }}{{ tx.amount.toFixed(4) }}
                      </TableCell>
                      <TableCell class="px-2.5 py-2.5 align-top">
                        <div
                          class="max-w-full truncate text-sm font-medium text-muted-foreground tabular-nums"
                          :title="`${tx.balance_before.toFixed(4)}→${tx.balance_after.toFixed(4)}`"
                        >
                          {{ tx.balance_before.toFixed(4) }}→{{ tx.balance_after.toFixed(4) }}
                        </div>
                        <div
                          v-if="tx.recharge_balance_before !== null && tx.recharge_balance_before !== undefined && tx.gift_balance_before !== null && tx.gift_balance_before !== undefined"
                          class="mt-1 max-w-full truncate text-xs text-muted-foreground tabular-nums"
                          :title="`充${Number(tx.recharge_balance_before).toFixed(4)}→${Number(tx.recharge_balance_after ?? 0).toFixed(4)} · 赠${Number(tx.gift_balance_before).toFixed(4)}→${Number(tx.gift_balance_after ?? 0).toFixed(4)}`"
                        >
                          充{{ Number(tx.recharge_balance_before).toFixed(4) }}→{{ Number(tx.recharge_balance_after ?? 0).toFixed(4) }}
                          · 赠{{ Number(tx.gift_balance_before).toFixed(4) }}→{{ Number(tx.gift_balance_after ?? 0).toFixed(4) }}
                        </div>
                      </TableCell>
                      <TableCell class="px-2.5 py-2.5 align-top">
                        <div
                          class="max-w-full truncate text-sm text-muted-foreground"
                          :title="tx.description || '-'"
                        >
                          {{ tx.description || '-' }}
                        </div>
                        <div
                          class="mt-1 max-w-full truncate text-xs text-muted-foreground"
                          :title="walletTransactionReasonLabel(tx.reason_code)"
                        >
                          {{ walletTransactionReasonLabel(tx.reason_code) }}
                        </div>
                      </TableCell>
                      <TableCell class="px-2.5 py-2.5 align-top">
                        <Button
                          size="sm"
                          variant="outline"
                          class="h-8 min-w-[66px] whitespace-nowrap px-3"
                          @click="openLedgerDrawer(tx)"
                        >
                          详情
                        </Button>
                      </TableCell>
                    </TableRow>
                    <TableRow v-if="!loadingLedger && ledgerItems.length === 0">
                      <TableCell
                        colspan="7"
                        class="py-12"
                      >
                        <EmptyState
                          title="暂无资金流水"
                          description="当前筛选条件下没有资金动作记录"
                        />
                      </TableCell>
                    </TableRow>
                  </TableBody>
                </Table>
              </div>
            </div>

            <Pagination
              :current="ledgerPage"
              :total="ledgerTotal"
              :page-size="ledgerPageSize"
              @update:current="handleLedgerPageChange"
              @update:page-size="handleLedgerPageSizeChange"
            />
          </TabsContent>

          <TabsContent
            value="refunds"
            class="mt-5 space-y-4"
          >
            <div class="flex flex-col gap-3 lg:flex-row lg:items-center lg:justify-between">
              <div class="grid grid-cols-1 gap-2 sm:flex sm:flex-wrap sm:items-center">
                <Select v-model="refundStatusFilter">
                  <SelectTrigger class="w-full sm:w-[170px]">
                    <SelectValue placeholder="退款状态" />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="all">
                      全部状态
                    </SelectItem>
                    <SelectItem value="pending_approval">
                      待审批
                    </SelectItem>
                    <SelectItem value="approved">
                      已审批
                    </SelectItem>
                    <SelectItem value="processing">
                      处理中
                    </SelectItem>
                    <SelectItem value="succeeded">
                      已完成
                    </SelectItem>
                    <SelectItem value="failed">
                      已失败
                    </SelectItem>
                  </SelectContent>
                </Select>

                <Select v-model="refundOwnerFilter">
                  <SelectTrigger class="w-full sm:w-[170px]">
                    <SelectValue placeholder="归属类型" />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="all">
                      全部归属
                    </SelectItem>
                    <SelectItem value="user">
                      用户钱包
                    </SelectItem>
                  </SelectContent>
                </Select>
              </div>

              <div class="flex items-center justify-between gap-3">
                <div class="text-sm text-muted-foreground">
                  共 {{ refundTotal }} 条
                </div>
                <RefreshButton
                  :loading="loadingRefunds"
                  @click="loadRefunds"
                />
              </div>
            </div>

            <div class="space-y-2.5 sm:hidden">
              <div
                v-for="refund in refundItems"
                :key="refund.id"
                class="rounded-2xl border border-border/60 bg-card/95 p-3.5 shadow-[0_16px_34px_-30px_hsl(var(--foreground))]"
              >
                <div class="flex items-start justify-between gap-3">
                  <div class="min-w-0">
                    <div class="truncate text-sm font-semibold text-foreground">
                      {{ ownerDisplayName(refund.owner_name, refund.owner_type) }}
                    </div>
                    <div class="mt-1 truncate text-[11px] text-muted-foreground">
                      {{ ownerTypeLabel(refund.owner_type) }}
                    </div>
                  </div>
                  <div class="shrink-0 text-right">
                    <div class="text-sm font-semibold tabular-nums text-foreground">
                      {{ formatCurrency(refund.amount_usd) }}
                    </div>
                    <div class="mt-1 text-[11px] text-muted-foreground">
                      {{ formatDateLabel(refund.created_at) }}
                    </div>
                    <div class="text-[11px] text-muted-foreground">
                      {{ formatTimeLabel(refund.created_at) }}
                    </div>
                  </div>
                </div>

                <div class="mt-2 flex flex-wrap gap-1.5">
                  <Badge
                    :variant="refundStatusBadge(refund.status)"
                    class="h-6 whitespace-nowrap px-2 py-0 text-[11px]"
                  >
                    {{ refundStatusLabel(refund.status) }}
                  </Badge>
                  <Badge
                    variant="outline"
                    class="h-6 whitespace-nowrap px-2 py-0 text-[11px]"
                  >
                    {{ refundModeLabel(refund.refund_mode) }}
                  </Badge>
                  <Badge
                    v-if="refund.wallet_status"
                    variant="outline"
                    class="h-6 whitespace-nowrap px-2 py-0 text-[11px] text-muted-foreground"
                  >
                    {{ walletStatusLabel(refund.wallet_status) }}
                  </Badge>
                </div>

                <div class="mt-3 grid gap-2">
                  <div class="rounded-xl border border-border/50 bg-muted/[0.08] p-2.5">
                    <div class="text-[11px] text-muted-foreground">
                      退款单号
                    </div>
                    <div
                      class="mt-1 truncate font-mono text-xs text-foreground"
                      :title="refund.refund_no"
                    >
                      {{ refund.refund_no }}
                    </div>
                  </div>
                  <div class="rounded-xl border border-border/50 bg-muted/[0.08] p-2.5">
                    <div class="text-[11px] text-muted-foreground">
                      原因
                    </div>
                    <div
                      class="mt-1 text-sm text-foreground"
                      :title="refund.reason || refund.failure_reason || '-'"
                    >
                      {{ refund.reason || refund.failure_reason || '-' }}
                    </div>
                  </div>
                </div>

                <div class="mt-3">
                  <Button
                    variant="outline"
                    size="sm"
                    class="h-8 w-full text-xs"
                    @click="openRefundDrawer(refund)"
                  >
                    处理审批
                  </Button>
                </div>
              </div>

              <EmptyState
                v-if="!loadingRefunds && refundItems.length === 0"
                size="sm"
                title="暂无退款申请"
                description="当前筛选条件下没有退款单"
              />
            </div>

            <div class="hidden overflow-hidden rounded-2xl border border-border/60 bg-background sm:block">
              <div class="overflow-x-auto">
                <Table class="w-full table-auto">
                  <TableHeader>
                    <TableRow>
                      <TableHead class="h-10 px-3 py-2">归属</TableHead>
                      <TableHead class="h-10 px-2.5 py-2 whitespace-nowrap">退款单号</TableHead>
                      <TableHead class="h-10 px-2 py-2 whitespace-nowrap">金额</TableHead>
                      <TableHead class="h-10 px-2 py-2 whitespace-nowrap">模式</TableHead>
                      <TableHead class="h-10 px-2 py-2 whitespace-nowrap">状态</TableHead>
                      <TableHead class="h-10 px-2.5 py-2">原因</TableHead>
                      <TableHead class="h-10 px-2.5 py-2 whitespace-nowrap">申请时间</TableHead>
                      <TableHead class="h-10 px-3 py-2 text-right whitespace-nowrap">
                        操作
                      </TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    <TableRow
                      v-for="refund in refundItems"
                      :key="refund.id"
                      class="hover:bg-muted/20"
                    >
                      <TableCell class="px-3 py-2.5 align-top">
                        <div class="font-medium text-sm">
                          {{ ownerDisplayName(refund.owner_name, refund.owner_type) }}
                        </div>
                        <div class="mt-1 flex items-center gap-1.5 text-xs text-muted-foreground">
                          <span>{{ ownerTypeLabel(refund.owner_type) }}</span>
                          <Badge
                            v-if="refund.wallet_status"
                            variant="outline"
                            class="text-[10px]"
                          >
                            {{ walletStatusLabel(refund.wallet_status) }}
                          </Badge>
                        </div>
                      </TableCell>
                      <TableCell
                        class="max-w-[180px] px-2.5 py-2.5 align-top font-mono text-xs"
                        :title="refund.refund_no"
                      >
                        <div class="truncate">
                          {{ refund.refund_no }}
                        </div>
                      </TableCell>
                      <TableCell class="px-2 py-2.5 align-top tabular-nums whitespace-nowrap">
                        {{ formatCurrency(refund.amount_usd) }}
                      </TableCell>
                      <TableCell class="px-2 py-2.5 align-top whitespace-nowrap">
                        {{ refundModeLabel(refund.refund_mode) }}
                      </TableCell>
                      <TableCell class="px-2 py-2.5 align-top">
                        <Badge :variant="refundStatusBadge(refund.status)">
                          {{ refundStatusLabel(refund.status) }}
                        </Badge>
                      </TableCell>
                      <TableCell
                        class="max-w-[240px] px-2.5 py-2.5 align-top text-xs text-muted-foreground"
                        :title="refund.reason || refund.failure_reason || '-'"
                      >
                        <div class="truncate">
                          {{ refund.reason || refund.failure_reason || '-' }}
                        </div>
                      </TableCell>
                      <TableCell class="px-2.5 py-2.5 align-top text-xs text-muted-foreground whitespace-nowrap">
                        {{ formatDateTime(refund.created_at) }}
                      </TableCell>
                      <TableCell class="px-3 py-2.5 align-top text-right">
                        <div class="flex justify-end gap-1">
                          <Button
                            size="sm"
                            variant="outline"
                            @click="openRefundDrawer(refund)"
                          >
                            审批
                          </Button>
                        </div>
                      </TableCell>
                    </TableRow>
                    <TableRow v-if="!loadingRefunds && refundItems.length === 0">
                      <TableCell
                        colspan="8"
                        class="py-12"
                      >
                        <EmptyState
                          title="暂无退款申请"
                          description="当前筛选条件下没有退款单"
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

          <TabsContent
            value="orders"
            class="mt-5 space-y-4"
          >
            <div class="flex flex-col gap-3 lg:flex-row lg:items-center lg:justify-between">
              <div class="grid grid-cols-1 gap-2 sm:flex sm:flex-wrap sm:items-center">
                <Select v-model="orderStatusFilter">
                  <SelectTrigger class="w-full sm:w-[180px]">
                    <SelectValue placeholder="订单状态" />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="all">
                      全部状态
                    </SelectItem>
                    <SelectItem value="pending">
                      待支付
                    </SelectItem>
                    <SelectItem value="pending_approval">
                      待审核
                    </SelectItem>
                    <SelectItem value="paid">
                      已支付
                    </SelectItem>
                    <SelectItem value="credited">
                      已到账
                    </SelectItem>
                    <SelectItem value="failed">
                      支付失败
                    </SelectItem>
                    <SelectItem value="expired">
                      已过期
                    </SelectItem>
                  </SelectContent>
                </Select>

                <Select v-model="orderMethodFilter">
                  <SelectTrigger class="w-full sm:w-[180px]">
                    <SelectValue placeholder="支付方式" />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="all">
                      全部方式
                    </SelectItem>
                    <SelectItem value="alipay">
                      支付宝
                    </SelectItem>
                    <SelectItem value="wechat">
                      微信支付
                    </SelectItem>
                    <SelectItem value="manual_review">
                      人工充值
                    </SelectItem>
                    <SelectItem value="admin_manual">
                      人工充值
                    </SelectItem>
                    <SelectItem value="card_code">
                      充值卡
                    </SelectItem>
                    <SelectItem value="gift_code">
                      礼品卡
                    </SelectItem>
                    <SelectItem value="card_recharge">
                      卡密充值
                    </SelectItem>
                  </SelectContent>
                </Select>
              </div>

              <div class="flex items-center justify-between gap-3">
                <div class="text-sm text-muted-foreground">
                  共 {{ orderTotal }} 条
                </div>
                <RefreshButton
                  :loading="loadingOrders"
                  @click="loadOrders"
                />
              </div>
            </div>

            <div class="space-y-2.5 sm:hidden">
              <div
                v-for="order in orders"
                :key="order.id"
                class="rounded-2xl border border-border/60 bg-card/95 p-3.5 shadow-[0_16px_34px_-30px_hsl(var(--foreground))]"
              >
                <div class="flex items-start justify-between gap-3">
                  <div class="min-w-0">
                    <div
                      class="truncate text-sm font-semibold text-foreground"
                      :title="order.order_no"
                    >
                      {{ compactOrderNo(order.order_no) }}
                    </div>
                    <div
                      class="mt-1 truncate text-[11px] text-muted-foreground"
                      :title="orderOwnerName(order.wallet_id)"
                    >
                      {{ orderOwnerName(order.wallet_id) }}
                    </div>
                  </div>
                  <div class="shrink-0 text-right">
                    <div class="text-sm font-semibold tabular-nums text-foreground">
                      {{ formatCurrency(order.amount_usd) }}
                    </div>
                    <div class="mt-1 text-[11px] text-muted-foreground">
                      {{ formatDateLabel(order.created_at) }}
                    </div>
                    <div class="text-[11px] text-muted-foreground">
                      {{ formatTimeLabel(order.created_at) }}
                    </div>
                  </div>
                </div>

                <div class="mt-2 flex flex-wrap gap-1.5">
                  <Badge
                    variant="outline"
                    class="h-6 whitespace-nowrap px-2 py-0 text-[11px]"
                  >
                    {{ paymentMethodLabel(order.payment_method) }}
                  </Badge>
                  <Badge
                    :variant="paymentStatusBadge(order.status)"
                    class="h-6 whitespace-nowrap px-2 py-0 text-[11px]"
                  >
                    {{ paymentStatusLabel(order.status) }}
                  </Badge>
                </div>

                <div class="mt-3 rounded-xl border border-border/50 bg-muted/[0.08] p-2.5">
                  <div class="text-[11px] text-muted-foreground">
                    归属
                  </div>
                  <div class="mt-1 truncate text-sm text-foreground">
                    {{ orderOwnerName(order.wallet_id) }}
                  </div>
                  <div class="mt-1 truncate text-[11px] text-muted-foreground">
                    {{ orderOwnerMetaLine(order.wallet_id) }}
                  </div>
                </div>
              </div>

              <EmptyState
                v-if="!loadingOrders && orders.length === 0"
                size="sm"
                title="暂无充值订单"
                description="当前筛选条件下没有数据"
              />
            </div>

            <div class="hidden overflow-hidden rounded-2xl border border-border/60 bg-background sm:block">
              <div class="overflow-x-auto">
                <Table class="w-full table-auto">
                  <TableHeader>
                    <TableRow>
                      <TableHead class="h-10 px-3 py-2 whitespace-nowrap">订单</TableHead>
                      <TableHead class="h-10 px-2.5 py-2 whitespace-nowrap">归属</TableHead>
                      <TableHead class="h-10 px-2 py-2 whitespace-nowrap">金额</TableHead>
                      <TableHead class="h-10 px-2 py-2 whitespace-nowrap text-center">支付方式</TableHead>
                      <TableHead class="h-10 px-2 py-2 whitespace-nowrap text-center">状态</TableHead>
                      <TableHead class="h-10 px-2.5 py-2 whitespace-nowrap">创建时间</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    <TableRow
                      v-for="order in orders"
                      :key="order.id"
                      class="border-b border-border/40 last:border-b-0"
                    >
                      <TableCell class="px-3 py-2.5 align-top">
                        <div
                          class="max-w-full truncate font-medium"
                          :title="order.order_no"
                        >
                          {{ compactOrderNo(order.order_no) }}
                        </div>
                        <div class="mt-1 truncate text-xs text-muted-foreground">
                          充值订单
                        </div>
                      </TableCell>
                      <TableCell class="px-2.5 py-2.5 align-top">
                        <div
                          class="max-w-full truncate font-medium"
                          :title="orderOwnerName(order.wallet_id)"
                        >
                          {{ orderOwnerName(order.wallet_id) }}
                        </div>
                        <div
                          class="mt-1 max-w-full truncate text-xs text-muted-foreground"
                          :title="orderOwnerMetaLine(order.wallet_id)"
                        >
                          {{ orderOwnerMetaLine(order.wallet_id) }}
                        </div>
                      </TableCell>
                      <TableCell class="px-2 py-2.5 align-top whitespace-nowrap text-sm font-medium tabular-nums">
                        {{ formatCurrency(order.amount_usd) }}
                      </TableCell>
                      <TableCell class="px-2 py-2.5 align-top text-center">
                        <Badge
                          variant="outline"
                          class="h-8 whitespace-nowrap px-3 py-0"
                        >
                          {{ paymentMethodLabel(order.payment_method) }}
                        </Badge>
                      </TableCell>
                      <TableCell class="px-2 py-2.5 align-top text-center">
                        <Badge
                          :variant="paymentStatusBadge(order.status)"
                          class="h-8 whitespace-nowrap px-3 py-0"
                        >
                          {{ paymentStatusLabel(order.status) }}
                        </Badge>
                      </TableCell>
                      <TableCell class="px-2.5 py-2.5 align-top text-sm text-muted-foreground">
                        <div class="whitespace-nowrap">
                          {{ formatDateLabel(order.created_at) }}
                        </div>
                        <div class="mt-1 whitespace-nowrap text-xs text-muted-foreground">
                          {{ formatTimeLabel(order.created_at) }}
                        </div>
                      </TableCell>
                    </TableRow>
                      <TableRow v-if="!loadingOrders && orders.length === 0">
                        <TableCell
                          colspan="6"
                          class="py-12 text-center text-sm text-muted-foreground"
                        >
                          <EmptyState
                            title="暂无充值订单"
                          description="当前筛选条件下没有数据"
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
            value="reviews"
            class="mt-5 space-y-4"
          >
            <div class="flex flex-col gap-3 lg:flex-row lg:items-center lg:justify-between">
              <div>
                <div class="text-sm font-medium">
                  待审核充值订单
                </div>
                <p class="mt-1 text-xs text-muted-foreground">
                  审核用户提交的人工充值订单，审批通过后会直接入账钱包余额。
                </p>
              </div>

              <div class="flex items-center justify-between gap-3">
                <div class="text-sm text-muted-foreground">
                  共 {{ reviewTotal }} 条
                </div>
                <RefreshButton
                  :loading="loadingReviewOrders"
                  @click="loadReviewOrders"
                />
              </div>
            </div>

            <div class="space-y-2.5 sm:hidden">
              <div
                v-for="order in reviewOrders"
                :key="order.id"
                class="rounded-2xl border border-border/60 bg-card/95 p-3.5 shadow-[0_16px_34px_-30px_hsl(var(--foreground))]"
              >
                <div class="flex items-start justify-between gap-3">
                  <div class="min-w-0">
                    <div
                      class="truncate text-sm font-semibold text-foreground"
                      :title="order.order_no"
                    >
                      {{ compactOrderNo(order.order_no) }}
                    </div>
                    <div class="mt-1 truncate text-[11px] text-muted-foreground">
                      {{ orderOwnerName(order.wallet_id) }}
                    </div>
                  </div>
                  <div class="shrink-0 text-right">
                    <div class="text-sm font-semibold tabular-nums text-foreground">
                      {{ formatCurrency(order.amount_usd) }}
                    </div>
                    <div class="mt-1 text-[11px] text-muted-foreground">
                      {{ formatDateLabel(order.created_at) }}
                    </div>
                    <div class="text-[11px] text-muted-foreground">
                      {{ formatTimeLabel(order.created_at) }}
                    </div>
                  </div>
                </div>

                <div class="mt-2 flex flex-wrap gap-1.5">
                  <Badge
                    variant="outline"
                    class="h-6 whitespace-nowrap px-2 py-0 text-[11px]"
                  >
                    {{ paymentMethodLabel(order.payment_method) }}
                  </Badge>
                  <Badge
                    variant="outline"
                    class="h-6 whitespace-nowrap px-2 py-0 text-[11px] text-muted-foreground"
                  >
                    待审核
                  </Badge>
                </div>

                <div class="mt-3 rounded-xl border border-border/50 bg-muted/[0.08] p-2.5">
                  <div class="text-[11px] text-muted-foreground">
                    归属
                  </div>
                  <div class="mt-1 truncate text-sm text-foreground">
                    {{ orderOwnerName(order.wallet_id) }}
                  </div>
                  <div class="mt-1 truncate text-[11px] text-muted-foreground">
                    {{ orderOwnerMetaLine(order.wallet_id) }}
                  </div>
                </div>

                <div class="mt-3 grid grid-cols-2 gap-2">
                  <Button
                    variant="outline"
                    size="sm"
                    class="h-8 text-xs"
                    :disabled="submittingOrderAction"
                    @click="openCreditDialog(order)"
                  >
                    <CheckCircle2 class="mr-1.5 h-3.5 w-3.5" />
                    通过
                  </Button>
                  <Button
                    variant="outline"
                    size="sm"
                    class="h-8 border-rose-200 text-xs text-rose-600 hover:bg-rose-50 dark:border-rose-900/60 dark:hover:bg-rose-950/40"
                    :disabled="submittingOrderAction"
                    @click="rejectReviewOrder(order)"
                  >
                    <XCircle class="mr-1.5 h-3.5 w-3.5" />
                    拒绝
                  </Button>
                </div>
              </div>

              <EmptyState
                v-if="!loadingReviewOrders && reviewOrders.length === 0"
                size="sm"
                title="暂无待审核充值订单"
                description="当前没有需要人工审批的钱包充值订单"
              />
            </div>

            <div class="hidden overflow-hidden rounded-2xl border border-border/60 bg-background sm:block">
              <div class="overflow-x-auto">
                <Table class="w-full table-auto">
                  <TableHeader>
                    <TableRow>
                      <TableHead class="h-10 px-3 py-2 whitespace-nowrap">订单</TableHead>
                      <TableHead class="h-10 px-2.5 py-2 whitespace-nowrap">归属</TableHead>
                      <TableHead class="h-10 px-2 py-2 whitespace-nowrap">金额</TableHead>
                      <TableHead class="h-10 px-2 py-2 whitespace-nowrap text-center">支付方式</TableHead>
                      <TableHead class="h-10 px-2.5 py-2 whitespace-nowrap">提交时间</TableHead>
                      <TableHead class="h-10 px-3 py-2 whitespace-nowrap text-right">
                        审核
                      </TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    <TableRow
                      v-for="order in reviewOrders"
                      :key="order.id"
                      class="border-b border-border/40 last:border-b-0"
                    >
                      <TableCell class="px-3 py-2.5 align-top">
                        <div
                          class="max-w-full truncate font-medium"
                          :title="order.order_no"
                        >
                          {{ compactOrderNo(order.order_no) }}
                        </div>
                        <div class="mt-1 truncate text-xs text-muted-foreground">
                          人工充值
                        </div>
                      </TableCell>
                      <TableCell class="px-2.5 py-2.5 align-top">
                        <div
                          class="max-w-full truncate font-medium"
                          :title="orderOwnerName(order.wallet_id)"
                        >
                          {{ orderOwnerName(order.wallet_id) }}
                        </div>
                        <div
                          class="mt-1 max-w-full truncate text-xs text-muted-foreground"
                          :title="orderOwnerMetaLine(order.wallet_id)"
                        >
                          {{ orderOwnerMetaLine(order.wallet_id) }}
                        </div>
                      </TableCell>
                      <TableCell class="px-2 py-2.5 align-top whitespace-nowrap text-sm font-medium tabular-nums">
                        {{ formatCurrency(order.amount_usd) }}
                      </TableCell>
                      <TableCell class="px-2 py-2.5 align-top text-center">
                        <Badge
                          variant="outline"
                          class="h-8 whitespace-nowrap px-3 py-0"
                        >
                          {{ paymentMethodLabel(order.payment_method) }}
                        </Badge>
                      </TableCell>
                      <TableCell class="px-2.5 py-2.5 align-top text-sm text-muted-foreground">
                        <div class="whitespace-nowrap">
                          {{ formatDateLabel(order.created_at) }}
                        </div>
                        <div class="mt-1 whitespace-nowrap text-xs text-muted-foreground">
                          {{ formatTimeLabel(order.created_at) }}
                        </div>
                      </TableCell>
                      <TableCell class="px-3 py-2.5 align-top text-right">
                        <div class="flex justify-end gap-1.5 whitespace-nowrap">
                          <Button
                            variant="outline"
                            size="sm"
                            class="h-8 min-w-[66px] whitespace-nowrap px-3"
                            :disabled="submittingOrderAction"
                            @click="openCreditDialog(order)"
                          >
                            <CheckCircle2 class="mr-1.5 h-4 w-4" />
                            通过
                          </Button>
                          <Button
                            variant="outline"
                            size="sm"
                            class="h-8 min-w-[66px] whitespace-nowrap border-rose-200 px-3 text-rose-600 hover:bg-rose-50 dark:border-rose-900/60 dark:hover:bg-rose-950/40"
                            :disabled="submittingOrderAction"
                            @click="rejectReviewOrder(order)"
                          >
                            <XCircle class="mr-1.5 h-4 w-4" />
                            拒绝
                          </Button>
                        </div>
                      </TableCell>
                    </TableRow>
                      <TableRow v-if="!loadingReviewOrders && reviewOrders.length === 0">
                        <TableCell
                          colspan="6"
                          class="py-12 text-center text-sm text-muted-foreground"
                        >
                          <EmptyState
                            title="暂无待审核充值订单"
                          description="当前没有需要人工审批的钱包充值订单"
                        />
                      </TableCell>
                    </TableRow>
                  </TableBody>
                </Table>
              </div>
            </div>

            <Pagination
              :current="reviewPage"
              :total="reviewTotal"
              :page-size="reviewPageSize"
              @update:current="handleReviewPageChange"
              @update:page-size="handleReviewPageSizeChange"
            />
          </TabsContent>

          <TabsContent
            value="callbacks"
            class="mt-5 space-y-4"
          >
            <div class="flex flex-col gap-3 lg:flex-row lg:items-center lg:justify-between">
              <div class="grid grid-cols-1 gap-2 sm:flex sm:flex-wrap sm:items-center">
                <Select v-model="callbackMethodFilter">
                  <SelectTrigger class="w-full sm:w-[180px]">
                    <SelectValue placeholder="支付方式" />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="all">
                      全部方式
                    </SelectItem>
                    <SelectItem value="alipay">
                      支付宝
                    </SelectItem>
                    <SelectItem value="wechat">
                      微信支付
                    </SelectItem>
                  </SelectContent>
                </Select>
              </div>
              <div class="flex items-center justify-between gap-3">
                <div class="text-sm text-muted-foreground">
                  共 {{ callbackTotal }} 条
                </div>
                <RefreshButton
                  :loading="loadingCallbacks"
                  @click="loadCallbacks"
                />
              </div>
            </div>

            <div class="space-y-2.5 sm:hidden">
              <div
                v-for="callback in callbacks"
                :key="callback.id"
                class="rounded-2xl border border-border/60 bg-card/95 p-3.5 shadow-[0_16px_34px_-30px_hsl(var(--foreground))]"
              >
                <div class="flex items-start justify-between gap-3">
                  <div class="min-w-0">
                    <div
                      class="truncate font-mono text-xs font-medium text-foreground"
                      :title="callback.callback_key"
                    >
                      {{ callback.callback_key }}
                    </div>
                    <div
                      class="mt-1 truncate text-[11px] text-muted-foreground"
                      :title="callback.order_no || '-'"
                    >
                      订单 {{ callback.order_no || '-' }}
                    </div>
                  </div>
                  <div class="shrink-0 text-right">
                    <div class="text-[11px] text-muted-foreground">
                      {{ formatDateLabel(callback.created_at) }}
                    </div>
                    <div class="text-[11px] text-muted-foreground">
                      {{ formatTimeLabel(callback.created_at) }}
                    </div>
                  </div>
                </div>

                <div class="mt-2 flex flex-wrap gap-1.5">
                  <Badge
                    variant="outline"
                    class="h-6 whitespace-nowrap px-2 py-0 text-[11px]"
                  >
                    {{ paymentMethodLabel(callback.payment_method) }}
                  </Badge>
                  <Badge
                    :variant="callback.signature_valid ? 'success' : 'destructive'"
                    class="h-6 whitespace-nowrap px-2 py-0 text-[11px]"
                  >
                    {{ callback.signature_valid ? '验签通过' : '验签失败' }}
                  </Badge>
                  <Badge
                    :variant="callbackStatusBadge(callback.status)"
                    class="h-6 whitespace-nowrap px-2 py-0 text-[11px]"
                  >
                    {{ callbackStatusLabel(callback.status) }}
                  </Badge>
                </div>
              </div>

              <EmptyState
                v-if="!loadingCallbacks && callbacks.length === 0"
                size="sm"
                title="暂无充值回调"
                description="当前筛选条件下没有数据"
              />
            </div>

            <div class="hidden overflow-hidden rounded-2xl border border-border/60 bg-background sm:block">
              <div class="overflow-x-auto">
                <Table class="w-full table-auto">
                  <TableHeader>
                    <TableRow>
                      <TableHead class="h-10 px-3 py-2">回调键</TableHead>
                      <TableHead class="h-10 px-2.5 py-2 whitespace-nowrap">订单号</TableHead>
                      <TableHead class="h-10 px-2 py-2 whitespace-nowrap">方式</TableHead>
                      <TableHead class="h-10 px-2 py-2 whitespace-nowrap">验签</TableHead>
                      <TableHead class="h-10 px-2 py-2 whitespace-nowrap">状态</TableHead>
                      <TableHead class="h-10 px-2.5 py-2 whitespace-nowrap">时间</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    <TableRow
                      v-for="callback in callbacks"
                      :key="callback.id"
                    >
                      <TableCell
                        class="max-w-[240px] px-3 py-2.5 align-top font-mono text-xs"
                        :title="callback.callback_key"
                      >
                        <div class="truncate">
                          {{ callback.callback_key }}
                        </div>
                      </TableCell>
                      <TableCell
                        class="max-w-[180px] px-2.5 py-2.5 align-top font-mono text-xs"
                        :title="callback.order_no || '-'"
                      >
                        <div class="truncate whitespace-nowrap">
                          {{ callback.order_no || '-' }}
                        </div>
                      </TableCell>
                      <TableCell class="px-2 py-2.5 align-top whitespace-nowrap">{{ paymentMethodLabel(callback.payment_method) }}</TableCell>
                      <TableCell class="px-2 py-2.5 align-top">
                        <Badge :variant="callback.signature_valid ? 'success' : 'destructive'">
                          {{ callback.signature_valid ? '通过' : '失败' }}
                        </Badge>
                      </TableCell>
                      <TableCell class="px-2 py-2.5 align-top">
                        <Badge :variant="callbackStatusBadge(callback.status)">
                          {{ callbackStatusLabel(callback.status) }}
                        </Badge>
                      </TableCell>
                      <TableCell class="px-2.5 py-2.5 align-top text-xs text-muted-foreground whitespace-nowrap">
                        {{ formatDateTime(callback.created_at) }}
                      </TableCell>
                    </TableRow>
                    <TableRow v-if="!loadingCallbacks && callbacks.length === 0">
                      <TableCell
                        colspan="6"
                        class="py-10"
                      >
                        <EmptyState
                          title="暂无充值回调"
                          description="当前筛选条件下没有数据"
                        />
                      </TableCell>
                    </TableRow>
                  </TableBody>
                </Table>
              </div>
            </div>

            <Pagination
              :current="callbackPage"
              :total="callbackTotal"
              :page-size="callbackPageSize"
              @update:current="handleCallbackPageChange"
              @update:page-size="handleCallbackPageSizeChange"
            />
          </TabsContent>
        </Tabs>
      </div>
    </Card>

    <Teleport to="body">
      <Transition name="drawer">
        <div
          v-if="showLedgerDrawer && currentLedger"
          class="fixed inset-0 z-[80] flex justify-end"
        >
          <div
            class="absolute inset-0 bg-black/35 backdrop-blur-sm"
            @click="closeLedgerDrawer"
          />
          <div class="drawer-panel relative h-full w-full sm:w-[760px] lg:w-[860px] sm:max-w-[95vw] border-l border-border bg-background shadow-2xl overflow-y-auto">
            <div class="sticky top-0 z-10 border-b border-border bg-background/95 backdrop-blur px-4 py-3 sm:px-6 sm:py-4">
              <div class="flex items-start justify-between gap-3">
                <div class="min-w-0">
                  <h3 class="text-lg font-semibold text-foreground leading-tight">
                    流水详情
                  </h3>
                  <p class="text-xs text-muted-foreground">
                    资金动作审计信息
                  </p>
                </div>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-9 w-9 shrink-0"
                  title="关闭"
                  @click="closeLedgerDrawer"
                >
                  <X class="h-4 w-4" />
                </Button>
              </div>
            </div>

            <div class="p-4 sm:p-6 space-y-5">
              <div class="rounded-2xl border border-border/60 bg-muted/30 p-4 space-y-3">
                <div class="flex flex-wrap items-center justify-between gap-2">
                  <div class="flex items-center gap-2">
                    <Badge variant="outline">
                      {{ walletTransactionCategoryLabel(currentLedger.category) }}
                    </Badge>
                    <Badge variant="secondary">
                      {{ walletTransactionReasonLabel(currentLedger.reason_code) }}
                    </Badge>
                  </div>
                  <span
                    class="text-sm font-semibold tabular-nums"
                    :class="currentLedger.amount >= 0 ? 'text-emerald-600' : 'text-rose-600'"
                  >
                    {{ currentLedger.amount >= 0 ? '+' : '' }}{{ currentLedger.amount.toFixed(4) }}
                  </span>
                </div>
                <div class="text-xs text-muted-foreground">
                  {{ formatDateTime(currentLedger.created_at) }}
                </div>
              </div>

              <div class="grid gap-3 sm:grid-cols-2">
                <div class="rounded-xl border border-border/60 p-3">
                  <div class="text-[11px] uppercase tracking-[0.18em] text-muted-foreground">
                    归属
                  </div>
                  <div class="mt-1 text-sm font-medium">
                    {{ ownerDisplayName(currentLedger.owner_name, currentLedger.owner_type) }}
                  </div>
                  <div class="mt-1 text-xs text-muted-foreground flex items-center gap-2">
                    <span>{{ ownerTypeLabel(currentLedger.owner_type) }}</span>
                    <Badge
                      v-if="currentLedger.wallet_status"
                      variant="outline"
                      class="text-[10px]"
                    >
                      {{ walletStatusLabel(currentLedger.wallet_status) }}
                    </Badge>
                  </div>
                </div>
                <div class="rounded-xl border border-border/60 p-3">
                  <div class="text-[11px] uppercase tracking-[0.18em] text-muted-foreground">
                    余额变化
                  </div>
                  <div class="mt-1 text-sm font-medium tabular-nums">
                    {{ currentLedger.balance_before.toFixed(4) }} → {{ currentLedger.balance_after.toFixed(4) }}
                  </div>
                  <div
                    v-if="currentLedger.recharge_balance_before !== null && currentLedger.recharge_balance_before !== undefined && currentLedger.gift_balance_before !== null && currentLedger.gift_balance_before !== undefined"
                    class="mt-1 text-xs text-muted-foreground tabular-nums"
                  >
                    充 {{ Number(currentLedger.recharge_balance_before).toFixed(4) }}→{{ Number(currentLedger.recharge_balance_after ?? 0).toFixed(4) }}
                    · 赠 {{ Number(currentLedger.gift_balance_before).toFixed(4) }}→{{ Number(currentLedger.gift_balance_after ?? 0).toFixed(4) }}
                  </div>
                </div>
              </div>

              <div class="grid gap-3 sm:grid-cols-2">
                <div class="rounded-xl border border-border/60 p-3">
                  <div class="text-[11px] uppercase tracking-[0.18em] text-muted-foreground">
                    关联类型
                  </div>
                  <div class="mt-1 text-sm font-medium break-all">
                    {{ walletLinkTypeLabel(currentLedger.link_type) }}
                  </div>
                </div>
                <div class="rounded-xl border border-border/60 p-3">
                  <div class="text-[11px] uppercase tracking-[0.18em] text-muted-foreground">
                    交易ID
                  </div>
                  <div class="mt-1 text-sm font-mono break-all">
                    {{ currentLedger.id }}
                  </div>
                </div>
              </div>

              <div
                v-if="currentLedger.link_type === 'payment_order'"
                class="grid gap-3 sm:grid-cols-2"
              >
                <div class="rounded-xl border border-border/60 p-3">
                  <div class="text-[11px] uppercase tracking-[0.18em] text-muted-foreground">
                    支付方式
                  </div>
                  <div class="mt-1 text-sm font-medium">
                    <span v-if="loadingLedgerOrderNo">加载中...</span>
                    <span v-else>{{ ledgerPaymentMethod ? paymentMethodLabel(ledgerPaymentMethod) : '-' }}</span>
                  </div>
                </div>
                <div class="rounded-xl border border-border/60 p-3">
                  <div class="text-[11px] uppercase tracking-[0.18em] text-muted-foreground">
                    充值订单号
                  </div>
                  <div class="mt-1 text-sm font-mono break-all">
                    <span v-if="loadingLedgerOrderNo">加载中...</span>
                    <span v-else>{{ ledgerPaymentOrderNo || '-' }}</span>
                  </div>
                </div>
              </div>

              <div class="rounded-xl border border-border/60 p-3">
                <div class="text-[11px] uppercase tracking-[0.18em] text-muted-foreground">
                  操作用户
                </div>
                <div class="mt-1 text-sm font-medium">
                  {{ currentLedger.operator_name || (currentLedger.operator_id ? '已删除用户' : '系统自动') }}
                </div>
                <div class="mt-1 text-xs text-muted-foreground">
                  ID: {{ currentLedger.operator_id || '-' }}
                </div>
                <div
                  v-if="currentLedger.operator_email"
                  class="mt-1 text-xs text-muted-foreground"
                >
                  邮箱: {{ currentLedger.operator_email }}
                </div>
              </div>

              <div class="rounded-xl border border-border/60 p-3">
                <div class="text-[11px] uppercase tracking-[0.18em] text-muted-foreground">
                  说明
                </div>
                <div class="mt-1 text-sm text-foreground whitespace-pre-wrap break-words">
                  {{ currentLedger.description || '-' }}
                </div>
              </div>
            </div>
          </div>
        </div>
      </Transition>
    </Teleport>

    <Teleport to="body">
      <Transition name="drawer">
        <div
          v-if="showRefundDrawer && currentRefund"
          class="fixed inset-0 z-[80] flex justify-end"
        >
          <div
            class="absolute inset-0 bg-black/35 backdrop-blur-sm"
            @click="closeRefundDrawer"
          />
          <div class="drawer-panel relative h-full w-full sm:w-[760px] lg:w-[860px] sm:max-w-[95vw] border-l border-border bg-background shadow-2xl overflow-y-auto">
            <div class="sticky top-0 z-10 border-b border-border bg-background/95 backdrop-blur px-4 py-3 sm:px-6 sm:py-4">
              <div class="flex items-start justify-between gap-3">
                <div class="min-w-0">
                  <h3 class="text-lg font-semibold text-foreground leading-tight">
                    退款审批
                  </h3>
                  <p class="text-xs text-muted-foreground">
                    退款单: {{ currentRefund.refund_no }}
                  </p>
                </div>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-9 w-9 shrink-0"
                  title="关闭"
                  @click="closeRefundDrawer"
                >
                  <X class="h-4 w-4" />
                </Button>
              </div>
            </div>

            <div class="p-4 sm:p-6 space-y-5">
              <div class="rounded-2xl border border-border/60 bg-muted/30 p-4">
                <div class="grid gap-3 sm:grid-cols-2">
                  <div>
                    <div class="text-[11px] uppercase tracking-[0.18em] text-muted-foreground">
                      归属
                    </div>
                    <div class="mt-1 text-sm font-medium">
                      {{ ownerDisplayName(currentRefund.owner_name, currentRefund.owner_type) }}
                    </div>
                    <div class="mt-1 text-xs text-muted-foreground">
                      {{ ownerTypeLabel(currentRefund.owner_type) }}
                    </div>
                  </div>
                  <div>
                    <div class="text-[11px] uppercase tracking-[0.18em] text-muted-foreground">
                      金额
                    </div>
                    <div class="mt-1 text-sm font-semibold tabular-nums">
                      {{ formatCurrency(currentRefund.amount_usd) }}
                    </div>
                  </div>
                  <div>
                    <div class="text-[11px] uppercase tracking-[0.18em] text-muted-foreground">
                      退款模式
                    </div>
                    <div class="mt-1 text-sm">
                      {{ refundModeLabel(currentRefund.refund_mode) }}
                    </div>
                  </div>
                  <div>
                    <div class="text-[11px] uppercase tracking-[0.18em] text-muted-foreground">
                      状态
                    </div>
                    <div class="mt-1">
                      <Badge :variant="refundStatusBadge(currentRefund.status)">
                        {{ refundStatusLabel(currentRefund.status) }}
                      </Badge>
                    </div>
                  </div>
                  <div class="sm:col-span-2">
                    <div class="text-[11px] uppercase tracking-[0.18em] text-muted-foreground">
                      申请原因
                    </div>
                    <div class="mt-1 text-sm text-foreground whitespace-pre-wrap break-words">
                      {{ currentRefund.reason || '-' }}
                    </div>
                  </div>
                  <div
                    v-if="currentRefund.failure_reason"
                    class="sm:col-span-2"
                  >
                    <div class="text-[11px] uppercase tracking-[0.18em] text-muted-foreground">
                      失败原因
                    </div>
                    <div class="mt-1 text-sm text-rose-600 whitespace-pre-wrap break-words">
                      {{ currentRefund.failure_reason }}
                    </div>
                  </div>
                </div>
              </div>

              <div
                v-if="canFailRefund(currentRefund.status)"
                class="rounded-xl border border-border/60 p-4 space-y-2"
              >
                <Label>驳回原因</Label>
                <Input
                  v-model="failRefundForm.reason"
                  placeholder="请填写驳回原因"
                />
              </div>

              <div
                v-if="canCompleteRefund(currentRefund.status)"
                class="rounded-xl border border-border/60 p-4 space-y-3"
              >
                <div class="space-y-1.5">
                  <Label>网关退款号（可选）</Label>
                  <Input v-model="completeRefundForm.gateway_refund_id" />
                </div>
                <div class="space-y-1.5">
                  <Label>打款凭证 / 参考号（可选）</Label>
                  <Input v-model="completeRefundForm.payout_reference" />
                </div>
              </div>
            </div>

            <div class="sticky bottom-0 border-t border-border bg-background/95 backdrop-blur px-4 py-3 sm:px-6 sm:py-4">
              <div class="flex flex-col-reverse gap-2 sm:flex-row sm:justify-end">
                <Button
                  variant="outline"
                  @click="closeRefundDrawer"
                >
                  关闭
                </Button>
                <Button
                  v-if="canProcessRefund(currentRefund.status)"
                  variant="outline"
                  :disabled="submittingRefundAction"
                  @click="processRefund(currentRefund)"
                >
                  {{ submittingRefundAction ? '处理中...' : '处理退款' }}
                </Button>
                <Button
                  v-if="canCompleteRefund(currentRefund.status)"
                  :disabled="submittingRefundAction"
                  @click="submitCompleteRefund"
                >
                  {{ submittingRefundAction ? '提交中...' : '确认完成' }}
                </Button>
                <Button
                  v-if="canFailRefund(currentRefund.status)"
                  variant="destructive"
                  :disabled="submittingRefundAction"
                  @click="submitFailRefund"
                >
                  {{ submittingRefundAction ? '提交中...' : '驳回退款' }}
                </Button>
              </div>
            </div>
          </div>
        </div>
      </Transition>
    </Teleport>

    <Dialog v-model="showCreditDialog">
      <template #header>
        <div class="px-6 py-4 border-b border-border">
          <h3 class="text-lg font-semibold">
            {{ currentOrder?.status === 'pending_approval' ? '充值审批' : '人工到账' }}
          </h3>
          <p class="text-xs text-muted-foreground mt-1">
            订单: {{ currentOrder?.order_no || '-' }}
          </p>
        </div>
      </template>
      <div class="space-y-4">
        <div class="space-y-1.5">
          <Label>网关订单号（可选）</Label>
          <Input v-model="creditForm.gateway_order_id" />
        </div>
        <div class="grid grid-cols-1 sm:grid-cols-3 gap-3">
          <div class="space-y-1.5">
            <Label>实付金额（可选）</Label>
            <Input
              v-model.number="creditForm.pay_amount"
              type="number"
              min="0.01"
              step="0.01"
            />
          </div>
          <div class="space-y-1.5">
            <Label>币种（可选）</Label>
            <Input v-model="creditForm.pay_currency" />
          </div>
          <div class="space-y-1.5">
            <Label>汇率（可选）</Label>
            <Input
              v-model.number="creditForm.exchange_rate"
              type="number"
              min="0.000001"
              step="0.000001"
            />
          </div>
        </div>
      </div>
      <template #footer>
        <Button
          variant="outline"
          @click="showCreditDialog = false"
        >
          取消
        </Button>
        <Button
          :disabled="submittingOrderAction"
          @click="submitCreditOrder"
        >
          {{ submittingOrderAction ? '提交中...' : currentOrder?.status === 'pending_approval' ? '审批通过并到账' : '确认到账' }}
        </Button>
      </template>
    </Dialog>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, reactive, ref, watch } from 'vue'
import { useRoute } from 'vue-router'
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
} from '@/components/ui'
import { EmptyState } from '@/components/common'
import { CheckCircle2, X, XCircle } from 'lucide-vue-next'
import {
  adminWalletApi,
  type AdminGlobalRefund,
  type AdminLedgerTransaction,
} from '@/api/admin-wallets'
import { adminPaymentsApi, type PaymentCallbackRecord } from '@/api/admin-payments'
import type { PaymentOrder } from '@/api/wallet'
import { useConfirm } from '@/composables/useConfirm'
import { parseApiError } from '@/utils/errorParser'
import { useToast } from '@/composables/useToast'
import { log } from '@/utils/logger'
import {
  callbackStatusBadge,
  callbackStatusLabel,
  formatWalletCurrency as formatCurrency,
  paymentMethodLabel,
  paymentStatusBadge,
  paymentStatusLabel,
  refundModeLabel,
  refundStatusBadge,
  refundStatusLabel,
  walletLinkTypeLabel,
  walletStatusLabel,
  walletTransactionCategoryLabel,
  walletTransactionReasonLabel,
} from '@/utils/walletDisplay'

type WalletManagementTab = 'ledger' | 'refunds' | 'orders' | 'reviews' | 'callbacks'
type LedgerCategory = 'recharge' | 'gift' | 'adjust' | 'refund'
type LedgerReasonOption = {
  value: string
  label: string
  category: LedgerCategory
}

const LEDGER_REASON_OPTIONS: LedgerReasonOption[] = [
  { value: 'topup_admin_manual', label: '人工充值', category: 'recharge' },
  { value: 'topup_gateway', label: '支付充值', category: 'recharge' },
  { value: 'topup_card_code', label: '卡密充值', category: 'recharge' },
  { value: 'gift_initial', label: '初始赠款', category: 'gift' },
  { value: 'gift_campaign', label: '活动赠款', category: 'gift' },
  { value: 'gift_expire_reclaim', label: '赠款回收', category: 'gift' },
  { value: 'adjust_admin', label: '人工调账', category: 'adjust' },
  { value: 'adjust_system', label: '系统调账', category: 'adjust' },
  { value: 'refund_out', label: '退款扣减', category: 'refund' },
  { value: 'refund_revert', label: '退款回补', category: 'refund' },
]

const { success, error: showError } = useToast()
const { confirmDanger } = useConfirm()
const route = useRoute()

const activeTab = ref<WalletManagementTab>('ledger')

const loadingLedger = ref(false)
const loadingRefunds = ref(false)
const loadingOrders = ref(false)
const loadingReviewOrders = ref(false)
const loadingCallbacks = ref(false)
const submittingRefundAction = ref(false)
const submittingOrderAction = ref(false)

const ledgerItems = ref<AdminLedgerTransaction[]>([])
const ledgerTotal = ref(0)
const ledgerPage = ref(1)
const ledgerPageSize = ref(20)
const ledgerCategoryFilter = ref('all')
const ledgerReasonFilter = ref('all')
const ledgerOwnerFilter = ref('all')
const ledgerReasonOptions = computed(() => {
  if (ledgerCategoryFilter.value === 'all') {
    return LEDGER_REASON_OPTIONS
  }
  return LEDGER_REASON_OPTIONS.filter((option) => option.category === ledgerCategoryFilter.value)
})

const refundItems = ref<AdminGlobalRefund[]>([])
const refundTotal = ref(0)
const refundPage = ref(1)
const refundPageSize = ref(20)
const refundStatusFilter = ref('all')
const refundOwnerFilter = ref('all')

const orders = ref<PaymentOrder[]>([])
const orderTotal = ref(0)
const orderPage = ref(1)
const orderPageSize = ref(20)
const orderStatusFilter = ref('all')
const orderMethodFilter = ref('all')

const reviewOrders = ref<PaymentOrder[]>([])
const reviewTotal = ref(0)
const reviewPage = ref(1)
const reviewPageSize = ref(20)

const callbacks = ref<PaymentCallbackRecord[]>([])
const callbackTotal = ref(0)
const callbackPage = ref(1)
const callbackPageSize = ref(20)
const callbackMethodFilter = ref('all')

type WalletOwnerMeta = {
  ownerName: string
  ownerType: 'user' | 'api_key'
  ownerEmail: string | null
}

const walletMetaMap = ref<Record<string, WalletOwnerMeta>>({})

const showLedgerDrawer = ref(false)
const showRefundDrawer = ref(false)
const currentLedger = ref<AdminLedgerTransaction | null>(null)
const currentRefund = ref<AdminGlobalRefund | null>(null)
const loadingLedgerOrderNo = ref(false)
const ledgerPaymentOrderNo = ref<string | null>(null)
const ledgerPaymentMethod = ref<string | null>(null)

const showCreditDialog = ref(false)
const currentOrder = ref<PaymentOrder | null>(null)

const failRefundForm = reactive({
  reason: '',
})

const completeRefundForm = reactive({
  gateway_refund_id: '',
  payout_reference: '',
})

const creditForm = reactive({
  gateway_order_id: '',
  pay_amount: undefined as number | undefined,
  pay_currency: '',
  exchange_rate: undefined as number | undefined,
})

watch([ledgerCategoryFilter, ledgerReasonFilter, ledgerOwnerFilter], () => {
  ledgerPage.value = 1
  void loadLedger()
})

watch(ledgerCategoryFilter, () => {
  if (ledgerReasonFilter.value === 'all') {
    return
  }
  const valid = ledgerReasonOptions.value.some((option) => option.value === ledgerReasonFilter.value)
  if (!valid) {
    ledgerReasonFilter.value = 'all'
  }
})

watch([refundStatusFilter, refundOwnerFilter], () => {
  refundPage.value = 1
  void loadRefunds()
})

watch([orderStatusFilter, orderMethodFilter], () => {
  orderPage.value = 1
  void loadOrders()
})

watch(callbackMethodFilter, () => {
  callbackPage.value = 1
  void loadCallbacks()
})

watch(
  () => route.query.tab,
  (tab) => {
    const tabValue = Array.isArray(tab) ? tab[0] : tab
    if (isValidTab(tabValue)) {
      activeTab.value = tabValue
    }
  },
  { immediate: true }
)

onMounted(async () => {
  await Promise.all([
    loadWalletMetaMap(),
    loadLedger(),
    loadRefunds(),
    loadOrders(),
    loadReviewOrders(),
    loadCallbacks(),
  ])
})

function isValidTab(tab: unknown): tab is WalletManagementTab {
  return tab === 'ledger' || tab === 'refunds' || tab === 'orders' || tab === 'reviews' || tab === 'callbacks'
}

async function loadWalletMetaMap() {
  try {
    const wallets = await adminWalletApi.listAllWallets()
    walletMetaMap.value = wallets.reduce<Record<string, WalletOwnerMeta>>(
      (acc, wallet) => {
        const ownerName =
          wallet.owner_name || (wallet.owner_type === 'user' ? '未命名用户' : '未命名密钥')
        acc[wallet.id] = {
          ownerName,
          ownerType: wallet.owner_type,
          ownerEmail: wallet.owner_email ?? null,
        }
        return acc
      },
      {}
    )
  } catch (error) {
    log.error('加载钱包名称映射失败:', error)
  }
}

async function loadLedger() {
  loadingLedger.value = true
  try {
    const offset = (ledgerPage.value - 1) * ledgerPageSize.value
    const resp = await adminWalletApi.listLedger({
      category: ledgerCategoryFilter.value !== 'all' ? ledgerCategoryFilter.value : undefined,
      reason_code: ledgerReasonFilter.value !== 'all' ? ledgerReasonFilter.value : undefined,
      owner_type: ledgerOwnerFilter.value !== 'all' ? ledgerOwnerFilter.value : undefined,
      limit: ledgerPageSize.value,
      offset,
    })
    ledgerItems.value = resp.items
    ledgerTotal.value = resp.total
  } catch (error) {
    log.error('加载全局资金流水失败:', error)
    showError(parseApiError(error, '加载全局资金流水失败'))
  } finally {
    loadingLedger.value = false
  }
}

async function loadRefunds() {
  loadingRefunds.value = true
  try {
    const offset = (refundPage.value - 1) * refundPageSize.value
    const resp = await adminWalletApi.listGlobalRefunds({
      status: refundStatusFilter.value !== 'all' ? refundStatusFilter.value : undefined,
      owner_type: refundOwnerFilter.value === 'user' ? 'user' : undefined,
      limit: refundPageSize.value,
      offset,
    })
    refundItems.value = resp.items
    refundTotal.value = resp.total
    if (currentRefund.value) {
      syncCurrentRefund(currentRefund.value.id)
    }
  } catch (error) {
    log.error('加载全局退款列表失败:', error)
    showError(parseApiError(error, '加载全局退款列表失败'))
  } finally {
    loadingRefunds.value = false
  }
}

async function loadOrders() {
  loadingOrders.value = true
  try {
    const offset = (orderPage.value - 1) * orderPageSize.value
    const resp = await adminPaymentsApi.listOrders({
      status: orderStatusFilter.value !== 'all' ? orderStatusFilter.value : undefined,
      payment_method: orderMethodFilter.value !== 'all' ? orderMethodFilter.value : undefined,
      limit: orderPageSize.value,
      offset,
    })
    orders.value = resp.items
    orderTotal.value = resp.total
  } catch (error) {
    log.error('加载充值订单失败:', error)
    showError(parseApiError(error, '加载充值订单失败'))
  } finally {
    loadingOrders.value = false
  }
}

async function loadReviewOrders() {
  loadingReviewOrders.value = true
  try {
    const offset = (reviewPage.value - 1) * reviewPageSize.value
    const resp = await adminPaymentsApi.listOrders({
      status: 'pending_approval',
      limit: reviewPageSize.value,
      offset,
    })
    reviewOrders.value = resp.items
    reviewTotal.value = resp.total
  } catch (error) {
    log.error('加载待审核充值订单失败:', error)
    showError(parseApiError(error, '加载待审核充值订单失败'))
  } finally {
    loadingReviewOrders.value = false
  }
}

async function loadCallbacks() {
  loadingCallbacks.value = true
  try {
    const offset = (callbackPage.value - 1) * callbackPageSize.value
    const resp = await adminPaymentsApi.listCallbacks({
      payment_method: callbackMethodFilter.value !== 'all' ? callbackMethodFilter.value : undefined,
      limit: callbackPageSize.value,
      offset,
    })
    callbacks.value = resp.items
    callbackTotal.value = resp.total
  } catch (error) {
    log.error('加载充值回调失败:', error)
    showError(parseApiError(error, '加载充值回调失败'))
  } finally {
    loadingCallbacks.value = false
  }
}

function orderOwnerName(walletId: string) {
  return walletMetaMap.value[walletId]?.ownerName || '未知归属'
}

function orderOwnerMetaLine(walletId: string) {
  const owner = walletMetaMap.value[walletId]
  if (!owner) return '未知归属'
  const ownerTypeText = ownerTypeLabel(owner.ownerType)
  if (owner.ownerType === 'user' && owner.ownerEmail) {
    return `${ownerTypeText} · ${owner.ownerEmail}`
  }
  return ownerTypeText
}

function openLedgerDrawer(tx: AdminLedgerTransaction) {
  currentLedger.value = tx
  ledgerPaymentOrderNo.value = null
  ledgerPaymentMethod.value = null
  showLedgerDrawer.value = true
  void resolveLedgerRechargeOrderNo(tx)
}

async function resolveLedgerRechargeOrderNo(tx: AdminLedgerTransaction) {
  if (tx.link_type !== 'payment_order' || !tx.link_id) {
    ledgerPaymentOrderNo.value = null
    ledgerPaymentMethod.value = null
    return
  }

  if (tx.link_id.startsWith('po_')) {
    ledgerPaymentOrderNo.value = tx.link_id
    ledgerPaymentMethod.value = null
    return
  }

  loadingLedgerOrderNo.value = true
  try {
    const resp = await adminPaymentsApi.getOrder(tx.link_id)
    ledgerPaymentOrderNo.value = resp.order.order_no || null
    ledgerPaymentMethod.value = resp.order.payment_method || null
  } catch (error) {
    log.error('加载关联充值订单失败:', error)
    ledgerPaymentOrderNo.value = null
    ledgerPaymentMethod.value = null
  } finally {
    loadingLedgerOrderNo.value = false
  }
}

function closeLedgerDrawer() {
  showLedgerDrawer.value = false
}

function openRefundDrawer(refund: AdminGlobalRefund) {
  currentRefund.value = refund
  failRefundForm.reason = ''
  completeRefundForm.gateway_refund_id = ''
  completeRefundForm.payout_reference = ''
  showRefundDrawer.value = true
}

function closeRefundDrawer() {
  showRefundDrawer.value = false
}

function syncCurrentRefund(refundId: string) {
  const latest = refundItems.value.find((item) => item.id === refundId)
  if (latest) {
    currentRefund.value = latest
  }
}

async function processRefund(refund: AdminGlobalRefund) {
  submittingRefundAction.value = true
  try {
    await adminWalletApi.processRefund(refund.wallet_id, refund.id)
    success('退款已进入 processing')
    await Promise.all([loadRefunds(), loadLedger()])
    syncCurrentRefund(refund.id)
  } catch (error) {
    log.error('处理退款失败:', error)
    showError(parseApiError(error, '处理退款失败'))
  } finally {
    submittingRefundAction.value = false
  }
}

async function submitFailRefund() {
  if (!currentRefund.value) return
  if (!failRefundForm.reason.trim()) {
    showError('请填写驳回原因')
    return
  }

  submittingRefundAction.value = true
  try {
    await adminWalletApi.failRefund(currentRefund.value.wallet_id, currentRefund.value.id, {
      reason: failRefundForm.reason.trim(),
    })
    success('退款已驳回')
    await Promise.all([loadRefunds(), loadLedger()])
    syncCurrentRefund(currentRefund.value.id)
  } catch (error) {
    log.error('驳回退款失败:', error)
    showError(parseApiError(error, '驳回退款失败'))
  } finally {
    submittingRefundAction.value = false
  }
}

async function submitCompleteRefund() {
  if (!currentRefund.value) return

  submittingRefundAction.value = true
  try {
    await adminWalletApi.completeRefund(currentRefund.value.wallet_id, currentRefund.value.id, {
      gateway_refund_id: completeRefundForm.gateway_refund_id || undefined,
      payout_reference: completeRefundForm.payout_reference || undefined,
    })
    success('退款已完成')
    await Promise.all([loadRefunds(), loadLedger()])
    syncCurrentRefund(currentRefund.value.id)
  } catch (error) {
    log.error('完成退款失败:', error)
    showError(parseApiError(error, '完成退款失败'))
  } finally {
    submittingRefundAction.value = false
  }
}

function openCreditDialog(order: PaymentOrder) {
  currentOrder.value = order
  creditForm.gateway_order_id = order.gateway_order_id || ''
  creditForm.pay_amount = order.pay_amount || undefined
  creditForm.pay_currency = order.pay_currency || ''
  creditForm.exchange_rate = order.exchange_rate || undefined
  showCreditDialog.value = true
}

async function submitCreditOrder() {
  if (!currentOrder.value) return
  submittingOrderAction.value = true
  try {
    const payload = {
      gateway_order_id: creditForm.gateway_order_id || undefined,
      pay_amount: creditForm.pay_amount,
      pay_currency: creditForm.pay_currency || undefined,
      exchange_rate: creditForm.exchange_rate,
    }
    if (currentOrder.value.status === 'pending_approval') {
      await adminPaymentsApi.approveOrder(currentOrder.value.id, payload)
      success('充值审批已通过')
    } else {
      await adminPaymentsApi.creditOrder(currentOrder.value.id, payload)
      success('充值订单已手动到账')
    }
    showCreditDialog.value = false
    await Promise.all([loadOrders(), loadReviewOrders(), loadLedger(), loadWalletMetaMap()])
  } catch (error) {
    log.error('充值订单审批失败:', error)
    showError(parseApiError(error, currentOrder.value.status === 'pending_approval' ? '通过充值审批失败' : '充值订单手动到账失败'))
  } finally {
    submittingOrderAction.value = false
  }
}

async function rejectReviewOrder(order: PaymentOrder) {
  const confirmed = await confirmDanger(
    `确认拒绝订单 ${compactOrderNo(order.order_no)} 吗？拒绝后该充值申请将不会入账。`,
    '拒绝充值审批',
    '拒绝'
  )
  if (!confirmed) return

  submittingOrderAction.value = true
  try {
    await adminPaymentsApi.rejectOrder(order.id)
    success('充值审批已拒绝')
    await Promise.all([loadOrders(), loadReviewOrders()])
  } catch (error) {
    log.error('拒绝充值审批失败:', error)
    showError(parseApiError(error, '拒绝充值审批失败'))
  } finally {
    submittingOrderAction.value = false
  }
}

function canProcessRefund(status: string) {
  return status === 'pending_approval' || status === 'approved'
}

function canFailRefund(status: string) {
  return status === 'processing' || status === 'pending_approval' || status === 'approved'
}

function canCompleteRefund(status: string) {
  return status === 'processing'
}

function handleLedgerPageChange(page: number) {
  ledgerPage.value = page
  void loadLedger()
}

function handleLedgerPageSizeChange(size: number) {
  ledgerPageSize.value = size
  ledgerPage.value = 1
  void loadLedger()
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

function handleOrderPageChange(page: number) {
  orderPage.value = page
  void loadOrders()
}

function handleOrderPageSizeChange(size: number) {
  orderPageSize.value = size
  orderPage.value = 1
  void loadOrders()
}

function handleReviewPageChange(page: number) {
  reviewPage.value = page
  void loadReviewOrders()
}

function handleReviewPageSizeChange(size: number) {
  reviewPageSize.value = size
  reviewPage.value = 1
  void loadReviewOrders()
}

function handleCallbackPageChange(page: number) {
  callbackPage.value = page
  void loadCallbacks()
}

function handleCallbackPageSizeChange(size: number) {
  callbackPageSize.value = size
  callbackPage.value = 1
  void loadCallbacks()
}

function ownerTypeLabel(ownerType: 'user' | 'api_key') {
  return ownerType === 'user' ? '用户钱包' : '独立密钥'
}

function ownerDisplayName(name: string | null | undefined, ownerType: 'user' | 'api_key') {
  if (name) return name
  return ownerType === 'user' ? '未命名用户' : '未命名密钥'
}

function ledgerOwnerMetaLine(tx: AdminLedgerTransaction) {
  const parts = [ownerTypeLabel(tx.owner_type)]
  if (tx.wallet_status) {
    parts.push(walletStatusLabel(tx.wallet_status))
  }
  return parts.join(' · ')
}

function formatDateTime(value: string | null | undefined) {
  if (!value) return '-'
  return new Date(value).toLocaleString('zh-CN', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  })
}

function formatDateLabel(value: string | null | undefined) {
  if (!value) return '-'
  return new Date(value).toLocaleDateString('zh-CN', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
  })
}

function formatTimeLabel(value: string | null | undefined) {
  if (!value) return '--:--'
  return new Date(value).toLocaleTimeString('zh-CN', {
    hour: '2-digit',
    minute: '2-digit',
    hour12: false,
  })
}

function compactOrderNo(orderNo: string | null | undefined): string {
  if (!orderNo) return '-'
  if (orderNo.length <= 26) return orderNo
  return `${orderNo.slice(0, 18)}...${orderNo.slice(-8)}`
}
</script>

<style scoped>
.drawer-enter-active,
.drawer-leave-active {
  transition: opacity 0.3s ease;
}

.drawer-enter-active .drawer-panel,
.drawer-leave-active .drawer-panel {
  transition: transform 0.3s ease;
}

.drawer-enter-from,
.drawer-leave-to {
  opacity: 0;
}

.drawer-enter-from .drawer-panel,
.drawer-leave-to .drawer-panel {
  transform: translateX(100%);
}
</style>
