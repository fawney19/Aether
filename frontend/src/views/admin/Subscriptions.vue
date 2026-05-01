<template>
  <div class="space-y-6 pb-8">
    <Card class="overflow-hidden">
      <div class="border-b border-border/60 px-4 py-4 sm:px-5">
        <div class="flex flex-col gap-3 lg:flex-row lg:items-center lg:justify-between">
          <div>
            <h3 class="text-base font-semibold">
              订阅管理
            </h3>
            <p class="mt-1 text-xs text-muted-foreground">
              管理订阅产品、用户下单记录、人工充值订单与支付回调。
            </p>
          </div>
          <RefreshButton
            :loading="loadingProducts || loadingOrders || loadingCallbacks || loadingUserGroups"
            @click="refreshAll"
          />
        </div>
      </div>

      <div class="px-4 py-4 sm:px-5 sm:py-5">
        <Tabs v-model="activeTab">
          <TabsList class="tabs-button-list grid w-full max-w-[720px] grid-cols-2 gap-1 sm:grid-cols-4">
            <TabsTrigger
              value="products"
              class="text-xs sm:text-sm"
            >
              订阅产品
            </TabsTrigger>
            <TabsTrigger
              value="orders"
              class="text-xs sm:text-sm"
            >
              订阅订单
            </TabsTrigger>
            <TabsTrigger
              value="reviews"
              class="text-xs sm:text-sm"
            >
              订阅审批
            </TabsTrigger>
            <TabsTrigger
              value="callbacks"
              class="text-xs sm:text-sm"
            >
              回调日志
            </TabsTrigger>
          </TabsList>

          <TabsContent
            value="products"
            class="mt-5 space-y-4"
          >
            <div class="flex flex-col gap-3 lg:flex-row lg:items-center lg:justify-between">
              <div class="relative max-w-md flex-1">
                <Search class="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
                <Input
                  v-model="productSearch"
                  class="pl-9"
                  placeholder="搜索产品、版本、编码或用户分组..."
                />
              </div>

              <Button
                class="w-full sm:w-auto"
                @click="openCreateProductDialog"
              >
                <Plus class="mr-1.5 h-4 w-4" />
                新增产品
              </Button>
            </div>

            <div class="space-y-2.5 sm:hidden">
              <div
                v-for="product in filteredProducts"
                :key="product.id"
                class="rounded-2xl border border-border/60 bg-card/95 p-3.5 shadow-[0_16px_34px_-30px_hsl(var(--foreground))]"
              >
                <div class="flex items-start justify-between gap-3">
                  <div class="min-w-0">
                    <div class="truncate text-sm font-semibold text-foreground">
                      {{ product.name }}
                    </div>
                    <div class="mt-1 flex flex-wrap items-center gap-x-1.5 gap-y-0.5 text-[11px] text-muted-foreground">
                      <span class="font-mono">{{ product.code }}</span>
                      <span>Level {{ product.plan_level }}</span>
                    </div>
                  </div>
                  <Badge
                    variant="outline"
                    class="h-6 shrink-0 whitespace-nowrap px-2 py-0 text-[11px]"
                    :class="product.is_active ? 'text-emerald-600 dark:text-emerald-400' : 'text-muted-foreground'"
                  >
                    {{ product.is_active ? '启用中' : '已停用' }}
                  </Badge>
                </div>

                <div class="mt-2 flex flex-wrap gap-1.5">
                  <Badge
                    variant="outline"
                    class="h-5 whitespace-nowrap px-2 py-0 text-[10px]"
                  >
                    {{ overagePolicyLabel(product.overage_policy) }}
                  </Badge>
                  <Badge
                    variant="outline"
                    class="h-5 whitespace-nowrap px-2 py-0 text-[10px] text-muted-foreground"
                  >
                    {{ product.user_group_name || '未绑定分组' }}
                  </Badge>
                  <Badge
                    variant="outline"
                    class="h-5 whitespace-nowrap px-2 py-0 text-[10px] text-muted-foreground"
                  >
                    {{ product.active_subscription_count }} 订阅
                  </Badge>
                </div>

                <div
                  v-if="product.description"
                  class="mt-2 text-[11px] leading-5 text-muted-foreground"
                >
                  {{ product.description }}
                </div>

                <div class="mt-3 rounded-xl border border-border/50 bg-muted/[0.08] p-2.5">
                  <div class="text-[11px] text-muted-foreground">
                    版本
                  </div>
                  <div class="mt-2 flex flex-wrap gap-1">
                    <div
                      v-for="variant in sortedVariants(product.variants)"
                      :key="variant.id"
                      class="inline-flex max-w-full items-center gap-1 rounded-full border border-border/50 bg-background px-2.5 py-0.5 text-[11px] leading-5"
                    >
                      <span
                        class="truncate font-medium text-foreground"
                        :title="variant.name"
                      >{{ variant.name }}</span>
                      <span class="shrink-0 text-muted-foreground">·</span>
                      <span class="shrink-0 whitespace-nowrap text-muted-foreground">
                        {{ formatCurrency(variant.monthly_price_usd) }}
                      </span>
                      <span class="shrink-0 text-muted-foreground">·</span>
                      <span class="shrink-0 whitespace-nowrap text-muted-foreground">
                        {{ formatCurrency(variant.monthly_quota_usd) }}额度
                      </span>
                      <Badge
                        v-if="variant.is_default_variant"
                        variant="outline"
                        class="h-4 shrink-0 whitespace-nowrap px-1.5 py-0 text-[9px]"
                      >
                        默认
                      </Badge>
                      <Badge
                        v-if="!variant.is_active"
                        variant="outline"
                        class="h-4 shrink-0 whitespace-nowrap px-1.5 py-0 text-[9px] text-muted-foreground"
                      >
                        停用
                      </Badge>
                    </div>
                  </div>
                </div>

                <div class="mt-3 flex gap-2">
                  <Button
                    variant="outline"
                    size="sm"
                    class="h-8 flex-1 text-xs"
                    @click="openEditProductDialog(product)"
                  >
                    编辑
                  </Button>
                  <Button
                    variant="outline"
                    size="sm"
                    class="h-8 flex-1 border-rose-200 text-xs text-rose-600 hover:bg-rose-50 dark:border-rose-900/60 dark:hover:bg-rose-950/40"
                    :disabled="product.active_subscription_count > 0"
                    @click="removeProduct(product)"
                  >
                    删除
                  </Button>
                </div>
              </div>

              <EmptyState
                v-if="!loadingProducts && filteredProducts.length === 0"
                :type="productSearch ? 'search' : 'empty'"
                size="sm"
                :title="productSearch ? '没有匹配的订阅产品' : '还没有订阅产品'"
                :description="productSearch ? '换个关键词再试试。' : '创建产品后会显示在这里。'"
              />
            </div>

            <div class="hidden overflow-hidden rounded-2xl border border-border/60 bg-background sm:block">
              <div class="overflow-x-auto">
                <Table class="w-full table-auto">
                  <TableHeader>
                    <TableRow>
                      <TableHead class="h-10 w-[28%] px-3 py-2 whitespace-nowrap">
                        产品 / 分组
                      </TableHead>
                      <TableHead class="h-10 px-2.5 py-2 whitespace-nowrap">
                        版本
                      </TableHead>
                      <TableHead class="h-10 px-2 py-2 whitespace-nowrap text-center">
                        超额策略
                      </TableHead>
                      <TableHead class="h-10 px-2 py-2 whitespace-nowrap text-center">
                        状态
                      </TableHead>
                      <TableHead class="h-10 px-2 py-2 whitespace-nowrap text-center">
                        活跃订阅
                      </TableHead>
                      <TableHead class="h-10 px-3 py-2 whitespace-nowrap text-right">
                        操作
                      </TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    <TableRow
                      v-for="product in filteredProducts"
                      :key="product.id"
                      class="border-b border-border/40 transition-colors hover:bg-muted/10 last:border-b-0"
                    >
                      <TableCell class="px-3 py-2.5 align-top">
                        <div class="truncate text-sm font-medium text-foreground">
                          {{ product.name }}
                        </div>
                        <div class="mt-0.5 flex flex-wrap items-center gap-x-1.5 gap-y-0.5 text-[11px] text-muted-foreground">
                          <span class="font-mono">{{ product.code }}</span>
                          <span>Level {{ product.plan_level }}</span>
                          <span>{{ product.user_group_name || '未绑定分组' }}</span>
                          <span>{{ product.variant_count }} 个版本</span>
                        </div>
                        <div
                          v-if="product.description"
                          class="mt-0.5 line-clamp-1 text-[11px] text-muted-foreground"
                        >
                          {{ product.description }}
                        </div>
                      </TableCell>
                      <TableCell class="px-2.5 py-2.5 align-top">
                        <div class="flex flex-wrap gap-1">
                          <div
                            v-for="variant in sortedVariants(product.variants)"
                            :key="variant.id"
                            class="inline-flex max-w-full items-center gap-1 rounded-full border border-border/50 bg-muted/[0.08] px-2.5 py-0.5 text-[11px] leading-5"
                          >
                            <span
                              class="truncate font-medium text-foreground"
                              :title="variant.name"
                            >{{ variant.name }}</span>
                            <span class="shrink-0 text-muted-foreground">·</span>
                            <span class="shrink-0 whitespace-nowrap text-muted-foreground">
                              {{ formatCurrency(variant.monthly_price_usd) }}
                            </span>
                            <span class="shrink-0 text-muted-foreground">·</span>
                            <span class="shrink-0 whitespace-nowrap text-muted-foreground">
                              {{ formatCurrency(variant.monthly_quota_usd) }}额度
                            </span>
                            <Badge
                              v-if="variant.is_default_variant"
                              variant="outline"
                              class="h-4 shrink-0 whitespace-nowrap px-1.5 py-0 text-[9px]"
                            >
                              默认
                            </Badge>
                            <Badge
                              v-if="!variant.is_active"
                              variant="outline"
                              class="h-4 shrink-0 whitespace-nowrap px-1.5 py-0 text-[9px] text-muted-foreground"
                            >
                              停用
                            </Badge>
                          </div>
                        </div>
                      </TableCell>
                      <TableCell class="px-2 py-2.5 align-top text-center">
                        <Badge
                          variant="outline"
                          class="h-6 whitespace-nowrap px-2 py-0 text-[11px]"
                        >
                          {{ overagePolicyLabel(product.overage_policy) }}
                        </Badge>
                      </TableCell>
                      <TableCell class="px-2 py-2.5 align-top text-center">
                        <Badge
                          variant="outline"
                          class="h-6 whitespace-nowrap px-2 py-0 text-[11px]"
                          :class="product.is_active ? 'text-emerald-600 dark:text-emerald-400' : 'text-muted-foreground'"
                        >
                          {{ product.is_active ? '启用中' : '已停用' }}
                        </Badge>
                      </TableCell>
                      <TableCell class="px-2 py-2.5 align-top text-center text-[13px] font-medium whitespace-nowrap">
                        {{ product.active_subscription_count }}
                      </TableCell>
                      <TableCell class="px-3 py-2.5 align-top text-right">
                        <div class="flex justify-end gap-1 whitespace-nowrap">
                          <Button
                            variant="outline"
                            size="sm"
                            class="h-7 min-w-[50px] whitespace-nowrap px-2 text-xs"
                            @click="openEditProductDialog(product)"
                          >
                            编辑
                          </Button>
                          <Button
                            variant="outline"
                            size="sm"
                            class="h-7 min-w-[50px] whitespace-nowrap border-rose-200 px-2 text-xs text-rose-600 hover:bg-rose-50 dark:border-rose-900/60 dark:hover:bg-rose-950/40"
                            :disabled="product.active_subscription_count > 0"
                            @click="removeProduct(product)"
                          >
                            删除
                          </Button>
                        </div>
                      </TableCell>
                    </TableRow>
                    <TableRow v-if="!loadingProducts && filteredProducts.length === 0">
                      <TableCell
                        colspan="6"
                        class="py-12 text-center text-sm text-muted-foreground"
                      >
                        {{ productSearch ? '没有匹配的订阅产品' : '还没有订阅产品' }}
                      </TableCell>
                    </TableRow>
                  </TableBody>
                </Table>
              </div>
            </div>
          </TabsContent>

          <TabsContent
            value="orders"
            class="mt-5 space-y-4"
          >
            <div class="flex flex-col gap-3 xl:flex-row xl:items-center xl:justify-between">
              <div class="flex flex-1 flex-col gap-3 lg:flex-row">
                <div class="relative max-w-md flex-1">
                  <Search class="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
                  <Input
                    v-model="orderSearch"
                    class="pl-9"
                    placeholder="搜索订单号、用户、产品或版本..."
                  />
                </div>

                <Select v-model="orderStatusFilter">
                  <SelectTrigger class="w-full lg:w-[160px]">
                    <SelectValue placeholder="全部状态" />
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
                    <SelectItem value="credited">
                      已到账
                    </SelectItem>
                    <SelectItem value="failed">
                      已拒绝
                    </SelectItem>
                    <SelectItem value="expired">
                      已过期
                    </SelectItem>
                  </SelectContent>
                </Select>

                <Select v-model="orderPaymentMethodFilter">
                  <SelectTrigger class="w-full lg:w-[160px]">
                    <SelectValue placeholder="全部方式" />
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
                  </SelectContent>
                </Select>
              </div>
            </div>

            <div class="flex items-center justify-between px-1 sm:px-0">
              <div class="text-sm text-muted-foreground">
                共 {{ filteredOrders.length }} 条
              </div>
              <RefreshButton
                :loading="loadingOrders"
                @click="loadOrders"
              />
            </div>

            <div class="space-y-2.5 sm:hidden">
              <div
                v-for="order in filteredOrders"
                :key="order.id"
                class="rounded-2xl border border-border/60 bg-card/95 p-3.5 shadow-[0_16px_34px_-30px_hsl(var(--foreground))]"
              >
                <div class="flex items-start justify-between gap-3">
                  <div class="min-w-0">
                    <div class="flex flex-wrap items-center gap-1.5">
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
                      class="mt-2 truncate text-sm font-semibold text-foreground"
                      :title="subscriptionOrderPlanLabel(order)"
                    >
                      {{ subscriptionOrderPlanLabel(order) }}
                    </div>
                    <div
                      class="mt-1 truncate font-mono text-[11px] text-muted-foreground"
                      :title="order.order_no"
                    >
                      {{ order.order_no }}
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
                      用户
                    </div>
                    <div
                      class="mt-0.5 truncate font-medium text-foreground"
                      :title="order.username || '-'"
                    >
                      {{ order.username || '-' }}
                    </div>
                    <div
                      class="mt-1 truncate text-[11px] text-muted-foreground"
                      :title="order.email || order.user_id || '-'"
                    >
                      {{ order.email || order.user_id || '-' }}
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

                <div class="mt-2.5 rounded-xl border border-border/40 bg-background/85 px-3 py-2 text-[11px] leading-5 text-muted-foreground">
                  {{ subscriptionOrderTypeLabel(order.order_type) }} · {{ order.purchased_months || 0 }} 个月
                </div>
              </div>

              <EmptyState
                v-if="!loadingOrders && filteredOrders.length === 0"
                :type="orderSearch || orderStatusFilter !== 'all' || orderPaymentMethodFilter !== 'all' ? 'filter' : 'empty'"
                size="sm"
                :title="orderSearch || orderStatusFilter !== 'all' || orderPaymentMethodFilter !== 'all' ? '没有匹配的订阅订单' : '还没有订阅订单'"
                :description="orderSearch || orderStatusFilter !== 'all' || orderPaymentMethodFilter !== 'all' ? '调整搜索词或筛选条件后再试试。' : '用户下单后会显示在这里。'"
              />
            </div>

            <div class="hidden overflow-hidden rounded-2xl border border-border/60 bg-background sm:block">
              <div class="overflow-x-auto">
                <Table class="min-w-[1030px] table-fixed">
                  <TableHeader>
                    <TableRow>
                      <TableHead class="w-[250px] whitespace-nowrap">
                        订单
                      </TableHead>
                      <TableHead class="w-[170px] whitespace-nowrap">
                        用户
                      </TableHead>
                      <TableHead class="w-[170px] whitespace-nowrap">
                        套餐
                      </TableHead>
                      <TableHead class="w-[96px] whitespace-nowrap">
                        金额
                      </TableHead>
                      <TableHead class="w-[110px] whitespace-nowrap text-center">
                        支付方式
                      </TableHead>
                      <TableHead class="w-[100px] whitespace-nowrap text-center">
                        状态
                      </TableHead>
                      <TableHead class="w-[134px] whitespace-nowrap">
                        创建时间
                      </TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    <TableRow
                      v-for="order in filteredOrders"
                      :key="order.id"
                      class="border-b border-border/40 last:border-b-0"
                    >
                      <TableCell class="py-5 align-top">
                        <div
                          class="max-w-full truncate font-medium"
                          :title="order.order_no"
                        >
                          {{ order.order_no }}
                        </div>
                        <div class="mt-1 truncate text-xs text-muted-foreground">
                          {{ subscriptionOrderTypeLabel(order.order_type) }}
                        </div>
                      </TableCell>
                      <TableCell class="py-5 align-top">
                        <div
                          class="max-w-full truncate font-medium"
                          :title="order.username || '-'"
                        >
                          {{ order.username || '-' }}
                        </div>
                        <div
                          class="mt-1 truncate text-xs text-muted-foreground"
                          :title="order.email || order.user_id || '-'"
                        >
                          {{ order.email || order.user_id || '-' }}
                        </div>
                      </TableCell>
                      <TableCell class="py-5 align-top">
                        <div
                          class="max-w-full truncate font-medium"
                          :title="subscriptionOrderPlanLabel(order)"
                        >
                          {{ subscriptionOrderPlanLabel(order) }}
                        </div>
                        <div class="mt-1 truncate text-xs text-muted-foreground">
                          {{ order.purchased_months || 0 }} 个月
                        </div>
                      </TableCell>
                      <TableCell class="py-5 align-top whitespace-nowrap text-sm font-medium">
                        {{ formatCurrency(order.amount_usd) }}
                      </TableCell>
                      <TableCell class="py-5 align-top text-center">
                        <Badge
                          variant="outline"
                          class="h-8 whitespace-nowrap px-3 py-0"
                        >
                          {{ paymentMethodLabel(order.payment_method) }}
                        </Badge>
                      </TableCell>
                      <TableCell class="py-5 align-top text-center">
                        <Badge
                          :variant="paymentStatusBadge(order.status)"
                          class="h-8 whitespace-nowrap px-3 py-0"
                        >
                          {{ paymentStatusLabel(order.status) }}
                        </Badge>
                      </TableCell>
                      <TableCell class="py-5 align-top text-sm text-muted-foreground">
                        <div class="whitespace-nowrap">
                          {{ formatDateLabel(order.created_at) }}
                        </div>
                        <div class="mt-1 whitespace-nowrap text-xs text-muted-foreground">
                          {{ formatTimeLabel(order.created_at) }}
                        </div>
                      </TableCell>
                    </TableRow>
                    <TableRow v-if="!loadingOrders && filteredOrders.length === 0">
                      <TableCell
                        colspan="7"
                        class="py-12 text-center text-sm text-muted-foreground"
                      >
                        {{ orderSearch || orderStatusFilter !== 'all' || orderPaymentMethodFilter !== 'all'
                          ? '没有匹配的订阅订单'
                          : '还没有订阅订单'
                        }}
                      </TableCell>
                    </TableRow>
                  </TableBody>
                </Table>
              </div>
            </div>
          </TabsContent>

          <TabsContent
            value="reviews"
            class="mt-5 space-y-4"
          >
            <div class="flex flex-col gap-3 lg:flex-row lg:items-center lg:justify-between">
              <div>
                <div class="text-sm font-medium">
                  待审核订单
                </div>
                <p class="mt-1 text-xs text-muted-foreground">
                  审核用户提交的人工充值订阅订单，审批通过后自动生效订阅。
                </p>
              </div>

              <div class="relative max-w-md flex-1 lg:max-w-sm">
                <Search class="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
                <Input
                  v-model="reviewSearch"
                  class="pl-9"
                  placeholder="搜索订单号、用户、产品或版本..."
                />
              </div>
            </div>

            <div class="flex items-center justify-between px-1 sm:px-0">
              <div class="text-sm text-muted-foreground">
                共 {{ reviewOrders.length }} 条
              </div>
              <RefreshButton
                :loading="loadingOrders"
                @click="loadOrders"
              />
            </div>

            <div class="space-y-2.5 sm:hidden">
              <div
                v-for="order in reviewOrders"
                :key="order.id"
                class="rounded-2xl border border-border/60 bg-card/95 p-3.5 shadow-[0_16px_34px_-30px_hsl(var(--foreground))]"
              >
                <div class="flex items-start justify-between gap-3">
                  <div class="min-w-0">
                    <div class="flex flex-wrap items-center gap-1.5">
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
                      class="mt-2 truncate text-sm font-semibold text-foreground"
                      :title="subscriptionOrderPlanLabel(order)"
                    >
                      {{ subscriptionOrderPlanLabel(order) }}
                    </div>
                    <div
                      class="mt-1 truncate font-mono text-[11px] text-muted-foreground"
                      :title="order.order_no"
                    >
                      {{ order.order_no }}
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
                      用户
                    </div>
                    <div
                      class="mt-0.5 truncate font-medium text-foreground"
                      :title="order.username || '-'"
                    >
                      {{ order.username || '-' }}
                    </div>
                    <div
                      class="mt-1 truncate text-[11px] text-muted-foreground"
                      :title="order.email || order.user_id || '-'"
                    >
                      {{ order.email || order.user_id || '-' }}
                    </div>
                  </div>
                  <div class="rounded-xl border border-border/40 bg-muted/18 p-2.5">
                    <div class="text-muted-foreground">
                      提交时间
                    </div>
                    <div class="mt-0.5 font-medium text-foreground">
                      {{ formatDateLabel(order.created_at) }}
                    </div>
                    <div class="mt-1 text-[11px] text-muted-foreground">
                      {{ formatTimeLabel(order.created_at) }}
                    </div>
                  </div>
                </div>

                <div class="mt-2.5 flex gap-2">
                  <Button
                    variant="outline"
                    size="sm"
                    class="h-8 flex-1 text-xs"
                    :disabled="reviewingOrderId === order.id"
                    @click="approveOrder(order)"
                  >
                    <CheckCircle2 class="mr-1.5 h-3.5 w-3.5" />
                    {{ reviewingOrderId === order.id ? '处理中...' : '通过' }}
                  </Button>
                  <Button
                    variant="outline"
                    size="sm"
                    class="h-8 flex-1 border-rose-200 text-xs text-rose-600 hover:bg-rose-50 dark:border-rose-900/60 dark:hover:bg-rose-950/40"
                    :disabled="reviewingOrderId === order.id"
                    @click="rejectOrder(order)"
                  >
                    <XCircle class="mr-1.5 h-3.5 w-3.5" />
                    {{ reviewingOrderId === order.id ? '处理中...' : '拒绝' }}
                  </Button>
                </div>
              </div>

              <EmptyState
                v-if="!loadingOrders && reviewOrders.length === 0"
                :type="reviewSearch ? 'search' : 'empty'"
                size="sm"
                :title="reviewSearch ? '没有匹配的待审核订单' : '当前没有待审核订阅订单'"
                :description="reviewSearch ? '换个关键词再试试。' : '人工充值待审核订单会显示在这里。'"
              />
            </div>

            <div class="hidden overflow-hidden rounded-2xl border border-border/60 bg-background sm:block">
              <div class="overflow-x-auto">
                <Table class="min-w-[980px] table-fixed">
                  <TableHeader>
                    <TableRow>
                      <TableHead class="w-[250px] whitespace-nowrap">
                        订单
                      </TableHead>
                      <TableHead class="w-[170px] whitespace-nowrap">
                        用户
                      </TableHead>
                      <TableHead class="w-[170px] whitespace-nowrap">
                        套餐
                      </TableHead>
                      <TableHead class="w-[96px] whitespace-nowrap">
                        金额
                      </TableHead>
                      <TableHead class="w-[128px] whitespace-nowrap">
                        提交时间
                      </TableHead>
                      <TableHead class="w-[166px] whitespace-nowrap text-right">
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
                      <TableCell class="py-5 align-top">
                        <div
                          class="max-w-full truncate font-medium"
                          :title="order.order_no"
                        >
                          {{ order.order_no }}
                        </div>
                        <div class="mt-1 truncate text-xs text-muted-foreground">
                          {{ subscriptionOrderTypeLabel(order.order_type) }}
                        </div>
                      </TableCell>
                      <TableCell class="py-5 align-top">
                        <div
                          class="max-w-full truncate font-medium"
                          :title="order.username || '-'"
                        >
                          {{ order.username || '-' }}
                        </div>
                        <div
                          class="mt-1 truncate text-xs text-muted-foreground"
                          :title="order.email || order.user_id || '-'"
                        >
                          {{ order.email || order.user_id || '-' }}
                        </div>
                      </TableCell>
                      <TableCell class="py-5 align-top">
                        <div
                          class="max-w-full truncate font-medium"
                          :title="subscriptionOrderPlanLabel(order)"
                        >
                          {{ subscriptionOrderPlanLabel(order) }}
                        </div>
                        <div class="mt-1 truncate text-xs text-muted-foreground">
                          {{ order.purchased_months || 0 }} 个月
                        </div>
                      </TableCell>
                      <TableCell class="py-5 align-top whitespace-nowrap text-sm font-medium">
                        {{ formatCurrency(order.amount_usd) }}
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
                          <Button
                            variant="outline"
                            size="sm"
                            class="h-8 min-w-[66px] whitespace-nowrap px-3"
                            :disabled="reviewingOrderId === order.id"
                            @click="approveOrder(order)"
                          >
                            <CheckCircle2 class="mr-1.5 h-4 w-4" />
                            通过
                          </Button>
                          <Button
                            variant="outline"
                            size="sm"
                            class="h-8 min-w-[66px] whitespace-nowrap border-rose-200 px-3 text-rose-600 hover:bg-rose-50 dark:border-rose-900/60 dark:hover:bg-rose-950/40"
                            :disabled="reviewingOrderId === order.id"
                            @click="rejectOrder(order)"
                          >
                            <XCircle class="mr-1.5 h-4 w-4" />
                            拒绝
                          </Button>
                        </div>
                      </TableCell>
                    </TableRow>
                    <TableRow v-if="!loadingOrders && reviewOrders.length === 0">
                      <TableCell
                        colspan="6"
                        class="py-12 text-center text-sm text-muted-foreground"
                      >
                        {{ reviewSearch ? '没有匹配的待审核订单' : '当前没有待审核订阅订单' }}
                      </TableCell>
                    </TableRow>
                  </TableBody>
                </Table>
              </div>
            </div>
          </TabsContent>

          <TabsContent
            value="callbacks"
            class="mt-5 space-y-4"
          >
            <div class="flex flex-col gap-3 lg:flex-row lg:items-center lg:justify-between">
              <div class="flex flex-wrap items-center gap-2">
                <Select
                  v-model="callbackMethodFilter"
                  @update:model-value="handleCallbackMethodChange"
                >
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
                      class="truncate font-mono text-[11px] text-foreground"
                      :title="callback.callback_key"
                    >
                      {{ callback.callback_key }}
                    </div>
                    <div
                      class="mt-1 truncate text-[11px] text-muted-foreground"
                      :title="callback.gateway_order_id || callback.id"
                    >
                      {{ callback.gateway_order_id || callback.id }}
                    </div>
                  </div>
                  <Badge
                    variant="outline"
                    class="h-6 shrink-0 whitespace-nowrap px-2.5 py-0 text-[11px]"
                  >
                    {{ paymentMethodLabel(callback.payment_method) }}
                  </Badge>
                </div>

                <div class="mt-2.5 flex flex-wrap gap-1.5">
                  <Badge :variant="callback.signature_valid ? 'success' : 'destructive'">
                    {{ callback.signature_valid ? '验签通过' : '验签失败' }}
                  </Badge>
                  <Badge :variant="callbackStatusBadge(callback.status)">
                    {{ callbackStatusLabel(callback.status) }}
                  </Badge>
                </div>

                <div class="mt-3 grid grid-cols-2 gap-2 text-xs">
                  <div class="rounded-xl border border-border/40 bg-muted/18 p-2.5">
                    <div class="text-muted-foreground">
                      订单
                    </div>
                    <div
                      class="mt-0.5 truncate font-medium text-foreground"
                      :title="callback.order_no || '-'"
                    >
                      {{ callback.order_no || '-' }}
                    </div>
                    <div
                      class="mt-1 truncate text-[11px] text-muted-foreground"
                      :title="callback.payment_order_id || '未关联支付单'"
                    >
                      {{ callback.payment_order_id || '未关联支付单' }}
                    </div>
                  </div>
                  <div class="rounded-xl border border-border/40 bg-muted/18 p-2.5">
                    <div class="text-muted-foreground">
                      时间
                    </div>
                    <div class="mt-0.5 font-medium text-foreground">
                      {{ formatDateTime(callback.created_at) }}
                    </div>
                    <div class="mt-1 text-[11px] text-muted-foreground">
                      {{ callback.processed_at ? `处理于 ${formatDateTime(callback.processed_at)}` : '等待处理' }}
                    </div>
                  </div>
                </div>
              </div>

              <EmptyState
                v-if="!loadingCallbacks && callbacks.length === 0"
                :type="callbackMethodFilter !== 'all' ? 'filter' : 'empty'"
                size="sm"
                :title="callbackMethodFilter !== 'all' ? '没有匹配的订阅回调日志' : '当前没有订阅回调日志'"
                :description="callbackMethodFilter !== 'all' ? '切换支付方式筛选后再试试。' : '支付回调到达后会显示在这里。'"
              />
            </div>

            <div class="hidden overflow-hidden rounded-2xl border border-border/60 bg-background sm:block">
              <div class="overflow-x-auto">
                <Table class="min-w-[980px] table-fixed">
                  <TableHeader>
                    <TableRow>
                      <TableHead class="w-[280px] whitespace-nowrap">
                        回调记录
                      </TableHead>
                      <TableHead class="w-[220px] whitespace-nowrap">
                        订单
                      </TableHead>
                      <TableHead class="w-[110px] whitespace-nowrap">
                        支付方式
                      </TableHead>
                      <TableHead class="w-[96px] whitespace-nowrap text-center">
                        验签
                      </TableHead>
                      <TableHead class="w-[96px] whitespace-nowrap text-center">
                        状态
                      </TableHead>
                      <TableHead class="w-[178px] whitespace-nowrap">
                        时间
                      </TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    <TableRow
                      v-for="callback in callbacks"
                      :key="callback.id"
                      class="border-b border-border/40 last:border-b-0"
                    >
                      <TableCell class="py-5 align-top">
                        <div
                          class="max-w-full truncate font-mono text-xs"
                          :title="callback.callback_key"
                        >
                          {{ callback.callback_key }}
                        </div>
                        <div
                          class="mt-1 truncate text-xs text-muted-foreground"
                          :title="callback.gateway_order_id || callback.id"
                        >
                          {{ callback.gateway_order_id || callback.id }}
                        </div>
                      </TableCell>
                      <TableCell class="py-5 align-top">
                        <div
                          class="max-w-full truncate font-medium"
                          :title="callback.order_no || '-'"
                        >
                          {{ callback.order_no || '-' }}
                        </div>
                        <div
                          class="mt-1 truncate text-xs text-muted-foreground"
                          :title="callback.payment_order_id || '未关联支付单'"
                        >
                          {{ callback.payment_order_id || '未关联支付单' }}
                        </div>
                      </TableCell>
                      <TableCell class="py-5 align-top">
                        <Badge
                          variant="outline"
                          class="h-8 whitespace-nowrap px-3 py-0"
                        >
                          {{ paymentMethodLabel(callback.payment_method) }}
                        </Badge>
                      </TableCell>
                      <TableCell class="py-5 align-top text-center">
                        <Badge :variant="callback.signature_valid ? 'success' : 'destructive'">
                          {{ callback.signature_valid ? '通过' : '失败' }}
                        </Badge>
                      </TableCell>
                      <TableCell class="py-5 align-top text-center">
                        <Badge :variant="callbackStatusBadge(callback.status)">
                          {{ callbackStatusLabel(callback.status) }}
                        </Badge>
                      </TableCell>
                      <TableCell class="py-5 align-top text-sm text-muted-foreground">
                        <div class="whitespace-nowrap">
                          {{ formatDateTime(callback.created_at) }}
                        </div>
                        <div class="mt-1 whitespace-nowrap text-xs text-muted-foreground">
                          {{ callback.processed_at ? `处理于 ${formatDateTime(callback.processed_at)}` : '等待处理' }}
                        </div>
                      </TableCell>
                    </TableRow>
                    <TableRow v-if="!loadingCallbacks && callbacks.length === 0">
                      <TableCell
                        colspan="6"
                        class="py-12 text-center text-sm text-muted-foreground"
                      >
                        {{ callbackMethodFilter !== 'all' ? '没有匹配的订阅回调日志' : '当前没有订阅回调日志' }}
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

    <Dialog
      :model-value="showProductDialog"
      size="4xl"
      @update:model-value="handleProductDialogUpdate"
    >
      <template #header>
        <div class="border-b border-border px-4 py-4 sm:px-6">
          <div class="flex items-center gap-3">
            <div class="flex h-9 w-9 items-center justify-center rounded-lg bg-primary/10">
              <BadgePercent class="h-5 w-5 text-primary" />
            </div>
            <div>
              <h3 class="text-lg font-semibold">
                {{ editingProduct ? '编辑订阅产品' : '新增订阅产品' }}
              </h3>
              <p class="text-xs text-muted-foreground">
                产品决定权限档位、超额策略和统一折扣，版本决定价格与额度。
              </p>
            </div>
          </div>
        </div>
      </template>

      <form @submit.prevent="submitProduct">
        <div class="max-h-[74vh] overflow-y-auto px-4 py-4 sm:px-6 sm:py-6">
          <div class="grid gap-6 xl:grid-cols-[minmax(0,0.9fr)_minmax(0,1.1fr)] xl:gap-0">
            <div class="space-y-5 xl:pr-6">
              <div class="flex min-h-9 items-center justify-between gap-3 border-b border-border/60 pb-2">
                <div class="flex h-9 items-center">
                  <span class="text-sm font-medium">产品配置</span>
                </div>
                <div class="flex h-9 items-center gap-2 text-sm">
                  <span class="font-medium">产品状态</span>
                  <Badge
                    variant="outline"
                    class="h-6 px-2 text-[11px] text-muted-foreground"
                  >
                    {{ productForm.is_active ? '启用中' : '已停用' }}
                  </Badge>
                  <Switch v-model="productForm.is_active" />
                </div>
              </div>

              <div class="grid gap-3 sm:grid-cols-2">
                <div class="space-y-2">
                  <Label>产品编码</Label>
                  <Input
                    v-model="productForm.code"
                    placeholder="如 claude-max"
                  />
                </div>
                <div class="space-y-2">
                  <Label>产品名称</Label>
                  <Input
                    v-model="productForm.name"
                    placeholder="如 Claude Max"
                  />
                </div>
              </div>

              <div class="grid gap-3 lg:grid-cols-[minmax(0,1.25fr)_minmax(0,0.8fr)_minmax(0,0.9fr)]">
                <div class="space-y-2">
                  <Label>绑定用户组</Label>
                  <Select v-model="productForm.user_group_id">
                    <SelectTrigger class="h-10">
                      <SelectValue placeholder="选择用户组" />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem
                        v-for="group in userGroups"
                        :key="group.id"
                        :value="group.id"
                      >
                        {{ group.name }}
                      </SelectItem>
                    </SelectContent>
                  </Select>
                </div>
                <div class="space-y-2">
                  <Label>权限等级</Label>
                  <Input
                    :model-value="String(productForm.plan_level)"
                    type="number"
                    min="0"
                    @update:model-value="(value) => productForm.plan_level = parseIntValue(value, 0)"
                  />
                </div>
                <div class="space-y-2">
                  <Label>超额策略</Label>
                  <Select v-model="productForm.overage_policy">
                    <SelectTrigger class="h-10">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="block">
                        拦截
                      </SelectItem>
                      <SelectItem value="use_wallet_balance">
                        扣钱包
                      </SelectItem>
                    </SelectContent>
                  </Select>
                </div>
              </div>

              <div class="space-y-2">
                <Label>描述</Label>
                <Textarea
                  v-model="productForm.description"
                  rows="4"
                  class="min-h-[112px] resize-none"
                  placeholder="可选，简要描述这个订阅产品的定位"
                />
              </div>

              <div class="rounded-xl border border-border/60 bg-muted/10 p-3.5">
                <div class="mb-3 flex items-center justify-between gap-3">
                  <span class="text-sm font-medium">购买时长折扣</span>
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    class="h-8 px-2.5 text-xs"
                    @click="addDiscountRow"
                  >
                    <Plus class="mr-1.5 h-3.5 w-3.5" />
                    添加
                  </Button>
                </div>

                <div class="grid grid-cols-[minmax(0,1fr)_minmax(0,1fr)_40px] gap-2 px-1 text-[11px] text-muted-foreground">
                  <div>满几个月</div>
                  <div>折扣系数</div>
                  <div class="text-center">
                    操作
                  </div>
                </div>

                <div class="mt-2 space-y-2">
                  <div
                    v-for="(row, discountIndex) in productForm.term_discounts_json"
                    :key="row.key"
                    class="grid grid-cols-[minmax(0,1fr)_minmax(0,1fr)_40px] items-center gap-2 rounded-lg border border-border/60 bg-background p-2"
                  >
                    <Input
                      :model-value="String(row.months)"
                      type="number"
                      min="1"
                      @update:model-value="(value) => row.months = parseIntValue(value, 1)"
                    />
                    <Input
                      :model-value="String(row.discount_factor)"
                      type="number"
                      min="0.01"
                      step="0.01"
                      @update:model-value="(value) => row.discount_factor = parseFloatValue(value, 0.01)"
                    />
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon"
                      class="h-8 w-8 rounded-lg text-rose-600 hover:bg-rose-50 hover:text-rose-600 dark:hover:bg-rose-950/30"
                      :disabled="productForm.term_discounts_json.length === 1"
                      @click="removeDiscountRow(discountIndex)"
                    >
                      <Trash2 class="h-4 w-4" />
                    </Button>
                  </div>
                </div>
              </div>
            </div>

            <div class="space-y-4 xl:border-l xl:border-border xl:pl-6">
              <div class="flex min-h-9 items-center justify-between gap-3 border-b border-border/60 pb-2">
                <div class="flex h-9 items-center">
                  <span class="text-sm font-medium">版本配置</span>
                </div>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  class="h-9 px-3.5"
                  @click="addVariantRow"
                >
                  <Plus class="mr-1.5 h-3.5 w-3.5" />
                  添加版本
                </Button>
              </div>

              <div class="space-y-3">
                <div
                  v-for="(variant, index) in productForm.variants"
                  :key="variant.key"
                  class="rounded-xl border border-border/60 bg-muted/10 p-3.5"
                >
                  <div class="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
                    <div class="flex min-w-0 items-start gap-2">
                      <Badge
                        variant="secondary"
                        class="mt-0.5 h-6 min-w-6 shrink-0 justify-center px-1.5 py-0 text-[10px]"
                      >
                        {{ index + 1 }}
                      </Badge>
                      <div class="min-w-0">
                        <div class="flex flex-wrap items-center gap-2">
                          <button
                            v-if="editingVariantNameKey !== variant.key"
                            type="button"
                            class="truncate text-left text-sm font-medium transition-colors hover:text-primary"
                            @click="startVariantNameEdit(variant)"
                          >
                            {{ variantDisplayName(variant, index) }}
                          </button>
                          <Input
                            v-else
                            :id="variantNameInputId(variant.key)"
                            v-model="variant.name"
                            class="h-8 w-40"
                            :placeholder="`版本${index + 1}`"
                            @blur="finishVariantNameEdit(index, variant)"
                            @keydown.enter.prevent="finishVariantNameEdit(index, variant)"
                            @keydown.esc.prevent="finishVariantNameEdit(index, variant)"
                          />
                          <Badge
                            v-if="variant.is_default_variant"
                            variant="outline"
                            class="h-6 px-2 text-[11px]"
                          >
                            默认
                          </Badge>
                          <Badge
                            variant="outline"
                            class="h-6 px-2 text-[11px] text-muted-foreground"
                          >
                            {{ variant.is_active ? '启用中' : '已停用' }}
                          </Badge>
                        </div>
                        <button
                          v-if="editingVariantCodeKey !== variant.key"
                          type="button"
                          class="mt-1 text-[11px] text-muted-foreground transition-colors hover:text-foreground"
                          @click="startVariantCodeEdit(variant)"
                        >
                          {{ variantDisplayCode(variant) }}
                        </button>
                        <Input
                          v-else
                          :id="variantCodeInputId(variant.key)"
                          v-model="variant.code"
                          class="mt-1 h-7 w-56 text-xs"
                          placeholder="如 claude-max-5x"
                          @blur="finishVariantCodeEdit(variant)"
                          @keydown.enter.prevent="finishVariantCodeEdit(variant)"
                          @keydown.esc.prevent="finishVariantCodeEdit(variant)"
                        />
                      </div>
                    </div>

                    <div class="flex shrink-0 items-center gap-1.5 self-end sm:self-start">
                      <Button
                        v-if="!variant.is_default_variant"
                        type="button"
                        variant="outline"
                        size="sm"
                        class="h-8 px-2.5 text-xs"
                        @click="setDefaultVariant(variant.key)"
                      >
                        设为默认
                      </Button>
                      <Switch v-model="variant.is_active" />
                      <Button
                        type="button"
                        variant="ghost"
                        size="icon"
                        class="h-8 w-8 rounded-lg text-rose-600 hover:bg-rose-50 hover:text-rose-600 dark:hover:bg-rose-950/30"
                        :disabled="productForm.variants.length === 1"
                        @click="removeVariantRow(index)"
                      >
                        <Trash2 class="h-4 w-4" />
                      </Button>
                    </div>
                  </div>

                  <div class="mt-3 grid gap-3 sm:grid-cols-3">
                    <div class="space-y-2">
                      <Label>月费 (USD)</Label>
                      <Input
                        :model-value="String(variant.monthly_price_usd)"
                        type="number"
                        min="0"
                        step="0.01"
                        @update:model-value="(value) => variant.monthly_price_usd = parseFloatValue(value, 0)"
                      />
                    </div>
                    <div class="space-y-2">
                      <Label>月额度 (USD)</Label>
                      <Input
                        :model-value="String(variant.monthly_quota_usd)"
                        type="number"
                        min="0"
                        step="0.01"
                        @update:model-value="(value) => variant.monthly_quota_usd = parseFloatValue(value, 0)"
                      />
                    </div>
                    <div class="space-y-2">
                      <Label>版本等级</Label>
                      <Input
                        :model-value="String(variant.variant_rank)"
                        type="number"
                        min="0"
                        @update:model-value="(value) => variant.variant_rank = parseIntValue(value, 0)"
                      />
                    </div>
                  </div>
                </div>
              </div>
            </div>
          </div>
        </div>
      </form>

      <template #footer>
        <Button
          variant="outline"
          type="button"
          class="h-10 flex-1 px-5 sm:flex-none"
          @click="handleProductDialogUpdate(false)"
        >
          取消
        </Button>
        <Button
          class="h-10 flex-1 px-5 sm:flex-none"
          :disabled="savingProduct"
          @click="submitProduct"
        >
          {{ savingProduct ? '保存中...' : editingProduct ? '更新产品' : '创建产品' }}
        </Button>
      </template>
    </Dialog>
  </div>
