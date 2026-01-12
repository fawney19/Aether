<template>
  <!-- 自定义抽屉 -->
  <Teleport to="body">
    <Transition name="drawer">
      <div
        v-if="open && (loading || provider)"
        class="fixed inset-0 z-50 flex justify-end"
        @click.self="handleBackdropClick"
      >
        <!-- 背景遮罩 -->
        <div
          class="absolute inset-0 bg-black/30 backdrop-blur-sm"
          @click="handleBackdropClick"
        />

        <!-- 抽屉内容 -->
        <Card class="relative h-full w-full sm:w-[700px] sm:max-w-[90vw] rounded-none shadow-2xl overflow-y-auto">
          <!-- 加载状态 -->
          <div
            v-if="loading"
            class="flex items-center justify-center py-12"
          >
            <div class="animate-spin rounded-full h-8 w-8 border-b-2 border-primary" />
          </div>

          <template v-else-if="provider">
            <!-- 头部:名称 + 快捷操作 -->
            <div class="sticky top-0 z-10 bg-background border-b px-4 sm:px-6 pt-4 sm:pt-6 pb-3 sm:pb-3">
              <div class="flex items-start justify-between gap-3 sm:gap-4">
                <div class="space-y-1 flex-1 min-w-0">
                  <div class="flex items-center gap-2">
                    <h2 class="text-lg sm:text-xl font-bold truncate">
                      {{ provider.name }}
                    </h2>
                    <Badge
                      :variant="provider.is_active ? 'default' : 'secondary'"
                      class="text-xs shrink-0"
                    >
                      {{ provider.is_active ? '活跃' : '已停用' }}
                    </Badge>
                  </div>
                  <!-- 网站链接 -->
                  <div
                    v-if="provider.website"
                    class="flex items-center gap-2"
                  >
                    <span class="text-muted-foreground">·</span>
                    <a
                      :href="provider.website"
                      target="_blank"
                      rel="noopener noreferrer"
                      class="text-xs text-primary hover:underline truncate"
                      title="访问官网"
                    >{{ provider.website }}</a>
                  </div>
                </div>
                <div class="flex items-center gap-1 shrink-0">
                  <Button
                    variant="ghost"
                    size="icon"
                    title="编辑提供商"
                    @click="$emit('edit', provider)"
                  >
                    <Edit class="w-4 h-4" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    :title="provider.is_active ? '点击停用' : '点击启用'"
                    @click="$emit('toggleStatus', provider)"
                  >
                    <Power class="w-4 h-4" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    title="关闭"
                    @click="handleClose"
                  >
                    <X class="w-4 h-4" />
                  </Button>
                </div>
              </div>
              <!-- 端点 API 格式 -->
              <div class="flex items-center gap-1.5 flex-wrap mt-3">
                <template v-for="endpoint in endpoints" :key="endpoint.id">
                  <span
                    class="text-xs px-2 py-0.5 rounded-md border border-border bg-background hover:bg-accent hover:border-accent-foreground/20 cursor-pointer transition-colors font-medium"
                    :class="{ 'opacity-40': !endpoint.is_active }"
                    :title="`编辑 ${API_FORMAT_LABELS[endpoint.api_format]} 端点`"
                    @click="handleEditEndpoint(endpoint)"
                  >{{ API_FORMAT_LABELS[endpoint.api_format] || endpoint.api_format }}</span>
                </template>
                <span
                  class="text-xs px-2 py-0.5 rounded-md border border-dashed border-border hover:bg-accent hover:border-accent-foreground/20 cursor-pointer transition-colors text-muted-foreground"
                  title="编辑端点"
                  @click="showAddEndpointDialog"
                >编辑</span>
              </div>
            </div>

            <div class="space-y-6 p-4 sm:p-6">
              <!-- 配额使用情况 -->
              <Card
                v-if="provider.billing_type === 'monthly_quota' && provider.monthly_quota_usd"
                class="p-4"
              >
                <div class="space-y-3">
                  <div class="flex items-center justify-between">
                    <h3 class="text-sm font-semibold">
                      订阅配额
                    </h3>
                    <Badge
                      variant="secondary"
                      class="text-xs"
                    >
                      {{ ((provider.monthly_used_usd || 0) / provider.monthly_quota_usd * 100).toFixed(1) }}%
                    </Badge>
                  </div>
                  <div class="relative w-full h-2 bg-muted rounded-full overflow-hidden">
                    <div
                      class="absolute left-0 top-0 h-full transition-all duration-300"
                      :class="{
                        'bg-green-500': (provider.monthly_used_usd || 0) / provider.monthly_quota_usd < 0.7,
                        'bg-yellow-500': (provider.monthly_used_usd || 0) / provider.monthly_quota_usd >= 0.7 && (provider.monthly_used_usd || 0) / provider.monthly_quota_usd < 0.9,
                        'bg-red-500': (provider.monthly_used_usd || 0) / provider.monthly_quota_usd >= 0.9
                      }"
                      :style="{ width: `${Math.min((provider.monthly_used_usd || 0) / provider.monthly_quota_usd * 100, 100)}%` }"
                    />
                  </div>
                  <div class="flex items-center justify-between text-xs">
                    <span class="font-semibold">
                      ${{ (provider.monthly_used_usd || 0).toFixed(2) }} / ${{ provider.monthly_quota_usd.toFixed(2) }}
                    </span>
                    <span
                      v-if="provider.quota_reset_day"
                      class="text-muted-foreground"
                    >
                      每月 {{ provider.quota_reset_day }} 号重置
                    </span>
                  </div>
                </div>
              </Card>

              <!-- 密钥管理 -->
              <Card class="overflow-hidden">
                <div class="p-4 border-b border-border/60">
                  <div class="flex items-center justify-between">
                    <h3 class="text-sm font-semibold">
                      密钥管理
                    </h3>
                    <Button
                      v-if="endpoints.length > 0"
                      variant="outline"
                      size="sm"
                      class="h-8"
                      @click="handleAddKeyToFirstEndpoint"
                    >
                      <Plus class="w-3.5 h-3.5 mr-1.5" />
                      添加密钥
                    </Button>
                  </div>
                </div>

                <!-- 密钥列表 -->
                <div
                  v-if="allKeys.length > 0"
                  class="divide-y divide-border/40"
                >
                  <div
                    v-for="({ key, endpoint }, index) in allKeys"
                    :key="key.id"
                    class="px-4 py-2.5 hover:bg-muted/30 transition-colors group/item"
                    :class="{
                      'opacity-50': keyDragState.isDragging && keyDragState.draggedIndex === index,
                      'bg-primary/5 border-l-2 border-l-primary': keyDragState.targetIndex === index && keyDragState.isDragging
                    }"
                    draggable="true"
                    @dragstart="handleKeyDragStart($event, index)"
                    @dragend="handleKeyDragEnd"
                    @dragover="handleKeyDragOver($event, index)"
                    @dragleave="handleKeyDragLeave"
                    @drop="handleKeyDrop($event, index)"
                  >
                    <!-- 第一行：名称 + 状态 + 操作按钮 -->
                    <div class="flex items-center justify-between gap-2">
                      <div class="flex items-center gap-2 flex-1 min-w-0">
                        <!-- 拖拽手柄 -->
                        <div class="cursor-grab active:cursor-grabbing text-muted-foreground/30 group-hover/item:text-muted-foreground transition-colors shrink-0">
                          <GripVertical class="w-4 h-4" />
                        </div>
                        <div class="flex flex-col min-w-0">
                          <span class="text-sm font-medium truncate">{{ key.name || '未命名密钥' }}</span>
                          <div class="flex items-center gap-1">
                            <span class="text-[11px] font-mono text-muted-foreground">
                              {{ key.api_key_masked }}
                            </span>
                            <Button
                              variant="ghost"
                              size="icon"
                              class="h-4 w-4 shrink-0"
                              title="复制密钥"
                              @click.stop="copyFullKey(key)"
                            >
                              <Copy class="w-2.5 h-2.5" />
                            </Button>
                          </div>
                        </div>
                        <Badge
                          v-if="!key.is_active"
                          variant="secondary"
                          class="text-[10px] px-1.5 py-0 shrink-0"
                        >
                          禁用
                        </Badge>
                        <Badge
                          v-if="key.circuit_breaker_open"
                          variant="destructive"
                          class="text-[10px] px-1.5 py-0 shrink-0"
                        >
                          熔断
                        </Badge>
                      </div>
                      <!-- 并发 + 健康度 + 操作按钮 -->
                      <div class="flex items-center gap-1 shrink-0">
                        <!-- RPM 限制信息（放在最前面） -->
                        <span
                          v-if="key.rpm_limit || key.is_adaptive"
                          class="text-[10px] text-muted-foreground mr-1"
                        >
                          {{ key.is_adaptive ? '自适应' : key.rpm_limit }} RPM
                        </span>
                        <!-- 健康度 -->
                        <div
                          v-if="key.health_score !== undefined"
                          class="flex items-center gap-1 mr-1"
                        >
                          <div class="w-10 h-1.5 bg-muted/80 rounded-full overflow-hidden">
                            <div
                              class="h-full transition-all duration-300"
                              :class="getHealthScoreBarColor(key.health_score || 0)"
                              :style="{ width: `${(key.health_score || 0) * 100}%` }"
                            />
                          </div>
                          <span
                            class="text-[10px] font-medium tabular-nums"
                            :class="getHealthScoreColor(key.health_score || 0)"
                          >
                            {{ ((key.health_score || 0) * 100).toFixed(0) }}%
                          </span>
                        </div>
                        <Button
                          v-if="key.circuit_breaker_open || (key.health_score !== undefined && key.health_score < 0.5)"
                          variant="ghost"
                          size="icon"
                          class="h-7 w-7 text-green-600"
                          title="刷新健康状态"
                          @click="handleRecoverKey(key)"
                        >
                          <RefreshCw class="w-3.5 h-3.5" />
                        </Button>
                        <Button
                          variant="ghost"
                          size="icon"
                          class="h-7 w-7"
                          title="模型权限"
                          @click="handleKeyPermissions(key)"
                        >
                          <Shield class="w-3.5 h-3.5" />
                        </Button>
                        <Button
                          variant="ghost"
                          size="icon"
                          class="h-7 w-7"
                          title="编辑密钥"
                          @click="handleEditKey(endpoint, key)"
                        >
                          <Edit class="w-3.5 h-3.5" />
                        </Button>
                        <Button
                          variant="ghost"
                          size="icon"
                          class="h-7 w-7"
                          :disabled="togglingKeyId === key.id"
                          :title="key.is_active ? '点击停用' : '点击启用'"
                          @click="toggleKeyActive(key)"
                        >
                          <Power class="w-3.5 h-3.5" />
                        </Button>
                        <Button
                          variant="ghost"
                          size="icon"
                          class="h-7 w-7"
                          title="删除密钥"
                          @click="handleDeleteKey(key)"
                        >
                          <Trash2 class="w-3.5 h-3.5" />
                        </Button>
                      </div>
                    </div>
                    <!-- 第二行：优先级 + API 格式（展开显示） + 统计信息 -->
                    <div class="flex items-center gap-1.5 mt-1 text-[11px] text-muted-foreground">
                      <!-- 优先级放最前面，支持点击编辑 -->
                      <span
                        v-if="editingPriorityKey !== key.id"
                        title="点击编辑优先级"
                        class="font-medium text-foreground/80 cursor-pointer hover:text-primary hover:underline"
                        @click="startEditPriority(key)"
                      >P{{ key.internal_priority }}</span>
                      <input
                        v-else
                        ref="priorityInputRef"
                        v-model="editingPriorityValue"
                        type="text"
                        inputmode="numeric"
                        pattern="[0-9]*"
                        class="w-8 h-5 px-1 text-[11px] text-center border rounded bg-background focus:outline-none focus:ring-1 focus:ring-primary font-medium text-foreground/80"
                        @keydown="(e) => handlePriorityKeydown(e, key)"
                        @blur="handlePriorityBlur(key)"
                      >
                      <span class="text-muted-foreground/40">|</span>
                      <!-- API 格式：展开显示每个格式和倍率 -->
                      <template
                        v-for="(format, idx) in getKeyApiFormats(key, endpoint)"
                        :key="format"
                      >
                        <span v-if="idx > 0" class="text-muted-foreground/40">/</span>
                        <span>{{ API_FORMAT_SHORT[format] || format }} {{ getKeyRateMultiplier(key, format) }}x</span>
                      </template>
                      <span v-if="key.rate_limit">| {{ key.rate_limit }}rpm</span>
                      <span
                        v-if="key.next_probe_at"
                        class="text-amber-600 dark:text-amber-400"
                      >
                        | {{ formatProbeTime(key.next_probe_at) }}探测
                      </span>
                    </div>
                  </div>
                </div>

                <!-- 空状态 -->
                <div
                  v-else
                  class="p-8 text-center text-muted-foreground"
                >
                  <Key class="w-12 h-12 mx-auto mb-3 opacity-50" />
                  <p class="text-sm">
                    暂无密钥配置
                  </p>
                  <p class="text-xs mt-1">
                    {{ endpoints.length > 0 ? '点击上方"添加密钥"按钮创建第一个密钥' : '请先添加端点，然后再添加密钥' }}
                  </p>
                </div>
              </Card>

              <!-- 模型查看 -->
              <ModelsTab
                v-if="provider"
                :key="`models-${provider.id}`"
                :provider="provider"
                @edit-model="handleEditModel"
                @delete-model="handleDeleteModel"
                @batch-assign="handleBatchAssign"
                @add-mapping="handleAddMapping"
              />

              <!-- 模型名称映射 -->
              <ModelAliasesTab
                v-if="provider"
                ref="modelAliasesTabRef"
                :key="`aliases-${provider.id}`"
                :provider="provider"
                @refresh="handleRelatedDataRefresh"
              />
            </div>
          </template>
        </Card>
      </div>
    </Transition>
  </Teleport>

  <!-- 端点表单对话框（管理/编辑） -->
  <EndpointFormDialog
    v-if="provider && open"
    v-model="endpointDialogOpen"
    :provider="provider"
    :endpoints="endpoints"
    @endpoint-created="handleEndpointChanged"
    @endpoint-updated="handleEndpointChanged"
  />

  <!-- 删除端点确认对话框 -->
  <AlertDialog
    v-if="open"
    :model-value="deleteEndpointConfirmOpen"
    title="删除端点"
    :description="`确定要删除端点 ${endpointToDelete?.api_format} 吗？这将同时删除其所有密钥。`"
    confirm-text="删除"
    cancel-text="取消"
    type="danger"
    @update:model-value="deleteEndpointConfirmOpen = $event"
    @confirm="confirmDeleteEndpoint"
    @cancel="deleteEndpointConfirmOpen = false"
  />

  <!-- 密钥编辑对话框 -->
  <KeyFormDialog
    v-if="open"
    :open="keyFormDialogOpen"
    :endpoint="currentEndpoint"
    :editing-key="editingKey"
    :provider-id="provider ? provider.id : null"
    :available-api-formats="provider?.api_formats || []"
    @close="keyFormDialogOpen = false"
    @saved="handleKeyChanged"
  />

  <!-- 模型权限对话框 -->
  <KeyAllowedModelsEditDialog
    v-if="open"
    :open="keyPermissionsDialogOpen"
    :api-key="editingKey"
    :provider-id="providerId || ''"
    @close="keyPermissionsDialogOpen = false"
    @saved="handleKeyChanged"
  />

  <!-- 删除密钥确认对话框 -->
  <AlertDialog
    v-if="open"
    :model-value="deleteKeyConfirmOpen"
    title="删除密钥"
    :description="`确定要删除密钥 ${keyToDelete?.api_key_masked} 吗？`"
    confirm-text="删除"
    cancel-text="取消"
    type="danger"
    @update:model-value="deleteKeyConfirmOpen = $event"
    @confirm="confirmDeleteKey"
    @cancel="deleteKeyConfirmOpen = false"
  />

  <!-- 添加/编辑模型对话框 -->
  <ProviderModelFormDialog
    v-if="open && provider"
    :open="modelFormDialogOpen"
    :provider-id="provider.id"
    :provider-name="provider.name"
    :editing-model="editingModel"
    @update:open="modelFormDialogOpen = $event"
    @saved="handleModelSaved"
  />

  <!-- 删除模型确认对话框 -->
  <AlertDialog
    v-if="open"
    :model-value="deleteModelConfirmOpen"
    title="移除模型支持"
    :description="`确定要移除提供商 ${provider?.name} 对模型 ${modelToDelete?.global_model_display_name || modelToDelete?.provider_model_name} 的支持吗？这不会删除全局模型，只是该提供商将不再支持此模型。`"
    confirm-text="移除"
    cancel-text="取消"
    type="danger"
    @update:model-value="deleteModelConfirmOpen = $event"
    @confirm="confirmDeleteModel"
    @cancel="deleteModelConfirmOpen = false"
  />

  <!-- 批量关联模型对话框 -->
  <BatchAssignModelsDialog
    v-if="open && provider"
    :open="batchAssignDialogOpen"
    :provider-id="provider.id"
    :provider-name="provider.name"
    :provider-identifier="provider.name"
    @update:open="batchAssignDialogOpen = $event"
    @changed="handleBatchAssignChanged"
  />
