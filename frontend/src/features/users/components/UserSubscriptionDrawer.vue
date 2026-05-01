<template>
  <Teleport to="body">
    <Transition name="drawer">
      <div
        v-if="open && user"
        class="fixed inset-0 z-[80] flex justify-end"
      >
        <div
          class="absolute inset-0 bg-black/35 backdrop-blur-sm"
          @click="handleClose"
        />

        <div class="drawer-panel relative h-full w-full border-l border-border bg-background shadow-2xl overflow-y-auto sm:max-w-[95vw] sm:w-[760px] lg:w-[880px]">
          <div class="sticky top-0 z-10 border-b border-border bg-background/95 px-4 py-3 backdrop-blur sm:px-6 sm:py-4">
            <div class="flex items-start justify-between gap-3">
              <div class="flex min-w-0 items-center gap-3">
                <div class="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl bg-primary/10 text-primary">
                  <CreditCard class="h-5 w-5" />
                </div>
                <div class="min-w-0">
                  <div class="flex items-center gap-2">
                    <h3 class="text-lg font-semibold text-foreground leading-tight">
                      管理订阅
                    </h3>
                    <Badge
                      v-if="currentManagedSubscription"
                      :variant="subscriptionStatusBadge(currentManagedSubscription.status)"
                      class="w-fit px-2 py-0.5 text-[11px] leading-none"
                    >
                      {{ formatSubscriptionStatus(currentManagedSubscription.status) }}
                    </Badge>
                  </div>
                  <p class="text-xs text-muted-foreground">
                    {{ user.username }}<span v-if="user.email"> · {{ user.email }}</span>
                  </p>
                </div>
              </div>

              <div class="flex shrink-0 items-center gap-2">
                <RefreshButton
                  :loading="loadingDrawerData"
                  @click="refreshDrawerData"
                />
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-9 w-9"
                  title="关闭"
                  @click="handleClose"
                >
                  <X class="h-4 w-4" />
                </Button>
              </div>
            </div>
          </div>

          <div class="space-y-5 p-4 sm:p-6">
            <div
              v-if="loadingDrawerData"
              class="rounded-2xl border border-dashed border-border/60 bg-muted/20 px-4 py-12 text-center text-sm text-muted-foreground"
            >
              正在加载订阅信息...
            </div>

            <template v-else>
              <div
                v-if="currentManagedSubscription"
                class="rounded-2xl border border-border/60 bg-muted/30 p-3.5"
              >
                <div class="space-y-3">
                  <div class="flex flex-col gap-3 lg:flex-row lg:items-start lg:justify-between">
                    <div class="min-w-0 space-y-1.5">
                      <div class="flex flex-wrap items-center gap-2">
                        <h4 class="text-base font-semibold text-foreground">
                          {{ formatManagedSubscriptionName(currentManagedSubscription) }}
                        </h4>
                        <Badge
                          :variant="subscriptionStatusBadge(currentManagedSubscription.status)"
                          class="h-6 px-2 text-[11px]"
                        >
                          {{ formatSubscriptionStatus(currentManagedSubscription.status) }}
                        </Badge>
                        <Badge
                          v-if="currentManagedSubscription.cancel_at_period_end"
                          variant="outline"
                          class="h-6 px-2 text-[11px]"
                        >
                          到期取消
                        </Badge>
                      </div>
                      <div class="flex flex-wrap gap-x-3 gap-y-1 text-xs text-muted-foreground">
                        <span>{{ currentManagedSubscription.user_group_name || '未绑定分组' }}</span>
                        <span>已购 {{ currentManagedSubscription.purchased_months || 0 }} 个月</span>
                        <span>开始 {{ formatDateTime(currentManagedSubscription.started_at) }}</span>
                        <span>到期 {{ formatDateTime(currentManagedSubscription.ends_at) }}</span>
                      </div>
                    </div>

                    <Button
                      variant="outline"
                      class="h-9 self-start border-rose-200 px-3 text-rose-600 hover:bg-rose-50 dark:border-rose-900/60 dark:hover:bg-rose-950/40"
                      :disabled="savingSubscriptionAction"
                      @click="openCancelConfirmDialog"
                    >
                      {{ savingSubscriptionAction ? '处理中...' : '取消订阅' }}
                    </Button>
                  </div>

                  <div class="rounded-xl bg-background/85 px-3 py-3">
                    <div class="flex flex-col gap-2 lg:flex-row lg:items-start lg:justify-between">
                      <div class="min-w-0">
                        <div class="text-sm font-medium text-foreground">
                          周期额度进度
                        </div>
                        <div class="mt-1 flex flex-wrap gap-x-3 gap-y-1 text-xs text-muted-foreground">
                          <span>已使用 {{ formatCurrencyValue(currentManagedSubscription.cycle_used_usd) }}</span>
                          <span>剩余 {{ formatCurrencyValue(currentManagedSubscription.remaining_quota_usd) }}</span>
                          <span>周期 {{ formatDateTime(currentManagedSubscription.current_cycle_start) }} - {{ formatDateTime(currentManagedSubscription.current_cycle_end) }}</span>
                        </div>
                      </div>
                      <div class="shrink-0 text-left lg:text-right">
                        <div class="text-[11px] text-muted-foreground">
                          使用率
                        </div>
                        <div class="text-sm font-semibold text-foreground">
                          {{ subscriptionUsagePercent }}%
                        </div>
                      </div>
                    </div>

                    <div class="mt-3 h-2 overflow-hidden rounded-full bg-muted">
                      <div
                        class="h-full rounded-full bg-gradient-to-r from-primary/80 via-primary to-amber-400 transition-all"
                        :style="{ width: `${subscriptionUsagePercent}%` }"
                      />
                    </div>

                    <div class="mt-2 flex items-center justify-between text-[11px] text-muted-foreground">
                      <span>{{ formatCurrencyValue(currentManagedSubscription.cycle_used_usd) }}</span>
                      <span>{{ formatCurrencyValue(currentManagedSubscription.cycle_quota_usd) }}</span>
                    </div>

                    <div class="mt-3 grid gap-2 sm:grid-cols-2 xl:grid-cols-4">
                      <div class="rounded-lg bg-muted/25 px-3 py-2.5">
                        <div class="text-[11px] uppercase tracking-[0.16em] text-muted-foreground">
                          支付金额
                        </div>
                        <div class="mt-1 text-sm font-semibold text-foreground">
                          {{ formatCurrencyValue(currentManagedSubscription.total_price_usd) }}
                        </div>
                        <div class="mt-0.5 text-[11px] text-muted-foreground">
                          月单价 {{ formatCurrencyValue(currentManagedSubscription.monthly_price_usd_snapshot) }}
                        </div>
                      </div>

                      <div class="rounded-lg bg-muted/25 px-3 py-2.5">
                        <div class="text-[11px] uppercase tracking-[0.16em] text-muted-foreground">
                          支付方式
                        </div>
                        <div class="mt-1 text-sm font-medium text-foreground">
                          {{ paymentMethodLabel(currentSubscriptionOrder?.payment_method) }}
                        </div>
                        <div class="mt-0.5 truncate text-[11px] text-muted-foreground">
                          {{ currentSubscriptionOrder?.order_no || '无关联订单' }}
                        </div>
                      </div>

                      <div class="rounded-lg bg-muted/25 px-3 py-2.5">
                        <div class="text-[11px] uppercase tracking-[0.16em] text-muted-foreground">
                          订单状态
                        </div>
                        <div class="mt-1 text-sm font-medium text-foreground">
                          {{ paymentStatusLabel(currentSubscriptionOrder?.status) }}
                        </div>
                        <div class="mt-0.5 text-[11px] text-muted-foreground">
                          下单 {{ formatDateTime(currentSubscriptionOrder?.created_at) }}
                        </div>
                      </div>

                      <div class="rounded-lg bg-muted/25 px-3 py-2.5">
                        <div class="text-[11px] uppercase tracking-[0.16em] text-muted-foreground">
                          超额策略
                        </div>
                        <div class="mt-1 text-sm font-medium text-foreground">
                          {{ formatOveragePolicy(currentManagedSubscriptionOveragePolicy) }}
                        </div>
                        <div class="mt-0.5 text-[11px] text-muted-foreground">
                          额度 {{ formatCurrencyValue(currentManagedSubscription.cycle_quota_usd) }}
                        </div>
                      </div>
                    </div>
                  </div>
                </div>
              </div>

              <div
                v-else
                class="rounded-2xl border border-dashed border-border/60 bg-muted/15 px-4 py-6 text-center"
              >
                <div class="text-sm font-medium text-foreground">
                  当前用户还没有订阅
                </div>
                <div class="mt-1 text-xs text-muted-foreground">
                  可以在下方直接开通新的订阅版本。
                </div>
              </div>

              <Tabs v-model="activeTab">
                <TabsList :class="tabsListClass">
                  <TabsTrigger value="adjustment">
                    订阅调整
                  </TabsTrigger>
                  <TabsTrigger value="orders">
                    订阅订单
                  </TabsTrigger>
                  <TabsTrigger value="reviews">
                    订阅审批
                  </TabsTrigger>
                </TabsList>

                <TabsContent
                  value="adjustment"
                  class="mt-4 space-y-4"
                >
                  <div
                    class="rounded-xl border border-dashed px-4 py-3 text-sm"
                    :class="adjustmentHintClass"
                  >
                    {{ adjustmentHintText }}
                  </div>

                  <div class="rounded-2xl border border-border/60 bg-background p-4 space-y-4">
                    <div class="grid gap-4 sm:grid-cols-2">
                      <div class="space-y-2">
                        <Label>订阅套餐</Label>
                        <Select v-model="subscriptionProductId">
                          <SelectTrigger class="h-11">
                            <SelectValue :placeholder="availableSubscriptionProducts.length > 0 ? '选择订阅套餐' : '暂无可用套餐'" />
                          </SelectTrigger>
                          <SelectContent>
                            <SelectItem
                              v-for="product in availableSubscriptionProducts"
                              :key="product.id"
                              :value="product.id"
                            >
                              {{ subscriptionProductLabel(product) }}
                            </SelectItem>
                          </SelectContent>
                        </Select>
                      </div>

                      <div class="space-y-2">
                        <Label>套餐版本</Label>
                        <Select v-model="subscriptionPlanId">
                          <SelectTrigger class="h-11">
                            <SelectValue :placeholder="availableProductVariants.length > 0 ? '选择版本' : '暂无可用版本'" />
                          </SelectTrigger>
                          <SelectContent>
                            <SelectItem
                              v-for="variant in availableProductVariants"
                              :key="variant.id"
                              :value="variant.id"
                            >
                              {{ variant.name }}
                            </SelectItem>
                          </SelectContent>
                        </Select>
                        <p
                          v-if="currentManagedSubscription && availableSubscriptionVariants.length === 0"
                          class="text-xs text-muted-foreground"
                        >
                          当前暂无更高版本可升级。
                        </p>
                      </div>
                    </div>

                    <div
                      v-if="selectedVariant"
                      class="rounded-2xl border border-border/60 bg-muted/15 p-4 space-y-4"
                    >
                      <div class="flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between">
                        <div class="space-y-1">
                          <div class="text-sm font-semibold text-foreground">
                            {{ selectedVariantProduct?.name || '未命名套餐' }} · {{ selectedVariant.name }}
                          </div>
                          <div class="text-xs text-muted-foreground">
                            {{
                              selectedActionKind === 'renewal'
                                ? '管理员可直接续订当前版本，新的订阅周期会顺延生效。'
                                : selectedActionKind === 'upgrade'
                                  ? '根据购买周期自动应用折扣，提交后会按所选版本立即升级。'
                                  : '根据购买周期自动应用折扣，提交后会按所选版本直接开通。'
                            }}
                          </div>
                        </div>
                        <Badge
                          variant="outline"
                          class="h-7 w-fit whitespace-nowrap px-3 py-0"
                        >
                          {{ formatOveragePolicy(selectedVariantProduct?.overage_policy) }}
                        </Badge>
                      </div>

                      <div class="grid gap-3 xl:grid-cols-[minmax(0,13.5rem)_minmax(0,1fr)]">
                        <div class="rounded-2xl border border-border/60 bg-background/90 px-4 py-3.5">
                          <div class="text-[11px] text-muted-foreground">
                            当前价格
                          </div>
                          <div class="mt-1 text-3xl font-semibold tabular-nums text-foreground sm:text-[2.35rem]">
                            {{ formatCurrencyValue(selectedDiscountedPrice) }}
                          </div>
                          <div class="mt-1 text-[11px] text-muted-foreground">
                            {{ `${subscriptionMonths} 个月合计` }}
                          </div>
                          <div
                            v-if="selectedDiscountedPrice !== selectedOriginalPrice"
                            class="mt-1 text-[11px] text-muted-foreground line-through"
                          >
                            原价 {{ formatCurrencyValue(selectedOriginalPrice) }}
                          </div>

                          <div class="mt-4 flex items-center gap-2.5">
                            <Input
                              :model-value="String(subscriptionMonths)"
                              type="number"
                              min="1"
                              inputmode="numeric"
                              class="h-11 w-28 bg-background text-lg font-medium"
                              @update:model-value="(value) => subscriptionMonths = parseNumberInput(value, { min: 1 }) || 1"
                            />
                            <span class="text-sm text-muted-foreground">个月</span>
                          </div>
                        </div>

                        <div class="rounded-2xl border border-border/60 bg-background px-4 py-3.5">
                          <div class="grid gap-x-4 gap-y-2 text-sm text-muted-foreground">
                            <div class="flex items-start justify-between gap-4">
                              <span class="shrink-0 whitespace-nowrap">月额度</span>
                              <span class="min-w-0 whitespace-nowrap text-right font-medium tabular-nums text-foreground">
                                {{ formatCurrencyValue(selectedVariant.monthly_quota_usd) }}
                              </span>
                            </div>
                            <div class="flex items-start justify-between gap-4">
                              <span class="shrink-0 whitespace-nowrap">超额策略</span>
                              <span class="min-w-0 whitespace-nowrap text-right font-medium text-foreground">
                                {{ formatOveragePolicy(selectedVariantProduct?.overage_policy) }}
                              </span>
                            </div>
                            <div class="flex items-start justify-between gap-4">
                              <span class="shrink-0 whitespace-nowrap">基础月费</span>
                              <span class="min-w-0 whitespace-nowrap text-right font-medium tabular-nums text-foreground">
                                {{ formatCurrencyValue(selectedVariant.monthly_price_usd) }}
                              </span>
                            </div>
                            <div class="flex items-start justify-between gap-4">
                              <span class="shrink-0 whitespace-nowrap">当前折扣</span>
                              <span class="min-w-0 whitespace-nowrap text-right font-medium text-foreground">
                                {{ selectedDiscountLabel }}
                              </span>
                            </div>
                          </div>

                          <div class="mt-3 flex flex-wrap gap-1.5">
                            <Badge
                              v-for="option in availableDurationOptions"
                              :key="`${selectedVariant.id}-rule-${option.months}`"
                              variant="outline"
                              class="h-6 rounded-md px-2 text-[11px]"
                            >
                              满 {{ option.months }} 个月 {{ formatDurationDiscount(option.discount_factor) }}
                            </Badge>
                          </div>
                          <div class="mt-3 rounded-xl border border-border/50 bg-muted/[0.08] px-3 py-2 text-[11px] leading-5 text-muted-foreground">
                            {{ selectedDiscountHint }}
                          </div>
                        </div>
                      </div>
                    </div>

                    <p
                      v-if="!selectedVariant && availableSubscriptionVariants.length === 0"
                      class="rounded-xl bg-muted/20 px-3 py-2 text-xs text-muted-foreground"
                    >
                      当前没有可供开通或升级的订阅版本。
                    </p>

                    <div class="flex flex-col-reverse gap-2 sm:flex-row sm:justify-end">
                      <Button
                        variant="outline"
                        class="h-10 px-5"
                        @click="handleClose"
                      >
                        关闭
                      </Button>
                      <Button
                        class="h-10 px-5"
                        :disabled="submitSubscriptionDisabled"
                        @click="submitSubscriptionAction"
                      >
                        {{ submitSubscriptionLabel }}
                      </Button>
                    </div>
                  </div>
                </TabsContent>

                <TabsContent
                  value="orders"
                  class="mt-4 space-y-3"
                >
                  <div class="flex items-center justify-between gap-3">
                    <div class="text-sm text-muted-foreground">
                      共 {{ subscriptionOrders.length }} 条
                    </div>
                    <RefreshButton
                      :loading="loadingDrawerData"
                      @click="refreshDrawerData"
                    />
                  </div>

                  <div
                    v-if="subscriptionOrders.length === 0"
                    class="rounded-2xl border border-dashed border-border/60 bg-muted/15 px-4 py-10 text-center"
                  >
                    <div class="text-sm font-medium text-foreground">
                      暂无订阅订单
                    </div>
                    <div class="mt-1 text-xs text-muted-foreground">
                      当前用户还没有产生订阅购买或升级订单。
                    </div>
                  </div>

                  <div
                    v-else
                    class="space-y-3"
                  >
                    <div
                      v-for="order in subscriptionOrders"
                      :key="order.id"
                      class="rounded-2xl border border-border/60 bg-background p-4"
                    >
                      <div class="flex flex-col gap-3">
                        <div class="flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between">
                          <div class="min-w-0">
                            <div class="flex flex-wrap items-center gap-2">
                              <div class="truncate text-sm font-semibold text-foreground">
                                {{ order.order_no }}
                              </div>
                              <Badge
                                :variant="paymentStatusBadge(order.status)"
                                class="h-6 px-2 text-[11px]"
                              >
                                {{ paymentStatusLabel(order.status) }}
                              </Badge>
                            </div>
                            <div class="mt-1 text-xs text-muted-foreground">
                              {{ subscriptionOrderTypeLabel(order.order_type) }} · {{ subscriptionOrderPlanLabel(order) }}
                            </div>
                          </div>

                          <div class="text-left sm:text-right">
                            <div class="text-sm font-semibold text-foreground">
                              {{ formatCurrencyValue(order.amount_usd) }}
                            </div>
                            <div class="mt-1 text-xs text-muted-foreground">
                              {{ paymentMethodLabel(order.payment_method) }}
                            </div>
                          </div>
                        </div>

                        <div class="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
                          <div class="rounded-xl bg-muted/20 p-3">
                            <div class="text-[11px] uppercase tracking-[0.18em] text-muted-foreground">
                              购买月数
                            </div>
                            <div class="mt-1 text-sm font-medium text-foreground">
                              {{ order.purchased_months || 0 }} 个月
                            </div>
                          </div>
                          <div class="rounded-xl bg-muted/20 p-3">
                            <div class="text-[11px] uppercase tracking-[0.18em] text-muted-foreground">
                              创建时间
                            </div>
                            <div class="mt-1 text-sm font-medium text-foreground">
                              {{ formatDateTime(order.created_at) }}
                            </div>
                          </div>
                          <div class="rounded-xl bg-muted/20 p-3">
                            <div class="text-[11px] uppercase tracking-[0.18em] text-muted-foreground">
                              支付时间
                            </div>
                            <div class="mt-1 text-sm font-medium text-foreground">
                              {{ formatDateTime(order.paid_at) }}
                            </div>
                          </div>
                          <div class="rounded-xl bg-muted/20 p-3">
                            <div class="text-[11px] uppercase tracking-[0.18em] text-muted-foreground">
                              到账 / 过期
                            </div>
                            <div class="mt-1 text-sm font-medium text-foreground">
                              {{ formatDateTime(order.credited_at || order.expires_at) }}
                            </div>
                          </div>
                        </div>
                      </div>
                    </div>
                  </div>
                </TabsContent>

                <TabsContent
                  value="reviews"
                  class="mt-4 space-y-3"
                >
                  <div class="flex items-center justify-between gap-3">
                    <div class="text-sm text-muted-foreground">
                      待审核 {{ reviewOrders.length }} 条
                    </div>
                    <RefreshButton
                      :loading="loadingDrawerData"
                      @click="refreshDrawerData"
                    />
                  </div>

                  <div
                    v-if="reviewOrders.length === 0"
                    class="rounded-2xl border border-dashed border-border/60 bg-muted/15 px-4 py-10 text-center"
                  >
                    <div class="text-sm font-medium text-foreground">
                      暂无待审核订阅订单
                    </div>
                    <div class="mt-1 text-xs text-muted-foreground">
                      当前用户没有需要人工审批的订阅申请。
                    </div>
                  </div>

                  <div
                    v-else
                    class="space-y-3"
                  >
                    <div
                      v-for="order in reviewOrders"
                      :key="order.id"
                      class="rounded-2xl border border-border/60 bg-background p-4"
                    >
                      <div class="flex flex-col gap-3">
                        <div class="flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between">
                          <div class="min-w-0">
                            <div class="flex flex-wrap items-center gap-2">
                              <div class="truncate text-sm font-semibold text-foreground">
                                {{ order.order_no }}
                              </div>
                              <Badge
                                :variant="paymentStatusBadge(order.status)"
                                class="h-6 px-2 text-[11px]"
                              >
                                {{ paymentStatusLabel(order.status) }}
                              </Badge>
                            </div>
                            <div class="mt-1 text-xs text-muted-foreground">
                              {{ subscriptionOrderPlanLabel(order) }} · {{ paymentMethodLabel(order.payment_method) }}
                            </div>
                          </div>

                          <div class="text-left sm:text-right">
                            <div class="text-sm font-semibold text-foreground">
                              {{ formatCurrencyValue(order.amount_usd) }}
                            </div>
                            <div class="mt-1 text-xs text-muted-foreground">
                              创建于 {{ formatDateTime(order.created_at) }}
                            </div>
                          </div>
                        </div>

                        <div class="flex flex-col-reverse gap-2 sm:flex-row sm:justify-end">
                          <Button
                            variant="outline"
                            class="h-9 px-4"
                            :disabled="reviewingOrderId === order.id"
                            @click="approveOrder(order)"
                          >
                            <CheckCircle2 class="mr-1.5 h-4 w-4" />
                            {{ reviewingOrderId === order.id ? '处理中...' : '通过' }}
                          </Button>
                          <Button
                            variant="outline"
                            class="h-9 px-4 border-rose-200 text-rose-600 hover:bg-rose-50 dark:border-rose-900/60 dark:hover:bg-rose-950/40"
                            :disabled="reviewingOrderId === order.id"
                            @click="rejectOrder(order)"
                          >
                            <XCircle class="mr-1.5 h-4 w-4" />
                            {{ reviewingOrderId === order.id ? '处理中...' : '拒绝' }}
                          </Button>
                        </div>
                      </div>
                    </div>
                  </div>
                </TabsContent>
              </Tabs>
            </template>
          </div>
        </div>
      </div>
    </Transition>

    <Dialog
      :model-value="showCancelConfirmDialog"
      size="md"
      z-index="120"
      @update:model-value="handleCancelDialogUpdate"
    >
      <template #header>
        <div class="border-b border-border px-6 py-4">
          <div class="flex items-center gap-3">
            <div class="flex h-9 w-9 items-center justify-center rounded-lg bg-rose-500/10 text-rose-600">
              <AlertTriangle class="h-5 w-5" />
            </div>
            <div>
              <h3 class="text-lg font-semibold text-foreground">
                取消订阅
              </h3>
              <p class="text-xs text-muted-foreground">
                选择取消方式后确认执行。
              </p>
            </div>
          </div>
        </div>
      </template>

      <div class="space-y-3 py-2">
        <button
          type="button"
          class="w-full rounded-2xl border p-4 text-left transition-colors"
          :class="cancelImmediateChoice ? 'border-rose-300 bg-rose-50/70 dark:border-rose-800 dark:bg-rose-950/20' : 'border-border/60 hover:border-foreground/20'"
          @click="cancelImmediateChoice = true"
        >
          <div class="text-sm font-semibold text-foreground">
            立即取消
          </div>
          <div class="mt-1 text-xs text-muted-foreground">
            立即终止当前订阅，当前周期额度也会一并结束。
          </div>
        </button>

        <button
          type="button"
          class="w-full rounded-2xl border p-4 text-left transition-colors"
          :class="!cancelImmediateChoice ? 'border-primary/40 bg-primary/5' : 'border-border/60 hover:border-foreground/20'"
          @click="cancelImmediateChoice = false"
        >
          <div class="text-sm font-semibold text-foreground">
            到期取消
          </div>
          <div class="mt-1 text-xs text-muted-foreground">
            保留当前订阅到本周期结束，到期后不再续订。
          </div>
        </button>
      </div>

      <template #footer>
        <Button
          variant="outline"
          class="h-10 px-5"
          @click="handleCancelDialogUpdate(false)"
        >
          返回
        </Button>
        <Button
          class="h-10 px-5"
          :disabled="savingSubscriptionAction"
          @click="submitCancelSubscription"
        >
          {{ savingSubscriptionAction ? '处理中...' : '确认取消' }}
        </Button>
      </template>
    </Dialog>
  </Teleport>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import type { User } from '@/api/users'