</template>

<script setup lang="ts">
import { computed, nextTick, onMounted, reactive, ref } from 'vue'
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
  Switch,
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
import {
  adminSubscriptionsApi,
  type SubscriptionCallbackListResponse,
  type SubscriptionOrder,
  type SubscriptionProduct,
  type SubscriptionTermDiscount,
  type SubscriptionVariant,
} from '@/api/admin-subscriptions'
import type { PaymentCallbackRecord } from '@/api/admin-payments'
import { usersApi, type UserGroup } from '@/api/users'
import { useConfirm } from '@/composables/useConfirm'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'
import {
  callbackStatusBadge,
  callbackStatusLabel,
  paymentMethodLabel,
  paymentStatusBadge,
  paymentStatusLabel,
} from '@/utils/walletDisplay'
import { EmptyState } from '@/components/common'
import { BadgePercent, CheckCircle2, Plus, Search, Trash2, XCircle } from 'lucide-vue-next'

interface DiscountRow extends SubscriptionTermDiscount {
  key: string
}

interface VariantFormState {
  key: string
  id?: string
  code: string
  name: string
  monthly_price_usd: number
  monthly_quota_usd: number
  variant_rank: number
  is_active: boolean
  is_default_variant: boolean
}

interface ProductFormState {
  code: string
  name: string
  description: string
  user_group_id: string
  plan_level: number
  overage_policy: 'block' | 'use_wallet_balance'
  is_active: boolean
  term_discounts_json: DiscountRow[]
  variants: VariantFormState[]
}

