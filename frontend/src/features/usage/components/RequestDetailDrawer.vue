<template>
  <!-- 请求详情抽屉 -->
  <Teleport to="body">
    <Transition name="drawer">
      <div
        v-if="isOpen"
        class="fixed inset-0 z-50 flex justify-end"
        @click.self="handleClose"
      >
        <!-- 背景遮罩 -->
        <div
          class="absolute inset-0 bg-black/30 backdrop-blur-sm"
          @click="handleClose"
        />

        <!-- 抽屉内容 -->
        <Card class="relative h-full w-full sm:w-[800px] sm:max-w-[90vw] rounded-none shadow-2xl flex flex-col">
          <!-- 固定头部 - 整合基本信息 -->
          <div class="sticky top-0 z-10 bg-background border-b px-3 sm:px-6 py-3 sm:py-4 flex-shrink-0">
            <!-- 第一行：标题、模型、状态、操作按钮 -->
            <div class="flex items-center justify-between gap-4 mb-3">
              <div class="flex items-center gap-3 flex-wrap">
                <h3 class="text-lg font-semibold">
                  请求详情
                </h3>
                <div class="flex items-center gap-1 text-sm font-mono text-muted-foreground bg-muted px-2 py-0.5 rounded">
                  <span>{{ detail?.model || '-' }}</span>
                  <template v-if="detail?.target_model && detail.target_model !== detail.model">
                    <svg
                      xmlns="http://www.w3.org/2000/svg"
                      viewBox="0 0 20 20"
                      fill="currentColor"
                      class="w-3 h-3 flex-shrink-0"
                    >
                      <path
                        fill-rule="evenodd"
                        d="M3 10a.75.75 0 01.75-.75h10.638L10.23 5.29a.75.75 0 111.04-1.08l5.5 5.25a.75.75 0 010 1.08l-5.5 5.25a.75.75 0 11-1.04-1.08l4.158-3.96H3.75A.75.75 0 013 10z"
                        clip-rule="evenodd"
                      />
                    </svg>
                    <span>{{ detail.target_model }}</span>
                  </template>
                </div>
                <Badge
                  v-if="detail?.status_code === 200"
                  variant="success"
                >
                  {{ detail.status_code }}
                </Badge>
                <Badge
                  v-else-if="detail"
                  variant="destructive"
                >
                  {{ detail.status_code }}
                </Badge>
                <Badge
                  v-if="detail && resolveUsageStreamLabelSegments(detail).hasConversion"
                  :variant="resolveUsageStreamLabelSegments(detail).client === '流式' ? 'secondary' : 'outline'"
                  :class="resolveUsageStreamLabelSegments(detail).client === '流式'
                    ? 'text-xs inline-flex items-center gap-1'
                    : 'text-xs inline-flex items-center gap-1 border-border/60 text-muted-foreground'"
                >
                  <span>{{ resolveUsageStreamLabelSegments(detail).client }}</span>
                  <span class="opacity-60">→</span>
                  <span>{{ resolveUsageStreamLabelSegments(detail).upstream }}</span>
                </Badge>
                <Badge
                  v-else-if="detail"
                  :variant="isUsageUpstreamStream(detail) ? 'secondary' : 'outline'"
                  :class="isUsageUpstreamStream(detail)
                    ? 'text-xs'
                    : 'text-xs border-border/60 text-muted-foreground'"
                >
                  {{ formatUsageStreamLabel(detail) }}
                </Badge>
              </div>
              <div class="flex items-center gap-1 shrink-0">
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-8 w-8"
                  title="回放请求"
                  :disabled="loading"
                  @click="openReplayDialog"
                >
                  <Play class="w-4 h-4" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-8 w-8"
                  :disabled="loading && !autoRefreshing"
                  :title="refreshButtonTitle"
                  @click="refreshDetail"
                >
                  <RefreshCw
                    class="w-4 h-4"
                    :class="{ 'animate-spin': loading || autoRefreshing, 'text-primary': autoRefreshing }"
                  />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-8 w-8"
                  title="关闭"
                  @click="handleClose"
                >
                  <X class="w-4 h-4" />
                </Button>
              </div>
            </div>
            <!-- 第二行：关键元信息 -->
            <div
              v-if="detail"
              class="flex items-center flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground"
            >
              <span class="flex items-center gap-1">
                <span class="font-medium text-foreground">ID:</span>
                <span
                  class="font-mono"
                  :title="fullRequestId"
                >{{ displayRequestId }}</span>
              </span>
              <span class="opacity-40">|</span>
              <span>{{ formatDateTime(detail.created_at) }}</span>
              <span class="opacity-40">|</span>
              <span>{{ formatApiFormat(detail.api_format) }}</span>
              <span class="opacity-40">|</span>
              <span>用户: {{ detail.user?.username || 'Unknown' }}</span>
            </div>
          </div>

          <!-- 可滚动内容区域 -->
          <div class="flex-1 min-h-0 overflow-y-auto px-3 sm:px-6 py-3 sm:py-4 scrollbar-stable">
            <!-- Loading State -->
            <div
              v-if="loading"
              class="py-8 space-y-4"
            >
              <Skeleton class="h-8 w-full" />
              <Skeleton class="h-32 w-full" />
              <Skeleton class="h-64 w-full" />
            </div>

            <!-- Error State -->
            <Card
              v-else-if="error"
              class="border-red-200 dark:border-red-800"
            >
              <div class="p-4">
                <p class="text-sm text-red-600 dark:text-red-400">
                  {{ error }}
                </p>
              </div>
            </Card>

            <!-- Detail Content -->
            <div
              v-else-if="detail"
              class="space-y-4"
            >
              <!-- 费用与性能概览 -->
              <Card>
                <div class="p-3 sm:p-4">
                  <!-- 总费用和响应时间（独立显示） -->
                  <div class="flex items-center mb-4">
                    <div class="flex items-center">
                      <span class="text-xs text-muted-foreground w-[56px]">总费用</span>
                      <span class="text-lg font-bold text-green-600 dark:text-green-400">
                        ${{ ((typeof detail.cost === 'object' ? detail.cost?.total : detail.cost) || detail.total_cost || 0).toFixed(6) }}
                      </span>
                    </div>
                    <Separator
                      orientation="vertical"
                      class="h-6 mx-6"
                    />
                    <div class="flex items-center">
                      <span class="text-xs text-muted-foreground w-[56px]">响应时间</span>
                      <span class="text-lg font-bold">{{ detail.response_time_ms ? formatResponseTime(detail.response_time_ms).value : 'N/A' }}</span>
                      <span class="text-sm text-muted-foreground ml-1">{{ detail.response_time_ms ? formatResponseTime(detail.response_time_ms).unit : '' }}</span>
                    </div>
                  </div>

                  <!-- 分隔线 -->
                  <Separator class="mb-4" />

                  <!-- ========== 1. 费用聚合计算 ========== -->
                  <div class="text-xs text-muted-foreground mb-3 flex items-center gap-2 flex-wrap">
                    <span class="text-[10px] px-1.5 py-0.5 rounded bg-muted text-muted-foreground/70">{{ priceSourceLabel }}</span>
                    <span class="text-foreground">|</span>
                    <span class="font-mono text-foreground">
                      总费用 = Token费用 <span class="font-medium">${{ tokenCostTotal.toFixed(6) }}</span>
                      <template v-if="perRequestCost > 0">
                        + 按次费用 <span class="font-medium">${{ perRequestCost.toFixed(6) }}</span>
                      </template>
                      <template v-if="videoCostTotal > 0">
                        + {{ detail.video_billing?.task_type === 'image' ? '图像' : detail.video_billing?.task_type === 'audio' ? '音频' : '视频' }}费用 <span class="font-medium">${{ videoCostTotal.toFixed(6) }}</span>
                      </template>
                    </span>
                  </div>

                  <!-- ========== 2. Token分阶段成本 ========== -->
                  <div
                    v-if="hasTokenCost"
                    class="space-y-2 mb-3"
                  >
                    <!-- 阶梯标题 -->
                    <div class="text-xs text-muted-foreground flex items-center gap-2 flex-wrap">
                      <span class="font-medium text-foreground">Token 计费</span>
                      <span class="text-muted-foreground/60">(输入 {{ formatNumber(displayInputTokens) }} + 缓存创建 {{ cacheCreationSummaryText }} + 缓存读取 {{ formatNumber(detail.cache_read_input_tokens || 0) }})</span>
                      <Badge
                        v-if="displayTiers.length > 1"
                        variant="outline"
                        class="text-[10px] px-1.5 py-0 h-4"
                      >
                        命中第 {{ currentTierIndex + 1 }} 阶
                      </Badge>
                    </div>

                    <!-- 阶梯展示 -->
                    <div
                      v-for="(tier, index) in displayTiers"
                      :key="index"
                      class="rounded-lg p-3 space-y-2"
                      :class="index === currentTierIndex
                        ? 'bg-primary/5 border border-primary/30'
                        : 'bg-muted/20 border border-border/50 opacity-60'"
                    >
                      <!-- 阶梯标题行 -->
                      <div class="flex flex-col sm:flex-row sm:items-center sm:justify-between gap-1 sm:gap-2 text-xs">
                        <div class="flex items-center gap-2">
                          <span
                            class="font-medium"
                            :class="index === currentTierIndex ? 'text-primary' : 'text-muted-foreground'"
                          >
                            第 {{ index + 1 }} 阶
                          </span>
                          <span class="text-muted-foreground">
                            {{ getTierRangeText(tier, index, displayTiers) }}
                          </span>
                          <Badge
                            v-if="index === currentTierIndex"
                            variant="default"
                            class="text-[10px] px-1.5 py-0 h-4"
                          >
                            当前
                          </Badge>
                        </div>
                        <!-- 单价信息 -->
                        <div class="text-muted-foreground flex items-center gap-2 flex-wrap">
                          <span>输入 ${{ formatPrice(tier.input_price_per_1m) }}/M</span>
                          <span>输出 ${{ formatPrice(tier.output_price_per_1m) }}/M</span>
                          <span v-if="getTierActiveCacheCreationDisplay(tier)">
                            {{ getTierActiveCacheCreationDisplay(tier)?.label }}
                            ${{ formatPrice(getTierActiveCacheCreationDisplay(tier)?.price || 0) }}/M
                          </span>
                          <span v-if="shouldShowCacheReadPrice(tier)">
                            缓存读取 ${{ formatPrice(getTierActiveCacheReadPrice(tier) ?? 0) }}/M
                          </span>
                        </div>
                      </div>

                      <!-- 当前阶梯的详细计算 -->
                      <template v-if="index === currentTierIndex">
                        <!-- 输入 输出 -->
                        <div class="flex items-center">
                          <div class="flex items-center flex-1">
                            <span class="text-xs text-muted-foreground w-[56px]">输入</span>
                            <span class="text-sm font-semibold font-mono flex-1 text-center">{{ displayInputTokens }}</span>
                            <span class="text-xs font-mono">${{ effectiveInputCost.toFixed(6) }}</span>
                          </div>
                          <Separator
                            orientation="vertical"
                            class="h-4 mx-4"
                          />
                          <div class="flex items-center flex-1">
                            <span class="text-xs text-muted-foreground w-[56px]">输出</span>
                            <span class="text-sm font-semibold font-mono flex-1 text-center">{{ detail.tokens?.output || detail.output_tokens || 0 }}</span>
                            <span class="text-xs font-mono">${{ effectiveOutputCost.toFixed(6) }}</span>
                          </div>
                        </div>
                        <!-- 缓存创建 缓存读取 -->
                        <div class="flex items-center">
                          <div class="flex items-center flex-1">
                            <span class="text-xs text-muted-foreground w-[56px]">{{ cacheCreationSplitRows.length > 0 ? '创建合计' : '缓存创建' }}</span>
                            <span class="text-sm font-semibold font-mono flex-1 text-center">{{ totalCacheCreationTokens }}</span>
                            <span class="text-xs font-mono">${{ effectiveCacheCreationCost.toFixed(6) }}</span>
                          </div>
                          <Separator
                            orientation="vertical"
                            class="h-4 mx-4"
                          />
                          <div class="flex items-center flex-1">
                            <span class="text-xs text-muted-foreground w-[56px]">缓存读取</span>
                            <span class="text-sm font-semibold font-mono flex-1 text-center">{{ detail.cache_read_input_tokens || 0 }}</span>
                            <span class="text-xs font-mono">${{ effectiveCacheReadCost.toFixed(6) }}</span>
                          </div>
                        </div>
                        <!-- 缓存创建 5m/1h 细分 -->
                        <div
                          v-if="cacheCreationSplitRows.length > 0"
                          class="space-y-1 pl-[56px]"
                        >
                          <div
                            v-for="row in cacheCreationSplitRows"
                            :key="row.key"
                            class="flex items-center gap-4 text-xs text-muted-foreground/70"
                          >
                            <span class="w-[72px]">{{ row.label }}</span>
                            <span class="font-mono text-foreground/90">{{ formatNumber(row.tokens) }}</span>
                            <span v-if="row.pricePer1M !== null">${{ formatPrice(row.pricePer1M) }}/M</span>
                            <span
                              v-if="row.cost !== null"
                              class="font-mono"
                            >${{ row.cost.toFixed(6) }}</span>
                          </div>
                        </div>
                      </template>
                    </div>
                  </div>

                  <!-- ========== 3. 按次计费 ========== -->
                  <div
                    v-if="perRequestCost > 0 && !detail.video_billing"
                    class="space-y-2 mb-3"
                  >
                    <div class="flex items-center justify-between text-xs">
                      <span class="font-medium text-foreground">按次计费</span>
                    </div>
                    <div class="rounded-lg p-3 bg-primary/5 border border-primary/30 space-y-2">
                      <div
                        v-if="effectivePricePerRequest > 0"
                        class="flex items-center justify-end text-xs"
                      >
                        <span class="text-muted-foreground">${{ effectivePricePerRequest.toFixed(6) }}/次</span>
                      </div>
                      <div class="flex items-center">
                        <div class="flex items-center flex-1">
                          <span class="text-xs text-muted-foreground w-[56px]">请求次数</span>
                          <span class="text-sm font-semibold font-mono flex-1 text-center">1</span>
                          <span class="text-xs font-mono font-medium">${{ perRequestCost.toFixed(6) }}</span>
                        </div>
                      </div>
                    </div>
                  </div>

                  <!-- ========== 4. 视频/图像/音频计费（独立隔离，与Token计费风格一致） ========== -->
                  <div
                    v-if="detail.video_billing"
                    class="rounded-lg p-3 space-y-2 bg-primary/5 border border-primary/30"
                  >
                    <!-- 标题行（与阶梯标题行风格一致） -->
                    <div class="flex flex-col sm:flex-row sm:items-center sm:justify-between gap-1 sm:gap-2 text-xs">
                      <div class="flex items-center gap-2">
                        <span class="font-medium text-primary">
                          {{ getTaskTypeLabel(detail.video_billing.task_type) }}
                        </span>
                        <span
                          v-if="detail.video_billing.resolution"
                          class="text-muted-foreground"
                        >
                          {{ detail.video_billing.resolution }}
                        </span>
                      </div>
                      <!-- 费用计算公式 -->
                      <div class="text-muted-foreground flex items-center gap-2 flex-wrap">
                        <span
                          v-if="detail.video_billing.duration_seconds && detail.video_billing.video_price_per_second"
                          class="font-mono"
                        >
                          {{ detail.video_billing.duration_seconds.toFixed(1) }}s × ${{ detail.video_billing.video_price_per_second.toFixed(4) }}/s = ${{ videoCostTotal.toFixed(6) }}
                        </span>
                        <span
                          v-else-if="detail.video_billing.video_price_per_second"
                          class="font-mono"
                        >
                          ${{ detail.video_billing.video_price_per_second.toFixed(4) }}/秒
                        </span>
                      </div>
                    </div>

                    <!-- 费用详情（与Token详情行风格一致） -->
                    <div class="flex items-center">
                      <div class="flex items-center flex-1">
                        <span class="text-xs text-muted-foreground w-[56px]">
                          {{ detail.video_billing.task_type === 'video' ? '时长' : detail.video_billing.task_type === 'audio' ? '时长' : '数量' }}
                        </span>
                        <span class="text-sm font-semibold font-mono flex-1 text-center">
                          {{ detail.video_billing.duration_seconds ? formatDuration(detail.video_billing.duration_seconds) : '1' }}
                        </span>
                        <span class="text-xs font-mono">${{ videoCostTotal.toFixed(6) }}</span>
                      </div>
                      <Separator
                        orientation="vertical"
                        class="h-4 mx-4 invisible"
                      />
                      <div class="flex items-center flex-1 invisible">
                        <span class="text-xs text-muted-foreground w-[56px]">占位</span>
                        <span class="text-sm font-semibold font-mono flex-1 text-center">0</span>
                        <span class="text-xs font-mono">$0.000000</span>
                      </div>
                    </div>
                  </div>
                </div>
              </Card>

              <!-- 请求链路追踪卡片 -->
              <div>
                <HorizontalRequestTimeline
                  v-if="showTimeline && (detail.request_id || detail.id)"
                  ref="timelineRef"
                  :request-id="detail.request_id || detail.id"
                  :override-status-code="detail.status_code"
                  :request-status="detail.status"
                  :request-api-format="detail.api_format || null"
                  :request-metadata="traceRequestMetadata"
                  @trace-state="handleTraceState"
                />
              </div>

              <!-- 错误域卡片：保持上游响应与客户端响应两个边界可对照 -->
              <div
                v-if="hasVisibleErrorCards"
                class="space-y-3"
              >
                <div
                  class="grid gap-3"
                  :class="visibleErrorCardCount > 1 ? 'lg:grid-cols-2' : 'grid-cols-1'"
                >
                  <Card
                    v-if="displayClientErrorMessage"
                    class="border-amber-200 dark:border-amber-800"
                  >
                    <div class="p-4">
                      <h4 class="text-sm font-semibold text-amber-700 dark:text-amber-300 mb-2">
                        返回客户端错误
                      </h4>
                      <div class="bg-amber-50 dark:bg-amber-900/20 rounded-lg p-3 space-y-1">
                        <p class="text-sm text-amber-900 dark:text-amber-200">
                          {{ displayClientErrorMessage }}
                        </p>
                      </div>
                    </div>
                  </Card>

                  <Card
                    v-if="normalizedUpstreamError"
                    class="border-orange-200 dark:border-orange-800"
                  >
                    <div class="p-4">
                      <h4 class="text-sm font-semibold text-orange-700 dark:text-orange-300 mb-2">
                        上游响应错误
                      </h4>
                      <div class="bg-orange-50 dark:bg-orange-900/20 rounded-lg p-3 space-y-1">
                        <p class="text-sm text-orange-900 dark:text-orange-200">
                          {{ normalizedUpstreamError.message }}
                        </p>
                        <p
                          v-if="formatErrorDomainMeta(normalizedUpstreamError)"
                          class="text-xs text-orange-800/70 dark:text-orange-200/70 font-mono"
                        >
                          {{ formatErrorDomainMeta(normalizedUpstreamError) }}
                        </p>
                      </div>
                    </div>
                  </Card>
                </div>
              </div>

              <!-- Tabs 区域 -->
              <Card>
                <div class="p-3 sm:p-4">
                  <Tabs
                    v-model="activeTab"
                    :default-value="activeTab"
                  >
                    <!-- Tab 行 -->
                    <div class="flex items-center border-b pb-2 mb-3">
                      <button
                        v-for="tab in visibleTabs"
                        :key="tab.name"
                        class="px-2 sm:px-3 py-1.5 text-sm transition-colors border-b-2 -mb-[9px] whitespace-nowrap"
                        :class="activeTab === tab.name
                          ? 'border-primary text-foreground font-medium'
                          : 'border-transparent text-muted-foreground hover:text-foreground'"
                        @click="activeTab = tab.name"
                      >
                        {{ tab.label }}
                      </button>
                    </div>

                    <!-- Tab 内容（统一容器） -->
                    <JsonContentPanel
                      class="content-block"
                      :title="activeJsonPanelTitle"
                      :data="activeJsonPanelData"
                      :is-dark="isDark"
                      :expand-depth="currentExpandDepth"
                      :copied="Boolean(copiedStates[activeTab])"
                      :custom-copy="true"
                      :expand-disabled="viewMode === 'compare' || (supportsConversationView && contentViewMode === 'conversation')"
                      :copy-disabled="viewMode === 'compare'"
                      max-height="500px"
                      @update:expand-depth="currentExpandDepth = $event"
                      @copy="copyContent(activeTab)"
                    >
                      <template #toolbar-actions-before>
                        <!-- 区域1：条件性按钮（cURL、视图切换、对比） -->
                        <!-- cURL 复制（仅在请求头/请求体 Tab） -->
                        <template v-if="['request-headers', 'request-body'].includes(activeTab)">
                          <button
                            :title="curlCopied ? '已复制 cURL' : '复制 cURL'"
                            class="p-1 rounded transition-colors text-muted-foreground hover:bg-muted"
                            :disabled="curlCopying"
                            @click="copyCurlCommand"
                          >
                            <Check
                              v-if="curlCopied"
                              class="w-3.5 h-3.5 text-green-500"
                            />
                            <Terminal
                              v-else
                              class="w-3.5 h-3.5"
                              :class="{ 'animate-pulse': curlCopying }"
                            />
                          </button>
                        </template>

                        <!-- 请求体/响应体专用：JSON/对话 视图切换 -->
                        <template v-if="supportsConversationView">
                          <button
                            :title="contentViewMode === 'json' ? '切换到对话视图' : '切换到 JSON 视图'"
                            class="p-1 rounded transition-colors"
                            :class="hasValidConversation || contentViewMode === 'conversation'
                              ? 'text-muted-foreground hover:bg-muted'
                              : 'text-muted-foreground/40 cursor-not-allowed'"
                            :disabled="!hasValidConversation && contentViewMode === 'json'"
                            @click="toggleContentView"
                          >
                            <Code2
                              v-if="contentViewMode === 'conversation'"
                              class="w-3.5 h-3.5"
                            />
                            <MessageSquareText
                              v-else
                              class="w-3.5 h-3.5"
                            />
                          </button>
                        </template>

                        <!-- 请求头/响应头：对比模式 -->
                        <template v-if="canCompare">
                          <button
                            title="对比"
                            class="p-1 rounded transition-colors"
                            :class="viewMode === 'compare' ? 'bg-primary text-primary-foreground' : 'text-muted-foreground hover:bg-muted'"
                            @click="viewMode = 'compare'"
                          >
                            <Columns2 class="w-3.5 h-3.5" />
                          </button>
                        </template>

                        <!-- 区域2：客户端/提供商切换 -->
                        <template v-if="showDataSourceToggle">
                          <div class="w-px h-3.5 bg-border mx-0.5" />
                          <button
                            title="客户端"
                            class="p-1 rounded transition-colors"
                            :class="activeDataSource === 'client' ? 'bg-primary text-primary-foreground' : 'text-muted-foreground hover:bg-muted'"
                            @click="setDataSource('client')"
                          >
                            <Monitor class="w-3.5 h-3.5" />
                          </button>
                          <button
                            title="提供商"
                            class="p-1 rounded transition-colors"
                            :class="activeDataSource === 'provider' ? 'bg-primary text-primary-foreground' : 'text-muted-foreground hover:bg-muted'"
                            @click="setDataSource('provider')"
                          >
                            <Server class="w-3.5 h-3.5" />
                          </button>
                        </template>

                        <!-- 区域3：常驻按钮（展开/收缩、复制） -->
                        <div class="w-px h-3.5 bg-border mx-0.5" />
                      </template>
                      <TabsContent value="request-headers">
                        <RequestHeadersContent
                          :detail="detail"
                          :view-mode="viewMode"
                          :data-source="dataSource"
                          :current-header-data="currentHeaderData"
                          :current-expand-depth="currentExpandDepth"
                          :has-provider-headers="hasProviderHeaders"
                          :header-stats="headerStats"
                          :is-dark="isDark"
                        />
                      </TabsContent>

                      <TabsContent value="request-body">
                        <div
                          v-if="isRequestBodyLoading"
                          class="p-4"
                        >
                          <Skeleton class="h-32 w-full" />
                        </div>
                        <ConversationView
                          v-else-if="contentViewMode === 'conversation'"
                          :render-result="requestRenderResult"
                          empty-message="无请求体信息"
                        />
                        <JsonContent
                          v-else
                          :data="currentRequestBody"
                          :view-mode="viewMode"
                          :expand-depth="currentExpandDepth"
                          :is-dark="isDark"
                          empty-message="无请求体信息"
                        />
                      </TabsContent>

                      <TabsContent value="response-headers">
                        <RequestHeadersContent
                          v-if="viewMode === 'compare'"
                          :detail="detail"
                          :view-mode="viewMode"
                          :data-source="dataSource"
                          :current-header-data="currentResponseHeaderData"
                          :current-expand-depth="currentExpandDepth"
                          :has-provider-headers="hasProviderResponseHeaders"
                          :header-stats="responseHeaderStats"
                          :is-dark="isDark"
                          :client-headers="detail.client_response_headers"
                          :provider-headers="detail.response_headers"
                          client-label="客户端响应头"
                          provider-label="提供商响应头"
                          empty-message="无响应头信息"
                        />
                        <JsonContent
                          v-else
                          :data="currentResponseHeaderData"
                          :view-mode="viewMode"
                          :expand-depth="currentExpandDepth"
                          :is-dark="isDark"
                          empty-message="无响应头信息"
                        />
                      </TabsContent>

                      <TabsContent value="response-body">
                        <div
                          v-if="isResponseBodyLoading"
                          class="p-4"
                        >
                          <Skeleton class="h-32 w-full" />
                        </div>
                        <ConversationView
                          v-else-if="contentViewMode === 'conversation'"
                          :render-result="responseRenderResult"
                          empty-message="无响应体信息"
                        />
                        <JsonContent
                          v-else
                          :data="currentResponseBody"
                          :view-mode="viewMode"
                          :expand-depth="currentExpandDepth"
                          :is-dark="isDark"
                          empty-message="无响应体信息"
                        />
                      </TabsContent>

                      <TabsContent value="metadata">
                        <JsonContent
                          :data="metadataPanelData"
                          :view-mode="viewMode"
                          :expand-depth="currentExpandDepth"
                          :is-dark="isDark"
                          empty-message="无元数据信息"
                        />
                      </TabsContent>
                    </JsonContentPanel>
                  </Tabs>
                </div>
              </Card>
            </div>
          </div>
        </Card>
      </div>
    </Transition>
  </Teleport>

  <!-- 请求回放对话框 -->
  <ReplayDialog
    :is-open="replayDialogOpen"
    :request-id="requestId"
    :detail="detail"
    @close="replayDialogOpen = false"
  />