</template>

<script setup lang="ts">
import { ref, watch, computed, nextTick } from 'vue'
import {
  Server,
  Plus,
  Key,
  ChevronRight,
  Edit,
  Trash2,
  RefreshCw,
  X,
  Loader2,
  Power,
  GripVertical,
  Copy,
  Eye,
  EyeOff,
  ExternalLink,
  Shield
} from 'lucide-vue-next'
import { useEscapeKey } from '@/composables/useEscapeKey'
import Button from '@/components/ui/button.vue'
import Badge from '@/components/ui/badge.vue'
import Card from '@/components/ui/card.vue'
import { useToast } from '@/composables/useToast'
import { useClipboard } from '@/composables/useClipboard'
import { getProvider, getProviderEndpoints } from '@/api/endpoints'
import {
  KeyFormDialog,
  KeyAllowedModelsEditDialog,
  ModelsTab,
  ModelAliasesTab,
  BatchAssignModelsDialog
} from '@/features/providers/components'
import EndpointFormDialog from '@/features/providers/components/EndpointFormDialog.vue'
import ProviderModelFormDialog from '@/features/providers/components/ProviderModelFormDialog.vue'
import AlertDialog from '@/components/common/AlertDialog.vue'
import {
  deleteEndpoint as deleteEndpointAPI,
  deleteEndpointKey,
  recoverKeyHealth,
  getProviderKeys,
  updateEndpoint,
  updateProviderKey,
  revealEndpointKey,
  type ProviderEndpoint,
  type EndpointAPIKey,
  type Model,
  API_FORMAT_LABELS,
  API_FORMAT_ORDER,
  API_FORMAT_SHORT,
  sortApiFormats,
} from '@/api/endpoints'
import { deleteModel as deleteModelAPI } from '@/api/endpoints/models'