const { success, error } = useToast()
const { confirmDanger, confirmWarning } = useConfirm()

const activeTab = ref<'products' | 'orders' | 'reviews' | 'callbacks'>('products')
const loadingProducts = ref(false)
const loadingOrders = ref(false)
const loadingCallbacks = ref(false)
const loadingUserGroups = ref(false)
const savingProduct = ref(false)
const reviewingOrderId = ref<string | null>(null)

const products = ref<SubscriptionProduct[]>([])
const orders = ref<SubscriptionOrder[]>([])
const callbacks = ref<PaymentCallbackRecord[]>([])
const userGroups = ref<UserGroup[]>([])

const productSearch = ref('')
const orderSearch = ref('')
const reviewSearch = ref('')
const orderStatusFilter = ref('all')
const orderPaymentMethodFilter = ref('all')
const callbackMethodFilter = ref('all')
const callbackTotal = ref(0)
const callbackPage = ref(1)
const callbackPageSize = ref(20)

const showProductDialog = ref(false)
const editingProduct = ref<SubscriptionProduct | null>(null)
const editingVariantNameKey = ref<string | null>(null)
const editingVariantCodeKey = ref<string | null>(null)
const productForm = reactive<ProductFormState>(createEmptyProductForm())

const filteredProducts = computed(() => {
  const keyword = productSearch.value.trim().toLowerCase()
  if (!keyword) return products.value
  return products.value.filter((product) => {
    const variantsText = product.variants
      .map((variant) => `${variant.code} ${variant.name} ${variant.description || ''}`)
      .join(' ')
    const text = `${product.code} ${product.name} ${product.description || ''} ${product.user_group_name || ''} ${variantsText}`.toLowerCase()
    return text.includes(keyword)
  })
})