</template>

<script setup lang="ts">
import { ref, watch, computed, onMounted, onBeforeUnmount } from 'vue'
import Button from '@/components/ui/button.vue'
import { useEscapeKey } from '@/composables/useEscapeKey'
import { useClipboard } from '@/composables/useClipboard'
import Card from '@/components/ui/card.vue'
import Badge from '@/components/ui/badge.vue'
import Separator from '@/components/ui/separator.vue'
import Skeleton from '@/components/ui/skeleton.vue'
import Tabs from '@/components/ui/tabs.vue'
import TabsContent from '@/components/ui/tabs-content.vue'
import { Check, Columns2, RefreshCw, X, Monitor, Server, MessageSquareText, Code2, Terminal, Play } from 'lucide-vue-next'
import { dashboardApi, type RequestDetail, type RequestErrorDomain } from '@/api/dashboard'
import { formatApiFormat } from '@/api/endpoints/types/api-format'
import { formatShortRequestId } from '@/utils/format'
import { log } from '@/utils/logger'
import { getEffectiveInputTokens } from '../token-normalization'
import {
  formatUsageStreamLabel,
  isUsageUpstreamStream,
  resolveDisplayRequestStatus,
  resolveUsageStreamLabelSegments,
} from '../utils/status'

// 子组件
import RequestHeadersContent from './RequestDetailDrawer/RequestHeadersContent.vue'
import JsonContent from './RequestDetailDrawer/JsonContent.vue'
import JsonContentPanel from './JsonContentPanel.vue'
import ConversationView from './RequestDetailDrawer/ConversationView.vue'
import HorizontalRequestTimeline from './HorizontalRequestTimeline.vue'
import ReplayDialog from './ReplayDialog.vue'