// 扩展端点类型,包含密钥列表
interface ProviderEndpointWithKeys extends ProviderEndpoint {
  keys?: EndpointAPIKey[]
  rpm_limit?: number
}

interface Props {
  providerId: string | null
  open: boolean
}

const props = defineProps<Props>()
const emit = defineEmits<{
  (e: 'update:open', value: boolean): void
  (e: 'edit', provider: any): void
  (e: 'toggleStatus', provider: any): void
  (e: 'refresh'): void
}>()

const { error: showError, success: showSuccess } = useToast()
const { copyToClipboard } = useClipboard()

const loading = ref(false)
const provider = ref<any>(null)
const endpoints = ref<ProviderEndpointWithKeys[]>([])
const providerKeys = ref<EndpointAPIKey[]>([])  // Provider 级别的 keys
const expandedEndpoints = ref<Set<string>>(new Set())

// 端点相关状态
const endpointDialogOpen = ref(false)
const deleteEndpointConfirmOpen = ref(false)
const endpointToDelete = ref<ProviderEndpoint | null>(null)

// 密钥相关状态
const keyFormDialogOpen = ref(false)
const keyPermissionsDialogOpen = ref(false)
const currentEndpoint = ref<ProviderEndpoint | null>(null)
const editingKey = ref<EndpointAPIKey | null>(null)
const deleteKeyConfirmOpen = ref(false)
const keyToDelete = ref<EndpointAPIKey | null>(null)
const recoveringEndpointId = ref<string | null>(null)
const togglingEndpointId = ref<string | null>(null)
const togglingKeyId = ref<string | null>(null)