import {
  adminSubscriptionsApi,
  type SubscriptionOrder,
  type SubscriptionProduct,
  type SubscriptionVariant,
  type UserSubscription,
} from '@/api/admin-subscriptions'
import { useToast } from '@/composables/useToast'
import { useConfirm } from '@/composables/useConfirm'
import { parseApiError } from '@/utils/errorParser'
import { parseNumberInput } from '@/utils/form'
import { paymentMethodLabel, paymentStatusBadge, paymentStatusLabel } from '@/utils/walletDisplay'
import {
  Badge,
  Button,
  Dialog,
  Input,
  Label,
  RefreshButton,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from '@/components/ui'
import {
  AlertTriangle,
  CheckCircle2,
  CreditCard,
  X,
  XCircle,
} from 'lucide-vue-next'

interface DurationOption {
  months: number
  discount_factor: number
}

type SubscriptionActionKind = 'purchase' | 'renewal' | 'upgrade' | 'unavailable'

const props = defineProps<{
  open: boolean
  user: User | null
}>()

const emit = defineEmits<{
  close: []
  changed: []
}>()

const { success, error } = useToast()
const { confirmDanger, confirmWarning } = useConfirm()

const activeTab = ref<'adjustment' | 'orders' | 'reviews'>('adjustment')
const loadingDrawerData = ref(false)
const savingSubscriptionAction = ref(false)
const reviewingOrderId = ref<string | null>(null)

const currentManagedSubscription = ref<UserSubscription | null>(null)
const subscriptionProducts = ref<SubscriptionProduct[]>([])
const subscriptionOrders = ref<SubscriptionOrder[]>([])
const subscriptionProductId = ref('')
const subscriptionPlanId = ref('')
const subscriptionMonths = ref(1)

const showCancelConfirmDialog = ref(false)
const cancelImmediateChoice = ref(false)

const tabsListClass = computed(() => ['tabs-button-list', 'grid', 'w-full', 'grid-cols-3'])

const subscriptionProductMap = computed(() => {
  const map = new Map<string, SubscriptionProduct>()
  for (const product of subscriptionProducts.value) {
    map.set(product.id, product)
  }
  return map
})

const subscriptionVariantMap = computed(() => {
  const map = new Map<string, SubscriptionVariant>()
  for (const product of subscriptionProducts.value) {
    for (const variant of product.variants) {
      map.set(variant.id, variant)
    }
  }
  return map
})

const hasActiveManagedSubscription = computed(() => currentManagedSubscription.value?.status === 'active')

const currentManagedSubscriptionOveragePolicy = computed<'block' | 'use_wallet_balance'>(() => {
  const subscription = currentManagedSubscription.value
  if (!subscription?.product_id) return 'block'
  const product = subscriptionProductMap.value.get(subscription.product_id)
  return product?.overage_policy === 'use_wallet_balance' ? 'use_wallet_balance' : 'block'
})

const currentSubscriptionOrder = computed(() => {
  const subscription = currentManagedSubscription.value
  if (!subscription) {
    return subscriptionOrders.value[0] || null
  }
  return subscriptionOrders.value.find(order => order.subscription_id === subscription.id)
    || subscriptionOrders.value.find(order => order.plan_id === subscription.plan_id)
    || subscriptionOrders.value[0]
    || null
})

const reviewOrders = computed(() => {
  return subscriptionOrders.value.filter(order =>
    ['manual', 'manual_review'].includes(String(order.payment_method || ''))
    && order.status === 'pending_approval'
  )
})

const availableSubscriptionVariants = computed(() => {
  const products = subscriptionProducts.value.filter(product => product.is_active)
  const variants = products.flatMap(product => product.variants.filter(variant => variant.is_active))
  if (!currentManagedSubscription.value || !hasActiveManagedSubscription.value) {
    return currentManagedSubscription.value ? [] : variants
  }

  return variants.filter(variant => transitionKindForVariant(currentManagedSubscription.value!, variant) !== 'unavailable')
})

const availableSubscriptionProducts = computed(() => {
  const map = new Map<string, SubscriptionProduct>()
  for (const variant of sortVariants(availableSubscriptionVariants.value)) {
    const product = subscriptionProductMap.value.get(variant.product_id)
    if (product && !map.has(product.id)) {
      map.set(product.id, product)
    }
  }
  return [...map.values()].sort((a, b) => {
    if (a.plan_level !== b.plan_level) return a.plan_level - b.plan_level
    return a.name.localeCompare(b.name, 'zh-CN')
  })
})

const availableProductVariants = computed(() => {
  return sortVariants(
    availableSubscriptionVariants.value.filter(
      variant => !subscriptionProductId.value || variant.product_id === subscriptionProductId.value
    )
  )
})

const selectedVariant = computed(() => {
  if (!subscriptionPlanId.value) return null
  return subscriptionVariantMap.value.get(subscriptionPlanId.value) || null
})

const selectedVariantProduct = computed(() => {
  if (!selectedVariant.value) return null
  return subscriptionProductMap.value.get(selectedVariant.value.product_id) || null
})

const selectedActionKind = computed<SubscriptionActionKind>(() => {
  if (!selectedVariant.value) return 'unavailable'
  if (!currentManagedSubscription.value || !hasActiveManagedSubscription.value) return 'purchase'
  return transitionKindForVariant(currentManagedSubscription.value, selectedVariant.value)
})

const availableDurationOptions = computed<DurationOption[]>(() => {
  const source = selectedVariant.value?.term_discounts_json
  if (source && source.length > 0) {
    return sortDiscounts(source)
  }
  return [{ months: 1, discount_factor: 1 }]
})

const selectedDurationOption = computed(() => {
  const options = availableDurationOptions.value
  if (options.length === 0) return null
  let matched = options[0]
  for (const option of options) {
    if (option.months > subscriptionMonths.value) break
    matched = option
  }
  return matched
})

const selectedOriginalPrice = computed(() => {
  if (!selectedVariant.value) return 0
  return Number((selectedVariant.value.monthly_price_usd * subscriptionMonths.value).toFixed(2))
})

const selectedDiscountedPrice = computed(() => {
  const factor = selectedDurationOption.value?.discount_factor ?? 1
  return Number((selectedOriginalPrice.value * factor).toFixed(2))
})

const selectedDiscountLabel = computed(() => {
  const factor = selectedDurationOption.value?.discount_factor ?? 1
  if (factor === 1) return '原价'
  if (factor < 1) return formatDurationDiscount(factor)
  return `系数 ${factor.toFixed(2)}`
})

const selectedDiscountHint = computed(() => {
  const factor = selectedDurationOption.value?.discount_factor ?? 1
  const ruleMonths = selectedDurationOption.value?.months ?? 1
  if (factor === 1) return `按满 ${ruleMonths} 个月档标准价格计价`
  if (factor < 1) return `按满 ${ruleMonths} 个月档计价，节省 ${formatCurrencyValue(selectedOriginalPrice.value - selectedDiscountedPrice.value)}`
  return `按 ${factor.toFixed(2)} 倍价格计费`
})

const submitSubscriptionLabel = computed(() => {
  if (savingSubscriptionAction.value) return '处理中...'
  if (selectedActionKind.value === 'renewal') return '续订订阅'
  if (selectedActionKind.value === 'upgrade') return '升级订阅'
  return '开通订阅'
})

const submitSubscriptionDisabled = computed(() => {
  if (savingSubscriptionAction.value) return true
  if (!subscriptionPlanId.value) return true
  if (currentManagedSubscription.value && !hasActiveManagedSubscription.value) return true
  return false
})

const subscriptionUsagePercent = computed(() => {
  const subscription = currentManagedSubscription.value
  if (!subscription) return 0
  const quota = Number(subscription.cycle_quota_usd || 0)
  if (quota <= 0) return 0
  const used = Number(subscription.cycle_used_usd || 0)
  const percent = Math.round((used / quota) * 100)
  return Math.max(0, Math.min(percent, 100))
})

const adjustmentHintText = computed(() => {
  if (hasActiveManagedSubscription.value) {
    return '当前用户已有生效中的订阅，可在这里续订当前版本，或切换到更高版本完成升级；取消订阅时会在下一步选择立即终止或到期取消。'
  }
  if (currentManagedSubscription.value) {
    return '当前存在未生效或已结束的订阅记录。待订单完成或状态变更后，才能继续进行升级操作。'
  }
  return '当前用户还没有生效中的订阅，可以在这里选择套餐和版本直接开通。'
})

const adjustmentHintClass = computed(() => {
  if (hasActiveManagedSubscription.value) {
    return 'border-primary/20 bg-primary/5 text-muted-foreground'
  }
  if (currentManagedSubscription.value) {
    return 'border-amber-300/50 bg-amber-50/70 text-amber-900 dark:border-amber-800/60 dark:bg-amber-950/20 dark:text-amber-100'
  }
  return 'border-border/60 bg-muted/20 text-muted-foreground'
})

watch(
  () => [props.open, props.user?.id] as const,
  async ([open]) => {
    if (!open || !props.user) return
    activeTab.value = 'adjustment'
    showCancelConfirmDialog.value = false
    cancelImmediateChoice.value = false
    subscriptionProductId.value = ''
    subscriptionPlanId.value = ''
    subscriptionMonths.value = 1
    await refreshDrawerData()
  },
)

watch(
  availableSubscriptionProducts,
  (products) => {
    if (!products.some(product => product.id === subscriptionProductId.value)) {
      subscriptionProductId.value = products[0]?.id || ''
    }
  },
  { immediate: true },
)

watch(
  availableProductVariants,
  (variants) => {
    if (!variants.some(variant => variant.id === subscriptionPlanId.value)) {
      subscriptionPlanId.value = variants[0]?.id || ''
    }
  },
  { immediate: true },
)

watch(
  availableDurationOptions,
  (options) => {
    if (!Number.isFinite(subscriptionMonths.value) || subscriptionMonths.value <= 0) {
      subscriptionMonths.value = options[0]?.months || 1
    }
  },
  { immediate: true },
)

function handleClose() {
  showCancelConfirmDialog.value = false
  emit('close')
}

async function refreshDrawerData() {
  if (!props.user) return

  loadingDrawerData.value = true
  try {
    const [productsResponse, subscription, ordersResponse] = await Promise.all([
      adminSubscriptionsApi.listProducts(),
      adminSubscriptionsApi.getCurrentUserSubscription(props.user.id),
      adminSubscriptionsApi.listOrders({ user_id: props.user.id }),
    ])
    subscriptionProducts.value = productsResponse.products
    currentManagedSubscription.value = subscription
    subscriptionOrders.value = ordersResponse.orders
    subscriptionProductId.value = subscription?.product_id || ''
    subscriptionPlanId.value = ''
    subscriptionMonths.value = subscription?.purchased_months || 1
  } catch (err) {
    error(parseApiError(err, '加载用户订阅信息失败'))
  } finally {
    loadingDrawerData.value = false
  }
}

async function submitSubscriptionAction() {
  if (!props.user || !subscriptionPlanId.value) return
  const actionKind = selectedActionKind.value

  try {
    savingSubscriptionAction.value = true
    if (currentManagedSubscription.value) {
      await adminSubscriptionsApi.upgradeUserSubscription(currentManagedSubscription.value.id, {
        new_plan_id: subscriptionPlanId.value,
        purchased_months: subscriptionMonths.value,
      })
      success(actionKind === 'renewal' ? '订阅已续订' : '订阅已升级')
    } else {
      await adminSubscriptionsApi.createUserSubscription(props.user.id, {
        plan_id: subscriptionPlanId.value,
        purchased_months: subscriptionMonths.value,
      })
      success('订阅已开通')
    }
    await refreshDrawerData()
    emit('changed')
  } catch (err) {
    error(parseApiError(
      err,
      currentManagedSubscription.value
        ? actionKind === 'renewal' ? '续订订阅失败' : '升级订阅失败'
        : '开通订阅失败'
    ))
  } finally {
    savingSubscriptionAction.value = false
  }
}

function openCancelConfirmDialog() {
  if (!currentManagedSubscription.value) return
  cancelImmediateChoice.value = false
  showCancelConfirmDialog.value = true
}

function handleCancelDialogUpdate(value: boolean) {
  showCancelConfirmDialog.value = value
}

async function submitCancelSubscription() {
  if (!currentManagedSubscription.value) return

  try {
    savingSubscriptionAction.value = true
    await adminSubscriptionsApi.cancelUserSubscription(currentManagedSubscription.value.id, {
      immediate: cancelImmediateChoice.value,
    })
    success(cancelImmediateChoice.value ? '订阅已立即取消' : '订阅将在到期后取消')
    showCancelConfirmDialog.value = false
    await refreshDrawerData()
    emit('changed')
  } catch (err) {
    error(parseApiError(err, '取消订阅失败'))
  } finally {
    savingSubscriptionAction.value = false
  }
}

async function approveOrder(order: SubscriptionOrder) {
  const confirmed = await confirmWarning(
    `确认通过订单 ${order.order_no} 吗？通过后会立即生效对应订阅。`,
    '通过订阅审核',
  )
  if (!confirmed) return

  try {
    reviewingOrderId.value = order.id
    await adminSubscriptionsApi.approveOrder(order.id)
    success('订阅订单已通过')
    await refreshDrawerData()
    emit('changed')
  } catch (err) {
    error(parseApiError(err, '通过订阅审核失败'))
  } finally {
    reviewingOrderId.value = null
  }
}

async function rejectOrder(order: SubscriptionOrder) {
  const confirmed = await confirmDanger(
    `确认拒绝订单 ${order.order_no} 吗？拒绝后该订阅申请会被取消。`,
    '拒绝订阅审核',
    '拒绝',
  )
  if (!confirmed) return

  try {
    reviewingOrderId.value = order.id
    await adminSubscriptionsApi.rejectOrder(order.id)
    success('订阅订单已拒绝')
    await refreshDrawerData()
    emit('changed')
  } catch (err) {
    error(parseApiError(err, '拒绝订阅审核失败'))
  } finally {
    reviewingOrderId.value = null
  }
}

function formatCurrencyValue(value: number | null | undefined, nullLabel = '-'): string {
  if (value == null || Number.isNaN(Number(value))) return nullLabel
  return `$${Number(value).toFixed(2)}`
}

function formatDateTime(dateString: string | null | undefined): string {
  if (!dateString) return '-'
  const date = new Date(dateString)
  if (Number.isNaN(date.getTime())) return '-'
  return date.toLocaleString('zh-CN', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  })
}