// 对话解析器
import {
  renderRequest,
  renderResponse,
  type RenderResult,
  type RenderBlock,
} from '../conversation'

const props = defineProps<{
  isOpen: boolean
  requestId: string | null
}>()

const emit = defineEmits<{
  close: []
}>()

const loading = ref(false)
const error = ref<string | null>(null)
const detail = ref<RequestDetail | null>(null)
const timelineRef = ref<InstanceType<typeof HorizontalRequestTimeline> | null>(null)
const timelineLoaded = ref(false)
const timelineHasTrace = ref(false)
const activeTab = ref('request-body')
const copiedStates = ref<Record<string, boolean>>({})
const viewMode = ref<'compare' | 'formatted' | 'raw'>('formatted')
const currentExpandDepth = ref(0)
const dataSource = ref<'client' | 'provider'>('provider')
const contentViewMode = ref<'json' | 'conversation'>('json')
const { copyToClipboard } = useClipboard()
const historicalPricing = ref<{
  input_price: string
  output_price: string
  cache_creation_price: string
  cache_read_price: string
  request_price: string
} | null>(null)

type CacheTTLPriceEntry = {
  ttl_minutes?: number | null
  cache_creation_price_per_1m?: number | null
  cache_read_price_per_1m?: number | null
}

type PricingTierLike = {
  up_to?: number | null
  input_price_per_1m?: number | null
  output_price_per_1m?: number | null
  cache_creation_price_per_1m?: number | null
  cache_read_price_per_1m?: number | null
  cache_ttl_pricing?: CacheTTLPriceEntry[] | null
}

type JsonRecord = Record<string, unknown>

type NormalizedErrorDomain = {
  source?: string | null
  status_code?: number | null
  type?: string | null
  message: string
  code?: string | number | null
  category?: string | null
}

function asRecord(value: unknown): JsonRecord | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null
  return value as JsonRecord
}