const filteredOrders = computed(() => {
  const keyword = orderSearch.value.trim().toLowerCase()
  return orders.value.filter((order) => {
    if (orderStatusFilter.value !== 'all' && order.status !== orderStatusFilter.value) {
      return false
    }
    if (
      orderPaymentMethodFilter.value !== 'all'
      && !(
        orderPaymentMethodFilter.value === 'manual_review'
          ? ['manual', 'manual_review'].includes(order.payment_method)
          : order.payment_method === orderPaymentMethodFilter.value
      )
    ) {
      return false
    }
    if (!keyword) return true
    const text = `${order.order_no} ${order.username || ''} ${order.email || ''} ${order.product_name || ''} ${order.variant_name || ''}`.toLowerCase()
    return text.includes(keyword)
  })
})

const reviewOrders = computed(() => {
  const keyword = reviewSearch.value.trim().toLowerCase()
  return orders.value.filter((order) => {
    if (!['manual', 'manual_review'].includes(order.payment_method)) return false
    if (order.status !== 'pending_approval') return false
    if (!keyword) return true
    const text = `${order.order_no} ${order.username || ''} ${order.email || ''} ${order.product_name || ''} ${order.variant_name || ''}`.toLowerCase()
    return text.includes(keyword)
  })
})

function createDiscountRow(months = 1, discountFactor = 1): DiscountRow {
  return {
    key: crypto.randomUUID(),
    months,
    discount_factor: discountFactor,
  }
}

