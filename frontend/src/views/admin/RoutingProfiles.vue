<template>
  <PageContainer>
    <section
      v-if="!isDetailView"
      class="mt-6"
    >
      <TableCard class="overflow-hidden">
        <template #header>
          <div class="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
            <div>
              <h2 class="text-sm font-semibold">
                策略分组
              </h2>
              <p class="mt-1 text-xs text-muted-foreground">
                共 {{ groups.length }} 个
              </p>
            </div>
            <Button
              variant="ghost"
              size="icon"
              class="h-8 w-8"
              :disabled="loading"
              aria-label="新建策略"
              title="新建策略"
              @click="goToCreate"
            >
              <Plus class="h-4 w-4" />
            </Button>
          </div>
        </template>
        <div>
          <Table class="hidden lg:table">
            <TableHeader>
              <TableRow>
                <TableHead class="w-[28%]">
                  策略分组
                </TableHead>
                <TableHead class="w-[120px]">
                  状态
                </TableHead>
                <TableHead class="w-[120px]">
                  维度
                </TableHead>
                <TableHead class="w-[140px]">
                  模型范围
                </TableHead>
                <TableHead class="w-[140px]">
                  默认策略
                </TableHead>
                <TableHead class="w-[180px]">
                  更新时间
                </TableHead>
                <TableHead class="w-[80px] text-right">
                  操作
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              <TableRow v-if="loading">
                <TableCell
                  colspan="7"
                  class="py-10 text-center text-sm text-muted-foreground"
                >
                  正在加载调度策略
                </TableCell>
              </TableRow>
              <TableRow v-else-if="groups.length === 0">
                <TableCell
                  colspan="7"
                  class="py-10 text-center text-sm text-muted-foreground"
                >
                  暂无调度策略，可以先创建一个默认分组
                </TableCell>
              </TableRow>
              <TableRow
                v-for="group in groups"
                v-else
                :key="group.id"
                class="cursor-pointer hover:bg-muted/50"
                @click="openGroup(group)"
              >
                <TableCell>
                  <div class="min-w-0">
                    <div class="flex items-center gap-2">
                      <span class="truncate font-medium">{{ group.name }}</span>
                      <Badge
                        v-if="group.is_system_default"
                        variant="secondary"
                        class="shrink-0"
                      >
                        系统默认
                      </Badge>
                    </div>
                    <p class="mt-1 line-clamp-1 text-xs text-muted-foreground">
                      {{ group.description || '未填写描述' }}
                    </p>
                  </div>
                </TableCell>
                <TableCell>
                  <Badge :variant="group.enabled ? 'default' : 'secondary'">
                    {{ group.enabled ? '启用' : '停用' }}
                  </Badge>
                </TableCell>
                <TableCell>
                  {{ groupSortingScopeLabel(group) }}
                </TableCell>
                <TableCell>
                  {{ groupModelScopeLabel(group) }}
                </TableCell>
                <TableCell>
                  {{ groupSchedulingSummary(group) }}
                </TableCell>
                <TableCell class="text-muted-foreground">
                  {{ formatUnixSeconds(group.updated_at) }}
                </TableCell>
                <TableCell class="text-right">
                  <Button
                    variant="ghost"
                    size="icon"
                    class="h-8 w-8"
                    title="配置策略"
                    @click.stop="openGroup(group)"
                  >
                    <ChevronRight class="h-4 w-4" />
                  </Button>
                </TableCell>
              </TableRow>
            </TableBody>
          </Table>

          <div
            v-if="loading"
            class="py-10 text-center text-sm text-muted-foreground lg:hidden"
          >
            正在加载调度策略
          </div>
          <div
            v-else-if="groups.length === 0"
            class="px-4 py-10 text-center text-sm text-muted-foreground lg:hidden"
          >
            暂无调度策略，可以先创建一个默认分组
          </div>
          <div
            v-else
            class="divide-y divide-border/40 lg:hidden"
          >
            <button
              v-for="group in groups"
              :key="group.id"
              type="button"
              class="flex w-full items-start justify-between gap-3 px-4 py-3 text-left transition-colors hover:bg-muted/50"
              @click="openGroup(group)"
            >
              <div class="min-w-0 flex-1">
                <div class="flex flex-wrap items-center gap-2">
                  <span class="truncate text-sm font-medium">{{ group.name }}</span>
                  <Badge :variant="group.enabled ? 'default' : 'secondary'">
                    {{ group.enabled ? '启用' : '停用' }}
                  </Badge>
                  <Badge
                    v-if="group.is_system_default"
                    variant="secondary"
                  >
                    系统默认
                  </Badge>
                </div>
                <p class="mt-1 line-clamp-2 text-xs text-muted-foreground">
                  {{ group.description || '未填写描述' }}
                </p>
                <div class="mt-2 flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground">
                  <span>{{ groupSortingScopeLabel(group) }}</span>
                  <span>{{ groupModelScopeLabel(group) }}</span>
                  <span>{{ groupSchedulingSummary(group) }}</span>
                </div>
              </div>
              <ChevronRight class="mt-1 h-4 w-4 shrink-0 text-muted-foreground" />
            </button>
          </div>
        </div>
      </TableCard>
    </section>

    <section
      v-else
      class="mt-6"
    >
      <Card
        v-if="draft"
        class="overflow-hidden"
        :inert="saving"
        :aria-busy="saving"
      >
        <div class="border-b border-border/60 px-5 py-4">
          <div class="flex flex-col gap-3 lg:flex-row lg:items-start lg:justify-between">
            <div>
              <div class="flex flex-wrap items-center gap-2">
                <h2 class="text-base font-semibold">
                  {{ isCreating ? '新建调度策略' : draft.name || '未命名策略' }}
                </h2>
                <Badge
                  v-if="draft.is_system_default"
                  variant="secondary"
                >
                  系统默认
                </Badge>
                <Badge :variant="draft.enabled ? 'default' : 'secondary'">
                  {{ draft.enabled ? '启用' : '停用' }}
                </Badge>
              </div>
              <p class="mt-1 text-xs text-muted-foreground">
                更新时间 {{ formatUnixSeconds(draft.updated_at) }}
              </p>
            </div>
            <div class="flex flex-wrap items-center gap-2">
              <Button
                variant="ghost"
                size="icon"
                class="h-8 w-8"
                :class="draft.is_system_default
                  ? 'text-primary hover:text-primary'
                  : 'text-muted-foreground/70 hover:text-foreground'"
                :aria-label="draft.is_system_default ? '系统默认' : '设为系统默认'"
                :title="draft.is_system_default ? '系统默认' : '设为系统默认'"
                @click="draft.is_system_default = !draft.is_system_default"
              >
                <Star class="h-4 w-4" />
              </Button>
              <Button
                variant="ghost"
                size="icon"
                class="h-8 w-8 text-muted-foreground/70 hover:text-foreground"
                :disabled="!canSaveDraft"
                aria-label="保存"
                title="保存"
                @click="saveDraft"
              >
                <Save
                  class="h-4 w-4"
                  :class="{ 'animate-pulse': saving }"
                />
              </Button>
              <Button
                v-if="!isCreating"
                variant="ghost"
                size="icon"
                class="h-8 w-8 text-muted-foreground/70 hover:text-destructive"
                :disabled="deleting"
                aria-label="删除"
                title="删除"
                @click="deleteDraft"
              >
                <Trash2 class="h-4 w-4" />
              </Button>
            </div>
          </div>
        </div>

        <div class="space-y-6 p-5">
          <div class="grid gap-3 lg:grid-cols-[minmax(0,0.9fr)_minmax(0,1.2fr)_180px_320px]">
            <label class="space-y-1 text-sm">
              <span class="text-muted-foreground">名称</span>
              <Input
                v-model="draft.name"
                placeholder="新调度策略"
              />
            </label>
            <label class="space-y-1 text-sm">
              <span class="text-muted-foreground">描述</span>
              <Input
                v-model="draft.description"
                placeholder="例如：默认策略 / 高推理策略 / 号池优先策略"
              />
            </label>
            <div class="space-y-1 text-sm">
              <span class="text-muted-foreground">状态</span>
              <div class="grid grid-cols-2 gap-1 rounded-lg bg-muted/40 p-1">
                <button
                  type="button"
                  class="h-9 rounded-md px-3 text-sm font-medium transition-colors"
                  :class="draft.enabled
                    ? 'bg-primary/10 text-primary shadow-sm ring-1 ring-border'
                    : 'text-muted-foreground hover:bg-background/60 hover:text-foreground'"
                  @click="setDraftEnabled(true)"
                >
                  启用
                </button>
                <button
                  type="button"
                  class="h-9 rounded-md px-3 text-sm font-medium transition-colors"
                  :class="!draft.enabled
                    ? 'bg-primary/10 text-primary shadow-sm ring-1 ring-border'
                    : 'text-muted-foreground hover:bg-background/60 hover:text-foreground'"
                  @click="setDraftEnabled(false)"
                >
                  停用
                </button>
              </div>
            </div>
            <div class="space-y-1 text-sm">
              <span class="text-muted-foreground">
                维度
              </span>
              <div class="grid grid-cols-2 gap-1 rounded-lg bg-muted/40 p-1">
                <button
                  type="button"
                  class="h-9 rounded-md px-3 text-sm font-medium transition-colors"
                  :class="sortingScope === 'unified'
                    ? 'bg-primary/10 text-primary shadow-sm ring-1 ring-border'
                    : 'text-muted-foreground hover:bg-background/60 hover:text-foreground'"
                  @click="setSortingScope('unified')"
                >
                  统一调度
                </button>
                <button
                  type="button"
                  class="h-9 rounded-md px-3 text-sm font-medium transition-colors"
                  :class="sortingScope === 'per_model'
                    ? 'bg-primary/10 text-primary shadow-sm ring-1 ring-border'
                    : 'text-muted-foreground hover:bg-background/60 hover:text-foreground'"
                  @click="setSortingScope('per_model')"
                >
                  区分模型
                </button>
              </div>
            </div>
          </div>

          <section
            class="space-y-3 rounded-lg border border-border/60 p-4"
            aria-labelledby="model-allowlist-heading"
          >
            <div class="flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between">
              <div>
                <div class="flex flex-wrap items-center gap-2">
                  <h3
                    id="model-allowlist-heading"
                    class="text-sm font-medium"
                  >
                    模型白名单
                  </h3>
                  <Badge :variant="routingModelScopeLabel(draft.config_json) === '全部模型' ? 'secondary' : 'outline'">
                    {{ routingModelScopeLabel(draft.config_json) }}
                  </Badge>
                </div>
                <p class="mt-1 text-xs text-muted-foreground">
                  控制此策略分组适用于哪些模型；留空表示全部模型。它与“区分模型”中的专属调度覆盖相互独立，支持精确值、* 和前缀通配符（如 gpt-*），每行填写一个值。
                </p>
              </div>
              <Button
                v-if="routingModelScopeLabel(draft.config_json) !== '全部模型'"
                type="button"
                variant="ghost"
                size="sm"
                class="shrink-0 text-muted-foreground hover:text-foreground"
                :disabled="saving"
                data-testid="clear-allowed-models"
                @click="clearAllowedModelScope"
              >
                改为全部模型
              </Button>
            </div>

            <Textarea
              :model-value="allowedModelsInput"
              class="min-h-[96px] font-mono"
              :disabled="saving"
              data-testid="allowed-models-input"
              aria-label="模型白名单"
              placeholder="留空表示全部模型，每行一个模型"
              @update:model-value="updateAllowedModelScope"
            />

            <div
              v-if="draft.config_json.allowed_models.length"
              class="flex flex-wrap gap-2"
              data-testid="allowed-model-values"
            >
              <Badge
                v-for="(model, index) in draft.config_json.allowed_models"
                :key="`${model}:${index}`"
                variant="outline"
                class="font-mono font-normal"
              >
                {{ model }}
              </Badge>
            </div>

            <div
              v-if="allowedModelsLookLikeLegacyMirror"
              class="rounded-md border border-amber-500/30 bg-amber-500/5 px-3 py-2 text-xs text-muted-foreground"
              data-testid="allowed-models-legacy-mirror"
            >
              当前白名单与按模型策略列表一致，可能来自旧版界面的联动保存。现有范围会原样保留；如需让其他模型也使用默认策略，请显式点击“改为全部模型”。
            </div>
          </section>

          <section
            v-if="sortingScope === 'unified'"
            class="space-y-4"
          >
            <div class="space-y-3 rounded-lg border border-border/60 p-4">
              <div>
                <h3 class="text-sm font-medium">
                  优先级模式与调度策略
                </h3>
                <p class="mt-1 text-xs text-muted-foreground">
                  统一作用于策略范围内的全部模型。
                </p>
              </div>
              <div class="grid grid-cols-2 gap-3">
                <div class="space-y-1 text-sm">
                  <span class="text-muted-foreground">优先级模式</span>
                  <div class="grid grid-cols-2 gap-1 rounded-lg bg-muted/40 p-1">
                    <button
                      type="button"
                      class="flex h-9 items-center justify-center gap-2 rounded-md px-3 text-sm font-medium transition-colors"
                      :class="firstStepPriorityMode === 'provider'
                        ? 'bg-primary/10 text-primary shadow-sm ring-1 ring-border'
                        : 'text-muted-foreground hover:bg-background/60 hover:text-foreground'"
                      @click="updateFirstStepPriorityMode('provider')"
                    >
                      <Layers class="h-4 w-4" />
                      Provider
                    </button>
                    <button
                      type="button"
                      class="flex h-9 items-center justify-center gap-2 rounded-md px-3 text-sm font-medium transition-colors"
                      :class="firstStepPriorityMode === 'global_key'
                        ? 'bg-primary/10 text-primary shadow-sm ring-1 ring-border'
                        : 'text-muted-foreground hover:bg-background/60 hover:text-foreground'"
                      @click="updateFirstStepPriorityMode('global_key')"
                    >
                      <Key class="h-4 w-4" />
                      Key
                    </button>
                  </div>
                </div>

                <div class="space-y-1 text-sm">
                  <span class="text-muted-foreground">调度策略</span>
                  <div class="grid grid-cols-3 gap-1 rounded-lg bg-muted/40 p-1">
                    <button
                      v-for="mode in schedulingModes"
                      :key="mode.value"
                      type="button"
                      class="h-9 rounded-md px-3 text-sm font-medium transition-colors"
                      :class="firstStepSchedulingMode === mode.value
                        ? 'bg-primary/10 text-primary shadow-sm ring-1 ring-border'
                        : 'text-muted-foreground hover:bg-background/60 hover:text-foreground'"
                      @click="updateFirstStepSchedulingMode(mode.value)"
                    >
                      {{ mode.label }}
                    </button>
                  </div>
                </div>
              </div>
            </div>

            <RoutingPriorityPolicyEditor
              :config="draft.config_json"
              :model="DEFAULT_ROUTING_POLICY_MODEL"
              :show-priority-mode="false"
              :show-scheduling-mode="false"
              subtitle="统一作用于当前策略范围内的所有模型"
              @update:config="updateDraftConfig"
            />
          </section>

          <section v-else>
            <div class="flex max-h-[560px] flex-col gap-3 overflow-hidden rounded-lg border border-border/60 p-3">
              <div class="grid grid-cols-2 gap-3">
                <Input
                  v-model="globalModelSearch"
                  placeholder="搜索模型"
                  class="w-full"
                />
                <div class="grid grid-cols-2 gap-1 rounded-lg bg-muted/40 p-1 text-xs">
                  <button
                    v-for="filter in modelFilters"
                    :key="filter.value"
                    type="button"
                    class="h-9 rounded-md px-3 font-medium transition-colors"
                    :class="modelFilter === filter.value
                      ? 'bg-primary/10 text-primary shadow-sm ring-1 ring-border'
                      : 'text-muted-foreground hover:bg-background/60 hover:text-foreground'"
                    @click="modelFilter = filter.value"
                  >
                    {{ filter.label }}
                  </button>
                </div>
              </div>

              <div
                v-if="loadingGlobalModels"
                class="rounded-md border border-dashed border-border/70 px-3 py-6 text-center text-xs text-muted-foreground"
              >
                正在加载模型
              </div>
              <div
                v-else-if="globalModelsError"
                class="rounded-md border border-destructive/30 bg-destructive/5 px-3 py-2 text-xs text-destructive"
              >
                {{ globalModelsError }}
              </div>
              <div
                v-else-if="modelRows.length === 0"
                class="rounded-md border border-dashed border-border/70 px-3 py-6 text-center text-xs text-muted-foreground"
              >
                {{ globalModelSearch.trim() ? '未匹配到模型' : modelFilter === 'configured' ? '暂无已配置模型' : '暂无未配置模型' }}
              </div>
              <div
                v-else
                class="min-h-0 flex-1 space-y-2 overflow-y-auto pr-1"
              >
                <div
                  v-for="row in modelRows"
                  :key="row.name"
                  class="rounded-lg border transition-colors"
                  :class="selectedPerModelName === row.name
                    ? 'border-primary/50 bg-primary/5'
                    : 'border-border/60'"
                >
                  <div class="flex w-full items-center gap-3 px-4 py-3">
                    <button
                      type="button"
                      class="flex min-w-0 flex-1 items-center gap-3 text-left text-sm"
                      @click="selectGlobalModel(row.name)"
                    >
                      <span
                        v-if="row.configured"
                        class="h-2 w-2 shrink-0 rounded-full bg-primary"
                        aria-hidden="true"
                      />
                      <Plus
                        v-else
                        class="h-3.5 w-3.5 shrink-0 text-muted-foreground"
                        aria-hidden="true"
                      />
                      <span class="min-w-0 flex-1">
                        <span class="block truncate font-medium">{{ row.displayName }}</span>
                        <span class="block truncate text-xs text-muted-foreground">{{ row.name }}</span>
                      </span>
                    </button>
                    <template v-if="selectedPerModelName === row.name && activePerModelPolicy">
                      <DropdownMenu>
                        <DropdownMenuTrigger as-child>
                          <Button
                            type="button"
                            variant="ghost"
                            size="icon"
                            class="h-8 w-8 shrink-0 text-muted-foreground/70 hover:text-foreground"
                            :disabled="copySourceCandidates.length === 0"
                            title="加载其他模型配置"
                          >
                            <Copy class="h-4 w-4" />
                          </Button>
                        </DropdownMenuTrigger>
                        <DropdownMenuContent
                          align="end"
                          class="max-h-[320px] overflow-y-auto"
                        >
                          <DropdownMenuItem
                            v-for="source in copySourceCandidates"
                            :key="source.model"
                            @select="copyModelConfig(source.model)"
                          >
                            <span class="min-w-0">
                              <span class="block truncate text-sm font-medium">{{ source.label }}</span>
                              <span class="block truncate text-xs text-muted-foreground">{{ source.model }}</span>
                            </span>
                          </DropdownMenuItem>
                        </DropdownMenuContent>
                      </DropdownMenu>
                      <Button
                        type="button"
                        variant="ghost"
                        size="icon"
                        class="h-8 w-8 shrink-0 text-muted-foreground/70 hover:text-foreground"
                        :disabled="!canSaveCurrentModel"
                        title="保存到草稿"
                        @click="saveCurrentModel"
                      >
                        <Save class="h-4 w-4" />
                      </Button>
                      <Button
                        v-if="hasModelPolicy(activePerModelPolicy.model)"
                        type="button"
                        variant="ghost"
                        size="icon"
                        class="h-8 w-8 shrink-0"
                        :class="canRemoveCurrentModel ? 'text-muted-foreground/70 hover:text-destructive' : 'text-muted-foreground/30'"
                        :disabled="!canRemoveCurrentModel"
                        :title="canRemoveCurrentModel ? '移除当前模型排序' : '当前有未保存改动，不能移除'"
                        @click="removePerModelPolicy(activePerModelPolicy.model)"
                      >
                        <Trash2 class="h-4 w-4" />
                      </Button>
                    </template>
                    <button
                      type="button"
                      class="shrink-0"
                      @click="selectGlobalModel(row.name)"
                    >
                      <ChevronDown
                        class="h-4 w-4 text-muted-foreground transition-transform"
                        :class="selectedPerModelName === row.name ? 'rotate-180' : ''"
                      />
                    </button>
                  </div>

                  <div
                    v-if="selectedPerModelName === row.name && activePerModelPolicy"
                    class="border-t border-border/60 p-4"
                  >
                    <div class="mb-4 space-y-3 rounded-lg border border-border/60 p-4">
                      <h3 class="text-sm font-medium">
                        优先级模式与调度策略
                      </h3>
                      <div class="grid gap-3 lg:grid-cols-2">
                        <div class="space-y-1 text-sm">
                          <span class="text-muted-foreground">优先级模式</span>
                          <div class="grid grid-cols-2 gap-1 rounded-lg bg-muted/40 p-1">
                            <button
                              type="button"
                              class="flex h-9 items-center justify-center gap-2 rounded-md px-3 text-sm font-medium transition-colors"
                              :class="modelPriorityMode(activePerModelPolicy.model) === 'provider'
                                ? 'bg-primary/10 text-primary shadow-sm ring-1 ring-border'
                                : 'text-muted-foreground hover:bg-background/60 hover:text-foreground'"
                              @click="updateModelPriorityMode(activePerModelPolicy.model, 'provider')"
                            >
                              <Layers class="h-4 w-4" />
                              Provider
                            </button>
                            <button
                              type="button"
                              class="flex h-9 items-center justify-center gap-2 rounded-md px-3 text-sm font-medium transition-colors"
                              :class="modelPriorityMode(activePerModelPolicy.model) === 'global_key'
                                ? 'bg-primary/10 text-primary shadow-sm ring-1 ring-border'
                                : 'text-muted-foreground hover:bg-background/60 hover:text-foreground'"
                              @click="updateModelPriorityMode(activePerModelPolicy.model, 'global_key')"
                            >
                              <Key class="h-4 w-4" />
                              Key
                            </button>
                          </div>
                        </div>
                        <div class="space-y-1 text-sm">
                          <span class="text-muted-foreground">调度策略</span>
                          <div class="grid grid-cols-3 gap-1 rounded-lg bg-muted/40 p-1">
                            <button
                              v-for="mode in schedulingModes"
                              :key="mode.value"
                              type="button"
                              class="h-9 rounded-md px-3 text-sm font-medium transition-colors"
                              :class="modelSchedulingMode(activePerModelPolicy.model) === mode.value
                                ? 'bg-primary/10 text-primary shadow-sm ring-1 ring-border'
                                : 'text-muted-foreground hover:bg-background/60 hover:text-foreground'"
                              @click="updateModelSchedulingMode(activePerModelPolicy.model, mode.value)"
                            >
                              {{ mode.label }}
                            </button>
                          </div>
                        </div>
                      </div>
                    </div>

                    <RoutingPriorityPolicyEditor
                      :config="activeConfigForReading"
                      :model="activePerModelPolicy.model"
                      :priority-mode="modelPriorityMode(activePerModelPolicy.model)"
                      :scheduling-mode="modelSchedulingMode(activePerModelPolicy.model)"
                      :show-priority-mode="false"
                      :show-scheduling-mode="false"
                      :subtitle="`仅作用于 ${activePerModelPolicy.model}`"
                      @update:config="updateEditingConfig"
                      @update:priority-mode="mode => updateModelPriorityMode(activePerModelPolicy.model, mode)"
                      @update:scheduling-mode="mode => updateModelSchedulingMode(activePerModelPolicy.model, mode)"
                    />
                  </div>
                </div>
              </div>
            </div>
          </section>
        </div>
      </Card>

      <Card
        v-else
        class="flex min-h-[360px] items-center justify-center p-8 text-center"
      >
        <div>
          <SlidersHorizontal class="mx-auto h-8 w-8 text-muted-foreground" />
          <p class="mt-3 text-sm font-medium">
            {{ loading ? '正在加载调度策略' : '未找到调度策略' }}
          </p>
          <Button
            v-if="!loading"
            variant="outline"
            class="mt-4"
            @click="goToList"
          >
            返回分组
          </Button>
        </div>
      </Card>
    </section>

    <AlertDialog
      v-model="switchModelDialogOpen"
      type="warning"
      title="切换模型"
      description="当前模型有未保存的改动，切换将丢弃这些改动，是否继续？"
      confirm-text="继续"
      @confirm="confirmSwitchModel"
      @cancel="cancelSwitchModel"
    />

    <AlertDialog
      v-model="deleteDialogOpen"
      type="destructive"
      title="删除调度策略"
      :description="`确认删除调度策略「${draft?.name ?? ''}」？此操作无法撤销。`"
      confirm-text="删除"
      :loading="deleting"
      @confirm="confirmDeleteDraft"
    />
  </PageContainer>