function normalizeErrorDomain(domain: RequestErrorDomain | null | undefined): NormalizedErrorDomain | null {
  if (!domain || typeof domain !== 'object') return null
  const message = typeof domain.message === 'string' ? domain.message.trim() : ''
  if (!message) return null
  return {
    source: domain.source ?? null,
    status_code: domain.status_code ?? null,
    type: domain.type ?? null,
    message,
    code: domain.code ?? null,
    category: domain.category ?? null,
  }
}

function formatErrorDomainMeta(domain: NormalizedErrorDomain): string {
  const parts: string[] = []
  if (domain.status_code != null) parts.push(`HTTP ${domain.status_code}`)
  if (domain.type) parts.push(domain.type)
  if (domain.source) parts.push(`source=${domain.source}`)
  return parts.join(' · ')
}

function handleTraceState(state: { loaded: boolean, hasTrace: boolean }) {
  timelineLoaded.value = state.loaded
  timelineHasTrace.value = state.hasTrace
}

function toNumber(value: unknown): number | null {
  if (typeof value === 'number' && Number.isFinite(value)) return value
  if (typeof value === 'string') {
    const parsed = Number(value)
    return Number.isFinite(parsed) ? parsed : null
  }
  return null
}

function getNestedValue(record: JsonRecord | null, ...path: string[]): unknown {
  let current: unknown = record
  for (const key of path) {
    const object = asRecord(current)
    if (!object) return null
    current = object[key]
  }
  return current
}

function getNestedNumber(record: JsonRecord | null, ...path: string[]): number | null {
  return toNumber(getNestedValue(record, ...path))
}

function normalizeCacheTtlPricing(value: unknown): CacheTTLPriceEntry[] {
  if (!Array.isArray(value)) return []
  return value
    .map((entry) => asRecord(entry))
    .filter((entry): entry is JsonRecord => entry !== null)
    .map((entry) => ({
      ttl_minutes: toNumber(entry.ttl_minutes),
      cache_creation_price_per_1m: toNumber(entry.cache_creation_price_per_1m),
      cache_read_price_per_1m: toNumber(entry.cache_read_price_per_1m),
    }))
}
const autoRefreshTimer = ref<ReturnType<typeof setInterval> | null>(null)
const autoRefreshing = ref(false)
const isPageVisible = ref(typeof document === 'undefined' ? true : !document.hidden)
const curlCopying = ref(false)
const curlCopied = ref(false)
const replayDialogOpen = ref(false)
const bodyLoading = ref(false)
const bodiesLoadedForRequestId = ref<string | null>(null)
const showTimeline = ref(false)
const AUTO_REFRESH_INTERVAL_MS = 1000
const TIMELINE_MOUNT_DELAY_MS = 120
let loadDetailRequestId = 0
let bodyLoadRequestId = 0
let loadDetailInFlight = false
let timelineMountTimer: ReturnType<typeof setTimeout> | null = null

const fullRequestId = computed(() => detail.value?.request_id || detail.value?.id || '-')
const displayRequestId = computed(() => formatShortRequestId(fullRequestId.value))
const refreshButtonTitle = computed(() => {
  if (autoRefreshing.value) return '停止自动刷新'
  return isRequestCompleted() ? '刷新' : '开启自动刷新'
})
const displayInputTokens = computed(() => {
  if (!detail.value) return 0
  return getEffectiveInputTokens({
    effective_input_tokens: detail.value.effective_input_tokens,
    input_tokens: detail.value.input_tokens ?? detail.value.tokens?.input,
    cache_read_input_tokens: detail.value.cache_read_input_tokens,
    api_format: detail.value.api_format,
    endpoint_api_format: detail.value.endpoint_api_format,
  })
})

// 监听标签页切换
watch(activeTab, (newTab) => {
  if (!['request-headers', 'response-headers'].includes(newTab) && viewMode.value === 'compare') {
    viewMode.value = 'formatted'
  }
  // 切换到不支持对话视图的 Tab 时，重置为 JSON 视图
  if (!['request-body', 'response-body'].includes(newTab)) {
    contentViewMode.value = 'json'
  }
  dataSource.value = getDefaultDataSourceForTab(newTab)

  if (['request-body', 'response-body'].includes(newTab)) {
    void ensureBodyContentLoaded()
  }
})

// 检测暗色模式
const isDark = computed(() => {
  return document.documentElement.classList.contains('dark')
})

const traceRequestMetadata = computed<Record<string, unknown> | null>(() => {
  const meta = detail.value?.metadata
  if (!meta || typeof meta !== 'object' || Array.isArray(meta)) return null
  return meta as Record<string, unknown>
})

const metadataPanelData = computed<Record<string, unknown> | null>(() => {
  if (!detail.value) return null

  const merged: Record<string, unknown> = {}
  if (hasContent(detail.value.metadata)) {
    Object.assign(merged, detail.value.metadata || {})
  }
  if (hasContent(detail.value.routing)) {
    merged.routing = detail.value.routing
  }
  if (hasContent(detail.value.body_capture)) {
    merged.body_capture = detail.value.body_capture
  }
  if (hasContent(detail.value.trace)) {
    merged.trace = detail.value.trace
  }
  if (hasContent(detail.value.settlement)) {
    merged.settlement = detail.value.settlement
  }

  return Object.keys(merged).length > 0 ? merged : null
})

const normalizedClientError = computed(() =>
  normalizeErrorDomain(detail.value?.errors?.client_error ?? detail.value?.client_error),
)

const normalizedUpstreamError = computed(() =>
  normalizeErrorDomain(detail.value?.errors?.upstream_error ?? detail.value?.upstream_error),
)

const displayClientErrorMessage = computed(() =>
  normalizedClientError.value?.message ?? '',
)

const hasVisibleErrorCards = computed(() =>
  Boolean(displayClientErrorMessage.value || normalizedUpstreamError.value),
)

const visibleErrorCardCount = computed(() =>
  (displayClientErrorMessage.value ? 1 : 0)
    + (normalizedUpstreamError.value ? 1 : 0),
)

const settlementInfo = computed<JsonRecord | null>(() =>
  asRecord(detail.value?.settlement ?? null),
)

const billingSnapshot = computed<JsonRecord | null>(() =>
  asRecord(settlementInfo.value?.billing_snapshot)
  ?? asRecord(traceRequestMetadata.value?.billing_snapshot),
)

const billingResolvedVariables = computed<JsonRecord | null>(() =>
  asRecord(billingSnapshot.value?.resolved_variables),
)

const billingCostBreakdown = computed<JsonRecord | null>(() =>
  asRecord(billingSnapshot.value?.cost_breakdown),
)

const billingTierInfo = computed<JsonRecord | null>(() =>
  asRecord(billingSnapshot.value?.tier_info),
)

function hasBodyContent(flag: boolean | undefined, data: unknown): boolean {
  return Boolean(flag) || hasContent(data)
}

const hasRequestBodyAvailable = computed(() => {
  return hasBodyContent(detail.value?.has_request_body, detail.value?.request_body)
    || hasBodyContent(detail.value?.has_provider_request_body, detail.value?.provider_request_body)
})

const hasResponseBodyAvailable = computed(() => {
  return hasBodyContent(detail.value?.has_response_body, detail.value?.response_body)
    || hasBodyContent(detail.value?.has_client_response_body, detail.value?.client_response_body)
})

const isRequestBodyLoading = computed(() => {
  return bodyLoading.value && activeTab.value === 'request-body' && !currentRequestBody.value
})

const isResponseBodyLoading = computed(() => {
  return bodyLoading.value && activeTab.value === 'response-body' && !currentResponseBody.value
})

function clearTimelineMountTimer() {
  if (timelineMountTimer) {
    clearTimeout(timelineMountTimer)
    timelineMountTimer = null
  }
}

function scheduleTimelineMount() {
  clearTimelineMountTimer()
  if (typeof window === 'undefined') {
    showTimeline.value = true
    return
  }
  timelineMountTimer = window.setTimeout(() => {
    showTimeline.value = true
  }, TIMELINE_MOUNT_DELAY_MS)
}

// 检测是否有提供商请求头
const hasProviderHeaders = computed(() => {
  return !!(detail.value?.provider_request_headers &&
         Object.keys(detail.value.provider_request_headers).length > 0)
})

// 请求体：仅当 provider_request_body 存在时才展示来源切换
const hasProviderRequestBody = computed(() => {
  return hasBodyContent(detail.value?.has_provider_request_body, detail.value?.provider_request_body)
})

// 响应体：只有客户端侧和 provider 侧都存在时才展示来源切换
const hasProviderResponseBody = computed(() => {
  return hasBodyContent(detail.value?.has_response_body, detail.value?.response_body)
    && hasBodyContent(detail.value?.has_client_response_body, detail.value?.client_response_body)
})

// 检测是否有两套响应头（客户端侧 + 提供商侧）
const hasProviderResponseHeaders = computed(() => {
  return !!(detail.value?.response_headers &&
         Object.keys(detail.value.response_headers).length > 0) &&
         !!(detail.value?.client_response_headers &&
         Object.keys(detail.value.client_response_headers).length > 0)
})

// 当前 Tab 是否支持对比模式
const canCompare = computed(() => {
  if (activeTab.value === 'request-headers') return hasProviderHeaders.value
  if (activeTab.value === 'response-headers') return hasProviderResponseHeaders.value
  return false
})

// 响应头 diff 统计（用于对比模式标题栏）
const responseHeaderStats = computed(() => {
  const counts = { added: 0, modified: 0, removed: 0, unchanged: 0 }
  if (!detail.value?.client_response_headers || !detail.value?.response_headers) return counts

  const clientHeaders = detail.value.client_response_headers as Record<string, unknown>
  const providerHeaders = detail.value.response_headers as Record<string, unknown>
  const clientKeys = new Set(Object.keys(clientHeaders))
  const providerKeys = new Set(Object.keys(providerHeaders))
  const allKeys = new Set([...clientKeys, ...providerKeys])

  for (const key of allKeys) {
    if (clientKeys.has(key) && providerKeys.has(key)) {
      counts[clientHeaders[key] === providerHeaders[key] ? 'unchanged' : 'modified']++
    } else if (clientKeys.has(key)) {
      counts.removed++
    } else {
      counts.added++
    }
  }
  return counts
})