function createVariantDefaultName(index: number): string {
  return `版本${index + 1}`
}

function cloneDiscountRows(items?: Array<{ months: number; discount_factor: number }> | null): DiscountRow[] {
  const source = items && items.length > 0 ? sortedDiscounts(items) : [{ months: 1, discount_factor: 1 }]
  return source.map((item) => createDiscountRow(item.months, item.discount_factor))
}

function createEmptyVariantForm(index = 0): VariantFormState {
  return {
    key: crypto.randomUUID(),
    code: '',
    name: createVariantDefaultName(index),
    monthly_price_usd: 0,
    monthly_quota_usd: 0,
    variant_rank: (index + 1) * 10,
    is_active: true,
    is_default_variant: index === 0,
  }
}

function createEmptyProductForm(): ProductFormState {
  return {
    code: '',
    name: '',
    description: '',
    user_group_id: '',
    plan_level: 10,
    overage_policy: 'block',
    is_active: true,
    term_discounts_json: [createDiscountRow(1, 1)],
    variants: [createEmptyVariantForm(0)],
  }
}

function sortedVariants(variants: SubscriptionVariant[]) {
  return [...variants].sort((a, b) => a.variant_rank - b.variant_rank)
}

function sortedDiscounts(items: Array<{ months: number; discount_factor: number }>) {
  return [...items].sort((a, b) => a.months - b.months)
}

