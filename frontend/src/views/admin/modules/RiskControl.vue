<template>
  <PageContainer>
    <PageHeader
      title="风控中心"
      :icon="ShieldAlert"
    >
      <template #actions>
        <Button
          variant="outline"
          :disabled="loading || saving"
          @click="loadAll"
        >
          <RefreshCw
            class="mr-2 h-4 w-4"
            :class="{ 'animate-spin': loading }"
          />
          刷新状态
        </Button>
        <Button
          variant="outline"
          :disabled="saving"
          @click="configDialogOpen = true"
        >
          <Settings class="mr-2 h-4 w-4" />
          配置
        </Button>
      </template>
    </PageHeader>

    <div class="mt-6 space-y-6">
      <section class="relative overflow-hidden rounded-2xl border border-border/80 bg-gradient-to-br from-card via-card to-orange-50/60 p-5 shadow-sm dark:from-slate-950 dark:via-slate-950 dark:to-slate-900">
        <div class="pointer-events-none absolute inset-0 opacity-70">
          <div class="absolute -left-24 -top-24 h-72 w-72 rounded-full bg-orange-200/30 blur-3xl dark:bg-orange-500/10" />
          <div class="absolute right-10 top-8 h-48 w-48 rounded-full bg-rose-200/25 blur-3xl dark:bg-rose-500/10" />
          <div class="absolute bottom-0 left-1/3 h-px w-1/2 bg-gradient-to-r from-transparent via-orange-200/70 to-transparent dark:via-orange-300/20" />
        </div>

        <div class="relative space-y-5">
          <div class="space-y-5">
            <div class="flex flex-wrap items-start justify-between gap-4">
              <div class="max-w-3xl">
                <div class="mb-3 flex flex-wrap items-center gap-2">
                  <Badge class="border-orange-200 bg-orange-50 text-orange-700 dark:border-orange-400/30 dark:bg-orange-400/10 dark:text-orange-100">
                    <RadioTower class="mr-1 h-3.5 w-3.5" />
                    Local Guardrail
                  </Badge>
                  <Badge
                    variant="outline"
                    class="border-border/80 bg-background/70 text-foreground"
                  >
                    {{ riskModeLabel }}
                  </Badge>
                </div>
                <h2 class="text-2xl font-semibold tracking-tight text-foreground md:text-3xl">
                  风控运行态
                </h2>
              </div>

              <div class="relative hidden h-28 w-28 shrink-0 place-items-center rounded-full border border-orange-200/70 bg-orange-50/80 xl:grid dark:border-orange-300/20 dark:bg-white/5">
                <div class="absolute h-20 w-20 rounded-full border border-orange-200/70 dark:border-orange-300/20" />
                <div class="absolute h-12 w-12 rounded-full border border-rose-200/80 dark:border-rose-300/20" />
                <div class="risk-radar-sweep absolute h-24 w-24 rounded-full" />
                <ShieldCheck class="relative h-9 w-9 text-orange-600 dark:text-orange-200" />
              </div>
            </div>

            <div
              v-if="status?.config_error"
              class="rounded-xl border border-destructive/20 bg-destructive/10 px-4 py-3 shadow-sm sm:flex sm:items-center sm:justify-between sm:gap-4"
            >
              <div class="flex min-w-0 items-start gap-3">
                <AlertTriangle class="mt-0.5 h-4 w-4 shrink-0 text-destructive" />
                <div class="min-w-0">
                  <p class="text-sm font-semibold text-destructive">
                    配置异常
                  </p>
                  <p class="mt-1 text-xs leading-5 text-destructive/80">
                    {{ status.config_error }}
                  </p>
                </div>
              </div>
              <div class="mt-3 grid gap-2 sm:mt-0 sm:flex sm:shrink-0">
                <Button
                  size="sm"
                  class="justify-center"
                  @click="openConfigPanel('provider')"
                >
                  去配置
                </Button>
                <Button
                  size="sm"
                  variant="outline"
                  class="justify-center border-destructive/25 bg-background/70 text-destructive hover:bg-destructive/10"
                  @click="switchToKeywordOnly"
                >
                  切到仅关键词
                </Button>
              </div>
            </div>

            <div
              v-if="status?.notification_warning"
              class="rounded-xl border border-amber-200 bg-amber-50 px-4 py-3 shadow-sm sm:flex sm:items-center sm:justify-between sm:gap-4 dark:border-amber-300/25 dark:bg-amber-400/10"
            >
              <div class="flex min-w-0 items-start gap-3">
                <AlertTriangle class="mt-0.5 h-4 w-4 shrink-0 text-amber-600 dark:text-amber-200" />
                <div class="min-w-0">
                  <p class="text-sm font-semibold text-amber-800 dark:text-amber-100">
                    命中告警未就绪
                  </p>
                  <p class="mt-1 text-xs leading-5 text-amber-700 dark:text-amber-100/80">
                    {{ status.notification_warning }}
                  </p>
                </div>
              </div>
              <Button
                size="sm"
                variant="outline"
                class="mt-3 justify-center border-amber-300 bg-background/70 text-amber-800 hover:bg-amber-100 sm:mt-0 sm:shrink-0 dark:border-amber-300/30 dark:text-amber-100 dark:hover:bg-amber-400/10"
                @click="openConfigPanel('retention')"
              >
                查看告警
              </Button>
            </div>

            <div class="grid gap-3 sm:grid-cols-2 xl:grid-cols-7">
              <div
                v-for="metric in statusMetrics"
                :key="metric.label"
                class="rounded-xl border border-border/70 bg-background/80 p-4 shadow-sm backdrop-blur"
              >
                <div class="flex items-center justify-between gap-3">
                  <p class="text-xs font-medium text-muted-foreground">
                    {{ metric.label }}
                  </p>
                  <span
                    class="h-2 w-2 rounded-full"
                    :class="metric.dotClass"
                  />
                </div>
                <p class="mt-3 text-2xl font-semibold text-foreground">
                  {{ metric.value }}
                </p>
                <p class="mt-1 text-xs text-muted-foreground">
                  {{ metric.detail }}
                </p>
              </div>
            </div>
          </div>
        </div>
      </section>

      <div class="grid gap-4">
        <CardSection
          title="审核记录"
        >
          <template #actions>
            <div class="hidden items-center gap-2 lg:flex">
              <Button
                variant="outline"
                size="sm"
                class="rounded-xl border-border bg-background/80 px-4"
                :disabled="logsLoading || !hasActiveLogFilters"
                @click="resetLogFilters"
              >
                <ArchiveX class="mr-2 h-4 w-4" />
                重置筛选
              </Button>
              <Button
                variant="outline"
                size="sm"
                class="rounded-xl border-border bg-background/80 px-4"
                :disabled="logsLoading"
                @click="refreshLogs"
              >
                <RefreshCw
                  class="mr-2 h-4 w-4"
                  :class="{ 'animate-spin': logsLoading }"
                />
                查询/刷新
              </Button>
            </div>
          </template>
          <div class="mb-4 space-y-3">
            <div class="grid grid-cols-2 gap-2 lg:hidden">
              <Button
                variant="outline"
                class="h-10 rounded-xl border-border bg-background/80"
                :disabled="logsLoading"
                @click="refreshLogs"
              >
                <RefreshCw
                  class="mr-2 h-4 w-4"
                  :class="{ 'animate-spin': logsLoading }"
                />
                查询/刷新
              </Button>
              <Button
                variant="outline"
                class="h-10 rounded-xl border-border bg-background/80"
                :disabled="logsLoading || !hasActiveLogFilters"
                @click="resetLogFilters"
              >
                <ArchiveX class="mr-2 h-4 w-4" />
                重置
              </Button>
            </div>

            <div class="grid gap-3 lg:grid-cols-[minmax(130px,1fr)_minmax(130px,1fr)_minmax(130px,1fr)_minmax(170px,1fr)_minmax(170px,1fr)_minmax(170px,1fr)]">
              <Select v-model="logFlaggedFilter">
                <SelectTrigger class="h-11 rounded-xl">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="all">
                    全部结果
                  </SelectItem>
                  <SelectItem value="true">
                    仅命中
                  </SelectItem>
                  <SelectItem value="false">
                    未命中
                  </SelectItem>
                </SelectContent>
              </Select>

              <Select v-model="logFilters.decision_source">
                <SelectTrigger class="h-11 rounded-xl">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="all">
                    全部分组
                  </SelectItem>
                  <SelectItem value="keyword">
                    关键词
                  </SelectItem>
                  <SelectItem value="api">
                    Provider API
                  </SelectItem>
                  <SelectItem value="api_error">
                    Provider 异常
                  </SelectItem>
                  <SelectItem value="hash">
                    哈希命中
                  </SelectItem>
                  <SelectItem value="none">
                    无命中
                  </SelectItem>
                </SelectContent>
              </Select>

              <Select v-model="logFilters.endpoint">
                <SelectTrigger class="h-11 rounded-xl">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="all">
                    全部端点
                  </SelectItem>
                  <SelectItem
                    v-for="endpoint in logEndpointOptions"
                    :key="endpoint"
                    :value="endpoint"
                  >
                    {{ endpoint }}
                  </SelectItem>
                </SelectContent>
              </Select>

              <Input
                v-model="logSearchText"
                class="h-11 rounded-xl"
                placeholder="全量搜索用户/Key/摘要"
                @keydown.enter.prevent="refreshLogs"
              />

              <Input
                v-model="logDateFrom"
                class="h-11 rounded-xl"
                type="datetime-local"
                title="开始时间"
                @keydown.enter.prevent="refreshLogs"
              />
              <Input
                v-model="logDateTo"
                class="h-11 rounded-xl"
                type="datetime-local"
                title="结束时间"
                @keydown.enter.prevent="refreshLogs"
              />
            </div>
          </div>

          <div class="space-y-3 lg:hidden">
            <LoadingState
              v-if="logsLoading"
              variant="pulse"
              size="sm"
              message="正在拉取风控日志..."
            />
            <EmptyState
              v-else-if="visibleLogItems.length === 0"
              type="filter"
              size="sm"
              :icon="Activity"
              title="没有风控日志"
              description="当前筛选条件下没有记录。"
              action-text="重新查询"
              :action-icon="RefreshCw"
              action-variant="outline"
              @action="refreshLogs"
            />
            <template v-else>
              <article
                v-for="item in visibleLogItems"
                :key="item.id"
                class="rounded-2xl border border-border bg-background p-4 shadow-sm transition-colors active:bg-muted/40"
                role="button"
                tabindex="0"
                @click="openLogDetail(item)"
                @keydown.enter.prevent="openLogDetail(item)"
                @keydown.space.prevent="openLogDetail(item)"
              >
                <div class="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
                  <div class="min-w-0">
                    <p class="truncate text-sm font-semibold text-foreground">
                      {{ item.model || item.endpoint || item.api_format || '未记录模型' }}
                    </p>
                    <p class="mt-1 text-xs text-muted-foreground">
                      {{ formatDate(item.created_at) }}
                    </p>
                  </div>
                  <Badge
                    class="w-fit"
                    :variant="actionBadge(item.action)"
                  >
                    {{ item.action }}
                  </Badge>
                </div>

                <p class="mt-3 line-clamp-3 text-sm leading-6 text-muted-foreground">
                  {{ logExcerptText(item) }}
                </p>

                <div
                  v-if="item.matched_keywords?.length"
                  class="mt-3 flex flex-wrap gap-1"
                >
                  <Badge
                    v-for="keyword in item.matched_keywords.slice(0, 4)"
                    :key="keyword"
                    variant="warning"
                  >
                    {{ keyword }}
                  </Badge>
                </div>

                <div class="mt-4 grid grid-cols-2 gap-2 text-xs">
                  <div class="rounded-xl bg-muted/30 p-2">
                    <p class="text-muted-foreground">
                      来源
                    </p>
                    <p class="mt-1 font-medium text-foreground">
                      {{ item.decision_source }}
                    </p>
                  </div>
                  <div class="rounded-xl bg-muted/30 p-2">
                    <p class="text-muted-foreground">
                      结果
                    </p>
                    <p class="mt-1 font-medium text-foreground">
                      {{ item.flagged ? '已命中' : '未命中' }}
                    </p>
                  </div>
                  <div class="rounded-xl bg-muted/30 p-2">
                    <p class="text-muted-foreground">
                      用户 / Key
                    </p>
                    <p class="mt-1 truncate font-medium text-foreground">
                      {{ item.username || item.user_id || item.api_key_name || item.api_key_id || '-' }}
                    </p>
                  </div>
                  <div class="rounded-xl bg-muted/30 p-2">
                    <p class="text-muted-foreground">
                      分类 / 耗时
                    </p>
                    <p class="mt-1 font-medium text-foreground">
                      {{ item.highest_category || '-' }} · {{ formatLatency(item.latency_ms) }}
                    </p>
                  </div>
                  <div class="col-span-2 rounded-xl bg-muted/30 p-2">
                    <p class="text-muted-foreground">
                      通知
                    </p>
                    <p class="mt-1 break-words font-medium text-foreground">
                      {{ notificationStatusText(item) }}
                    </p>
                  </div>
                </div>
              </article>
            </template>
          </div>

          <div class="hidden overflow-x-auto rounded-lg border border-border lg:block">
            <table class="min-w-[1380px] w-full text-sm">
              <thead class="bg-muted/50 text-left text-xs text-muted-foreground">
                <tr>
                  <th class="px-3 py-2">
                    时间
                  </th>
                  <th class="px-3 py-2">
                    分组
                  </th>
                  <th class="px-3 py-2">
                    用户
                  </th>
                  <th class="px-3 py-2">
                    API KEY
                  </th>
                  <th class="px-3 py-2">
                    端点
                  </th>
                  <th class="px-3 py-2">
                    结果
                  </th>
                  <th class="px-3 py-2">
                    最高分
                  </th>
                  <th class="px-3 py-2">
                    处置
                  </th>
                  <th class="px-3 py-2">
                    上游耗时
                  </th>
                  <th class="px-3 py-2">
                    输入摘要
                  </th>
                </tr>
              </thead>
              <tbody>
                <tr v-if="logsLoading">
                  <td
                    colspan="10"
                    class="px-3 py-8"
                  >
                    <LoadingState
                      variant="pulse"
                      size="sm"
                      message="正在拉取风控日志..."
                    />
                  </td>
                </tr>
                <template v-else>
                  <tr
                    v-for="item in visibleLogItems"
                    :key="item.id"
                    class="cursor-pointer border-t border-border align-top transition-colors hover:bg-muted/35 focus:bg-muted/35"
                    role="button"
                    tabindex="0"
                    @click="openLogDetail(item)"
                    @keydown.enter.prevent="openLogDetail(item)"
                    @keydown.space.prevent="openLogDetail(item)"
                  >
                    <td class="whitespace-nowrap px-3 py-3 text-xs text-muted-foreground">
                      {{ formatDate(item.created_at) }}
                    </td>
                    <td class="px-3 py-3">
                      <div class="font-medium text-foreground">
                        {{ routeGroupText(item) }}
                      </div>
                      <div class="text-xs text-muted-foreground">
                        {{ item.decision_source }}
                      </div>
                    </td>
                    <td class="px-3 py-3">
                      <div class="font-medium text-foreground">
                        {{ userPrimaryText(item) }}
                      </div>
                      <div class="text-xs text-muted-foreground">
                        {{ userSecondaryText(item) }}
                      </div>
                    </td>
                    <td class="px-3 py-3">
                      <div class="font-medium text-foreground">
                        {{ apiKeyPrimaryText(item) }}
                      </div>
                      <div class="text-xs text-muted-foreground">
                        {{ apiKeySecondaryText(item) }}
                      </div>
                    </td>
                    <td class="px-3 py-3">
                      <div class="font-medium text-foreground">
                        {{ item.endpoint || '-' }}
                      </div>
                      <div class="font-mono text-xs text-foreground">
                        {{ item.model || '-' }}
                      </div>
                      <div class="text-xs text-muted-foreground">
                        {{ item.api_format || item.route_kind || '' }}
                      </div>
                    </td>
                    <td class="px-3 py-3">
                      <Badge :variant="logResultBadge(item)">
                        {{ logResultLabel(item) }}
                      </Badge>
                    </td>
                    <td class="px-3 py-3">
                      <div class="font-mono text-sm text-foreground">
                        {{ item.highest_category || '-' }}
                      </div>
                      <div class="text-xs text-muted-foreground">
                        {{ formatScore(item.highest_score) }}
                      </div>
                    </td>
                    <td class="px-3 py-3">
                      <Badge :variant="actionBadge(item.action)">
                        {{ item.action }}
                      </Badge>
                      <div class="mt-1 text-xs text-muted-foreground">
                        {{ item.auto_action || notificationStatusText(item) }}
                      </div>
                    </td>
                    <td class="whitespace-nowrap px-3 py-3">
                      <div class="font-mono text-sm text-foreground">
                        {{ formatLatency(item.latency_ms) }}
                      </div>
                      <div class="text-xs text-muted-foreground">
                        排队 {{ formatLatency(item.queue_delay_ms) }}
                      </div>
                    </td>
                    <td class="max-w-[360px] px-3 py-3">
                      <div class="flex items-start gap-2">
                        <div class="min-w-0 flex-1">
                          <div class="line-clamp-2 text-xs text-muted-foreground">
                            {{ logExcerptText(item) }}
                          </div>
                          <div
                            v-if="item.matched_keywords?.length"
                            class="mt-1 flex flex-wrap gap-1"
                          >
                            <Badge
                              v-for="keyword in item.matched_keywords.slice(0, 3)"
                              :key="keyword"
                              variant="warning"
                            >
                              {{ keyword }}
                            </Badge>
                          </div>
                        </div>
                        <Eye class="mt-0.5 h-3.5 w-3.5 shrink-0 text-muted-foreground/60" />
                      </div>
                    </td>
                  </tr>
                  <tr v-if="visibleLogItems.length === 0">
                    <td
                      colspan="10"
                      class="px-3 py-8"
                    >
                      <EmptyState
                        type="filter"
                        size="sm"
                        :icon="Activity"
                        title="没有风控日志"
                        description="当前筛选条件下没有记录。"
                        action-text="重新查询"
                        :action-icon="RefreshCw"
                        action-variant="outline"
                        @action="refreshLogs"
                      />
                    </td>
                  </tr>
                </template>
              </tbody>
            </table>
          </div>
          <Pagination
            v-if="logs.total > 0"
            v-model:current="logsPage"
            v-model:page-size="logsPageSize"
            :total="logs.total"
            cache-key="risk-control-logs-page-size"
          />
        </CardSection>
      </div>

      <Dialog
        v-model="logDetailDialogOpen"
        size="6xl"
        :title="logDetailTitle"
        :description="logDetailDescription"
        :icon="Activity"
      >
        <div
          v-if="selectedLog"
          class="max-h-[calc(100vh-10rem)] space-y-4 overflow-y-auto pr-1 sm:max-h-[72vh]"
        >
          <div class="grid gap-3 md:grid-cols-4">
            <div class="rounded-xl border border-border bg-muted/20 p-3">
              <p class="text-xs text-muted-foreground">
                动作
              </p>
              <Badge
                class="mt-2"
                :variant="actionBadge(selectedLog.action)"
              >
                {{ selectedLog.action }}
              </Badge>
            </div>
            <div class="rounded-xl border border-border bg-muted/20 p-3">
              <p class="text-xs text-muted-foreground">
                决策来源
              </p>
              <p class="mt-2 text-sm font-semibold text-foreground">
                {{ selectedLog.decision_source }}
              </p>
            </div>
            <div class="rounded-xl border border-border bg-muted/20 p-3">
              <p class="text-xs text-muted-foreground">
                命中状态
              </p>
              <Badge
                class="mt-2"
                :variant="selectedLog.flagged ? 'destructive' : 'secondary'"
              >
                {{ selectedLog.flagged ? '已命中' : '未命中' }}
              </Badge>
            </div>
            <div class="rounded-xl border border-border bg-muted/20 p-3">
              <p class="text-xs text-muted-foreground">
                耗时
              </p>
              <p class="mt-2 text-sm font-semibold text-foreground">
                {{ formatLatency(selectedLog.latency_ms) }}
              </p>
            </div>
          </div>

          <div class="grid gap-4 lg:grid-cols-[minmax(0,1fr)_280px]">
            <div class="space-y-4">
              <section class="rounded-xl border border-border bg-background p-4">
                <div class="mb-3 flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between sm:gap-3">
                  <div>
                    <h4 class="text-sm font-semibold text-foreground">
                      输入摘要
                    </h4>
                    <p class="text-xs text-muted-foreground">
                      仅展示截断摘要，不展示完整用户原文。
                    </p>
                  </div>
                  <Badge
                    variant="outline"
                    class="w-fit"
                  >
                    {{ selectedLog.api_format || selectedLog.route_family || 'unknown' }}
                  </Badge>
                </div>
                <p class="whitespace-pre-wrap rounded-lg bg-muted/30 p-3 text-sm leading-6 text-foreground">
                  {{ logExcerptText(selectedLog) }}
                </p>
              </section>

              <section class="rounded-xl border border-border bg-background p-4">
                <h4 class="text-sm font-semibold text-foreground">
                  分类分数
                </h4>
                <div
                  v-if="selectedLogScoreRows.length"
                  class="mt-3 space-y-3"
                >
                  <div
                    v-for="[category, score] in selectedLogScoreRows"
                    :key="category"
                    class="space-y-1.5"
                  >
                    <div class="flex items-center justify-between gap-3 text-xs">
                      <span class="font-medium text-foreground">{{ category }}</span>
                      <span class="font-mono text-muted-foreground">{{ formatScore(score) }}</span>
                    </div>
                    <div class="h-2 overflow-hidden rounded-full bg-muted">
                      <div
                        class="h-full rounded-full bg-orange-500"
                        :style="{ width: scorePercent(score) }"
                      />
                    </div>
                  </div>
                </div>
                <EmptyState
                  v-else
                  type="empty"
                  size="sm"
                  :icon="ShieldCheck"
                  title="没有分类分数"
                  description="本地规则命中时不一定包含 Provider 分类分数。"
                />
              </section>

              <section class="rounded-xl border border-border bg-background p-4">
                <div class="mb-3 flex items-center justify-between gap-3">
                  <h4 class="text-sm font-semibold text-foreground">
                    原始审计记录
                  </h4>
                  <Badge variant="outline">
                    JSON
                  </Badge>
                </div>
                <pre class="max-h-64 overflow-auto rounded-lg border border-border bg-muted/30 p-3 text-xs leading-5 text-muted-foreground">{{ formatLogJson(selectedLog) }}</pre>
              </section>
            </div>

            <aside class="space-y-4">
              <section class="rounded-xl border border-border bg-background p-4">
                <h4 class="text-sm font-semibold text-foreground">
                  请求上下文
                </h4>
                <dl class="risk-detail-list mt-3 space-y-2 text-xs">
                  <div class="flex justify-between gap-3">
                    <dt class="text-muted-foreground">
                      时间
                    </dt>
                    <dd class="text-right text-foreground">
                      {{ formatDate(selectedLog.created_at) }}
                    </dd>
                  </div>
                  <div class="flex justify-between gap-3">
                    <dt class="text-muted-foreground">
                      用户
                    </dt>
                    <dd class="text-right text-foreground">
                      {{ selectedLog.username || selectedLog.user_email || selectedLog.user_id || '-' }}
                    </dd>
                  </div>
                  <div class="flex justify-between gap-3">
                    <dt class="text-muted-foreground">
                      API Key
                    </dt>
                    <dd class="text-right text-foreground">
                      {{ selectedLog.api_key_name || selectedLog.api_key_id || '-' }}
                    </dd>
                  </div>
                  <div class="flex justify-between gap-3">
                    <dt class="text-muted-foreground">
                      模型
                    </dt>
                    <dd class="text-right font-mono text-foreground">
                      {{ selectedLog.model || '-' }}
                    </dd>
                  </div>
                  <div class="flex justify-between gap-3">
                    <dt class="text-muted-foreground">
                      端点
                    </dt>
                    <dd class="text-right text-foreground">
                      {{ selectedLog.endpoint || '-' }}
                    </dd>
                  </div>
                  <div class="flex justify-between gap-3">
                    <dt class="text-muted-foreground">
                      Trace
                    </dt>
                    <dd class="text-right font-mono text-foreground">
                      {{ selectedLog.trace_id || selectedLog.request_id || '-' }}
                    </dd>
                  </div>
                  <div class="flex justify-between gap-3">
                    <dt class="text-muted-foreground">
                      队列延迟
                    </dt>
                    <dd class="text-right text-foreground">
                      {{ formatLatency(selectedLog.queue_delay_ms) }}
                    </dd>
                  </div>
                </dl>
              </section>

              <section class="rounded-xl border border-border bg-background p-4">
                <h4 class="text-sm font-semibold text-foreground">
                  命中线索
                </h4>
                <div class="mt-3 space-y-3">
                  <div>
                    <p class="text-xs text-muted-foreground">
                      最高分类
                    </p>
                    <p class="mt-1 text-sm font-semibold text-foreground">
                      {{ selectedLog.highest_category || '-' }}
                      <span class="font-mono text-xs text-muted-foreground">
                        {{ formatScore(selectedLog.highest_score) }}
                      </span>
                    </p>
                  </div>
                  <div>
                    <p class="text-xs text-muted-foreground">
                      匹配关键词
                    </p>
                    <div
                      v-if="selectedLog.matched_keywords?.length"
                      class="mt-2 flex flex-wrap gap-1"
                    >
                      <Badge
                        v-for="keyword in selectedLog.matched_keywords"
                        :key="keyword"
                        variant="warning"
                      >
                        {{ keyword }}
                      </Badge>
                    </div>
                    <p
                      v-else
                      class="mt-1 text-xs text-muted-foreground"
                    >
                      无关键词命中
                    </p>
                  </div>
                  <div>
                    <p class="text-xs text-muted-foreground">
                      阈值快照
                    </p>
                    <div
                      v-if="selectedLogThresholdRows.length"
                      class="mt-2 flex flex-wrap gap-1"
                    >
                      <Badge
                        v-for="[category, threshold] in selectedLogThresholdRows"
                        :key="category"
                        variant="outline"
                      >
                        {{ category }} ≥ {{ formatScore(threshold) }}
                      </Badge>
                    </div>
                    <p
                      v-else
                      class="mt-1 text-xs text-muted-foreground"
                    >
                      没有阈值快照
                    </p>
                  </div>
                </div>
              </section>

              <section class="rounded-xl border border-border bg-background p-4">
                <h4 class="text-sm font-semibold text-foreground">
                  哈希指纹
                </h4>
                <div
                  v-if="selectedLog.input_hash"
                  class="mt-3 space-y-3"
                >
                  <code class="block break-all rounded-lg border border-border bg-muted/30 p-3 text-xs text-foreground">
                    {{ selectedLog.input_hash }}
                  </code>
                  <Button
                    variant="outline"
                    size="sm"
                    class="w-full text-destructive"
                    :disabled="dangerDialogLoading"
                    @click="deleteSelectedLogHash"
                  >
                    <Trash2 class="mr-2 h-4 w-4" />
                    删除此哈希
                  </Button>
                </div>
                <p
                  v-else
                  class="mt-2 text-xs text-muted-foreground"
                >
                  这条日志没有学习哈希。
                </p>
              </section>

              <section class="rounded-xl border border-border bg-background p-4">
                <h4 class="text-sm font-semibold text-foreground">
                  自动处置
                </h4>
                <dl class="risk-detail-list mt-3 space-y-2 text-xs">
                  <div class="flex justify-between gap-3">
                    <dt class="text-muted-foreground">
                      违规次数
                    </dt>
                    <dd class="text-foreground">
                      {{ selectedLog.violation_count }}
                    </dd>
                  </div>
                  <div class="flex justify-between gap-3">
                    <dt class="text-muted-foreground">
                      处置动作
                    </dt>
                    <dd class="text-foreground">
                      {{ selectedLog.auto_action || '-' }}
                    </dd>
                  </div>
                  <div class="flex justify-between gap-3">
                    <dt class="text-muted-foreground">
                      通知
                    </dt>
                    <dd class="text-foreground">
                      {{ notificationStatusText(selectedLog) }}
                    </dd>
                  </div>
                  <div class="flex justify-between gap-3">
                    <dt class="text-muted-foreground">
                      通知尝试
                    </dt>
                    <dd class="text-foreground">
                      {{ selectedLog.notification_attempts }}
                    </dd>
                  </div>
                  <div class="flex justify-between gap-3">
                    <dt class="text-muted-foreground">
                      最后尝试
                    </dt>
                    <dd class="text-right text-foreground">
                      {{ selectedLog.notification_last_attempt_at || '-' }}
                    </dd>
                  </div>
                  <div
                    v-if="selectedLog.notification_outbox"
                    class="flex justify-between gap-3"
                  >
                    <dt class="text-muted-foreground">
                      下次发送
                    </dt>
                    <dd class="text-right text-foreground">
                      {{ selectedLog.notification_outbox.next_attempt_at || '-' }}
                    </dd>
                  </div>
                  <div
                    v-if="selectedLog.notification_outbox"
                    class="flex justify-between gap-3"
                  >
                    <dt class="text-muted-foreground">
                      Outbox
                    </dt>
                    <dd class="text-right font-mono text-foreground">
                      {{ selectedLog.notification_outbox.status }}
                    </dd>
                  </div>
                  <div
                    v-if="notificationOutboxes(selectedLog).length > 1"
                    class="grid gap-2"
                  >
                    <dt class="text-muted-foreground">
                      通知任务
                    </dt>
                    <dd class="grid gap-1 text-right text-xs text-foreground">
                      <span
                        v-for="outbox in notificationOutboxes(selectedLog)"
                        :key="outbox.id"
                        class="break-all font-mono leading-5"
                      >
                        {{ outbox.item_key }} · {{ outbox.status }}
                      </span>
                    </dd>
                  </div>
                </dl>
                <p
                  v-if="notificationErrorText(selectedLog)"
                  class="mt-3 rounded-lg border border-amber-200 bg-amber-50 px-3 py-2 text-xs leading-5 text-amber-800 dark:border-amber-300/25 dark:bg-amber-400/10 dark:text-amber-100"
                >
                  {{ notificationErrorText(selectedLog) }}
                </p>
                <Button
                  v-if="canRetryNotification(selectedLog)"
                  variant="outline"
                  size="sm"
                  class="mt-3 w-full justify-center"
                  :disabled="retryingNotification"
                  @click="retrySelectedNotification"
                >
                  <RefreshCw
                    class="mr-2 h-4 w-4"
                    :class="{ 'animate-spin': retryingNotification }"
                  />
                  {{ retryingNotification ? '入队中...' : '重新发送通知' }}
                </Button>
                <div
                  v-if="selectedLog.auto_action_enforced"
                  class="mt-3 grid gap-2"
                >
                  <Button
                    v-if="selectedLog.auto_action === 'disable_user' && selectedLog.user_id"
                    variant="outline"
                    size="sm"
                    class="justify-center"
                    :disabled="restoringAutoAction !== null"
                    @click="restoreSelectedUser"
                  >
                    <UserCheck class="mr-2 h-4 w-4" />
                    {{ restoringAutoAction === 'user' ? '恢复中...' : '恢复用户' }}
                  </Button>
                  <Button
                    v-if="selectedLog.auto_action === 'lock_api_key' && selectedLog.user_id && selectedLog.api_key_id"
                    variant="outline"
                    size="sm"
                    class="justify-center"
                    :disabled="restoringAutoAction !== null"
                    @click="unlockSelectedApiKey"
                  >
                    <KeyRound class="mr-2 h-4 w-4" />
                    {{ restoringAutoAction === 'api_key' ? '解锁中...' : '解锁 Key' }}
                  </Button>
                </div>
              </section>
            </aside>
          </div>
        </div>
        <template #footer>
          <Button
            variant="outline"
            @click="closeLogDetail"
          >
            关闭
          </Button>
        </template>
      </Dialog>

      <Dialog
        v-model="configDialogOpen"
        size="7xl"
        title="风控配置"
        description="集中管理策略、Provider、处置与保留规则。"
        :icon="ShieldAlert"
      >
        <div class="max-h-[calc(100vh-10rem)] overflow-y-auto pr-1 sm:max-h-[72vh]">
          <Tabs v-model="configTab">
            <TabsList class="risk-config-tabs flex w-full flex-wrap justify-center gap-2">
              <TabsTrigger value="basic">
                基础
              </TabsTrigger>
              <TabsTrigger value="scope">
                范围
              </TabsTrigger>
              <TabsTrigger value="provider">
                Provider
              </TabsTrigger>
              <TabsTrigger value="response">
                响应
              </TabsTrigger>
              <TabsTrigger value="keywords">
                关键词
              </TabsTrigger>
              <TabsTrigger value="retention">
                保留
              </TabsTrigger>
              <TabsTrigger value="test">
                测试
              </TabsTrigger>
            </TabsList>

            <TabsContent
              value="basic"
              class="space-y-4"
            >
              <CardSection padding="none">
                <div class="overflow-hidden rounded-2xl border border-border/80 bg-background shadow-sm">
                  <div class="border-b border-border/70 bg-muted/20 p-4">
                    <div class="flex flex-col gap-4 xl:flex-row xl:items-start xl:justify-between">
                      <div class="flex min-w-0 items-start justify-between gap-4 xl:max-w-[420px]">
                        <div class="min-w-0">
                          <div class="flex items-center gap-2">
                            <span
                              class="h-2.5 w-2.5 rounded-full"
                              :class="decisionStatusDotClass"
                            />
                            <p class="text-xs font-semibold uppercase tracking-[0.22em] text-muted-foreground">
                              Guardrail
                            </p>
                          </div>
                          <h3 class="mt-2 text-lg font-semibold tracking-tight text-foreground">
                            {{ decisionStatusTitle }}
                          </h3>
                          <p class="mt-1 truncate text-xs text-muted-foreground">
                            {{ decisionStatusDetail }}
                          </p>
                        </div>
                        <div class="flex shrink-0 items-center gap-2 rounded-full border border-border bg-background px-3 py-2">
                          <span class="text-xs font-medium text-muted-foreground">启用</span>
                          <Switch
                            :model-value="config.enabled"
                            @update:model-value="(value: boolean) => config.enabled = value"
                          />
                        </div>
                      </div>

                      <div class="grid flex-1 gap-2 sm:grid-cols-2 xl:grid-cols-5">
                        <div
                          v-for="chip in strategyChips"
                          :key="chip.label"
                          class="min-w-0 rounded-xl border border-border bg-background px-3 py-2"
                        >
                          <p class="text-[11px] text-muted-foreground">
                            {{ chip.label }}
                          </p>
                          <p
                            class="mt-1 truncate text-sm font-semibold text-foreground"
                            :class="chip.danger ? 'text-destructive' : ''"
                          >
                            {{ chip.value }}
                          </p>
                        </div>
                      </div>
                    </div>
                  </div>

                  <div class="grid gap-4 p-4 xl:grid-cols-2">
                    <div class="rounded-2xl border border-border bg-card p-3 shadow-sm">
                      <div class="flex items-center justify-between gap-3 px-1">
                        <div class="min-w-0">
                          <p class="text-sm font-semibold text-foreground">
                            运行模式
                          </p>
                          <p class="mt-0.5 text-xs text-muted-foreground">
                            Off / Observe / Block 三选一。
                          </p>
                        </div>
                        <Badge
                          variant="outline"
                          class="shrink-0 bg-background"
                        >
                          {{ riskModeText(config.mode) }}
                        </Badge>
                      </div>

                      <div class="mt-3 grid gap-2">
                        <button
                          v-for="option in riskModeCards"
                          :key="option.value"
                          type="button"
                          class="group flex min-h-[64px] items-center gap-3 rounded-xl border px-3 py-2 text-left transition-all"
                          :class="modeCardClass(option.value)"
                          :aria-pressed="config.mode === option.value"
                          @click="config.mode = option.value"
                        >
                          <span
                            class="flex h-9 w-9 shrink-0 items-center justify-center rounded-xl border text-[11px] font-semibold uppercase tracking-[0.08em]"
                            :class="config.mode === option.value ? 'border-primary/30 bg-primary text-primary-foreground' : 'border-border bg-muted/40 text-muted-foreground'"
                          >
                            {{ option.eyebrow.slice(0, 3) }}
                          </span>
                          <span class="min-w-0 flex-1">
                            <span class="flex items-center justify-between gap-3">
                              <span class="text-sm font-semibold text-foreground">
                                {{ option.label }}
                              </span>
                              <span
                                class="flex h-5 w-5 shrink-0 items-center justify-center rounded-full border text-[11px]"
                                :class="config.mode === option.value ? 'border-primary bg-primary text-primary-foreground' : 'border-border text-transparent group-hover:text-muted-foreground'"
                              >
                                ✓
                              </span>
                            </span>
                            <span class="mt-0.5 block truncate text-xs text-muted-foreground">
                              {{ option.description }}
                            </span>
                          </span>
                        </button>
                      </div>
                    </div>

                    <div class="rounded-2xl border border-border bg-card p-3 shadow-sm">
                      <div class="flex items-center justify-between gap-3 px-1">
                        <div class="min-w-0">
                          <p class="text-sm font-semibold text-foreground">
                            审核链路
                          </p>
                          <p class="mt-0.5 text-xs text-muted-foreground">
                            Keyword 与 Provider 的参与顺序。
                          </p>
                        </div>
                        <Badge
                          variant="outline"
                          class="shrink-0 bg-background"
                        >
                          {{ keywordModeText(config.keyword_mode) }}
                        </Badge>
                      </div>

                      <div class="mt-3 grid gap-2">
                        <button
                          v-for="option in keywordModeCards"
                          :key="option.value"
                          type="button"
                          class="group flex min-h-[64px] items-center gap-3 rounded-xl border px-3 py-2 text-left transition-all"
                          :class="keywordModeCardClass(option.value)"
                          :aria-pressed="config.keyword_mode === option.value"
                          @click="config.keyword_mode = option.value"
                        >
                          <span
                            class="flex h-9 w-9 shrink-0 items-center justify-center rounded-xl border text-[11px] font-semibold uppercase tracking-[0.08em]"
                            :class="config.keyword_mode === option.value ? 'border-primary/30 bg-primary text-primary-foreground' : 'border-border bg-muted/40 text-muted-foreground'"
                          >
                            {{ option.eyebrow.slice(0, 3) }}
                          </span>
                          <span class="min-w-0 flex-1">
                            <span class="flex items-center justify-between gap-3">
                              <span class="text-sm font-semibold text-foreground">
                                {{ option.label }}
                              </span>
                              <span
                                class="flex h-5 w-5 shrink-0 items-center justify-center rounded-full border text-[11px]"
                                :class="config.keyword_mode === option.value ? 'border-primary bg-primary text-primary-foreground' : 'border-border text-transparent group-hover:text-muted-foreground'"
                              >
                                ✓
                              </span>
                            </span>
                            <span class="mt-0.5 block truncate text-xs text-muted-foreground">
                              {{ option.description }}
                            </span>
                          </span>
                        </button>
                      </div>
                    </div>
                  </div>

                  <div class="border-t border-border/70 bg-muted/20 p-4">
                    <div class="flex flex-col gap-3 xl:flex-row xl:items-center">
                      <div class="flex items-center gap-2 text-sm font-semibold text-foreground xl:w-28">
                        <Activity class="h-4 w-4 text-primary" />
                        生效顺序
                      </div>
                      <div class="grid flex-1 gap-2 sm:grid-cols-2 xl:grid-cols-5">
                        <div
                          v-for="(step, index) in decisionFlowSteps"
                          :key="step.label"
                          class="min-w-0 rounded-xl border px-3 py-2"
                          :class="step.active ? 'border-primary/30 bg-primary/10' : 'border-border bg-background'"
                        >
                          <div class="flex min-w-0 items-center gap-2">
                            <span
                              class="flex h-5 w-5 shrink-0 items-center justify-center rounded-full text-[10px] font-semibold"
                              :class="step.active ? 'bg-primary text-primary-foreground' : 'bg-muted text-muted-foreground'"
                            >
                              {{ index + 1 }}
                            </span>
                            <p class="truncate text-xs font-semibold text-foreground">
                              {{ step.label }}
                            </p>
                          </div>
                          <p class="mt-1 truncate pl-7 text-[11px] text-muted-foreground">
                            {{ step.value }}
                          </p>
                        </div>
                      </div>
                    </div>
                  </div>
                </div>
              </CardSection>
            </TabsContent>

            <TabsContent
              value="scope"
              class="space-y-6"
            >
              <CardSection
                title="模型范围"
                description="按模型名称限定风控生效范围。"
              >
                <div class="mb-4 flex flex-col gap-3 rounded-xl border border-border bg-muted/20 p-4 lg:flex-row lg:items-center lg:justify-between">
                  <div>
                    <p class="text-sm font-semibold text-foreground">
                      {{ modelFilterSummary }}
                    </p>
                    <p class="mt-1 text-xs text-muted-foreground">
                      当前模式会影响哪些模型进入风控检测。
                    </p>
                  </div>
                  <Badge
                    variant="outline"
                    class="w-fit bg-background/70"
                  >
                    {{ parseLines(modelFilterModelsText).length }} 个模型
                  </Badge>
                </div>
                <div class="grid gap-4 lg:grid-cols-[320px_minmax(0,1fr)]">
                  <FieldBox label="范围模式">
                    <Select v-model="config.model_filter.mode">
                      <SelectTrigger><SelectValue /></SelectTrigger>
                      <SelectContent>
                        <SelectItem value="all">
                          全部模型
                        </SelectItem>
                        <SelectItem value="include">
                          仅审核这些模型
                        </SelectItem>
                        <SelectItem value="exclude">
                          排除这些模型
                        </SelectItem>
                      </SelectContent>
                    </Select>
                  </FieldBox>
                  <FieldBox label="模型列表">
                    <Textarea
                      v-model="modelFilterModelsText"
                      class="min-h-[180px] font-mono text-xs"
                      :disabled="config.model_filter.mode === 'all'"
                      placeholder="每行一个模型，例如：model-large"
                    />
                    <p class="mt-2 text-xs text-muted-foreground">
                      {{ modelFilterModeText(config.model_filter.mode) }}；每行一个模型名，大小写不敏感。
                    </p>
                  </FieldBox>
                </div>
              </CardSection>

              <CardSection
                title="策略粒度"
                description="按用户、Key 与路由维度限定策略生效范围。"
              >
                <div class="mb-4 flex flex-col gap-3 rounded-xl border border-border bg-muted/20 p-4 lg:flex-row lg:items-center lg:justify-between">
                  <div>
                    <p class="text-sm font-semibold text-foreground">
                      {{ scopeSummary }}
                    </p>
                    <p class="mt-1 text-xs text-muted-foreground">
                      include / exclude 需要至少一条值。
                    </p>
                  </div>
                  <Badge
                    variant="outline"
                    class="w-fit bg-background/70"
                  >
                    {{ activeScopeRuleCount }} 组规则
                  </Badge>
                </div>

                <div class="grid min-w-0 gap-4 md:grid-cols-2 xl:grid-cols-3">
                  <div
                    v-for="group in scopeGroups"
                    :key="group.key"
                    class="min-w-0 rounded-xl border border-border bg-card p-4 shadow-sm"
                  >
                    <div class="flex min-w-0 items-center justify-between gap-3">
                      <div class="min-w-0">
                        <p class="text-sm font-semibold text-foreground">
                          {{ group.label }}
                        </p>
                        <p class="mt-1 truncate text-xs text-muted-foreground">
                          {{ scopeModeText(config.scope[group.key].mode, group.label, scopeValueCount(group.key)) }}
                        </p>
                      </div>
                      <Badge
                        variant="outline"
                        class="shrink-0 bg-background/70"
                      >
                        {{ scopeValueCount(group.key) }}
                      </Badge>
                    </div>

                    <div class="mt-3 grid min-w-0 gap-3">
                      <Select v-model="config.scope[group.key].mode">
                        <SelectTrigger class="w-full">
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          <SelectItem value="all">
                            全部
                          </SelectItem>
                          <SelectItem value="include">
                            仅包含
                          </SelectItem>
                          <SelectItem value="exclude">
                            排除
                          </SelectItem>
                        </SelectContent>
                      </Select>

                      <Textarea
                        v-model="scopeValuesText[group.key]"
                        class="min-h-[132px] min-w-0 font-mono text-xs"
                        :disabled="config.scope[group.key].mode === 'all'"
                        :placeholder="group.placeholder"
                      />
                    </div>
                  </div>
                </div>
              </CardSection>
            </TabsContent>

            <TabsContent
              value="provider"
              class="space-y-6"
            >
              <CardSection
                title="Moderation Provider"
                description="配置审核接口、Key 轮询与失败策略。"
              >
                <div class="mb-4 rounded-xl border border-emerald-500/20 bg-emerald-500/10 p-4">
                  <div class="flex items-center gap-2 text-sm font-semibold text-emerald-700 dark:text-emerald-200">
                    <ShieldCheck class="h-4 w-4" />Provider 安全边界已启用
                  </div>
                  <p class="mt-2 text-xs leading-5 text-emerald-700/80 dark:text-emerald-100/80">
                    仅允许 HTTPS 公网地址，拒绝内网 DNS 目标，审核请求会禁用重定向并固定到已校验地址。
                  </p>
                </div>
                <div class="grid gap-4 lg:grid-cols-2">
                  <FieldBox label="Base URL">
                    <Input
                      v-model="config.provider.base_url"
                      placeholder="https://api.openai.com"
                    />
                  </FieldBox>
                  <FieldBox label="模型">
                    <Input
                      v-model="config.provider.model"
                      placeholder="omni-moderation-latest"
                    />
                  </FieldBox>
                </div>
                <div class="mt-4 grid gap-4 lg:grid-cols-[minmax(0,1fr)_320px]">
                  <FieldBox label="API Keys">
                    <div class="mb-3 flex flex-wrap items-center gap-2">
                      <Badge
                        variant="outline"
                        class="bg-background/70"
                      >
                        {{ providerKeyInputCount }} 总数
                      </Badge>
                      <Badge
                        variant="outline"
                        class="border-emerald-500/25 bg-emerald-500/10 text-emerald-700 dark:text-emerald-200"
                      >
                        {{ plainProviderKeyInputCount }} 明文
                      </Badge>
                      <Badge
                        variant="outline"
                        class="border-amber-500/25 bg-amber-500/10 text-amber-700 dark:text-amber-200"
                      >
                        {{ maskedProviderKeyInputCount }} 脱敏
                      </Badge>
                    </div>
                    <Textarea
                      ref="apiKeysTextareaRef"
                      v-model="apiKeysText"
                      class="min-h-[180px] font-mono text-xs"
                      placeholder="每行一个 API Key"
                    />
                    <div class="mt-3 rounded-xl border border-border bg-muted/25 p-3">
                      <div class="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
                        <div>
                          <p class="text-sm font-semibold text-foreground">
                            写入模式
                          </p>
                          <p class="mt-1 text-xs leading-5 text-muted-foreground">
                            {{ providerKeyWriteModeHint }}
                          </p>
                        </div>
                        <div class="inline-flex w-full rounded-xl border border-border bg-background p-1 sm:w-fit">
                          <Button
                            size="sm"
                            :variant="apiKeysWriteMode === 'append' ? 'default' : 'ghost'"
                            class="h-10 flex-1 rounded-lg px-3 sm:flex-none"
                            @click="apiKeysWriteMode = 'append'"
                          >
                            追加
                          </Button>
                          <Button
                            size="sm"
                            :variant="apiKeysWriteMode === 'replace' ? 'default' : 'ghost'"
                            class="h-10 flex-1 rounded-lg px-3 sm:flex-none"
                            @click="apiKeysWriteMode = 'replace'"
                          >
                            替换
                          </Button>
                        </div>
                      </div>
                      <p class="mt-3 text-xs leading-5 text-muted-foreground">
                        脱敏 Key 仅用于保留已保存密钥；测试 Provider 时不会把 <span class="font-mono">****</span> 当成明文发送。
                      </p>
                    </div>
                  </FieldBox>
                  <div class="space-y-3 rounded-lg border border-border bg-muted/25 p-4">
                    <div class="grid gap-3 sm:grid-cols-2">
                      <FieldBox label="超时 ms">
                        <Input
                          :model-value="String(config.provider.timeout_ms)"
                          type="number"
                          min="500"
                          @update:model-value="value => config.provider.timeout_ms = parseInteger(value, 8000)"
                        />
                      </FieldBox>
                      <FieldBox label="重试">
                        <Input
                          :model-value="String(config.provider.max_retries)"
                          type="number"
                          min="0"
                          max="8"
                          @update:model-value="value => config.provider.max_retries = parseInteger(value, 2)"
                        />
                      </FieldBox>
                      <FieldBox label="冻结秒数">
                        <Input
                          :model-value="String(config.provider.key_freeze_seconds)"
                          type="number"
                          min="0"
                          @update:model-value="value => config.provider.key_freeze_seconds = parseInteger(value, 300)"
                        />
                      </FieldBox>
                      <FieldBox label="失败拦截">
                        <div class="flex h-11 items-center justify-between rounded-xl border border-border/60 bg-background px-4">
                          <span class="text-sm text-muted-foreground">
                            Fail closed
                          </span>
                          <Switch
                            :model-value="config.provider.fail_closed"
                            @update:model-value="(value: boolean) => config.provider.fail_closed = value"
                          />
                        </div>
                      </FieldBox>
                    </div>
                    <Button
                      variant="outline"
                      class="w-full"
                      :disabled="testingProvider"
                      @click="testProvider"
                    >
                      <FlaskConical class="mr-2 h-4 w-4" />{{ testingProvider ? '测试中...' : '测试 Provider' }}
                    </Button>
                  </div>
                </div>
              </CardSection>

              <CardSection
                title="Provider Key 状态"
                description="展示 Key 可用状态、冻结时间和最近错误。"
              >
                <div
                  v-if="providerKeyRows.length === 0"
                  class="rounded-xl border border-dashed border-border bg-muted/20 p-6"
                >
                  <div class="mx-auto flex h-12 w-12 items-center justify-center rounded-full border border-border bg-background shadow-sm">
                    <FlaskConical class="h-5 w-5 text-primary" />
                  </div>
                  <div class="mt-3 text-center">
                    <p class="text-sm font-semibold text-foreground">
                      还没有 Key 健康记录
                    </p><p class="mt-1 text-xs leading-5 text-muted-foreground">
                      测试或真实调用后会展示 Key 状态。
                    </p>
                  </div>
                  <p
                    v-if="providerKeyInputCount === 0"
                    class="mt-3 text-center text-xs text-muted-foreground"
                  >
                    未配置 API Key 时，建议切换为「仅关键词」模式。
                  </p>
                </div>
                <div
                  v-else
                  class="space-y-2"
                >
                  <div
                    v-for="row in providerKeyRows"
                    :key="row.key_hash || row.masked || row.index"
                    class="rounded-xl border border-border bg-background p-3"
                  >
                    <div class="flex flex-wrap items-start justify-between gap-3">
                      <div class="min-w-0">
                        <div class="flex min-w-0 items-center gap-2">
                          <span class="truncate font-mono text-sm text-foreground">{{ row.masked || 'Key #' + (row.index + 1) }}</span><Badge :variant="providerKeyStatusBadge(row.status)">
                            {{ providerKeyStatusLabel(row.status) }}
                          </Badge>
                        </div>
                        <p class="mt-1 text-xs text-muted-foreground">
                          {{ providerKeyMeta(row) }}
                        </p>
                        <p
                          v-if="row.last_error"
                          class="mt-2 line-clamp-2 rounded-lg border border-destructive/20 bg-destructive/10 px-2 py-1 text-xs text-destructive"
                        >
                          {{ row.last_error }}
                        </p>
                      </div>
                      <span class="font-mono text-[11px] text-muted-foreground">{{ row.key_hash ? row.key_hash.slice(0, 10) : '-' }}</span>
                    </div>
                  </div>
                </div>
              </CardSection>
            </TabsContent>

            <TabsContent
              value="response"
              class="space-y-5"
            >
              <CardSection
                title="拦截与采样"
                description="控制拦截响应、请求裁剪和日志采样。"
              >
                <div class="grid gap-4 xl:grid-cols-[minmax(0,1fr)_340px]">
                  <div class="space-y-4">
                    <div class="rounded-xl border border-border bg-background p-4">
                      <div class="mb-4 flex items-start justify-between gap-3">
                        <div>
                          <p class="text-sm font-semibold text-foreground">
                            拦截响应
                          </p>
                          <p class="mt-1 text-xs text-muted-foreground">
                            命中前置拦截时返回给客户端。
                          </p>
                        </div>
                        <Badge
                          variant="outline"
                          class="shrink-0 bg-muted/40"
                        >
                          HTTP {{ config.block_status }}
                        </Badge>
                      </div>
                      <div class="grid gap-4 md:grid-cols-[180px_minmax(0,1fr)]">
                        <FieldBox label="拦截状态码">
                          <Input
                            :model-value="String(config.block_status)"
                            type="number"
                            min="400"
                            max="499"
                            @update:model-value="value => config.block_status = parseInteger(value, 400)"
                          />
                        </FieldBox>
                        <FieldBox label="拦截提示">
                          <Input
                            v-model="config.block_message"
                            maxlength="160"
                          />
                        </FieldBox>
                      </div>
                    </div>

                    <div class="rounded-xl border border-border bg-background p-4">
                      <div class="mb-4">
                        <p class="text-sm font-semibold text-foreground">
                          请求裁剪
                        </p>
                        <p class="mt-1 text-xs text-muted-foreground">
                          控制送审文本长度和日志摘要长度。
                        </p>
                      </div>
                      <div class="grid gap-4 md:grid-cols-2">
                        <FieldBox label="最大审核字符">
                          <Input
                            :model-value="String(config.max_text_chars)"
                            type="number"
                            min="256"
                            @update:model-value="value => config.max_text_chars = parseInteger(value, 65536)"
                          />
                        </FieldBox>
                        <FieldBox label="日志摘要字符">
                          <Input
                            :model-value="String(config.excerpt_chars)"
                            type="number"
                            min="64"
                            @update:model-value="value => config.excerpt_chars = parseInteger(value, 512)"
                          />
                        </FieldBox>
                      </div>
                    </div>
                  </div>

                  <div class="rounded-xl border border-border bg-muted/20 p-4">
                    <div class="mb-4 flex items-start justify-between gap-3">
                      <div>
                        <p class="text-sm font-semibold text-foreground">
                          日志采样
                        </p>
                        <p class="mt-1 text-xs text-muted-foreground">
                          决定多少请求进入审计记录。
                        </p>
                      </div>
                      <Badge
                        variant="outline"
                        class="shrink-0 bg-background"
                      >
                        {{ formatPercent(config.sample_rate) }}
                      </Badge>
                    </div>
                    <FieldBox label="采样率">
                      <Input
                        :model-value="String(config.sample_rate)"
                        type="number"
                        step="0.01"
                        min="0"
                        max="1"
                        @update:model-value="value => config.sample_rate = parseNumber(value, 1)"
                      />
                    </FieldBox>
                    <div class="mt-4 flex items-center justify-between gap-4 rounded-xl border border-border bg-background px-3 py-3">
                      <div class="min-w-0">
                        <p class="text-sm font-medium text-foreground">
                          记录所有检查
                        </p>
                        <p class="mt-0.5 text-xs text-muted-foreground">
                          包含未命中请求
                        </p>
                      </div>
                      <Switch
                        :model-value="config.log_all"
                        @update:model-value="(value: boolean) => config.log_all = value"
                      />
                    </div>
                  </div>
                </div>
              </CardSection>

              <CardSection>
                <template #header>
                  <div class="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
                    <div>
                      <h3 class="text-lg font-medium leading-6 text-foreground">
                        自动处置
                      </h3>
                      <p class="mt-1 text-sm text-muted-foreground">
                        同一用户在窗口期内多次命中后自动禁用用户或锁定 Key。
                      </p>
                    </div>
                    <div class="flex shrink-0 items-center gap-3 rounded-full border border-border bg-background px-3 py-2">
                      <span class="text-sm font-medium text-foreground">启用</span>
                      <Switch
                        :model-value="config.auto_action.enabled"
                        @update:model-value="(value: boolean) => config.auto_action.enabled = value"
                      />
                    </div>
                  </div>
                </template>

                <div
                  class="space-y-4 transition-opacity"
                  :class="config.auto_action.enabled ? '' : 'opacity-60'"
                >
                  <div class="grid gap-4 md:grid-cols-2">
                    <FieldBox label="命中次数">
                      <Input
                        :model-value="String(config.auto_action.violation_threshold)"
                        type="number"
                        min="1"
                        :disabled="!config.auto_action.enabled"
                        @update:model-value="value => config.auto_action.violation_threshold = parseInteger(value, 3)"
                      />
                    </FieldBox>
                    <FieldBox label="窗口秒数">
                      <Input
                        :model-value="String(config.auto_action.window_seconds)"
                        type="number"
                        min="60"
                        :disabled="!config.auto_action.enabled"
                        @update:model-value="value => config.auto_action.window_seconds = parseInteger(value, 86400)"
                      />
                    </FieldBox>
                  </div>

                  <div class="grid gap-3 sm:grid-cols-2">
                    <div class="flex items-center justify-between gap-4 rounded-xl border border-border bg-muted/20 px-4 py-3">
                      <div class="min-w-0">
                        <p class="text-sm font-medium text-foreground">
                          禁用用户
                        </p>
                        <p class="mt-0.5 text-xs text-muted-foreground">
                          命中阈值后禁用账号
                        </p>
                      </div>
                      <Switch
                        :model-value="config.auto_action.disable_user"
                        :disabled="!config.auto_action.enabled"
                        @update:model-value="(value: boolean) => config.auto_action.disable_user = value"
                      />
                    </div>
                    <div class="flex items-center justify-between gap-4 rounded-xl border border-border bg-muted/20 px-4 py-3">
                      <div class="min-w-0">
                        <p class="text-sm font-medium text-foreground">
                          锁定 Key
                        </p>
                        <p class="mt-0.5 text-xs text-muted-foreground">
                          命中阈值后锁定 API Key
                        </p>
                      </div>
                      <Switch
                        :model-value="config.auto_action.lock_api_key"
                        :disabled="!config.auto_action.enabled"
                        @update:model-value="(value: boolean) => config.auto_action.lock_api_key = value"
                      />
                    </div>
                  </div>
                </div>
              </CardSection>
            </TabsContent>

            <TabsContent
              value="keywords"
              class="space-y-6"
            >
              <CardSection
                title="关键词模式"
                description="本地关键词可独立或配合 Provider 使用，支持 contains / exact / regex。"
              >
                <div class="grid gap-2 sm:grid-cols-3">
                  <button
                    type="button"
                    class="rounded-xl border p-4 text-left transition-colors"
                    :class="config.keyword_mode === 'keyword_and_api' ? 'border-primary bg-primary/10 text-primary' : 'border-border bg-card hover:bg-muted/40'"
                    @click="config.keyword_mode = 'keyword_and_api'"
                  >
                    <p class="text-sm font-semibold">
                      关键词 + API
                    </p><p class="mt-1 text-xs text-muted-foreground">
                      本地命中和 Provider 都参与判断。
                    </p>
                  </button><button
                    type="button"
                    class="rounded-xl border p-4 text-left transition-colors"
                    :class="config.keyword_mode === 'keyword_only' ? 'border-primary bg-primary/10 text-primary' : 'border-border bg-card hover:bg-muted/40'"
                    @click="config.keyword_mode = 'keyword_only'"
                  >
                    <p class="text-sm font-semibold">
                      仅关键词
                    </p><p class="mt-1 text-xs text-muted-foreground">
                      不调用 Provider，仅按本地规则判定。
                    </p>
                  </button><button
                    type="button"
                    class="rounded-xl border p-4 text-left transition-colors"
                    :class="config.keyword_mode === 'api_only' ? 'border-primary bg-primary/10 text-primary' : 'border-border bg-card hover:bg-muted/40'"
                    @click="config.keyword_mode = 'api_only'"
                  >
                    <p class="text-sm font-semibold">
                      仅 API
                    </p><p class="mt-1 text-xs text-muted-foreground">
                      跳过本地关键词，仅使用 Provider 结果。
                    </p>
                  </button>
                </div>
              </CardSection>
              <CardSection
                title="关键词、豁免与阈值"
                description="通过匹配模式、豁免短语和分类阈值控制关键词命中。"
              >
                <div class="mb-5 grid gap-2 sm:grid-cols-3">
                  <button
                    type="button"
                    class="rounded-xl border p-4 text-left transition-colors"
                    :class="config.keyword_match_mode === 'contains' ? 'border-primary bg-primary/10 text-primary' : 'border-border bg-card hover:bg-muted/40'"
                    :disabled="config.keyword_mode === 'api_only'"
                    @click="config.keyword_match_mode = 'contains'"
                  >
                    <p class="text-sm font-semibold">
                      contains
                    </p>
                    <p class="mt-1 text-xs leading-5 text-muted-foreground">
                      按子串匹配，覆盖面最广。
                    </p>
                  </button>
                  <button
                    type="button"
                    class="rounded-xl border p-4 text-left transition-colors"
                    :class="config.keyword_match_mode === 'exact' ? 'border-primary bg-primary/10 text-primary' : 'border-border bg-card hover:bg-muted/40'"
                    :disabled="config.keyword_mode === 'api_only'"
                    @click="config.keyword_match_mode = 'exact'"
                  >
                    <p class="text-sm font-semibold">
                      exact
                    </p>
                    <p class="mt-1 text-xs leading-5 text-muted-foreground">
                      仅在全文等于限制词时命中。
                    </p>
                  </button>
                  <button
                    type="button"
                    class="rounded-xl border p-4 text-left transition-colors"
                    :class="config.keyword_match_mode === 'regex' ? 'border-primary bg-primary/10 text-primary' : 'border-border bg-card hover:bg-muted/40'"
                    :disabled="config.keyword_mode === 'api_only'"
                    @click="config.keyword_match_mode = 'regex'"
                  >
                    <p class="text-sm font-semibold">
                      regex
                    </p>
                    <p class="mt-1 text-xs leading-5 text-muted-foreground">
                      按 Rust Regex 匹配，适合编号、变体和组合规则；运行时有扫描预算，超长文本会截断后检测。
                    </p>
                  </button>
                </div>
                <div class="grid gap-5 lg:grid-cols-[minmax(0,1fr)_420px]">
                  <div class="space-y-4">
                    <div class="space-y-2">
                      <div class="flex items-center justify-between gap-3">
                        <Label>限制词</Label>
                        <Badge
                          variant="outline"
                          class="bg-muted/40"
                        >
                          {{ keywordInputCount }} keywords
                        </Badge>
                      </div>
                      <Textarea
                        v-model="keywordsText"
                        class="min-h-[220px] font-mono text-xs"
                        :disabled="config.keyword_mode === 'api_only'"
                        :placeholder="keywordPlaceholder"
                      />
                    </div>
                    <div class="space-y-2 rounded-xl border border-border bg-muted/20 p-3">
                      <div class="flex items-center justify-between gap-3">
                        <div>
                          <Label>豁免短语</Label>
                          <p class="mt-1 text-xs leading-5 text-muted-foreground">
                            命中片段被豁免短语完整覆盖时放行。
                          </p>
                        </div>
                        <Badge
                          variant="outline"
                          class="shrink-0 bg-background/70"
                        >
                          {{ keywordExemptionInputCount }} allow
                        </Badge>
                      </div>
                      <Textarea
                        v-model="keywordExemptionsText"
                        class="min-h-[150px] font-mono text-xs"
                        :disabled="config.keyword_mode === 'api_only'"
                        placeholder="每行一个豁免短语&#10;safe sample&#10;approved phrase"
                      />
                      <p class="text-xs leading-5 text-muted-foreground">
                        {{ keywordExemptionExample }}
                      </p>
                    </div>
                  </div>
                  <div class="space-y-3">
                    <div class="flex items-center justify-between gap-3">
                      <Label>分类阈值</Label><Button
                        variant="outline"
                        size="sm"
                        @click="addThresholdRow"
                      >
                        <Plus class="mr-2 h-4 w-4" />新增
                      </Button>
                    </div><div class="overflow-hidden rounded-lg border border-border">
                      <table class="w-full text-sm">
                        <thead class="bg-muted/50 text-left text-xs text-muted-foreground">
                          <tr>
                            <th class="px-3 py-2">
                              分类
                            </th><th class="w-28 px-3 py-2">
                              阈值
                            </th><th class="w-12 px-2 py-2" />
                          </tr>
                        </thead><tbody>
                          <tr
                            v-for="(row, index) in thresholdRows"
                            :key="row.id"
                            class="border-t border-border"
                          >
                            <td class="px-3 py-2">
                              <Input
                                :model-value="row.category"
                                size="sm"
                                class="min-h-10 font-mono"
                                placeholder="violence"
                                @update:model-value="value => updateThresholdRow(index, { category: String(value) })"
                              />
                            </td><td class="px-3 py-2">
                              <Input
                                :model-value="row.value"
                                size="sm"
                                class="min-h-10"
                                type="number"
                                step="0.01"
                                min="0"
                                max="1"
                                @update:model-value="value => updateThresholdRow(index, { value: String(value) })"
                              />
                            </td><td class="px-2 py-2 text-right">
                              <Button
                                variant="ghost"
                                size="icon"
                                class="h-10 w-10 text-muted-foreground hover:text-destructive"
                                title="删除"
                                @click="removeThresholdRow(index)"
                              >
                                <Trash2 class="h-4 w-4" />
                              </Button>
                            </td>
                          </tr><tr v-if="thresholdRows.length === 0">
                            <td
                              colspan="3"
                              class="px-3 py-8 text-center text-sm text-muted-foreground"
                            >
                              未设置自定义阈值
                            </td>
                          </tr>
                        </tbody>
                      </table>
                    </div>
                  </div>
                </div>
              </CardSection>
            </TabsContent>

            <TabsContent
              value="retention"
              class="space-y-6"
            >
              <CardSection
                title="命中哈希"
                description="已确认风险输入写入哈希表，相同内容可快速命中。"
              >
                <template #actions>
                  <Button
                    variant="outline"
                    size="sm"
                    class="rounded-xl border-border bg-background/80 px-3 text-muted-foreground hover:text-destructive"
                    :disabled="hashesLoading || hashes.total === 0 || dangerDialogLoading"
                    @click="openClearHashesDialog"
                  >
                    <Trash2 class="mr-2 h-4 w-4" />
                    清空哈希
                  </Button>
                </template>
                <div class="grid gap-4 md:grid-cols-2">
                  <ToggleRow
                    label="启用哈希拦截"
                    :value="config.hash_block.enabled"
                    @update:value="value => config.hash_block.enabled = value"
                  /><ToggleRow
                    label="命中后学习哈希"
                    :value="config.hash_block.learn_from_flagged"
                    @update:value="value => config.hash_block.learn_from_flagged = value"
                  />
                </div>
              </CardSection>
              <CardSection
                title="保留策略"
                description="清理风险日志，不影响命中哈希表。"
              >
                <div class="mb-4 grid gap-3 md:grid-cols-3">
                  <div class="rounded-lg border border-border bg-muted/20 px-3 py-2">
                    <p class="text-xs text-muted-foreground">
                      上次清理
                    </p>
                    <p class="mt-1 truncate text-sm font-semibold text-foreground">
                      {{ formatUnixDate(status?.retention_status.last_completed_at_unix_secs ?? 0) }}
                    </p>
                  </div>
                  <div class="rounded-lg border border-border bg-muted/20 px-3 py-2">
                    <p class="text-xs text-muted-foreground">
                      删除数量
                    </p>
                    <p class="mt-1 text-sm font-semibold text-foreground">
                      命中 {{ status?.retention_status.last_hit_deleted ?? 0 }} · 未命中 {{ status?.retention_status.last_non_hit_deleted ?? 0 }}
                    </p>
                  </div>
                  <div class="rounded-lg border border-border bg-muted/20 px-3 py-2">
                    <p class="text-xs text-muted-foreground">
                      下次运行
                    </p>
                    <p class="mt-1 truncate text-sm font-semibold text-foreground">
                      {{ formatUnixDate(status?.retention_status.next_run_at_unix_secs ?? 0) }}
                    </p>
                  </div>
                </div>
                <p
                  v-if="status?.retention_status.last_error"
                  class="mb-4 rounded-lg border border-destructive/20 bg-destructive/10 px-3 py-2 text-xs leading-5 text-destructive"
                >
                  {{ status.retention_status.last_error }}
                </p>
                <div class="grid gap-4 md:grid-cols-2 md:items-end xl:grid-cols-[220px_220px_220px_1fr]">
                  <FieldBox label="命中日志天数">
                    <Input
                      :model-value="String(config.retention.hit_days)"
                      type="number"
                      min="0"
                      @update:model-value="value => config.retention.hit_days = parseInteger(value, 90)"
                    />
                  </FieldBox><FieldBox label="未命中日志天数">
                    <Input
                      :model-value="String(config.retention.non_hit_days)"
                      type="number"
                      min="0"
                      @update:model-value="value => config.retention.non_hit_days = parseInteger(value, 14)"
                    />
                  </FieldBox>
                  <FieldBox label="自动清理间隔（分钟，0=关闭）">
                    <Input
                      :model-value="String(config.retention.auto_run_interval_minutes)"
                      type="number"
                      min="0"
                      @update:model-value="value => config.retention.auto_run_interval_minutes = parseInteger(value, 60)"
                    />
                  </FieldBox>
                  <Button
                    variant="outline"
                    :disabled="runningRetention"
                    @click="openRetentionDialog"
                  >
                    <ArchiveX class="mr-2 h-4 w-4" />{{ runningRetention ? '清理中...' : '立即清理过期日志' }}
                  </Button>
                </div>
              </CardSection>
              <CardSection
                title="命中告警"
                description="通过通知中心向运维推送命中事件与自动处置；需先在通知中心配置好渠道。"
              >
                <div
                  v-if="status?.notification_warning"
                  class="mb-4 flex items-start gap-3 rounded-lg border border-amber-200 bg-amber-50 px-3 py-2 text-xs text-amber-800 dark:border-amber-300/25 dark:bg-amber-400/10 dark:text-amber-100"
                >
                  <AlertTriangle class="mt-0.5 h-4 w-4 shrink-0" />
                  <span class="leading-5">{{ status.notification_warning }}</span>
                </div>
                <div class="grid gap-4 md:grid-cols-2 xl:grid-cols-5">
                  <ToggleRow
                    label="启用通知"
                    :value="config.notification.enabled"
                    @update:value="value => config.notification.enabled = value"
                  />
                  <ToggleRow
                    label="命中事件通知"
                    :value="config.notification.notify_on_flagged"
                    @update:value="value => config.notification.notify_on_flagged = value"
                  />
                  <ToggleRow
                    label="自动处置通知"
                    :value="config.notification.notify_on_auto_action"
                    @update:value="value => config.notification.notify_on_auto_action = value"
                  />
                  <ToggleRow
                    label="用户处置通知"
                    :value="config.notification.notify_on_user_action_notice"
                    @update:value="value => config.notification.notify_on_user_action_notice = value"
                  />
                  <ToggleRow
                    label="附带命中摘要"
                    :value="config.notification.include_excerpt"
                    @update:value="value => config.notification.include_excerpt = value"
                  />
                </div>
              </CardSection>
            </TabsContent>

            <TabsContent
              value="test"
              class="space-y-6"
            >
              <CardSection
                title="在线测试"
                description="用当前编辑中的配置检测一段文本。"
              >
                <div class="grid gap-5 lg:grid-cols-[minmax(0,1fr)_360px]">
                  <div class="space-y-3">
                    <Textarea
                      v-model="testText"
                      class="min-h-[240px]"
                      placeholder="输入待检测文本"
                    /><Button
                      :disabled="testingText"
                      @click="runTest"
                    >
                      <FlaskConical class="mr-2 h-4 w-4" />{{ testingText ? '检测中...' : '运行检测' }}
                    </Button>
                  </div><div class="rounded-lg border border-border bg-muted/25 p-4">
                    <template v-if="testResult">
                      <div class="flex items-center justify-between">
                        <span class="text-sm font-medium text-foreground">结果</span><Badge :variant="testResult.result.flagged ? 'destructive' : 'success'">
                          {{ testResult.result.flagged ? '命中' : '放行' }}
                        </Badge>
                      </div><dl class="mt-4 space-y-3 text-sm">
                        <div class="flex justify-between gap-4">
                          <dt class="text-muted-foreground">
                            动作
                          </dt><dd class="font-medium text-foreground">
                            {{ testResult.result.action }}
                          </dd>
                        </div><div class="flex justify-between gap-4">
                          <dt class="text-muted-foreground">
                            来源
                          </dt><dd class="font-medium text-foreground">
                            {{ testResult.result.decision_source }}
                          </dd>
                        </div><div class="flex justify-between gap-4">
                          <dt class="text-muted-foreground">
                            分类
                          </dt><dd class="font-medium text-foreground">
                            {{ testResult.result.highest_category || '-' }}
                          </dd>
                        </div><div class="flex justify-between gap-4">
                          <dt class="text-muted-foreground">
                            分数
                          </dt><dd class="font-medium text-foreground">
                            {{ formatScore(testResult.result.highest_score) }}
                          </dd>
                        </div>
                      </dl><div
                        v-if="showRegexScanStats"
                        class="mt-4 grid gap-2 text-xs sm:grid-cols-2"
                      >
                        <div class="rounded-lg border border-border bg-background/70 px-3 py-2">
                          <p class="text-muted-foreground">
                            Regex 截断
                          </p>
                          <p class="mt-1 font-medium text-foreground">
                            {{ formatBoolean(testResult.result.regex_scan_limited === true) }}
                          </p>
                        </div>
                        <div class="rounded-lg border border-border bg-background/70 px-3 py-2">
                          <p class="text-muted-foreground">
                            扫描窗口
                          </p>
                          <p class="mt-1 font-medium text-foreground">
                            {{ formatScanChars(testResult.result.regex_scan_chars) }}
                          </p>
                        </div>
                        <div class="rounded-lg border border-border bg-background/70 px-3 py-2">
                          <p class="text-muted-foreground">
                            Regex 规则
                          </p>
                          <p class="mt-1 font-medium text-foreground">
                            {{ testResult.result.regex_pattern_count ?? 0 }} 条
                          </p>
                        </div>
                        <div class="rounded-lg border border-border bg-background/70 px-3 py-2">
                          <p class="text-muted-foreground">
                            总预算
                          </p>
                          <p class="mt-1 font-medium text-foreground">
                            {{ formatScanChars(testResult.result.regex_total_scan_budget_chars) }}
                          </p>
                        </div>
                      </div><div
                        v-if="testResult.result.matched_keywords.length"
                        class="mt-4 flex flex-wrap gap-1"
                      >
                        <Badge
                          v-for="keyword in testResult.result.matched_keywords"
                          :key="keyword"
                          variant="warning"
                        >
                          {{ keyword }}
                        </Badge>
                      </div><p
                        v-if="testResult.result.regex_scan_limited"
                        class="mt-4 rounded-lg border border-amber-200 bg-amber-50 px-3 py-2 text-xs leading-5 text-amber-800 dark:border-amber-300/25 dark:bg-amber-400/10 dark:text-amber-100"
                      >
                        Regex 扫描已按运行时预算截断：{{ testResult.result.regex_pattern_count || 0 }} 条规则，窗口 {{ formatScanChars(testResult.result.regex_scan_chars) }}，总预算 {{ formatScanChars(testResult.result.regex_total_scan_budget_chars) }}。
                      </p><p
                        v-if="testResult.result.regex_pattern_limited"
                        class="mt-4 rounded-lg border border-amber-200 bg-amber-50 px-3 py-2 text-xs leading-5 text-amber-800 dark:border-amber-300/25 dark:bg-amber-400/10 dark:text-amber-100"
                      >
                        Regex 规则超过运行时上限，仅扫描前 {{ testResult.result.regex_pattern_count || 0 }} 条，正式请求会按预算受限处理。
                      </p><p
                        v-if="(testResult.result.regex_invalid_pattern_count || 0) > 0"
                        class="mt-4 rounded-lg border border-destructive/30 bg-destructive/10 px-3 py-2 text-xs leading-5 text-destructive"
                      >
                        Regex 配置中有 {{ testResult.result.regex_invalid_pattern_count }} 条规则运行时不可用，正式请求会按配置异常处理。
                      </p><p
                        v-if="testResult.result.error_message"
                        class="mt-4 rounded-lg border border-destructive/30 bg-destructive/10 px-3 py-2 text-xs text-destructive"
                      >
                        {{ testResult.result.error_message }}
                      </p>
                    </template><div
                      v-else
                      class="flex min-h-[220px] items-center justify-center text-sm text-muted-foreground"
                    >
                      尚未运行测试
                    </div>
                  </div>
                </div>
              </CardSection>
            </TabsContent>
          </Tabs>
        </div>
        <template #footer>
          <Button
            :disabled="saving"
            @click="saveConfig"
          >
            <Save class="mr-2 h-4 w-4" />
            {{ saving ? '保存中...' : '保存配置' }}
          </Button>
          <Button
            variant="outline"
            :disabled="saving"
            @click="configDialogOpen = false"
          >
            关闭
          </Button>
        </template>
      </Dialog>
    </div>
    <AlertDialog
      v-model="dangerDialogOpen"
      type="danger"
      :title="dangerDialogTitle"
      :description="dangerDialogDescription"
      :confirm-text="dangerDialogConfirmText"
      :loading="dangerDialogLoading"
      @confirm="confirmDangerAction"
      @cancel="closeDangerDialog"
    />
  </PageContainer>
