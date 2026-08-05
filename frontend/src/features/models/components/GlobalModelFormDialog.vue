<template>
  <Dialog
    :model-value="open"
    :title="isEditMode ? '编辑模型' : '创建统一模型'"
    :description="isEditMode ? '修改模型配置和价格信息' : ''"
    :icon="isEditMode ? SquarePen : Layers"
    :size="isEditMode ? '4xl' : '3xl'"
    @update:model-value="handleDialogUpdate"
  >
    <div
      class="flex gap-4"
      :class="isEditMode ? '' : 'h-[600px] flex-col'"
    >
      <!-- 上方：搜索和加载预设（仅创建模式） -->
      <section
        v-if="!isEditMode && !presetPanelCollapsed"
        class="h-full flex flex-col space-y-3"
      >
        <!-- 搜索框 -->
        <div class="relative">
          <Search class="absolute left-2.5 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground" />
          <Input
            v-model="searchQuery"
            type="text"
            placeholder="搜索模型、提供商..."
            class="pl-8 h-8 text-sm"
          />
        </div>

        <!-- 横向提供商 Logo 与模型列表 -->
        <div class="flex-1 min-h-0 overflow-hidden border rounded-lg flex flex-col">
          <div
            v-if="loading"
            class="flex items-center justify-center flex-1"
          >
            <Loader2 class="w-5 h-5 animate-spin text-muted-foreground" />
          </div>
          <template v-else>
            <!-- 提供商 Logo 横向选择 -->
            <div
              v-if="groupedModels.length > 0"
              class="relative shrink-0 border-b"
            >
              <Button
                type="button"
                variant="outline"
                size="icon"
                class="absolute left-1 top-1/2 z-10 h-8 w-8 -translate-y-1/2 bg-background/95 shadow-sm"
                title="向左滚动"
                aria-label="向左滚动提供商"
                @click="scrollProviderLogos(-1)"
              >
                <ChevronLeft class="h-4 w-4" />
              </Button>
              <div
                ref="providerLogoScroller"
                class="mx-11 flex gap-2 overflow-x-auto p-2 scrollbar-hide"
              >
                <button
                  v-for="group in groupedModels"
                  :key="group.providerId"
                  type="button"
                  class="w-[76px] shrink-0 rounded-md border px-2 py-1.5 flex flex-col items-center gap-1 transition-colors"
                  :class="expandedProvider === group.providerId
                    ? 'border-primary bg-primary/10 text-primary'
                    : 'border-transparent hover:border-border hover:bg-muted'"
                  :title="`${group.providerName}（${group.models.length}）`"
                  @click="toggleProvider(group.providerId)"
                >
                  <img
                    :src="getProviderLogoUrl(group.providerId)"
                    :alt="group.providerName"
                    class="w-7 h-7 rounded object-contain dark:invert dark:brightness-90"
                    @error="handleLogoError"
                  >
                  <span class="w-full truncate text-[10px] font-medium text-center">{{ group.providerName }}</span>
                </button>
              </div>
              <Button
                type="button"
                variant="outline"
                size="icon"
                class="absolute right-1 top-1/2 z-10 h-8 w-8 -translate-y-1/2 bg-background/95 shadow-sm"
                title="向右滚动"
                aria-label="向右滚动提供商"
                @click="scrollProviderLogos(1)"
              >
                <ChevronRight class="h-4 w-4" />
              </Button>
            </div>

            <!-- 当前提供商模型 -->
            <div
              v-if="expandedProviderGroup"
              class="flex-1 min-h-0 overflow-y-auto p-2 scrollbar-thin"
            >
              <div class="grid grid-cols-1 gap-3 sm:grid-cols-2">
                <div
                  v-for="item in expandedProviderGroup.models"
                  :key="item.modelId"
                  class="relative min-w-0"
                >
                  <button
                    type="button"
                    class="group relative flex h-full min-h-[152px] w-full min-w-0 flex-col rounded-lg border bg-card p-4 text-left shadow-sm transition-[border-color,box-shadow,transform,background-color] duration-200 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/40"
                    :class="getExistingModel(item)
                      ? 'cursor-default border-border/70'
                      : selectedModel?.modelId === item.modelId && selectedModel?.providerId === item.providerId
                        ? 'cursor-pointer border-primary bg-primary/5 ring-1 ring-primary hover:-translate-y-0.5 hover:border-primary/35 hover:shadow-md active:scale-[0.96]'
                        : 'cursor-pointer border-border/70 hover:-translate-y-0.5 hover:border-primary/35 hover:shadow-md active:scale-[0.96]'"
                    :disabled="!!getExistingModel(item)"
                    @click="selectModel(item)"
                  >
                    <span
                      v-if="selectedModel?.modelId === item.modelId && selectedModel?.providerId === item.providerId"
                      class="absolute right-2.5 top-2.5 flex h-5 w-5 items-center justify-center rounded-full bg-primary text-primary-foreground shadow-sm"
                    >
                      <Check class="h-3 w-3" />
                    </span>

                    <span
                      class="flex w-full items-start gap-2"
                      :class="getExistingModel(item)
                        ? 'pr-24'
                        : selectedModel?.modelId === item.modelId && selectedModel?.providerId === item.providerId
                          ? 'pr-7'
                          : ''"
                    >
                      <span class="min-w-0 flex-1">
                        <span class="block truncate text-sm font-semibold leading-5">{{ item.modelName }}</span>
                        <span class="block truncate font-mono text-[10px] text-muted-foreground">{{ item.modelId }}</span>
                      </span>
                    </span>

                    <span class="mt-2 flex min-h-5 flex-wrap gap-1">
                      <span
                        v-if="item.supportsReasoning"
                        class="inline-flex items-center gap-1 rounded-md border border-violet-500/20 bg-violet-500/10 px-1.5 py-0.5 text-[9px] font-medium text-violet-700 dark:text-violet-300"
                      >
                        <BrainCircuit class="h-2.5 w-2.5" />推理
                      </span>
                      <span
                        v-if="item.supportsVision"
                        class="inline-flex items-center gap-1 rounded-md border border-sky-500/20 bg-sky-500/10 px-1.5 py-0.5 text-[9px] font-medium text-sky-700 dark:text-sky-300"
                      >
                        <Eye class="h-2.5 w-2.5" />视觉
                      </span>
                      <span
                        v-if="item.supportsToolCall"
                        class="inline-flex items-center gap-1 rounded-md border border-amber-500/20 bg-amber-500/10 px-1.5 py-0.5 text-[9px] font-medium text-amber-700 dark:text-amber-300"
                      >
                        <Wrench class="h-2.5 w-2.5" />工具
                      </span>
                      <span
                        v-if="item.supportsStructuredOutput"
                        class="inline-flex items-center gap-1 rounded-md border border-emerald-500/20 bg-emerald-500/10 px-1.5 py-0.5 text-[9px] font-medium text-emerald-700 dark:text-emerald-300"
                      >
                        <Braces class="h-2.5 w-2.5" />结构化
                      </span>
                      <span
                        v-if="item.supportsEmbedding"
                        class="inline-flex items-center gap-1 rounded-md border border-fuchsia-500/20 bg-fuchsia-500/10 px-1.5 py-0.5 text-[9px] font-medium text-fuchsia-700 dark:text-fuchsia-300"
                      >
                        <Database class="h-2.5 w-2.5" />Embedding
                      </span>
                      <span
                        v-if="item.openWeights"
                        class="inline-flex items-center gap-1 rounded-md border border-border bg-muted/70 px-1.5 py-0.5 text-[9px] font-medium text-muted-foreground"
                      >
                        <PackageOpen class="h-2.5 w-2.5" />开放权重
                      </span>
                    </span>

                    <span class="mt-auto flex w-full items-end justify-between gap-2 border-t border-border/60 pt-2 text-[9px] text-muted-foreground">
                      <span class="flex min-w-0 flex-col">
                        <span v-if="item.contextLimit">上下文 {{ formatTokenLimit(item.contextLimit) }}</span>
                        <span v-else>上下文未知</span>
                        <span v-if="item.outputLimit">输出 {{ formatTokenLimit(item.outputLimit) }}</span>
                      </span>
                      <span
                        v-if="item.inputPrice !== undefined || item.outputPrice !== undefined"
                        class="shrink-0 text-right font-medium tabular-nums text-foreground/70"
                      >
                        <span class="block">输入 ${{ formatModelPrice(item.inputPrice) }}/M</span>
                        <span class="block">输出 ${{ formatModelPrice(item.outputPrice) }}/M</span>
                      </span>
                      <span
                        v-else-if="item.releaseDate"
                        class="shrink-0"
                      >{{ item.releaseDate }}</span>
                    </span>
                  </button>
                  <div
                    v-if="getExistingModel(item)"
                    class="absolute right-2.5 top-2.5 z-10 flex items-center gap-1.5"
                  >
                    <span class="inline-flex h-6 items-center rounded-md bg-muted px-2 text-[10px] font-medium text-muted-foreground">已添加</span>
                    <Button
                      type="button"
                      variant="outline"
                      size="icon"
                      class="h-7 w-7 bg-background/90 text-muted-foreground shadow-sm hover:text-foreground"
                      title="去编辑"
                      :aria-label="`编辑 ${item.modelName}`"
                      :data-testid="`edit-existing-model-${item.modelId}`"
                      @click="editExistingModel(item)"
                    >
                      <SquarePen class="h-3.5 w-3.5" />
                    </Button>
                  </div>
                </div>
              </div>
            </div>
            <div
              v-else-if="groupedModels.length > 0"
              class="flex flex-1 items-center justify-center text-xs text-muted-foreground"
            >
              点击提供商 Logo 展开模型
            </div>
            <div
              v-else
              class="flex flex-1 items-center justify-center text-sm text-muted-foreground"
            >
              {{ searchQuery ? '未找到模型' : '暂无可用模型' }}
            </div>
          </template>
        </div>
      </section>

      <!-- 第二步：详细信息表单 -->
      <div
        v-if="isEditMode || presetPanelCollapsed"
        class="flex-1 min-h-0 overflow-y-auto scrollbar-thin"
        :class="isEditMode ? 'max-h-[70vh]' : ''"
      >
        <div
          v-if="!isEditMode"
          class="mb-4 flex items-center gap-3 rounded-lg border bg-muted/20 px-4 py-3"
        >
          <div
            v-if="selectedModel"
            class="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg border bg-background"
          >
            <img
              :src="getProviderLogoUrl(selectedModel.providerId)"
              :alt="selectedModel.providerName"
              class="h-7 w-7 rounded object-contain dark:invert dark:brightness-90"
              @error="handleLogoError"
            >
          </div>
          <div
            v-else
            class="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg border bg-background"
          >
            <SquarePen class="h-5 w-5 text-muted-foreground" />
          </div>
          <div class="min-w-0 flex-1">
            <div class="text-xs text-muted-foreground">
              {{ selectedModel ? `已加载 ${selectedModel.providerName} 预设` : '手动填写模式' }}
            </div>
            <div class="truncate text-sm font-medium">
              {{ selectedModel ? selectedModel.modelName : '填写模型详细信息' }}
            </div>
            <div
              v-if="selectedModel"
              class="truncate text-xs text-muted-foreground"
            >
              {{ selectedModel.modelId }}
            </div>
          </div>
          <div class="flex shrink-0 items-center gap-2">
            <Button
              type="button"
              variant="outline"
              size="sm"
              class="shrink-0"
              @click="reopenPresetPanel"
            >
              返回选择模型
            </Button>
          </div>
        </div>
        <form
          class="space-y-5"
          @submit.prevent="handleSubmit"
        >
          <!-- 基本信息 -->
          <section
            ref="basicInfoSection"
            class="space-y-3 rounded-lg border bg-card p-4"
          >
            <h4 class="font-medium text-sm">
              基本信息
            </h4>
            <div class="grid grid-cols-2 gap-3">
              <div class="space-y-1.5">
                <Label
                  for="model-display-name"
                  class="text-xs"
                >名称 *</Label>
                <Input
                  id="model-display-name"
                  v-model="form.display_name"
                  placeholder="Claude 3.5 Sonnet"
                  required
                />
              </div>
              <div class="space-y-1.5">
                <Label
                  for="model-name"
                  class="text-xs"
                >模型ID *</Label>
                <Input
                  id="model-name"
                  v-model="form.name"
                  placeholder="claude-3-5-sonnet-20241022"
                  :disabled="isEditMode"
                  required
                />
              </div>
            </div>
            <div class="space-y-1.5">
              <Label
                for="model-description"
                class="text-xs"
              >描述</Label>
              <Input
                id="model-description"
                :model-value="getConfigInputValue('description')"
                placeholder="简短描述此模型的特点"
                @update:model-value="(v) => setConfigField('description', v || undefined)"
              />
            </div>
            <div class="grid grid-cols-2 gap-3">
              <div class="space-y-1.5">
                <Label
                  for="model-output-limit"
                  class="text-xs"
                >最大输出 Token</Label>
                <Input
                  id="model-output-limit"
                  :model-value="getConfigInputValue('output_limit')"
                  type="number"
                  min="1"
                  placeholder="如 8192"
                  @update:model-value="(v) => setConfigField('output_limit', parseNumberInput(v, { allowFloat: false }))"
                />
              </div>
              <div class="space-y-1.5">
                <Label
                  for="model-context-limit"
                  class="text-xs"
                >上下文窗口</Label>
                <Input
                  id="model-context-limit"
                  :model-value="getConfigInputValue('context_limit')"
                  type="number"
                  min="1"
                  placeholder="如 200000"
                  @update:model-value="(v) => setConfigField('context_limit', parseNumberInput(v, { allowFloat: false }))"
                />
              </div>
            </div>
            <div class="rounded-lg border border-border/60 bg-muted/20 p-3 space-y-2">
              <div class="flex items-start gap-2">
                <Checkbox
                  :model-value="isEmbeddingEnabled"
                  class="mt-0.5"
                  @update:model-value="setEmbeddingEnabled"
                />
                <div class="space-y-1">
                  <div class="text-sm font-medium">
                    Embedding
                  </div>
                  <p class="text-xs text-muted-foreground">
                    标记为 Embeddings 模型，并使用独立的 embedding API 格式，不按 Chat 模型处理。
                  </p>
                  <div
                    v-if="isEmbeddingEnabled"
                    class="flex flex-wrap gap-1.5"
                  >
                    <span
                      v-for="format in embeddingApiFormats"
                      :key="format"
                      class="rounded-md border border-border/60 bg-background px-2 py-0.5 text-[11px] font-mono text-muted-foreground"
                    >{{ format }}</span>
                  </div>
                </div>
              </div>
            </div>
          </section>

          <!-- 价格配置 -->
          <section
            class="space-y-3 rounded-lg border bg-card p-4"
          >
            <div class="flex items-center justify-between gap-2">
              <h4 class="font-medium text-sm">
                选择计费模式
              </h4>
              <Popover
                v-if="isEditMode"
                :open="onlinePricingSourcePopoverOpen"
                :modal="false"
                @update:open="handleOnlinePricingSourcePopoverUpdate"
              >
                <PopoverTrigger as-child>
                  <Button
                    type="button"
                    variant="outline"
                    size="icon"
                    class="h-8 w-8 shrink-0"
                    :disabled="syncingOnlinePricing || submitting"
                    :title="syncingOnlinePricing ? '正在同步在线价格' : '同步最新在线价格'"
                    aria-label="同步最新在线价格"
                    data-testid="sync-online-pricing"
                    @click="syncOnlinePricing"
                  >
                    <RefreshCw
                      class="h-4 w-4"
                      :class="syncingOnlinePricing ? 'animate-spin' : ''"
                    />
                  </Button>
                </PopoverTrigger>
                <PopoverContent
                  class="z-[130] w-80 max-w-[calc(100vw-2rem)] p-3"
                  side="bottom"
                  align="end"
                >
                  <div class="space-y-3">
                    <div class="flex items-center justify-between gap-3">
                      <div class="min-w-0">
                        <div class="truncate text-xs font-medium">
                          选择在线价格来源
                        </div>
                        <div class="truncate text-[10px] text-muted-foreground">
                          {{ props.model?.display_name || props.model?.name || '当前模型' }}
                        </div>
                      </div>
                      <Loader2
                        v-if="syncingOnlinePricing"
                        class="h-3.5 w-3.5 shrink-0 animate-spin text-muted-foreground"
                      />
                    </div>

                    <div
                      v-if="syncingOnlinePricing && onlinePricingCandidates.length === 0"
                      class="flex items-center justify-center py-5 text-xs text-muted-foreground"
                    >
                      正在刷新在线价格...
                    </div>
                    <div
                      v-else
                      class="max-h-64 space-y-1.5 overflow-y-auto pr-0.5"
                      role="radiogroup"
                      aria-label="在线价格来源"
                      @keydown="handleOnlinePricingSourcePopoverKeydown"
                    >
                      <button
                        v-for="candidate in onlinePricingCandidates"
                        :key="candidate.providerId"
                        type="button"
                        role="radio"
                        class="flex w-full items-center gap-2 rounded-md border px-2.5 py-2 text-left transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/40 disabled:cursor-not-allowed disabled:opacity-55"
                        :class="selectedOnlinePricingProviderId === candidate.providerId
                          ? 'border-primary bg-primary/5 ring-1 ring-primary'
                          : 'border-border/70 hover:border-primary/35 hover:bg-muted/40'"
                        :aria-checked="selectedOnlinePricingProviderId === candidate.providerId"
                        :disabled="syncingOnlinePricing || !isOnlinePricingCandidateSyncable(candidate)"
                        :tabindex="selectedOnlinePricingProviderId === candidate.providerId
                          || (!selectedOnlinePricingProviderId
                            && firstSyncableOnlinePricingProviderId === candidate.providerId)
                          ? 0
                          : -1"
                        data-online-pricing-source-control
                        data-online-pricing-source-option
                        :data-provider-id="candidate.providerId"
                        :data-testid="`online-pricing-source-${candidate.providerId}`"
                        @click="selectedOnlinePricingProviderId = candidate.providerId"
                      >
                        <span class="flex h-7 w-7 shrink-0 items-center justify-center rounded border bg-background">
                          <img
                            :src="getProviderLogoUrl(candidate.providerId)"
                            :alt="candidate.providerName"
                            class="h-5 w-5 rounded object-contain dark:invert dark:brightness-90"
                            @error="handleLogoError"
                          >
                        </span>
                        <span class="min-w-0 flex-1">
                          <span class="block truncate text-xs font-medium">{{ candidate.providerName }}</span>
                          <span class="block truncate font-mono text-[9px] text-muted-foreground">{{ candidate.providerId }}</span>
                          <span
                            v-if="getOnlinePricingCandidateUnavailableReason(candidate)"
                            class="block truncate text-[10px] text-rose-600 dark:text-rose-400"
                          >{{ getOnlinePricingCandidateUnavailableReason(candidate) }}</span>
                        </span>
                        <span
                          v-if="isOnlinePricingCandidateSyncable(candidate)"
                          class="shrink-0 text-right text-[9px] tabular-nums text-muted-foreground"
                        >
                          <span class="block">输入 ${{ formatModelPrice(candidate.inputPrice) }}/M</span>
                          <span class="block">输出 ${{ formatModelPrice(candidate.outputPrice) }}/M</span>
                        </span>
                        <span
                          class="flex h-4 w-4 shrink-0 items-center justify-center rounded-full border"
                          :class="selectedOnlinePricingProviderId === candidate.providerId
                            ? 'border-primary bg-primary text-primary-foreground'
                            : 'border-border'"
                        >
                          <Check
                            v-if="selectedOnlinePricingProviderId === candidate.providerId"
                            class="h-2.5 w-2.5"
                          />
                        </span>
                      </button>
                      <div
                        v-if="onlinePricingCandidates.length === 0"
                        class="py-4 text-center text-xs text-muted-foreground"
                      >
                        暂无可用在线价格来源
                      </div>
                    </div>

                    <div class="flex justify-end gap-2 border-t border-border/60 pt-2">
                      <Button
                        type="button"
                        variant="ghost"
                        size="sm"
                        :disabled="syncingOnlinePricing"
                        data-online-pricing-source-control
                        data-testid="online-pricing-source-cancel"
                        @keydown="handleOnlinePricingSourcePopoverKeydown"
                        @click="closeOnlinePricingSourcePopover"
                      >
                        取消
                      </Button>
                      <Button
                        type="button"
                        size="sm"
                        :disabled="!selectedOnlinePricingCandidate
                          || !isOnlinePricingCandidateSyncable(selectedOnlinePricingCandidate)
                          || syncingOnlinePricing"
                        data-online-pricing-source-control
                        @keydown="handleOnlinePricingSourcePopoverKeydown"
                        @click="confirmOnlinePricingSource"
                      >
                        <Loader2
                          v-if="syncingOnlinePricing"
                          class="mr-1.5 h-3.5 w-3.5 animate-spin"
                        />
                        {{ syncingOnlinePricing ? '同步中...' : '同步此来源' }}
                      </Button>
                    </div>
                  </div>
                </PopoverContent>
              </Popover>
            </div>
            <Tabs
              v-model="billingMode"
              @update:model-value="handleBillingModeChange"
            >
              <TabsList class="grid w-full grid-cols-4">
                <TabsTrigger value="token">
                  Token
                </TabsTrigger>
                <TabsTrigger value="request">
                  按次
                </TabsTrigger>
                <TabsTrigger value="image">
                  图片
                </TabsTrigger>
                <TabsTrigger value="video">
                  视频
                </TabsTrigger>
              </TabsList>

              <TieredPricingEditor
                v-show="billingMode === 'token' || billingMode === 'image'"
                ref="tieredPricingEditorRef"
                v-model="tieredPricing"
                class="mt-3"
                :auto-fill-missing-cache-prices="autoFillMissingCachePrices"
                :show-token-pricing="billingMode === 'token'"
                :show-image-pricing="isImageGenerationEnabled"
                :show-image-editor="billingMode === 'image'"
                :show-processing-tier-controls="false"
                :show-processing-tier-multiplier-controls="true"
              />

              <TabsContent
                value="request"
                class="pt-2"
              >
                <div class="rounded-lg border bg-muted/20 p-4 space-y-2">
                  <Label class="text-xs">每次请求价格（美元）</Label>
                  <Input
                    :model-value="form.default_price_per_request ?? ''"
                    type="number"
                    step="0.001"
                    min="0"
                    class="max-w-48"
                    placeholder="如 0.01"
                    @update:model-value="(v) => form.default_price_per_request = parseNumberInput(v, { allowFloat: true })"
                  />
                  <p class="text-xs text-muted-foreground">
                    按每次 API 请求收取固定费用，可与 Token 计费同时使用。
                  </p>
                </div>
              </TabsContent>

              <TabsContent
                value="video"
                class="pt-2"
              >
                <div class="space-y-4 rounded-lg border bg-muted/20 p-4">
                  <div class="flex items-start justify-between gap-3">
                    <div>
                      <div class="text-sm font-medium">
                        {{ videoBillingMode === 'per_second' ? '按秒计费（分辨率 × 时长）' : '按 Token 计费' }}
                      </div>
                      <p class="mt-1 text-xs text-muted-foreground">
                        按输出分辨率配置单价。「含视频输入」列可选，留空则沿用左侧单价；未列出的分辨率按默认价计费。
                      </p>
                    </div>
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      class="h-7 px-2 text-xs text-muted-foreground shrink-0"
                      :aria-label="`切换视频计费方式，当前${videoBillingMode === 'per_second' ? '按秒' : '按 Token'}`"
                      @click="toggleVideoBillingMode"
                    >
                      <Repeat2 class="mr-1 h-3.5 w-3.5" />
                      {{ videoBillingMode === 'per_second' ? '按 Token' : '按秒' }}
                    </Button>
                  </div>

                  <template v-if="videoBillingMode === 'per_second'">
                    <div class="flex items-center gap-1.5 flex-wrap">
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        class="h-7 text-xs"
                        @click="fillVideoResolutionPricePreset('common')"
                      >
                        通用
                      </Button>
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        class="h-7 text-xs"
                        @click="fillVideoResolutionPricePreset('sora')"
                      >
                        Sora
                      </Button>
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        class="h-7 text-xs"
                        @click="fillVideoResolutionPricePreset('veo')"
                      >
                        Veo
                      </Button>
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        class="h-7 text-xs"
                        @click="addVideoResolutionPriceRow"
                      >
                        <Plus class="w-3.5 h-3.5 mr-0.5" />
                        自定义
                      </Button>
                    </div>

                    <div class="rounded-lg border border-border overflow-hidden">
                      <div class="grid grid-cols-[1fr_1fr_1fr_32px] gap-2 text-xs text-muted-foreground bg-muted/50 px-3 py-1.5 border-b border-border">
                        <span>分辨率</span>
                        <span>单价（$/秒）</span>
                        <span>含视频输入（可选）</span>
                        <span />
                      </div>
                      <div class="divide-y divide-border">
                        <div class="grid grid-cols-[1fr_1fr_1fr_32px] gap-2 items-center px-3 py-1.5 bg-muted/20">
                          <span class="text-sm text-muted-foreground">默认价</span>
                          <Input
                            :model-value="videoDefaultPricePerSecond ?? ''"
                            type="number"
                            step="0.0001"
                            min="0"
                            class="h-7 text-sm"
                            placeholder="未列出的分辨率"
                            data-testid="video-second-default-price"
                            @update:model-value="(v) => videoDefaultPricePerSecond = parseNumberInput(v, { allowFloat: true })"
                          />
                          <Input
                            :model-value="videoDefaultPricePerSecondWithInput ?? ''"
                            type="number"
                            step="0.0001"
                            min="0"
                            class="h-7 text-sm"
                            placeholder="可选"
                            data-testid="video-second-default-price-with-input"
                            @update:model-value="(v) => videoDefaultPricePerSecondWithInput = parseNumberInput(v, { allowFloat: true })"
                          />
                          <span />
                        </div>
                        <div
                          v-for="(row, idx) in videoResolutionPrices"
                          :key="idx"
                          class="grid grid-cols-[1fr_1fr_1fr_32px] gap-2 items-center px-3 py-1.5"
                        >
                          <Input
                            v-model="row.resolution"
                            class="h-7 text-sm"
                            placeholder="如 720p"
                          />
                          <Input
                            :model-value="row.price_per_second ?? ''"
                            type="number"
                            step="0.0001"
                            min="0"
                            class="h-7 text-sm"
                            placeholder="0"
                            @update:model-value="(v) => row.price_per_second = parseNumberInput(v, { allowFloat: true })"
                          />
                          <Input
                            :model-value="row.price_per_second_with_input ?? ''"
                            type="number"
                            step="0.0001"
                            min="0"
                            class="h-7 text-sm"
                            placeholder="可选"
                            @update:model-value="(v) => row.price_per_second_with_input = parseNumberInput(v, { allowFloat: true })"
                          />
                          <Button
                            type="button"
                            variant="ghost"
                            size="icon"
                            class="h-7 w-7"
                            title="删除"
                            @click="removeVideoResolutionPriceRow(idx)"
                          >
                            <Trash2 class="w-3.5 h-3.5" />
                          </Button>
                        </div>
                      </div>
                    </div>
                  </template>

                  <template v-else>
                    <div class="flex items-center gap-1.5 flex-wrap">
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        class="h-7 text-xs"
                        @click="addVideoTokenPriceRow"
                      >
                        <Plus class="w-3.5 h-3.5 mr-0.5" />
                        添加分辨率
                      </Button>
                    </div>

                    <div class="rounded-lg border border-border overflow-hidden">
                      <div class="grid grid-cols-[1.1fr_1fr_1fr_1fr_1fr_32px] gap-2 text-xs text-muted-foreground bg-muted/50 px-3 py-1.5 border-b border-border">
                        <span>分辨率</span>
                        <span>输入（$/1M）</span>
                        <span>输出（$/1M）</span>
                        <span>含视频输入·输入</span>
                        <span>含视频输入·输出</span>
                        <span />
                      </div>
                      <div class="divide-y divide-border">
                        <div class="grid grid-cols-[1.1fr_1fr_1fr_1fr_1fr_32px] gap-2 items-center px-3 py-1.5 bg-muted/20">
                          <span class="text-sm text-muted-foreground">默认价</span>
                          <Input
                            :model-value="videoDefaultTokenPrice.input_price_per_1m ?? ''"
                            type="number"
                            step="0.0001"
                            min="0"
                            class="h-7 text-sm"
                            placeholder="0"
                            data-testid="video-token-default-input-price"
                            @update:model-value="(v) => videoDefaultTokenPrice.input_price_per_1m = parseNumberInput(v, { allowFloat: true })"
                          />
                          <Input
                            :model-value="videoDefaultTokenPrice.output_price_per_1m ?? ''"
                            type="number"
                            step="0.0001"
                            min="0"
                            class="h-7 text-sm"
                            placeholder="0"
                            data-testid="video-token-default-output-price"
                            @update:model-value="(v) => videoDefaultTokenPrice.output_price_per_1m = parseNumberInput(v, { allowFloat: true })"
                          />
                          <Input
                            :model-value="videoDefaultTokenPriceWithInput.input_price_per_1m ?? ''"
                            type="number"
                            step="0.0001"
                            min="0"
                            class="h-7 text-sm"
                            placeholder="可选"
                            data-testid="video-token-default-with-input-input-price"
                            @update:model-value="(v) => videoDefaultTokenPriceWithInput.input_price_per_1m = parseNumberInput(v, { allowFloat: true })"
                          />
                          <Input
                            :model-value="videoDefaultTokenPriceWithInput.output_price_per_1m ?? ''"
                            type="number"
                            step="0.0001"
                            min="0"
                            class="h-7 text-sm"
                            placeholder="可选"
                            data-testid="video-token-default-with-input-output-price"
                            @update:model-value="(v) => videoDefaultTokenPriceWithInput.output_price_per_1m = parseNumberInput(v, { allowFloat: true })"
                          />
                          <span />
                        </div>
                        <div
                          v-for="(row, idx) in videoTokenPrices"
                          :key="idx"
                          class="grid grid-cols-[1.1fr_1fr_1fr_1fr_1fr_32px] gap-2 items-center px-3 py-1.5"
                        >
                          <Input
                            v-model="row.resolution"
                            class="h-7 text-sm"
                            placeholder="如 720p"
                            data-testid="video-token-resolution"
                          />
                          <Input
                            :model-value="row.input_price_per_1m ?? ''"
                            type="number"
                            step="0.0001"
                            min="0"
                            class="h-7 text-sm"
                            placeholder="0"
                            data-testid="video-token-input-price"
                            @update:model-value="(v) => row.input_price_per_1m = parseNumberInput(v, { allowFloat: true })"
                          />
                          <Input
                            :model-value="row.output_price_per_1m ?? ''"
                            type="number"
                            step="0.0001"
                            min="0"
                            class="h-7 text-sm"
                            placeholder="0"
                            data-testid="video-token-output-price"
                            @update:model-value="(v) => row.output_price_per_1m = parseNumberInput(v, { allowFloat: true })"
                          />
                          <Input
                            :model-value="row.input_price_per_1m_with_input ?? ''"
                            type="number"
                            step="0.0001"
                            min="0"
                            class="h-7 text-sm"
                            placeholder="可选"
                            data-testid="video-token-with-input-input-price"
                            @update:model-value="(v) => row.input_price_per_1m_with_input = parseNumberInput(v, { allowFloat: true })"
                          />
                          <Input
                            :model-value="row.output_price_per_1m_with_input ?? ''"
                            type="number"
                            step="0.0001"
                            min="0"
                            class="h-7 text-sm"
                            placeholder="可选"
                            data-testid="video-token-with-input-output-price"
                            @update:model-value="(v) => row.output_price_per_1m_with_input = parseNumberInput(v, { allowFloat: true })"
                          />
                          <Button
                            type="button"
                            variant="ghost"
                            size="icon"
                            class="h-7 w-7"
                            title="删除"
                            @click="removeVideoTokenPriceRow(idx)"
                          >
                            <Trash2 class="w-3.5 h-3.5" />
                          </Button>
                        </div>
                      </div>
                    </div>
                  </template>
                </div>
              </TabsContent>
            </Tabs>
          </section>
        </form>
      </div>
    </div>

    <template #footer>
      <Button
        type="button"
        variant="outline"
        @click="handleCancel"
      >
        取消
      </Button>
      <Button
        v-if="!isEditMode && !presetPanelCollapsed"
        type="button"
        variant="outline"
        @click="enterManualEntryMode"
      >
        手动填写
      </Button>
      <Button
        v-if="isEditMode || presetPanelCollapsed"
        :disabled="submitting || syncingOnlinePricing || !form.name || !form.display_name"
        @click="handleSubmit"
      >
        <Loader2
          v-if="submitting"
          class="w-4 h-4 mr-2 animate-spin"
        />
        {{ isEditMode ? '保存' : '添加' }}
      </Button>
      <Button
        v-if="selectedModel && !isEditMode && presetPanelCollapsed"
        type="button"
        variant="ghost"
        @click="clearSelection"
      >
        清空
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { ref, computed, nextTick, watch } from 'vue'
import {
  Loader2, Layers, SquarePen,
  Search, ChevronLeft, ChevronRight, Plus, Trash2, Check,
  BrainCircuit, Eye, Wrench, Braces, Database, PackageOpen, RefreshCw, Repeat2
} from 'lucide-vue-next'
import {
  Dialog, Button, Input, Label, Checkbox,
  Tabs, TabsContent, TabsList, TabsTrigger,
  Popover, PopoverTrigger, PopoverContent,
} from '@/components/ui'
import { useToast } from '@/composables/useToast'
import { useFormDialog } from '@/composables/useFormDialog'
import { parseNumberInput, sortResolutionEntries } from '@/utils/form'
import { log } from '@/utils/logger'
import { parseApiError } from '@/utils/errorParser'
import TieredPricingEditor from './TieredPricingEditor.vue'
import {
  getModelsDevList,
  getProviderLogoUrl,
  refreshModelsDevList,
  type ModelsDevModelItem,
} from '@/api/models-dev'
import {
  createGlobalModel,
  listGlobalModels,
  updateGlobalModel,
  type GlobalModelResponse,
} from '@/api/global-models'
import type { TieredPricingConfig } from '@/api/endpoints/types'
import {
  EMBEDDING_API_FORMATS,
  buildGlobalModelCreatePayload,
  buildGlobalModelUpdatePayload,
  cloneTieredPricingConfig,
  findGlobalModelByName,
  tieredPricingConfigsEqual,
} from './global-model-form-helpers'
import { tieredPricingHasImageOutputPricing } from '../utils/tiered-pricing'
import { useModelsDevPricingSources } from '../composables/useModelsDevPricingSources'