// 是否显示客户端/提供商切换按钮
const showDataSourceToggle = computed(() => {
  if (activeTab.value === 'request-headers') return hasProviderHeaders.value
  if (activeTab.value === 'response-headers') return hasProviderResponseHeaders.value
  if (activeTab.value === 'request-body') return hasProviderRequestBody.value
  if (activeTab.value === 'response-body') return hasProviderResponseBody.value
  return false
})

// 当前高亮的数据源（对比模式下两个都不高亮）
const activeDataSource = computed(() => {
  if (['request-headers', 'response-headers'].includes(activeTab.value) && viewMode.value === 'compare') return null
  return dataSource.value
})

// 设置数据源（对比模式下切换数据源需退出对比）
function setDataSource(source: 'client' | 'provider') {
  dataSource.value = source
  if (['request-headers', 'response-headers'].includes(activeTab.value) && viewMode.value === 'compare') {
    viewMode.value = 'formatted'
  }
}

// 获取当前数据源的请求体数据
const currentRequestBody = computed(() => {
  if (!detail.value) return null
  if (dataSource.value === 'provider' && detail.value.provider_request_body) {
    return detail.value.provider_request_body
  }
  return detail.value.request_body
})

// 获取当前数据源的响应体数据
const currentResponseBody = computed(() => {
  if (!detail.value) return null
  if (dataSource.value === 'client' && detail.value.client_response_body) {
    return detail.value.client_response_body
  }
  return detail.value.response_body
})

const currentRequestBodyApiFormat = computed(() => {
  if (!detail.value) return undefined
  if (dataSource.value === 'provider') {
    return detail.value.endpoint_api_format || detail.value.api_format
  }
  return detail.value.api_format || detail.value.endpoint_api_format
})

const currentResponseBodyApiFormat = computed(() => {
  if (!detail.value) return undefined
  if (dataSource.value === 'provider') {
    return detail.value.endpoint_api_format || detail.value.api_format
  }
  return detail.value.api_format || detail.value.endpoint_api_format
})

// 获取当前数据源的请求头数据
const currentHeaderData = computed(() => {
  if (!detail.value) return null
  if (dataSource.value === 'client' && hasContent(detail.value.request_headers)) {
    return detail.value.request_headers
  }
  if (dataSource.value === 'provider' && hasContent(detail.value.provider_request_headers)) {
    return detail.value.provider_request_headers
  }
  // 回退：优先 client，再 provider
  if (hasContent(detail.value.request_headers)) {
    return detail.value.request_headers
  }
  return detail.value.provider_request_headers
})

const activeJsonPanelData = computed(() => {
  switch (activeTab.value) {
    case 'request-headers':
      return currentHeaderData.value
    case 'request-body':
      return currentRequestBody.value
    case 'response-headers':
      return currentResponseHeaderData.value
    case 'response-body':
      return currentResponseBody.value
    case 'metadata':
      return metadataPanelData.value
    default:
      return null
  }
})

const activeJsonPanelTitle = computed(() => {
  if (viewMode.value === 'compare') return '对比'
  if (supportsConversationView.value && contentViewMode.value === 'conversation') return 'Chat'
  return 'JSON'
})

// 请求体渲染结果
const requestRenderResult = computed<RenderResult>(() => {
  const body = currentRequestBody.value
  if (!body) {
    return { blocks: [], isStream: false }
  }
  if (activeTab.value !== 'request-body' || contentViewMode.value !== 'conversation') {
    return { blocks: [], isStream: false }
  }
  return renderRequest(body, currentResponseBody.value, currentRequestBodyApiFormat.value)
})

// 响应体渲染结果
const responseRenderResult = computed<RenderResult>(() => {
  const body = currentResponseBody.value
  if (!body) {
    return { blocks: [], isStream: false }
  }
  if (activeTab.value !== 'response-body' || contentViewMode.value !== 'conversation') {
    return { blocks: [], isStream: false }
  }
  return renderResponse(body, currentRequestBody.value, currentResponseBodyApiFormat.value)
})

// 当前 Tab 是否支持对话视图
const supportsConversationView = computed(() => {
  return ['request-body', 'response-body'].includes(activeTab.value)
})

// 当前对话数据是否有效（用于禁用按钮）
// 不依赖带 tab/mode 守卫的 renderResult，直接检查 body 数据是否存在
const hasValidConversation = computed(() => {
  if (activeTab.value === 'request-body') {
    return !!currentRequestBody.value
  }
  if (activeTab.value === 'response-body') {
    return !!currentResponseBody.value
  }
  return false
})

// 价格来源标签
// tiered_pricing.source 表示定价来源: 'provider' 或 'global'
const priceSourceLabel = computed(() => {
  if (!detail.value) return '历史定价'

  const source = detail.value.tiered_pricing?.source
  if (source === 'provider') {
    return '提供商定价'
  } else if (source === 'global') {
    return '全局定价'
  }

  // 没有 tiered_pricing 时，使用历史价格
  return '历史定价'
})

const cacheCreationInputTokens5m = computed(() => {
  if (!detail.value) return 0
  return toNumber(detail.value.cache_creation_input_tokens_5m)
    ?? getNestedNumber(detail.value as unknown as JsonRecord, 'cache_creation_ephemeral_5m_input_tokens')
    ?? 0
})

const cacheCreationInputTokens1h = computed(() => {
  if (!detail.value) return 0
  return toNumber(detail.value.cache_creation_input_tokens_1h)
    ?? getNestedNumber(detail.value as unknown as JsonRecord, 'cache_creation_ephemeral_1h_input_tokens')
    ?? 0
})

const totalCacheCreationTokens = computed(() => {
  if (!detail.value) return 0
  const classified = cacheCreationInputTokens5m.value + cacheCreationInputTokens1h.value
  const total = detail.value.cache_creation_input_tokens || 0
  if (total === 0 && classified > 0) return classified
  return total
})

const effectiveInputPricePer1M = computed(() =>
  getNestedNumber(billingResolvedVariables.value, 'input_price_per_1m')
  ?? toNumber(detail.value?.input_price_per_1m)
)

const effectiveOutputPricePer1M = computed(() =>
  getNestedNumber(billingResolvedVariables.value, 'output_price_per_1m')
  ?? toNumber(detail.value?.output_price_per_1m)
)

const effectiveCacheCreationPricePer1M = computed(() =>
  getNestedNumber(billingResolvedVariables.value, 'cache_creation_price_per_1m')
  ?? toNumber(detail.value?.cache_creation_price_per_1m)
)

const effectiveCacheReadPricePer1M = computed(() =>
  getNestedNumber(billingResolvedVariables.value, 'cache_read_price_per_1m')
  ?? toNumber(detail.value?.cache_read_price_per_1m)
)

const effectivePricePerRequest = computed(() =>
  getNestedNumber(billingResolvedVariables.value, 'price_per_request')
  ?? toNumber(detail.value?.price_per_request)
  ?? 0,
)

const effectiveInputCost = computed(() =>
  getNestedNumber(billingCostBreakdown.value, 'input_cost')
  ?? toNumber(detail.value?.input_cost)
  ?? 0,
)

const effectiveOutputCost = computed(() =>
  getNestedNumber(billingCostBreakdown.value, 'output_cost')
  ?? toNumber(detail.value?.output_cost)
  ?? 0,
)

const effectiveCacheCreationCost = computed(() => {
  const snapshotCost = [
    getNestedNumber(billingCostBreakdown.value, 'cache_creation_uncategorized_cost'),
    getNestedNumber(billingCostBreakdown.value, 'cache_creation_ephemeral_5m_cost'),
    getNestedNumber(billingCostBreakdown.value, 'cache_creation_ephemeral_1h_cost'),
  ].reduce((sum, value) => sum + (value ?? 0), 0)
  if (snapshotCost > 0) return snapshotCost
  return toNumber(detail.value?.cache_creation_cost) ?? 0
})

const effectiveCacheReadCost = computed(() =>
  getNestedNumber(billingCostBreakdown.value, 'cache_read_cost')
  ?? toNumber(detail.value?.cache_read_cost)
  ?? 0,
)

const effectiveRequestCost = computed(() => {
  const snapshotCost = getNestedNumber(billingCostBreakdown.value, 'request_cost')
  if (snapshotCost !== null) return snapshotCost
  if (effectivePricePerRequest.value > 0) {
    return toNumber(detail.value?.request_cost) ?? 0
  }
  return 0
})

const fallbackCacheTtlPricing = computed<CacheTTLPriceEntry[]>(() => {
  const tierPricing = normalizeCacheTtlPricing(billingTierInfo.value?.cache_ttl_pricing)
  if (tierPricing.length > 0) return tierPricing

  const rows: CacheTTLPriceEntry[] = []
  const cache5mCreationPrice = getNestedNumber(
    billingResolvedVariables.value,
    'cache_creation_ephemeral_5m_price_per_1m',
  )
  const cache1hCreationPrice = getNestedNumber(
    billingResolvedVariables.value,
    'cache_creation_ephemeral_1h_price_per_1m',
  )

  if (cache5mCreationPrice !== null) {
    rows.push({
      ttl_minutes: 5,
      cache_creation_price_per_1m: cache5mCreationPrice,
      cache_read_price_per_1m: effectiveCacheReadPricePer1M.value,
    })
  }
  if (cache1hCreationPrice !== null) {
    rows.push({
      ttl_minutes: 60,
      cache_creation_price_per_1m: cache1hCreationPrice,
      cache_read_price_per_1m: effectiveCacheReadPricePer1M.value,
    })
  }
  return rows
})

const activeCacheTtlMinutes = computed(() => {
  const snapshotTtl = getNestedNumber(billingSnapshot.value, 'resolved_dimensions', 'cache_ttl_minutes')
  if (snapshotTtl !== null && snapshotTtl > 0) {
    return Math.trunc(snapshotTtl)
  }
  if (cacheCreationInputTokens1h.value > 0 && cacheCreationInputTokens5m.value === 0) {
    return 60
  }
  if (cacheCreationInputTokens5m.value > 0 && cacheCreationInputTokens1h.value === 0) {
    return 5
  }
  return null
})