</template>

<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import {
  ChevronDown,
  ChevronRight,
  Copy,
  Key,
  Layers,
  Plus,
  Save,
  SlidersHorizontal,
  Star,
  Trash2,
} from 'lucide-vue-next'

import { PageContainer } from '@/components/layout'
import {
  Badge,
  Button,
  Card,
  Input,
  Table,
  TableBody,
  TableCard,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  Textarea,
} from '@/components/ui'
import { DropdownMenu, DropdownMenuTrigger, DropdownMenuContent, DropdownMenuItem } from '@/components/ui/dropdown-menu'
import { AlertDialog } from '@/components/common'
import {
  DEFAULT_ROUTING_POLICY_MODEL,
  allowedModelsMirrorPerModelPolicies,
  clearAllowedModels,
  copyPerModelRoutingConfig,
  createEmptyModelPolicy,
  createEmptyRoutingGroupConfig,
  formatAllowedModelsInput,
  getModelScheduling,
  isGeneratedModelSchedulingRule,
  modelSchedulingRuleId,
  normalizeRoutingGroupConfig,
  removePerModelRoutingConfig,
  routingModelScopeLabel,
  savePerModelRoutingConfig,
  setRoutingSortingScope,
  updateAllowedModelsFromInput,
  upsertModelSchedulingRule,
  type RoutingGroupConfig,
  type RoutingPriorityMode,
  type RoutingSchedulingMode,
  type RoutingSortingScope,
} from '@/features/routing/utils/routingPolicy'
import { RoutingPriorityPolicyEditor } from '@/features/routing/components'
import {
  createRoutingGroup,
  deleteRoutingGroup,
  listRoutingGroups,
  updateRoutingGroup,
  type RoutingGroupRecord,
} from '@/api/routing-profiles'
import { getGlobalModels, type GlobalModelResponse } from '@/api/global-models'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'
import { log } from '@/utils/logger'