const props = defineProps<{
  open: boolean
  model?: GlobalModelResponse | null
}>()

const emit = defineEmits<{
  'update:open': [value: boolean]
  'success': []
  'editModel': [model: GlobalModelResponse]
  'pricingSynced': [model: GlobalModelResponse]
}>()

const { success, error: showError } = useToast()
const { getSource, setSource } = useModelsDevPricingSources()
const submitting = ref(false)
const syncingOnlinePricing = ref(false)
const tieredPricingEditorRef = ref<InstanceType<typeof TieredPricingEditor> | null>(null)
const basicInfoSection = ref<HTMLElement | null>(null)

// 模型列表相关
const loading = ref(false)
const searchQuery = ref('')
const allModelsCache = ref<ModelsDevModelItem[]>([]) // 全部模型（缓存）
const existingModelsCache = ref<GlobalModelResponse[]>([])
const selectedModel = ref<ModelsDevModelItem | null>(null)
const expandedProvider = ref<string | null>(null)
const providerLogoScroller = ref<HTMLElement | null>(null)
const presetPanelCollapsed = ref(false)
const billingMode = ref('token')
const editingOnlinePricingSource = ref<{
  model_id: string
  provider_id: string
  provider_name: string
} | null>(null)
const onlinePricingSourcePopoverOpen = ref(false)
const onlinePricingCandidates = ref<ModelsDevModelItem[]>([])
const selectedOnlinePricingProviderId = ref('')

