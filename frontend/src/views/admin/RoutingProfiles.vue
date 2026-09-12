<template>
  <PageContainer
    padding="none"
    class="space-y-6 pb-8"
  >
    <section
      v-if="!isDetailView"
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
                <TableHead
                  class="w-10"
                  aria-label="拖动调整顺序"
                />
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
                  默认策略
                </TableHead>
                <TableHead class="w-[180px]">
                  更新时间
                </TableHead>
                <TableHead class="w-[180px] text-right">
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
                :draggable="groupActionId === null"
                class="hover:bg-muted/50"
                :class="{
                  'bg-muted/60': dragOverGroupId === group.id,
                  'opacity-50': draggedGroupId === group.id,
                }"
                @dragstart="handleGroupDragStart(group.id, $event)"
                @dragend="handleGroupDragEnd"
                @dragover.prevent="handleGroupDragOver(group.id)"
                @dragleave="handleGroupDragLeave"
                @drop.prevent="handleGroupDrop(group.id)"
              >
                <TableCell class="w-10 px-2">
                  <GripVertical
                    class="h-4 w-4 cursor-grab text-muted-foreground/60"
                    title="拖动调整顺序"
                    aria-hidden="true"
                  />
                </TableCell>
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
                  {{ groupSchedulingSummary(group) }}
                </TableCell>
                <TableCell class="text-muted-foreground">
                  {{ formatUnixSeconds(group.updated_at) }}
                </TableCell>
                <TableCell class="text-right">
                  <div class="flex justify-end gap-1">
                    <Button
                      v-if="!group.is_system_default"
                      variant="ghost"
                      size="icon"
                      class="h-8 w-8 text-muted-foreground/70 hover:text-primary"
                      :disabled="groupActionId !== null"
                      aria-label="设为默认"
                      title="设为默认"
                      @click.stop="setDefaultGroup(group)"
                    >
                      <Star class="h-4 w-4" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon"
                      class="h-8 w-8 text-muted-foreground/70 hover:text-foreground"
                      :disabled="groupActionId !== null"
                      :aria-label="group.enabled ? '禁用策略' : '启用策略'"
                      :title="group.enabled ? '禁用策略' : '启用策略'"
                      @click.stop="toggleGroupEnabled(group)"
                    >
                      <Power class="h-4 w-4" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon"
                      class="h-8 w-8 text-muted-foreground/70 hover:text-destructive"
                      :disabled="groupActionId !== null || deleting"
                      aria-label="删除策略"
                      title="删除策略"
                      @click.stop="requestDeleteGroup(group)"
                    >
                      <Trash2 class="h-4 w-4" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon"
                      class="h-8 w-8"
                      title="配置策略"
                      aria-label="配置策略"
                      @click.stop="openGroup(group)"
                    >
                      <ChevronRight class="h-4 w-4" />
                    </Button>
                  </div>
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
            <div
              v-for="group in groups"
              :key="group.id"
              :draggable="groupActionId === null"
              class="flex w-full items-start justify-between gap-2 px-3 py-3 text-left transition-colors hover:bg-muted/50"
              :class="{
                'bg-muted/60': dragOverGroupId === group.id,
                'opacity-50': draggedGroupId === group.id,
              }"
              @dragstart="handleGroupDragStart(group.id, $event)"
              @dragend="handleGroupDragEnd"
              @dragover.prevent="handleGroupDragOver(group.id)"
              @dragleave="handleGroupDragLeave"
              @drop.prevent="handleGroupDrop(group.id)"
            >
              <GripVertical
                class="mt-1 h-4 w-4 shrink-0 cursor-grab text-muted-foreground/60"
                title="拖动调整顺序"
                aria-hidden="true"
              />
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
                  <span>{{ groupSchedulingSummary(group) }}</span>
                </div>
              </div>
              <div class="flex shrink-0 items-start gap-1">
                <Button
                  v-if="!group.is_system_default"
                  variant="ghost"
                  size="icon"
                  class="h-8 w-8 text-muted-foreground/70 hover:text-primary"
                  :disabled="groupActionId !== null"
                  aria-label="设为默认"
                  title="设为默认"
                  @click.stop="setDefaultGroup(group)"
                >
                  <Star class="h-4 w-4" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-8 w-8 text-muted-foreground/70 hover:text-foreground"
                  :disabled="groupActionId !== null"
                  :aria-label="group.enabled ? '禁用策略' : '启用策略'"
                  :title="group.enabled ? '禁用策略' : '启用策略'"
                  @click.stop="toggleGroupEnabled(group)"
                >
                  <Power class="h-4 w-4" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-8 w-8 text-muted-foreground/70 hover:text-destructive"
                  :disabled="groupActionId !== null || deleting"
                  aria-label="删除策略"
                  title="删除策略"
                  @click.stop="requestDeleteGroup(group)"
                >
                  <Trash2 class="h-4 w-4" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-8 w-8"
                  aria-label="配置策略"
                  title="配置策略"
                  @click.stop="openGroup(group)"
                >
                  <ChevronRight class="h-4 w-4 text-muted-foreground" />
                </Button>
              </div>
            </div>
          </div>
        </div>
      </TableCard>
    </section>

    <section
      v-else
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
                class="h-8 w-8"
                :class="draft.enabled
                  ? 'text-emerald-600 hover:text-emerald-700 dark:text-emerald-400 dark:hover:text-emerald-300'
                  : 'text-muted-foreground/70 hover:text-foreground'"
                :disabled="saving"
                :aria-label="draft.enabled ? '禁用策略' : '启用策略'"
                :title="draft.enabled ? '禁用策略' : '启用策略'"
                @click="setDraftEnabled(!draft.enabled)"
              >
                <Power class="h-4 w-4" />
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
          <div class="grid gap-3 lg:grid-cols-[minmax(0,1fr)_minmax(0,3fr)]">
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
          </div>

          <section class="space-y-3 rounded-lg border border-border/60 p-4">
            <div>
              <h3 class="text-sm font-medium">
                系统配置
              </h3>
              <p class="mt-1 text-xs text-muted-foreground">
                这些选项作用于当前调度策略。
              </p>
            </div>
            <div class="grid auto-rows-fr grid-cols-1 gap-2 md:grid-cols-2 xl:grid-cols-4">
              <div
                class="order-1 flex min-h-12 items-center justify-between gap-3 rounded-lg border border-border/60 px-3 py-2 text-sm"
                data-testid="keep-priority-on-conversion"
              >
                <div class="flex min-w-0 items-center gap-1.5">
                  <span class="font-medium">格式转换保持优先级</span>
                  <HelpHint
                    label="格式转换保持优先级"
                    text="开启后，跨 API 格式转换的候选不会被降级到同格式候选之后；Provider 自身的同名开关仍单独生效。"
                  />
                </div>
                <Switch
                  :model-value="keepPriorityOnConversion"
                  :disabled="saving"
                  aria-label="格式转换保持优先级"
                  @update:model-value="updateKeepPriorityOnConversion"
                />
              </div>
              <div
                class="order-4 flex min-h-12 items-center justify-between gap-3 rounded-lg border border-border/60 px-3 py-2 text-sm"
                data-testid="sticky-key-attempts"
              >
                <div class="flex min-w-0 items-center gap-1.5">
                  <span class="font-medium">错误重试次数</span>
                  <HelpHint
                    label="错误重试次数"
                    text="首个候选（缓存亲和命中的 Key）的总尝试次数。2 表示失败后同 Key 重试 1 次再转移；0 或 1 表示不重试。"
                  />
                </div>
                <Input
                  :model-value="stickyKeyAttempts"
                  type="number"
                  min="0"
                  max="99"
                  class="w-20 shrink-0"
                  :disabled="saving"
                  aria-label="错误重试次数"
                  @update:model-value="updateStickyKeyAttempts"
                />
              </div>
              <div
                class="order-3 flex min-h-12 items-center justify-between gap-3 rounded-lg border border-border/60 px-3 py-2 text-sm"
                data-testid="cf-heartbeat"
              >
                <div class="flex min-w-0 items-center gap-1.5">
                  <span class="font-medium">CF保持心跳</span>
                  <HelpHint
                    label="CF保持心跳"
                    text="同步生图和标准文本非流式失败时保持外层 HTTP 状态为 200，并在响应体中返回错误。"
                  />
                </div>
                <Switch
                  :model-value="cfHeartbeat"
                  :disabled="saving"
                  aria-label="CF保持心跳"
                  @update:model-value="updateExecutionPolicy('enable_cf_heartbeat', $event)"
                />
              </div>
              <div
                class="order-2 flex min-h-12 items-center justify-between gap-3 rounded-lg border border-border/60 px-3 py-2 text-sm"
                data-testid="cyber-continue-failover"
              >
                <div class="flex min-w-0 items-center gap-1.5">
                  <span class="font-medium">Cyber继续转移</span>
                  <HelpHint
                    label="Cyber继续转移"
                    text="响应开始前遇到 Cyber Policy 错误时继续故障转移。"
                  />
                </div>
                <Switch
                  :model-value="cyberContinueFailover"
                  :disabled="saving"
                  aria-label="Cyber继续转移"
                  @update:model-value="updateExecutionPolicy('cyber_continue_failover', $event)"
                />
              </div>
              <div
                class="order-5 flex min-h-12 items-center justify-between gap-3 rounded-lg border border-border/60 px-3 py-2 text-sm"
                data-testid="cancel-on-client-disconnect"
              >
                <div class="flex min-w-0 items-center gap-1.5">
                  <span class="font-medium">取消请求立即打断</span>
                  <HelpHint
                    label="取消请求立即打断"
                    text="默认关闭：客户端取消或断开后，服务端继续等待请求完成并正常计费。开启后立即打断且不计费，按次计费的请求仍收取单次请求费用。仅作用于当前调度策略。"
                  />
                </div>
                <Switch
                  :model-value="cancelOnClientDisconnect"
                  :disabled="saving"
                  aria-label="取消请求立即打断"
                  @update:model-value="updateExecutionPolicy('cancel_on_client_disconnect', $event)"
                />
              </div>
            </div>
          </section>

          <RoutingFailoverPolicyEditor
            :key="draftGeneration"
            ref="routingFailoverPolicyEditor"
            :model-value="draft.config_json.default_policy"
            :disabled="saving"
            @update:model-value="updateRoutingFailoverPolicy"
            @pending-change="routingFailoverPending = $event"
          />

          <RoutingSchedulingPolicyEditor
            :key="draftGeneration"
            :config="draft.config_json"
            :global-models="globalModels"
            :loading-models="loadingGlobalModels"
            :models-error="globalModelsError"
            :disabled="saving"
            @update:config="updateDraftConfig"
            @validity-change="routingSchedulingValid = $event"
            @reload-models="loadGlobalModels()"
          />
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
      v-model="deleteDialogOpen"
      type="destructive"
      title="删除调度策略"
      :description="`确认删除调度策略「${draft?.name ?? listDeleteTarget?.name ?? ''}」？此操作无法撤销。`"
      confirm-text="删除"
      :loading="deleting"
      @confirm="confirmDeleteDraft"
    />
  </PageContainer>