interface RoutingGroupDraft {
  id?: string
  name: string
  description: string
  enabled: boolean
  is_system_default: boolean
  config_json: RoutingGroupConfig
  version: number
  updated_at?: number | null
}

type ModelFilter = 'configured' | 'unconfigured'

const modelFilters: Array<{ value: ModelFilter; label: string }> = [
  { value: 'unconfigured', label: '未配置' },
  { value: 'configured', label: '已配置' },
]

const { success, error: showError } = useToast()
const route = useRoute()
const router = useRouter()

const schedulingModes: Array<{ value: RoutingSchedulingMode; label: string }> = [
  { value: 'cache_affinity', label: '缓存亲和' },
  { value: 'load_balance', label: '负载均衡' },
  { value: 'fixed_order', label: '固定顺序' },
]

const groups = ref<RoutingGroupRecord[]>([])
const selectedGroupId = ref<string | null>(null)
const draft = ref<RoutingGroupDraft | null>(null)
const savedDraftSnapshot = ref<string | null>(null)
const sortingScope = ref<RoutingSortingScope>('unified')
const selectedPerModelName = ref<string | null>(null)
const editingConfig = ref<RoutingGroupConfig | null>(null)
const allowedModelsInput = ref('')
const globalModelSearch = ref('')
const modelFilter = ref<ModelFilter>('unconfigured')
const globalModels = ref<GlobalModelResponse[]>([])
const loadingGlobalModels = ref(false)
const globalModelsError = ref<string | null>(null)