const selectedOnlinePricingCandidate = computed(() => (
  onlinePricingCandidates.value.find(candidate => (
    candidate.providerId === selectedOnlinePricingProviderId.value
  )) ?? null
))
const firstSyncableOnlinePricingProviderId = computed(() => (
  onlinePricingCandidates.value.find(isOnlinePricingCandidateSyncable)?.providerId ?? ''
))

watch([onlinePricingSourcePopoverOpen, syncingOnlinePricing], ([open, syncing]) => {
  if (!open || syncing) return
  void nextTick().then(() => {
    if (onlinePricingSourcePopoverOpen.value && !syncingOnlinePricing.value) {
      focusOnlinePricingSourcePopover()
    }
  })
}, { flush: 'post' })

function getExistingModel(model: ModelsDevModelItem): GlobalModelResponse | undefined {
  return findGlobalModelByName(existingModelsCache.value, model.modelId)
}

function formatTokenLimit(value: number): string {
  if (value >= 1_000_000) {
    return `${Number((value / 1_000_000).toFixed(1))}M`
  }
  if (value >= 1_000) {
    return `${Number((value / 1_000).toFixed(1))}K`
  }
  return String(value)
}

function formatModelPrice(value?: number): string {
  if (value === undefined) return '-'
  if (value === 0) return '0'
  const precision = value < 0.01 ? 4 : value < 1 ? 3 : 2
  return value.toFixed(precision).replace(/\.?0+$/, '')
}

