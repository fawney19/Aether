<template>
  <div class="space-y-6 pb-8">
    <div
      v-if="loadingInitial"
      class="py-16"
    >
      <LoadingState message="正在加载订阅信息..." />
    </div>

    <template v-else>
      <div class="space-y-2.5 sm:hidden">
        <Card class="overflow-hidden border border-border/60 bg-[radial-gradient(circle_at_top_left,hsl(var(--primary)/0.12),transparent_58%),linear-gradient(180deg,hsl(var(--background)),hsl(var(--muted)/0.34))] p-3.5">
          <div class="flex items-start justify-between gap-3">
            <div class="min-w-0 space-y-1">
              <div class="inline-flex items-center gap-2 text-[11px] font-medium text-muted-foreground">
                <CreditCard class="h-3.5 w-3.5" />
                订阅中心
              </div>
              <h2 class="text-[1.35rem] font-semibold tracking-tight text-foreground">
                {{ currentSubscriptionTitle }}
              </h2>
              <p class="text-xs leading-5 text-muted-foreground">
                {{ currentSubscriptionSubtitle }}
              </p>
            </div>

            <Badge
              variant="outline"
              class="h-6 shrink-0 px-2.5 text-[11px] text-muted-foreground"
            >
              {{ subscriptionStatusLabel(currentSubscription?.status) }}
            </Badge>
          </div>

          <div class="mt-3 grid grid-cols-2 gap-2">
            <div class="rounded-xl border border-border/50 bg-background/88 p-2.5">
              <div class="text-[11px] text-muted-foreground">
                钱包余额
              </div>
              <div class="mt-0.5 text-[15px] font-semibold tabular-nums text-foreground">
                {{ formatCurrency(walletBalance?.balance) }}
              </div>
              <div class="mt-1 text-[11px] text-muted-foreground">
                {{ overagePolicyLabel(currentSubscriptionOveragePolicy) }}
              </div>
            </div>
            <div class="rounded-xl border border-border/50 bg-background/88 p-2.5">
              <div class="text-[11px] text-muted-foreground">
                购买时长
              </div>
              <div class="mt-0.5 text-[15px] font-semibold text-foreground">
                {{ currentSubscription ? `${currentSubscription.purchased_months} 个月` : '-' }}
              </div>
            </div>
            <div class="rounded-xl border border-border/50 bg-background/88 p-2.5">
              <div class="text-[11px] text-muted-foreground">
                开始时间
              </div>
              <div class="mt-0.5 text-[15px] font-semibold text-foreground">
                {{ currentSubscription ? formatDate(currentSubscription.started_at) : '-' }}
              </div>
            </div>
            <div class="rounded-xl border border-border/50 bg-background/88 p-2.5">
              <div class="text-[11px] text-muted-foreground">
                到期时间
              </div>
              <div class="mt-0.5 text-[15px] font-semibold text-foreground">
                {{ currentSubscription ? formatDate(currentSubscription.ends_at) : '-' }}
              </div>
            </div>
          </div>
        </Card>

        <Card class="border border-border/60 bg-card/95 p-3.5">
          <div class="flex items-start justify-between gap-3">
            <div>
              <div class="text-sm font-semibold text-foreground">
                本周期用量
              </div>
              <div class="mt-1 text-[11px] text-muted-foreground">
                {{ currentSubscription ? `${formatDate(currentSubscription.current_cycle_start)} 至 ${formatDate(currentSubscription.current_cycle_end)}` : '开通订阅后按月重置' }}
              </div>
            </div>
            <Badge
              variant="outline"
              class="h-6 shrink-0 px-2.5 text-[11px]"
            >
              {{ currentSubscription ? `${quotaProgress(currentSubscription)}%` : '未开通' }}
            </Badge>
          </div>

          <div class="mt-3">
            <div class="flex items-end justify-between gap-3">
              <div>
                <div class="text-[1.45rem] font-semibold tabular-nums text-foreground">
                  {{ formatCurrency(currentSubscription?.remaining_quota_usd) }}
                </div>
                <div class="mt-1 text-[11px] text-muted-foreground">
                  月额度 {{ formatCurrency(currentSubscription?.cycle_quota_usd) }}
                </div>
              </div>
              <div class="text-right text-[11px] text-muted-foreground">
                已用 {{ formatCurrency(currentSubscription?.cycle_used_usd) }}
              </div>
            </div>

            <div class="mt-3 h-2 overflow-hidden rounded-full bg-muted">
              <div
                class="h-full rounded-full bg-primary transition-all"
                :style="{ width: `${quotaProgress(currentSubscription)}%` }"
              />
            </div>
          </div>

          <div class="mt-3 grid grid-cols-2 gap-2">
            <div class="rounded-xl border border-border/50 bg-muted/18 p-2.5">
              <div class="text-[11px] text-muted-foreground">
                当前版本
              </div>
              <div class="mt-0.5 text-[15px] font-semibold text-foreground">
                {{ currentSubscription?.variant_name || '-' }}
              </div>
            </div>
            <div class="rounded-xl border border-border/50 bg-muted/18 p-2.5">
              <div class="text-[11px] text-muted-foreground">
                下次重置
              </div>
              <div class="mt-0.5 text-[15px] font-semibold text-foreground">
                {{ currentSubscription ? formatDate(currentSubscription.current_cycle_end) : '-' }}
              </div>
            </div>
          </div>

          <div class="mt-3 rounded-xl border border-border/40 bg-background/85 px-3 py-2 text-[11px] leading-5 text-muted-foreground">
            额度每周期自动清零，不会结转；若支持超额，将继续从钱包余额扣费。
          </div>
        </Card>
      </div>

      <div class="hidden gap-4 sm:grid xl:grid-cols-[0.96fr_1.04fr]">
        <Card class="p-4">
          <div class="flex flex-wrap items-start justify-between gap-4">
            <div class="space-y-1.5">
              <div class="inline-flex items-center gap-2 text-xs font-medium text-muted-foreground">
                <CreditCard class="h-3.5 w-3.5" />
                订阅中心
              </div>
              <h2 class="text-2xl font-semibold tracking-tight text-foreground">
                {{ currentSubscriptionTitle }}
              </h2>
              <p class="text-sm text-muted-foreground">
                {{ currentSubscriptionSubtitle }}
              </p>
            </div>

            <Badge
              variant="outline"
              class="h-7 px-2.5 text-[11px] text-muted-foreground"
            >
              {{ subscriptionStatusLabel(currentSubscription?.status) }}
            </Badge>
          </div>

          <div class="mt-4 grid gap-3 sm:grid-cols-2">
            <div class="rounded-xl border border-border/60 bg-muted/10 px-4 py-3">
              <div class="text-xs text-muted-foreground">
                钱包余额
              </div>
              <div class="mt-1 text-lg font-semibold tabular-nums">
                {{ formatCurrency(walletBalance?.balance) }}
              </div>
              <div class="mt-1 text-xs text-muted-foreground">
                超额策略 {{ overagePolicyLabel(currentSubscriptionOveragePolicy) }}
              </div>
            </div>

            <div class="rounded-xl border border-border/60 bg-muted/10 px-4 py-3">
              <div class="text-xs text-muted-foreground">
                购买时长
              </div>
              <div class="mt-1 text-lg font-semibold">
                {{ currentSubscription ? `${currentSubscription.purchased_months} 个月` : '-' }}
              </div>
            </div>

            <div class="rounded-xl border border-border/60 bg-muted/10 px-4 py-3">
              <div class="text-xs text-muted-foreground">
                开始时间
              </div>
              <div class="mt-1 text-lg font-semibold">
                {{ currentSubscription ? formatDate(currentSubscription.started_at) : '-' }}
              </div>
            </div>

            <div class="rounded-xl border border-border/60 bg-muted/10 px-4 py-3">
              <div class="text-xs text-muted-foreground">
                到期时间
              </div>
              <div class="mt-1 text-lg font-semibold">
                {{ currentSubscription ? formatDate(currentSubscription.ends_at) : '-' }}
              </div>
            </div>
          </div>
        </Card>

        <Card class="p-5">
          <div class="flex items-center justify-between gap-3">
            <div>
              <div class="text-sm font-semibold">
                本周期用量
              </div>
              <div class="mt-1 text-xs text-muted-foreground">
                {{ currentSubscription ? `${formatDate(currentSubscription.current_cycle_start)} 至 ${formatDate(currentSubscription.current_cycle_end)}` : '开通订阅后按月重置' }}
              </div>
            </div>
            <Badge
              variant="outline"
              class="h-7 px-2.5 text-[11px]"
            >
              {{ currentSubscription ? `${quotaProgress(currentSubscription)}%` : '未开通' }}
            </Badge>
          </div>

          <div class="mt-5 space-y-4">
            <div>
              <div class="flex items-end justify-between gap-3">
                <div>
                  <div class="text-3xl font-semibold tabular-nums">
                    {{ formatCurrency(currentSubscription?.remaining_quota_usd) }}
                  </div>
                  <div class="mt-1 text-xs text-muted-foreground">
                    剩余额度 / 月额度 {{ formatCurrency(currentSubscription?.cycle_quota_usd) }}
                  </div>
                </div>
                <div class="text-right text-xs text-muted-foreground">
                  已用 {{ formatCurrency(currentSubscription?.cycle_used_usd) }}
                </div>
              </div>

              <div class="mt-4 h-2.5 overflow-hidden rounded-full bg-muted">
                <div
                  class="h-full rounded-full bg-primary transition-all"
                  :style="{ width: `${quotaProgress(currentSubscription)}%` }"
                />
              </div>
            </div>

            <div class="grid gap-3 sm:grid-cols-2">
              <div class="rounded-xl border border-border/60 bg-muted/10 px-4 py-3">
                <div class="text-xs text-muted-foreground">
                  当前版本
                </div>
                <div class="mt-1 text-lg font-semibold">
                  {{ currentSubscription?.variant_name || '-' }}
                </div>
              </div>
              <div class="rounded-xl border border-border/60 bg-muted/10 px-4 py-3">
                <div class="text-xs text-muted-foreground">
                  下次重置
                </div>
                <div class="mt-1 text-lg font-semibold">
                  {{ currentSubscription ? formatDate(currentSubscription.current_cycle_end) : '-' }}
                </div>
              </div>
            </div>
          </div>

          <div class="mt-4 rounded-xl border border-dashed border-border/70 bg-muted/10 px-4 py-3 text-xs leading-6 text-muted-foreground">
            订阅额度每个周期自动清零，不会结转到下个周期。若额度耗尽且套餐支持超额，将继续从钱包余额扣费。
          </div>
        </Card>
      </div>

      <Card
        v-if="latestPendingOrder"
        class="overflow-hidden border border-border/60 bg-gradient-to-r from-muted/25 via-background to-muted/10 sm:hidden"
      >
        <div class="space-y-3 px-3.5 py-3">
          <div class="flex flex-wrap items-center gap-2">
            <Badge
              :variant="paymentStatusBadge(latestPendingOrder.status)"
              class="h-6 whitespace-nowrap px-2.5 text-[11px]"
            >
              {{ latestPendingOrder.status === 'pending_approval' ? '待审核订单' : '待支付订单' }}
            </Badge>
            <Badge
              variant="outline"
              class="h-6 whitespace-nowrap px-2.5 text-[11px]"
            >
              {{ subscriptionOrderTypeLabel(latestPendingOrder.order_type) }}
            </Badge>
            <Badge
              variant="outline"
              class="h-6 whitespace-nowrap px-2.5 text-[11px]"
            >
              {{ paymentMethodLabel(latestPendingOrder.payment_method) }}
            </Badge>
          </div>

          <div class="flex items-start justify-between gap-3">
            <div class="min-w-0">
              <div
                class="truncate text-sm font-semibold text-foreground"
                :title="`${subscriptionOrderPlanLabel(latestPendingOrder)} · ${formatCurrency(latestPendingOrder.amount_usd)}`"
              >
                {{ subscriptionOrderPlanLabel(latestPendingOrder) }}
              </div>
              <div class="mt-1 text-xs text-muted-foreground">
                {{ pendingOrderDateLabel(latestPendingOrder) }}
              </div>
              <div
                class="mt-1 truncate text-[11px] text-muted-foreground"
                :title="latestPendingOrder.order_no"
              >
                订单号 {{ compactOrderNo(latestPendingOrder.order_no) }}
              </div>
            </div>

            <div class="text-right">
              <div class="text-[11px] text-muted-foreground">
                应付金额
              </div>
              <div class="mt-0.5 text-base font-semibold tabular-nums text-foreground">
                {{ formatCurrency(latestPendingOrder.amount_usd) }}
              </div>
            </div>
          </div>

          <div class="flex flex-wrap gap-2">
            <a
              v-if="paymentUrl(latestPendingOrder)"
              :href="paymentUrl(latestPendingOrder)"
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
              v-if="canCancelSubscriptionOrder(latestPendingOrder)"
              variant="outline"
              size="sm"
              class="h-8 px-3"
              :disabled="cancelingOrderId === latestPendingOrder.id"
              @click="cancelSubscriptionOrder(latestPendingOrder)"
            >
              {{ cancelingOrderId === latestPendingOrder.id ? '取消中...' : '取消订单' }}
            </Button>
          </div>
        </div>
      </Card>

      <Card
        v-if="latestPendingOrder"
        class="hidden overflow-hidden border border-border/60 bg-gradient-to-r from-muted/25 via-background to-muted/10 sm:block"
      >
        <div class="flex flex-col gap-3 px-4 py-3 xl:flex-row xl:items-center xl:justify-between">
          <div class="min-w-0 flex-1">
            <div class="flex flex-wrap items-center gap-x-3 gap-y-2 xl:flex-nowrap">
              <div
                class="flex shrink-0 flex-wrap items-center gap-2"
              >
                <Badge
                  :variant="paymentStatusBadge(latestPendingOrder.status)"
                  class="h-6 shrink-0 whitespace-nowrap px-2.5 text-[11px]"
                >
                  {{ latestPendingOrder.status === 'pending_approval' ? '待审核订单' : '待支付订单' }}
                </Badge>
                <Badge
                  variant="outline"
                  class="h-6 shrink-0 whitespace-nowrap px-2.5 text-[11px]"
                >
                  {{ subscriptionOrderTypeLabel(latestPendingOrder.order_type) }}
                </Badge>
                <Badge
                  variant="outline"
                  class="h-6 shrink-0 whitespace-nowrap px-2.5 text-[11px]"
                >
                  {{ paymentMethodLabel(latestPendingOrder.payment_method) }}
                </Badge>
                <Badge
                  v-if="latestPendingOrder.purchased_months"
                  variant="outline"
                  class="h-6 shrink-0 whitespace-nowrap px-2.5 text-[11px]"
                >
                  {{ latestPendingOrder.purchased_months }} 个月
                </Badge>
              </div>
              <div class="min-w-0 flex flex-1 flex-wrap items-center gap-x-3 gap-y-1">
                <div
                  class="min-w-0 truncate text-sm font-semibold text-foreground"
                  :title="`${subscriptionOrderPlanLabel(latestPendingOrder)} · ${formatCurrency(latestPendingOrder.amount_usd)}`"
                >
                  {{ subscriptionOrderPlanLabel(latestPendingOrder) }} · {{ formatCurrency(latestPendingOrder.amount_usd) }}
                </div>
                <span class="shrink-0 text-xs text-muted-foreground">
                  {{ pendingOrderDateLabel(latestPendingOrder) }}
                </span>
                <span
                  class="min-w-0 truncate text-[11px] text-muted-foreground"
                  :title="latestPendingOrder.order_no"
                >
                  订单号 {{ latestPendingOrder.order_no }}
                </span>
              </div>
            </div>
          </div>

          <div class="flex shrink-0 items-center gap-2">
            <a
              v-if="paymentUrl(latestPendingOrder)"
              :href="paymentUrl(latestPendingOrder)"
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
              v-if="canCancelSubscriptionOrder(latestPendingOrder)"
              variant="outline"
              size="sm"
              class="h-8 px-3"
              :disabled="cancelingOrderId === latestPendingOrder.id"
              @click="cancelSubscriptionOrder(latestPendingOrder)"
            >
              {{ cancelingOrderId === latestPendingOrder.id ? '取消中...' : '取消订单' }}
            </Button>
          </div>
        </div>
      </Card>

      <Card class="overflow-hidden">
        <div class="border-b border-border/60 px-5 py-4">
          <div class="flex flex-col gap-3 lg:flex-row lg:items-center lg:justify-between">
            <div>
              <h3 class="text-base font-semibold">
                套餐与订单
              </h3>
              <p class="mt-1 text-xs text-muted-foreground">
                选购不同的套餐，或升级套餐、用量。
              </p>
            </div>
          </div>
        </div>

        <div class="px-4 py-4 sm:px-5 sm:py-5">
          <Tabs v-model="activeTab">
            <TabsList class="tabs-button-list grid w-full max-w-[360px] grid-cols-2">
              <TabsTrigger value="products">
                购买套餐
              </TabsTrigger>
              <TabsTrigger value="orders">
                订阅订单
              </TabsTrigger>
            </TabsList>

            <TabsContent
              value="products"
              class="mt-4 space-y-3"
            >
              <div class="grid gap-3 [grid-template-columns:repeat(auto-fit,minmax(min(100%,18rem),1fr))] xl:[grid-template-columns:repeat(auto-fit,minmax(min(100%,19rem),1fr))]">
                <Card
                  v-for="product in products"
                  :key="product.id"
                  class="h-full overflow-hidden border border-border/70 p-0"
                >
                  <div class="border-b border-border/60 bg-muted/10 px-3.5 py-3 sm:px-4">
                    <div class="flex items-start justify-between gap-3">
                      <div class="min-w-0 flex-1">
                        <div class="truncate text-[15px] font-semibold sm:text-base">
                          {{ product.name }}
                        </div>
                        <p
                          class="mt-0.5 h-4 truncate text-[10px] leading-4 text-muted-foreground sm:h-5 sm:text-[11px] sm:leading-5"
                          :title="product.description || undefined"
                        >
                          {{ product.description || '' }}
                        </p>
                      </div>
                      <Badge
                        variant="outline"
                        class="h-6 px-2 text-[10px] text-muted-foreground sm:h-7 sm:px-2.5 sm:text-[11px]"
                      >
                        {{ selectedVariantStatusLabel(selectedVariant(product)) }}
                      </Badge>
                    </div>
                  </div>

                  <div class="space-y-2.5 px-3.5 py-3.5 sm:px-4 sm:py-4">
                    <div class="flex flex-wrap gap-1.5">
                      <Button
                        v-for="variant in sortedVariants(product.variants)"
                        :key="variant.id"
                        type="button"
                        size="sm"
                        :variant="selectedVariant(product)?.id === variant.id ? 'secondary' : 'outline'"
                        class="h-6 max-w-full rounded-md px-2 text-[11px] whitespace-nowrap sm:h-7 sm:px-2.5 sm:text-xs"
                        @click="selectVariant(product.id, variant.id)"
                      >
                        {{ variant.name }}
                      </Button>
                    </div>

                    <div v-if="selectedVariant(product)">
                      <div class="grid gap-2.5 sm:grid-cols-[minmax(0,11.5rem)_minmax(0,1fr)]">
                        <div class="rounded-xl border border-border/60 bg-muted/10 px-3.5 py-3 sm:px-4">
                          <div class="text-[10px] text-muted-foreground sm:text-[11px]">
                            当前价格
                          </div>
                          <div class="mt-1 text-xl font-semibold tabular-nums sm:text-2xl">
                            {{ formatCurrency(variantPrice(selectedVariant(product)!).total) }}
                          </div>
                          <div class="mt-1 text-[10px] text-muted-foreground sm:text-[11px]">
                            {{ `${selectedMonthsForVariant(selectedVariant(product)!)} 个月合计` }}
                          </div>
                          <div class="mt-2.5 flex items-center gap-2">
                            <Input
                              :model-value="String(selectedMonthsForVariant(selectedVariant(product)!))"
                              type="number"
                              min="1"
                              inputmode="numeric"
                              class="h-8 w-20 bg-background text-sm sm:h-9 sm:w-24"
                              @update:model-value="(value) => updateSelectedMonthsForVariant(selectedVariant(product)!, value)"
                            />
                            <span class="text-[11px] text-muted-foreground sm:text-xs">个月</span>
                          </div>
                        </div>

                        <div class="rounded-xl border border-border/60 bg-background px-3.5 py-3 sm:px-4">
                          <div class="grid gap-x-4 gap-y-1.5 text-[13px] text-muted-foreground sm:text-sm">
                            <div class="flex items-start justify-between gap-4">
                              <span class="shrink-0 whitespace-nowrap">月额度</span>
                              <span class="min-w-0 whitespace-nowrap text-right font-medium tabular-nums text-foreground">
                                {{ formatCurrency(selectedVariant(product)!.monthly_quota_usd) }}
                              </span>
                            </div>
                            <div class="flex items-start justify-between gap-4">
                              <span class="shrink-0 whitespace-nowrap">超额策略</span>
                              <span class="min-w-0 whitespace-nowrap text-right font-medium text-foreground">
                                {{ overagePolicyLabel(product.overage_policy) }}
                              </span>
                            </div>
                            <div class="flex items-start justify-between gap-4">
                              <span class="shrink-0 whitespace-nowrap">基础月费</span>
                              <span class="min-w-0 whitespace-nowrap text-right font-medium tabular-nums text-foreground">
                                {{ formatCurrency(selectedVariant(product)!.monthly_price_usd) }}
                              </span>
                            </div>
                            <div class="flex items-start justify-between gap-4">
                              <span class="shrink-0 whitespace-nowrap">当前折扣</span>
                              <span class="min-w-0 whitespace-nowrap text-right font-medium text-foreground">
                                {{ formatDiscount(appliedDiscountFactor(selectedVariant(product)!)) }}
                              </span>
                            </div>
                          </div>

                          <div class="mt-2.5 flex flex-wrap gap-1.5">
                            <Badge
                              v-for="discount in sortedDiscounts(selectedVariant(product)!.term_discounts_json)"
                              :key="`${selectedVariant(product)!.id}-${discount.months}`"
                              variant="outline"
                              class="h-5 rounded-md px-1.5 text-[10px] sm:h-6 sm:px-2"
                            >
                              满 {{ discount.months }} 个月 {{ formatDiscount(discount.discount_factor) }}
                            </Badge>
                          </div>
                        </div>
                      </div>

                      <div
                        v-if="variantPrice(selectedVariant(product)!).original !== null"
                        class="mt-1.5 text-[10px] text-muted-foreground line-through sm:text-[11px]"
                      >
                        原价 {{ formatCurrency(variantPrice(selectedVariant(product)!).original ?? 0) }}
                      </div>
                    </div>

                    <Button
                      variant="outline"
                      class="h-8 w-full text-xs sm:h-9"
                      @click="openModelsDialog(product)"
                    >
                      查看可用模型
                    </Button>

                    <Button
                      class="h-9 w-full text-sm"
                      :disabled="!selectedVariant(product) || !canSelectVariant(selectedVariant(product)!) || purchasingVariantId === selectedVariant(product)!.id"
                      @click="openCheckoutDialog(product)"
                    >
                      {{ selectedVariant(product) ? selectedVariantButtonLabel(selectedVariant(product)!) : '暂不可购买' }}
                    </Button>
                  </div>
                </Card>
              </div>
            </TabsContent>

            <TabsContent
              value="orders"
              class="mt-5 space-y-4"
            >
              <div class="flex items-center justify-between px-1 sm:px-0">
                <div class="text-sm text-muted-foreground">
                  共 {{ orders.length }} 条
                </div>
                <RefreshButton
                  :loading="loadingOrders"
                  @click="loadOrders"
                />
              </div>

              <div class="space-y-2.5 sm:hidden">
                <div
                  v-for="order in orders"
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
                        class="mt-2.5 truncate text-sm font-semibold text-foreground"
                        :title="subscriptionOrderPlanLabel(order)"
                      >
                        {{ subscriptionOrderPlanLabel(order) }}
                      </div>
                      <div
                        class="mt-1 truncate font-mono text-[11px] text-muted-foreground"
                        :title="order.order_no"
                      >
                        {{ compactOrderNo(order.order_no) }}
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
                        订单类型
                      </div>
                      <div class="mt-0.5 font-medium text-foreground">
                        {{ subscriptionOrderTypeLabel(order.order_type) }}
                      </div>
                      <div class="mt-1 text-[11px] text-muted-foreground">
                        {{ order.purchased_months ? `${order.purchased_months} 个月` : '套餐信息待同步' }}
                      </div>
                    </div>
                    <div class="rounded-xl border border-border/40 bg-muted/18 p-2.5">
                      <div class="text-muted-foreground">
                        创建时间
                      </div>
                      <div class="mt-0.5 font-medium text-foreground">
                        {{ formatDateLabel(order.created_at) }}
                      </div>
                      <div class="mt-1 text-[11px] text-muted-foreground">
                        {{ formatTimeLabel(order.created_at) }}
                      </div>
                    </div>
                  </div>

                  <div
                    v-if="!canCancelSubscriptionOrder(order) && !(order.status === 'pending' && paymentUrl(order))"
                    class="mt-2.5 rounded-xl border border-border/40 bg-background/85 px-3 py-2 text-[11px] leading-5 text-muted-foreground"
                  >
                    {{ subscriptionOrderActionText(order) }}
                  </div>

                  <div
                    v-if="(order.status === 'pending' && paymentUrl(order)) || canCancelSubscriptionOrder(order)"
                    class="mt-2.5 flex flex-wrap gap-2"
                  >
                    <a
                      v-if="order.status === 'pending' && paymentUrl(order)"
                      :href="paymentUrl(order)"
                      target="_blank"
                      rel="noopener noreferrer"
                      class="inline-flex h-8 items-center justify-center rounded-md border border-border bg-background px-3 text-xs font-medium text-foreground transition hover:bg-muted"
                    >
                      去支付
                    </a>
                    <Button
                      v-if="canCancelSubscriptionOrder(order)"
                      size="sm"
                      variant="outline"
                      class="h-8 px-3"
                      :disabled="cancelingOrderId === order.id"
                      @click="cancelSubscriptionOrder(order)"
                    >
                      {{ cancelingOrderId === order.id ? '取消中...' : '取消订单' }}
                    </Button>
                  </div>
                </div>

                <EmptyState
                  v-if="!loadingOrders && orders.length === 0"
                  title="还没有订阅订单"
                  description="购买或升级订阅后，支付订单会出现在这里。"
                />
              </div>

              <div class="hidden overflow-hidden rounded-2xl border border-border/60 bg-background sm:block">
                <div class="overflow-x-auto">
                  <Table class="w-full table-fixed">
                    <TableHeader>
                      <TableRow>
                        <TableHead class="w-[29%] whitespace-nowrap">订单号</TableHead>
                        <TableHead class="w-[15%] whitespace-nowrap">套餐</TableHead>
                        <TableHead class="w-[10%] whitespace-nowrap">金额</TableHead>
                        <TableHead class="w-[11%] whitespace-nowrap">支付方式</TableHead>
                        <TableHead class="w-[10%] whitespace-nowrap">状态</TableHead>
                        <TableHead class="w-[13%] whitespace-nowrap">创建时间</TableHead>
                        <TableHead class="w-[12%] whitespace-nowrap text-right">
                          操作
                        </TableHead>
                      </TableRow>
                    </TableHeader>
                    <TableBody>
                      <TableRow
                        v-for="order in orders"
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
                            {{ subscriptionOrderTypeLabel(order.order_type) }}
                          </div>
                        </TableCell>
                        <TableCell class="py-4 align-top">
                          <div
                            class="max-w-full truncate font-medium"
                            :title="subscriptionOrderPlanLabel(order)"
                          >
                            {{ subscriptionOrderPlanLabel(order) }}
                          </div>
                          <div class="mt-1 truncate text-xs text-muted-foreground">
                            {{ order.purchased_months ? `${order.purchased_months} 个月` : '套餐信息待同步' }}
                          </div>
                        </TableCell>
                        <TableCell class="py-4 align-top whitespace-nowrap text-sm font-medium tabular-nums">
                          {{ formatCurrency(order.amount_usd) }}
                        </TableCell>
                        <TableCell class="py-4 align-top">
                          <Badge
                            variant="outline"
                            class="h-8 whitespace-nowrap px-3 py-0"
                          >
                            {{ paymentMethodLabel(order.payment_method) }}
                          </Badge>
                        </TableCell>
                        <TableCell class="py-4 align-top">
                          <Badge
                            :variant="paymentStatusBadge(order.status)"
                            class="h-8 whitespace-nowrap px-3 py-0"
                          >
                            {{ paymentStatusLabel(order.status) }}
                          </Badge>
                        </TableCell>
                        <TableCell class="py-4 align-top text-sm text-muted-foreground">
                          <div class="whitespace-nowrap">
                            {{ formatDateLabel(order.created_at) }}
                          </div>
                          <div class="mt-1 whitespace-nowrap text-xs text-muted-foreground">
                            {{ formatTimeLabel(order.created_at) }}
                          </div>
                        </TableCell>
                        <TableCell class="py-4 align-top text-right">
                          <div class="flex justify-end gap-2">
                            <a
                              v-if="order.status === 'pending' && paymentUrl(order)"
                              :href="paymentUrl(order)"
                              target="_blank"
                              rel="noopener noreferrer"
                              class="inline-flex rounded-md border border-border bg-background px-3 py-1.5 text-xs font-medium text-foreground transition hover:bg-muted"
                            >
                              去支付
                            </a>
                            <Button
                              v-if="canCancelSubscriptionOrder(order)"
                              size="sm"
                              variant="outline"
                              :disabled="cancelingOrderId === order.id"
                              @click="cancelSubscriptionOrder(order)"
                            >
                              {{ cancelingOrderId === order.id ? '取消中...' : '取消订单' }}
                            </Button>
                            <span
                              v-if="!canCancelSubscriptionOrder(order) && !(order.status === 'pending' && paymentUrl(order))"
                              class="inline-block max-w-full truncate text-xs text-muted-foreground"
                              :title="subscriptionOrderActionText(order)"
                            >
                              {{ subscriptionOrderActionText(order) }}
                            </span>
                          </div>
                        </TableCell>
                      </TableRow>
                      <TableRow v-if="!loadingOrders && orders.length === 0">
                        <TableCell
                          colspan="7"
                          class="py-12"
                        >
                          <EmptyState
                            title="还没有订阅订单"
                            description="购买或升级订阅后，支付订单会出现在这里。"
                          />
                        </TableCell>
                      </TableRow>
                    </TableBody>
                  </Table>
                </div>
              </div>
            </TabsContent>
          </Tabs>
        </div>
      </Card>

      <Dialog
        :model-value="showCheckoutDialog"
        size="md"
        @update:model-value="handleCheckoutDialogUpdate"
      >
        <template #header>
          <div class="border-b border-border px-6 py-4">
            <div>
              <h3 class="text-lg font-semibold">
                {{
                  checkoutMode === 'upgrade'
                    ? '确认升级订阅'
                    : checkoutMode === 'renewal'
                      ? '确认续订订阅'
                      : '确认购买订阅'
                }}
              </h3>
              <p class="text-xs text-muted-foreground">
                请确认版本信息并选择支付方式。
              </p>
            </div>
          </div>
        </template>

        <div class="space-y-4">
          <div class="rounded-2xl border border-border/60 bg-muted/20 p-4 text-sm">
            <div class="flex items-center justify-between gap-4">
              <span class="text-muted-foreground">产品</span>
              <span class="font-medium text-foreground">
                {{ checkoutTargetProduct?.name || '-' }}
              </span>
            </div>
            <div class="mt-3 flex items-center justify-between gap-4">
              <span class="text-muted-foreground">版本</span>
              <span class="font-medium text-foreground">
                {{ checkoutTargetVariant?.name || '-' }}
              </span>
            </div>
            <div class="mt-3 flex items-center justify-between gap-4">
              <span class="text-muted-foreground">购买时长</span>
              <span class="font-medium text-foreground">
                {{ checkoutMonths }} 个月
              </span>
            </div>
            <div class="mt-3 flex items-center justify-between gap-4">
              <span class="text-muted-foreground">应付金额</span>
              <span class="font-semibold text-foreground">
                {{ formatCurrency(checkoutTargetVariant ? variantPrice(checkoutTargetVariant, checkoutMonths).total : 0) }}
              </span>
            </div>
          </div>

          <div class="space-y-2">
            <Label>支付方式</Label>
            <div class="grid grid-cols-3 gap-3">
              <Button
                type="button"
                :variant="checkoutPaymentMethod === 'alipay' ? 'secondary' : 'outline'"
                @click="checkoutPaymentMethod = 'alipay'"
              >
                支付宝
              </Button>
              <Button
                type="button"
                :variant="checkoutPaymentMethod === 'wechat' ? 'secondary' : 'outline'"
                @click="checkoutPaymentMethod = 'wechat'"
              >
                微信支付
              </Button>
              <Button
                type="button"
                :variant="checkoutPaymentMethod === 'manual_review' ? 'secondary' : 'outline'"
                @click="checkoutPaymentMethod = 'manual_review'"
              >
                人工充值
              </Button>
            </div>
          </div>

          <div class="rounded-2xl border border-border/60 bg-muted/20 p-4 text-xs leading-6 text-muted-foreground">
            {{
              checkoutMode === 'upgrade'
                ? '升级支付完成后会立即切换到新版本或新产品，当前不支持降级。'
                : checkoutMode === 'renewal'
                  ? '续订成功后会在当前套餐结束后顺延生效，不会损失剩余时长。'
                  : '支付完成后会自动开通订阅，并立即生效对应的额度与套餐能力。'
            }}
          </div>
        </div>

        <template #footer>
          <Button
            variant="ghost"
            :disabled="purchasingVariantId !== null"
            @click="handleCheckoutDialogUpdate(false)"
          >
            取消
          </Button>
          <Button
            :disabled="!checkoutTargetVariant || purchasingVariantId !== null"
            @click="submitCheckout"
          >
            {{
              purchasingVariantId
                ? '处理中...'
                : checkoutMode === 'upgrade'
                  ? '确认升级'
                  : checkoutMode === 'renewal'
                    ? '确认续订'
                    : '确认购买'
            }}
          </Button>
        </template>
      </Dialog>

      <Dialog
        :model-value="showModelsDialog"
        size="md"
        @update:model-value="handleModelsDialogUpdate"
      >
        <template #header>
          <div class="border-b border-border px-6 py-4">
            <div>
              <h3 class="text-lg font-semibold">
                {{ modelsTargetProduct?.name || '可用模型' }}
              </h3>
              <p class="text-xs text-muted-foreground">
                当前套餐可使用的统一模型名称
              </p>
            </div>
          </div>
        </template>

        <div class="space-y-4">
          <div
            v-if="!modelsTargetProduct?.available_model_names?.length"
            class="rounded-xl border border-dashed border-border/70 bg-muted/10 px-4 py-5 text-sm text-muted-foreground"
          >
            当前套餐暂未配置可用模型。
          </div>

          <div
            v-else
            class="flex flex-wrap gap-2"
          >
            <Badge
              v-for="modelName in modelsTargetProduct.available_model_names"
              :key="modelName"
              variant="outline"
              class="h-8 rounded-md px-3 text-xs"
            >
              {{ modelName }}
            </Badge>
          </div>
        </div>

        <template #footer>
          <Button
            variant="outline"
            @click="handleModelsDialogUpdate(false)"
          >
            关闭
          </Button>
        </template>
      </Dialog>
    </template>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { CreditCard } from 'lucide-vue-next'