const loading = ref(false)
const saving = ref(false)
const deleting = ref(false)
const isCreating = ref(false)
let draftGeneration = 0

const switchModelTarget = ref<string | null>(null)
const switchModelDialogOpen = ref(false)
const deleteDialogOpen = ref(false)

const isCreateRoute = computed(() => route.name === 'RoutingProfileCreate')
const routeGroupId = computed(() => paramToString(route.params.groupId))
const isDetailView = computed(() => isCreateRoute.value || route.name === 'RoutingProfileDetail')
const perModelPolicies = computed(() => {
  return draft.value?.config_json.model_policies
    .filter(policy => policy.model !== DEFAULT_ROUTING_POLICY_MODEL)
    ?? []
})
const activePerModelPolicy = computed(() => {
  if (!selectedPerModelName.value) return null
  const existing = perModelPolicies.value.find(policy => policy.model === selectedPerModelName.value)
  if (existing) return existing
  return createEmptyModelPolicy(selectedPerModelName.value)
})
const firstStepPriorityMode = computed<RoutingPriorityMode>(() => {
  if (sortingScope.value === 'per_model' && activePerModelPolicy.value) {
    return modelPriorityMode(activePerModelPolicy.value.model)
  }
  return draft.value?.config_json.default_policy.priority_mode ?? 'provider'
})
const firstStepSchedulingMode = computed<RoutingSchedulingMode>(() => {
  if (sortingScope.value === 'per_model' && activePerModelPolicy.value) {
    return modelSchedulingMode(activePerModelPolicy.value.model)
  }
  return draft.value?.config_json.default_policy.scheduling_mode ?? 'cache_affinity'
})
const allowedModelsLookLikeLegacyMirror = computed(() => {
  return draft.value
    ? allowedModelsMirrorPerModelPolicies(draft.value.config_json)
    : false
})