// 当前显示的模型列表：有搜索词时用全部，否则只用官方
const allModels = computed(() => {
  if (searchQuery.value) {
    return allModelsCache.value
  }
  return allModelsCache.value.filter(m => m.official)
})

// 按提供商分组的模型
interface ProviderGroup {
  providerId: string
  providerName: string
  models: ModelsDevModelItem[]
}

const PROVIDER_PRIORITY_KEYWORDS = [
  ['anthropic', 'claude'],
  ['openai'],
  ['google', 'gemini'],
]

function getProviderPriority(group: ProviderGroup): number {
  const searchableText = `${group.providerId} ${group.providerName}`.toLowerCase()
  const priority = PROVIDER_PRIORITY_KEYWORDS.findIndex(keywords => (
    keywords.some(keyword => searchableText.includes(keyword))
  ))
  return priority === -1 ? PROVIDER_PRIORITY_KEYWORDS.length : priority
}

function getDefaultProviderId(groups: ProviderGroup[]): string | null {
  const claudeProvider = groups.find(group => getProviderPriority(group) === 0)
  return claudeProvider?.providerId ?? groups[0]?.providerId ?? null
}

const groupedModels = computed(() => {
  let models = allModels.value.filter(m => !m.deprecated)
  // 搜索（支持空格分隔的多关键词 AND 搜索）
  if (searchQuery.value) {
    const keywords = searchQuery.value.toLowerCase().split(/\s+/).filter(k => k.length > 0)
    models = models.filter(model => {
      const searchableText = `${model.providerId} ${model.providerName} ${model.modelId} ${model.modelName} ${model.family || ''}`.toLowerCase()
      return keywords.every(keyword => searchableText.includes(keyword))
    })
  }

  // 按提供商分组
  const groups = new Map<string, ProviderGroup>()
  for (const model of models) {
    if (!groups.has(model.providerId)) {
      groups.set(model.providerId, {
        providerId: model.providerId,
        providerName: model.providerName,
        models: []
      })
    }
    groups.get(model.providerId)?.models.push(model)
  }

  // 转换为数组并排序
  const result = Array.from(groups.values())

  const searchKeywords = searchQuery.value.toLowerCase().split(/\s+/).filter(k => k.length > 0)
  result.sort((a, b) => {
    // 搜索时，优先展示提供商名称或 ID 直接匹配的结果
    if (searchKeywords.length > 0) {
      const aText = `${a.providerId} ${a.providerName}`.toLowerCase()
      const bText = `${b.providerId} ${b.providerName}`.toLowerCase()
      const aProviderMatch = searchKeywords.some(keyword => aText.includes(keyword))
      const bProviderMatch = searchKeywords.some(keyword => bText.includes(keyword))
      if (aProviderMatch && !bProviderMatch) return -1
      if (!aProviderMatch && bProviderMatch) return 1
    }

    // Claude（Anthropic）、OpenAI、Google 固定排在最前
    const priorityDifference = getProviderPriority(a) - getProviderPriority(b)
    if (priorityDifference !== 0) return priorityDifference
    return a.providerName.localeCompare(b.providerName)
  })

  return result
})

