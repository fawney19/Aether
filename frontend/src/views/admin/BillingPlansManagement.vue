<template>
  <PageContainer padding="lg">
    <PageHeader
      title="套餐管理"
      description="配置额度、流量限制、会员权益和组合套餐"
    >
      <template #actions>
        <Button
          size="sm"
          @click="openCreateDialog"
        >
          <Plus class="mr-2 h-4 w-4" />
          新建套餐
        </Button>
      </template>
    </PageHeader>

    <div class="mt-6 space-y-6">
      <div
        v-if="loading"
        class="py-16"
      >
        <LoadingState message="正在加载套餐..." />
      </div>

      <CardSection
        v-else
        title="套餐列表"
        description="启用后的套餐会出现在用户套餐中心"
      >
        <div
          v-if="plans.length === 0"
          class="py-12"
        >
          <EmptyState
            title="暂无套餐"
            description="创建第一个套餐后，用户可以在套餐中心购买"
          />
        </div>

        <div
          v-else
          class="overflow-x-auto"
        >
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead class="w-[24%]">
                  套餐
                </TableHead>
                <TableHead class="w-[10%] whitespace-nowrap">
                  价格
                </TableHead>
                <TableHead class="w-[18%] whitespace-nowrap">
                  权益有效期
                </TableHead>
                <TableHead class="w-[20%]">
                  权益
                </TableHead>
                <TableHead class="w-[6%] whitespace-nowrap text-center">
                  排序
                </TableHead>
                <TableHead class="w-[7%] whitespace-nowrap text-center">
                  状态
                </TableHead>
                <TableHead class="w-[15%] whitespace-nowrap text-right">
                  操作
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              <TableRow
                v-for="plan in plans"
                :key="plan.id"
              >
                <TableCell>
                  <div class="text-sm font-medium">
                    {{ plan.title }}
                  </div>
                  <div
                    v-if="plan.description"
                    class="mt-0.5 max-w-[280px] truncate text-xs text-muted-foreground"
                  >
                    {{ plan.description }}
                  </div>
                </TableCell>
                <TableCell class="whitespace-nowrap text-sm tabular-nums">
                  {{ formatPlanPriceAmount(plan) }} {{ plan.price_currency }}
                </TableCell>
                <TableCell>
                  <div class="whitespace-nowrap text-sm">
                    {{ formatPlanPeriod(plan) }}
                  </div>
                  <div class="mt-0.5 text-xs text-muted-foreground">
                    {{ planDurationHint(plan) }} · {{ planPurchaseLimitHint(plan) }}
                  </div>
                </TableCell>
                <TableCell>
                  <div class="flex flex-wrap items-center gap-1">
                    <span
                      v-for="item in entitlementBadges(plan)"
                      :key="item"
                      class="inline-flex whitespace-nowrap rounded-md border border-border/60 bg-muted/40 px-2 py-0.5 text-xs"
                    >
                      {{ item }}
                    </span>
                  </div>
                </TableCell>
                <TableCell class="text-center text-sm tabular-nums text-muted-foreground">
                  {{ plan.sort_order }}
                </TableCell>
                <TableCell class="text-center">
                  <span
                    class="inline-flex items-center gap-1.5 text-xs"
                    :class="plan.enabled ? 'text-emerald-500' : 'text-muted-foreground'"
                  >
                    <span
                      class="h-1.5 w-1.5 rounded-full"
                      :class="plan.enabled ? 'bg-emerald-500' : 'bg-muted-foreground/40'"
                    />
                    {{ plan.enabled ? '已启用' : '已停用' }}
                  </span>
                </TableCell>
                <TableCell class="whitespace-nowrap text-right">
                  <div class="inline-flex items-center">
                    <Button
                      variant="ghost"
                      size="sm"
                      :disabled="deletingPlanId === plan.id"
                      @click="togglePlanStatus(plan)"
                    >
                      {{ plan.enabled ? '停用' : '启用' }}
                    </Button>
                    <Button
                      variant="ghost"
                      size="sm"
                      :disabled="deletingPlanId === plan.id"
                      @click="openEditDialog(plan)"
                    >
                      编辑
                    </Button>
                    <DropdownMenu>
                      <DropdownMenuTrigger as-child>
                        <Button
                          variant="ghost"
                          size="sm"
                          class="h-9 w-9 p-0"
                          :disabled="deletingPlanId === plan.id"
                        >
                          <MoreHorizontal class="h-4 w-4" />
                        </Button>
                      </DropdownMenuTrigger>
                      <DropdownMenuContent align="end">
                        <DropdownMenuItem
                          class="text-destructive focus:text-destructive"
                          :disabled="deletingPlanId === plan.id"
                          @select="deletePlan(plan)"
                        >
                          <Trash2 class="mr-2 h-4 w-4" />
                          {{ deletingPlanId === plan.id ? '删除中...' : '删除套餐' }}
                        </DropdownMenuItem>
                      </DropdownMenuContent>
                    </DropdownMenu>
                  </div>
                </TableCell>
              </TableRow>
            </TableBody>
          </Table>
        </div>
      </CardSection>
    </div>

    <Dialog
      v-model:open="dialogOpen"
      size="4xl"
      :title="editingPlan ? '编辑套餐' : '新建套餐'"
      description="通过固定权益控件生成套餐配置"
      no-padding
    >
      <div class="max-h-[calc(100vh-193px)] space-y-4 overflow-y-auto px-6 py-4">
        <div class="grid grid-cols-1 gap-2 md:grid-cols-2 xl:grid-cols-4">
          <Button
            variant="outline"
            size="sm"
            class="h-12 justify-start rounded-xl px-3 text-left"
            @click="applyTemplate('daily')"
          >
            <span>
              <span class="block text-sm font-medium leading-5">每日额度月卡</span>
              <span class="block text-xs font-normal leading-4 text-muted-foreground">周期内每天重置</span>
            </span>
          </Button>
          <Button
            variant="outline"
            size="sm"
            class="h-12 justify-start rounded-xl px-3 text-left"
            @click="applyTemplate('traffic')"
          >
            <span>
              <span class="block text-sm font-medium leading-5">流量限制套餐</span>
              <span class="block text-xs font-normal leading-4 text-muted-foreground">QPS、RPM 与并发组合</span>
            </span>
          </Button>
          <Button
            variant="outline"
            size="sm"
            class="h-12 justify-start rounded-xl px-3 text-left"
            @click="applyTemplate('membership')"
          >
            <span>
              <span class="block text-sm font-medium leading-5">会员权益包</span>
              <span class="block text-xs font-normal leading-4 text-muted-foreground">动态授予分组</span>
            </span>
          </Button>
          <Button
            variant="outline"
            size="sm"
            class="h-12 justify-start rounded-xl px-3 text-left"
            @click="applyTemplate('mixed')"
          >
            <span>
              <span class="block text-sm font-medium leading-5">混合套餐</span>
              <span class="block text-xs font-normal leading-4 text-muted-foreground">周期权益组合</span>
            </span>
          </Button>
        </div>

        <div class="rounded-xl border border-border/60 bg-muted/20 px-3 py-2">
          <div class="grid grid-cols-1 gap-2 lg:grid-cols-[1fr_auto]">
            <div class="min-w-0 space-y-1.5">
              <div class="flex flex-wrap items-center gap-2">
                <div class="text-sm font-medium">
                  {{ planModeGuide.title }}
                </div>
                <Badge variant="outline">
                  {{ planModeGuide.badge }}
                </Badge>
              </div>
              <p class="text-xs leading-5 text-muted-foreground">
                {{ planModeGuide.description }}
              </p>
            </div>
            <div class="flex flex-wrap items-center gap-1.5 lg:justify-end">
              <span
                v-for="note in planModeGuide.notes"
                :key="note"
                class="rounded-full border border-border/60 bg-card/60 px-2.5 py-1 text-xs leading-4 text-muted-foreground"
              >
                {{ note }}
              </span>
            </div>
          </div>
        </div>

        <div
          v-if="planMode !== 'empty'"
          class="mx-auto w-full max-w-[880px] rounded-2xl border border-border/60 bg-muted/10 p-6"
        >
          <div class="grid grid-cols-1 gap-x-4 gap-y-3 xl:grid-cols-12">
            <div class="border-b border-border/70 pb-2 xl:col-span-12">
              <h3 class="text-sm font-semibold leading-5">
                基础信息
              </h3>
            </div>

            <div class="space-y-1.5 xl:col-span-8">
              <Label
                for="plan-title"
                class="inline-flex items-center gap-1.5 text-sm font-medium"
              >
                <span>套餐名称</span>
                <span class="text-destructive">*</span>
                <TooltipProvider>
                  <Tooltip>
                    <TooltipTrigger as-child>
                      <button
                        type="button"
                        class="text-muted-foreground/60 hover:text-muted-foreground"
                        aria-label="套餐名称说明"
                      >
                        <CircleHelp class="h-3.5 w-3.5" />
                      </button>
                    </TooltipTrigger>
                    <TooltipContent
                      side="top"
                      class="max-w-64 text-xs"
                    >
                      用户端购买页和订单快照里显示的套餐名称。
                    </TooltipContent>
                  </Tooltip>
                </TooltipProvider>
              </Label>
              <Input
                id="plan-title"
                v-model="form.title"
                class="h-9 rounded-xl bg-muted/70"
                placeholder="Pro 月卡"
              />
            </div>

            <div class="space-y-1.5 xl:col-span-4">
              <Label
                for="plan-price"
                class="inline-flex items-center gap-1.5 text-sm font-medium"
              >
                <span>价格</span>
                <span class="text-destructive">*</span>
                <TooltipProvider>
                  <Tooltip>
                    <TooltipTrigger as-child>
                      <button
                        type="button"
                        class="text-muted-foreground/60 hover:text-muted-foreground"
                        aria-label="价格字段说明"
                      >
                        <CircleHelp class="h-3.5 w-3.5" />
                      </button>
                    </TooltipTrigger>
                    <TooltipContent
                      side="top"
                      class="max-w-72 text-xs"
                    >
                      设置用户实际支付的套餐价格。
                    </TooltipContent>
                  </Tooltip>
                </TooltipProvider>
              </Label>
              <div class="grid grid-cols-[minmax(0,1fr)_88px]">
                <Input
                  id="plan-price"
                  v-model.number="form.price_amount"
                  class="h-9 rounded-l-xl rounded-r-none border-r-0 bg-muted/70 focus-visible:z-10"
                  type="number"
                  inputmode="decimal"
                  min="0.01"
                  step="0.01"
                  @blur="normalizePriceAmount"
                />
                <Select v-model="form.price_currency">
                  <SelectTrigger class="h-9 rounded-l-none rounded-r-xl border-l-0 bg-muted/70 px-3 focus:z-10">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem
                      v-for="currency in priceCurrencyOptions"
                      :key="currency"
                      :value="currency"
                    >
                      {{ currency }}
                    </SelectItem>
                  </SelectContent>
                </Select>
              </div>
            </div>

            <div class="space-y-1.5 xl:col-span-12">
              <Label
                for="plan-description"
                class="inline-flex items-center gap-1.5 text-sm font-medium"
              >
                <span>说明</span>
                <TooltipProvider>
                  <Tooltip>
                    <TooltipTrigger as-child>
                      <button
                        type="button"
                        class="text-muted-foreground/60 hover:text-muted-foreground"
                        aria-label="套餐说明字段说明"
                      >
                        <CircleHelp class="h-3.5 w-3.5" />
                      </button>
                    </TooltipTrigger>
                    <TooltipContent
                      side="top"
                      class="max-w-72 text-xs"
                    >
                      简短描述套餐包含的权益，建议控制在一两句话内。
                    </TooltipContent>
                  </Tooltip>
                </TooltipProvider>
              </Label>
              <Textarea
                id="plan-description"
                v-model="form.description"
                class="min-h-[72px] resize-y rounded-2xl bg-muted/70"
                rows="2"
                placeholder="简短说明套餐权益"
              />
            </div>
          </div>

          <section class="mt-5 space-y-3">
            <div class="border-b border-border/70 pb-2">
              <h3 class="text-sm font-semibold leading-5">
                购买限制
              </h3>
            </div>
            <div class="grid grid-cols-1 gap-x-4 gap-y-3 xl:grid-cols-12">
              <div
                class="space-y-1.5"
                :class="purchaseLimitFieldSpanClass"
              >
                <Label
                  for="plan-purchase-limit-scope"
                  class="inline-flex items-center gap-1.5 text-sm font-medium"
                >
                  <span>重复购买限制</span>
                  <TooltipProvider>
                    <Tooltip>
                      <TooltipTrigger as-child>
                        <button
                          type="button"
                          class="text-muted-foreground/60 hover:text-muted-foreground"
                          aria-label="重复购买限制说明"
                        >
                          <CircleHelp class="h-3.5 w-3.5" />
                        </button>
                      </TooltipTrigger>
                      <TooltipContent
                        side="top"
                        class="max-w-72 text-xs"
                      >
                        控制同一用户能否重复购买本套餐；不决定余额、每日额度或会员分组怎么发放。
                      </TooltipContent>
                    </Tooltip>
                  </TooltipProvider>
                </Label>
                <Select v-model="form.purchase_limit_scope">
                  <SelectTrigger
                    id="plan-purchase-limit-scope"
                    class="h-9 rounded-xl bg-muted/70 px-3"
                  >
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="active_period">
                      按周期限制
                    </SelectItem>
                    <SelectItem value="lifetime">
                      永久限制
                    </SelectItem>
                    <SelectItem value="unlimited">
                      不限购
                    </SelectItem>
                  </SelectContent>
                </Select>
              </div>

              <div
                class="space-y-1.5"
                :class="purchaseLimitFieldSpanClass"
              >
                <Label
                  for="plan-duration"
                  class="inline-flex items-center gap-1.5 text-sm font-medium"
                >
                  <span>{{ durationFieldLabel }}</span>
                  <span class="text-destructive">*</span>
                  <TooltipProvider>
                    <Tooltip>
                      <TooltipTrigger as-child>
                        <button
                          type="button"
                          class="text-muted-foreground/60 hover:text-muted-foreground"
                          aria-label="周期窗口说明"
                        >
                          <CircleHelp class="h-3.5 w-3.5" />
                        </button>
                      </TooltipTrigger>
                      <TooltipContent
                        side="top"
                        class="max-w-72 text-xs"
                      >
                        {{ durationTooltipText }}
                      </TooltipContent>
                    </Tooltip>
                  </TooltipProvider>
                </Label>
                <div class="grid grid-cols-[minmax(0,1fr)_88px]">
                  <Input
                    id="plan-duration"
                    v-model.number="form.duration_value"
                    class="h-9 rounded-l-xl rounded-r-none border-r-0 bg-muted/70 focus-visible:z-10"
                    type="number"
                    inputmode="numeric"
                    min="1"
                    step="1"
                    @blur="normalizeDurationValue"
                  />
                  <Select v-model="form.duration_unit">
                    <SelectTrigger class="h-9 rounded-l-none rounded-r-xl border-l-0 bg-muted/70 px-3 focus:z-10">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="day">
                        日
                      </SelectItem>
                      <SelectItem value="month">
                        月
                      </SelectItem>
                      <SelectItem value="year">
                        年
                      </SelectItem>
                      <SelectItem value="custom">
                        自定义天数
                      </SelectItem>
                    </SelectContent>
                  </Select>
                </div>
              </div>

              <div
                v-if="showPurchaseLimitCount"
                class="space-y-1.5"
                :class="purchaseLimitFieldSpanClass"
              >
                <Label
                  for="plan-max-active"
                  class="inline-flex items-center gap-1.5 text-sm font-medium"
                >
                  <span>{{ activeLimitFieldLabel }}</span>
                  <TooltipProvider>
                    <Tooltip>
                      <TooltipTrigger as-child>
                        <button
                          type="button"
                          class="text-muted-foreground/60 hover:text-muted-foreground"
                          aria-label="最多持有份数字段说明"
                        >
                          <CircleHelp class="h-3.5 w-3.5" />
                        </button>
                      </TooltipTrigger>
                      <TooltipContent
                        side="top"
                        class="max-w-72 text-xs"
                      >
                        {{ activeLimitTooltipText }}
                      </TooltipContent>
                    </Tooltip>
                  </TooltipProvider>
                </Label>
                <Input
                  id="plan-max-active"
                  v-model.number="form.max_active_per_user"
                  class="h-9 rounded-xl bg-muted/70"
                  type="number"
                  inputmode="numeric"
                  min="1"
                  step="1"
                  @blur="normalizeActiveLimit"
                />
              </div>
              <div class="xl:col-span-12 rounded-xl border border-border/60 bg-muted/20 px-3 py-2 text-xs leading-5 text-muted-foreground">
                <span class="font-medium text-foreground/80">当前逻辑：</span>
                {{ purchaseLimitSummaryText }}
              </div>
              <div class="xl:col-span-12 rounded-xl border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-xs leading-5 text-amber-200">
                每日额度和会员权益仍按类型互斥；使用限制套餐默认可同时生效，相同指标与窗口按最严格上限执行。仅同名套餐互斥组会替换旧套餐；替换时组合权益整体失效。
              </div>
            </div>
          </section>

          <section class="mt-5 space-y-3">
            <div class="border-b border-border/70 pb-2">
              <h3 class="text-sm font-semibold leading-5">
                展示与上架
              </h3>
            </div>
            <div class="grid grid-cols-1 gap-x-4 gap-y-3 xl:grid-cols-12">
              <div class="space-y-1.5 xl:col-span-6">
                <Label
                  for="plan-sort"
                  class="inline-flex items-center gap-1.5 text-sm font-medium"
                >
                  <span>展示排序</span>
                  <TooltipProvider>
                    <Tooltip>
                      <TooltipTrigger as-child>
                        <button
                          type="button"
                          class="text-muted-foreground/60 hover:text-muted-foreground"
                          aria-label="展示排序说明"
                        >
                          <CircleHelp class="h-3.5 w-3.5" />
                        </button>
                      </TooltipTrigger>
                      <TooltipContent
                        side="top"
                        class="max-w-64 text-xs"
                      >
                        数值越小越靠前，用户端套餐列表按排序值升序展示。
                      </TooltipContent>
                    </Tooltip>
                  </TooltipProvider>
                </Label>
                <Input
                  id="plan-sort"
                  v-model.number="form.sort_order"
                  class="h-9 rounded-xl bg-muted/70"
                  type="number"
                  step="1"
                />
              </div>
              <div class="space-y-1.5 xl:col-span-6">
                <Label class="inline-flex items-center gap-1.5 text-sm font-medium">
                  <span>上架状态</span>
                  <TooltipProvider>
                    <Tooltip>
                      <TooltipTrigger as-child>
                        <button
                          type="button"
                          class="text-muted-foreground/60 hover:text-muted-foreground"
                          aria-label="上架状态说明"
                        >
                          <CircleHelp class="h-3.5 w-3.5" />
                        </button>
                      </TooltipTrigger>
                      <TooltipContent
                        side="top"
                        class="max-w-64 text-xs"
                      >
                        停用后保留配置，但用户端套餐中心不再展示。
                      </TooltipContent>
                    </Tooltip>
                  </TooltipProvider>
                </Label>
                <div class="flex h-9 items-center justify-between rounded-xl border border-border/60 bg-muted/70 px-3">
                  <span class="text-sm text-muted-foreground">
                    {{ form.enabled ? '已上架' : '未上架' }}
                  </span>
                  <Switch v-model="form.enabled" />
                </div>
              </div>
            </div>
          </section>
        </div>

        <section
          v-if="planMode !== 'empty'"
          class="space-y-4"
        >
          <h3 class="text-sm font-semibold">
            权益配置
          </h3>

          <div
            v-if="showWalletCreditConfig"
            class="space-y-3 rounded-2xl border border-border/60 bg-muted/20 p-4"
          >
            <div class="flex items-center justify-between gap-3">
              <div>
                <Label class="text-sm font-medium">附赠余额</Label>
                <p class="mt-1 text-xs text-muted-foreground">
                  {{ walletCreditSummaryText }}
                </p>
              </div>
              <Switch v-model="form.wallet_credit_enabled" />
            </div>
            <div
              v-if="form.wallet_credit_enabled"
              class="grid grid-cols-1 gap-3 md:grid-cols-2"
            >
              <div class="space-y-1.5">
                <Label>发放金额 (USD)</Label>
                <Input
                  v-model.number="form.wallet_credit_amount_usd"
                  type="number"
                  min="0.01"
                  step="0.01"
                />
              </div>
              <div class="space-y-1.5">
                <Label>余额类型</Label>
                <Select v-model="form.wallet_credit_balance_bucket">
                  <SelectTrigger>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="recharge">
                      充值余额
                    </SelectItem>
                    <SelectItem value="gift">
                      赠款余额
                    </SelectItem>
                  </SelectContent>
                </Select>
              </div>
              <div class="space-y-1.5 md:col-span-2">
                <Label>套餐互斥组（可选）</Label>
                <Input
                  v-model="form.wallet_credit_replacement_group"
                  maxlength="128"
                  placeholder="pro-tier"
                />
                <p class="text-xs leading-5 text-muted-foreground">
                  与新套餐的任一权益使用同名组时，旧套餐及其组合权益会整体失效。
                </p>
              </div>
              <p class="rounded-xl border border-border/50 bg-card/60 px-3 py-2 text-xs leading-5 text-muted-foreground md:col-span-2">
                {{ walletCreditDetailText }}
              </p>
            </div>
          </div>

          <div
            v-if="showDailyQuotaConfig"
            class="space-y-3 rounded-2xl border border-border/60 bg-muted/20 p-4"
          >
            <div class="flex items-center justify-between gap-3">
              <div>
                <Label class="text-sm font-medium">每日额度</Label>
                <p class="mt-1 text-xs text-muted-foreground">
                  {{ dailyQuotaSummaryText }}
                </p>
              </div>
              <Switch v-model="form.daily_quota_enabled" />
            </div>
            <div
              v-if="form.daily_quota_enabled"
              class="grid grid-cols-1 gap-3 md:grid-cols-2"
            >
              <div class="space-y-1.5">
                <Label>每日额度 (USD)</Label>
                <Input
                  v-model.number="form.daily_quota_usd"
                  type="number"
                  min="0.01"
                  step="0.01"
                />
              </div>
              <div class="space-y-1.5">
                <Label>重置时区</Label>
                <Input
                  v-model="form.reset_timezone"
                  placeholder="Asia/Shanghai"
                />
              </div>
              <div class="space-y-1.5 md:col-span-2">
                <Label>套餐互斥组（可选）</Label>
                <Input
                  v-model="form.daily_quota_replacement_group"
                  maxlength="128"
                  placeholder="pro-tier"
                />
                <p class="text-xs leading-5 text-muted-foreground">
                  每日额度套餐本身已按类型互斥；填写后还可与其他权益类型的同名组整包互斥。
                </p>
              </div>
              <div class="flex items-center justify-between rounded-xl border border-border/60 bg-card/50 p-3">
                <div>
                  <Label>允许超额扣钱包</Label>
                  <p class="mt-1 text-xs text-muted-foreground">
                    额度不足时继续使用钱包余额
                  </p>
                </div>
                <Switch v-model="form.allow_wallet_overage" />
              </div>
              <div class="flex items-center justify-between rounded-xl border border-border/60 bg-card/50 p-3 opacity-70">
                <div>
                  <Label>额度结转</Label>
                  <p class="mt-1 text-xs text-muted-foreground">
                    当前后端固定不支持结转
                  </p>
                </div>
                <Switch
                  v-model="form.carry_over"
                  disabled
                />
              </div>
              <p class="rounded-xl border border-border/50 bg-card/60 px-3 py-2 text-xs leading-5 text-muted-foreground md:col-span-2">
                {{ dailyQuotaDetailText }}
              </p>
            </div>
          </div>

          <div
            v-if="showMembershipGroupConfig"
            class="space-y-3 rounded-2xl border border-border/60 bg-muted/20 p-4"
          >
            <div class="flex items-center justify-between gap-3">
              <div>
                <Label class="text-sm font-medium">会员分组</Label>
                <p class="mt-1 text-xs text-muted-foreground">
                  {{ membershipSummaryText }}
                </p>
              </div>
              <Switch v-model="form.membership_group_enabled" />
            </div>
            <div
              v-if="form.membership_group_enabled"
              class="space-y-3"
            >
              <p class="rounded-xl border border-border/50 bg-card/60 px-3 py-2 text-xs leading-5 text-muted-foreground">
                {{ membershipDetailText }}
              </p>
              <MultiSelect
                v-model="form.grant_user_groups"
                :options="userGroupOptions"
                placeholder="选择要授予的用户分组"
                empty-text="暂无用户分组"
              />
              <div class="grid grid-cols-1 gap-2 md:grid-cols-[1fr_auto]">
                <Input
                  v-model="manualGroupId"
                  placeholder="手动输入分组 ID"
                  @keyup.enter="addManualGroup"
                />
                <Button
                  variant="outline"
                  @click="addManualGroup"
                >
                  添加
                </Button>
              </div>
              <div class="space-y-1.5">
                <Label>套餐互斥组（可选）</Label>
                <Input
                  v-model="form.membership_group_replacement_group"
                  maxlength="128"
                  placeholder="pro-tier"
                />
                <p class="text-xs leading-5 text-muted-foreground">
                  会员权益包本身已按类型互斥；填写后还可与其他权益类型的同名组整包互斥。
                </p>
              </div>
            </div>
          </div>

          <div class="space-y-3 rounded-2xl border border-border/60 bg-muted/20 p-4">
            <div class="flex items-center justify-between gap-3">
              <div>
                <Label class="text-sm font-medium">使用限制</Label>
                <p class="mt-1 text-xs text-muted-foreground">
                  消费额度、QPS、RPM、周期请求数和并发硬限制
                </p>
              </div>
              <Switch v-model="form.usage_policy_enabled" />
            </div>

            <div
              v-if="form.usage_policy_enabled"
              class="space-y-3"
            >
              <div
                v-for="(policy, policyIndex) in form.usage_policies"
                :key="policy.local_id"
                class="space-y-3 rounded-xl border border-border/60 bg-card/50 p-3"
              >
                <div class="flex items-center justify-between gap-3">
                  <div class="text-sm font-medium">
                    策略 {{ policyIndex + 1 }}
                  </div>
                  <Button
                    variant="ghost"
                    size="sm"
                    class="h-8 w-8 p-0"
                    :aria-label="`删除策略 ${policyIndex + 1}`"
                    :title="`删除策略 ${policyIndex + 1}`"
                    @click="removeUsagePolicy(policyIndex)"
                  >
                    <Trash2 class="h-4 w-4" />
                  </Button>
                </div>

                <div class="grid grid-cols-1 gap-3 md:grid-cols-3">
                  <div class="space-y-1.5">
                    <Label>策略名称</Label>
                    <Input
                      v-model="policy.name"
                      maxlength="128"
                      placeholder="标准流量限制"
                    />
                  </div>
                  <div class="space-y-1.5">
                    <Label>策略标识</Label>
                    <Input
                      v-model="policy.policy_id"
                      maxlength="128"
                      placeholder="standard-traffic"
                    />
                  </div>
                  <div class="space-y-1.5">
                    <Label>套餐互斥组（可选）</Label>
                    <Input
                      v-model="policy.replacement_group"
                      maxlength="128"
                      placeholder="pro-tier"
                    />
                    <p class="text-xs leading-5 text-muted-foreground">
                      同组套餐换购时旧套餐整包失效；留空时此策略可与其他策略同时生效，相同指标与窗口按较低上限执行。每日额度和会员权益仍按类型互斥。
                    </p>
                  </div>
                </div>

                <div class="space-y-3">
                  <div class="space-y-3 lg:space-y-2">
                    <div
                      v-for="(rule, ruleIndex) in policy.rules"
                      :key="rule.local_id"
                      class="grid grid-cols-1 gap-3 rounded-xl border border-border/60 bg-background/50 p-3 lg:grid-cols-[150px_190px_minmax(260px,1fr)_110px_36px] lg:items-end lg:gap-2 lg:rounded-none lg:border-0 lg:bg-transparent lg:p-0"
                    >
                      <div class="space-y-1.5">
                        <Label>指标</Label>
                        <Select
                          v-model="rule.metric"
                          @update:model-value="onUsageMetricChanged(rule)"
                        >
                          <SelectTrigger>
                            <SelectValue />
                          </SelectTrigger>
                          <SelectContent>
                            <SelectItem value="request_count">
                              请求数
                            </SelectItem>
                            <SelectItem value="actual_cost_usd">
                              消费金额 (USD)
                            </SelectItem>
                            <SelectItem value="concurrency">
                              并发数
                            </SelectItem>
                          </SelectContent>
                        </Select>
                      </div>

                      <div class="space-y-1.5">
                        <Label>窗口</Label>
                        <Select
                          v-model="rule.window_kind"
                          @update:model-value="onUsageWindowChanged(rule)"
                        >
                          <SelectTrigger>
                            <SelectValue />
                          </SelectTrigger>
                          <SelectContent>
                            <SelectItem
                              v-if="rule.metric !== 'concurrency'"
                              value="rolling"
                            >
                              滚动窗口
                            </SelectItem>
                            <SelectItem
                              v-if="rule.metric !== 'concurrency'"
                              value="calendar_day"
                            >
                              自然日
                            </SelectItem>
                            <SelectItem
                              v-if="rule.metric !== 'concurrency'"
                              value="calendar_week"
                            >
                              自然周
                            </SelectItem>
                            <SelectItem
                              v-if="rule.metric !== 'concurrency'"
                              value="calendar_month"
                            >
                              自然月
                            </SelectItem>
                            <SelectItem
                              v-if="rule.metric !== 'concurrency'"
                              value="subscription_period"
                            >
                              套餐周期
                            </SelectItem>
                            <SelectItem
                              v-if="rule.metric === 'concurrency'"
                              value="concurrent"
                            >
                              同时进行
                            </SelectItem>
                          </SelectContent>
                        </Select>
                      </div>

                      <div class="space-y-1.5">
                        <Label>{{ usageWindowParameterLabel(rule) }}</Label>
                        <Input
                          v-if="rule.window_kind === 'rolling'"
                          v-model.number="rule.rolling_seconds"
                          type="number"
                          inputmode="numeric"
                          min="1"
                          max="2592000"
                          step="1"
                        />
                        <div
                          v-else-if="rule.window_kind === 'calendar_week'"
                          class="grid grid-cols-[minmax(140px,1fr)_108px] gap-2"
                        >
                          <Input
                            v-model="rule.timezone"
                            placeholder="继承系统时区"
                          />
                          <Select
                            :model-value="String(rule.week_start)"
                            @update:model-value="rule.week_start = Number($event)"
                          >
                            <SelectTrigger>
                              <SelectValue />
                            </SelectTrigger>
                            <SelectContent>
                              <SelectItem
                                v-for="option in weekStartOptions"
                                :key="option.value"
                                :value="option.value"
                              >
                                {{ option.label }}
                              </SelectItem>
                            </SelectContent>
                          </Select>
                        </div>
                        <Input
                          v-else-if="rule.window_kind === 'calendar_day' || rule.window_kind === 'calendar_month'"
                          v-model="rule.timezone"
                          placeholder="继承系统时区"
                        />
                        <Input
                          v-else
                          model-value="-"
                          disabled
                        />
                      </div>

                      <div class="space-y-1.5">
                        <Label>{{ rule.metric === 'actual_cost_usd' ? '上限 (USD)' : '上限' }}</Label>
                        <Input
                          v-model.number="rule.limit"
                          type="number"
                          inputmode="numeric"
                          :min="rule.metric === 'actual_cost_usd' ? 0.00000001 : 1"
                          :step="rule.metric === 'actual_cost_usd' ? 0.01 : 1"
                        />
                      </div>

                      <Button
                        variant="ghost"
                        size="sm"
                        class="h-9 w-9 justify-self-end p-0"
                        :aria-label="`删除规则 ${ruleIndex + 1}`"
                        :title="`删除规则 ${ruleIndex + 1}`"
                        @click="removeUsagePolicyRule(policy, ruleIndex)"
                      >
                        <Trash2 class="h-4 w-4" />
                      </Button>
                    </div>
                  </div>
                </div>

                <div class="flex flex-wrap items-center gap-2">
                  <DropdownMenu>
                    <DropdownMenuTrigger as-child>
                      <Button
                        variant="outline"
                        size="sm"
                        :disabled="usagePolicyRuleLimitReached(policy)"
                      >
                        <Plus class="mr-2 h-4 w-4" />
                        添加规则
                      </Button>
                    </DropdownMenuTrigger>
                    <DropdownMenuContent align="start">
                      <DropdownMenuItem @select="addUsagePolicyRule(policy, 'qps')">
                        QPS
                      </DropdownMenuItem>
                      <DropdownMenuItem @select="addUsagePolicyRule(policy, 'rpm')">
                        RPM
                      </DropdownMenuItem>
                      <DropdownMenuItem @select="addUsagePolicyRule(policy, 'rolling_5h')">
                        滚动 5 小时请求数
                      </DropdownMenuItem>
                      <DropdownMenuItem @select="addUsagePolicyRule(policy, 'cost_5h')">
                        5 小时消费额度
                      </DropdownMenuItem>
                      <DropdownMenuItem @select="addUsagePolicyRule(policy, 'cost_daily')">
                        每日消费额度
                      </DropdownMenuItem>
                      <DropdownMenuItem @select="addUsagePolicyRule(policy, 'cost_weekly')">
                        每周消费额度
                      </DropdownMenuItem>
                      <DropdownMenuItem @select="addUsagePolicyRule(policy, 'cost_monthly')">
                        每月消费额度
                      </DropdownMenuItem>
                      <DropdownMenuItem @select="addUsagePolicyRule(policy, 'cost_subscription')">
                        套餐周期消费额度
                      </DropdownMenuItem>
                      <DropdownMenuItem @select="addUsagePolicyRule(policy, 'daily')">
                        每日请求数
                      </DropdownMenuItem>
                      <DropdownMenuItem @select="addUsagePolicyRule(policy, 'weekly')">
                        每周请求数
                      </DropdownMenuItem>
                      <DropdownMenuItem @select="addUsagePolicyRule(policy, 'monthly')">
                        每月请求数
                      </DropdownMenuItem>
                      <DropdownMenuItem @select="addUsagePolicyRule(policy, 'subscription')">
                        套餐周期请求数
                      </DropdownMenuItem>
                      <DropdownMenuItem @select="addUsagePolicyRule(policy, 'concurrency')">
                        并发数
                      </DropdownMenuItem>
                    </DropdownMenuContent>
                  </DropdownMenu>
                  <span
                    v-if="policy.rules.length >= MAX_USAGE_POLICY_RULES_PER_POLICY"
                    class="text-xs text-muted-foreground"
                  >每个策略最多 {{ MAX_USAGE_POLICY_RULES_PER_POLICY }} 条规则</span>
                  <span
                    v-else-if="totalUsagePolicyRules >= MAX_USAGE_POLICY_TOTAL_RULES"
                    class="text-xs text-muted-foreground"
                  >每个套餐的规则合计最多 {{ MAX_USAGE_POLICY_TOTAL_RULES }} 条</span>
                </div>
              </div>

              <Button
                variant="outline"
                size="sm"
                :disabled="usagePolicyLimitReached"
                @click="addUsagePolicy"
              >
                <Plus class="mr-2 h-4 w-4" />
                添加策略
              </Button>
              <span
                v-if="form.usage_policies.length >= MAX_USAGE_POLICY_ENTITLEMENTS"
                class="text-xs text-muted-foreground"
              >每个套餐最多 {{ MAX_USAGE_POLICY_ENTITLEMENTS }} 份策略</span>
              <span
                v-else-if="totalUsagePolicyRules >= MAX_USAGE_POLICY_TOTAL_RULES"
                class="text-xs text-muted-foreground"
              >每个套餐的规则合计最多 {{ MAX_USAGE_POLICY_TOTAL_RULES }} 条</span>
            </div>
          </div>
        </section>
      </div>

      <div class="flex h-14 items-center justify-end gap-3 border-t border-border bg-muted/10 px-6">
        <Button
          variant="outline"
          class="h-9"
          :disabled="saving"
          @click="dialogOpen = false"
        >
          取消
        </Button>
        <Button
          variant="default"
          class="h-9"
          :disabled="saving || isSaveDisabled"
          @click="savePlan"
        >
          {{ saving ? '保存中...' : '保存套餐' }}
        </Button>
      </div>
    </Dialog>
  </PageContainer>
