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
            <div class="sticky top-0 z-10 bg-background border-b p-4 sm:p-6">
              <div class="flex items-start justify-between gap-3 sm:gap-4">
                <div class="space-y-1 flex-1 min-w-0">
                  <div class="flex items-center gap-2">
                    <h2 class="text-lg sm:text-xl font-bold truncate">
                      {{ provider.display_name }}
                    </h2>
                    <Badge
                      :variant="provider.is_active ? 'default' : 'secondary'"
                      class="text-xs shrink-0"
                    >
                      {{ provider.is_active ? '活跃' : '已停用' }}
                    </Badge>
                  </div>
                  <div class="flex items-center gap-2 flex-wrap">
                    <span class="text-sm text-muted-foreground font-mono">{{ provider.name }}</span>
                    <template v-if="provider.website">
                      <span class="text-muted-foreground">·</span>
                      <a
                        :href="provider.website"
                        target="_blank"
                        rel="noopener noreferrer"
                        class="text-xs text-primary hover:underline truncate"
                        title="访问官网"
                      >
                        {{ provider.website }}
                      </a>
                    </template>
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

              <!-- 端点与密钥管理 -->
              <Card class="overflow-hidden">
                <div class="p-4 border-b border-border/60">
                  <div class="flex items-center justify-between">
                    <h3 class="text-sm font-semibold flex items-center gap-2">
                      <span>端点与密钥管理</span>
                    </h3>
                    <Button
                      variant="outline"
                      size="sm"
                      class="h-8"
                      @click="showAddEndpointDialog"
                    >
                      <Plus class="w-3.5 h-3.5 mr-1.5" />
                      添加端点
                    </Button>
                  </div>
                </div>

                <!-- 端点列表 -->
                <div
                  v-if="endpoints.length > 0"
                  class="divide-y divide-border/40"
                >
                  <div
                    v-for="endpoint in endpoints"
                    :key="endpoint.id"
                    class="group"
                  >
                    <!-- 端点头部 - 可点击展开/收起 -->
                    <div
                      class="p-4 hover:bg-muted/30 transition-colors cursor-pointer"
                      @click="toggleEndpoint(endpoint.id)"
                    >
                      <div class="flex items-center justify-between">
                        <div class="flex items-center gap-3 flex-1 min-w-0">
                          <ChevronRight
                            class="w-4 h-4 text-muted-foreground transition-transform shrink-0"
                            :class="{ 'rotate-90': expandedEndpoints.has(endpoint.id) }"
                          />
                          <div class="flex-1 min-w-0">
                            <div class="flex items-center gap-2">
                              <span class="text-sm font-medium">{{ endpoint.api_format }}</span>
                              <Badge
                                v-if="!endpoint.is_active"
                                variant="secondary"
                                class="text-[10px] px-1.5 py-0"
                              >
                                已停用
                              </Badge>
                              <span class="text-xs text-muted-foreground flex items-center gap-1">
                                <Key class="w-3 h-3" />
                                {{ endpoint.keys?.filter((k: EndpointAPIKey) => k.is_active).length || 0 }}
                              </span>
                              <span
                                v-if="endpoint.max_retries"
                                class="text-xs text-muted-foreground"
                              >
                                {{ endpoint.max_retries }}次重试
                              </span>
                              <span
                                v-if="endpoint.timeout"
                                class="text-xs text-muted-foreground"
                              >
                                {{ endpoint.timeout }}s
                              </span>
                            </div>
                            <div class="flex items-center gap-1.5 mt-0.5">
                              <span class="text-xs text-muted-foreground font-mono truncate">
                                {{ endpoint.base_url }}
                              </span>
                              <Button
                                variant="ghost"
                                size="icon"
                                class="h-5 w-5 shrink-0"
                                title="复制 Base URL"
                                @click.stop="copyToClipboard(endpoint.base_url)"
                              >
                                <Copy class="w-3 h-3" />
                              </Button>
                            </div>
                          </div>
                        </div>
                        <div
                          class="flex items-center gap-1"
                          @click.stop
                        >
                          <Button
                            v-if="hasUnhealthyKeys(endpoint)"
                            variant="ghost"
                            size="icon"
                            class="h-8 w-8 text-green-600"
                            title="恢复所有密钥健康状态"
                            :disabled="recoveringEndpointId === endpoint.id"
                            @click="handleRecoverAllKeys(endpoint)"
                          >
                            <Loader2
                              v-if="recoveringEndpointId === endpoint.id"
                              class="w-3.5 h-3.5 animate-spin"
                            />
                            <RefreshCw
                              v-else
                              class="w-3.5 h-3.5"
                            />
                          </Button>
                          <Button
                            variant="ghost"
                            size="icon"
                            class="h-8 w-8"
                            title="添加密钥"
                            @click="handleAddKey(endpoint)"
                          >
                            <Plus class="w-4 h-4" />
                          </Button>
                          <Button
                            variant="ghost"
                            size="icon"
                            class="h-8 w-8"
                            title="编辑端点"
                            @click="handleEditEndpoint(endpoint)"
                          >
                            <Edit class="w-3.5 h-3.5" />
                          </Button>
                          <Button
                            variant="ghost"
                            size="icon"
                            class="h-8 w-8"
                            :disabled="togglingEndpointId === endpoint.id"
                            :title="endpoint.is_active ? '点击停用' : '点击启用'"
                            @click="toggleEndpointActive(endpoint)"
                          >
                            <Power class="w-3.5 h-3.5" />
                          </Button>
                          <Button
                            variant="ghost"
                            size="icon"
                            class="h-8 w-8"
                            title="删除端点"
                            @click="handleDeleteEndpoint(endpoint)"
                          >
                            <Trash2 class="w-3.5 h-3.5" />
                          </Button>
                        </div>
                      </div>
                    </div>

                    <!-- 端点详情 - 可展开区域 -->
                    <div
                      v-if="expandedEndpoints.has(endpoint.id)"
                      class="px-4 pb-4 bg-muted/20 border-t border-border/40"
                    >
                      <div class="space-y-3 pt-3">
                        <!-- 端点配置信息 -->
                        <div
                          v-if="endpoint.custom_path || endpoint.rpm_limit"
                          class="flex flex-wrap gap-x-4 gap-y-1 text-xs"
                        >
                          <div v-if="endpoint.custom_path">
                            <span class="text-muted-foreground">自定义路径:</span>
                            <span class="ml-1 font-mono">{{ endpoint.custom_path }}</span>
                          </div>
                          <div v-if="endpoint.rpm_limit">
                            <span class="text-muted-foreground">RPM:</span>
                            <span class="ml-1 font-medium">{{ endpoint.rpm_limit }}</span>
                          </div>
                        </div>

                        <!-- 密钥列表 -->
                        <div class="space-y-2">
                          <div
                            v-if="endpoint.keys && endpoint.keys.length > 0"
                            class="space-y-2"
                          >
                            <div
                              v-for="key in endpoint.keys"
                              :key="key.id"
                              draggable="true"
                              class="p-3 bg-background rounded-md border transition-all duration-150 group/key"
                              :class="{
                                'border-border/40 hover:border-border/80': dragState.targetKeyId !== key.id,
                                'border-primary border-2 bg-primary/5': dragState.targetKeyId === key.id,
                                'opacity-50': dragState.draggedKeyId === key.id,
                                'cursor-grabbing': dragState.isDragging
                              }"
                              @dragstart="handleDragStart($event, key, endpoint)"
                              @dragend="handleDragEnd"
                              @dragover="handleDragOver($event, key)"
                              @dragleave="handleDragLeave"
                              @drop="handleDrop($event, key, endpoint)"
                            >
                              <!-- 密钥主要信息行 -->
                              <div class="flex items-center justify-between mb-2">
                                <div class="flex items-center gap-2 flex-1 min-w-0">
                                  <!-- 拖动手柄 -->
                                  <div
                                    class="cursor-grab active:cursor-grabbing text-muted-foreground/50 hover:text-muted-foreground"
                                    title="拖动排序"
                                  >
                                    <GripVertical class="w-4 h-4" />
                                  </div>
                                  <div class="min-w-0">
                                    <div class="flex items-center gap-1.5">
                                      <span class="text-xs font-medium truncate">{{ key.name || '未命名密钥' }}</span>
                                      <Badge
                                        :variant="key.is_active ? 'default' : 'secondary'"
                                        class="text-[10px] px-1.5 py-0 shrink-0"
                                      >
                                        {{ key.is_active ? '活跃' : '禁用' }}
                                      </Badge>
                                    </div>
                                    <div class="text-[10px] font-mono text-muted-foreground truncate">
                                      {{ key.api_key_masked }}
                                    </div>
                                  </div>
                                  <div class="flex items-center gap-1.5 ml-auto shrink-0">
                                    <div
                                      v-if="key.health_score !== undefined"
                                      class="flex items-center gap-1"
                                    >
                                      <div class="w-12 h-1 bg-muted/80 rounded-full overflow-hidden">
                                        <div
                                          class="h-full transition-all duration-300"
                                          :class="getHealthScoreBarColor(key.health_score || 0)"
                                          :style="{ width: `${(key.health_score || 0) * 100}%` }"
                                        />
                                      </div>
                                      <span
                                        class="text-[10px] font-bold tabular-nums w-[30px] text-right"
                                        :class="getHealthScoreColor(key.health_score || 0)"
                                      >
                                        {{ ((key.health_score || 0) * 100).toFixed(0) }}%
                                      </span>
                                    </div>
                                    <Badge
                                      v-if="key.circuit_breaker_open"
                                      variant="destructive"
                                      class="text-[10px] px-1.5 py-0"
                                    >
                                      熔断
                                    </Badge>
                                  </div>
                                </div>
                                <div class="flex items-center gap-1 ml-2">
                                  <Button
                                    v-if="key.circuit_breaker_open || (key.health_score !== undefined && key.health_score < 0.5)"
                                    variant="ghost"
                                    size="icon"
                                    class="h-7 w-7 text-green-600"
                                    title="刷新健康状态"
                                    @click="handleRecoverKey(key)"
                                  >
                                    <RefreshCw class="w-3 h-3" />
                                  </Button>
                                  <Button
                                    variant="ghost"
                                    size="icon"
                                    class="h-7 w-7"
                                    title="配置允许的模型"
                                    @click="handleConfigKeyModels(key)"
                                  >
                                    <Layers class="w-3 h-3" />
                                  </Button>
                                  <Button
                                    variant="ghost"
                                    size="icon"
                                    class="h-7 w-7"
                                    title="编辑密钥"
                                    @click="handleEditKey(endpoint, key)"
                                  >
                                    <Edit class="w-3 h-3" />
                                  </Button>
                                  <Button
                                    variant="ghost"
                                    size="icon"
                                    class="h-7 w-7"
                                    :disabled="togglingKeyId === key.id"
                                    :title="key.is_active ? '点击停用' : '点击启用'"
                                    @click="toggleKeyActive(key)"
                                  >
                                    <Power class="w-3 h-3" />
                                  </Button>
                                  <Button
                                    variant="ghost"
                                    size="icon"
                                    class="h-7 w-7"
                                    title="删除密钥"
                                    @click="handleDeleteKey(key)"
                                  >
                                    <Trash2 class="w-3 h-3" />
                                  </Button>
                                </div>
                              </div>

                              <!-- 密钥详细信息 -->
                              <div class="flex items-center text-[11px]">
                                <!-- 左侧固定信息 -->
                                <div class="flex items-center gap-2">
                                  <!-- 可点击编辑的优先级 -->
                                  <span
                                    v-if="editingPriorityKey !== key.id"
                                    class="text-muted-foreground cursor-pointer hover:text-foreground hover:bg-muted/50 px-1 rounded transition-colors"
                                    title="点击编辑优先级，数字越小优先级越高"
                                    @click="startEditPriority(key)"
                                  >
                                    P {{ key.internal_priority }}
                                  </span>
                                  <!-- 编辑模式 -->
                                  <span
                                    v-else
                                    class="flex items-center gap-1"
                                  >
                                    <span class="text-muted-foreground">P</span>
                                    <input
                                      ref="priorityInput"
                                      v-model.number="editingPriorityValue"
                                      type="number"
                                      class="w-12 h-5 px-1 text-[11px] border rounded bg-background focus:outline-none focus:ring-1 focus:ring-primary"
                                      min="0"
                                      @keyup.enter="savePriority(key, endpoint)"
                                      @keyup.escape="cancelEditPriority"
                                      @blur="savePriority(key, endpoint)"
                                    >
                                  </span>
                                  <span
                                    class="text-muted-foreground"
                                    title="成本倍率，实际成本 = 模型价格 × 倍率"
                                  >
                                    {{ key.rate_multiplier }}x
                                  </span>
                                  <span
                                    v-if="key.success_rate !== undefined"
                                    class="text-muted-foreground"
                                    title="成功率 = 成功次数 / 总请求数"
                                  >
                                    {{ (key.success_rate * 100).toFixed(1) }}% ({{ key.success_count }}/{{ key.request_count }})
                                  </span>
                                </div>
                                <!-- 右侧动态信息 -->
                                <div class="flex items-center gap-2 ml-auto">
                                  <span
                                    v-if="key.next_probe_at"
                                    class="text-amber-600 dark:text-amber-400"
                                    title="熔断器探测恢复时间"
                                  >
                                    {{ formatProbeTime(key.next_probe_at) }}探测
                                  </span>
                                  <span
                                    v-if="key.rate_limit"
                                    class="text-muted-foreground"
                                    title="每分钟请求数限制"
                                  >
                                    {{ key.rate_limit }}rpm
                                  </span>
                                  <span
                                    v-if="key.max_concurrent || key.is_adaptive"
                                    class="text-muted-foreground"
                                    :title="key.is_adaptive ? `自适应并发限制（学习值: ${key.learned_max_concurrent ?? '未学习'}）` : `固定并发限制: ${key.max_concurrent}`"
                                  >
                                    {{ key.is_adaptive ? '自适应' : '固定' }}并发: {{ key.is_adaptive ? (key.learned_max_concurrent ?? '学习中') : key.max_concurrent }}
                                  </span>
                                </div>
                              </div>
                            </div>
                          </div>
                          <div
                            v-else
                            class="text-xs text-muted-foreground text-center py-4"
                          >
                            暂无密钥
                          </div>
                        </div>
                      </div>
                    </div>
                  </div>
                </div>

                <!-- 空状态 -->
                <div
                  v-else
                  class="p-8 text-center text-muted-foreground"
                >
                  <Server class="w-12 h-12 mx-auto mb-3 opacity-50" />
                  <p class="text-sm">
                    暂无端点配置
                  </p>
                  <p class="text-xs mt-1">
                    点击上方"添加端点"按钮创建第一个端点
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

  <!-- 端点表单对话框（添加/编辑） -->
  <EndpointFormDialog
    v-if="provider && open"
    v-model="endpointDialogOpen"
    :provider="provider"
    :endpoint="endpointToEdit"
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
    @close="keyFormDialogOpen = false"
    @saved="handleKeyChanged"
  />

  <!-- 密钥允许模型配置对话框 -->
  <KeyAllowedModelsDialog
    v-if="open"
    :open="keyAllowedModelsDialogOpen"
    :api-key="editingKey"
    :provider-id="provider ? provider.id : null"
    @close="keyAllowedModelsDialogOpen = false"
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
    :provider-name="provider.display_name"
    :editing-model="editingModel"
    @update:open="modelFormDialogOpen = $event"
    @saved="handleModelSaved"
  />

  <!-- 删除模型确认对话框 -->
  <AlertDialog
    v-if="open"
    :model-value="deleteModelConfirmOpen"
    title="移除模型支持"
    :description="`确定要移除提供商 ${provider?.display_name} 对模型 ${modelToDelete?.global_model_display_name || modelToDelete?.provider_model_name} 的支持吗？这不会删除全局模型，只是该提供商将不再支持此模型。`"
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
    :provider-name="provider.display_name"
    :provider-identifier="provider.name"
    @update:open="batchAssignDialogOpen = $event"
    @changed="handleBatchAssignChanged"
  />