const expandedProviderGroup = computed(() => (
  groupedModels.value.find(group => group.providerId === expandedProvider.value) ?? null
))

// 搜索时如果只有一个提供商，自动展开
watch(groupedModels, (groups) => {
  if (expandedProvider.value && !groups.some(group => group.providerId === expandedProvider.value)) {
    expandedProvider.value = null
  }
  if (searchQuery.value && groups.length === 1) {
    expandedProvider.value = groups[0].providerId
  } else if (!searchQuery.value && !expandedProvider.value) {
    expandedProvider.value = getDefaultProviderId(groups)
  }
})

// 切换提供商展开状态
function toggleProvider(providerId: string) {
  expandedProvider.value = expandedProvider.value === providerId ? null : providerId
}

function scrollProviderLogos(direction: -1 | 1) {
  providerLogoScroller.value?.scrollBy({
    left: direction * 280,
    behavior: 'smooth',
  })
}

function handleBillingModeChange(mode: string) {
  billingMode.value = mode
  if (mode === 'image' && !isImageGenerationEnabled.value) {
    setImageGenerationEnabled(true)
  }
}

function scrollToBasicInformation() {
  nextTick(() => {
    basicInfoSection.value?.scrollIntoView({
      behavior: 'smooth',
      block: 'nearest',
    })
  })
}

function enterManualEntryMode() {
  clearSelection()
  presetPanelCollapsed.value = true
  scrollToBasicInformation()
}

function reopenPresetPanel() {
  clearSelection()
  presetPanelCollapsed.value = false
}

// 阶梯计费配置
const tieredPricing = ref<TieredPricingConfig | null>(null)

/** One resolution row; the `_with_input` fields are the optional override column. */
type VideoResolutionPriceRow = {
  resolution: string
  price_per_second: number | undefined
  price_per_second_with_input: number | undefined
}

type VideoTokenPriceRow = {
  resolution: string
  input_price_per_1m: number | undefined
  output_price_per_1m: number | undefined
  input_price_per_1m_with_input: number | undefined
  output_price_per_1m_with_input: number | undefined
}

type VideoTokenDefaultPrice = {
  input_price_per_1m: number | undefined
  output_price_per_1m: number | undefined
}

const videoResolutionPrices = ref<VideoResolutionPriceRow[]>([])
/** Per-resolution token prices, used when the model bills per token. */
const videoTokenPrices = ref<VideoTokenPriceRow[]>([])
/** Charged for any resolution the table does not list. */
const videoDefaultPricePerSecond = ref<number | undefined>(undefined)
const videoDefaultPricePerSecondWithInput = ref<number | undefined>(undefined)
const videoDefaultTokenPrice = ref<VideoTokenDefaultPrice>({
  input_price_per_1m: undefined,
  output_price_per_1m: undefined,
})
const videoDefaultTokenPriceWithInput = ref<VideoTokenDefaultPrice>({
  input_price_per_1m: undefined,
  output_price_per_1m: undefined,
})
const videoBillingMode = ref<'per_second' | 'per_token'>('per_second')

function toggleVideoBillingMode() {
  // Switching modes keeps the other mode's tables intact so operators can flip
  // back and forth while comparing.
  videoBillingMode.value = videoBillingMode.value === 'per_second' ? 'per_token' : 'per_second'
}

const VIDEO_RESOLUTION_PRICE_PRESETS: Record<
  'common' | 'sora' | 'veo',
  Array<{ resolution: string; price_per_second: number }>
> = {
  common: [
    { resolution: '480p', price_per_second: 0 },
    { resolution: '720p', price_per_second: 0 },
    { resolution: '1080p', price_per_second: 0 },
    { resolution: '4k', price_per_second: 0 },
  ],
  sora: [
    { resolution: '720x1080', price_per_second: 0 },
    { resolution: '1024x1792', price_per_second: 0 },
  ],
  veo: [
    { resolution: '720p', price_per_second: 0 },
    { resolution: '1080p', price_per_second: 0 },
    { resolution: '4k', price_per_second: 0 },
  ],
}

const embeddingApiFormats = [...EMBEDDING_API_FORMATS]

interface FormData {
  name: string
  display_name: string
  default_price_per_request?: number
  supported_capabilities?: string[]
  config?: Record<string, unknown>
  is_active?: boolean
}

const defaultForm = (): FormData => ({
  name: '',
  display_name: '',
  default_price_per_request: undefined,
  supported_capabilities: [],
  config: { streaming: true },
  is_active: true,
})

const form = ref<FormData>(defaultForm())
const imageGenerationExplicitOverride = ref<boolean | null>(null)

const isEmbeddingEnabled = computed(() => {
  return form.value.supported_capabilities?.includes('embedding') === true
    || form.value.config?.embedding === true
    || form.value.config?.model_type === 'embedding'
})

const isImageGenerationEnabled = computed(() => {
  if (imageGenerationExplicitOverride.value !== null) {
    return imageGenerationExplicitOverride.value
  }
  return form.value.supported_capabilities?.includes('image_generation') === true
    || form.value.config?.image_generation === true
    || form.value.config?.model_type === 'image'
    || (Array.isArray(form.value.config?.api_formats)
      && form.value.config.api_formats.some((format) => String(format).endsWith(':image')))
    || tieredPricingHasImageOutputPricing(tieredPricing.value)
})

const KEEP_FALSE_CONFIG_KEYS = new Set(['streaming'])

// 设置 config 字段
function setConfigField(key: string, value: unknown) {
  if (!form.value.config) {
    form.value.config = {}
  }
  if (value === undefined || value === '' || (value === false && !KEEP_FALSE_CONFIG_KEYS.has(key))) {
    delete form.value.config[key]
  } else {
    form.value.config[key] = value
  }
}

function getConfigInputValue(key: string): string | number {
  const value = form.value.config?.[key]
  return typeof value === 'string' || typeof value === 'number' ? value : ''
}

function setEmbeddingEnabled(enabled: boolean) {
  const caps = new Set(form.value.supported_capabilities || [])
  if (enabled) {
    caps.add('embedding')
    setConfigField('embedding', true)
    setConfigField('model_type', 'embedding')
    setConfigField('streaming', false)
    form.value.config = {
      ...(form.value.config || {}),
      api_formats: [...embeddingApiFormats],
    }
  } else {
    caps.delete('embedding')
    setConfigField('embedding', undefined)
    if (form.value.config?.model_type === 'embedding') setConfigField('model_type', undefined)
    if (Array.isArray(form.value.config?.api_formats)
      && form.value.config.api_formats.every((format) => embeddingApiFormats.includes(String(format)))) {
      setConfigField('api_formats', undefined)
    }
  }
  form.value.supported_capabilities = [...caps]
}

