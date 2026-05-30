<template>
  <PageContainer>
    <PageHeader
      title="调度策略"
      description="管理调度分组、模型范围、默认调度模式和规则配置"
    />

    <div class="mt-6 grid gap-5 xl:grid-cols-[320px_minmax(0,1fr)]">
      <Card class="overflow-hidden">
        <div class="border-b border-border/60 px-5 py-4">
          <div class="flex items-center justify-between gap-3">
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
              class="h-8 w-8 text-muted-foreground/70 hover:text-foreground"
              :disabled="loading"
              aria-label="添加策略"
              title="添加策略"
              @click="startCreate"
            >
              <Plus class="h-4 w-4" />
            </Button>
          </div>
        </div>

        <div class="max-h-[calc(100vh-18rem)] overflow-y-auto p-3">
          <div
            v-if="loading"
            class="py-10 text-center text-sm text-muted-foreground"
          >
            正在加载调度策略
          </div>
          <div
            v-else
            class="space-y-2"
          >
            <div
              v-if="groups.length === 0 && !isCreating"
              class="rounded-lg border border-dashed border-border/70 px-4 py-8 text-center"
            >
              <p class="text-sm font-medium">
                暂无调度策略
              </p>
              <p class="mt-1 text-xs text-muted-foreground">
                可以先创建一个默认分组
              </p>
            </div>

            <div
              v-if="isCreating && draft"
              class="rounded-lg border border-primary/60 bg-primary/10 px-4 py-2.5 text-left"
            >
              <div class="flex items-start justify-between gap-3">
                <div class="min-w-0">
                  <p class="truncate text-sm font-medium">
                    {{ draft.name || '新调度策略' }}
                  </p>
                  <p class="mt-1 line-clamp-2 text-xs text-muted-foreground">
                    {{ draft.description || '未填写描述' }}
                  </p>
                </div>
                <div class="shrink-0">
                  <Badge :variant="draft.enabled ? 'default' : 'secondary'">
                    {{ draft.enabled ? '启用' : '停用' }}
                  </Badge>
                </div>
              </div>
              <div class="mt-1.5 flex items-end justify-between gap-3">
                <div class="flex min-w-0 flex-wrap items-center gap-2 text-xs text-muted-foreground">
                  <span v-if="draft.is_system_default">系统默认</span>
                  <span>{{ draft.config_json.allowed_models.length || '全部' }} 模型范围</span>
                </div>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-8 w-8 shrink-0"
                  :class="draft.enabled
                    ? 'text-primary hover:text-primary'
                    : 'text-muted-foreground/70 hover:text-foreground'"
                  :aria-label="draft.enabled ? '停用策略' : '启用策略'"
                  :title="draft.enabled ? '已启用，点击停用' : '已停用，点击启用'"
                  @click="setDraftEnabled(!draft.enabled)"
                >
                  <Power class="h-4 w-4" />
                </Button>
              </div>
            </div>

            <div
              v-for="group in groups"
              :key="group.id"
              role="button"
              tabindex="0"
              class="w-full rounded-lg border px-4 py-2.5 text-left transition-colors"
              :class="group.id === selectedGroupId
                ? 'border-primary/60 bg-primary/10'
                : 'border-border/60 bg-background hover:border-primary/40 hover:bg-muted/50'"
              @click="selectGroup(group)"
              @keydown.enter.prevent="selectGroup(group)"
              @keydown.space.prevent="selectGroup(group)"
            >
              <div class="flex items-start justify-between gap-3">
                <div class="min-w-0">
                  <p class="truncate text-sm font-medium">
                    {{ group.name }}
                  </p>
                  <p class="mt-1 line-clamp-2 text-xs text-muted-foreground">
                    {{ group.description || '未填写描述' }}
                  </p>
                </div>
                <div class="shrink-0">
                  <Badge :variant="displayGroupEnabled(group) ? 'default' : 'secondary'">
                    {{ displayGroupEnabled(group) ? '启用' : '停用' }}
                  </Badge>
                </div>
              </div>
              <div class="mt-1.5 flex items-end justify-between gap-3">
                <div class="flex min-w-0 flex-wrap items-center gap-2 text-xs text-muted-foreground">
                  <span v-if="group.is_system_default">系统默认</span>
                  <span>{{ group.config_json.allowed_models.length || '全部' }} 模型范围</span>
                </div>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-8 w-8 shrink-0"
                  :class="displayGroupEnabled(group)
                    ? 'text-primary hover:text-primary'
                    : 'text-muted-foreground/70 hover:text-foreground'"
                  :aria-label="displayGroupEnabled(group) ? '停用策略' : '启用策略'"
                  :title="displayGroupEnabled(group) ? '已启用，点击停用' : '已停用，点击启用'"
                  @keydown.stop
                  @click.stop="updateGroupEnabled(group, !displayGroupEnabled(group))"
                >
                  <Power class="h-4 w-4" />
                </Button>
              </div>
            </div>
          </div>
        </div>
      </Card>

      <Card
        v-if="draft"
        class="overflow-hidden"
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
          <div class="grid gap-3 lg:grid-cols-[minmax(0,0.9fr)_minmax(0,1.2fr)_320px]">
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
            请选择或新建调度策略
          </p>
        </div>
      </Card>
    </div>

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
import { computed, onMounted, ref } from 'vue'
import { ChevronDown, Copy, Key, Layers, Plus, Power, Save, Star, Trash2 } from 'lucide-vue-next'