// 密钥显示状态：key_id -> 完整密钥
const revealedKeys = ref<Map<string, string>>(new Map())
const revealingKeyId = ref<string | null>(null)

// 模型相关状态
const modelFormDialogOpen = ref(false)
const editingModel = ref<Model | null>(null)
const deleteModelConfirmOpen = ref(false)
const modelToDelete = ref<Model | null>(null)
const batchAssignDialogOpen = ref(false)

// ModelAliasesTab 组件引用
const modelAliasesTabRef = ref<InstanceType<typeof ModelAliasesTab> | null>(null)

// 拖动排序相关状态（旧的端点级别拖拽，保留以兼容）
const dragState = ref({
  isDragging: false,
  draggedKeyId: null as string | null,
  targetKeyId: null as string | null,
  dragEndpointId: null as string | null
})

// 密钥列表拖拽排序状态
const keyDragState = ref({
  isDragging: false,
  draggedIndex: null as number | null,
  targetIndex: null as number | null
})

// 点击编辑优先级相关状态
const editingPriorityKey = ref<string | null>(null)
const editingPriorityValue = ref<number>(0)
const priorityInputRef = ref<HTMLInputElement[] | null>(null)
const prioritySaving = ref(false)

// 任意模态窗口打开时,阻止抽屉被误关闭
const hasBlockingDialogOpen = computed(() =>
  endpointDialogOpen.value ||
  deleteEndpointConfirmOpen.value ||
  keyFormDialogOpen.value ||
  keyPermissionsDialogOpen.value ||
  deleteKeyConfirmOpen.value ||
  modelFormDialogOpen.value ||
  deleteModelConfirmOpen.value ||
  batchAssignDialogOpen.value ||
  // 检测 ModelAliasesTab 子组件的 Dialog 是否打开
  modelAliasesTabRef.value?.dialogOpen
)