function setImageGenerationEnabled(value: boolean | 'indeterminate') {
  const enabled = value === true
  imageGenerationExplicitOverride.value = enabled
  const caps = new Set(form.value.supported_capabilities || [])
  if (enabled) {
    caps.add('image_generation')
    setConfigField('image_generation', true)
  } else {
    caps.delete('image_generation')
    setConfigField('image_generation', undefined)
    if (form.value.config?.model_type === 'image') setConfigField('model_type', undefined)
  }
  form.value.supported_capabilities = [...caps]
}

function getNested(obj: unknown, path: string): unknown {
  if (!obj || typeof obj !== 'object') return undefined
  const parts = path.split('.').filter(Boolean)
  let cur = obj as Record<string, unknown>
  for (const p of parts) {
    if (!cur || typeof cur !== 'object') return undefined
    cur = cur[p] as Record<string, unknown>
  }
  return cur
}

function setNested(obj: unknown, path: string, value: unknown) {
  if (!obj || typeof obj !== 'object') return
  const parts = path.split('.').filter(Boolean)
  if (parts.length === 0) return
  let cur = obj as Record<string, unknown>
  for (let i = 0; i < parts.length - 1; i++) {
    const p = parts[i]
    if (!cur[p] || typeof cur[p] !== 'object') {
      cur[p] = {}
    }
    cur = cur[p] as Record<string, unknown>
  }
  cur[parts[parts.length - 1]] = value
}

function deleteNested(obj: unknown, path: string) {
  if (!obj || typeof obj !== 'object') return
  const parts = path.split('.').filter(Boolean)
  if (parts.length === 0) return
  let cur = obj as Record<string, unknown>
  for (let i = 0; i < parts.length - 1; i++) {
    const p = parts[i]
    if (!cur[p] || typeof cur[p] !== 'object') return
    cur = cur[p] as Record<string, unknown>
  }
  delete cur[parts[parts.length - 1]]
}

function pruneEmptyBillingConfig() {
  const cfg = form.value.config
  if (!cfg || typeof cfg !== 'object') return
  const billing = cfg.billing as Record<string, unknown> | undefined
  if (!billing || typeof billing !== 'object') return
  const video = billing.video as Record<string, unknown> | undefined
  if (video && typeof video === 'object') {
    // Both price tables can be cleared independently, so drop the container
    // once nothing is left inside it.
    const withInput = video.with_video_input as Record<string, unknown> | undefined
    if (withInput && typeof withInput === 'object' && Object.keys(withInput).length === 0) {
      delete video.with_video_input
    }
    if (Object.keys(video).length === 0) {
      delete billing.video
    }
  }
  if (Object.keys(billing).length === 0) {
    delete cfg.billing
  }
}

/**
 * Normalize resolution key:
 * - lowercase, remove spaces, × → x
 * - For WxH format, sort dimensions so smaller comes first (720x1080 = 1080x720)
 */
function normalizeResolutionKey(raw: string): string {
  let k = (raw || '').trim().toLowerCase().replace(/\s+/g, '').replace(/×/g, 'x')
  // Check if it's WxH format (e.g., 1080x720)
  const match = k.match(/^(\d+)x(\d+)$/)
  if (match) {
    const a = parseInt(match[1], 10)
    const b = parseInt(match[2], 10)
    // Sort: smaller dimension first
    k = a <= b ? `${a}x${b}` : `${b}x${a}`
  }
  return k
}

/** Merges the base table and its `with_video_input` overrides into one row list. */
function mergeVideoPriceRows<TRow extends { resolution: string }>(
  base: unknown,
  withInput: unknown,
  buildRow: (resolution: string, entry: unknown, overrideEntry: unknown) => TRow,
): TRow[] {
  const asTable = (raw: unknown): Record<string, unknown> =>
    (raw && typeof raw === 'object' && !Array.isArray(raw)) ? raw as Record<string, unknown> : {}
  const baseTable = asTable(base)
  const overrideTable = asTable(withInput)
  // A resolution priced only in the override column still needs its own row.
  const keys = [...new Set([...Object.keys(baseTable), ...Object.keys(overrideTable)])]
  return sortResolutionEntries(keys.map(key => [key, baseTable[key]] as [string, unknown]))
    .map(([key]) => buildRow(String(key), baseTable[key], overrideTable[key]))
}

function readVideoNumber(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined
}

function readTokenEntryField(entry: unknown, field: string): number | undefined {
  if (!entry || typeof entry !== 'object' || Array.isArray(entry)) return undefined
  return readVideoNumber((entry as Record<string, unknown>)[field])
}

function loadVideoPricingFromConfig() {
  const cfg = form.value.config || {}
  const mode = getNested(cfg, 'billing.video.mode')
  videoBillingMode.value = mode === 'per_token' ? 'per_token' : 'per_second'

  videoResolutionPrices.value = mergeVideoPriceRows(
    getNested(cfg, 'billing.video.price_per_second_by_resolution'),
    getNested(cfg, 'billing.video.with_video_input.price_per_second_by_resolution'),
    (resolution, entry, overrideEntry) => ({
      resolution,
      price_per_second: readVideoNumber(entry),
      price_per_second_with_input: readVideoNumber(overrideEntry),
    }),
  )
  videoDefaultPricePerSecond.value = readVideoNumber(
    getNested(cfg, 'billing.video.price_per_second_default'),
  )
  videoDefaultPricePerSecondWithInput.value = readVideoNumber(
    getNested(cfg, 'billing.video.with_video_input.price_per_second_default'),
  )

  videoTokenPrices.value = mergeVideoPriceRows(
    getNested(cfg, 'billing.video.token_prices_by_resolution'),
    getNested(cfg, 'billing.video.with_video_input.token_prices_by_resolution'),
    (resolution, entry, overrideEntry) => ({
      resolution,
      input_price_per_1m: readTokenEntryField(entry, 'input_price_per_1m'),
      output_price_per_1m: readTokenEntryField(entry, 'output_price_per_1m'),
      input_price_per_1m_with_input: readTokenEntryField(overrideEntry, 'input_price_per_1m'),
      output_price_per_1m_with_input: readTokenEntryField(overrideEntry, 'output_price_per_1m'),
    }),
  )
  const tokenDefault = getNested(cfg, 'billing.video.token_price_default')
  videoDefaultTokenPrice.value = {
    input_price_per_1m: readTokenEntryField(tokenDefault, 'input_price_per_1m'),
    output_price_per_1m: readTokenEntryField(tokenDefault, 'output_price_per_1m'),
  }
  const tokenDefaultWithInput = getNested(cfg, 'billing.video.with_video_input.token_price_default')
  videoDefaultTokenPriceWithInput.value = {
    input_price_per_1m: readTokenEntryField(tokenDefaultWithInput, 'input_price_per_1m'),
    output_price_per_1m: readTokenEntryField(tokenDefaultWithInput, 'output_price_per_1m'),
  }
}

/** Writes `value` at `path`, removing the key when the value is absent. */
function setOrDeleteNested(cfg: Record<string, unknown>, path: string, value: unknown) {
  const isEmptyObject = value !== null
    && typeof value === 'object'
    && Object.keys(value as Record<string, unknown>).length === 0
  if (value === undefined || value === null || isEmptyObject) {
    deleteNested(cfg, path)
  } else {
    setNested(cfg, path, value)
  }
}

function buildTokenPriceEntry(
  input: number | undefined,
  output: number | undefined,
): Record<string, number> | undefined {
  const entry: Record<string, number> = {}
  if (typeof input === 'number' && Number.isFinite(input)) entry.input_price_per_1m = input
  if (typeof output === 'number' && Number.isFinite(output)) entry.output_price_per_1m = output
  return Object.keys(entry).length > 0 ? entry : undefined
}

function applyVideoPricingToConfig() {
  if (!form.value.config) {
    form.value.config = {}
  }
  const cfg = form.value.config

  // Clean legacy keys
  deleteNested(cfg, 'billing.video.price_per_second')
  deleteNested(cfg, 'billing.video.resolution_multipliers')

  // per_second is the backend default, so it is only persisted when explicit.
  setOrDeleteNested(
    cfg,
    'billing.video.mode',
    videoBillingMode.value === 'per_token' ? 'per_token' : undefined,
  )

  // The merged table splits back into a base map and an override map, since
  // that is the shape the backend resolves prices from.
  const secondMap: Record<string, number> = {}
  const secondMapWithInput: Record<string, number> = {}
  for (const row of videoResolutionPrices.value) {
    const key = normalizeResolutionKey(row.resolution || '')
    if (!key) continue
    const price = readVideoNumber(row.price_per_second)
    if (price !== undefined) secondMap[key] = price
    const overridePrice = readVideoNumber(row.price_per_second_with_input)
    if (overridePrice !== undefined) secondMapWithInput[key] = overridePrice
  }
  setOrDeleteNested(cfg, 'billing.video.price_per_second_by_resolution', secondMap)
  setOrDeleteNested(
    cfg,
    'billing.video.with_video_input.price_per_second_by_resolution',
    secondMapWithInput,
  )
  setOrDeleteNested(
    cfg,
    'billing.video.price_per_second_default',
    readVideoNumber(videoDefaultPricePerSecond.value),
  )
  setOrDeleteNested(
    cfg,
    'billing.video.with_video_input.price_per_second_default',
    readVideoNumber(videoDefaultPricePerSecondWithInput.value),
  )

  const tokenMap: Record<string, unknown> = {}
  const tokenMapWithInput: Record<string, unknown> = {}
  for (const row of videoTokenPrices.value) {
    const key = normalizeResolutionKey(row.resolution || '')
    if (!key) continue
    const entry = buildTokenPriceEntry(row.input_price_per_1m, row.output_price_per_1m)
    if (entry) tokenMap[key] = entry
    const overrideEntry = buildTokenPriceEntry(
      row.input_price_per_1m_with_input,
      row.output_price_per_1m_with_input,
    )
    if (overrideEntry) tokenMapWithInput[key] = overrideEntry
  }
  setOrDeleteNested(cfg, 'billing.video.token_prices_by_resolution', tokenMap)
  setOrDeleteNested(
    cfg,
    'billing.video.with_video_input.token_prices_by_resolution',
    tokenMapWithInput,
  )
  setOrDeleteNested(
    cfg,
    'billing.video.token_price_default',
    buildTokenPriceEntry(
      videoDefaultTokenPrice.value.input_price_per_1m,
      videoDefaultTokenPrice.value.output_price_per_1m,
    ),
  )
  setOrDeleteNested(
    cfg,
    'billing.video.with_video_input.token_price_default',
    buildTokenPriceEntry(
      videoDefaultTokenPriceWithInput.value.input_price_per_1m,
      videoDefaultTokenPriceWithInput.value.output_price_per_1m,
    ),
  )

  pruneEmptyBillingConfig()
}