import { PageContainer, PageHeader } from '@/components/layout'
import { Badge, Button, Card, Input } from '@/components/ui'
import { DropdownMenu, DropdownMenuTrigger, DropdownMenuContent, DropdownMenuItem } from '@/components/ui/dropdown-menu'
import { AlertDialog } from '@/components/common'
import {
  DEFAULT_ROUTING_POLICY_MODEL,
  createEmptyModelPolicy,
  createEmptyRoutingGroupConfig,
  getModelPolicy,
  getModelScheduling,
  isGeneratedModelSchedulingRule,
  modelSchedulingRuleId,
  normalizeRoutingGroupConfig,
  removeGeneratedModelSchedulingRules,
  removeModelPolicy,
  removeModelSchedulingRule,
  upsertModelPolicy,
  upsertModelSchedulingRule,
  type RoutingGroupConfig,
  type RoutingPriorityMode,
  type RoutingSchedulingMode,
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

type SortingScope = 'unified' | 'per_model'
type ModelFilter = 'configured' | 'unconfigured'

const modelFilters: Array<{ value: ModelFilter; label: string }> = [
  { value: 'unconfigured', label: '未配置' },
  { value: 'configured', label: '已配置' },
]

const { success, error: showError } = useToast()

const schedulingModes: Array<{ value: RoutingSchedulingMode; label: string }> = [
  { value: 'cache_affinity', label: '缓存亲和' },
  { value: 'load_balance', label: '负载均衡' },
  { value: 'fixed_order', label: '固定顺序' },
]

const groups = ref<RoutingGroupRecord[]>([])
const selectedGroupId = ref<string | null>(null)
const draft = ref<RoutingGroupDraft | null>(null)
const savedDraftSnapshot = ref<string | null>(null)
const sortingScope = ref<SortingScope>('unified')
const selectedPerModelName = ref<string | null>(null)
const editingConfig = ref<RoutingGroupConfig | null>(null)
const globalModelSearch = ref('')
const modelFilter = ref<ModelFilter>('unconfigured')
const globalModels = ref<GlobalModelResponse[]>([])
const loadingGlobalModels = ref(false)
const globalModelsError = ref<string | null>(null)

const loading = ref(false)
const saving = ref(false)
const deleting = ref(false)
const isCreating = ref(false)

const switchModelTarget = ref<string | null>(null)
const switchModelDialogOpen = ref(false)
const deleteDialogOpen = ref(false)

const selectedGroup = computed(() => groups.value.find(group => group.id === selectedGroupId.value) ?? null)
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

function selectGroup(group: RoutingGroupRecord): void {
  const normalized = normalizeRecord(group)
  isCreating.value = false
  selectedGroupId.value = normalized.id
  draft.value = buildDraft(normalized)
  savedDraftSnapshot.value = draftSnapshotValue(draft.value)
  syncEditorStateFromConfig(draft.value.config_json)
  resetEditingConfig()
}

function displayGroupEnabled(group: RoutingGroupRecord): boolean {
  if (!isCreating.value && group.id === selectedGroupId.value && draft.value) {
    return draft.value.enabled
  }
  return group.enabled
}

function setDraftEnabled(value: boolean): void {
  if (!draft.value) return
  draft.value.enabled = value
}

function updateGroupEnabled(group: RoutingGroupRecord, value: boolean): void {
  if (isCreating.value || group.id !== selectedGroupId.value || !draft.value) {
    selectGroup(group)
  }
  setDraftEnabled(value)
}

function startCreate(): void {
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
  syncEditorStateFromConfig(draft.value.config_json)
  resetEditingConfig()
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

function setSortingScope(scope: SortingScope): void {
  if (!draft.value) return
  sortingScope.value = scope
  if (scope === 'unified') {
    const next = removeGeneratedModelSchedulingRules(draft.value.config_json)
    next.model_policies = next.model_policies.filter(policy => policy.model === DEFAULT_ROUTING_POLICY_MODEL)
    next.allowed_models = []
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
  let next = removeModelPolicy(draft.value.config_json, model)
  next = removeModelSchedulingRule(next, model)
  next.allowed_models = next.allowed_models.filter(item => item !== model)
  if (selectedPerModelName.value === model) {
    selectedPerModelName.value = null
  }
  modelFilter.value = 'unconfigured'
  updateDraftConfig(next)
  resetEditingConfig()
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
  const sourcePolicy = getModelPolicy(draft.value.config_json, sourceModel)
  const sourceScheduling = getModelScheduling(draft.value.config_json, sourceModel)
  let next = upsertModelPolicy(editingConfig.value, {
    ...sourcePolicy,
    model: target,
  })
  next = upsertModelSchedulingRule(next, target, {
    priority_mode: sourceScheduling.priority_mode,
    scheduling_mode: sourceScheduling.scheduling_mode,
  })
  if (!next.allowed_models.includes(target)) {
    next = { ...next, allowed_models: [...next.allowed_models, target] }
  }
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

function replaceGroup(group: RoutingGroupRecord): void {
  const normalized = normalizeRecord(group)
  const index = groups.value.findIndex(item => item.id === normalized.id)
  if (index >= 0) {
    groups.value[index] = normalized
  } else {
    groups.value.unshift(normalized)
  }
  selectGroup(normalized)
}

async function fetchGroups(): Promise<void> {
  loading.value = true
  try {
    const response = await listRoutingGroups()
    groups.value = response.items.map(normalizeRecord)
    const nextSelected = selectedGroup.value ?? groups.value[0] ?? null
    if (nextSelected) {
      selectGroup(nextSelected)
    } else if (!draft.value) {
      startCreate()
    }
  } catch (err) {
    showError(parseApiError(err, '加载调度策略失败'))
    log.error('加载调度策略失败:', err)
  } finally {
    loading.value = false
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
  if (!draft.value) return
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

  saving.value = true
  try {
    const payload = {
      name,
      description: draft.value.description.trim() || null,
      enabled: draft.value.enabled,
      is_system_default: draft.value.is_system_default,
      config_json: config,
    }
    const saved = isCreating.value || !draft.value.id
      ? await createRoutingGroup(payload)
      : await updateRoutingGroup(draft.value.id, payload)
    isCreating.value = false
    replaceGroup(saved)
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
  let next = editingConfig.value
  if (!next.model_policies.some(policy => policy.model === model)) {
    next = upsertModelPolicy(next, createEmptyModelPolicy(model))
  }
  if (!next.allowed_models.includes(model)) {
    next = { ...next, allowed_models: [...next.allowed_models, model] }
  }
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
    selectedGroupId.value = null
    draft.value = null
    if (groups.value.length > 0) {
      selectGroup(groups.value[0])
    } else {
      startCreate()
    }
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
</script>