interface ModelRow {
  name: string
  displayName: string
  configured: boolean
}

const modelRows = computed<ModelRow[]>(() => {
  const query = globalModelSearch.value.trim().toLowerCase()
  const seen = new Set<string>()
  const rows: ModelRow[] = []

  for (const policy of perModelPolicies.value) {
    const name = policy.model
    const found = globalModels.value.find(item => item.name === name)
    rows.push({
      name,
      displayName: found?.display_name || name,
      configured: true,
    })
    seen.add(name)
  }

  for (const model of globalModels.value) {
    if (seen.has(model.name)) continue
    rows.push({
      name: model.name,
      displayName: model.display_name || model.name,
      configured: false,
    })
  }

  return rows
    .filter(row => {
      if (modelFilter.value === 'configured' && !row.configured) return false
      if (modelFilter.value === 'unconfigured' && row.configured) return false
      if (!query) return true
      return (
        row.name.toLowerCase().includes(query)
        || row.displayName.toLowerCase().includes(query)
      )
    })
    .sort((left, right) => {
      if (left.configured !== right.configured) {
        return left.configured ? -1 : 1
      }
      return left.name.localeCompare(right.name)
    })
})

function normalizeRecord(group: RoutingGroupRecord): RoutingGroupRecord {
  return {
    ...group,
    config_json: normalizeRoutingGroupConfig(group.config_json),
  }
}