</template>

<script setup lang="ts">
import { computed, nextTick, onMounted, reactive, ref, watch } from 'vue'
import {
  Activity,
  AlertTriangle,
  ArchiveX,
  Eye,
  FlaskConical,
  KeyRound,
  Plus,
  RadioTower,
  RefreshCw,
  Save,
  Settings,
  ShieldAlert,
  ShieldCheck,
  Trash2,
  UserCheck,
} from 'lucide-vue-next'
import { PageContainer, PageHeader, CardSection } from '@/components/layout'
import {
  Badge,
  Button,
  Dialog,
  Input,
  Label,
  Pagination,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Switch,
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
  Textarea,
} from '@/components/ui'
import { AlertDialog, EmptyState, LoadingState } from '@/components/common'
import FieldBox from './RiskControlFieldBox.vue'
import ToggleRow from './RiskControlToggleRow.vue'
import {
  cloneRiskControlConfig,
  DEFAULT_RISK_CONTROL_CONFIG,
  riskControlApi,
  validateRiskControlProviderBaseUrl,
  validateRiskControlRegexConfig,
  type RiskControlConfig,
  type RiskControlHashItem,
  type RiskControlLogFilters,
  type RiskControlLogItem,
  type RiskControlModelFilterMode,
  type RiskControlPage,
  type RiskControlProviderKeyStatus,
  type RiskControlScopeConfig,
  type RiskControlScopeMode,
  type RiskControlStatus,
  type RiskControlTestResponse,
} from '@/api/risk-control'
import { useModuleStore } from '@/stores/modules'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'
import { log } from '@/utils/logger'