</template>

<script setup lang="ts">
import { getI18nLocale } from '@/i18n'
import { computed, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import {
  ChevronRight,
  GripVertical,
  Plus,
  Power,
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
  Switch,
  Table,
  TableBody,
  TableCard,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui'
import { AlertDialog } from '@/components/common'
import HelpHint from '@/components/common/HelpHint.vue'
import {
  DEFAULT_ROUTING_POLICY_MODEL,
  DEFAULT_STICKY_KEY_ATTEMPTS,
  createEmptyRoutingGroupConfig,
  isGeneratedModelSchedulingRule,
  isGeneratedSchedulingPolicyRule,
  normalizeRoutingGroupConfig,
  normalizeStickyKeyAttempts,
  type RoutingGroupConfig,
  type RoutingSchedulingMode,
} from '@/features/routing/utils/routingPolicy'
import { RoutingFailoverPolicyEditor, RoutingSchedulingPolicyEditor } from '@/features/routing/components'
import { normalizeRoutingFailoverPolicy, validateRoutingFailoverPolicy, type RoutingFailoverPolicy } from '@/features/routing/utils/routingFailover'
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
const routingFailoverPolicyEditor = ref<{ commitJsonDrafts: () => boolean } | null>(null)
const routingFailoverPending = ref(false)
const routingSchedulingValid = ref(true)
const savedDraftSnapshot = ref<string | null>(null)
const globalModels = ref<GlobalModelResponse[]>([])
const loadingGlobalModels = ref(false)
const globalModelsError = ref<string | null>(null)

const loading = ref(false)
const saving = ref(false)
const deleting = ref(false)
const groupActionId = ref<string | null>(null)
const draggedGroupId = ref<string | null>(null)
const dragOverGroupId = ref<string | null>(null)
const isCreating = ref(false)
const draftGeneration = ref(0)

const deleteDialogOpen = ref(false)
const listDeleteTarget = ref<RoutingGroupRecord | null>(null)

const isCreateRoute = computed(() => route.name === 'RoutingProfileCreate')
const routeGroupId = computed(() => paramToString(route.params.groupId))
const isDetailView = computed(() => isCreateRoute.value || route.name === 'RoutingProfileDetail')
const keepPriorityOnConversion = computed<boolean>(() => (
  draft.value?.config_json.default_policy.keep_priority_on_conversion ?? false
))
const stickyKeyAttempts = computed<number>(() => (
  draft.value?.config_json.default_policy.sticky_key_attempts ?? DEFAULT_STICKY_KEY_ATTEMPTS
))
const cfHeartbeat = computed<boolean>(() => (
  draft.value?.config_json.default_policy.enable_cf_heartbeat ?? false
))
const cyberContinueFailover = computed<boolean>(() => (
  draft.value?.config_json.default_policy.cyber_continue_failover ?? false
))
const cancelOnClientDisconnect = computed<boolean>(() => (
  draft.value?.config_json.default_policy.cancel_on_client_disconnect ?? false
))
function normalizeRecord(group: RoutingGroupRecord): RoutingGroupRecord {
  return {
    ...group,
    sort_order: Number.isFinite(group.sort_order) ? group.sort_order : 0,
    config_json: normalizeRoutingGroupConfig(group.config_json),
  }
}

function sortGroupsForDisplay(items: RoutingGroupRecord[]): RoutingGroupRecord[] {
  return [...items].sort((left, right) => {
    if (left.enabled !== right.enabled) return left.enabled ? -1 : 1
    if (left.sort_order !== right.sort_order) return left.sort_order - right.sort_order
    return left.name.localeCompare(right.name) || left.id.localeCompare(right.id)
  })
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
  draftGeneration.value += 1
  routingFailoverPending.value = false
  routingSchedulingValid.value = true
  isCreating.value = false
  selectedGroupId.value = null
  draft.value = null
  savedDraftSnapshot.value = null
  deleteDialogOpen.value = false
  listDeleteTarget.value = null
}

function selectGroup(group: RoutingGroupRecord): void {
  const normalized = normalizeRecord(group)
  draftGeneration.value += 1
  routingFailoverPending.value = false
  routingSchedulingValid.value = true
  isCreating.value = false
  selectedGroupId.value = normalized.id
  draft.value = buildDraft(normalized)
  savedDraftSnapshot.value = draftSnapshotValue(draft.value)
}

function setDraftEnabled(value: boolean): void {
  if (!draft.value) return
  draft.value.enabled = value
}

function startCreate(): void {
  draftGeneration.value += 1
  routingFailoverPending.value = false
  routingSchedulingValid.value = true
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
  return hasPerModelSorting(normalizeRoutingGroupConfig(group.config_json)) ? '指定模型' : '全部模型'
}

function groupSchedulingSummary(group: RoutingGroupRecord): string {
  const config = normalizeRoutingGroupConfig(group.config_json)
  if (hasPerModelSorting(config)) return '按适用范围配置'
  return schedulingModeLabel(config.default_policy.scheduling_mode)
}

function updateDraftConfig(value: RoutingGroupConfig): void {
  if (!draft.value) return
  draft.value.config_json = normalizeRoutingGroupConfig(value)
}

const draftDirty = computed(() => {
  if (!draft.value) return false
  if (isCreating.value) return true
  return routingFailoverPending.value || savedDraftSnapshot.value !== draftSnapshotValue(draft.value)
})

const canSaveDraft = computed(() => Boolean(draft.value)
  && !saving.value
  && draftDirty.value
  && routingSchedulingValid.value)

function hasPerModelSorting(config: RoutingGroupConfig): boolean {
  return config.model_policies.some(policy => policy.model !== DEFAULT_ROUTING_POLICY_MODEL)
    || config.rules.some(rule => isGeneratedModelSchedulingRule(rule) || isGeneratedSchedulingPolicyRule(rule))
}

function updateStickyKeyAttempts(value: string | number): void {
  if (!draft.value) return
  updateDraftConfig({
    ...draft.value.config_json,
    default_policy: {
      ...draft.value.config_json.default_policy,
      sticky_key_attempts: normalizeStickyKeyAttempts(value),
    },
  })
}

function updateKeepPriorityOnConversion(value: boolean): void {
  if (!draft.value) return
  updateDraftConfig({
    ...draft.value.config_json,
    default_policy: {
      ...draft.value.config_json.default_policy,
      keep_priority_on_conversion: value,
    },
  })
}

function updateExecutionPolicy(
  field: 'enable_cf_heartbeat' | 'cyber_continue_failover' | 'cancel_on_client_disconnect',
  value: boolean,
): void {
  if (!draft.value) return
  updateDraftConfig({
    ...draft.value.config_json,
    default_policy: {
      ...draft.value.config_json.default_policy,
      [field]: value,
    },
  })
}

function updateRoutingFailoverPolicy(value: RoutingFailoverPolicy): void {
  if (!draft.value) return
  const patch = normalizeRoutingFailoverPolicy(value)
  Object.assign(draft.value.config_json.default_policy, patch)
}

function replaceGroup(group: RoutingGroupRecord, select = true): void {
  const normalized = normalizeRecord(group)
  const index = groups.value.findIndex(item => item.id === normalized.id)
  if (index >= 0) {
    groups.value[index] = normalized
  } else {
    groups.value.unshift(normalized)
  }
  groups.value = sortGroupsForDisplay(groups.value)
  if (select) {
    selectGroup(normalized)
  }
}

function replaceGroupInList(group: RoutingGroupRecord, options: { setAsDefault?: boolean } = {}): void {
  const normalized = normalizeRecord(group)
  const setAsDefault = options.setAsDefault ?? normalized.is_system_default
  groups.value = groups.value.map(item => {
    if (item.id === normalized.id) return normalized
    if (setAsDefault) return { ...item, is_system_default: false }
    return item
  })
  groups.value = sortGroupsForDisplay(groups.value)
}

async function setDefaultGroup(group: RoutingGroupRecord): Promise<void> {
  if (group.is_system_default || groupActionId.value) return
  groupActionId.value = group.id
  try {
    const updated = await updateRoutingGroup(group.id, { is_system_default: true })
    replaceGroupInList(updated, { setAsDefault: true })
    if (draft.value?.id === group.id) {
      draft.value.is_system_default = true
    }
    success('已设为默认调度策略')
  } catch (err) {
    showError(parseApiError(err, '设置默认调度策略失败'))
    log.error('设置默认调度策略失败:', err)
  } finally {
    groupActionId.value = null
  }
}

async function toggleGroupEnabled(group: RoutingGroupRecord): Promise<void> {
  if (groupActionId.value) return
  const enabled = !group.enabled
  groupActionId.value = group.id
  try {
    const updated = await updateRoutingGroup(group.id, { enabled })
    replaceGroupInList(updated)
    if (draft.value?.id === group.id) {
      draft.value.enabled = updated.enabled
    }
    success(enabled ? '调度策略已启用' : '调度策略已禁用')
  } catch (err) {
    showError(parseApiError(err, enabled ? '启用调度策略失败' : '禁用调度策略失败'))
    log.error('切换调度策略状态失败:', err)
  } finally {
    groupActionId.value = null
  }
}

function handleGroupDragStart(groupId: string, event: DragEvent): void {
  if (groupActionId.value) return
  draggedGroupId.value = groupId
  dragOverGroupId.value = null
  if (event.dataTransfer) {
    event.dataTransfer.effectAllowed = 'move'
    event.dataTransfer.setData('text/plain', groupId)
  }
}

function handleGroupDragEnd(): void {
  draggedGroupId.value = null
  dragOverGroupId.value = null
}

function handleGroupDragOver(groupId: string): void {
  if (!draggedGroupId.value || draggedGroupId.value === groupId) return
  const source = groups.value.find(group => group.id === draggedGroupId.value)
  const target = groups.value.find(group => group.id === groupId)
  if (!source || !target || source.enabled !== target.enabled) return
  dragOverGroupId.value = groupId
}

function handleGroupDragLeave(): void {
  dragOverGroupId.value = null
}

async function handleGroupDrop(targetId: string): Promise<void> {
  const sourceId = draggedGroupId.value
  handleGroupDragEnd()
  if (!sourceId || sourceId === targetId || groupActionId.value) return
  const source = groups.value.find(group => group.id === sourceId)
  const target = groups.value.find(group => group.id === targetId)
  if (!source || !target || source.enabled !== target.enabled) return

  const reordered = [...groups.value]
  const sourceIndex = reordered.findIndex(group => group.id === sourceId)
  const targetIndex = reordered.findIndex(group => group.id === targetId)
  if (sourceIndex < 0 || targetIndex < 0) return
  const [moved] = reordered.splice(sourceIndex, 1)
  reordered.splice(targetIndex, 0, moved)
  groups.value = reordered.map((group, index) => ({ ...group, sort_order: index }))

  const orderSnapshot = groups.value.map(group => ({ id: group.id, sort_order: group.sort_order }))
  groupActionId.value = '__reorder__'
  try {
    const updates = await Promise.all(
      orderSnapshot.map(({ id, sort_order }) => updateRoutingGroup(id, { sort_order })),
    )
    const updatedById = new Map(updates.map(group => [group.id, normalizeRecord(group)]))
    groups.value = sortGroupsForDisplay(groups.value.map(group => updatedById.get(group.id) ?? group))
    success('调度策略顺序已更新')
  } catch (err) {
    showError(parseApiError(err, '保存调度策略顺序失败'))
    log.error('保存调度策略顺序失败:', err)
    await fetchGroups()
  } finally {
    groupActionId.value = null
  }
}

async function fetchGroups(): Promise<void> {
  loading.value = true
  try {
    const response = await listRoutingGroups()
    groups.value = sortGroupsForDisplay(response.items.map(normalizeRecord))
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
  if (routingFailoverPolicyEditor.value && !routingFailoverPolicyEditor.value.commitJsonDrafts()) return
  const failoverError = validateRoutingFailoverPolicy(draft.value.config_json.default_policy)
  if (failoverError) {
    showError(failoverError)
    return
  }
  const config = cloneConfig(draft.value.config_json)
  if (!routingSchedulingValid.value) {
    showError('请为每条调度配置选择适用模型')
    return
  }

  const targetGroupId = draft.value.id ?? null
  const submittedGeneration = draftGeneration.value
  const submittedSnapshot = draftSnapshotValue(draft.value)
  const wasCreating = isCreating.value || !draft.value.id
  saving.value = true
  try {
    const payload = {
      name,
      description: draft.value.description.trim() || null,
      enabled: draft.value.enabled,
      is_system_default: draft.value.is_system_default,
      sort_order: wasCreating
        ? groups.value.filter(group => group.enabled === draft.value?.enabled).length
        : undefined,
      config_json: config,
    }
    const saved = wasCreating || !targetGroupId
      ? await createRoutingGroup(payload)
      : await updateRoutingGroup(targetGroupId, payload)

    const sameDraftGeneration = draftGeneration.value === submittedGeneration
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

function deleteDraft(): void {
  if (!draft.value?.id) return
  listDeleteTarget.value = null
  deleteDialogOpen.value = true
}

function requestDeleteGroup(group: RoutingGroupRecord): void {
  if (groupActionId.value || deleting.value) return
  listDeleteTarget.value = group
  deleteDialogOpen.value = true
}

async function confirmDeleteDraft(): Promise<void> {
  const targetId = draft.value?.id ?? listDeleteTarget.value?.id
  if (!targetId) return

  deleting.value = true
  try {
    const deletedId = targetId
    await deleteRoutingGroup(deletedId)
    groups.value = groups.value.filter(group => group.id !== deletedId)
    const deletingCurrentDraft = draft.value?.id === deletedId
    if (deletingCurrentDraft) {
      clearDraftState()
      await router.replace({ name: 'RoutingProfiles' })
    }
    success('调度策略已删除')
    listDeleteTarget.value = null
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
  return new Date(value * 1000).toLocaleString(getI18nLocale())
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