// 统一的阶梯显示数据
// 如果有 tiered_pricing，使用它；否则用历史价格构建单阶梯
const displayTiers = computed(() => {
  if (!detail.value) return []

  // 如果有阶梯定价数据，直接使用
  if (detail.value.tiered_pricing?.tiers && detail.value.tiered_pricing.tiers.length > 0) {
    return detail.value.tiered_pricing.tiers
  }

  // 否则用历史价格构建单阶梯（无上限）
  return [{
    up_to: null,
    input_price_per_1m: effectiveInputPricePer1M.value ?? 0,
    output_price_per_1m: effectiveOutputPricePer1M.value ?? 0,
    cache_creation_price_per_1m: effectiveCacheCreationPricePer1M.value,
    cache_read_price_per_1m: effectiveCacheReadPricePer1M.value,
    cache_ttl_pricing: fallbackCacheTtlPricing.value,
  }]
})

// 当前命中的阶梯索引
const currentTierIndex = computed(() => {
  if (!detail.value) return 0

  // 如果有阶梯定价，使用它的 tier_index
  if (detail.value.tiered_pricing?.tier_index !== undefined) {
    return detail.value.tiered_pricing.tier_index
  }

  const snapshotTierIndex = getNestedNumber(billingSnapshot.value, 'tier_index')
  if (snapshotTierIndex !== null) {
    return Math.max(0, Math.trunc(snapshotTierIndex))
  }

  // 单阶梯时默认是第0阶
  return 0
})

const currentTier = computed<PricingTierLike | null>(() => {
  const tier = displayTiers.value[currentTierIndex.value]
  if (!tier || typeof tier !== 'object') return null
  return tier as PricingTierLike
})

const cacheCreationSummaryText = computed(() => {
  if (!detail.value) return '0'

  const total = totalCacheCreationTokens.value
  const cache5m = cacheCreationInputTokens5m.value
  const cache1h = cacheCreationInputTokens1h.value

  if (cache5m <= 0 && cache1h <= 0) {
    return formatNumber(total)
  }

  const parts: string[] = []
  if (cache5m > 0) parts.push(`5min ${formatNumber(cache5m)}`)
  if (cache1h > 0) parts.push(`1h ${formatNumber(cache1h)}`)
  const remaining = Math.max(0, total - cache5m - cache1h)
  if (remaining > 0) parts.push(`其他 ${formatNumber(remaining)}`)
  return parts.join(' + ')
})

const cacheCreationSplitRows = computed(() => {
  if (!detail.value) return []

  const rows: Array<{
    key: string
    label: string
    tokens: number
    pricePer1M: number | null
    cost: number | null
  }> = []

  const cache5m = cacheCreationInputTokens5m.value
  const cache1h = cacheCreationInputTokens1h.value

  if (cache5m > 0) {
    const pricePer1M = getActiveCachePriceForTTL(5, 'cache_creation_price_per_1m')
    rows.push({
      key: '5m',
      label: '5min 创建',
      tokens: cache5m,
      pricePer1M,
      cost: pricePer1M !== null ? (cache5m * pricePer1M) / 1_000_000 : null,
    })
  }

  if (cache1h > 0) {
    const pricePer1M = getActiveCachePriceForTTL(60, 'cache_creation_price_per_1m')
    rows.push({
      key: '1h',
      label: '1h 创建',
      tokens: cache1h,
      pricePer1M,
      cost: pricePer1M !== null ? (cache1h * pricePer1M) / 1_000_000 : null,
    })
  }

  return rows
})

// 总输入上下文（输入 + 缓存创建 + 缓存读取）
const _totalInputContext = computed(() => {
  if (!detail.value) return 0

  // 优先使用 tiered_pricing 中的值
  if (detail.value.tiered_pricing?.total_input_context !== undefined) {
    return detail.value.tiered_pricing.total_input_context
  }

  // 否则手动计算
  const input = detail.value.tokens?.input || detail.value.input_tokens || 0
  const cacheCreation = totalCacheCreationTokens.value
  const cacheRead = detail.value.cache_read_input_tokens || 0
  return input + cacheCreation + cacheRead
})

// Token 费用总计
const tokenCostTotal = computed(() => {
  if (!detail.value) return 0
  return effectiveInputCost.value
    + effectiveOutputCost.value
    + effectiveCacheCreationCost.value
    + effectiveCacheReadCost.value
})

// 按次计费费用（非视频任务时）
const perRequestCost = computed(() => {
  if (!detail.value) return 0
  // 视频任务的 request_cost 实际上是视频费用，不算按次
  if (detail.value.video_billing) return 0
  return effectiveRequestCost.value
})

// 视频/图像/音频费用
const videoCostTotal = computed(() => {
  if (!detail.value?.video_billing) return 0
  return detail.value.video_billing.video_cost
    || detail.value.video_billing.cost
    || detail.value.request_cost
    || 0
})

// 是否有 Token 费用（用于决定是否显示 Token 计费区块）
const hasTokenCost = computed(() => {
  if (!detail.value) return false
  const inputTokens = detail.value.tokens?.input || detail.value.input_tokens || 0
  const outputTokens = detail.value.tokens?.output || detail.value.output_tokens || 0
  const cacheCreation = totalCacheCreationTokens.value
  const cacheRead = detail.value.cache_read_input_tokens || 0
  return (inputTokens + outputTokens + cacheCreation + cacheRead) > 0 || tokenCostTotal.value > 0
})

const tabs = [
  { name: 'request-headers', label: '请求头' },
  { name: 'request-body', label: '请求体' },
  { name: 'response-headers', label: '响应头' },
  { name: 'response-body', label: '响应体' },
  { name: 'metadata', label: '元数据' },
]

// 判断数据是否有实际内容（非空对象/数组）
function hasContent(data: unknown): boolean {
  if (data === null || data === undefined) return false
  if (typeof data === 'object') {
    return Object.keys(data as object).length > 0
  }
  return true
}

function toFiniteNumber(value: unknown): number | null {
  const num = Number(value)
  return Number.isFinite(num) ? num : null
}

function getTierCachePriceForTTL(
  tier: PricingTierLike | null | undefined,
  ttlMinutes: number,
  priceKey: 'cache_creation_price_per_1m' | 'cache_read_price_per_1m',
): number | null {
  const fallback = toFiniteNumber(tier?.[priceKey])
  const ttlPricing = Array.isArray(tier?.cache_ttl_pricing)
    ? tier.cache_ttl_pricing
        .filter((entry): entry is CacheTTLPriceEntry => !!entry && typeof entry === 'object')
        .sort((a, b) => Number(a.ttl_minutes || 0) - Number(b.ttl_minutes || 0))
    : []

  if (ttlPricing.length === 0) return fallback

  const matched = ttlPricing.find((entry) => Number(entry.ttl_minutes || 0) === ttlMinutes)
  const price = toFiniteNumber(matched?.[priceKey])
  return price ?? fallback
}

function getTierMatchedCachePricingEntry(
  tier: PricingTierLike | null | undefined,
  ttlMinutes: number,
): CacheTTLPriceEntry | null {
  const ttlPricing = Array.isArray(tier?.cache_ttl_pricing)
    ? tier.cache_ttl_pricing
        .filter((entry): entry is CacheTTLPriceEntry => !!entry && typeof entry === 'object')
        .sort((a, b) => Number(a.ttl_minutes || 0) - Number(b.ttl_minutes || 0))
    : []

  if (ttlPricing.length === 0) return null
  return ttlPricing.find((entry) => Number(entry.ttl_minutes || 0) === ttlMinutes) ?? null
}

function hasTierCacheCreationSplitPricing(tier: PricingTierLike | null | undefined): boolean {
  const ttlPricing = Array.isArray(tier?.cache_ttl_pricing) ? tier.cache_ttl_pricing : []
  return ttlPricing.some((entry) =>
    Number(entry?.ttl_minutes || 0) >= 60
    && toFiniteNumber(entry?.cache_creation_price_per_1m) !== null,
  )
}

function formatCacheTtlLabel(ttlMinutes: number | null | undefined): string {
  if (!ttlMinutes || ttlMinutes <= 0) return '缓存创建'
  if (ttlMinutes >= 60) return '缓存创建(1h)'
  if (ttlMinutes <= 5) return '缓存创建(5min)'
  return `缓存创建(${ttlMinutes}min)`
}

function getTierActiveCacheCreationDisplay(
  tier: PricingTierLike | null | undefined,
): { label: string, price: number } | null {
  if (hasTierCacheCreationSplitPricing(tier)) {
    const activeTtl = activeCacheTtlMinutes.value
    if (activeTtl !== null) {
      const matchedEntry = getTierMatchedCachePricingEntry(tier, activeTtl)
      const matchedPrice = getTierCachePriceForTTL(tier, activeTtl, 'cache_creation_price_per_1m')
      if (matchedEntry && matchedPrice !== null) {
        return {
          label: formatCacheTtlLabel(matchedEntry.ttl_minutes),
          price: matchedPrice,
        }
      }
      const fallbackPrice = toFiniteNumber(tier?.cache_creation_price_per_1m)
      if (fallbackPrice !== null) {
        return {
          label: formatCacheTtlLabel(activeTtl),
          price: fallbackPrice,
        }
      }
    }
  }

  const fallbackPrice = toFiniteNumber(tier?.cache_creation_price_per_1m)
  if (fallbackPrice === null) return null
  return {
    label: '缓存创建',
    price: fallbackPrice,
  }
}

function getActiveCachePriceForTTL(
  ttlMinutes: number,
  priceKey: 'cache_creation_price_per_1m' | 'cache_read_price_per_1m',
): number | null {
  const tierPrice = getTierCachePriceForTTL(currentTier.value, ttlMinutes, priceKey)
  if (tierPrice !== null) return tierPrice

  if (priceKey === 'cache_creation_price_per_1m') {
    return toFiniteNumber(detail.value?.cache_creation_price_per_1m)
  }
  return toFiniteNumber(detail.value?.cache_read_price_per_1m)
}

function getTierActiveCacheReadPrice(tier: PricingTierLike | null | undefined): number | null {
  const activeTtl = activeCacheTtlMinutes.value
  if (activeTtl !== null) {
    const matchedPrice = getTierCachePriceForTTL(tier, activeTtl, 'cache_read_price_per_1m')
    if (matchedPrice !== null) return matchedPrice
  }

  return toFiniteNumber(tier?.cache_read_price_per_1m)
    ?? effectiveCacheReadPricePer1M.value
    ?? null
}

function shouldShowCacheReadPrice(tier: PricingTierLike | null | undefined): boolean {
  return getTierActiveCacheReadPrice(tier) !== null
}