interface ThresholdRow {
  id: string
  category: string
  value: string
}

type DangerAction = 'delete_hash' | 'clear_hashes' | 'run_retention'
type ApiKeysWriteMode = 'append' | 'replace'
type ScopeKey = keyof RiskControlScopeConfig
type DecisionOption<T extends string> = {
  value: T
  eyebrow: string
  label: string
  description: string
}

const riskModeCards: DecisionOption<RiskControlConfig['mode']>[] = [
  {
    value: 'off',
    eyebrow: 'Off',
    label: '关闭',
    description: '保留配置但不参与请求链路，适合维护窗口。',
  },
  {
    value: 'observe',
    eyebrow: 'Observe',
    label: '观察',
    description: '只记录命中和分数，不阻断真实请求。',
  },
  {
    value: 'pre_block',
    eyebrow: 'Block',
    label: '前置拦截',
    description: '命中风险后返回拦截响应，不再转发上游。',
  },
]

const keywordModeCards: DecisionOption<RiskControlConfig['keyword_mode']>[] = [
  {
    value: 'keyword_and_api',
    eyebrow: 'Hybrid',
    label: '关键词 + API',
    description: '本地规则先筛，再由 Provider 做分类判定。',
  },
  {
    value: 'keyword_only',
    eyebrow: 'Local',
    label: '仅关键词',
    description: '不调用 Provider，适合无 Key 或低成本拦截。',
  },
  {
    value: 'api_only',
    eyebrow: 'Provider',
    label: '仅 API',
    description: '跳过本地限制词，仅使用 Provider 审核结果。',
  },
]