function cloneConfig(config: RoutingGroupConfig): RoutingGroupConfig {
  return normalizeRoutingGroupConfig(JSON.parse(JSON.stringify(config)) as Partial<RoutingGroupConfig>)
}

function draftSnapshotValue(value: RoutingGroupDraft): string {
  return JSON.stringify({
    name: value.name.trim(),
    description: value.description.trim() || null,
    enabled: value.enabled,
    is_system_default: value.is_system_default,
    config_json: cloneConfig(value.config_json),
  })
}

function buildDraft(group: RoutingGroupRecord): RoutingGroupDraft {
  return {
    id: group.id,
    name: group.name,
    description: group.description ?? '',
    enabled: group.enabled,
    is_system_default: group.is_system_default,
    config_json: cloneConfig(group.config_json),
    version: group.version,
    updated_at: group.updated_at,
  }
}

function paramToString(value: unknown): string | null {
  if (Array.isArray(value)) return value[0] ?? null
  return typeof value === 'string' ? value : null
}

function clearDraftState(): void {
  draftGeneration += 1
  isCreating.value = false
  selectedGroupId.value = null
  draft.value = null
  savedDraftSnapshot.value = null
  selectedPerModelName.value = null
  editingConfig.value = null
  allowedModelsInput.value = ''
  switchModelTarget.value = null
  switchModelDialogOpen.value = false
  deleteDialogOpen.value = false
}

function selectGroup(group: RoutingGroupRecord): void {
  const normalized = normalizeRecord(group)
  draftGeneration += 1
  isCreating.value = false
  selectedGroupId.value = normalized.id
  draft.value = buildDraft(normalized)
  allowedModelsInput.value = formatAllowedModelsInput(draft.value.config_json.allowed_models)
  savedDraftSnapshot.value = draftSnapshotValue(draft.value)
  syncEditorStateFromConfig(draft.value.config_json)
  resetEditingConfig()
}

function setDraftEnabled(value: boolean): void {
  if (!draft.value) return
  draft.value.enabled = value
}

function startCreate(): void {
  draftGeneration += 1
  isCreating.value = true
  selectedGroupId.value = null
  draft.value = {
    name: '新调度策略',
    description: '',
    enabled: false,
    is_system_default: groups.value.length === 0,
    config_json: createEmptyRoutingGroupConfig(),
    version: 1,
    updated_at: null,
  }
  savedDraftSnapshot.value = null
  allowedModelsInput.value = ''
  syncEditorStateFromConfig(draft.value.config_json)
  resetEditingConfig()
}

function syncRouteState(): void {
  if (!isDetailView.value) {
    clearDraftState()
    return
  }

  if (isCreateRoute.value) {
    if (!isCreating.value || !draft.value || draft.value.id) {
      startCreate()
    }
    return
  }

  const groupId = routeGroupId.value
  if (!groupId) {
    clearDraftState()
    return
  }

  const group = groups.value.find(item => item.id === groupId)
  if (!group) {
    clearDraftState()
    selectedGroupId.value = groupId
    return
  }

  if (isCreating.value || selectedGroupId.value !== group.id || !draft.value) {
    selectGroup(group)
  }
}

function goToList(): void {
  void router.push({ name: 'RoutingProfiles' })
}

function goToCreate(): void {
  void router.push({ name: 'RoutingProfileCreate' })
}

function openGroup(group: RoutingGroupRecord): void {
  void router.push({ name: 'RoutingProfileDetail', params: { groupId: group.id } })
}

function schedulingModeLabel(mode: RoutingSchedulingMode): string {
  return schedulingModes.find(item => item.value === mode)?.label ?? mode
}

function groupSortingScopeLabel(group: RoutingGroupRecord): string {
  return hasPerModelSorting(normalizeRoutingGroupConfig(group.config_json)) ? '区分模型' : '统一调度'
}

function groupModelScopeLabel(group: RoutingGroupRecord): string {
  return routingModelScopeLabel(group.config_json)
}

function groupSchedulingSummary(group: RoutingGroupRecord): string {
  const config = normalizeRoutingGroupConfig(group.config_json)
  if (hasPerModelSorting(config)) return '按模型配置'
  return schedulingModeLabel(config.default_policy.scheduling_mode)
}

function updateDraftConfig(value: RoutingGroupConfig): void {
  if (!draft.value) return
  draft.value.config_json = normalizeRoutingGroupConfig(value)
  syncSelectedPerModelPolicy()
}

function resetEditingConfig(): void {
  if (!draft.value) {
    editingConfig.value = null
    return
  }
  editingConfig.value = cloneConfig(draft.value.config_json)
}

function updateEditingConfig(value: RoutingGroupConfig): void {
  editingConfig.value = normalizeRoutingGroupConfig(value)
}

const editingDirty = computed(() => {
  if (!editingConfig.value || !draft.value) return false
  return JSON.stringify(editingConfig.value) !== JSON.stringify(draft.value.config_json)
})

const draftDirty = computed(() => {
  if (!draft.value) return false
  if (isCreating.value) return true
  return savedDraftSnapshot.value !== draftSnapshotValue(draft.value)
})

const canSaveDraft = computed(() => {
  const hasPendingCurrentModel = perModelEditingActive.value
    && Boolean(activePerModelPolicy.value)
    && (editingDirty.value || !currentModelPersisted.value)
  return Boolean(draft.value)
    && !saving.value
    && draftDirty.value
    && !hasPendingCurrentModel
    && !(perModelEditingActive.value && perModelPolicies.value.length === 0)
})

const currentModelPersisted = computed(() => {
  const model = activePerModelPolicy.value?.model
  return model ? hasModelPolicy(model) : false
})