function getDefaultDataSourceForTab(tab: string): 'client' | 'provider' {
  if (!detail.value) {
    if (['request-headers', 'request-body'].includes(tab)) return 'provider'
    if (['response-headers', 'response-body'].includes(tab)) return 'client'
    return dataSource.value
  }

  if (tab === 'request-headers') {
    if (hasContent(detail.value.provider_request_headers)) return 'provider'
    if (hasContent(detail.value.request_headers)) return 'client'
    return 'provider'
  }

  if (tab === 'request-body') {
    if (hasBodyContent(detail.value.has_provider_request_body, detail.value.provider_request_body)) return 'provider'
    if (hasBodyContent(detail.value.has_request_body, detail.value.request_body)) return 'client'
    return 'provider'
  }

  if (tab === 'response-headers') {
    if (hasContent(detail.value.client_response_headers)) return 'client'
    if (hasContent(detail.value.response_headers)) return 'provider'
    return 'client'
  }

  if (tab === 'response-body') {
    if (hasBodyContent(detail.value.has_client_response_body, detail.value.client_response_body)) return 'client'
    if (hasBodyContent(detail.value.has_response_body, detail.value.response_body)) return 'provider'
    return 'client'
  }

  return dataSource.value
}

// 获取当前数据源的响应头数据
const currentResponseHeaderData = computed(() => {
  if (!detail.value) return null
  if (dataSource.value === 'client' && hasContent(detail.value.client_response_headers)) {
    return detail.value.client_response_headers
  }
  if (dataSource.value === 'provider' && hasContent(detail.value.response_headers)) {
    return detail.value.response_headers
  }
  // 回退：优先 client，再 provider
  if (hasContent(detail.value.client_response_headers)) {
    return detail.value.client_response_headers
  }
  return detail.value.response_headers
})

// 根据实际数据决定显示哪些 Tab
const visibleTabs = computed(() => {
  if (!detail.value) return []

  return tabs.filter(tab => {
    switch (tab.name) {
      case 'request-headers':
        return hasContent(detail.value?.request_headers) || hasContent(detail.value?.provider_request_headers)
      case 'request-body':
        return hasRequestBodyAvailable.value
      case 'response-headers':
        return hasContent(detail.value?.response_headers) || hasContent(detail.value?.client_response_headers)
      case 'response-body':
        return hasResponseBodyAvailable.value
      case 'metadata':
        return hasContent(metadataPanelData.value)
      default:
        return false
    }
  })
})

watch(() => props.requestId, async (newId) => {
  if (newId && props.isOpen) {
    await loadDetail(newId)
  }
})

watch(() => props.isOpen, async (isOpen) => {
  if (isOpen && props.requestId) {
    await loadDetail(props.requestId)
  } else if (!isOpen) {
    stopAutoRefresh()
    showTimeline.value = false
    clearTimelineMountTimer()
    bodyLoading.value = false
    bodiesLoadedForRequestId.value = null
  }
})

async function ensureBodyContentLoaded() {
  if (!props.requestId || !detail.value) return

  const cacheKey = detail.value.request_id || detail.value.id
  if (bodiesLoadedForRequestId.value === cacheKey || bodyLoading.value) return
  if (!hasRequestBodyAvailable.value && !hasResponseBodyAvailable.value) return

  const requestId = ++bodyLoadRequestId
  bodyLoading.value = true
  try {
    const response = await dashboardApi.getRequestDetail(props.requestId, { includeBodies: true })
    if (requestId !== bodyLoadRequestId || !detail.value) return
    detail.value = {
      ...detail.value,
      request_body: response.request_body,
      provider_request_body: response.provider_request_body,
      response_body: response.response_body,
      client_response_body: response.client_response_body,
      has_request_body: response.has_request_body,
      has_provider_request_body: response.has_provider_request_body,
      has_response_body: response.has_response_body,
      has_client_response_body: response.has_client_response_body,
      request_error: response.request_error,
      upstream_error: response.upstream_error,
      client_error: response.client_error,
      failure_summary: response.failure_summary,
      errors: response.errors,
      error_flow: response.error_flow,
    }
    bodiesLoadedForRequestId.value = cacheKey
  } catch (err) {
    if (requestId !== bodyLoadRequestId) return
    log.error('Failed to load request bodies:', err)
  } finally {
    if (requestId === bodyLoadRequestId) {
      bodyLoading.value = false
    }
  }
}

async function loadDetail(id: string, silent = false) {
  if (silent && loadDetailInFlight) {
    return
  }
  const requestId = ++loadDetailRequestId
  loadDetailInFlight = true
  if (!silent) {
    loading.value = true
    historicalPricing.value = null
    timelineLoaded.value = false
    timelineHasTrace.value = false
    showTimeline.value = false
    clearTimelineMountTimer()
    ++bodyLoadRequestId
    bodyLoading.value = false
  }
  error.value = null
  try {
    const response = await dashboardApi.getRequestDetail(id, {
      includeBodies: false,
      cacheTtlMs: silent ? 0 : 5_000
    })
    if (requestId !== loadDetailRequestId) return

    const previousDetail = detail.value
    const prevKey = previousDetail?.request_id || previousDetail?.id
    const currKey = response.request_id || response.id
    const sameRequest = !!prevKey && prevKey === currKey
    detail.value = {
      ...response,
      request_body: sameRequest ? previousDetail?.request_body : undefined,
      provider_request_body: sameRequest ? previousDetail?.provider_request_body : undefined,
      response_body: sameRequest ? previousDetail?.response_body : undefined,
      client_response_body: sameRequest ? previousDetail?.client_response_body : undefined,
      request_error: sameRequest ? (previousDetail?.request_error ?? response.request_error) : response.request_error,
      upstream_error: sameRequest ? (previousDetail?.upstream_error ?? response.upstream_error) : response.upstream_error,
      client_error: sameRequest ? (previousDetail?.client_error ?? response.client_error) : response.client_error,
      failure_summary: sameRequest ? (previousDetail?.failure_summary ?? response.failure_summary) : response.failure_summary,
      errors: sameRequest ? (previousDetail?.errors ?? response.errors) : response.errors,
      error_flow: sameRequest ? (previousDetail?.error_flow ?? response.error_flow) : response.error_flow,
    }
    bodiesLoadedForRequestId.value = sameRequest ? bodiesLoadedForRequestId.value : null

    // 首次加载时优先停留在轻量 tab，避免默认触发大 body 加载
    if (!silent) {
      const visibleTabNames = visibleTabs.value.map(t => t.name)
      if (visibleTabNames.includes('request-headers')) {
        activeTab.value = 'request-headers'
      } else if (visibleTabNames.includes('response-headers')) {
        activeTab.value = 'response-headers'
      } else if (visibleTabNames.includes('metadata')) {
        activeTab.value = 'metadata'
      } else if (visibleTabNames.includes('request-body')) {
        activeTab.value = 'request-body'
      } else if (visibleTabNames.includes('response-body')) {
        activeTab.value = 'response-body'
      } else if (visibleTabNames.length > 0) {
        activeTab.value = visibleTabNames[0]
      }
      scheduleTimelineMount()
    }

    // 根据当前 Tab 的数据可用性自动选择默认数据源
    dataSource.value = getDefaultDataSourceForTab(activeTab.value)

    // 使用请求记录中保存的历史价格
    if (detail.value.input_price_per_1m || detail.value.output_price_per_1m || detail.value.price_per_request) {
      historicalPricing.value = {
        input_price: detail.value.input_price_per_1m ? detail.value.input_price_per_1m.toFixed(4) : 'N/A',
        output_price: detail.value.output_price_per_1m ? detail.value.output_price_per_1m.toFixed(4) : 'N/A',
        cache_creation_price: detail.value.cache_creation_price_per_1m ? detail.value.cache_creation_price_per_1m.toFixed(4) : 'N/A',
        cache_read_price: detail.value.cache_read_price_per_1m ? detail.value.cache_read_price_per_1m.toFixed(4) : 'N/A',
        request_price: detail.value.price_per_request ? detail.value.price_per_request.toFixed(4) : 'N/A'
      }
    }

    // 静默刷新时同步刷新链路追踪
    if (silent) {
      timelineRef.value?.refresh()
    }

    // 已完成请求需要停止自动刷新；进行中的请求只在用户手动开启后才保持刷新
    if (props.isOpen && isRequestCompleted()) {
      stopAutoRefresh()
    }
  } catch (err) {
    if (requestId !== loadDetailRequestId) return
    log.error('Failed to load request detail:', err)
    if (!silent) {
      error.value = '加载请求详情失败'
    }
  } finally {
    if (!silent && requestId === loadDetailRequestId) {
      loading.value = false
    }
    if (requestId === loadDetailRequestId) {
      loadDetailInFlight = false
    }
  }
}

function handleClose() {
  stopAutoRefresh()
  emit('close')
}

function isRequestCompleted(): boolean {
  if (!detail.value?.status) return true
  const displayStatus = resolveDisplayRequestStatus(detail.value)
  return displayStatus !== 'pending' && displayStatus !== 'streaming'
}

function stopAutoRefresh() {
  if (autoRefreshTimer.value) {
    clearInterval(autoRefreshTimer.value)
    autoRefreshTimer.value = null
  }
  autoRefreshing.value = false
}

function startAutoRefresh() {
  if (autoRefreshTimer.value) {
    autoRefreshing.value = true
    return
  }
  if (!isPageVisible.value || !props.requestId || !props.isOpen) {
    autoRefreshing.value = false
    return
  }
  autoRefreshing.value = true
  autoRefreshTimer.value = setInterval(async () => {
    if (!isPageVisible.value || !props.requestId || !props.isOpen) {
      stopAutoRefresh()
      return
    }
    await loadDetail(props.requestId, true)
    if (isRequestCompleted()) {
      stopAutoRefresh()
    }
  }, AUTO_REFRESH_INTERVAL_MS)
}

async function refreshDetail() {
  if (!props.requestId) return

  // 已完成：单次静默刷新
  if (isRequestCompleted()) {
    await loadDetail(props.requestId, true)
    return
  }

  // 未完成：如果已在自动刷新则停止，否则启动
  if (autoRefreshing.value) {
    stopAutoRefresh()
    return
  }

  autoRefreshing.value = true
  await loadDetail(props.requestId, true)

  // 加载后可能已经完成了
  if (isRequestCompleted()) {
    stopAutoRefresh()
    return
  }

  startAutoRefresh()
}