// 所有密钥的扁平列表（带端点信息）
// key 通过 api_formats 字段确定支持的格式，endpoint 可能为 undefined
const allKeys = computed(() => {
  const result: { key: EndpointAPIKey; endpoint?: ProviderEndpointWithKeys }[] = []
  const seenKeyIds = new Set<string>()

  // 1. 先添加 Provider 级别的 keys
  for (const key of providerKeys.value) {
    if (!seenKeyIds.has(key.id)) {
      seenKeyIds.add(key.id)
      // key 没有关联特定 endpoint
      result.push({ key, endpoint: undefined })
    }
  }

  // 2. 再遍历所有端点的 keys（历史数据）
  for (const endpoint of endpoints.value) {
    if (endpoint.keys) {
      for (const key of endpoint.keys) {
        if (!seenKeyIds.has(key.id)) {
          seenKeyIds.add(key.id)
          result.push({ key, endpoint })
        }
      }
    }
  }

  return result
})

// 监听 providerId 变化
watch(() => props.providerId, (newId) => {
  if (newId && props.open) {
    loadProvider()
    loadEndpoints()
  }
}, { immediate: true })

// 监听 open 变化
watch(() => props.open, (newOpen) => {
  if (newOpen && props.providerId) {
    loadProvider()
    loadEndpoints()
  } else if (!newOpen) {
    // 重置所有状态
    provider.value = null
    endpoints.value = []
    providerKeys.value = []  // 清空 Provider 级别的 keys
    expandedEndpoints.value.clear()

    // 重置所有对话框状态
    endpointDialogOpen.value = false
    deleteEndpointConfirmOpen.value = false
    keyFormDialogOpen.value = false
    keyPermissionsDialogOpen.value = false
    deleteKeyConfirmOpen.value = false
    batchAssignDialogOpen.value = false

    // 重置临时数据
    endpointToDelete.value = null
    currentEndpoint.value = null
    editingKey.value = null
    keyToDelete.value = null

    // 清除已显示的密钥（安全考虑）
    revealedKeys.value.clear()
  }
})

// 处理背景点击
function handleBackdropClick() {
  if (!hasBlockingDialogOpen.value) {
    handleClose()
  }
}

// 关闭抽屉
function handleClose() {
  if (!hasBlockingDialogOpen.value) {
    emit('update:open', false)
  }
}

// 切换端点展开/收起
function toggleEndpoint(endpointId: string) {
  if (expandedEndpoints.value.has(endpointId)) {
    expandedEndpoints.value.delete(endpointId)
  } else {
    expandedEndpoints.value.add(endpointId)
  }
}

async function handleRelatedDataRefresh() {
  await loadProvider()
  emit('refresh')
}

// 显示端点管理对话框
function showAddEndpointDialog() {
  endpointDialogOpen.value = true
}

// ===== 端点事件处理 =====
function handleEditEndpoint(_endpoint: ProviderEndpoint) {
  // 点击任何端点都打开管理对话框
  endpointDialogOpen.value = true
}

function handleDeleteEndpoint(endpoint: ProviderEndpoint) {
  endpointToDelete.value = endpoint
  deleteEndpointConfirmOpen.value = true
}

async function confirmDeleteEndpoint() {
  if (!endpointToDelete.value) return

  try {
    await deleteEndpointAPI(endpointToDelete.value.id)
    showSuccess('端点已删除')
    await loadEndpoints()
    emit('refresh')
  } catch (err: any) {
    showError(err.response?.data?.detail || '删除失败', '错误')
  } finally {
    deleteEndpointConfirmOpen.value = false
    endpointToDelete.value = null
  }
}

async function handleEndpointChanged() {
  await Promise.all([loadProvider(), loadEndpoints()])
  emit('refresh')
}

// ===== 密钥事件处理 =====
function handleAddKey(endpoint: ProviderEndpoint) {
  currentEndpoint.value = endpoint
  editingKey.value = null
  keyFormDialogOpen.value = true
}

// 添加密钥（如果有多个端点则添加到第一个）
function handleAddKeyToFirstEndpoint() {
  if (endpoints.value.length > 0) {
    handleAddKey(endpoints.value[0])
  }
}

function handleEditKey(endpoint: ProviderEndpoint | undefined, key: EndpointAPIKey) {
  currentEndpoint.value = endpoint || null
  editingKey.value = key
  keyFormDialogOpen.value = true
}

function handleKeyPermissions(key: EndpointAPIKey) {
  editingKey.value = key
  keyPermissionsDialogOpen.value = true
}

// 切换密钥显示/隐藏
async function toggleKeyReveal(key: EndpointAPIKey) {
  if (revealedKeys.value.has(key.id)) {
    // 已显示，隐藏它
    revealedKeys.value.delete(key.id)
    return
  }

  // 未显示，调用 API 获取完整密钥
  revealingKeyId.value = key.id
  try {
    const result = await revealEndpointKey(key.id)
    revealedKeys.value.set(key.id, result.api_key)
  } catch (err: any) {
    showError(err.response?.data?.detail || '获取密钥失败', '错误')
  } finally {
    revealingKeyId.value = null
  }
}

// 复制完整密钥
async function copyFullKey(key: EndpointAPIKey) {
  // 如果已经显示了，直接复制
  if (revealedKeys.value.has(key.id)) {
    copyToClipboard(revealedKeys.value.get(key.id)!)
    return
  }

  // 否则先获取再复制
  try {
    const result = await revealEndpointKey(key.id)
    copyToClipboard(result.api_key)
  } catch (err: any) {
    showError(err.response?.data?.detail || '获取密钥失败', '错误')
  }
}

function handleDeleteKey(key: EndpointAPIKey) {
  keyToDelete.value = key
  deleteKeyConfirmOpen.value = true
}

async function confirmDeleteKey() {
  if (!keyToDelete.value) return

  const keyId = keyToDelete.value.id
  deleteKeyConfirmOpen.value = false
  keyToDelete.value = null

  try {
    await deleteEndpointKey(keyId)
    showSuccess('密钥已删除')
    await loadEndpoints()
    emit('refresh')
  } catch (err: any) {
    showError(err.response?.data?.detail || '删除密钥失败', '错误')
  }
}