const scopeGroups: { key: ScopeKey; label: string; placeholder: string }[] = [
  { key: 'users', label: '用户', placeholder: '每行一个用户 ID' },
  { key: 'user_groups', label: '用户组', placeholder: '每行一个用户组 ID' },
  { key: 'api_keys', label: 'API Key', placeholder: '每行一个 API Key ID' },
  { key: 'route_families', label: '路由族', placeholder: '每行一个 route family' },
  { key: 'route_kinds', label: '路由类型', placeholder: '每行一个 route kind' },
  { key: 'endpoints', label: '端点', placeholder: '每行一个 endpoint' },
]

const moduleStore = useModuleStore()
const { success, error } = useToast()

const loading = ref(false)
const saving = ref(false)
const configDialogOpen = ref(false)
const configTab = ref('basic')
const logDetailDialogOpen = ref(false)
const selectedLog = ref<RiskControlLogItem | null>(null)
const status = ref<RiskControlStatus | null>(null)
const config = ref<RiskControlConfig>(cloneRiskControlConfig(DEFAULT_RISK_CONTROL_CONFIG))
const originalConfig = ref<RiskControlConfig>(cloneRiskControlConfig(DEFAULT_RISK_CONTROL_CONFIG))
const keywordsText = ref('')
const keywordExemptionsText = ref('')
const apiKeysText = ref('')
const apiKeysWriteMode = ref<ApiKeysWriteMode>('append')
const apiKeysTextareaRef = ref<HTMLElement | { $el?: HTMLElement } | null>(null)
const modelFilterModelsText = ref('')
const scopeValuesText = reactive<Record<ScopeKey, string>>({
  users: '',
  user_groups: '',
  api_keys: '',
  route_families: '',
  route_kinds: '',
  endpoints: '',
})
const thresholdRows = ref<ThresholdRow[]>([])