function handleVisibilityChange() {
  isPageVisible.value = !document.hidden
  if (!isPageVisible.value) {
    stopAutoRefresh()
  }
}

onMounted(() => {
  document.addEventListener('visibilitychange', handleVisibilityChange)
})

onBeforeUnmount(() => {
  document.removeEventListener('visibilitychange', handleVisibilityChange)
  stopAutoRefresh()
  clearTimelineMountTimer()
  loadDetailRequestId += 1
  loadDetailInFlight = false
})

function formatDateTime(dateStr: string | null | undefined): string {
  if (!dateStr) return 'N/A'
  const date = new Date(dateStr)
  return date.toLocaleString('zh-CN', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit'
  })
}

// 格式化视频/音频时长
function formatDuration(seconds: number): string {
  if (seconds < 60) {
    return `${seconds.toFixed(1)}s`
  }
  const mins = Math.floor(seconds / 60)
  const secs = seconds % 60
  if (mins < 60) {
    return `${mins}m ${secs.toFixed(0)}s`
  }
  const hours = Math.floor(mins / 60)
  const remainMins = mins % 60
  return `${hours}h ${remainMins}m`
}

// 获取任务类型标签
function getTaskTypeLabel(taskType: string): string {
  switch (taskType) {
    case 'video':
      return '视频生成'
    case 'image':
      return '图像生成'
    case 'audio':
      return '音频生成'
    default:
      return taskType
  }
}

function formatNumber(num: number): string {
  if (num >= 1_000_000) {
    return `${(num / 1_000_000).toFixed(1)  }M`
  } else if (num >= 1_000) {
    return `${(num / 1_000).toFixed(1)  }K`
  }
  return num.toLocaleString()
}

// 格式化响应时间，自动选择合适的单位
function formatResponseTime(ms: number): { value: string; unit: string } {
  if (ms >= 1_000) {
    return { value: (ms / 1_000).toFixed(2), unit: 's' }
  }
  return { value: ms.toString(), unit: 'ms' }
}

// 格式化价格，修复浮点数精度问题
function formatPrice(price: number): string {
  // 处理浮点数精度问题，最多保留4位小数，去掉尾部的0
  const fixed = price.toFixed(4)
  return parseFloat(fixed).toString()
}

// 获取阶梯范围文本
function getTierRangeText(tier: { up_to?: number | null }, index: number, tiers: Array<{ up_to?: number | null }>): string {
  const prevTier = index > 0 ? tiers[index - 1] : null
  const start = prevTier?.up_to ? prevTier.up_to + 1 : 0

  if (tier.up_to) {
    if (start === 0) {
      return `0 ~ ${formatNumber(tier.up_to)} tokens`
    }
    return `${formatNumber(start)} ~ ${formatNumber(tier.up_to)} tokens`
  }
  // 无上限的情况
  return `> ${formatNumber(start)} tokens`
}

/** 将 RenderResult 格式化为可复制的文本 */
function formatRenderResultAsText(result: RenderResult): string {
  if (result.error) {
    return `[Error] ${result.error}`
  }

  const parts: string[] = []

  for (const block of result.blocks) {
    const text = formatBlockAsText(block)
    if (text) {
      parts.push(text)
    }
  }

  return parts.join('\n\n---\n\n')
}

/** 将单个 RenderBlock 格式化为文本 */
function formatBlockAsText(block: RenderBlock): string {
  switch (block.type) {
    case 'text':
      return block.content
    case 'code':
      return block.language
        ? `\`\`\`${block.language}\n${block.code}\n\`\`\``
        : `\`\`\`\n${block.code}\n\`\`\``
    case 'collapsible':
      return `[${block.title}]\n${block.content.map(formatBlockAsText).filter(Boolean).join('\n')}`
    case 'error':
      return `[Error${block.code ? `: ${block.code}` : ''}] ${block.message}`
    case 'image':
      return `[Image: ${block.mimeType || block.alt || 'unknown'}]`
    case 'tool_use':
      return `[Tool: ${block.toolName}]\n${block.input}`
    case 'tool_result':
      return `[Tool Result${block.isError ? ' (Error)' : ''}]\n${block.content}`
    case 'message': {
      const roleLabel = block.roleLabel || block.role
      const contentText = block.content.map(formatBlockAsText).filter(Boolean).join('\n\n')
      return `[${roleLabel}]\n${contentText}`
    }
    case 'container':
      return block.children.map(formatBlockAsText).filter(Boolean).join('\n')
    case 'label':
      return `${block.label}: ${block.value}`
    case 'divider':
      return '---'
    case 'badge':
      return ''
    default:
      return ''
  }
}

// 复制内容（支持 JSON 和对话两种模式）
function copyContent(tabName: string) {
  if (!detail.value) return
  if (viewMode.value === 'compare') return

  let textToCopy = ''

  // 对话视图模式：复制格式化的对话文本
  if (contentViewMode.value === 'conversation') {
    if (tabName === 'request-body') {
      textToCopy = formatRenderResultAsText(requestRenderResult.value)
    } else if (tabName === 'response-body') {
      textToCopy = formatRenderResultAsText(responseRenderResult.value)
    }
  } else {
    // JSON 视图模式：复制原始 JSON
    let data: unknown = null
    switch (tabName) {
      case 'request-headers':
        data = dataSource.value === 'provider'
          ? detail.value.provider_request_headers
          : detail.value.request_headers
        break
      case 'request-body':
        data = currentRequestBody.value
        break
      case 'response-headers':
        data = currentResponseHeaderData.value
        break
      case 'response-body':
        data = currentResponseBody.value
        break
      case 'metadata':
        data = metadataPanelData.value
        break
    }
    if (data) {
      textToCopy = JSON.stringify(data, null, 2)
    }
  }

  if (textToCopy) {
    copyToClipboard(textToCopy, false)
    copiedStates.value[tabName] = true
    setTimeout(() => {
      copiedStates.value[tabName] = false
    }, 2000)
  }
}

// 切换内容视图模式
function toggleContentView() {
  if (contentViewMode.value === 'json') {
    if (hasValidConversation.value) {
      contentViewMode.value = 'conversation'
    }
  } else {
    contentViewMode.value = 'json'
  }
}

// 复制 cURL 命令
async function copyCurlCommand() {
  if (!props.requestId || curlCopying.value) return
  curlCopying.value = true
  try {
    const data = await dashboardApi.getCurlData(props.requestId)
    if (data.curl) {
      copyToClipboard(data.curl, false)
      curlCopied.value = true
      setTimeout(() => { curlCopied.value = false }, 2000)
    }
  } catch (err) {
    log.error('Failed to generate cURL command:', err)
  } finally {
    curlCopying.value = false
  }
}

// 打开请求回放对话框
function openReplayDialog() {
  replayDialogOpen.value = true
}

// 请求头合并对比逻辑
interface HeaderEntry {
  key: string
  status: 'added' | 'modified' | 'removed' | 'unchanged'
  originalValue?: unknown
  newValue?: unknown
}

const mergedHeaderEntries = computed(() => {
  if (!detail.value?.request_headers && !detail.value?.provider_request_headers) {
    return []
  }

  const clientHeaders = detail.value?.request_headers || {}
  const providerHeaders = detail.value?.provider_request_headers || {}

  const clientKeys = new Set(Object.keys(clientHeaders))
  const providerKeys = new Set(Object.keys(providerHeaders))
  const allKeys = new Set([...clientKeys, ...providerKeys])

  const entries: HeaderEntry[] = []

  for (const key of Array.from(allKeys).sort()) {
    const entry: HeaderEntry = { key, status: 'unchanged' }

    if (clientKeys.has(key) && providerKeys.has(key)) {
      if (clientHeaders[key] !== providerHeaders[key]) {
        entry.status = 'modified'
        entry.originalValue = clientHeaders[key]
        entry.newValue = providerHeaders[key]
      } else {
        entry.status = 'unchanged'
        entry.originalValue = clientHeaders[key]
      }
    } else if (clientKeys.has(key)) {
      entry.status = 'removed'
      entry.originalValue = clientHeaders[key]
    } else {
      entry.status = 'added'
      entry.newValue = providerHeaders[key]
    }

    entries.push(entry)
  }

  return entries
})

const headerStats = computed(() => {
  const counts = {
    added: 0,
    modified: 0,
    removed: 0,
    unchanged: 0
  }

  for (const entry of mergedHeaderEntries.value) {
    counts[entry.status]++
  }

  return counts
})

// 添加 ESC 键监听
useEscapeKey(() => {
  if (props.isOpen) {
    handleClose()
  }
}, {
  disableOnInput: true,
  once: false
})
</script>

<style scoped>
/* 抽屉过渡动画 */
.drawer-enter-active,
.drawer-leave-active {
  transition: opacity 0.3s ease;
}

.drawer-enter-active .relative,
.drawer-leave-active .relative {
  transition: transform 0.3s ease;
}

.drawer-enter-from,
.drawer-leave-to {
  opacity: 0;
}

.drawer-enter-from .relative {
  transform: translateX(100%);
}

.drawer-leave-to .relative {
  transform: translateX(100%);
}

.drawer-enter-to .relative,
.drawer-leave-from .relative {
  transform: translateX(0);
}

/* 内容区融合：子组件的 Card 不再需要自己的边框和圆角，与表头栏融为一体 */
.content-block :deep(.rounded-2xl) {
  border: none !important;
  border-radius: 0 !important;
  box-shadow: none !important;
}
</style>

<style>
/* 滚动条始终预留空间，保持宽度稳定 */
.scrollbar-stable {
  scrollbar-gutter: stable;
}

/* Webkit 浏览器滚动条样式 */
.scrollbar-stable::-webkit-scrollbar {
  width: 8px;
}

.scrollbar-stable::-webkit-scrollbar-track {
  background: transparent;
}

.scrollbar-stable::-webkit-scrollbar-thumb {
  background-color: rgba(128, 128, 128, 0.5);
  border-radius: 4px;
}

.scrollbar-stable::-webkit-scrollbar-thumb:hover {
  background-color: rgba(128, 128, 128, 0.7);
}
</style>