async function handleRecoverKey(key: EndpointAPIKey) {
  try {
    const result = await recoverKeyHealth(key.id)
    showSuccess(result.message || 'Key已完全恢复')
    await loadEndpoints()
    emit('refresh')
  } catch (err: any) {
    showError(err.response?.data?.detail || 'Key恢复失败', '错误')
  }
}

// 检查端点是否有不健康的密钥
function hasUnhealthyKeys(endpoint: ProviderEndpointWithKeys): boolean {
  if (!endpoint.keys || endpoint.keys.length === 0) return false
  return endpoint.keys.some(key =>
    key.circuit_breaker_open ||
    (key.health_score !== undefined && key.health_score < 1)
  )
}

// 批量恢复端点下所有密钥的健康状态
async function handleRecoverAllKeys(endpoint: ProviderEndpointWithKeys) {
  if (!endpoint.keys || endpoint.keys.length === 0) return

  const keysToRecover = endpoint.keys.filter(key =>
    key.circuit_breaker_open ||
    (key.health_score !== undefined && key.health_score < 1)
  )

  if (keysToRecover.length === 0) {
    showSuccess('所有密钥已处于健康状态')
    return
  }

  recoveringEndpointId.value = endpoint.id
  let successCount = 0
  let failCount = 0

  try {
    for (const key of keysToRecover) {
      try {
        await recoverKeyHealth(key.id)
        successCount++
      } catch {
        failCount++
      }
    }

    if (failCount === 0) {
      showSuccess(`已恢复 ${successCount} 个密钥的健康状态`)
    } else {
      showSuccess(`恢复完成: ${successCount} 成功, ${failCount} 失败`)
    }

    await loadEndpoints()
    emit('refresh')
  } finally {
    recoveringEndpointId.value = null
  }
}

async function handleKeyChanged() {
  await loadEndpoints()
  emit('refresh')
}

// 切换端点启用状态
async function toggleEndpointActive(endpoint: ProviderEndpointWithKeys) {
  if (togglingEndpointId.value) return

  togglingEndpointId.value = endpoint.id
  try {
    const newStatus = !endpoint.is_active
    await updateEndpoint(endpoint.id, { is_active: newStatus })
    endpoint.is_active = newStatus
    showSuccess(newStatus ? '端点已启用' : '端点已停用')
    emit('refresh')
  } catch (err: any) {
    showError(err.response?.data?.detail || '操作失败', '错误')
  } finally {
    togglingEndpointId.value = null
  }
}

// 切换密钥启用状态
async function toggleKeyActive(key: EndpointAPIKey) {
  if (togglingKeyId.value) return

  togglingKeyId.value = key.id
  try {
    const newStatus = !key.is_active
    await updateProviderKey(key.id, { is_active: newStatus })
    key.is_active = newStatus
    showSuccess(newStatus ? '密钥已启用' : '密钥已停用')
    emit('refresh')
  } catch (err: any) {
    showError(err.response?.data?.detail || '操作失败', '错误')
  } finally {
    togglingKeyId.value = null
  }
}

// ===== 模型事件处理 =====
// 处理编辑模型
function handleEditModel(model: Model) {
  editingModel.value = model
  modelFormDialogOpen.value = true
}

// 处理打开批量关联对话框
function handleBatchAssign() {
  batchAssignDialogOpen.value = true
}

// 处理添加映射（从 ModelsTab 触发）
function handleAddMapping(model: Model) {
  modelAliasesTabRef.value?.openAddDialogForModel(model.id)
}

// 处理批量关联完成
async function handleBatchAssignChanged() {
  await loadProvider()
  emit('refresh')
}

// 处理模型保存完成
async function handleModelSaved() {
  editingModel.value = null
  await loadProvider()
  emit('refresh')
}

// 处理删除模型 - 打开确认对话框
function handleDeleteModel(model: Model) {
  modelToDelete.value = model
  deleteModelConfirmOpen.value = true
}

// 确认删除模型
async function confirmDeleteModel() {
  if (!provider.value || !modelToDelete.value) return

  try {
    await deleteModelAPI(provider.value.id, modelToDelete.value.id)
    showSuccess('已移除该提供商对此模型的支持')
    deleteModelConfirmOpen.value = false
    modelToDelete.value = null
    // 重新加载 Provider 数据以刷新模型列表
    await loadProvider()
    emit('refresh')
  } catch (err: any) {
    showError(err.response?.data?.detail || '删除失败', '错误')
  }
}

// ===== 拖动排序处理 =====
function handleDragStart(event: DragEvent, key: EndpointAPIKey, endpoint: ProviderEndpointWithKeys) {
  dragState.value.isDragging = true
  dragState.value.draggedKeyId = key.id
  dragState.value.dragEndpointId = endpoint.id
  if (event.dataTransfer) {
    event.dataTransfer.effectAllowed = 'move'
  }
}

function handleDragEnd() {
  dragState.value.isDragging = false
  dragState.value.draggedKeyId = null
  dragState.value.targetKeyId = null
  dragState.value.dragEndpointId = null
}

function handleDragOver(event: DragEvent, targetKey: EndpointAPIKey) {
  event.preventDefault()
  if (event.dataTransfer) {
    event.dataTransfer.dropEffect = 'move'
  }
  if (dragState.value.draggedKeyId !== targetKey.id) {
    dragState.value.targetKeyId = targetKey.id
  }
}

function handleDragLeave() {
  dragState.value.targetKeyId = null
}