function addVideoResolutionPriceRow() {
  videoResolutionPrices.value.push({
    resolution: '',
    price_per_second: undefined,
    price_per_second_with_input: undefined,
  })
}

function removeVideoResolutionPriceRow(idx: number) {
  videoResolutionPrices.value.splice(idx, 1)
}

function addVideoTokenPriceRow() {
  videoTokenPrices.value.push({
    resolution: '',
    input_price_per_1m: undefined,
    output_price_per_1m: undefined,
    input_price_per_1m_with_input: undefined,
    output_price_per_1m_with_input: undefined,
  })
}

function removeVideoTokenPriceRow(idx: number) {
  videoTokenPrices.value.splice(idx, 1)
}

function fillVideoResolutionPricePreset(preset: 'common' | 'sora' | 'veo') {
  videoResolutionPrices.value = VIDEO_RESOLUTION_PRICE_PRESETS[preset].map(r => ({
    resolution: r.resolution,
    price_per_second: r.price_per_second,
    price_per_second_with_input: undefined,
  }))
}


async function loadExistingModels() {
  const models: GlobalModelResponse[] = []
  let total = 0
  do {
    const response = await listGlobalModels({ skip: models.length, limit: 1000 })
    models.push(...response.models)
    total = response.total
    if (response.models.length === 0) break
  } while (models.length < total)
  existingModelsCache.value = models
}

// 加载在线目录和已有模型列表
async function loadModels() {
  loading.value = true
  await Promise.all([
    allModelsCache.value.length > 0
      ? Promise.resolve()
      : getModelsDevList(false)
          .then(models => { allModelsCache.value = models })
          .catch(err => log.error('Failed to load online models:', err)),
    loadExistingModels()
      .catch(err => log.error('Failed to load existing models:', err)),
  ])
  loading.value = false
}

// 打开对话框时加载数据
watch(() => props.open, async (isOpen) => {
  if (!isOpen) {
    editingOnlinePricingSource.value = null
    resetOnlinePricingSourceSelection()
    return
  }
  if (isOpen && !props.model) {
    await loadModels()
    if (!expandedProvider.value) {
      expandedProvider.value = getDefaultProviderId(groupedModels.value)
    }
  }
})

// 选择模型并填充表单
function selectModel(model: ModelsDevModelItem) {
  if (getExistingModel(model)) return

  imageGenerationExplicitOverride.value = null
  selectedModel.value = model
  expandedProvider.value = model.providerId

  // 构建 config
  const config: Record<string, unknown> = {
    streaming: model.supportsEmbedding ? false : true,
  }
  if (model.supportsVision) config.vision = true
  if (model.supportsToolCall) config.function_calling = true
  if (model.supportsReasoning) config.extended_thinking = true
  if (model.supportsStructuredOutput) config.structured_output = true
  if (model.supportsTemperature !== false) config.temperature = model.supportsTemperature
  if (model.supportsAttachment) config.attachment = true
  if (model.openWeights) config.open_weights = true
  if (model.contextLimit) config.context_limit = model.contextLimit
  if (model.outputLimit) config.output_limit = model.outputLimit
  if (model.knowledgeCutoff) config.knowledge_cutoff = model.knowledgeCutoff
  if (model.family) config.family = model.family
  if (model.releaseDate) config.release_date = model.releaseDate
  if (model.inputModalities?.length) config.input_modalities = model.inputModalities
  if (model.outputModalities?.length) config.output_modalities = model.outputModalities
  const supportedCapabilities = new Set<string>()
  if (model.supportsEmbedding) supportedCapabilities.add('embedding')
  if (model.outputModalities?.includes('image')) supportedCapabilities.add('image_generation')
  form.value = {
    ...defaultForm(),
    name: model.modelId,
    display_name: model.modelName,
    config,
    supported_capabilities: [...supportedCapabilities],
  }
  if (model.supportsEmbedding) {
    setEmbeddingEnabled(true)
  }
  if (model.outputModalities?.includes('image')) {
    billingMode.value = 'image'
  } else if (model.outputModalities?.includes('video')) {
    billingMode.value = 'video'
  } else {
    billingMode.value = 'token'
  }
  loadVideoPricingFromConfig()

  tieredPricing.value = model.tieredPricing
    ? cloneTieredPricingConfig(model.tieredPricing)
    : null

  presetPanelCollapsed.value = true
  scrollToBasicInformation()
}

function editExistingModel(model: ModelsDevModelItem) {
  const existingModel = getExistingModel(model)
  if (!existingModel) return
  editingOnlinePricingSource.value = {
    model_id: model.modelId,
    provider_id: model.providerId,
    provider_name: model.providerName,
  }
  emit('editModel', existingModel)
}

function normalizeModelId(value: string): string {
  return value.trim().toLowerCase()
}

function formatUnsupportedPricingFields(
  fields: ModelsDevModelItem['pricingUnsupportedFields'],
): string {
  const labels = {
    reasoning: '推理 Token',
    input_audio: '输入音频 Token',
    output_audio: '输出音频 Token',
  }
  return (fields ?? []).map(field => labels[field]).join('、')
}

function getOnlinePricingCandidates(
  models: ModelsDevModelItem[],
  model: GlobalModelResponse,
): ModelsDevModelItem[] {
  const modelId = normalizeModelId(model.name)
  return models.filter(item => normalizeModelId(item.modelId) === modelId)
}

function resolveOnlinePricingModel(
  candidates: ModelsDevModelItem[],
  model: GlobalModelResponse,
): ModelsDevModelItem | null {
  const modelId = normalizeModelId(model.name)
  const transientSource = editingOnlinePricingSource.value
  const storedSource = getSource(model.id)
  const preferredProviderId = transientSource?.model_id && normalizeModelId(transientSource.model_id) === modelId
    ? transientSource.provider_id
    : storedSource?.provider_id

  if (preferredProviderId) {
    const preferred = candidates.find(item => (
      item.providerId.trim().toLowerCase() === preferredProviderId.trim().toLowerCase()
    ))
    if (preferred && isOnlinePricingCandidateSyncable(preferred)) return preferred
  }
  if (candidates.length === 1) return candidates[0]
  if (candidates.length === 0) {
    throw new Error('models.dev 未找到该模型的在线价格')
  }
  return null
}

function isOnlinePricingCandidateSyncable(model: ModelsDevModelItem): boolean {
  return !model.pricingUnsupportedFields?.length && !!model.tieredPricing?.tiers?.length
}

function getOnlinePricingCandidateUnavailableReason(model: ModelsDevModelItem): string {
  if (model.pricingUnsupportedFields?.length) {
    return `不支持：${formatUnsupportedPricingFields(model.pricingUnsupportedFields)}`
  }
  if (!model.tieredPricing?.tiers?.length) return '在线目录未提供 Token 价格'
  return ''
}

function resetOnlinePricingSourceSelection() {
  onlinePricingSourcePopoverOpen.value = false
  onlinePricingCandidates.value = []
  selectedOnlinePricingProviderId.value = ''
}

function getOnlinePricingSourceControls(): HTMLElement[] {
  return [...document.querySelectorAll<HTMLElement>(
    '[data-online-pricing-source-control]:not([disabled]):not([tabindex="-1"])',
  )]
}

function focusOnlinePricingSourcePopover() {
  getOnlinePricingSourceControls()[0]?.focus()
}

function handleOnlinePricingSourcePopoverKeydown(event: KeyboardEvent) {
  const target = event.target instanceof HTMLElement
    ? event.target.closest<HTMLElement>('[data-online-pricing-source-control]')
    : null
  if (!target) return

  if (event.key === 'Tab') {
    const controls = getOnlinePricingSourceControls()
    const currentIndex = controls.indexOf(target)
    if (currentIndex === -1 || controls.length === 0) return
    if (event.shiftKey && currentIndex === 0) {
      event.preventDefault()
      controls[controls.length - 1]?.focus()
    } else if (!event.shiftKey && currentIndex === controls.length - 1) {
      event.preventDefault()
      controls[0]?.focus()
    }
    return
  }

  if (!['ArrowDown', 'ArrowRight', 'ArrowUp', 'ArrowLeft', 'Home', 'End'].includes(event.key)) {
    return
  }
  const options = [...document.querySelectorAll<HTMLButtonElement>(
    '[data-online-pricing-source-option]:not([disabled])',
  )]
  const currentIndex = options.indexOf(target as HTMLButtonElement)
  if (currentIndex === -1 || options.length === 0) return

  event.preventDefault()
  const nextIndex = event.key === 'Home'
    ? 0
    : event.key === 'End'
      ? options.length - 1
      : ['ArrowDown', 'ArrowRight'].includes(event.key)
        ? (currentIndex + 1) % options.length
        : (currentIndex - 1 + options.length) % options.length
  const nextOption = options[nextIndex]
  const providerId = nextOption?.dataset.providerId
  if (!nextOption || !providerId) return
  selectedOnlinePricingProviderId.value = providerId
  nextTick(() => nextOption.focus())
}