const logsLoading = ref(false)
const logsPage = ref(1)
const logsPageSize = ref(20)
const logFlaggedFilter = ref('all')
const logSearchText = ref('')
const logDateFrom = ref('')
const logDateTo = ref('')
const logFilters = reactive<{
  action: string
  decision_source: string
  endpoint: string
}>({
  action: 'all',
  decision_source: 'all',
  endpoint: 'all',
})
const logs = ref<RiskControlPage<RiskControlLogItem>>({
  items: [],
  total: 0,
  page: 1,
  page_size: 20,
  pages: 0,
})

const hashesLoading = ref(false)
const hashesPage = ref(1)
const hashesPageSize = ref(20)
const hashes = ref<RiskControlPage<RiskControlHashItem>>({
  items: [],
  total: 0,
  page: 1,
  page_size: 20,
  pages: 0,
})

const testingText = ref(false)
const testingProvider = ref(false)
const runningRetention = ref(false)
const restoringAutoAction = ref<'user' | 'api_key' | null>(null)
const retryingNotification = ref(false)
const testText = ref('')
const testResult = ref<RiskControlTestResponse | null>(null)
const testedProviderKeyStatuses = ref<RiskControlProviderKeyStatus[]>([])
const dangerDialogOpen = ref(false)
const dangerConfirmLoading = ref(false)
const pendingDangerAction = ref<DangerAction | null>(null)
const pendingHash = ref<string | null>(null)