async function handleDrop(event: DragEvent, targetKey: EndpointAPIKey, endpoint: ProviderEndpointWithKeys) {
  event.preventDefault()

  const draggedKeyId = dragState.value.draggedKeyId
  if (!draggedKeyId || !endpoint.keys || draggedKeyId === targetKey.id) {
    handleDragEnd()
    return
  }

  // 只允许在同一端点内拖动
  if (dragState.value.dragEndpointId !== endpoint.id) {
    showError('不能跨端点拖动密钥')
    handleDragEnd()
    return
  }

  const keys = [...endpoint.keys]
  const draggedIndex = keys.findIndex(k => k.id === draggedKeyId)
  const targetIndex = keys.findIndex(k => k.id === targetKey.id)

  if (draggedIndex === -1 || targetIndex === -1) {
    handleDragEnd()
    return
  }

  // 记录原始优先级分组（排除被拖动的密钥）
  // key: 原始优先级值, value: 密钥ID数组
  const originalGroups = new Map<number, string[]>()
  for (const key of keys) {
    if (key.id === draggedKeyId) continue // 被拖动的密钥离开原组
    const priority = key.internal_priority ?? 0
    if (!originalGroups.has(priority)) {
      originalGroups.set(priority, [])
    }
    originalGroups.get(priority)!.push(key.id)
  }

  // 重排数组
  const [removed] = keys.splice(draggedIndex, 1)
  keys.splice(targetIndex, 0, removed)
  endpoint.keys = keys

  // 按新顺序为每个组分配新的优先级
  // 同组的密钥保持相同的优先级
  const priorities: { key_id: string; internal_priority: number }[] = []
  const groupNewPriority = new Map<number, number>() // 原优先级 -> 新优先级
  let currentPriority = 0

  for (const key of keys) {
    if (key.id === draggedKeyId) {
      // 被拖动的密钥是独立的新组，获得当前优先级
      priorities.push({ key_id: key.id, internal_priority: currentPriority })
      currentPriority++
    } else {
      const originalPriority = key.internal_priority ?? 0
      
      if (groupNewPriority.has(originalPriority)) {
        // 这个组已经分配过优先级，使用相同的值
        priorities.push({ key_id: key.id, internal_priority: groupNewPriority.get(originalPriority)! })
      } else {
        // 这个组第一次出现，分配新优先级
        groupNewPriority.set(originalPriority, currentPriority)
        priorities.push({ key_id: key.id, internal_priority: currentPriority })
        currentPriority++
      }
    }
  }

  handleDragEnd()

  // 调用 API 批量更新（使用循环调用 updateProviderKey 替代已废弃的 batchUpdateKeyPriority）
  try {
    await Promise.all(
      priorities.map(p => updateProviderKey(p.key_id, { internal_priority: p.internal_priority }))
    )
    showSuccess('优先级已更新')
    // 重新加载以获取更新后的数据
    await loadEndpoints()
    emit('refresh')
  } catch (err: any) {
    showError(err.response?.data?.detail || '更新优先级失败', '错误')
    // 回滚 - 重新加载
    await loadEndpoints()
  }
}

// ===== 点击编辑优先级 =====
function startEditPriority(key: EndpointAPIKey) {
  editingPriorityKey.value = key.id
  editingPriorityValue.value = key.internal_priority ?? 0
  prioritySaving.value = false
  nextTick(() => {
    // v-for 中的 ref 是数组，取第一个元素
    const input = Array.isArray(priorityInputRef.value) ? priorityInputRef.value[0] : priorityInputRef.value
    input?.focus()
    input?.select()
  })
}

function cancelEditPriority() {
  editingPriorityKey.value = null
  prioritySaving.value = false
}

function handlePriorityKeydown(e: KeyboardEvent, key: EndpointAPIKey) {
  if (e.key === 'Enter') {
    e.preventDefault()
    e.stopPropagation()
    if (!prioritySaving.value) {
      prioritySaving.value = true
      savePriority(key)
    }
  } else if (e.key === 'Escape') {
    e.preventDefault()
    cancelEditPriority()
  }
}

function handlePriorityBlur(key: EndpointAPIKey) {
  // 如果已经在保存中（Enter触发），不重复保存
  if (prioritySaving.value) return
  savePriority(key)
}

async function savePriority(key: EndpointAPIKey) {
  const keyId = editingPriorityKey.value
  const newPriority = parseInt(String(editingPriorityValue.value), 10) || 0

  if (!keyId || newPriority < 0) {
    cancelEditPriority()
    return
  }

  // 如果优先级没有变化，直接取消编辑
  if (key.internal_priority === newPriority) {
    cancelEditPriority()
    return
  }

  cancelEditPriority()

  try {
    await updateProviderKey(keyId, { internal_priority: newPriority })
    showSuccess('优先级已更新')
    // 更新本地数据 - 更新 providerKeys 中的数据
    const keyToUpdate = providerKeys.value.find(k => k.id === keyId)
    if (keyToUpdate) {
      keyToUpdate.internal_priority = newPriority
    }
    // 重新排序
    providerKeys.value.sort((a, b) => (a.internal_priority ?? 0) - (b.internal_priority ?? 0))
    emit('refresh')
  } catch (err: any) {
    showError(err.response?.data?.detail || '更新优先级失败', '错误')
  }
}

// ===== 密钥列表拖拽排序 =====
function handleKeyDragStart(event: DragEvent, index: number) {
  keyDragState.value.isDragging = true
  keyDragState.value.draggedIndex = index
  if (event.dataTransfer) {
    event.dataTransfer.effectAllowed = 'move'
    event.dataTransfer.setData('text/plain', String(index))
  }
}

function handleKeyDragEnd() {
  keyDragState.value.isDragging = false
  keyDragState.value.draggedIndex = null
  keyDragState.value.targetIndex = null
}

function handleKeyDragOver(event: DragEvent, index: number) {
  event.preventDefault()
  if (event.dataTransfer) {
    event.dataTransfer.dropEffect = 'move'
  }
  if (keyDragState.value.draggedIndex !== index) {
    keyDragState.value.targetIndex = index
  }
}