function handleOnlinePricingSourcePopoverUpdate(open: boolean) {
  if (open) {
    // The trigger starts the async refresh. Only show the popover once the
    // refresh is in progress or source candidates are available.
    if (syncingOnlinePricing.value || onlinePricingCandidates.value.length > 0) {
      onlinePricingSourcePopoverOpen.value = true
    }
    return
  }
  if (!syncingOnlinePricing.value) resetOnlinePricingSourceSelection()
}

function closeOnlinePricingSourcePopover() {
  if (syncingOnlinePricing.value) return
  resetOnlinePricingSourceSelection()
}

async function applyOnlinePricingModel(onlineModel: ModelsDevModelItem) {
  if (!props.model) return
  if (onlineModel.pricingUnsupportedFields?.length) {
    throw new Error(
      `在线价格包含当前计费引擎无法独立结算的${formatUnsupportedPricingFields(onlineModel.pricingUnsupportedFields)}`,
    )
  }
  if (!onlineModel.tieredPricing?.tiers?.length) {
    throw new Error('在线目录未提供该模型的价格配置')
  }

  const pricing = cloneTieredPricingConfig(onlineModel.tieredPricing)
  const pricingChanged = !tieredPricingConfigsEqual(
    props.model.default_tiered_pricing,
    pricing,
  )
  let syncedModel: GlobalModelResponse
  if (pricingChanged) {
    syncedModel = await updateGlobalModel(props.model.id, {
      default_tiered_pricing: pricing,
    })
  } else {
    syncedModel = {
      ...props.model,
      default_tiered_pricing: pricing,
    }
  }
  tieredPricing.value = cloneTieredPricingConfig(pricing)
  billingMode.value = 'token'
  setSource(props.model.id, {
    provider_id: onlineModel.providerId,
    provider_name: onlineModel.providerName,
  })
  emit('pricingSynced', syncedModel)
  success(
    pricingChanged
      ? `已同步 ${onlineModel.providerName} 的最新价格`
      : `当前价格已是 ${onlineModel.providerName} 的最新价格`,
  )
}

async function confirmOnlinePricingSource() {
  const onlineModel = selectedOnlinePricingCandidate.value
  if (!onlineModel || syncingOnlinePricing.value || submitting.value) return

  syncingOnlinePricing.value = true
  let synced = false
  try {
    await applyOnlinePricingModel(onlineModel)
    synced = true
  } catch (err: unknown) {
    log.error('同步在线模型价格失败:', err)
    showError(parseApiError(err, '同步在线价格失败'), '同步失败')
  } finally {
    syncingOnlinePricing.value = false
    if (synced) resetOnlinePricingSourceSelection()
  }
}

async function syncOnlinePricing() {
  if (!isEditMode.value || !props.model || syncingOnlinePricing.value || submitting.value) return

  syncingOnlinePricing.value = true
  try {
    const onlineModels = await refreshModelsDevList(false)
    allModelsCache.value = onlineModels
    const candidates = getOnlinePricingCandidates(onlineModels, props.model)
    const onlineModel = resolveOnlinePricingModel(candidates, props.model)
    if (onlineModel) {
      await applyOnlinePricingModel(onlineModel)
      resetOnlinePricingSourceSelection()
      return
    }

    onlinePricingCandidates.value = candidates
    selectedOnlinePricingProviderId.value = ''
    onlinePricingSourcePopoverOpen.value = true
  } catch (err: unknown) {
    resetOnlinePricingSourceSelection()
    log.error('同步在线模型价格失败:', err)
    showError(parseApiError(err, '同步在线价格失败'), '同步失败')
  } finally {
    syncingOnlinePricing.value = false
  }
}

// 清除选择（手动填写）
function clearSelection() {
  imageGenerationExplicitOverride.value = null
  selectedModel.value = null
  form.value = defaultForm()
  tieredPricing.value = null
  videoResolutionPrices.value = []
  videoTokenPrices.value = []
  videoDefaultPricePerSecond.value = undefined
  videoDefaultPricePerSecondWithInput.value = undefined
  videoDefaultTokenPrice.value = {
    input_price_per_1m: undefined,
    output_price_per_1m: undefined,
  }
  videoDefaultTokenPriceWithInput.value = {
    input_price_per_1m: undefined,
    output_price_per_1m: undefined,
  }
  videoBillingMode.value = 'per_second'
  billingMode.value = 'token'
}

// Logo 加载失败处理
function handleLogoError(event: Event) {
  const img = event.target as HTMLImageElement
  img.style.display = 'none'
}

// 重置表单
function resetForm() {
  resetOnlinePricingSourceSelection()
  imageGenerationExplicitOverride.value = null
  editingOnlinePricingSource.value = null
  form.value = defaultForm()
  tieredPricing.value = null
  videoResolutionPrices.value = []
  videoTokenPrices.value = []
  videoDefaultPricePerSecond.value = undefined
  videoDefaultPricePerSecondWithInput.value = undefined
  videoDefaultTokenPrice.value = {
    input_price_per_1m: undefined,
    output_price_per_1m: undefined,
  }
  videoDefaultTokenPriceWithInput.value = {
    input_price_per_1m: undefined,
    output_price_per_1m: undefined,
  }
  videoBillingMode.value = 'per_second'
  searchQuery.value = ''
  selectedModel.value = null
  expandedProvider.value = null
  presetPanelCollapsed.value = false
  billingMode.value = 'token'
}

function populateFormFromGlobalModel(model: GlobalModelResponse) {
  imageGenerationExplicitOverride.value = null
  const modelTieredPricing = model.default_tiered_pricing
    ? cloneTieredPricingConfig(model.default_tiered_pricing)
    : null
  const supportedCapabilities = new Set(model.supported_capabilities || [])
  if (tieredPricingHasImageOutputPricing(modelTieredPricing)) {
    supportedCapabilities.add('image_generation')
  }

  form.value = {
    name: model.name,
    display_name: model.display_name,
    default_price_per_request: model.default_price_per_request,
    supported_capabilities: [...supportedCapabilities],
    config: model.config ? { ...model.config } : { streaming: true },
    is_active: model.is_active,
  }
  tieredPricing.value = modelTieredPricing
  loadVideoPricingFromConfig()
  billingMode.value = 'token'
}

// 加载模型数据（编辑模式）
function loadModelData() {
  if (!props.model) return
  resetOnlinePricingSourceSelection()
  if (
    editingOnlinePricingSource.value
    && normalizeModelId(editingOnlinePricingSource.value.model_id) !== normalizeModelId(props.model.name)
  ) {
    editingOnlinePricingSource.value = null
  }
  // 先重置创建模式的残留状态
  selectedModel.value = null
  searchQuery.value = ''
  expandedProvider.value = null
  presetPanelCollapsed.value = false
  populateFormFromGlobalModel(props.model)
}

// 使用 useFormDialog 统一处理对话框逻辑
const { isEditMode, handleDialogUpdate, handleCancel } = useFormDialog({
  isOpen: () => props.open,
  entity: () => props.model,
  isLoading: submitting,
  onClose: () => emit('update:open', false),
  loadData: loadModelData,
  resetForm,
  extraLoadingStates: [syncingOnlinePricing],
})

const autoFillMissingCachePrices = computed(() => (
  !isEditMode.value && selectedModel.value === null
))

async function handleSubmit() {
  if (syncingOnlinePricing.value) return
  if (!form.value.name || !form.value.display_name) {
    showError('请填写模型ID和名称')
    return
  }

  const pricingValidationError = tieredPricingEditorRef.value?.getValidationError()
  if (pricingValidationError) {
    showError(pricingValidationError, '价格配置错误')
    return
  }

  const finalTieredPricing = tieredPricingEditorRef.value?.getFinalPricing() ?? tieredPricing.value

  if (!finalTieredPricing?.tiers?.length) {
    showError('请配置至少一个价格阶梯')
    return
  }

  // Apply billing (video) pricing into config before cleaning/submitting.
  applyVideoPricingToConfig()

  // Auto-infer supported_capabilities from tiered pricing config
  const caps = new Set(form.value.supported_capabilities || [])
  if (tieredPricingHasImageOutputPricing(finalTieredPricing)) {
    caps.add('image_generation')
  }
  form.value.supported_capabilities = caps.size > 0 ? [...caps] : []

  submitting.value = true
  try {
    if (isEditMode.value && props.model) {
      const updateData = buildGlobalModelUpdatePayload(form.value, finalTieredPricing)
      await updateGlobalModel(props.model.id, updateData)
      success('模型更新成功')
    } else {
      const createData = buildGlobalModelCreatePayload(form.value, finalTieredPricing)
      const createdModel = await createGlobalModel(createData)
      existingModelsCache.value.unshift(createdModel)
      if (selectedModel.value) {
        setSource(createdModel.id, {
          provider_id: selectedModel.value.providerId,
          provider_name: selectedModel.value.providerName,
        })
      }
      success('模型创建成功')
      clearSelection()
      emit('success')
      return
    }
    emit('update:open', false)
    emit('success')
  } catch (err: unknown) {
    showError(parseApiError(err, isEditMode.value ? '更新失败' : '创建失败'), isEditMode.value ? '更新失败' : '创建失败')
  } finally {
    submitting.value = false
  }
}

</script>