function resetProductForm(product?: SubscriptionProduct | null) {
  editingVariantNameKey.value = null
  editingVariantCodeKey.value = null
  const nextState = product
    ? (() => {
        const orderedVariants = sortedVariants(product.variants)
        const sharedDiscountsSource = orderedVariants.find((variant) => variant.is_default_variant)?.term_discounts_json
          ?? orderedVariants[0]?.term_discounts_json
        return {
        code: product.code,
        name: product.name,
        description: product.description || '',
        user_group_id: product.user_group_id,
        plan_level: product.plan_level,
        overage_policy: product.overage_policy,
        is_active: product.is_active,
        term_discounts_json: cloneDiscountRows(sharedDiscountsSource),
        variants: orderedVariants.map((variant, index) => ({
          key: crypto.randomUUID(),
          id: variant.id,
          code: variant.code,
          name: variant.name || createVariantDefaultName(index),
          monthly_price_usd: variant.monthly_price_usd,
          monthly_quota_usd: variant.monthly_quota_usd,
          variant_rank: variant.variant_rank,
          is_active: variant.is_active,
          is_default_variant: variant.is_default_variant,
        })),
      }})()
    : createEmptyProductForm()

  Object.assign(productForm, nextState)
  if (!productForm.variants.some((variant) => variant.is_default_variant) && productForm.variants[0]) {
    productForm.variants[0].is_default_variant = true
  }
}