import {
  Badge,
  Button,
  Card,
  Dialog,
  Input,
  Label,
  RefreshButton,
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
import { EmptyState, LoadingState } from '@/components/common'
import {
  subscriptionsApi,
  type SubscriptionDashboardResponse,
  type SubscriptionProduct,
  type SubscriptionOrder,
  type SubscriptionVariant,
  type UserSubscription,
} from '@/api/subscriptions'
import { walletApi, type WalletBalanceResponse } from '@/api/wallet'
import { useConfirm } from '@/composables/useConfirm'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'
import { log } from '@/utils/logger'
import {
  formatWalletCurrency as formatCurrency,
  paymentMethodLabel,
  paymentStatusBadge,
  paymentStatusLabel,
} from '@/utils/walletDisplay'

const { success, error: showError } = useToast()
const { confirm } = useConfirm()

const loadingInitial = ref(true)
const loadingDashboard = ref(false)
const loadingProducts = ref(false)
const loadingOrders = ref(false)
const loadingWallet = ref(false)
const purchasingVariantId = ref<string | null>(null)
const cancelingOrderId = ref<string | null>(null)

const activeTab = ref<'products' | 'orders'>('products')
const showCheckoutDialog = ref(false)
const showModelsDialog = ref(false)
const dashboard = ref<SubscriptionDashboardResponse | null>(null)
const products = ref<SubscriptionProduct[]>([])
const orders = ref<SubscriptionOrder[]>([])
const walletBalance = ref<WalletBalanceResponse | null>(null)
const selectedVariantByProduct = ref<Record<string, string>>({})
const selectedMonthsByVariant = ref<Record<string, number>>({})
const checkoutTargetProduct = ref<SubscriptionProduct | null>(null)
const checkoutTargetVariant = ref<SubscriptionVariant | null>(null)
const modelsTargetProduct = ref<SubscriptionProduct | null>(null)
const checkoutMonths = ref(1)
const checkoutPaymentMethod = ref<'alipay' | 'wechat' | 'manual_review'>('alipay')
const checkoutMode = ref<'purchase' | 'upgrade' | 'renewal'>('purchase')

const currentSubscription = computed<UserSubscription | null>(() => dashboard.value?.current_subscription ?? null)
const productMap = computed(() => new Map(products.value.map(product => [product.id, product])))
const variantMap = computed(() => {
  const map = new Map<string, SubscriptionVariant>()
  products.value.forEach((product) => {
    product.variants.forEach((variant) => {
      map.set(variant.id, variant)
    })
  })
  return map
})

const currentSubscriptionTitle = computed(() => {
  if (!currentSubscription.value) return '尚未开通订阅'
  if (currentSubscription.value.product_name && currentSubscription.value.variant_name) {
    return `${currentSubscription.value.product_name} · ${currentSubscription.value.variant_name}`
  }
  return currentSubscription.value.plan_name || '已开通订阅'
})

const currentSubscriptionSubtitle = computed(() => {
  if (!currentSubscription.value) {
    return '选择合适的套餐和版本后，即可按月获得对应额度。'
  }

  return `当前周期 ${formatDate(currentSubscription.value.current_cycle_start)} - ${formatDate(currentSubscription.value.current_cycle_end)}`
})

const currentSubscriptionOveragePolicy = computed<string | null>(() => {
  const subscription = currentSubscription.value
  if (!subscription?.product_id) return null
  return productMap.value.get(subscription.product_id)?.overage_policy ?? null
})

const latestPendingOrder = computed<SubscriptionOrder | null>(() => {
  return orders.value.find(order => ['pending', 'pending_approval'].includes(order.status)) ?? null
})

onMounted(async () => {
  try {
    await refreshAll()
  } finally {
    loadingInitial.value = false
  }
})

async function refreshAll() {
  await Promise.all([
    loadDashboard(),
    loadProducts(),
    loadOrders(),
    loadWalletBalance(),
  ])
}

async function loadDashboard() {
  loadingDashboard.value = true
  try {
    dashboard.value = await subscriptionsApi.getDashboard()
  } catch (error) {
    log.error('加载订阅概览失败:', error)
    showError(parseApiError(error, '加载订阅概览失败'))
  } finally {
    loadingDashboard.value = false
  }
}

async function loadProducts() {
  loadingProducts.value = true
  try {
    const response = await subscriptionsApi.listProducts()
    products.value = response.products
    response.products.forEach((product) => {
      const defaultVariant = sortedVariants(product.variants).find(variant => variant.is_default_variant) || sortedVariants(product.variants)[0]
      if (defaultVariant && !selectedVariantByProduct.value[product.id]) {
        selectedVariantByProduct.value[product.id] = defaultVariant.id
      }
      product.variants.forEach((variant) => {
        if (!selectedMonthsByVariant.value[variant.id]) {
          selectedMonthsByVariant.value[variant.id] = defaultMonthsForVariant(variant)
        }
      })
    })
  } catch (error) {
    log.error('加载订阅产品失败:', error)
    showError(parseApiError(error, '加载订阅产品失败'))
  } finally {
    loadingProducts.value = false
  }
}

async function loadOrders() {
  loadingOrders.value = true
  try {
    const response = await subscriptionsApi.listOrders({ limit: 20, offset: 0 })
    orders.value = response.items
  } catch (error) {
    log.error('加载订阅订单失败:', error)
    showError(parseApiError(error, '加载订阅订单失败'))
  } finally {
    loadingOrders.value = false
  }
}

async function loadWalletBalance() {
  loadingWallet.value = true
  try {
    walletBalance.value = await walletApi.getBalance()
  } catch (error) {
    log.error('加载钱包余额失败:', error)
  } finally {
    loadingWallet.value = false
  }
}

function sortedVariants(variants: SubscriptionVariant[]) {
  return [...variants].sort((a, b) => a.variant_rank - b.variant_rank)
}

function selectedVariant(product: SubscriptionProduct): SubscriptionVariant | null {
  const selectedId = selectedVariantByProduct.value[product.id]
  const variants = sortedVariants(product.variants)
  return variants.find(variant => variant.id === selectedId) || variants[0] || null
}

function selectVariant(productId: string, variantId: string) {
  selectedVariantByProduct.value[productId] = variantId
}

function sortedDiscounts(discounts: SubscriptionVariant['term_discounts_json']) {
  return [...discounts].sort((a, b) => a.months - b.months)
}

function defaultMonthsForVariant(variant: SubscriptionVariant): number {
  const sorted = sortedDiscounts(variant.term_discounts_json)
  const preferred = sorted.find(item => item.months === 12)
  return preferred?.months ?? sorted[0]?.months ?? 1
}

function selectedMonthsForVariant(variant: SubscriptionVariant): number {
  return selectedMonthsByVariant.value[variant.id] ?? defaultMonthsForVariant(variant)
}

function updateSelectedMonthsForVariant(variant: SubscriptionVariant, value: string | number) {
  const parsed = Number.parseInt(String(value), 10)
  selectedMonthsByVariant.value[variant.id] = Number.isFinite(parsed) && parsed > 0 ? parsed : 1
}

function applicableDiscount(variant: SubscriptionVariant, months = selectedMonthsForVariant(variant)) {
  const sorted = sortedDiscounts(variant.term_discounts_json)
  const safeMonths = Math.max(1, Number.parseInt(String(months), 10) || 1)
  let matched = sorted[0] ?? { months: 1, discount_factor: 1 }
  sorted.forEach((item) => {
    if (item.months <= safeMonths) {
      matched = item
    }
  })
  return matched
}

function appliedDiscountFactor(variant: SubscriptionVariant): number {
  return applicableDiscount(variant).discount_factor
}

function variantPrice(variant: SubscriptionVariant, months = selectedMonthsForVariant(variant)) {
  const selectedDiscount = applicableDiscount(variant, months)
  const original = variant.monthly_price_usd * months
  const total = original * selectedDiscount.discount_factor
  return {
    total,
    original: selectedDiscount.discount_factor < 1 ? original : null,
    factor: selectedDiscount.discount_factor,
  }
}

function transitionKindForVariant(variant: SubscriptionVariant): 'purchase' | 'renewal' | 'upgrade' | 'unavailable' {
  if (!currentSubscription.value || currentSubscription.value.status !== 'active') return 'purchase'
  const currentVariant = variantMap.value.get(currentSubscription.value.plan_id)
  if (!currentVariant) return 'unavailable'
  if (currentVariant.id === variant.id) return 'renewal'
  const currentProduct = productMap.value.get(currentVariant.product_id)
  const targetProduct = productMap.value.get(variant.product_id)
  if (!currentProduct || !targetProduct) return 'unavailable'
  if (currentProduct.id === targetProduct.id) {
    return variant.variant_rank > currentVariant.variant_rank ? 'upgrade' : 'unavailable'
  }
  return targetProduct.plan_level > currentProduct.plan_level ? 'upgrade' : 'unavailable'
}

function canSelectVariant(variant: SubscriptionVariant): boolean {
  if (purchasingVariantId.value !== null) return false
  if (latestPendingOrder.value) return false
  return transitionKindForVariant(variant) !== 'unavailable'
}

function selectedVariantStatusLabel(variant: SubscriptionVariant | null): string {
  if (!variant) return '暂不可购买'
  if (latestPendingOrder.value) {
    return '待支付'
  }
  const transitionKind = transitionKindForVariant(variant)
  if (transitionKind === 'renewal') return '可续期'
  if (transitionKind === 'purchase') return '可购买'
  if (transitionKind === 'upgrade') return '可升级'
  return '不可购买'
}

function selectedVariantButtonLabel(variant: SubscriptionVariant | null): string {
  if (!variant) return '暂不可购买'
  if (purchasingVariantId.value === variant.id) {
    return '处理中...'
  }
  if (latestPendingOrder.value) {
    return '查看待支付订单'
  }
  const transitionKind = transitionKindForVariant(variant)
  if (transitionKind === 'renewal') return '续订此套餐'
  if (transitionKind === 'purchase') return '立即购买'
  if (transitionKind === 'upgrade') return '升级此版本'
  return '暂不可购买'
}

function openCheckoutDialog(product: SubscriptionProduct) {
  const variant = selectedVariant(product)
  if (!variant || !canSelectVariant(variant)) {
    if (latestPendingOrder.value) {
      activeTab.value = 'orders'
    }
    return
  }

  checkoutTargetProduct.value = product
  checkoutTargetVariant.value = variant
  checkoutMonths.value = selectedMonthsForVariant(variant)
  checkoutPaymentMethod.value = 'alipay'
  checkoutMode.value = transitionKindForVariant(variant)
  showCheckoutDialog.value = true
}

function handleCheckoutDialogUpdate(value: boolean) {
  showCheckoutDialog.value = value
  if (!value) {
    checkoutTargetProduct.value = null
    checkoutTargetVariant.value = null
  }
}

function openModelsDialog(product: SubscriptionProduct) {
  modelsTargetProduct.value = product
  showModelsDialog.value = true
}

function handleModelsDialogUpdate(value: boolean) {
  showModelsDialog.value = value
  if (!value) {
    modelsTargetProduct.value = null
  }
}

async function submitCheckout() {
  const variant = checkoutTargetVariant.value
  if (!variant) return

  purchasingVariantId.value = variant.id
  try {
    if (currentSubscription.value?.status === 'active' && checkoutMode.value !== 'purchase') {
      const result = await subscriptionsApi.upgrade(currentSubscription.value.id, {
        new_plan_id: variant.id,
        purchased_months: checkoutMonths.value,
        payment_method: checkoutPaymentMethod.value,
      })
      if (result.order) {
        success(checkoutMode.value === 'renewal' ? '续订订单已创建，请完成支付' : '升级订单已创建，请完成支付')
        activeTab.value = 'orders'
      } else {
        success(checkoutMode.value === 'renewal' ? '续订已生效' : '升级已立即生效')
        activeTab.value = 'products'
      }
    } else {
      const result = await subscriptionsApi.purchase({
        plan_id: variant.id,
        purchased_months: checkoutMonths.value,
        payment_method: checkoutPaymentMethod.value,
      })
      if (result.order) {
        success('订阅订单已创建，请完成支付')
        activeTab.value = 'orders'
      } else {
        success('订阅已开通')
        activeTab.value = 'products'
      }
    }
    handleCheckoutDialogUpdate(false)
    await refreshAll()
  } catch (error) {
    log.error('创建订阅订单失败:', error)
    showError(parseApiError(error, '创建订阅订单失败'))
  } finally {
    purchasingVariantId.value = null
  }
}

async function cancelSubscriptionOrder(order: SubscriptionOrder) {
  const confirmed = await confirm({
    title: '取消订阅订单',
    message: `确认取消订单 ${compactOrderNo(order.order_no)} 吗？取消后需要重新创建订阅订单。`,
    confirmText: '确认取消',
    variant: 'warning',
  })
  if (!confirmed) return

  cancelingOrderId.value = order.id
  try {
    await subscriptionsApi.cancelOrder(order.id)
    success('订阅订单已取消')
    await refreshAll()
  } catch (error) {
    log.error('取消订阅订单失败:', error)
    showError(parseApiError(error, '取消订阅订单失败'))
  } finally {
    cancelingOrderId.value = null
  }
}

function quotaProgress(subscription: UserSubscription | null | undefined): number {
  if (!subscription || subscription.cycle_quota_usd <= 0) return 0
  const ratio = subscription.cycle_used_usd / subscription.cycle_quota_usd
  return Math.min(100, Math.max(0, Math.round(ratio * 100)))
}

function subscriptionStatusLabel(status?: string | null): string {
  switch (status) {
    case 'active':
      return '生效中'
    case 'pending_payment':
      return '待支付'
    case 'canceled':
      return '已取消'
    case 'expired':
      return '已过期'
    default:
      return '未开通'
  }
}

function overagePolicyLabel(policy?: string | null): string {
  return policy === 'use_wallet_balance' ? '扣钱包' : '拦截'
}

function subscriptionOrderTypeLabel(orderType?: string | null): string {
  if (orderType === 'subscription_upgrade') return '升级订阅'
  if (orderType === 'subscription_renewal') return '续订订阅'
  return '购买订阅'
}

function subscriptionOrderPlanLabel(order: SubscriptionOrder): string {
  if (order.product_name && order.variant_name) {
    return `${order.product_name} · ${order.variant_name}`
  }
  return order.plan_name || order.product_name || '-'
}

function pendingOrderScheduleLabel(order: SubscriptionOrder): string {
  if (order.status === 'pending_approval') return '等待管理员审核'
  if (order.expires_at) return `支付截止 ${formatDateTime(order.expires_at)}`
  return '等待支付回调确认'
}

function pendingOrderDateLabel(order: SubscriptionOrder): string {
  if (order.expires_at) return `截止 ${formatDateTime(order.expires_at)}`
  return `创建 ${formatDateTime(order.created_at)}`
}

function compactOrderNo(orderNo: string | null | undefined): string {
  if (!orderNo) return '-'
  if (orderNo.length <= 26) return orderNo
  return `${orderNo.slice(0, 18)}...${orderNo.slice(-8)}`
}

function canCancelSubscriptionOrder(order: SubscriptionOrder | null | undefined): boolean {
  if (!order) return false
  return order.status === 'pending' || order.status === 'pending_approval'
}

function subscriptionOrderActionText(order: SubscriptionOrder): string {
  if (order.status === 'pending_approval') return '审核中'
  if (order.status === 'credited') return '已完成'
  if (order.status === 'paid') return '支付中'
  if (order.status === 'expired') return '已过期'
  if (order.status === 'failed') return '支付失败'
  if (order.status === 'pending') {
    if (order.expires_at) return `截止 ${formatTimeLabel(order.expires_at)}`
    return '待支付'
  }
  return paymentStatusLabel(order.status)
}

function formatDiscount(factor: number): string {
  if (factor >= 1) return '原价'
  const discount = factor * 10
  return `${discount.toFixed(discount % 1 === 0 ? 0 : 1)} 折`
}

function paymentUrl(order: SubscriptionOrder | null | undefined): string {
  const value = order?.gateway_response?.payment_url
  return typeof value === 'string' ? value : ''
}

function formatDate(value: string | null | undefined): string {
  if (!value) return '-'
  return new Date(value).toLocaleDateString('zh-CN', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
  })
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
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return value
  return date.toLocaleDateString('zh-CN', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
  })
}

function formatTimeLabel(value: string | null | undefined): string {
  if (!value) return '-'
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return value
  return date.toLocaleTimeString('zh-CN', {
    hour: '2-digit',
    minute: '2-digit',
  })
}
</script>