</template>

<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import { CircleHelp, MoreHorizontal, Plus, Trash2 } from 'lucide-vue-next'
import {
  adminBillingPlansApi,
  type BillingDurationUnit,
  type BillingEntitlement,
  type BillingPlan,
  type BillingPurchaseLimitScope,
  type BillingPlanWriteRequest,
  type DailyQuotaEntitlement,
  type MembershipGroupEntitlement,
  type UsagePolicyEntitlement,
  type UsagePolicyMetric,
  type UsagePolicyRule,
  type UsagePolicyWindow,
  type WalletCreditBucket,
  type WalletCreditEntitlement,
} from '@/api/billing'
import { usersApi, type UserGroup } from '@/api/users'
import {
  Badge,
  Button,
  Dialog,
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
  Input,
  Label,
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
  Textarea,
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from '@/components/ui'
import { EmptyState, LoadingState, MultiSelect } from '@/components/common'
import { CardSection, PageContainer, PageHeader } from '@/components/layout'
import { useToast } from '@/composables/useToast'
import { useI18n } from '@/i18n'
import { parseApiError } from '@/utils/errorParser'
import { log } from '@/utils/logger'

type TemplateKey = 'daily' | 'membership' | 'mixed' | 'traffic'
type PlanMode = 'empty' | 'wallet' | 'daily' | 'membership' | 'usage' | 'mixed'
type UsagePolicyWindowKind = UsagePolicyWindow['kind']
type UsagePolicyRulePreset =
  | 'qps'
  | 'rpm'
  | 'rolling_5h'
  | 'cost_5h'
  | 'cost_daily'
  | 'cost_weekly'
  | 'cost_monthly'
  | 'cost_subscription'
  | 'daily'
  | 'weekly'
  | 'monthly'
  | 'subscription'
  | 'concurrency'

interface UsagePolicyRuleForm {
  local_id: string
  metric: UsagePolicyMetric
  window_kind: UsagePolicyWindowKind
  rolling_seconds: number
  timezone: string
  week_start: number
  limit: number
}

interface UsagePolicyForm {
  local_id: string
  policy_id: string
  name: string
  replacement_group: string
  rules: UsagePolicyRuleForm[]
}

interface PlanModeGuide {
  badge: string
  title: string
  description: string
  notes: string[]
}

interface PlanFormState {
  title: string
  description: string
  price_amount: number
  price_currency: string
  duration_unit: BillingDurationUnit
  duration_value: number
  enabled: boolean
  sort_order: number
  max_active_per_user: number
  purchase_limit_scope: BillingPurchaseLimitScope
  wallet_credit_enabled: boolean
  wallet_credit_amount_usd: number
  wallet_credit_balance_bucket: WalletCreditBucket
  wallet_credit_replacement_group: string
  daily_quota_enabled: boolean
  daily_quota_usd: number
  reset_timezone: string
  carry_over: boolean
  allow_wallet_overage: boolean
  daily_quota_replacement_group: string
  membership_group_enabled: boolean
  grant_user_groups: string[]
  membership_group_replacement_group: string
  usage_policy_enabled: boolean
  usage_policies: UsagePolicyForm[]
}

const MAX_USAGE_POLICY_ENTITLEMENTS = 16
const MAX_USAGE_POLICY_RULES_PER_POLICY = 32
const MAX_USAGE_POLICY_TOTAL_RULES = 64
const MAX_USAGE_POLICY_EXACT_INTEGER = Number.MAX_SAFE_INTEGER
const MIN_USAGE_POLICY_COST_USD = 0.00000001
const MAX_USAGE_POLICY_COST_USD = Number.MAX_SAFE_INTEGER / 100_000_000

let localIdSequence = 0

const { success, error: showError } = useToast()
const { legacyT } = useI18n()

const loading = ref(true)
const saving = ref(false)
const deletingPlanId = ref<string | null>(null)
const dialogOpen = ref(false)
const plans = ref<BillingPlan[]>([])
const editingPlan = ref<BillingPlan | null>(null)
const userGroups = ref<UserGroup[]>([])
const manualGroupId = ref('')

const form = reactive<PlanFormState>(buildDefaultForm())

const userGroupOptions = computed(() =>
  userGroups.value.map((group) => ({
    value: group.id,
    label: group.name,
  }))
)

const priceCurrencyOptions = computed(() => {
  const normalized = form.price_currency.trim().toUpperCase()
  const options = ['CNY', 'USD']
  return normalized && !options.includes(normalized) ? [...options, normalized] : options
})

const hasValidPriceAmount = computed(() => {
  const value = Number(form.price_amount)
  if (!Number.isFinite(value) || value <= 0) return false
  return /^\d+(\.\d{1,2})?$/.test(String(form.price_amount))
})

const hasValidDuration = computed(() =>
  Number.isInteger(Number(form.duration_value)) && Number(form.duration_value) > 0
)

const hasValidDurationUnit = computed(() =>
  ['day', 'month', 'year', 'custom'].includes(form.duration_unit)
)

const hasValidActiveLimit = computed(() =>
  Number.isInteger(Number(form.max_active_per_user)) && Number(form.max_active_per_user) > 0
)

const hasValidPurchaseLimitScope = computed(() =>
  ['active_period', 'lifetime', 'unlimited'].includes(form.purchase_limit_scope)
)

const hasSelectedPackageEntitlement = computed(() =>
  form.daily_quota_enabled || form.membership_group_enabled || form.usage_policy_enabled
)

const showPurchaseLimitCount = computed(() =>
  form.purchase_limit_scope !== 'unlimited'
)

const purchaseLimitFieldSpanClass = computed(() =>
  showPurchaseLimitCount.value ? 'xl:col-span-4' : 'xl:col-span-6'
)

const isSaveDisabled = computed(() =>
  !form.title.trim()
  || !form.price_currency.trim()
  || !hasValidPriceAmount.value
  || !hasValidDuration.value
  || !hasValidDurationUnit.value
  || (showPurchaseLimitCount.value && !hasValidActiveLimit.value)
  || !hasValidPurchaseLimitScope.value
  || !hasSelectedPackageEntitlement.value
)

const totalUsagePolicyRules = computed(() =>
  form.usage_policies.reduce((total, policy) => total + policy.rules.length, 0)
)

const usagePolicyLimitReached = computed(() =>
  form.usage_policies.length >= MAX_USAGE_POLICY_ENTITLEMENTS
  || totalUsagePolicyRules.value >= MAX_USAGE_POLICY_TOTAL_RULES
)

const planMode = computed<PlanMode>(() => {
  const enabledCount = [
    form.wallet_credit_enabled,
    form.daily_quota_enabled,
    form.membership_group_enabled,
    form.usage_policy_enabled,
  ].filter(Boolean).length

  if (enabledCount === 0) return 'empty'
  if (enabledCount > 1) return 'mixed'
  if (form.wallet_credit_enabled) return 'wallet'
  if (form.daily_quota_enabled) return 'daily'
  if (form.membership_group_enabled) return 'membership'
  return 'usage'
})

const planModeGuide = computed<PlanModeGuide>(() => {
  switch (planMode.value) {
    case 'wallet':
      return {
        badge: '旧余额权益',
        title: '旧余额套餐',
        description: '纯余额套餐已由钱包充值功能承接。建议停用该套餐，或补充每日额度/会员分组后作为混合套餐。',
        notes: [
          '新建套餐不再提供余额包模板',
          '钱包入账请使用充值功能',
          '附赠余额仍可放在混合套餐中',
        ],
      }
    case 'daily':
      return {
        badge: '周期额度',
        title: '每日额度套餐',
        description: '适合月卡、季卡、年卡。周期内每天给用户独立 USD 额度，到期后不再生效。',
        notes: [
          '默认每天按重置时区刷新',
          '默认用完后拒绝继续消费',
          '同一用户只保留一个有效每日额度套餐',
        ],
      }
    case 'membership':
      return {
        badge: '会员权限',
        title: '会员权益包',
        description: '适合 Pro、Plus、团队会员。购买后动态合并用户分组权限，到期自然失效。',
        notes: [
          '不会永久修改用户基础分组',
          '同一用户只保留一个有效会员权益包',
          '适合解锁模型组或高级功能',
        ],
      }
    case 'usage':
      return {
        badge: '流量策略',
        title: '使用限制套餐',
        description: '按套餐限制请求频率、周期请求数和同时进行的请求数，多条规则可以自由组合。',
        notes: [
          '任意规则触顶都会拒绝新请求',
          'QPS 和 RPM 使用滚动窗口',
          '策略随套餐权益到期失效',
        ],
      }
    case 'mixed':
      return {
        badge: '混合套餐',
        title: '组合权益套餐',
        description: '适合同时包含额度、会员权限和流量限制的产品，也可以按需附赠少量钱包余额。',
        notes: [
          '包含每日额度或会员权益时，同类旧套餐会整包失效',
          '附赠余额发放后不随周期结束扣回',
          '限购会同时影响整套组合权益',
        ],
      }
    default:
      return {
        badge: '待配置',
        title: '选择一种套餐模板',
        description: '先选择每日额度、会员分组、流量限制或混合套餐，再配置价格、购买限制和权益。',
        notes: [
          '每日额度和会员权益是周期权益',
          '钱包充值已从套餐中拆出',
          '混合套餐可以附赠余额',
        ],
      }
  }
})

const durationFieldLabel = computed(() => '权益有效期')

const durationTooltipText = computed(() => {
  if (planMode.value === 'wallet') {
    return '旧余额套餐仍会保存这个有效期，但余额发放后不会在到期时扣回；建议改用钱包充值功能。'
  }
  return '购买后套餐权益生效这么久，与重复购买限制的统计方式相互独立。自定义单位按天计算。'
})

const activeLimitFieldLabel = computed(() =>
  form.purchase_limit_scope === 'lifetime' ? '每人最多购买次数' : '每人最多同时生效'
)

const activeLimitTooltipText = computed(() =>
  form.purchase_limit_scope === 'lifetime'
    ? '按同一用户历史成功购买次数累计，达到该值后不能再次购买，适合首购特惠包。'
    : '只统计仍在周期内的已生效权益，过期后释放名额，用于防止周期权益无限叠加。'
)

const purchaseLimitSummaryText = computed(() => {
  if (form.purchase_limit_scope === 'unlimited') {
    return '不检查同一用户的重复购买次数；每次支付成功都会按下方权益配置发放。'
  }
  if (form.purchase_limit_scope === 'lifetime') {
    return `同一用户历史成功购买本套餐达到 ${form.max_active_per_user || 1} 次后不能再买；适合首购特惠或一次性礼包。`
  }
  return `同一用户最多同时生效 ${form.max_active_per_user || 1} 份本套餐；每份权益在 ${formatDuration(form.duration_unit, form.duration_value || 1)} 后到期并释放名额。`
})

const weekStartOptions = [
  { value: '1', label: '周一' },
  { value: '2', label: '周二' },
  { value: '3', label: '周三' },
  { value: '4', label: '周四' },
  { value: '5', label: '周五' },
  { value: '6', label: '周六' },
  { value: '7', label: '周日' },
]

const showWalletCreditConfig = computed(() =>
  planMode.value === 'mixed' || form.wallet_credit_enabled
)

const showDailyQuotaConfig = computed(() =>
  planMode.value === 'mixed' || form.daily_quota_enabled
)

const showMembershipGroupConfig = computed(() =>
  planMode.value === 'mixed' || form.membership_group_enabled
)

const walletCreditSummaryText = computed(() =>
  planMode.value === 'mixed'
    ? '作为套餐附赠余额一次性发放'
    : '旧余额套餐会一次性发放到充值余额或赠款余额'
)

const walletCreditDetailText = computed(() => {
  const bucket = form.wallet_credit_balance_bucket === 'recharge' ? '充值余额' : '赠款余额'
  return `当前会发放到${bucket}。附赠余额发放后进入钱包，不会因为套餐周期结束而自动扣回。`
})

const dailyQuotaSummaryText = computed(() =>
  planMode.value === 'mixed'
    ? '组合套餐内的周期性每日 USD 消费用量'
    : '每天独立 USD 消费用量，默认不结转'
)

const dailyQuotaDetailText = computed(() =>
  form.allow_wallet_overage
    ? '每日额度不足时会继续使用钱包余额，适合希望用户不中断请求的套餐。'
    : '每日额度不足时不再继续扣钱包，适合严格封顶的月卡或体验卡。'
)

const membershipSummaryText = computed(() =>
  planMode.value === 'mixed'
    ? '组合套餐内的动态会员权限'
    : '购买后动态合并分组权限，到期自动失效'
)

const membershipDetailText = computed(() =>
  '这里授予的是动态分组权限，不会永久改写用户基础分组；权益到期后权限解析会自动移除。'
)

onMounted(() => {
  void Promise.all([loadPlans(), loadUserGroups()]).finally(() => {
    loading.value = false
  })
})

function buildDefaultForm(): PlanFormState {
  return {
    title: '',
    description: '',
    price_amount: 100,
    price_currency: 'CNY',
    duration_unit: 'month',
    duration_value: 1,
    enabled: true,
    sort_order: 0,
    max_active_per_user: 1,
    purchase_limit_scope: 'active_period',
    wallet_credit_enabled: false,
    wallet_credit_amount_usd: 10,
    wallet_credit_balance_bucket: 'recharge',
    wallet_credit_replacement_group: '',
    daily_quota_enabled: false,
    daily_quota_usd: 50,
    reset_timezone: 'Asia/Shanghai',
    carry_over: false,
    allow_wallet_overage: false,
    daily_quota_replacement_group: '',
    membership_group_enabled: false,
    grant_user_groups: [],
    membership_group_replacement_group: '',
    usage_policy_enabled: false,
    usage_policies: [buildDefaultUsagePolicy()],
  }
}

function nextLocalId(prefix: string): string {
  localIdSequence += 1
  return `${prefix}-${localIdSequence}`
}

function buildDefaultUsageRule(preset: UsagePolicyRulePreset = 'rpm'): UsagePolicyRuleForm {
  const rule: UsagePolicyRuleForm = {
    local_id: nextLocalId('rule'),
    metric: 'request_count',
    window_kind: 'rolling',
    rolling_seconds: 60,
    timezone: 'Asia/Shanghai',
    week_start: 1,
    limit: 60,
  }
  switch (preset) {
    case 'qps':
      rule.rolling_seconds = 1
      rule.limit = 5
      break
    case 'rolling_5h':
      rule.rolling_seconds = 18_000
      rule.limit = 1_000
      break
    case 'cost_5h':
      rule.metric = 'actual_cost_usd'
      rule.rolling_seconds = 18_000
      rule.limit = 12.5
      break
    case 'cost_daily':
      rule.metric = 'actual_cost_usd'
      rule.window_kind = 'calendar_day'
      rule.limit = 25
      break
    case 'cost_weekly':
      rule.metric = 'actual_cost_usd'
      rule.window_kind = 'calendar_week'
      rule.limit = 100
      break
    case 'cost_monthly':
      rule.metric = 'actual_cost_usd'
      rule.window_kind = 'calendar_month'
      rule.limit = 300
      break
    case 'cost_subscription':
      rule.metric = 'actual_cost_usd'
      rule.window_kind = 'subscription_period'
      rule.limit = 300
      break
    case 'daily':
      rule.window_kind = 'calendar_day'
      rule.limit = 10_000
      break
    case 'weekly':
      rule.window_kind = 'calendar_week'
      rule.limit = 50_000
      break
    case 'monthly':
      rule.window_kind = 'calendar_month'
      rule.limit = 200_000
      break
    case 'subscription':
      rule.window_kind = 'subscription_period'
      rule.limit = 200_000
      break
    case 'concurrency':
      rule.metric = 'concurrency'
      rule.window_kind = 'concurrent'
      rule.limit = 5
      break
    case 'rpm':
      break
  }
  return rule
}

function buildDefaultUsagePolicy(): UsagePolicyForm {
  return {
    local_id: nextLocalId('policy'),
    policy_id: '',
    name: '',
    replacement_group: '',
    rules: [buildDefaultUsageRule()],
  }
}

function assignForm(next: PlanFormState) {
  Object.assign(form, next)
}

async function loadPlans() {
  try {
    const response = await adminBillingPlansApi.list()
    plans.value = [...response.items].sort((left, right) =>
      left.sort_order === right.sort_order
        ? left.price_amount - right.price_amount
        : left.sort_order - right.sort_order
    )
  } catch (err) {
    log.error('加载套餐失败:', err)
    showError(parseApiError(err, '加载套餐失败'))
  }
}

async function loadUserGroups() {
  try {
    const response = await usersApi.listUserGroups()
    userGroups.value = response.items
  } catch (err) {
    log.error('加载用户分组失败:', err)
    showError(parseApiError(err, '加载用户分组失败'))
  }
}

function openCreateDialog() {
  editingPlan.value = null
  assignForm(buildDefaultForm())
  manualGroupId.value = ''
  dialogOpen.value = true
}

function openEditDialog(plan: BillingPlan) {
  editingPlan.value = plan
  assignForm(formFromPlan(plan))
  manualGroupId.value = ''
  dialogOpen.value = true
}

function formFromPlan(plan: BillingPlan): PlanFormState {
  const next = buildDefaultForm()
  next.title = plan.title
  next.description = plan.description || ''
  next.price_amount = plan.price_amount
  next.price_currency = plan.price_currency.toUpperCase()
  next.duration_unit = plan.duration_unit
  next.duration_value = plan.duration_value
  next.enabled = plan.enabled
  next.sort_order = plan.sort_order
  next.max_active_per_user = plan.max_active_per_user
  next.purchase_limit_scope = plan.purchase_limit_scope || 'active_period'

  for (const entitlement of plan.entitlements || []) {
    if (entitlement.type === 'wallet_credit') {
      const wallet = entitlement as WalletCreditEntitlement
      next.wallet_credit_enabled = true
      next.wallet_credit_amount_usd = Number(wallet.amount_usd || next.wallet_credit_amount_usd)
      next.wallet_credit_balance_bucket = wallet.balance_bucket || 'recharge'
      next.wallet_credit_replacement_group = wallet.replacement_group || ''
    } else if (entitlement.type === 'daily_quota') {
      const quota = entitlement as DailyQuotaEntitlement
      next.daily_quota_enabled = true
      next.daily_quota_usd = Number(quota.daily_quota_usd || next.daily_quota_usd)
      next.reset_timezone = quota.reset_timezone || 'Asia/Shanghai'
      next.carry_over = Boolean(quota.carry_over)
      next.allow_wallet_overage = Boolean(quota.allow_wallet_overage)
      next.daily_quota_replacement_group = quota.replacement_group || ''
    } else if (entitlement.type === 'membership_group') {
      const membership = entitlement as MembershipGroupEntitlement
      next.membership_group_enabled = true
      next.grant_user_groups = Array.isArray(membership.grant_user_groups)
        ? [...membership.grant_user_groups]
        : []
      next.membership_group_replacement_group = membership.replacement_group || ''
    } else if (entitlement.type === 'usage_policy') {
      const policy = entitlement as UsagePolicyEntitlement
      next.usage_policy_enabled = true
      next.usage_policies.push(usagePolicyFormFromEntitlement(policy))
    }
  }
  if (next.usage_policy_enabled) {
    next.usage_policies = next.usage_policies.slice(1)
  }
  return next
}

function applyTemplate(template: TemplateKey) {
  const next = buildDefaultForm()
  if (template === 'daily') {
    next.title = '100 RMB 月卡'
    next.description = '每日 50 USD 独立额度，周期 1 个月'
    next.daily_quota_enabled = true
    next.daily_quota_usd = 50
    next.max_active_per_user = 1
  } else if (template === 'membership') {
    next.title = 'Pro 月卡'
    next.description = '动态授予 Pro 用户分组 1 个月'
    next.membership_group_enabled = true
    next.max_active_per_user = 1
  } else if (template === 'mixed') {
    next.title = 'Pro 混合月卡'
    next.description = '每日额度、会员分组和流量限制组合'
    next.daily_quota_enabled = true
    next.daily_quota_usd = 50
    next.membership_group_enabled = true
    next.usage_policy_enabled = true
    next.usage_policies = [buildDefaultUsagePolicy()]
    next.max_active_per_user = 1
  } else {
    next.title = '标准流量月卡'
    next.description = '每秒 5 次、每分钟 60 次，最多 5 个并发请求'
    next.usage_policy_enabled = true
    next.usage_policies = [
      {
        local_id: nextLocalId('policy'),
        policy_id: 'standard-traffic',
        name: '标准流量限制',
        replacement_group: '',
        rules: [
          buildDefaultUsageRule('qps'),
          buildDefaultUsageRule('rpm'),
          buildDefaultUsageRule('concurrency'),
        ],
      },
    ]
  }
  assignForm(next)
}

function buildEntitlements(): BillingEntitlement[] {
  const entitlements: BillingEntitlement[] = []
  if (form.wallet_credit_enabled) {
    const entitlement: WalletCreditEntitlement = {
      type: 'wallet_credit',
      amount_usd: Number(form.wallet_credit_amount_usd),
      balance_bucket: form.wallet_credit_balance_bucket,
    }
    attachReplacementGroup(entitlement, form.wallet_credit_replacement_group)
    entitlements.push(entitlement)
  }
  if (form.daily_quota_enabled) {
    const entitlement: DailyQuotaEntitlement = {
      type: 'daily_quota',
      daily_quota_usd: Number(form.daily_quota_usd),
      reset_timezone: form.reset_timezone.trim() || 'Asia/Shanghai',
      carry_over: false,
      allow_wallet_overage: Boolean(form.allow_wallet_overage),
    }
    attachReplacementGroup(entitlement, form.daily_quota_replacement_group)
    entitlements.push(entitlement)
  }
  if (form.membership_group_enabled) {
    const entitlement: MembershipGroupEntitlement = {
      type: 'membership_group',
      grant_user_groups: form.grant_user_groups.map((value) => value.trim()).filter(Boolean),
    }
    attachReplacementGroup(entitlement, form.membership_group_replacement_group)
    entitlements.push(entitlement)
  }
  if (form.usage_policy_enabled) {
    for (const policy of form.usage_policies) {
      entitlements.push(buildUsagePolicyEntitlement(policy))
    }
  }
  return entitlements
}

function attachReplacementGroup(
  entitlement: BillingEntitlement,
  replacementGroup: string,
): void {
  const normalized = replacementGroup.trim()
  if (normalized) entitlement.replacement_group = normalized
}

function usagePolicyFormFromEntitlement(policy: UsagePolicyEntitlement): UsagePolicyForm {
  return {
    local_id: nextLocalId('policy'),
    policy_id: policy.policy_id || '',
    name: policy.name || '',
    replacement_group: policy.replacement_group || '',
    rules: (policy.rules || []).map(usagePolicyRuleFormFromRule),
  }
}

function usagePolicyRuleFormFromRule(rule: UsagePolicyRule): UsagePolicyRuleForm {
  return {
    local_id: nextLocalId('rule'),
    metric: rule.metric,
    window_kind: rule.window.kind,
    rolling_seconds: rule.window.kind === 'rolling' ? Number(rule.window.seconds) : 60,
    timezone: 'timezone' in rule.window ? rule.window.timezone || '' : '',
    week_start: rule.window.kind === 'calendar_week' ? Number(rule.window.week_start || 1) : 1,
    limit: Number(rule.limit),
  }
}

function buildUsagePolicyEntitlement(policy: UsagePolicyForm): UsagePolicyEntitlement {
  const entitlement: UsagePolicyEntitlement = {
    type: 'usage_policy',
    rules: policy.rules.map(buildUsagePolicyRule),
  }
  const policyId = policy.policy_id.trim()
  const name = policy.name.trim()
  const replacementGroup = policy.replacement_group.trim()
  if (policyId) entitlement.policy_id = policyId
  if (name) entitlement.name = name
  if (replacementGroup) entitlement.replacement_group = replacementGroup
  return entitlement
}

function buildUsagePolicyRule(rule: UsagePolicyRuleForm): UsagePolicyRule {
  let window: UsagePolicyWindow
  switch (rule.window_kind) {
    case 'rolling':
      window = { kind: 'rolling', seconds: Number(rule.rolling_seconds) }
      break
    case 'calendar_day':
    case 'calendar_month':
      window = rule.timezone.trim()
        ? { kind: rule.window_kind, timezone: rule.timezone.trim() }
        : { kind: rule.window_kind }
      break
    case 'calendar_week':
      window = rule.timezone.trim()
        ? {
            kind: 'calendar_week',
            timezone: rule.timezone.trim(),
            week_start: Number(rule.week_start || 1),
          }
        : {
            kind: 'calendar_week',
            week_start: Number(rule.week_start || 1),
          }
      break
    case 'subscription_period':
      window = { kind: 'subscription_period' }
      break
    case 'concurrent':
      window = { kind: 'concurrent' }
      break
  }
  return {
    metric: rule.metric,
    window,
    limit: Number(rule.limit),
    enforcement: 'hard_cap',
  }
}

function addUsagePolicy() {
  if (usagePolicyLimitReached.value) return
  form.usage_policies = [...form.usage_policies, buildDefaultUsagePolicy()]
}

function removeUsagePolicy(index: number) {
  form.usage_policies = form.usage_policies.filter((_, itemIndex) => itemIndex !== index)
}

function addUsagePolicyRule(policy: UsagePolicyForm, preset: UsagePolicyRulePreset) {
  if (usagePolicyRuleLimitReached(policy)) return
  policy.rules = [...policy.rules, buildDefaultUsageRule(preset)]
}

function usagePolicyRuleLimitReached(policy: UsagePolicyForm): boolean {
  return policy.rules.length >= MAX_USAGE_POLICY_RULES_PER_POLICY
    || totalUsagePolicyRules.value >= MAX_USAGE_POLICY_TOTAL_RULES
}

function removeUsagePolicyRule(policy: UsagePolicyForm, index: number) {
  policy.rules = policy.rules.filter((_, itemIndex) => itemIndex !== index)
}

function onUsageMetricChanged(rule: UsagePolicyRuleForm) {
  if (rule.metric === 'concurrency') {
    rule.window_kind = 'concurrent'
  } else if (rule.window_kind === 'concurrent') {
    rule.window_kind = 'rolling'
  }
  if (rule.metric === 'actual_cost_usd' && Number.isInteger(rule.limit)) {
    rule.limit = 10
  }
}

function onUsageWindowChanged(rule: UsagePolicyRuleForm) {
  if (rule.window_kind === 'concurrent') {
    rule.metric = 'concurrency'
  } else if (rule.metric === 'concurrency') {
    rule.metric = 'request_count'
  }
}

function isCalendarWindow(kind: UsagePolicyWindowKind): boolean {
  return kind === 'calendar_day' || kind === 'calendar_week' || kind === 'calendar_month'
}

function usageWindowParameterLabel(rule: UsagePolicyRuleForm): string {
  if (rule.window_kind === 'rolling') return '窗口秒数'
  if (rule.window_kind === 'calendar_week') return '时区 / 周起始日'
  if (isCalendarWindow(rule.window_kind)) return '时区（可选）'
  return '窗口参数'
}

function normalizePriceAmount() {
  const value = Number(form.price_amount)
  if (!Number.isFinite(value) || value <= 0) return
  form.price_amount = Number(value.toFixed(2))
}

function normalizeDurationValue() {
  const value = Number(form.duration_value)
  if (!Number.isFinite(value) || value <= 0) return
  form.duration_value = Math.floor(value)
}

function normalizeActiveLimit() {
  const value = Number(form.max_active_per_user)
  if (!Number.isFinite(value) || value <= 0) return
  form.max_active_per_user = Math.floor(value)
}

function isValidIanaTimezone(value: string): boolean {
  const timezone = value.trim()
  if (!timezone) return true
  try {
    new Intl.DateTimeFormat('en-US', { timeZone: timezone }).format()
    return true
  } catch {
    return false
  }
}

function validatePlan(entitlements: BillingEntitlement[]): string | null {
  if (!form.title.trim()) return '请输入套餐名称'
  if (!Number.isFinite(Number(form.price_amount)) || Number(form.price_amount) <= 0) return '价格必须大于 0'
  if (!hasValidPriceAmount.value) return '价格最多支持两位小数'
  if (!form.price_currency.trim()) return '请输入价格币种'
  if (!hasValidPurchaseLimitScope.value) return '重复购买限制必须是按周期限制、永久限制或不限购'
  if (!hasValidDurationUnit.value) return '权益有效期单位必须是日/月/年/自定义天数'
  if (!hasValidDuration.value) return '权益有效期必须是正整数'
  if (showPurchaseLimitCount.value && !hasValidActiveLimit.value) {
    return `${activeLimitFieldLabel.value}必须是正整数`
  }
  if (entitlements.length === 0) return '至少启用一种权益'
  if (!hasPackageEntitlement(entitlements)) return '套餐至少需要包含每日额度、会员分组或使用限制；钱包充值请使用充值功能'
  if (form.wallet_credit_enabled && Number(form.wallet_credit_amount_usd) <= 0) return '附赠余额金额必须大于 0'
  if (form.daily_quota_enabled && Number(form.daily_quota_usd) <= 0) return '每日额度必须大于 0'
  if (form.membership_group_enabled && form.grant_user_groups.length === 0) return '会员分组权益至少选择一个分组'
  if (
    form.wallet_credit_enabled
    && form.wallet_credit_replacement_group.trim().length > 128
  ) return '附赠余额的套餐互斥组不能超过 128 个字符'
  if (
    form.daily_quota_enabled
    && form.daily_quota_replacement_group.trim().length > 128
  ) return '每日额度的套餐互斥组不能超过 128 个字符'
  if (
    form.membership_group_enabled
    && form.membership_group_replacement_group.trim().length > 128
  ) return '会员权益的套餐互斥组不能超过 128 个字符'
  if (form.usage_policy_enabled) {
    if (form.usage_policies.length === 0) return '至少添加一份使用限制策略'
    if (form.usage_policies.length > MAX_USAGE_POLICY_ENTITLEMENTS) {
      return `每个套餐最多支持 ${MAX_USAGE_POLICY_ENTITLEMENTS} 份使用限制策略`
    }
    if (totalUsagePolicyRules.value > MAX_USAGE_POLICY_TOTAL_RULES) {
      return `每个套餐的使用限制规则合计最多 ${MAX_USAGE_POLICY_TOTAL_RULES} 条`
    }
    for (const [policyIndex, policy] of form.usage_policies.entries()) {
      if (policy.policy_id.trim().length > 128) return `策略 ${policyIndex + 1} 的标识不能超过 128 个字符`
      if (policy.name.trim().length > 128) return `策略 ${policyIndex + 1} 的名称不能超过 128 个字符`
      if (policy.replacement_group.trim().length > 128) return `策略 ${policyIndex + 1} 的套餐互斥组不能超过 128 个字符`
      if (policy.rules.length === 0) return `策略 ${policyIndex + 1} 至少需要一条规则`
      if (policy.rules.length > MAX_USAGE_POLICY_RULES_PER_POLICY) {
        return `策略 ${policyIndex + 1} 最多支持 ${MAX_USAGE_POLICY_RULES_PER_POLICY} 条规则`
      }
      for (const [ruleIndex, rule] of policy.rules.entries()) {
        const label = `策略 ${policyIndex + 1} 的规则 ${ruleIndex + 1}`
        if (!Number.isFinite(Number(rule.limit)) || Number(rule.limit) <= 0) {
          return `${label}上限必须是正数`
        }
        if (rule.metric !== 'actual_cost_usd' && !Number.isInteger(Number(rule.limit))) {
          return `${label}上限必须是正整数`
        }
        if (
          rule.metric !== 'actual_cost_usd'
          && Number(rule.limit) > MAX_USAGE_POLICY_EXACT_INTEGER
        ) {
          return `${label}上限不能超过 ${MAX_USAGE_POLICY_EXACT_INTEGER}`
        }
        if (
          rule.metric === 'actual_cost_usd'
          && (
            Number(rule.limit) < MIN_USAGE_POLICY_COST_USD
            || Number(rule.limit) > MAX_USAGE_POLICY_COST_USD
          )
        ) {
          return `${label}的消费金额上限必须在 ${MIN_USAGE_POLICY_COST_USD} 到 ${MAX_USAGE_POLICY_COST_USD} USD 之间`
        }
        if (rule.metric === 'concurrency' && rule.window_kind !== 'concurrent') {
          return `${label}的并发指标必须使用同时进行窗口`
        }
        if (rule.metric === 'request_count' && rule.window_kind === 'concurrent') {
          return `${label}的请求数指标不能使用同时进行窗口`
        }
        if (rule.metric === 'actual_cost_usd' && rule.window_kind === 'concurrent') {
          return `${label}的消费金额指标不能使用同时进行窗口`
        }
        if (
          rule.window_kind === 'rolling'
          && (!Number.isInteger(Number(rule.rolling_seconds))
            || Number(rule.rolling_seconds) <= 0
            || Number(rule.rolling_seconds) > 2_592_000)
        ) {
          return `${label}的滚动窗口必须是 1 到 2592000 秒之间的整数`
        }
        if (
          rule.window_kind === 'calendar_week'
          && (!Number.isInteger(Number(rule.week_start))
            || Number(rule.week_start) < 1
            || Number(rule.week_start) > 7)
        ) {
          return `${label}的周起始日必须在周一到周日之间`
        }
        if (
          ['calendar_day', 'calendar_week', 'calendar_month'].includes(rule.window_kind)
          && !isValidIanaTimezone(rule.timezone)
        ) {
          return `${label}的时区必须是有效的 IANA 时区，例如 Asia/Shanghai`
        }
      }
    }
  }
  return null
}

function buildPlanPayload(): BillingPlanWriteRequest | null {
  const entitlements = buildEntitlements()
  const validationError = validatePlan(entitlements)
  if (validationError) {
    showError(validationError)
    return null
  }
  return {
    title: form.title.trim(),
    description: form.description.trim() || null,
    price_amount: Number(Number(form.price_amount).toFixed(2)),
    price_currency: form.price_currency.trim().toUpperCase(),
    duration_unit: form.duration_unit,
    duration_value: Number(form.duration_value),
    enabled: form.enabled,
    sort_order: Number(form.sort_order),
    max_active_per_user: showPurchaseLimitCount.value ? Number(form.max_active_per_user) : 1,
    purchase_limit_scope: form.purchase_limit_scope,
    entitlements,
  }
}

async function savePlan() {
  const payload = buildPlanPayload()
  if (!payload) return

  saving.value = true
  try {
    if (editingPlan.value) {
      await adminBillingPlansApi.update(editingPlan.value.id, payload)
      success('套餐已更新')
    } else {
      await adminBillingPlansApi.create(payload)
      success('套餐已创建')
    }
    dialogOpen.value = false
    await loadPlans()
  } catch (err) {
    log.error('保存套餐失败:', err)
    showError(parseApiError(err, '保存套餐失败'))
  } finally {
    saving.value = false
  }
}

async function togglePlanStatus(plan: BillingPlan) {
  try {
    await adminBillingPlansApi.setStatus(plan.id, !plan.enabled)
    success(plan.enabled ? '套餐已停用' : '套餐已启用')
    await loadPlans()
  } catch (err) {
    log.error('更新套餐状态失败:', err)
    showError(parseApiError(err, '更新套餐状态失败'))
  }
}

async function deletePlan(plan: BillingPlan) {
  if (deletingPlanId.value) return
  const confirmed = window.confirm(
    legacyT(`确定删除套餐「${plan.title}」吗？\n\n已有订单或权益的套餐不能删除，请改为停用。删除后无法恢复。`)
  )
  if (!confirmed) return

  deletingPlanId.value = plan.id
  try {
    await adminBillingPlansApi.delete(plan.id)
    success('套餐已删除')
    await loadPlans()
  } catch (err) {
    log.error('删除套餐失败:', err)
    showError(parseApiError(err, '删除套餐失败'))
  } finally {
    deletingPlanId.value = null
  }
}

function addManualGroup() {
  const value = manualGroupId.value.trim()
  if (!value) return
  if (!form.grant_user_groups.includes(value)) {
    form.grant_user_groups = [...form.grant_user_groups, value]
  }
  manualGroupId.value = ''
}

function formatPlanPriceAmount(plan: BillingPlan): string {
  return Number(plan.price_amount || 0).toFixed(2)
}

function durationUnitLabel(unit: BillingDurationUnit): string {
  const labels: Record<BillingDurationUnit, string> = {
    day: '天',
    month: '个月',
    year: '年',
    custom: '天',
  }
  return labels[unit] || labels.month
}

function formatDuration(unit: BillingDurationUnit, value: number): string {
  return `${value}${durationUnitLabel(unit)}`
}

function formatPlanPeriod(plan: BillingPlan): string {
  return formatDuration(plan.duration_unit, plan.duration_value)
}

function planPurchaseLimitHint(plan: BillingPlan): string {
  if (plan.purchase_limit_scope === 'unlimited') return '不限购'
  if (plan.purchase_limit_scope === 'lifetime') {
    return `每人最多购买 ${plan.max_active_per_user} 次`
  }
  return `最多同时生效 ${plan.max_active_per_user} 份`
}

function resolvePlanModeFromEntitlements(entitlements: BillingEntitlement[] | undefined): PlanMode {
  const items = entitlements || []
  const hasWallet = items.some((entitlement) => entitlement.type === 'wallet_credit')
  const hasDaily = items.some((entitlement) => entitlement.type === 'daily_quota')
  const hasMembership = items.some((entitlement) => entitlement.type === 'membership_group')
  const hasUsage = items.some((entitlement) => entitlement.type === 'usage_policy')
  const enabledCount = [hasWallet, hasDaily, hasMembership, hasUsage].filter(Boolean).length

  if (enabledCount === 0) return 'empty'
  if (enabledCount > 1) return 'mixed'
  if (hasWallet) return 'wallet'
  if (hasDaily) return 'daily'
  if (hasMembership) return 'membership'
  return 'usage'
}

function planDurationHint(plan: BillingPlan): string {
  if (plan.purchase_limit_scope === 'unlimited') return '不限制重复购买'
  if (plan.purchase_limit_scope === 'lifetime') return '不按周期重置购买次数'
  const mode = resolvePlanModeFromEntitlements(plan.entitlements)
  if (mode === 'wallet') return '旧余额套餐，建议停用'
  if (mode === 'daily') return '每日额度周期'
  if (mode === 'membership') return '会员权限周期'
  if (mode === 'usage') return '使用限制周期'
  if (mode === 'mixed') return '组合权益周期'
  return '未配置权益'
}

function groupName(groupId: string): string {
  return userGroups.value.find((group) => group.id === groupId)?.name || groupId
}

function entitlementBadges(plan: BillingPlan): string[] {
  return (plan.entitlements || []).map((entitlement) => {
    if (entitlement.type === 'wallet_credit') {
      return `附赠余额 $${Number(entitlement.amount_usd || 0).toFixed(2)}`
    }
    if (entitlement.type === 'daily_quota') {
      return `每日 $${Number(entitlement.daily_quota_usd || 0).toFixed(2)}`
    }
    if (entitlement.type === 'membership_group') {
      const groups = entitlement.grant_user_groups.map(groupName).join(', ')
      return `会员组 ${groups}`
    }
    if (entitlement.type === 'usage_policy') {
      return formatUsagePolicyBadge(entitlement)
    }
    return '未知权益'
  })
}

function formatUsagePolicyBadge(policy: UsagePolicyEntitlement): string {
  const rules = policy.rules || []
  if (rules.length === 0) return '使用限制'
  const first = rules[0]
  const suffix = rules.length > 1 ? ` +${rules.length - 1}` : ''
  if (first.metric === 'concurrency') return `并发 ${first.limit}${suffix}`
  if (first.metric === 'request_count' && first.window.kind === 'rolling' && first.window.seconds === 1) {
    return `QPS ${first.limit}${suffix}`
  }
  if (first.metric === 'request_count' && first.window.kind === 'rolling' && first.window.seconds === 60) {
    return `RPM ${first.limit}${suffix}`
  }
  const windowLabels: Record<UsagePolicyWindowKind, string> = {
    rolling: `${first.window.kind === 'rolling' ? first.window.seconds : 0} 秒`,
    calendar_day: '每日',
    calendar_week: '每周',
    calendar_month: '每月',
    subscription_period: '套餐周期',
    concurrent: '并发',
  }
  const limit = first.metric === 'actual_cost_usd' ? `$${first.limit}` : first.limit
  return `${windowLabels[first.window.kind]} ${limit}${suffix}`
}

function hasPackageEntitlement(entitlements: BillingEntitlement[] | undefined): boolean {
  return (entitlements || []).some((entitlement) =>
    entitlement.type === 'daily_quota'
    || entitlement.type === 'membership_group'
    || entitlement.type === 'usage_policy'
  )
}
</script>