const showRegexScanStats = computed(() => {
  const result = testResult.value?.result
  return !!result && (
    result.regex_scan_limited !== undefined
    || result.regex_pattern_limited !== undefined
    || result.regex_invalid_pattern_count !== undefined
    || result.regex_scan_chars !== undefined
    || result.regex_pattern_count !== undefined
    || result.regex_total_scan_budget_chars !== undefined
  )
})

const riskModeLabel = computed(() => {
  if (!config.value.enabled || config.value.mode === 'off') return 'Standby'
  if (config.value.mode === 'pre_block') return 'Blocking'
  return 'Observe'
})

const riskStatusDotClass = computed(() => {
  if (!config.value.enabled || config.value.mode === 'off') return 'bg-muted-foreground/40'
  if (status.value && !status.value.config_validated) return 'bg-destructive'
  return config.value.mode === 'pre_block' ? 'bg-emerald-500' : 'bg-amber-500'
})

const decisionStatusTitle = computed(() => {
  if (!config.value.enabled) return '总闸未启用'
  if (config.value.mode === 'off') return '逻辑已关闭'
  if (status.value && !status.value.config_validated) return '配置待修复'
  return config.value.mode === 'pre_block' ? '前置拦截已就绪' : '观察模式已就绪'
})

const decisionStatusDetail = computed(() => {
  if (!config.value.enabled) return '保存后不参与请求链路，可先完成 Provider、关键词和保留策略。'
  if (config.value.mode === 'off') return '总开关已启用，但运行模式为 off，仍不审核请求。'
  if (status.value && !status.value.config_validated) {
    return status.value.config_error || '配置校验失败，请补齐 Provider 或切换为仅关键词。'
  }
  if (config.value.mode === 'pre_block') return `命中风险会返回 ${config.value.block_status}，并写入审计日志与可复用 hash。`
  return '命中风险只写日志不阻断，适合上线前对真实流量做基线观察。'
})

const decisionStatusDotClass = computed(() => riskStatusDotClass.value)

const decisionFlowSteps = computed(() => [
  {
    label: '全部 user 输入',
    value: '抽取',
    active: config.value.enabled && config.value.mode !== 'off',
  },
  {
    label: '哈希预检',
    value: config.value.hash_block.enabled ? '启用' : '跳过',
    active: config.value.hash_block.enabled,
  },
  {
    label: '关键词',
    value: config.value.keyword_mode === 'api_only'
      ? '跳过'
      : `${keywordInputCount.value} 条 · ${keywordMatchModeText(config.value.keyword_match_mode)}`,
    active: config.value.keyword_mode !== 'api_only',
  },
  {
    label: 'Provider',
    value: config.value.keyword_mode === 'keyword_only'
      ? '跳过'
      : `${providerKeyInputCount.value} Keys`,
    active: config.value.keyword_mode !== 'keyword_only',
  },
  {
    label: '处置 / 日志',
    value: config.value.mode === 'pre_block' ? `${config.value.block_status} 拦截` : '记录',
    active: config.value.enabled && config.value.mode !== 'off',
  },
])

const statusMetrics = computed(() => [
  {
    label: '日志',
    value: status.value?.logs_total ?? 0,
    detail: '总记录',
    dotClass: 'bg-slate-400',
  },
  {
    label: '命中',
    value: status.value?.flagged_total ?? 0,
    detail: '风险事件',
    dotClass: 'bg-rose-400',
  },
  {
    label: '哈希',
    value: status.value?.flagged_hashes_total ?? 0,
    detail: '已学习',
    dotClass: 'bg-orange-400',
  },
  {
    label: '关键词',
    value: keywordInputCount.value,
    detail: `${keywordMatchModeText(config.value.keyword_match_mode)} · ${keywordExemptionInputCount.value} 个豁免`,
    dotClass: 'bg-amber-400',
  },
  {
    label: 'Keys',
    value: providerKeyInputCount.value,
    detail: providerKeyHealthSummary.value,
    dotClass: 'bg-emerald-400',
  },
  {
    label: '队列',
    value: status.value?.observe_queue.queued ?? 0,
    detail: `处理 ${status.value?.observe_queue.processed_total ?? 0} · 丢弃 ${status.value?.observe_queue.dropped_total ?? 0}`,
    dotClass: status.value?.observe_queue.dropped_total ? 'bg-red-400' : 'bg-cyan-400',
  },
  {
    label: '通知',
    value: status.value?.notification_outbox.pending ?? 0,
    detail: `发送中 ${status.value?.notification_outbox.processing ?? 0} · 死信 ${status.value?.notification_outbox.dead ?? 0}`,
    dotClass: status.value?.notification_outbox.dead ? 'bg-red-400' : 'bg-sky-400',
  },
])

const keywordInputCount = computed(() => parseLines(keywordsText.value).length)
const keywordExemptionInputCount = computed(() => parseLines(keywordExemptionsText.value).length)
const keywordPlaceholder = computed(() => {
  if (config.value.keyword_match_mode === 'regex') {
    return '每行一个正则限制词，例如：pattern-[a-z]+-\\d+'
  }
  if (config.value.keyword_match_mode === 'exact') {
    return '每行一个精确限制词，例如：risk'
  }
  return '每行一个限制词，例如：risk'
})
const keywordExemptionExample = computed(() => {
  if (config.value.keyword_match_mode === 'regex') {
    return '例：正则「pattern-[a-z]+-\\d+」+ 豁免「pattern-safe-123」时，被完整覆盖的位置会放行。'
  }
  if (config.value.keyword_match_mode === 'exact') {
    return '例：exact 下只有全文完全等于限制词才命中；「prefix-risk」不会触发「risk」。'
  }
  return '例：限制词「blocked」+ 豁免「safe sample」时，被豁免短语覆盖的片段会放行。'
})
const providerKeyLines = computed(() => parseLines(apiKeysText.value))
const providerKeyInputCount = computed(() => providerKeyLines.value.length)
const plainProviderKeyInputCount = computed(() => providerKeyLines.value.filter(value => !isMaskedProviderKey(value)).length)
const maskedProviderKeyInputCount = computed(() => providerKeyLines.value.filter(isMaskedProviderKey).length)
const providerKeyWriteModeHint = computed(() => (
  apiKeysWriteMode.value === 'replace'
    ? '替换模式：保存后以当前列表为准，未保留的旧 Key 会被删除。'
    : '追加模式：保存时保留已存 Key，并追加当前输入的新 Key。'
))

const strategyChips = computed(() => [
  {
    label: '运行模式',
    value: riskModeText(config.value.mode),
    danger: config.value.enabled && status.value?.config_validated === false,
  },
  {
    label: '审核链路',
    value: keywordModeText(config.value.keyword_mode),
    danger: config.value.keyword_mode !== 'keyword_only' && providerKeyInputCount.value === 0,
  },
  {
    label: '模型范围',
    value: modelFilterSummary.value,
    danger: config.value.model_filter.mode !== 'all' && parseLines(modelFilterModelsText.value).length === 0,
  },
  {
    label: '策略粒度',
    value: scopeSummary.value,
    danger: hasInvalidScope.value,
  },
  {
    label: 'Provider',
    value: config.value.keyword_mode === 'keyword_only'
      ? '不调用'
      : `${providerKeyInputCount.value} Keys · ${config.value.provider.fail_closed ? 'Fail closed' : 'Fail open'}`,
    danger: config.value.keyword_mode !== 'keyword_only' && providerKeyInputCount.value === 0,
  },
])

const providerKeyRows = computed(() => (
  testedProviderKeyStatuses.value.length > 0
    ? testedProviderKeyStatuses.value
    : status.value?.provider_key_statuses ?? []
))

const providerKeyHealthSummary = computed(() => {
  if (providerKeyRows.value.length === 0) return '未测试'
  const counts = providerKeyRows.value.reduce<Record<RiskControlProviderKeyStatus['status'], number>>((acc, item) => {
    acc[item.status] += 1
    return acc
  }, {
    unknown: 0,
    ok: 0,
    error: 0,
    frozen: 0,
  })
  return (['ok', 'frozen', 'error', 'unknown'] as RiskControlProviderKeyStatus['status'][])
    .filter(statusValue => counts[statusValue] > 0)
    .map(statusValue => `${providerKeyStatusLabel(statusValue)} ${counts[statusValue]}`)
    .join(' · ')
})