</template>

<script setup lang="ts">
import { ref, watch, computed } from 'vue'
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
  Layers,
  GripVertical,
  Copy
} from 'lucide-vue-next'
import { useEscapeKey } from '@/composables/useEscapeKey'
import Button from '@/components/ui/button.vue'
import Badge from '@/components/ui/badge.vue'
import Card from '@/components/ui/card.vue'
import { useToast } from '@/composables/useToast'
import { getProvider, getProviderEndpoints } from '@/api/endpoints'
import {
  KeyFormDialog,
  KeyAllowedModelsDialog,
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
  getEndpointKeys,
  updateEndpoint,
  updateEndpointKey,
  batchUpdateKeyPriority,
  type ProviderEndpoint,
  type EndpointAPIKey,
  type Model
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

const loading = ref(false)
const provider = ref<any>(null)
const endpoints = ref<ProviderEndpointWithKeys[]>([])
const expandedEndpoints = ref<Set<string>>(new Set())

// 端点相关状态
const endpointDialogOpen = ref(false)
const endpointToEdit = ref<ProviderEndpoint | null>(null)
const deleteEndpointConfirmOpen = ref(false)
const endpointToDelete = ref<ProviderEndpoint | null>(null)

// 密钥相关状态
const keyFormDialogOpen = ref(false)
const keyAllowedModelsDialogOpen = ref(false)
const currentEndpoint = ref<ProviderEndpoint | null>(null)
const editingKey = ref<EndpointAPIKey | null>(null)
const deleteKeyConfirmOpen = ref(false)
const keyToDelete = ref<EndpointAPIKey | null>(null)
const recoveringEndpointId = ref<string | null>(null)
const togglingEndpointId = ref<string | null>(null)
const togglingKeyId = ref<string | null>(null)

// 模型相关状态
const modelFormDialogOpen = ref(false)
const editingModel = ref<Model | null>(null)
const deleteModelConfirmOpen = ref(false)
const modelToDelete = ref<Model | null>(null)
const batchAssignDialogOpen = ref(false)

// ModelAliasesTab 组件引用
const modelAliasesTabRef = ref<InstanceType<typeof ModelAliasesTab> | null>(null)

// 拖动排序相关状态
const dragState = ref({
  isDragging: false,
  draggedKeyId: null as string | null,
  targetKeyId: null as string | null,
  dragEndpointId: null as string | null
})

// 点击编辑优先级相关状态
const editingPriorityKey = ref<string | null>(null)
const editingPriorityValue = ref<number>(0)

// 任意模态窗口打开时,阻止抽屉被误关闭
const hasBlockingDialogOpen = computed(() =>
  endpointDialogOpen.value ||
  deleteEndpointConfirmOpen.value ||
  keyFormDialogOpen.value ||
  keyAllowedModelsDialogOpen.value ||
  deleteKeyConfirmOpen.value ||
  modelFormDialogOpen.value ||
  deleteModelConfirmOpen.value ||
  batchAssignDialogOpen.value ||
  // 检测 ModelAliasesTab 子组件的 Dialog 是否打开
  modelAliasesTabRef.value?.dialogOpen
)

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
    expandedEndpoints.value.clear()

    // 重置所有对话框状态
    endpointDialogOpen.value = false
    deleteEndpointConfirmOpen.value = false
    keyFormDialogOpen.value = false
    keyAllowedModelsDialogOpen.value = false
    deleteKeyConfirmOpen.value = false
    batchAssignDialogOpen.value = false

    // 重置临时数据
    endpointToEdit.value = null
    endpointToDelete.value = null
    currentEndpoint.value = null
    editingKey.value = null
    keyToDelete.value = null
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

// 显示添加端点对话框
function showAddEndpointDialog() {
  endpointToEdit.value = null  // 添加模式
  endpointDialogOpen.value = true
}

// ===== 端点事件处理 =====
function handleEditEndpoint(endpoint: ProviderEndpoint) {
  endpointToEdit.value = endpoint  // 编辑模式
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
  await loadEndpoints()
  emit('refresh')
  endpointToEdit.value = null
}

// ===== 密钥事件处理 =====
function handleAddKey(endpoint: ProviderEndpoint) {
  currentEndpoint.value = endpoint
  editingKey.value = null
  keyFormDialogOpen.value = true
}

function handleEditKey(endpoint: ProviderEndpoint, key: EndpointAPIKey) {
  currentEndpoint.value = endpoint
  editingKey.value = key
  keyFormDialogOpen.value = true
}

function handleConfigKeyModels(key: EndpointAPIKey) {
  editingKey.value = key
  keyAllowedModelsDialogOpen.value = true
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
    await updateEndpointKey(key.id, { is_active: newStatus })
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

  // 调用 API 批量更新
  try {
    await batchUpdateKeyPriority(endpoint.id, priorities)
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
}

function cancelEditPriority() {
  editingPriorityKey.value = null
}

async function savePriority(key: EndpointAPIKey, endpoint: ProviderEndpointWithKeys) {
  const keyId = editingPriorityKey.value
  const newPriority = editingPriorityValue.value

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
    await updateEndpointKey(keyId, { internal_priority: newPriority })
    showSuccess('优先级已更新')
    // 更新本地数据
    if (endpoint.keys) {
      const keyToUpdate = endpoint.keys.find(k => k.id === keyId)
      if (keyToUpdate) {
        keyToUpdate.internal_priority = newPriority
      }
      // 重新排序
      endpoint.keys.sort((a, b) => (a.internal_priority ?? 0) - (b.internal_priority ?? 0))
    }
    emit('refresh')
  } catch (err: any) {
    showError(err.response?.data?.detail || '更新优先级失败', '错误')
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

// 复制到剪贴板
async function copyToClipboard(text: string) {
  try {
    await navigator.clipboard.writeText(text)
    showSuccess('已复制到剪贴板')
  } catch {
    showError('复制失败', '错误')
  }
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
    const endpointsList = await getProviderEndpoints(props.providerId)

    // 为每个端点加载其密钥
    const endpointsWithKeys = await Promise.all(
      endpointsList.map(async (endpoint) => {
        try {
          const keys = await getEndpointKeys(endpoint.id)
          return { ...endpoint, keys }
        } catch {
          // 如果获取密钥失败，返回空数组
          return { ...endpoint, keys: [] }
        }
      })
    )

    endpoints.value = endpointsWithKeys
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