const canSaveCurrentModel = computed(() => {
  return Boolean(activePerModelPolicy.value)
    && !saving.value
    && (editingDirty.value || !currentModelPersisted.value)
})

const canRemoveCurrentModel = computed(() => {
  return Boolean(activePerModelPolicy.value)
    && currentModelPersisted.value
    && !saving.value
    && !editingDirty.value
})

function syncEditorStateFromConfig(config: RoutingGroupConfig): void {
  const normalized = normalizeRoutingGroupConfig(config)
  sortingScope.value = hasPerModelSorting(normalized) ? 'per_model' : 'unified'
  syncSelectedPerModelPolicy()
}

function hasPerModelSorting(config: RoutingGroupConfig): boolean {
  return config.model_policies.some(policy => policy.model !== DEFAULT_ROUTING_POLICY_MODEL)
    || config.rules.some(isGeneratedModelSchedulingRule)
}

function setSortingScope(scope: RoutingSortingScope): void {
  if (!draft.value) return
  sortingScope.value = scope
  if (scope === 'unified') {
    const next = setRoutingSortingScope(draft.value.config_json, scope)
    updateDraftConfig(next)
    resetEditingConfig()
    return
  }
  resetEditingConfig()
}

function updateFirstStepPriorityMode(mode: RoutingPriorityMode): void {
  if (!draft.value) return
  if (sortingScope.value === 'per_model' && activePerModelPolicy.value) {
    updateModelPriorityMode(activePerModelPolicy.value.model, mode)
    return
  }
  updateDraftConfig({
    ...draft.value.config_json,
    default_policy: {
      ...draft.value.config_json.default_policy,
      priority_mode: mode,
    },
  })
}

function updateFirstStepSchedulingMode(mode: RoutingSchedulingMode): void {
  if (!draft.value) return
  if (sortingScope.value === 'per_model' && activePerModelPolicy.value) {
    updateModelSchedulingMode(activePerModelPolicy.value.model, mode)
    return
  }
  updateDraftConfig({
    ...draft.value.config_json,
    default_policy: {
      ...draft.value.config_json.default_policy,
      scheduling_mode: mode,
    },
  })
}

function removePerModelPolicy(model: string): void {
  if (!draft.value) return
  if (perModelEditingActive.value && editingDirty.value) {
    showError('请先保存当前改动后再移除模型')
    return
  }
  const next = removePerModelRoutingConfig(draft.value.config_json, model)
  if (selectedPerModelName.value === model) {
    selectedPerModelName.value = null
  }
  modelFilter.value = 'unconfigured'
  updateDraftConfig(next)
  resetEditingConfig()
  // 移除最后一个模型特殊配置后，自动回到统一调度维度：
  // 若仍停留在“区分模型”维度，canSaveDraft 会因
  // perModelPolicies 为空而禁用保存按钮，且保存时会拦截
  // “按模型排序时至少选择一个模型”，导致无法保存删除结果。
  if (perModelPolicies.value.length === 0) {
    setSortingScope('unified')
  }
}

function selectGlobalModel(model: string): void {
  if (!model) return
  if (model === selectedPerModelName.value) {
    resetEditingConfig()
    selectedPerModelName.value = null
    return
  }
  const shouldAddModel = !hasModelPolicy(model)
  if (perModelEditingActive.value && editingDirty.value) {
    switchModelTarget.value = model
    switchModelDialogOpen.value = true
    return
  }
  if (shouldAddModel) {
    resetEditingConfig()
  }
  selectedPerModelName.value = model
}

function confirmSwitchModel(): void {
  const target = switchModelTarget.value
  if (target) {
    resetEditingConfig()
    selectedPerModelName.value = target
  }
  switchModelTarget.value = null
  switchModelDialogOpen.value = false
}

function cancelSwitchModel(): void {
  switchModelTarget.value = null
}

function hasModelPolicy(model: string): boolean {
  if (perModelPolicies.value.some(policy => policy.model === model)) return true
  const ruleId = modelSchedulingRuleId(model)
  return draft.value?.config_json.rules.some(rule => rule.id === ruleId) ?? false
}

const copySourceCandidates = computed(() => {
  if (!draft.value) return []
  const current = selectedPerModelName.value
  return perModelPolicies.value
    .filter(policy => policy.model !== current)
    .map(policy => ({
      model: policy.model,
      label: globalModelLabel(policy.model),
    }))
})

function copyModelConfig(sourceModel: string): void {
  if (!draft.value || !editingConfig.value) return
  const target = selectedPerModelName.value
  if (!target || target === sourceModel) return
  const next = copyPerModelRoutingConfig(
    editingConfig.value,
    draft.value.config_json,
    sourceModel,
    target,
  )
  updateEditingConfig(next)
  success(`已加载 ${globalModelLabel(sourceModel)} 的配置，点击保存生效`)
}

function syncSelectedPerModelPolicy(): void {
  if (selectedPerModelName.value) return
  const firstConfigured = perModelPolicies.value[0]?.model
  selectedPerModelName.value = firstConfigured ?? null
}

const perModelEditingActive = computed(() => sortingScope.value === 'per_model')

const activeConfigForReading = computed<RoutingGroupConfig>(() => {
  if (perModelEditingActive.value && editingConfig.value) return editingConfig.value
  return draft.value?.config_json ?? createEmptyRoutingGroupConfig()
})

function modelPriorityMode(model: string): RoutingPriorityMode {
  return getModelScheduling(activeConfigForReading.value, model).priority_mode
}

function modelSchedulingMode(model: string): RoutingSchedulingMode {
  return getModelScheduling(activeConfigForReading.value, model).scheduling_mode
}

function updateModelPriorityMode(model: string, mode: RoutingPriorityMode): void {
  if (!draft.value) return
  const baseConfig = perModelEditingActive.value && editingConfig.value
    ? editingConfig.value
    : draft.value.config_json
  const current = getModelScheduling(baseConfig, model)
  const next = upsertModelSchedulingRule(baseConfig, model, {
    priority_mode: mode,
    scheduling_mode: current.scheduling_mode,
  })
  if (perModelEditingActive.value) {
    updateEditingConfig(next)
    return
  }
  updateDraftConfig(next)
}