const modelFilterSummary = computed(() => modelFilterModeText(config.value.model_filter.mode))
const activeScopeRuleCount = computed(() => (
  scopeGroups.filter(group => config.value.scope[group.key].mode !== 'all').length
))
const hasInvalidScope = computed(() => (
  scopeGroups.some(group => (
    config.value.scope[group.key].mode !== 'all'
    && parseLines(scopeValuesText[group.key]).length === 0
  ))
))
const scopeSummary = computed(() => {
  const active = scopeGroups
    .filter(group => config.value.scope[group.key].mode !== 'all')
    .map(group => `${group.label}${scopeValueCount(group.key)}`)
  return active.length > 0 ? active.join(' · ') : '全部策略范围'
})

const logEndpointOptions = computed(() => {
  const endpoints = new Set<string>()
  if (logFilters.endpoint && logFilters.endpoint !== 'all') {
    endpoints.add(logFilters.endpoint)
  }
  logs.value.items.forEach((item) => {
    if (item.endpoint) endpoints.add(item.endpoint)
  })
  return [...endpoints].sort((left, right) => left.localeCompare(right))
})

const hasActiveLogFilters = computed(() => (
  logFlaggedFilter.value !== 'all'
  || logSearchText.value.trim() !== ''
  || logDateFrom.value.trim() !== ''
  || logDateTo.value.trim() !== ''
  || logFilters.action !== 'all'
  || logFilters.decision_source !== 'all'
  || logFilters.endpoint !== 'all'
))

const visibleLogItems = computed(() => logs.value.items)

const logDetailTitle = computed(() => (
  selectedLog.value ? `日志详情 · ${selectedLog.value.action}` : '日志详情'
))

const logDetailDescription = computed(() => {
  if (!selectedLog.value) return '查看单条风控日志的完整审计记录。'
  const chunks = [
    formatDate(selectedLog.value.created_at),
    selectedLog.value.model || selectedLog.value.endpoint || selectedLog.value.api_format || null,
    selectedLog.value.trace_id || selectedLog.value.request_id || null,
  ].filter(Boolean)
  return chunks.join(' · ')
})

const selectedLogScoreRows = computed<[string, number][]>(() => (
  Object.entries(selectedLog.value?.category_scores ?? {})
    .sort(([, left], [, right]) => right - left)
))

const selectedLogThresholdRows = computed<[string, number][]>(() => (
  Object.entries(selectedLog.value?.thresholds ?? {})
    .sort(([left], [right]) => left.localeCompare(right))
))

const dangerDialogTitle = computed(() => {
  if (pendingDangerAction.value === 'delete_hash') return '删除命中哈希？'
  if (pendingDangerAction.value === 'clear_hashes') return '清空全部命中哈希？'
  if (pendingDangerAction.value === 'run_retention') return '立即清理过期日志？'
  return '确认危险操作？'
})

const dangerDialogDescription = computed(() => {
  if (pendingDangerAction.value === 'delete_hash') {
    return [
      '这会移除一个已学习的风险输入指纹。',
      pendingHash.value ? compactHash(pendingHash.value) : '未选择哈希',
      '相同内容不会继续通过 hash 快速命中，直到再次触发学习。',
    ].join('\n')
  }
  if (pendingDangerAction.value === 'clear_hashes') {
    return [
      '这会删除当前全部已学习命中哈希。',
      `预计影响：${hashes.value.total} 条`,
      '风险日志不会被删除，但 hash 快速拦截能力会被重置。',
    ].join('\n')
  }
  if (pendingDangerAction.value === 'run_retention') {
    return [
      '这会按当前保留策略清理过期风控日志。',
      `命中日志 ${config.value.retention.hit_days} 天 / 未命中日志 ${config.value.retention.non_hit_days} 天`,
      '命中哈希表不会被清理。',
    ].join('\n')
  }
  return '请确认后继续。'
})

const dangerDialogConfirmText = computed(() => {
  if (pendingDangerAction.value === 'delete_hash') return '删除哈希'
  if (pendingDangerAction.value === 'clear_hashes') return '清空哈希'
  if (pendingDangerAction.value === 'run_retention') return '立即清理'
  return '确认'
})

const dangerDialogLoading = computed(() => (
  dangerConfirmLoading.value
  || (pendingDangerAction.value === 'run_retention' && runningRetention.value)
))

watch([logsPage, logsPageSize], () => {
  void loadLogs()
})

function parseLines(value: string): string[] {
  return value
    .split(/\r?\n/)
    .map(item => item.trim())
    .filter(Boolean)
}

function isMaskedProviderKey(value: string): boolean {
  return value.includes('****')
}

function mergeProviderKeyLines(existing: string[], incoming: string[]): string[] {
  const merged: string[] = []
  for (const value of [...existing, ...incoming]) {
    const trimmed = value.trim()
    if (!trimmed) continue
    if (merged.some(item => providerKeyLineEquals(item, trimmed))) continue
    merged.push(trimmed)
  }
  return merged
}

function providerKeyLineEquals(left: string, right: string): boolean {
  return left.trim() === right.trim()
}

function parseNumber(value: unknown, fallback: number): number {
  const parsed = Number(value)
  return Number.isFinite(parsed) ? parsed : fallback
}

function parseInteger(value: unknown, fallback: number): number {
  return Math.round(parseNumber(value, fallback))
}

function formatPercent(value: number): string {
  if (!Number.isFinite(value)) return '-'
  return `${Math.round(Math.min(1, Math.max(0, value)) * 100)}%`
}

function riskModeText(value: RiskControlConfig['mode']): string {
  if (value === 'pre_block') return '前置拦截'
  if (value === 'observe') return '观察'
  return '关闭'
}

function keywordModeText(value: RiskControlConfig['keyword_mode']): string {
  if (value === 'keyword_only') return '仅关键词'
  if (value === 'api_only') return '仅 API'
  return '关键词 + API'
}

function keywordMatchModeText(value: RiskControlConfig['keyword_match_mode']): string {
  if (value === 'exact') return 'exact'
  if (value === 'regex') return 'regex'
  return 'contains'
}

function modeCardClass(value: RiskControlConfig['mode']): string {
  if (config.value.mode === value) {
    return 'border-primary/45 bg-primary/10 shadow-sm shadow-primary/10'
  }
  return 'border-border bg-background hover:border-primary/40 hover:bg-muted/40'
}

function keywordModeCardClass(value: RiskControlConfig['keyword_mode']): string {
  if (config.value.keyword_mode === value) {
    return 'border-primary/45 bg-primary/10 shadow-sm shadow-primary/10'
  }
  return 'border-border bg-background hover:border-primary/40 hover:bg-muted/40'
}

function modelFilterModeText(value: RiskControlModelFilterMode): string {
  if (value === 'include') return `仅审核 ${parseLines(modelFilterModelsText.value).length} 个模型`
  if (value === 'exclude') return `排除 ${parseLines(modelFilterModelsText.value).length} 个模型`
  return '全部模型'
}

function scopeValueCount(key: ScopeKey): number {
  return parseLines(scopeValuesText[key]).length
}

function scopeModeText(value: RiskControlScopeMode, label: string, count: number): string {
  if (value === 'include') return `仅包含 ${count} 个${label}`
  if (value === 'exclude') return `排除 ${count} 个${label}`
  return `全部${label}`
}

function providerKeyStatusLabel(value: RiskControlProviderKeyStatus['status']): string {
  if (value === 'ok') return '正常'
  if (value === 'frozen') return '冻结'
  if (value === 'error') return '异常'
  return '未知'
}

function providerKeyStatusBadge(value: RiskControlProviderKeyStatus['status']): 'default' | 'success' | 'destructive' | 'warning' | 'secondary' | 'outline' {
  if (value === 'ok') return 'success'
  if (value === 'frozen') return 'warning'
  if (value === 'error') return 'destructive'
  return 'secondary'
}

function providerKeyMeta(row: RiskControlProviderKeyStatus): string {
  const parts: string[] = []
  parts.push(`失败 ${row.failure_count}`)
  parts.push(`成功 ${row.success_count}`)
  if (row.last_latency_ms !== null && row.last_latency_ms !== undefined) {
    parts.push(`${row.last_latency_ms}ms`)
  }
  if (row.last_http_status !== null && row.last_http_status !== undefined) {
    parts.push(`HTTP ${row.last_http_status}`)
  }
  if (row.frozen_until_unix_secs) {
    parts.push(`冻结至 ${formatUnixDate(row.frozen_until_unix_secs)}`)
  }
  if (!row.last_tested) {
    parts.push('未测试')
  }
  return parts.join(' · ')
}

function formatUnixDate(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return '-'
  return formatDate(new Date(value * 1000).toISOString())
}

function openConfigPanel(tabName: string = 'basic') {
  configTab.value = tabName
  configDialogOpen.value = true
  if (tabName === 'provider') {
    void nextTick(focusApiKeysTextarea)
  }
}

function focusApiKeysTextarea() {
  configTab.value = 'provider'
  const target = ('$el' in (apiKeysTextareaRef.value ?? {})
    ? (apiKeysTextareaRef.value as { $el?: HTMLElement }).$el
    : apiKeysTextareaRef.value) as HTMLElement | null | undefined
  target?.scrollIntoView?.({ block: 'center', behavior: 'smooth' })
  target?.focus?.()
}

function switchToKeywordOnly() {
  config.value.keyword_mode = 'keyword_only'
  configTab.value = 'basic'
  configDialogOpen.value = true
  success('已切到仅关键词，补好关键词后记得保存配置')
}

function syncEditors(nextConfig: RiskControlConfig) {
  keywordsText.value = nextConfig.keywords.join('\n')
  keywordExemptionsText.value = nextConfig.keyword_exemptions.join('\n')
  apiKeysText.value = nextConfig.provider.api_keys.join('\n')
  apiKeysWriteMode.value = 'append'
  modelFilterModelsText.value = nextConfig.model_filter.models.join('\n')
  scopeGroups.forEach((group) => {
    scopeValuesText[group.key] = nextConfig.scope[group.key].values.join('\n')
  })
  thresholdRows.value = Object.entries(nextConfig.thresholds).map(([category, threshold]) => ({
    id: `${category}_${Math.random().toString(36).slice(2, 8)}`,
    category,
    value: String(threshold),
  }))
}

function buildThresholds(showErrors: boolean): Record<string, number> | null {
  const thresholds: Record<string, number> = {}
  for (const row of thresholdRows.value) {
    const category = row.category.trim()
    if (!category) continue
    const value = Number(row.value)
    if (!Number.isFinite(value) || value < 0 || value > 1) {
      if (showErrors) error('阈值必须在 0 到 1 之间')
      return null
    }
    thresholds[category] = value
  }
  return thresholds
}

function buildDraftScope(): RiskControlScopeConfig {
  return scopeGroups.reduce<RiskControlScopeConfig>((scope, group) => {
    const mode = config.value.scope[group.key].mode
    scope[group.key] = {
      mode,
      values: mode === 'all' ? [] : parseLines(scopeValuesText[group.key]),
    }
    return scope
  }, cloneRiskControlConfig(DEFAULT_RISK_CONTROL_CONFIG).scope)
}

function buildDraftConfig(showErrors: boolean): RiskControlConfig | null {
  const thresholds = buildThresholds(showErrors)
  if (!thresholds) return null
  const inputApiKeys = providerKeyLines.value
  const apiKeys = apiKeysWriteMode.value === 'append'
    ? mergeProviderKeyLines(originalConfig.value.provider.api_keys, inputApiKeys)
    : inputApiKeys
  return {
    ...cloneRiskControlConfig(config.value),
    keywords: parseLines(keywordsText.value),
    keyword_exemptions: parseLines(keywordExemptionsText.value),
    provider: {
      ...config.value.provider,
      api_keys: apiKeys,
    },
    model_filter: {
      mode: config.value.model_filter.mode,
      models: config.value.model_filter.mode === 'all' ? [] : parseLines(modelFilterModelsText.value),
    },
    scope: buildDraftScope(),
    thresholds,
  }
}

function validateConfig(draft: RiskControlConfig): boolean {
  if (draft.model_filter.mode !== 'all' && draft.model_filter.models.length === 0) {
    error('模型范围选择包含或排除时，需要至少填写一个模型')
    return false
  }
  const emptyScopeGroup = scopeGroups.find(group => (
    draft.scope[group.key].mode !== 'all'
    && draft.scope[group.key].values.length === 0
  ))
  if (emptyScopeGroup) {
    error(`${emptyScopeGroup.label}范围选择包含或排除时，需要至少填写一项`)
    return false
  }
  if (!draft.enabled || draft.mode === 'off') return true
  if (draft.keyword_mode === 'keyword_only' && draft.keywords.length === 0) {
    error('关键词模式需要至少配置一个关键词')
    return false
  }
  const regexError = validateRiskControlRegexConfig(draft)
  if (regexError) {
    error(regexError)
    return false
  }
  if (draft.keyword_mode !== 'keyword_only' && draft.provider.api_keys.length === 0) {
    error('API 审核模式需要至少配置一个 Provider API Key')
    return false
  }
  if (draft.keyword_mode !== 'keyword_only') {
    const providerError = validateRiskControlProviderBaseUrl(draft.provider.base_url)
    if (providerError) {
      error(providerError)
      return false
    }
  }
  if (draft.block_status < 400 || draft.block_status > 499) {
    error('拦截状态码必须是 4xx')
    return false
  }
  return true
}

async function loadAll() {
  loading.value = true
  try {
    const [configResponse, statusResponse] = await Promise.all([
      riskControlApi.getConfig(),
      riskControlApi.getStatus(),
      moduleStore.fetchModules(),
    ])
    config.value = cloneRiskControlConfig(configResponse.config)
    config.value.enabled = configResponse.enabled
    originalConfig.value = cloneRiskControlConfig(config.value)
    status.value = statusResponse
    testedProviderKeyStatuses.value = []
    syncEditors(config.value)
  } catch (err) {
    error(parseApiError(err, '加载风控中心配置失败'))
    log.error('加载风控中心配置失败:', err)
  } finally {
    loading.value = false
  }
}

async function refreshStatus() {
  try {
    status.value = await riskControlApi.getStatus()
    await moduleStore.fetchModules()
  } catch (err) {
    log.error('刷新风控中心状态失败:', err)
  }
}

async function saveConfig() {
  const draft = buildDraftConfig(true)
  if (!draft || !validateConfig(draft)) return
  saving.value = true
  try {
    const saved = await riskControlApi.updateConfig(draft.enabled, draft)
    config.value = cloneRiskControlConfig(saved.config)
    config.value.enabled = saved.enabled
    originalConfig.value = cloneRiskControlConfig(config.value)
    testedProviderKeyStatuses.value = []
    syncEditors(config.value)
    await refreshStatus()
    success('风控中心配置已保存')
  } catch (err) {
    error(parseApiError(err, '保存风控中心配置失败'))
    log.error('保存风控中心配置失败:', err)
  } finally {
    saving.value = false
  }
}

function addThresholdRow() {
  thresholdRows.value = [
    ...thresholdRows.value,
    {
      id: `threshold_${Date.now().toString(36)}`,
      category: '',
      value: '0.9',
    },
  ]
}

function updateThresholdRow(index: number, patch: Partial<ThresholdRow>) {
  const rows = [...thresholdRows.value]
  rows[index] = { ...rows[index], ...patch }
  thresholdRows.value = rows
}

function removeThresholdRow(index: number) {
  thresholdRows.value = thresholdRows.value.filter((_, itemIndex) => itemIndex !== index)
}

function normalizeFilterValue(value: string): string | undefined {
  const trimmed = value.trim()
  return trimmed && trimmed !== 'all' ? trimmed : undefined
}

function parseDateTimeLocalToUnix(value: string): number | undefined {
  const trimmed = value.trim()
  if (!trimmed) return undefined
  const parsed = new Date(trimmed)
  if (Number.isNaN(parsed.getTime())) return undefined
  return Math.floor(parsed.getTime() / 1000)
}