function parseIntValue(value: string | number, fallback: number): number {
  const parsed = Number.parseInt(String(value), 10)
  return Number.isFinite(parsed) && parsed >= 0 ? parsed : fallback
}

function parseFloatValue(value: string | number, fallback: number): number {
  const parsed = Number.parseFloat(String(value))
  return Number.isFinite(parsed) && parsed >= 0 ? parsed : fallback
}

function formatCurrency(value: number): string {
  return `$${value.toFixed(2)}`
}

function formatDateTime(value: string | null | undefined): string {
  if (!value) return '-'
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return value
  return date.toLocaleString('zh-CN', {
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
  return date.toLocaleDateString('zh-CN')
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

function overagePolicyLabel(policy: string): string {
  return policy === 'use_wallet_balance' ? '扣钱包' : '拦截'
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

function addVariantRow() {
  productForm.variants.push(createEmptyVariantForm(productForm.variants.length))
}

function removeVariantRow(index: number) {
  if (productForm.variants.length <= 1) return
  const removed = productForm.variants[index]
  productForm.variants.splice(index, 1)
  if (removed?.key === editingVariantNameKey.value) {
    editingVariantNameKey.value = null
  }
  if (removed?.key === editingVariantCodeKey.value) {
    editingVariantCodeKey.value = null
  }
  if (removed?.is_default_variant && productForm.variants[0]) {
    setDefaultVariant(productForm.variants[0].key)
  }
}

function setDefaultVariant(targetKey: string) {
  productForm.variants.forEach((variant) => {
    variant.is_default_variant = variant.key === targetKey
  })
}

function addDiscountRow() {
  productForm.term_discounts_json.push(createDiscountRow(productForm.term_discounts_json.length + 1, 1))
}

function removeDiscountRow(index: number) {
  if (productForm.term_discounts_json.length <= 1) return
  productForm.term_discounts_json.splice(index, 1)
}

function variantDisplayName(variant: VariantFormState, index: number): string {
  const name = variant.name.trim()
  return name || createVariantDefaultName(index)
}

function variantDisplayCode(variant: VariantFormState): string {
  const code = variant.code.trim()
  return code ? `版本编码：${code}` : '点击填写版本编码'
}

function variantNameInputId(key: string): string {
  return `subscription-variant-name-${key}`
}

function variantCodeInputId(key: string): string {
  return `subscription-variant-code-${key}`
}

function startVariantNameEdit(variant: VariantFormState) {
  editingVariantNameKey.value = variant.key
  editingVariantCodeKey.value = null
  void nextTick(() => {
    const input = document.getElementById(variantNameInputId(variant.key)) as HTMLInputElement | null
    input?.focus()
    input?.select()
  })
}

function finishVariantNameEdit(index: number, variant: VariantFormState) {
  variant.name = variantDisplayName(variant, index)
  editingVariantNameKey.value = null
}

function startVariantCodeEdit(variant: VariantFormState) {
  editingVariantCodeKey.value = variant.key
  editingVariantNameKey.value = null
  void nextTick(() => {
    const input = document.getElementById(variantCodeInputId(variant.key)) as HTMLInputElement | null
    input?.focus()
    input?.select()
  })
}

function finishVariantCodeEdit(variant: VariantFormState) {
  variant.code = variant.code.trim()
  editingVariantCodeKey.value = null
}

function normalizeDiscountRows(rows: DiscountRow[]) {
  const normalized = rows.map((item) => ({
    months: parseIntValue(item.months, 1),
    discount_factor: parseFloatValue(item.discount_factor, 1),
  }))
  const seen = new Set<number>()
  for (const item of normalized) {
    if (seen.has(item.months)) {
      throw new Error('购买月数不能重复')
    }
    seen.add(item.months)
    if (item.discount_factor <= 0) {
      throw new Error('折扣系数必须大于 0')
    }
  }
  return normalized.sort((a, b) => a.months - b.months)
}

function normalizeVariantPayload() {
  if (productForm.variants.length === 0) {
    throw new Error('至少需要一个订阅版本')
  }

  const sharedTermDiscounts = normalizeDiscountRows(productForm.term_discounts_json)
  const seenCodes = new Set<string>()
  const payload = productForm.variants.map((variant, index) => {
    const code = variant.code.trim()
    const name = variantDisplayName(variant, index)
    if (!code || !name) {
      throw new Error(`请完整填写版本 ${index + 1} 的编码和名称`)
    }
    if (seenCodes.has(code)) {
      throw new Error(`版本编码重复：${code}`)
    }
    seenCodes.add(code)
    return {
      id: variant.id,
      code,
      name,
      description: null,
      monthly_price_usd: parseFloatValue(variant.monthly_price_usd, 0),
      monthly_quota_usd: parseFloatValue(variant.monthly_quota_usd, 0),
      variant_rank: parseIntValue(variant.variant_rank, 0),
      is_active: variant.is_active,
      is_default_variant: variant.is_default_variant,
      term_discounts_json: sharedTermDiscounts,
    }
  })

  if (!payload.some((variant) => variant.is_default_variant)) {
    payload[0].is_default_variant = true
  }
  return payload
}

async function loadProducts() {
  loadingProducts.value = true
  try {
    const response = await adminSubscriptionsApi.listProducts()
    products.value = response.products
  } catch (err) {
    error(parseApiError(err, '加载订阅产品失败'))
  } finally {
    loadingProducts.value = false
  }
}

async function loadOrders() {
  loadingOrders.value = true
  try {
    const response = await adminSubscriptionsApi.listOrders()
    orders.value = response.orders
  } catch (err) {
    error(parseApiError(err, '加载订阅订单失败'))
  } finally {
    loadingOrders.value = false
  }
}

async function loadCallbacks() {
  loadingCallbacks.value = true
  try {
    const offset = (callbackPage.value - 1) * callbackPageSize.value
    const response: SubscriptionCallbackListResponse = await adminSubscriptionsApi.listCallbacks({
      payment_method: callbackMethodFilter.value !== 'all' ? callbackMethodFilter.value : undefined,
      limit: callbackPageSize.value,
      offset,
    })
    callbacks.value = response.items
    callbackTotal.value = response.total
  } catch (err) {
    error(parseApiError(err, '加载订阅回调失败'))
  } finally {
    loadingCallbacks.value = false
  }
}

async function loadUserGroups() {
  loadingUserGroups.value = true
  try {
    userGroups.value = await usersApi.getAllUserGroups()
  } catch (err) {
    error(parseApiError(err, '加载用户分组失败'))
  } finally {
    loadingUserGroups.value = false
  }
}

async function refreshAll() {
  await Promise.all([loadProducts(), loadOrders(), loadCallbacks(), loadUserGroups()])
}

function handleCallbackMethodChange() {
  callbackPage.value = 1
  void loadCallbacks()
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

function openCreateProductDialog() {
  editingProduct.value = null
  resetProductForm(null)
  showProductDialog.value = true
}

function openEditProductDialog(product: SubscriptionProduct) {
  editingProduct.value = product
  resetProductForm(product)
  showProductDialog.value = true
}

function handleProductDialogUpdate(value: boolean) {
  showProductDialog.value = value
  if (!value) {
    editingProduct.value = null
    resetProductForm(null)
  }
}

async function submitProduct() {
  if (savingProduct.value) return
  if (!productForm.code.trim() || !productForm.name.trim() || !productForm.user_group_id) {
    error('请先填写产品编码、名称和绑定用户组')
    return
  }

  try {
    savingProduct.value = true
    const payload = {
      code: productForm.code.trim(),
      name: productForm.name.trim(),
      description: productForm.description.trim() || null,
      user_group_id: productForm.user_group_id,
      plan_level: parseIntValue(productForm.plan_level, 0),
      overage_policy: productForm.overage_policy,
      is_active: productForm.is_active,
      variants: normalizeVariantPayload(),
    } as const

    if (editingProduct.value) {
      await adminSubscriptionsApi.updateProduct(editingProduct.value.id, payload)
      success('订阅产品已更新')
    } else {
      await adminSubscriptionsApi.createProduct(payload)
      success('订阅产品已创建')
    }

    handleProductDialogUpdate(false)
    await loadProducts()
  } catch (err) {
    error(parseApiError(err, editingProduct.value ? '更新订阅产品失败' : '创建订阅产品失败'))
  } finally {
    savingProduct.value = false
  }
}

async function removeProduct(product: SubscriptionProduct) {
  const confirmed = await confirmDanger(
    `确定删除订阅产品“${product.name}”吗？此操作不可撤销。`,
    '删除订阅产品',
    '删除'
  )
  if (!confirmed) return

  try {
    await adminSubscriptionsApi.deleteProduct(product.id)
    success('订阅产品已删除')
    await loadProducts()
  } catch (err) {
    error(parseApiError(err, '删除订阅产品失败'))
  }
}

async function approveOrder(order: SubscriptionOrder) {
  const confirmed = await confirmWarning(
    `确认通过订单 ${order.order_no} 吗？通过后会立即生效对应订阅。`,
    '通过订阅审核'
  )
  if (!confirmed) return

  try {
    reviewingOrderId.value = order.id
    await adminSubscriptionsApi.approveOrder(order.id)
    success('订阅订单已通过')
    await loadOrders()
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
    '拒绝'
  )
  if (!confirmed) return

  try {
    reviewingOrderId.value = order.id
    await adminSubscriptionsApi.rejectOrder(order.id)
    success('订阅订单已拒绝')
    await loadOrders()
  } catch (err) {
    error(parseApiError(err, '拒绝订阅审核失败'))
  } finally {
    reviewingOrderId.value = null
  }
}

onMounted(() => {
  refreshAll()
})
</script>