function formatOveragePolicy(policy?: string | null): string {
  return policy === 'use_wallet_balance' ? '扣钱包' : '拦截'
}

function formatManagedSubscriptionName(subscription: UserSubscription): string {
  if (subscription.product_name && subscription.variant_name) {
    return `${subscription.product_name} · ${subscription.variant_name}`
  }
  return subscription.plan_name || subscription.product_name || '-'
}

function formatSubscriptionStatus(status: string | null | undefined): string {
  const labels: Record<string, string> = {
    pending_payment: '待支付',
    active: '生效中',
    canceled: '已取消',
    expired: '已过期',
  }
  if (!status) return '未知'
  return labels[status] || status
}

function subscriptionStatusBadge(status: string | null | undefined): 'success' | 'warning' | 'destructive' | 'secondary' | 'outline' {
  if (status === 'active') return 'success'
  if (status === 'pending_payment') return 'warning'
  if (status === 'canceled') return 'destructive'
  if (status === 'expired') return 'secondary'
  return 'outline'
}

function subscriptionProductLabel(product: SubscriptionProduct): string {
  if (product.user_group_name) {
    return `${product.name} · ${product.user_group_name}`
  }
  return product.name
}

function transitionKindForVariant(
  subscription: UserSubscription,
  variant: SubscriptionVariant
): SubscriptionActionKind {
  const currentVariant = subscriptionVariantMap.value.get(subscription.plan_id)
  if (!currentVariant) return 'unavailable'
  if (currentVariant.id === variant.id) return 'renewal'
  const currentProduct = subscriptionProductMap.value.get(currentVariant.product_id)
  const targetProduct = subscriptionProductMap.value.get(variant.product_id)
  if (!currentProduct || !targetProduct) return 'unavailable'
  if (!targetProduct.is_active || !variant.is_active) return 'unavailable'
  if (currentProduct.id === targetProduct.id) {
    return variant.variant_rank > currentVariant.variant_rank ? 'upgrade' : 'unavailable'
  }
  return targetProduct.plan_level > currentProduct.plan_level ? 'upgrade' : 'unavailable'
}

function subscriptionOrderTypeLabel(orderType: string | null | undefined): string {
  if (orderType === 'subscription_renewal') return '订阅续期'
  if (orderType === 'subscription_upgrade') return '订阅升级'
  return '新购订阅'
}

function subscriptionOrderPlanLabel(order: SubscriptionOrder): string {
  if (order.product_name && order.variant_name) {
    return `${order.product_name} · ${order.variant_name}`
  }
  return order.plan_name || order.product_name || '-'
}

function sortVariants(variants: SubscriptionVariant[]) {
  return [...variants].sort((a, b) => a.variant_rank - b.variant_rank)
}

function sortDiscounts(items: Array<{ months: number; discount_factor: number }>) {
  return [...items].sort((a, b) => a.months - b.months)
}

function formatDurationDiscount(discountFactor: number): string {
  if (discountFactor === 1) return '标准价格'
  if (discountFactor < 1) {
    const discount = discountFactor * 10
    return `${Number.isInteger(discount) ? discount : discount.toFixed(1)} 折`
  }
  return `系数 ${discountFactor.toFixed(2)}`
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