function handleKeyDragLeave() {
  keyDragState.value.targetIndex = null
}

async function handleKeyDrop(event: DragEvent, targetIndex: number) {
  event.preventDefault()

  const draggedIndex = keyDragState.value.draggedIndex
  if (draggedIndex === null || draggedIndex === targetIndex) {
    handleKeyDragEnd()
    return
  }

  const keys = allKeys.value.map(item => item.key)
  if (draggedIndex < 0 || draggedIndex >= keys.length || targetIndex < 0 || targetIndex >= keys.length) {
    handleKeyDragEnd()
    return
  }

  const draggedKey = keys[draggedIndex]
  const targetKey = keys[targetIndex]
  const draggedPriority = draggedKey.internal_priority ?? 0
  const targetPriority = targetKey.internal_priority ?? 0

  // 如果是同组内拖拽（同优先级），忽略操作
  if (draggedPriority === targetPriority) {
    handleKeyDragEnd()
    return
  }

  // 检查目标 key 是否属于一个"组"（除了被拖拽的 key，还有其他 key 与目标同优先级）
  // 组的定义：2 个及以上同优先级的 key
  const keysAtTargetPriority = keys.filter(k =>
    k.id !== draggedKey.id && (k.internal_priority ?? 0) === targetPriority
  )
  // 如果有 2 个及以上 key 在目标优先级（不含被拖拽的），说明目标在组内
  const targetIsInGroup = keysAtTargetPriority.length >= 2

  handleKeyDragEnd()

  try {
    if (targetIsInGroup) {
      // 目标在组内，被拖拽的 key 加入该组
      await updateProviderKey(draggedKey.id, { internal_priority: targetPriority })
    } else {
      // 目标是单独的（或只有目标自己），交换优先级
      await Promise.all([
        updateProviderKey(draggedKey.id, { internal_priority: targetPriority }),
        updateProviderKey(targetKey.id, { internal_priority: draggedPriority })
      ])
    }
    showSuccess('优先级已更新')
    await loadEndpoints()
    emit('refresh')
  } catch (err: any) {
    showError(err.response?.data?.detail || '更新优先级失败', '错误')
    await loadEndpoints()
  }
}

// 格式化探测时间
function formatProbeTime(probeTime: string): string {
  if (!probeTime) return '-'
  const now = new Date()
  const probe = new Date(probeTime)
  const diffMs = probe.getTime() - now.getTime()

  if (diffMs < 0) return '待探测'

  const diffMinutes = Math.floor(diffMs / 60000)
  const diffHours = Math.floor(diffMinutes / 60)
  const diffDays = Math.floor(diffHours / 24)

  if (diffDays > 0) return `${diffDays}天后`
  if (diffHours > 0) return `${diffHours}小时后`
  if (diffMinutes > 0) return `${diffMinutes}分钟后`
  return '即将探测'
}

// 获取密钥的 API 格式列表（按指定顺序排序）
function getKeyApiFormats(key: EndpointAPIKey, endpoint?: ProviderEndpointWithKeys): string[] {
  let formats: string[] = []
  if (key.api_formats && key.api_formats.length > 0) {
    formats = [...key.api_formats]
  } else if (endpoint) {
    formats = [endpoint.api_format]
  }
  // 使用统一的排序函数
  return sortApiFormats(formats)
}

// 获取密钥在指定 API 格式下的成本倍率
function getKeyRateMultiplier(key: EndpointAPIKey, format: string): number {
  // 优先使用 rate_multipliers 中指定格式的倍率
  if (key.rate_multipliers && key.rate_multipliers[format] !== undefined) {
    return key.rate_multipliers[format]
  }
  // 回退到默认倍率
  return key.rate_multiplier || 1.0
}

// 健康度颜色
function getHealthScoreColor(score: number): string {
  if (score >= 0.8) return 'text-green-600 dark:text-green-400'
  if (score >= 0.5) return 'text-yellow-600 dark:text-yellow-400'
  return 'text-red-600 dark:text-red-400'
}

function getHealthScoreBarColor(score: number): string {
  if (score >= 0.8) return 'bg-green-500 dark:bg-green-400'
  if (score >= 0.5) return 'bg-yellow-500 dark:bg-yellow-400'
  return 'bg-red-500 dark:bg-red-400'
}

// 加载 Provider 信息
async function loadProvider() {
  if (!props.providerId) return

  try {
    loading.value = true
    provider.value = await getProvider(props.providerId)

    if (!provider.value) {
      throw new Error('Provider 不存在')
    }
  } catch (err: any) {
    showError(err.response?.data?.detail || err.message || '加载失败', '错误')
  } finally {
    loading.value = false
  }
}

// 加载端点列表
async function loadEndpoints() {
  if (!props.providerId) return

  try {
    // 并行加载端点列表和 Provider 级别的 keys
    const [endpointsList, providerKeysResult] = await Promise.all([
      getProviderEndpoints(props.providerId),
      getProviderKeys(props.providerId).catch(() => []),
    ])

    providerKeys.value = providerKeysResult
    // 按 API 格式排序
    endpoints.value = endpointsList.sort((a, b) => {
      const aIdx = API_FORMAT_ORDER.indexOf(a.api_format)
      const bIdx = API_FORMAT_ORDER.indexOf(b.api_format)
      if (aIdx === -1 && bIdx === -1) return 0
      if (aIdx === -1) return 1
      if (bIdx === -1) return -1
      return aIdx - bIdx
    })
  } catch (err: any) {
    showError(err.response?.data?.detail || '加载端点失败', '错误')
  }
}

// 添加 ESC 键监听
useEscapeKey(() => {
  if (props.open) {
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
</style>