function updateModelSchedulingMode(model: string, mode: RoutingSchedulingMode): void {
  if (!draft.value) return
  const baseConfig = perModelEditingActive.value && editingConfig.value
    ? editingConfig.value
    : draft.value.config_json
  const current = getModelScheduling(baseConfig, model)
  const next = upsertModelSchedulingRule(baseConfig, model, {
    priority_mode: current.priority_mode,
    scheduling_mode: mode,
  })
  if (perModelEditingActive.value) {
    updateEditingConfig(next)
    return
  }
  updateDraftConfig(next)
}

function globalModelLabel(modelName: string): string {
  const model = globalModels.value.find(item => item.name === modelName)
  if (!model) return modelName
  if (!model.display_name || model.display_name === model.name) return model.name
  return `${model.display_name} (${model.name})`
}

function updateAllowedModelScope(value: string): void {
  if (!draft.value || saving.value) return
  allowedModelsInput.value = value
  const next = updateAllowedModelsFromInput(draft.value.config_json, value)
  updateDraftConfig(next)
  if (editingConfig.value) {
    editingConfig.value = updateAllowedModelsFromInput(editingConfig.value, value)
  }
}

function clearAllowedModelScope(): void {
  if (!draft.value || saving.value) return
  const next = clearAllowedModels(draft.value.config_json)
  updateDraftConfig(next)
  if (editingConfig.value) {
    editingConfig.value = clearAllowedModels(editingConfig.value)
  }
  allowedModelsInput.value = ''
}

function replaceGroup(group: RoutingGroupRecord, select = true): void {
  const normalized = normalizeRecord(group)
  const index = groups.value.findIndex(item => item.id === normalized.id)
  if (index >= 0) {
    groups.value[index] = normalized
  } else {
    groups.value.unshift(normalized)
  }
  if (select) {
    selectGroup(normalized)
  }
}

async function fetchGroups(): Promise<void> {
  loading.value = true
  try {
    const response = await listRoutingGroups()
    groups.value = response.items.map(normalizeRecord)
  } catch (err) {
    showError(parseApiError(err, '加载调度策略失败'))
    log.error('加载调度策略失败:', err)
  } finally {
    loading.value = false
    syncRouteState()
  }
}

async function loadGlobalModels(options: { cacheTtlMs?: number } = {}): Promise<void> {
  loadingGlobalModels.value = true
  globalModelsError.value = null
  try {
    const response = await getGlobalModels(
      { limit: 1000, is_active: true },
      { cacheTtlMs: options.cacheTtlMs ?? 0 },
    )
    globalModels.value = response.models ?? []
  } catch (err) {
    globalModels.value = []
    globalModelsError.value = parseApiError(err, '加载全局模型失败')
    log.error('加载全局模型失败:', err)
  } finally {
    loadingGlobalModels.value = false
  }
}

async function saveDraft(): Promise<void> {
  if (!draft.value || saving.value) return
  const name = draft.value.name.trim()
  if (!name) {
    showError('策略名称不能为空')
    return
  }
  const config = cloneConfig(draft.value.config_json)
  if (sortingScope.value === 'per_model' && perModelPolicies.value.length === 0) {
    showError('按模型排序时至少选择一个模型')
    return
  }

  const targetGroupId = draft.value.id ?? null
  const submittedGeneration = draftGeneration
  const submittedSnapshot = draftSnapshotValue(draft.value)
  const wasCreating = isCreating.value || !draft.value.id
  saving.value = true
  try {
    const payload = {
      name,
      description: draft.value.description.trim() || null,
      enabled: draft.value.enabled,
      is_system_default: draft.value.is_system_default,
      config_json: config,
    }
    const saved = wasCreating
      ? await createRoutingGroup(payload)
      : await updateRoutingGroup(draft.value.id, payload)

    const sameDraftGeneration = draftGeneration === submittedGeneration
    const stillEditingSubmittedDraft = wasCreating
      ? sameDraftGeneration
        && isCreateRoute.value
        && isCreating.value
        && draft.value != null
        && draftSnapshotValue(draft.value) === submittedSnapshot
      : routeGroupId.value === targetGroupId
        && draft.value?.id === targetGroupId
        && (sameDraftGeneration
          ? draftSnapshotValue(draft.value) === submittedSnapshot
          : !draftDirty.value)

    if (stillEditingSubmittedDraft) {
      isCreating.value = false
    }
    replaceGroup(saved, stillEditingSubmittedDraft)
    if (wasCreating && stillEditingSubmittedDraft) {
      await router.replace({ name: 'RoutingProfileDetail', params: { groupId: saved.id } })
    }
    success('调度策略已保存')
  } catch (err) {
    showError(parseApiError(err, '保存调度策略失败'))
    log.error('保存调度策略失败:', err)
  } finally {
    saving.value = false
  }
}

function saveCurrentModel(): void {
  if (!draft.value || !editingConfig.value) return
  const model = selectedPerModelName.value
  if (!model) {
    showError('请先选择模型')
    return
  }
  const next = savePerModelRoutingConfig(editingConfig.value, model)
  updateDraftConfig(next)
  modelFilter.value = 'configured'
  resetEditingConfig()
  success('当前模型配置已保存到草稿，点击外层保存后生效')
}

function deleteDraft(): void {
  if (!draft.value?.id) return
  deleteDialogOpen.value = true
}

async function confirmDeleteDraft(): Promise<void> {
  if (!draft.value?.id) return

  deleting.value = true
  try {
    const deletedId = draft.value.id
    await deleteRoutingGroup(deletedId)
    groups.value = groups.value.filter(group => group.id !== deletedId)
    clearDraftState()
    await router.replace({ name: 'RoutingProfiles' })
    success('调度策略已删除')
    deleteDialogOpen.value = false
  } catch (err) {
    showError(parseApiError(err, '删除调度策略失败'))
    log.error('删除调度策略失败:', err)
  } finally {
    deleting.value = false
  }
}

function formatUnixSeconds(value?: number | null): string {
  if (!value) return '-'
  return new Date(value * 1000).toLocaleString('zh-CN')
}

onMounted(() => {
  void fetchGroups()
  void loadGlobalModels({ cacheTtlMs: 60_000 })
})

watch(
  () => [route.name, route.params.groupId],
  () => syncRouteState(),
)
</script>