function buildLogFilters(): RiskControlLogFilters {
  const flagged = logFlaggedFilter.value === 'all' ? null : logFlaggedFilter.value === 'true'
  return {
    page: logsPage.value,
    page_size: logsPageSize.value,
    flagged,
    action: normalizeFilterValue(logFilters.action),
    decision_source: normalizeFilterValue(logFilters.decision_source),
    endpoint: normalizeFilterValue(logFilters.endpoint),
    q: normalizeFilterValue(logSearchText.value),
    from: parseDateTimeLocalToUnix(logDateFrom.value),
    to: parseDateTimeLocalToUnix(logDateTo.value),
  }
}

async function loadLogs() {
  logsLoading.value = true
  try {
    logs.value = await riskControlApi.listLogs(buildLogFilters())
  } catch (err) {
    error(parseApiError(err, '加载风控日志失败'))
    log.error('加载风控日志失败:', err)
  } finally {
    logsLoading.value = false
  }
}

function refreshLogs() {
  reloadLogsFromFirstPage()
}

function reloadLogsFromFirstPage() {
  if (logsPage.value === 1) {
    void loadLogs()
    return
  }
  logsPage.value = 1
}

function resetLogFilters() {
  logFlaggedFilter.value = 'all'
  logFilters.action = 'all'
  logFilters.decision_source = 'all'
  logFilters.endpoint = 'all'
  logSearchText.value = ''
  logDateFrom.value = ''
  logDateTo.value = ''
  reloadLogsFromFirstPage()
}

function openLogDetail(item: RiskControlLogItem) {
  selectedLog.value = item
  logDetailDialogOpen.value = true
}

function closeLogDetail() {
  logDetailDialogOpen.value = false
}

function deleteSelectedLogHash() {
  if (!selectedLog.value?.input_hash) return
  openDeleteHashDialog(selectedLog.value.input_hash)
}

async function loadHashes() {
  hashesLoading.value = true
  try {
    hashes.value = await riskControlApi.listHashes(hashesPage.value, hashesPageSize.value)
  } catch (err) {
    error(parseApiError(err, '加载命中哈希失败'))
    log.error('加载命中哈希失败:', err)
  } finally {
    hashesLoading.value = false
  }
}

function openDeleteHashDialog(inputHash: string) {
  pendingDangerAction.value = 'delete_hash'
  pendingHash.value = inputHash
  dangerDialogOpen.value = true
}

function openClearHashesDialog() {
  pendingDangerAction.value = 'clear_hashes'
  pendingHash.value = null
  dangerDialogOpen.value = true
}

function openRetentionDialog() {
  pendingDangerAction.value = 'run_retention'
  pendingHash.value = null
  dangerDialogOpen.value = true
}

function closeDangerDialog() {
  if (dangerDialogLoading.value) return
  dangerDialogOpen.value = false
  pendingDangerAction.value = null
  pendingHash.value = null
}

async function confirmDangerAction() {
  const action = pendingDangerAction.value
  if (!action) {
    closeDangerDialog()
    return
  }

  dangerConfirmLoading.value = true
  try {
    if (action === 'delete_hash') {
      if (!pendingHash.value) return
      await deleteHash(pendingHash.value)
    } else if (action === 'clear_hashes') {
      await clearHashes()
    } else if (action === 'run_retention') {
      await runRetention()
    }
    pendingDangerAction.value = null
    pendingHash.value = null
    dangerDialogOpen.value = false
  } finally {
    dangerConfirmLoading.value = false
  }
}

async function deleteHash(inputHash: string) {
  try {
    const result = await riskControlApi.deleteHash(inputHash)
    if (result.deleted) success('命中哈希已删除')
    if (selectedLog.value?.input_hash === inputHash) {
      selectedLog.value = {
        ...selectedLog.value,
        input_hash: null,
      }
    }
    await loadHashes()
    await refreshStatus()
  } catch (err) {
    error(parseApiError(err, '删除命中哈希失败'))
    log.error('删除命中哈希失败:', err)
  }
}

async function clearHashes() {
  try {
    const result = await riskControlApi.clearHashes()
    success(`已清空 ${result.deleted} 条命中哈希`)
    hashesPage.value = 1
    await loadHashes()
    await refreshStatus()
  } catch (err) {
    error(parseApiError(err, '清空命中哈希失败'))
    log.error('清空命中哈希失败:', err)
  }
}

async function runRetention() {
  runningRetention.value = true
  try {
    const result = await riskControlApi.runRetention()
    success(`已清理命中 ${result.hit_deleted} 条，未命中 ${result.non_hit_deleted} 条`)
    await loadLogs()
    await refreshStatus()
  } catch (err) {
    error(parseApiError(err, '清理过期日志失败'))
    log.error('清理过期日志失败:', err)
  } finally {
    runningRetention.value = false
  }
}

async function restoreSelectedUser() {
  const userId = selectedLog.value?.user_id
  if (!userId) return
  restoringAutoAction.value = 'user'
  try {
    const result = await riskControlApi.unbanUser(userId)
    if (result.updated) {
      success('用户已恢复')
      await loadLogs()
      await refreshStatus()
    } else {
      error('用户不存在或无需恢复')
    }
  } catch (err) {
    error(parseApiError(err, '恢复用户失败'))
    log.error('恢复用户失败:', err)
  } finally {
    restoringAutoAction.value = null
  }
}

async function unlockSelectedApiKey() {
  const userId = selectedLog.value?.user_id
  const apiKeyId = selectedLog.value?.api_key_id
  if (!userId || !apiKeyId) return
  restoringAutoAction.value = 'api_key'
  try {
    const result = await riskControlApi.unlockUserApiKey(userId, apiKeyId)
    if (result.updated) {
      success('API Key 已解锁')
      await loadLogs()
      await refreshStatus()
    } else {
      error('API Key 不存在或无需解锁')
    }
  } catch (err) {
    error(parseApiError(err, '解锁 API Key 失败'))
    log.error('解锁 API Key 失败:', err)
  } finally {
    restoringAutoAction.value = null
  }
}

function canRetryNotification(item: RiskControlLogItem | null): boolean {
  return notificationOutboxes(item).some(outbox => outbox.status !== 'sent')
}

async function retrySelectedNotification() {
  const logId = selectedLog.value?.id
  if (!logId) return
  retryingNotification.value = true
  try {
    const result = await riskControlApi.retryNotification(logId)
    if (result.queued && result.notifications.length > 0 && selectedLog.value) {
      const primary = result.notification ?? primaryNotificationOutbox({
        ...selectedLog.value,
        notification_outboxes: result.notifications,
      })
      selectedLog.value = {
        ...selectedLog.value,
        notification_outbox: primary,
        notification_outboxes: result.notifications,
      }
      logs.value = {
        ...logs.value,
        items: logs.value.items.map(item => item.id === logId
          ? {
              ...item,
              notification_outbox: primary,
              notification_outboxes: result.notifications,
            }
          : item),
      }
      success('通知已重新入队')
      await loadLogs()
      selectedLog.value = logs.value.items.find(item => item.id === logId) ?? selectedLog.value
      await refreshStatus()
    } else {
      error('没有可重试的通知任务')
    }
  } catch (err) {
    error(parseApiError(err, '通知重试入队失败'))
    log.error('通知重试入队失败:', err)
  } finally {
    retryingNotification.value = false
  }
}

async function runTest() {
  const draft = buildDraftConfig(true)
  if (!draft || !testText.value.trim()) {
    error('请输入测试文本')
    return
  }
  testingText.value = true
  try {
    testResult.value = await riskControlApi.testText(testText.value, draft)
  } catch (err) {
    error(parseApiError(err, '风控测试失败'))
    log.error('风控测试失败:', err)
  } finally {
    testingText.value = false
  }
}

async function testProvider() {
  const draft = buildDraftConfig(true)
  if (!draft) return
  const apiKeys = providerKeyLines.value.filter(value => !isMaskedProviderKey(value))
  if (providerKeyInputCount.value === 0) {
    error('请至少填写一个 API Key')
    return
  }
  testingProvider.value = true
  try {
    const result = await riskControlApi.testProviderKeys(apiKeys, draft)
    success(result.result.error_message
      ? 'Provider 返回异常'
      : apiKeys.length > 0
        ? 'Provider 明文 Key 测试完成'
        : '已用已保存 Key 测试 Provider')
    testResult.value = result
    testedProviderKeyStatuses.value = result.provider_key_statuses ?? []
    await refreshStatus()
    configDialogOpen.value = true
  } catch (err) {
    error(parseApiError(err, 'Provider 测试失败'))
    log.error('Provider 测试失败:', err)
  } finally {
    testingProvider.value = false
  }
}

function routeGroupText(item: RiskControlLogItem): string {
  return item.route_kind || item.route_family || item.api_format || '-'
}

function userPrimaryText(item: RiskControlLogItem): string {
  return item.username || item.user_email || item.user_id || '-'
}

function userSecondaryText(item: RiskControlLogItem): string {
  return item.user_id ? `UID ${compactHash(item.user_id)}` : ''
}

function apiKeyPrimaryText(item: RiskControlLogItem): string {
  return item.api_key_name || item.api_key_id || '-'
}

function apiKeySecondaryText(item: RiskControlLogItem): string {
  return item.api_key_name && item.api_key_id ? compactHash(item.api_key_id) : ''
}

function logExcerptText(item: RiskControlLogItem | null): string {
  if (!item) return '没有摘要'
  if (item.excerpt_redacted) return '摘要已隐藏'
  return item.excerpt || item.error_message || '没有摘要'
}

function logResultLabel(item: RiskControlLogItem): string {
  if (item.error_message || item.decision_source === 'api_error') return '异常'
  return item.flagged ? '命中' : '通过'
}

function logResultBadge(item: RiskControlLogItem): 'default' | 'success' | 'destructive' | 'warning' | 'secondary' | 'outline' {
  if (item.error_message || item.decision_source === 'api_error') return 'warning'
  return item.flagged ? 'destructive' : 'success'
}

function notificationStatusText(item: RiskControlLogItem): string {
  const outboxes = notificationOutboxes(item)
  const outbox = primaryNotificationOutbox(item)
  if (outboxes.length > 1) {
    const pending = outboxes.filter(entry => entry.status === 'pending').length
    const processing = outboxes.filter(entry => entry.status === 'processing').length
    const dead = outboxes.filter(entry => entry.status === 'dead').length
    const sent = outboxes.filter(entry => entry.status === 'sent').length
    return `通知 ${sent}/${outboxes.length} · 待 ${pending + processing} · 死信 ${dead}`
  }
  if (outbox?.status === 'pending') return `待通知 · ${outbox.attempt_count}/${outbox.max_attempts}`
  if (outbox?.status === 'processing') return `通知发送中 · ${outbox.attempt_count}/${outbox.max_attempts}`
  if (outbox?.status === 'dead') return `通知死信 · ${outbox.attempt_count}/${outbox.max_attempts}`
  if (outbox?.status === 'sent') return `已通知 · ${outbox.attempt_count || item.notification_attempts || 1} 次`
  if (item.notification_sent) return `已通知 · ${item.notification_attempts || 1} 次`
  if (item.notification_attempts > 0) return `通知失败 · ${item.notification_attempts} 次`
  return '未通知'
}

function notificationErrorText(item: RiskControlLogItem | null): string | null {
  if (!item) return null
  const outboxError = notificationOutboxes(item)
    .filter(outbox => outbox.status !== 'sent' && outbox.last_error)
    .sort((left, right) => (right.updated_at_unix_secs ?? 0) - (left.updated_at_unix_secs ?? 0))[0]
    ?.last_error
  if (outboxError) return outboxError
  return item.notification_last_error
}

function notificationOutboxes(item: RiskControlLogItem | null): NonNullable<RiskControlLogItem['notification_outbox']>[] {
  if (!item) return []
  if (item.notification_outboxes?.length) return item.notification_outboxes
  return item.notification_outbox ? [item.notification_outbox] : []
}

function primaryNotificationOutbox(item: RiskControlLogItem | null): RiskControlLogItem['notification_outbox'] {
  const outboxes = notificationOutboxes(item)
  if (outboxes.length === 0) return null
  return [...outboxes].sort((left, right) => {
    const leftRank = notificationOutboxRank(left.status)
    const rightRank = notificationOutboxRank(right.status)
    if (leftRank !== rightRank) return leftRank - rightRank
    return (right.updated_at_unix_secs ?? 0) - (left.updated_at_unix_secs ?? 0)
  })[0]
}

function notificationOutboxRank(status: string): number {
  if (status === 'dead') return 0
  if (status === 'processing') return 1
  if (status === 'pending') return 2
  if (status === 'sent') return 3
  return 4
}

function actionBadge(action: string): 'default' | 'success' | 'destructive' | 'warning' | 'secondary' | 'outline' {
  if (action === 'block') return 'destructive'
  if (action === 'allow') return 'success'
  if (action === 'observe') return 'warning'
  return 'secondary'
}

function formatScore(value: number | null | undefined): string {
  if (typeof value !== 'number' || !Number.isFinite(value)) return '-'
  return value.toFixed(3)
}

function scorePercent(value: number | null | undefined): string {
  if (typeof value !== 'number' || !Number.isFinite(value)) return '0%'
  return `${Math.round(Math.min(1, Math.max(0, value)) * 100)}%`
}

function formatLatency(value: number | null | undefined): string {
  if (typeof value !== 'number' || !Number.isFinite(value)) return '-'
  return `${value} ms`
}

function formatBoolean(value: boolean): string {
  return value ? '是' : '否'
}

function formatScanChars(value: number | null | undefined): string {
  if (typeof value !== 'number' || !Number.isFinite(value)) return '0 字符'
  const normalized = Math.max(0, Math.round(value))
  if (normalized >= 1024 * 1024) return `${(normalized / 1024 / 1024).toFixed(1)} MiB`
  if (normalized >= 1024) return `${(normalized / 1024).toFixed(1)} KiB`
  return `${normalized} 字符`
}

function formatLogJson(item: RiskControlLogItem): string {
  return JSON.stringify(item, null, 2)
}

function formatDate(value: string | null | undefined): string {
  if (!value) return '-'
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return value
  return date.toLocaleString()
}

function compactHash(value: string): string {
  if (value.length <= 24) return value
  return `${value.slice(0, 12)}...${value.slice(-10)}`
}

onMounted(() => {
  void loadAll()
  void loadLogs()
  void loadHashes()
})
</script>

<style scoped>
.risk-radar-sweep {
  background:
    conic-gradient(from 0deg, rgb(249 115 22 / 0.26), transparent 36%, transparent 100%),
    radial-gradient(circle, rgb(251 146 60 / 0.14), transparent 62%);
  animation: risk-radar-rotate 4.5s linear infinite;
}

.risk-config-tabs :deep(button[data-value]) {
  flex: 1 1 7.25rem;
  max-width: 9.5rem;
}

.risk-detail-list dd {
  min-width: 0;
  overflow-wrap: anywhere;
}

@media (max-width: 639px) {
  .risk-config-tabs :deep(button[data-value]) {
    flex-basis: calc((100% - 0.5rem) / 2);
    max-width: none;
  }

  .risk-detail-list > div {
    align-items: flex-start;
    flex-direction: column;
    gap: 0.25rem;
  }

  .risk-detail-list dd {
    width: 100%;
    text-align: left;
  }
}

@media (min-width: 1024px) {
  .risk-config-tabs :deep(button[data-value]) {
    flex-basis: 0;
    max-width: none;
  }
}

@keyframes risk-radar-rotate {
  to {
    transform: rotate(360deg);
  }
}
</style>
