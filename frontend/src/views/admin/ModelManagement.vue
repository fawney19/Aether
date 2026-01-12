<template>
  <div class="flex flex-col h-[calc(100vh-12rem)]">
    <!-- 主内容区 -->
    <div class="flex-1 flex flex-col min-w-0">
      <!-- 模型列表 -->
      <Card class="overflow-hidden">
        <!-- 标题和操作栏 -->
        <div class="px-4 sm:px-6 py-3 sm:py-3.5 border-b border-border/60">
          <div class="flex flex-col sm:flex-row sm:items-center sm:justify-between gap-3 sm:gap-4">
            <!-- 左侧：标题 -->
            <h3 class="text-sm sm:text-base font-semibold shrink-0">
              模型管理
            </h3>

            <!-- 右侧：操作区 -->
            <div class="flex flex-wrap items-center gap-2">
              <!-- 搜索框 -->
              <div class="relative">
                <Search class="absolute left-2.5 top-1/2 -translate-y-1/2 h-3.5 w-3.5 text-muted-foreground/70 z-10 pointer-events-none" />
                <Input
                  id="model-search"
                  v-model="searchQuery"
                  type="text"
                  placeholder="搜索模型名称..."
                  class="w-32 sm:w-44 pl-8 pr-3 h-8 text-sm bg-muted/30 border-border/50 focus:border-primary/50 transition-colors"
                />
              </div>

              <div class="hidden sm:block h-4 w-px bg-border" />

              <!-- 能力筛选 -->
              <div class="flex items-center border rounded-md border-border/60 h-8 overflow-hidden">
                <button
                  class="px-2.5 h-full text-xs transition-colors"
                  :class="capabilityFilters.streaming ? 'bg-primary text-primary-foreground' : 'hover:bg-muted'"
                  title="流式输出"
                  @click="capabilityFilters.streaming = !capabilityFilters.streaming"
                >
                  <Zap class="w-3.5 h-3.5" />
                </button>
                <div class="w-px h-4 bg-border/60" />
                <button
                  class="px-2.5 h-full text-xs transition-colors"
                  :class="capabilityFilters.imageGeneration ? 'bg-primary text-primary-foreground' : 'hover:bg-muted'"
                  title="图像生成"
                  @click="capabilityFilters.imageGeneration = !capabilityFilters.imageGeneration"
                >
                  <Image class="w-3.5 h-3.5" />
                </button>
                <div class="w-px h-4 bg-border/60" />
                <button
                  class="px-2.5 h-full text-xs transition-colors"
                  :class="capabilityFilters.vision ? 'bg-primary text-primary-foreground' : 'hover:bg-muted'"
                  title="视觉理解"
                  @click="capabilityFilters.vision = !capabilityFilters.vision"
                >
                  <Eye class="w-3.5 h-3.5" />
                </button>
                <div class="w-px h-4 bg-border/60" />
                <button
                  class="px-2.5 h-full text-xs transition-colors"
                  :class="capabilityFilters.toolUse ? 'bg-primary text-primary-foreground' : 'hover:bg-muted'"
                  title="工具调用"
                  @click="capabilityFilters.toolUse = !capabilityFilters.toolUse"
                >
                  <Wrench class="w-3.5 h-3.5" />
                </button>
                <div class="w-px h-4 bg-border/60" />
                <button
                  class="px-2.5 h-full text-xs transition-colors"
                  :class="capabilityFilters.extendedThinking ? 'bg-primary text-primary-foreground' : 'hover:bg-muted'"
                  title="深度思考"
                  @click="capabilityFilters.extendedThinking = !capabilityFilters.extendedThinking"
                >
                  <Brain class="w-3.5 h-3.5" />
                </button>
              </div>

              <div class="hidden sm:block h-4 w-px bg-border" />

              <!-- 操作按钮 -->
              <Button
                variant="ghost"
                size="icon"
                class="h-8 w-8"
                title="创建模型"
                @click="openCreateModelDialog"
              >
                <Plus class="w-3.5 h-3.5" />
              </Button>
              <RefreshButton
                :loading="loading"
                @click="refreshData"
              />
            </div>
          </div>
        </div>

        <Table class="hidden xl:table">
          <TableHeader>
            <TableRow>
              <TableHead class="w-[240px]">
                模型名称
              </TableHead>
              <TableHead class="w-[140px]">
                能力/偏好
              </TableHead>
              <TableHead class="w-[160px] text-center">
                价格 ($/M)
              </TableHead>
              <TableHead class="w-[80px] text-center">
                提供商
              </TableHead>
              <TableHead class="w-[80px] text-center">
                调用次数
              </TableHead>
              <TableHead class="w-[70px]">
                状态
              </TableHead>
              <TableHead class="w-[140px] text-center">
                操作
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableRow v-if="loading">
              <TableCell
                colspan="7"
                class="text-center py-8"
              >
                <Loader2 class="w-6 h-6 animate-spin mx-auto" />
              </TableCell>
            </TableRow>
            <TableRow v-else-if="filteredGlobalModels.length === 0">
              <TableCell
                colspan="7"
                class="text-center py-8 text-muted-foreground"
              >
                没有找到匹配的模型
              </TableCell>
            </TableRow>
            <template v-else>
              <TableRow
                v-for="model in paginatedGlobalModels"
                :key="model.id"
                class="cursor-pointer hover:bg-muted/50 group"
                @mousedown="handleMouseDown"
                @click="handleRowClick($event, model)"
              >
                <TableCell>
                  <div>
                    <div class="font-medium">
                      {{ model.display_name }}
                    </div>
                    <div class="text-xs text-muted-foreground flex items-center gap-1">
                      <span>{{ model.name }}</span>
                      <button
                        class="p-0.5 rounded hover:bg-muted transition-colors"
                        title="复制模型 ID"
                        @click.stop="copyToClipboard(model.name)"
                      >
                        <Copy class="w-3 h-3" />
                      </button>
                    </div>
                  </div>
                </TableCell>
                <TableCell>
                  <div class="space-y-1 w-fit">
                    <div class="flex flex-wrap gap-1">
                      <Zap
                        v-if="model.config?.streaming !== false"
                        class="w-4 h-4 text-muted-foreground"
                        title="流式输出"
                      />
                      <Image
                        v-if="model.config?.image_generation === true"
                        class="w-4 h-4 text-muted-foreground"
                        title="图像生成"
                      />
                      <Eye
                        v-if="model.config?.vision === true"
                        class="w-4 h-4 text-muted-foreground"
                        title="视觉理解"
                      />
                      <Wrench
                        v-if="model.config?.function_calling === true"
                        class="w-4 h-4 text-muted-foreground"
                        title="工具调用"
                      />
                      <Brain
                        v-if="model.config?.extended_thinking === true"
                        class="w-4 h-4 text-muted-foreground"
                        title="深度思考"
                      />
                    </div>
                    <template v-if="model.supported_capabilities?.length">
                      <div class="border-t border-border/50" />
                      <div class="flex flex-wrap gap-0.5">
                        <span
                          v-for="capName in model.supported_capabilities"
                          :key="capName"
                          class="text-[11px] px-1 py-0.5 rounded bg-muted/60 text-muted-foreground"
                          :title="getCapabilityDisplayName(capName)"
                        >{{ getCapabilityShortName(capName) }}</span>
                      </div>
                    </template>
                  </div>
                </TableCell>
                <TableCell class="text-center">
                  <div class="text-xs space-y-0.5">
                    <!-- 按 Token 计费 -->
                    <div v-if="getFirstTierPrice(model, 'input') || getFirstTierPrice(model, 'output')">
                      <span class="text-muted-foreground">In:</span>
                      <span class="font-mono ml-1">{{ getFirstTierPrice(model, 'input')?.toFixed(2) || '-' }}</span>
                      <span class="text-muted-foreground mx-1">/</span>
                      <span class="text-muted-foreground">Out:</span>
                      <span class="font-mono ml-1">{{ getFirstTierPrice(model, 'output')?.toFixed(2) || '-' }}</span>
                      <!-- 阶梯计费标记 -->
                      <span
                        v-if="hasTieredPricing(model)"
                        class="ml-1 text-muted-foreground"
                        title="阶梯计费"
                      >[阶梯]</span>
                    </div>
                    <!-- 按次计费 -->
                    <div v-if="model.default_price_per_request && model.default_price_per_request > 0">
                      <span class="text-muted-foreground">按次:</span>
                      <span class="font-mono ml-1">${{ model.default_price_per_request.toFixed(3) }}/次</span>
                    </div>
                    <!-- 无计费配置 -->
                    <div
                      v-if="!getFirstTierPrice(model, 'input') && !getFirstTierPrice(model, 'output') && !model.default_price_per_request"
                      class="text-muted-foreground"
                    >
                      -
                    </div>
                  </div>
                </TableCell>
                <TableCell class="text-center">
                  <Badge variant="secondary">
                    {{ model.provider_count || 0 }}
                  </Badge>
                </TableCell>
                <TableCell class="text-center">
                  <span class="text-sm font-mono">{{ formatUsageCount(model.usage_count || 0) }}</span>
                </TableCell>
                <TableCell>
                  <Badge :variant="model.is_active ? 'default' : 'secondary'">
                    {{ model.is_active ? '活跃' : '停用' }}
                  </Badge>
                </TableCell>
                <TableCell>
                  <div class="flex items-center justify-center gap-1">
                    <Button
                      variant="ghost"
                      size="icon"
                      class="h-8 w-8"
                      title="查看详情"
                      @click.stop="selectModel(model)"
                    >
                      <Eye class="w-4 h-4" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon"
                      class="h-8 w-8"
                      title="编辑模型"
                      @click.stop="editModel(model)"
                    >
                      <Edit class="w-4 h-4" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon"
                      class="h-8 w-8"
                      :title="model.is_active ? '停用模型' : '启用模型'"
                      @click.stop="toggleModelStatus(model)"
                    >
                      <Power class="w-4 h-4" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon"
                      class="h-8 w-8"
                      title="删除模型"
                      @click.stop="deleteModel(model)"
                    >
                      <Trash2 class="w-4 h-4" />
                    </Button>
                  </div>
                </TableCell>
              </TableRow>
            </template>
          </TableBody>
        </Table>

        <!-- 移动端卡片列表 -->
        <div
          v-if="!loading && filteredGlobalModels.length > 0"
          class="xl:hidden divide-y divide-border/40"
        >
          <div
            v-for="model in paginatedGlobalModels"
            :key="model.id"
            class="p-4 space-y-3 hover:bg-muted/50 cursor-pointer transition-colors"
            @click="selectModel(model)"
          >
            <!-- 第一行：名称 + 状态 + 操作 -->
            <div class="flex items-start justify-between gap-3">
              <div class="flex-1 min-w-0">
                <div class="flex items-center gap-2">
                  <span class="font-medium truncate">{{ model.display_name }}</span>
                  <Badge
                    :variant="model.is_active ? 'default' : 'secondary'"
                    class="text-xs shrink-0"
                  >
                    {{ model.is_active ? '活跃' : '停用' }}
                  </Badge>
                </div>
                <div class="text-xs text-muted-foreground flex items-center gap-1 mt-0.5">
                  <span class="font-mono truncate">{{ model.name }}</span>
                  <button
                    class="p-0.5 rounded hover:bg-muted transition-colors shrink-0"
                    @click.stop="copyToClipboard(model.name)"
                  >
                    <Copy class="w-3 h-3" />
                  </button>
                </div>
              </div>
              <div
                class="flex items-center gap-0.5 shrink-0"
                @click.stop
              >
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-7 w-7"
                  @click="editModel(model)"
                >
                  <Edit class="w-3.5 h-3.5" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-7 w-7"
                  @click="toggleModelStatus(model)"
                >
                  <Power class="w-3.5 h-3.5" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-7 w-7"
                  @click="deleteModel(model)"
                >
                  <Trash2 class="w-3.5 h-3.5" />
                </Button>
              </div>
            </div>

            <!-- 第二行：能力图标 -->
            <div class="flex flex-wrap gap-1.5">
              <Zap
                v-if="model.config?.streaming !== false"
                class="w-4 h-4 text-muted-foreground"
              />
              <Image
                v-if="model.config?.image_generation === true"
                class="w-4 h-4 text-muted-foreground"
              />
              <Eye
                v-if="model.config?.vision === true"
                class="w-4 h-4 text-muted-foreground"
              />
              <Wrench
                v-if="model.config?.function_calling === true"
                class="w-4 h-4 text-muted-foreground"
              />
              <Brain
                v-if="model.config?.extended_thinking === true"
                class="w-4 h-4 text-muted-foreground"
              />
            </div>

            <!-- 第三行：统计信息 -->
            <div class="flex flex-wrap items-center gap-3 text-xs text-muted-foreground">
              <span>提供商 {{ model.provider_count || 0 }}</span>
              <span>调用 {{ formatUsageCount(model.usage_count || 0) }}</span>
              <span
                v-if="getFirstTierPrice(model, 'input') || getFirstTierPrice(model, 'output')"
                class="font-mono"
              >
                ${{ getFirstTierPrice(model, 'input')?.toFixed(2) || '-' }}/${{ getFirstTierPrice(model, 'output')?.toFixed(2) || '-' }}
              </span>
            </div>
          </div>
        </div>

        <!-- 分页 -->
        <Pagination
          v-if="!loading && filteredGlobalModels.length > 0"
          :current="catalogCurrentPage"
          :total="filteredGlobalModels.length"
          :page-size="catalogPageSize"
          @update:current="catalogCurrentPage = $event"
          @update:page-size="catalogPageSize = $event"
        />
      </Card>
    </div>

    <!-- 创建/编辑模型对话框 -->
    <GlobalModelFormDialog
      :open="createModelDialogOpen"
      :model="editingModel"
      @update:open="handleModelDialogUpdate"
      @success="handleModelFormSuccess"
    />

    <!-- 模型详情抽屉 -->
    <ModelDetailDrawer
      ref="modelDetailDrawerRef"
      :model="selectedModel"
      :open="!!selectedModel"
      :providers="selectedModelProviders"
      :loading-providers="loadingModelProviders"
      :has-blocking-dialog-open="hasBlockingDialogOpen"
      :capabilities="capabilities"
      @update:open="handleDrawerOpenChange"
      @edit-model="editModel"
      @toggle-model-status="toggleModelStatus"
      @add-provider="openAddProviderDialog"
      @edit-provider="openEditProviderImplementation"
      @delete-provider="confirmDeleteProviderImplementation"
      @toggle-provider-status="toggleProviderStatus"
      @refresh-providers="refreshSelectedModelProviders"
    />

    <!-- 批量添加关联提供商对话框 -->
    <Dialog
      :model-value="batchAddProvidersDialogOpen"
      :title="selectedModel ? `批量管理提供商 - ${selectedModel.display_name}` : '批量管理提供商'"
      description="选中的提供商将被关联到模型，取消选中将移除关联"
      :icon="Server"
      size="2xl"
      @update:model-value="handleBatchAddProvidersDialogUpdate"
    >
      <template #default>
        <div class="space-y-4">
          <!-- 搜索栏 -->
          <div class="flex items-center gap-2">
            <div class="flex-1 relative">
              <Search class="absolute left-2.5 top-1/2 -translate-y-1/2 w-4 h-4 text-muted-foreground" />
              <Input
                v-model="batchProviderSearchQuery"
                placeholder="搜索提供商..."
                class="pl-8 h-9"
              />
            </div>
          </div>

          <!-- 单列提供商列表 -->
          <div class="border rounded-lg overflow-hidden">
            <div class="max-h-96 overflow-y-auto">
              <div
                v-if="loadingProviderOptions"
                class="flex items-center justify-center py-12"
              >
                <Loader2 class="w-6 h-6 animate-spin text-primary" />
              </div>

              <template v-else>
                <!-- 提供商组 -->
                <div v-if="filteredBatchProviders.length > 0">
                  <div
                    class="flex items-center justify-between px-3 py-2 bg-muted sticky top-0 z-10"
                  >
                    <div class="flex items-center gap-2">
                      <span class="text-xs font-medium">提供商</span>
                      <span class="text-xs text-muted-foreground">({{ filteredBatchProviders.length }})</span>
                    </div>
                    <button
                      type="button"
                      class="text-xs text-primary hover:underline shrink-0"
                      @click="toggleAllBatchProviders"
                    >
                      {{ isAllBatchProvidersSelected ? '取消全选' : '全选' }}
                    </button>
                  </div>
                  <div class="space-y-1 p-2">
                    <div
                      v-for="provider in filteredBatchProviders"
                      :key="provider.id"
                      class="flex items-center gap-2 px-2 py-1.5 rounded hover:bg-muted cursor-pointer"
                      @click="toggleBatchProviderSelection(provider.id)"
                    >
                      <div
                        class="w-4 h-4 border rounded flex items-center justify-center shrink-0"
                        :class="isBatchProviderSelected(provider.id) ? 'bg-primary border-primary' : ''"
                      >
                        <Check
                          v-if="isBatchProviderSelected(provider.id)"
                          class="w-3 h-3 text-primary-foreground"
                        />
                      </div>
                      <div class="flex-1 min-w-0">
                        <p class="text-sm font-medium truncate">
                          {{ provider.name }}
                        </p>
                      </div>
                      <Badge
                        :variant="provider.is_active ? 'outline' : 'secondary'"
                        :class="provider.is_active ? 'text-green-600 border-green-500/60' : ''"
                        class="text-xs shrink-0"
                      >
                        {{ provider.is_active ? '活跃' : '停用' }}
                      </Badge>
                    </div>
                  </div>
                </div>

                <!-- 空状态 -->
                <div
                  v-if="filteredBatchProviders.length === 0"
                  class="flex flex-col items-center justify-center py-12 text-muted-foreground"
                >
                  <Building2 class="w-10 h-10 mb-2 opacity-30" />
                  <p class="text-sm">
                    {{ batchProviderSearchQuery ? '无匹配结果' : '暂无可用提供商' }}
                  </p>
                </div>
              </template>
            </div>
          </div>
        </div>
      </template>
      <template #footer>
        <div class="flex items-center justify-between w-full">
          <p class="text-xs text-muted-foreground">
            {{ hasBatchProviderChanges ? `${batchProviderPendingChangesCount} 项更改待保存` : '' }}
          </p>
          <div class="flex items-center gap-2">
            <Button
              :disabled="!hasBatchProviderChanges || submittingBatchProviders"
              @click="saveBatchProviderChanges"
            >
              <Loader2
                v-if="submittingBatchProviders"
                class="w-4 h-4 mr-1 animate-spin"
              />
              {{ submittingBatchProviders ? '保存中...' : '保存' }}
            </Button>
            <Button
              variant="outline"
              @click="closeBatchAddProvidersDialog"
            >
              关闭
            </Button>
          </div>
        </div>
      </template>
    </Dialog>

    <!-- 编辑提供商模型对话框 -->
    <ProviderModelFormDialog
      :open="editProviderDialogOpen"
      :provider-id="editingProvider?.id || ''"
      :provider-name="editingProvider?.name || ''"
      :editing-model="editingProviderModel"
      @update:open="handleEditProviderDialogUpdate"
      @saved="handleEditProviderSaved"
    />
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted, watch } from 'vue'
import {
  Plus,
  Edit,
  Trash2,
  Loader2,
  Eye,
  Wrench,
  Brain,
  Zap,
  Image,
  Building2,
  Search,
  Power,
  Copy,
  Server,
  Check,
} from 'lucide-vue-next'
import ModelDetailDrawer from '@/features/models/components/ModelDetailDrawer.vue'
import GlobalModelFormDialog from '@/features/models/components/GlobalModelFormDialog.vue'
import ProviderModelFormDialog from '@/features/providers/components/ProviderModelFormDialog.vue'
import type { Model } from '@/api/endpoints'
import { useToast } from '@/composables/useToast'
import { useConfirm } from '@/composables/useConfirm'
import { useClipboard } from '@/composables/useClipboard'
import { useRowClick } from '@/composables/useRowClick'
import { parseApiError } from '@/utils/errorParser'
import {
  Button,
  Card,
  Input,
  Table,
  TableHeader,
  TableBody,
  TableRow,
  TableHead,
  TableCell,
  Badge,
  Dialog,
  Pagination,
  RefreshButton,
} from '@/components/ui'
import {
  listGlobalModels,
  updateGlobalModel,
  deleteGlobalModel,
  batchAssignToProviders,
  getGlobalModelProviders,
  type GlobalModelResponse,
} from '@/api/global-models'
import { log } from '@/utils/logger'
import { getProvidersSummary } from '@/api/endpoints/providers'
import { getAllCapabilities, type CapabilityDefinition } from '@/api/endpoints'

const { success, error: showError } = useToast()
const { copyToClipboard } = useClipboard()

// 状态
const loading = ref(false)
const detailTab = ref('basic')
const searchQuery = ref('')
const selectedModel = ref<GlobalModelResponse | null>(null)
const modelDetailDrawerRef = ref<InstanceType<typeof ModelDetailDrawer> | null>(null)
const createModelDialogOpen = ref(false)
const editingModel = ref<GlobalModelResponse | null>(null)

// 数据
const globalModels = ref<GlobalModelResponse[]>([])
const providers = ref<any[]>([])
const capabilities = ref<CapabilityDefinition[]>([])

// 模型目录分页
const catalogCurrentPage = ref(1)
const catalogPageSize = ref(20)

// 选中模型的详细数据
const selectedModelProviders = ref<any[]>([])
const loadingModelProviders = ref(false)

// 批量添加关联提供商
const batchAddProvidersDialogOpen = ref(false)
const submittingBatchProviders = ref(false)
const providerOptions = ref<any[]>([])
const loadingProviderOptions = ref(false)

// 单列勾选模式所需状态
const batchProviderSearchQuery = ref('')
const selectedBatchProviderIds = ref<Set<string>>(new Set())
const initialBatchProviderIds = ref<Set<string>>(new Set())

// 编辑提供商模型
const editProviderDialogOpen = ref(false)
const editingProvider = ref<any>(null)

// 将 provider 数据转换为 Model 类型供 ProviderModelFormDialog 使用
const editingProviderModel = computed<Model | null>(() => {
  if (!editingProvider.value) return null

  const p = editingProvider.value

  return {
    id: p.model_id,
    provider_id: p.id,
    provider_model_name: p.target_model || '',
    // 使用 API 返回的完整阶梯配置
    tiered_pricing: null,  // 原始配置为空（继承模式）
    effective_tiered_pricing: p.effective_tiered_pricing,  // 有效配置（含继承）
    price_per_request: p.price_per_request,
    supports_streaming: p.supports_streaming,
    supports_vision: p.supports_vision,
    supports_function_calling: p.supports_function_calling,
    supports_extended_thinking: p.supports_extended_thinking,
    is_active: p.is_active,
    global_model_display_name: selectedModel.value?.display_name,
  } as Model
})

// 使用全局确认对话框
const { confirmDanger } = useConfirm()

// 格式化调用次数（大数字简化显示）
function formatUsageCount(count: number): string {
  if (count >= 1000000) {
    return `${(count / 1000000).toFixed(1)  }M`
  } else if (count >= 1000) {
    return `${(count / 1000).toFixed(1)  }K`
  }
  return count.toString()
}

// 从 GlobalModel 的 default_tiered_pricing 获取第一阶梯价格
function getFirstTierPrice(model: GlobalModelResponse, type: 'input' | 'output'): number | null {
  const tiered = model.default_tiered_pricing
  if (!tiered?.tiers?.length) return null
  const firstTier = tiered.tiers[0]
  if (type === 'input') {
    return firstTier.input_price_per_1m || null
  }
  return firstTier.output_price_per_1m || null
}

// 检测是否有阶梯计费（多于一个阶梯）
function hasTieredPricing(model: GlobalModelResponse): boolean {
  const tiered = model.default_tiered_pricing
  return (tiered?.tiers?.length || 0) > 1
}

// 检测是否有对话框打开（防止误关闭抽屉）
const hasBlockingDialogOpen = computed(() =>
  createModelDialogOpen.value ||
  batchAddProvidersDialogOpen.value ||
  editProviderDialogOpen.value
)

// 能力筛选
const capabilityFilters = ref({
  streaming: false,
  imageGeneration: false,
  vision: false,
  toolUse: false,
  extendedThinking: false,
})

// 过滤后的提供商列表
const filteredBatchProviders = computed(() => {
  const query = batchProviderSearchQuery.value.toLowerCase().trim()
  return providerOptions.value.filter(p => {
    if (query && !p.name.toLowerCase().includes(query)) {
      return false
    }
    return true
  })
})

// 检查提供商是否已选中
function isBatchProviderSelected(providerId: string): boolean {
  return selectedBatchProviderIds.value.has(providerId)
}

// 是否全选
const isAllBatchProvidersSelected = computed(() => {
  if (filteredBatchProviders.value.length === 0) return false
  return filteredBatchProviders.value.every(p => isBatchProviderSelected(p.id))
})

// 计算待添加的提供商
const batchProvidersToAdd = computed(() => {
  const toAdd: string[] = []
  for (const id of selectedBatchProviderIds.value) {
    if (!initialBatchProviderIds.value.has(id)) {
      toAdd.push(id)
    }
  }
  return toAdd
})

// 计算待移除的提供商
const batchProvidersToRemove = computed(() => {
  const toRemove: string[] = []
  for (const id of initialBatchProviderIds.value) {
    if (!selectedBatchProviderIds.value.has(id)) {
      toRemove.push(id)
    }
  }
  return toRemove
})

// 是否有变更
const hasBatchProviderChanges = computed(() => {
  return batchProvidersToAdd.value.length > 0 || batchProvidersToRemove.value.length > 0
})

// 待变更数量
const batchProviderPendingChangesCount = computed(() => {
  return batchProvidersToAdd.value.length + batchProvidersToRemove.value.length
})

// 切换提供商选择
function toggleBatchProviderSelection(providerId: string) {
  if (selectedBatchProviderIds.value.has(providerId)) {
    selectedBatchProviderIds.value.delete(providerId)
  } else {
    selectedBatchProviderIds.value.add(providerId)
  }
  selectedBatchProviderIds.value = new Set(selectedBatchProviderIds.value)
}

// 全选/取消全选
function toggleAllBatchProviders() {
  const allIds = filteredBatchProviders.value.map(p => p.id)
  if (isAllBatchProvidersSelected.value) {
    for (const id of allIds) {
      selectedBatchProviderIds.value.delete(id)
    }
  } else {
    for (const id of allIds) {
      selectedBatchProviderIds.value.add(id)
    }
  }
  selectedBatchProviderIds.value = new Set(selectedBatchProviderIds.value)
}

// 同步初始选择状态
function syncBatchProviderSelection() {
  const existingIds = new Set(selectedModelProviders.value.map((p: any) => p.id))
  selectedBatchProviderIds.value = new Set(existingIds)
  initialBatchProviderIds.value = new Set(existingIds)
}

// 保存变更
async function saveBatchProviderChanges() {
  if (!hasBatchProviderChanges.value || submittingBatchProviders.value || !selectedModel.value) return

  submittingBatchProviders.value = true
  try {
    let totalSuccess = 0
    const allErrors: string[] = []

    // 并行移除提供商
    if (batchProvidersToRemove.value.length > 0) {
      const { deleteModel } = await import('@/api/endpoints')
      const removePromises = batchProvidersToRemove.value.map(async (providerId) => {
        const existingProvider = selectedModelProviders.value.find((p: any) => p.id === providerId)
        if (existingProvider && existingProvider.model_id) {
          return deleteModel(providerId, existingProvider.model_id)
        }
        return null
      })

      const results = await Promise.allSettled(removePromises)
      for (const result of results) {
        if (result.status === 'fulfilled' && result.value !== null) {
          totalSuccess++
        } else if (result.status === 'rejected') {
          allErrors.push(parseApiError(result.reason, '移除失败'))
        }
      }
    }

    // 添加提供商
    if (batchProvidersToAdd.value.length > 0) {
      const result = await batchAssignToProviders(selectedModel.value.id, {
        provider_ids: batchProvidersToAdd.value,
        create_models: true
      })
      totalSuccess += result.success.length
      if (result.errors.length > 0) {
        allErrors.push(...result.errors.map(e => e.error))
      }
    }

    if (totalSuccess > 0) {
      success(`成功处理 ${totalSuccess} 个提供商`)
    }

    if (allErrors.length > 0) {
      showError(`部分操作失败: ${allErrors.slice(0, 3).join(', ')}${allErrors.length > 3 ? '...' : ''}`, '警告')
    }

    // 刷新数据并关闭对话框
    await loadModelProviders(selectedModel.value.id)
    await loadGlobalModels()
    // 刷新路由数据
    modelDetailDrawerRef.value?.refreshRoutingData?.()
    closeBatchAddProvidersDialog()
  } catch (err: any) {
    showError(parseApiError(err, '保存失败'), '错误')
  } finally {
    submittingBatchProviders.value = false
  }
}

// 筛选后的模型列表
const filteredGlobalModels = computed(() => {
  let result = globalModels.value

  // 搜索（支持空格分隔的多关键词 AND 搜索）
  if (searchQuery.value) {
    const keywords = searchQuery.value.toLowerCase().split(/\s+/).filter(k => k.length > 0)
    result = result.filter(m => {
      const searchableText = `${m.name} ${m.display_name || ''}`.toLowerCase()
      return keywords.every(keyword => searchableText.includes(keyword))
    })
  }

  // 能力筛选
  if (capabilityFilters.value.streaming) {
    result = result.filter(m => m.config?.streaming !== false)
  }
  if (capabilityFilters.value.imageGeneration) {
    result = result.filter(m => m.config?.image_generation === true)
  }
  if (capabilityFilters.value.vision) {
    result = result.filter(m => m.config?.vision === true)
  }
  if (capabilityFilters.value.toolUse) {
    result = result.filter(m => m.config?.function_calling === true)
  }
  if (capabilityFilters.value.extendedThinking) {
    result = result.filter(m => m.config?.extended_thinking === true)
  }

  return result
})

// 模型目录分页计算
const paginatedGlobalModels = computed(() => {
  const start = (catalogCurrentPage.value - 1) * catalogPageSize.value
  const end = start + catalogPageSize.value
  return filteredGlobalModels.value.slice(start, end)
})

// 搜索或筛选变化时重置到第一页
watch([searchQuery, capabilityFilters], () => {
  catalogCurrentPage.value = 1
}, { deep: true })

async function loadGlobalModels() {
  loading.value = true
  try {
    const response = await listGlobalModels()
    // API 返回 { models: [...], total: number }
    globalModels.value = response.models || []
  } catch (err: any) {
    log.error('加载模型失败:', err)
    showError(err.response?.data?.detail || err.message, '加载模型失败')
  } finally {
    loading.value = false
  }
}

// 使用复用的行点击逻辑
const { handleMouseDown, shouldTriggerRowClick } = useRowClick()

// 处理行点击，如果用户选择了文字则不触发抽屉
function handleRowClick(event: MouseEvent, model: GlobalModelResponse) {
  if (!shouldTriggerRowClick(event)) return
  selectModel(model)
}

async function selectModel(model: GlobalModelResponse) {
  selectedModel.value = model
  detailTab.value = 'basic'

  // 加载该模型的关联提供商
  await loadModelProviders(model.id)
}

// 加载指定模型的关联提供商
async function loadModelProviders(_globalModelId: string) {
  loadingModelProviders.value = true
  try {
    // 使用新的 API 获取所有关联提供商（包括非活跃的）
    const response = await getGlobalModelProviders(_globalModelId)

    // 转换为展示格式
    selectedModelProviders.value = response.providers.map(p => ({
      id: p.provider_id,
      model_id: p.model_id,
      name: p.provider_name,
      provider_type: 'API',
      target_model: p.target_model,
      is_active: p.is_active,
      // 价格信息
      input_price_per_1m: p.input_price_per_1m,
      output_price_per_1m: p.output_price_per_1m,
      cache_creation_price_per_1m: p.cache_creation_price_per_1m,
      cache_read_price_per_1m: p.cache_read_price_per_1m,
      cache_1h_creation_price_per_1m: p.cache_1h_creation_price_per_1m,
      price_per_request: p.price_per_request,
      effective_tiered_pricing: p.effective_tiered_pricing,
      tier_count: p.tier_count,
      // 能力信息
      supports_vision: p.supports_vision,
      supports_function_calling: p.supports_function_calling,
      supports_streaming: p.supports_streaming
    }))
  } catch (err: any) {
    log.error('加载关联提供商失败:', err)
    showError(parseApiError(err, '加载关联提供商失败'), '错误')
    selectedModelProviders.value = []
  } finally {
    loadingModelProviders.value = false
  }
}

// 刷新当前选中模型的关联提供商
async function refreshSelectedModelProviders() {
  if (selectedModel.value) {
    await loadModelProviders(selectedModel.value.id)
  }
}

// 确保 Provider 选项已加载
async function ensureProviderOptions() {
  if (providerOptions.value.length > 0 || loadingProviderOptions.value) {
    return
  }
  try {
    loadingProviderOptions.value = true
    providerOptions.value = await getProvidersSummary()
  } catch (err: any) {
    const message = parseApiError(err, '加载 Provider 列表失败')
    showError(message, '错误')
  } finally {
    loadingProviderOptions.value = false
  }
}

// 打开添加关联提供商对话框
function openAddProviderDialog() {
  if (!selectedModel.value) return
  batchProviderSearchQuery.value = ''
  batchAddProvidersDialogOpen.value = true
  ensureProviderOptions().then(() => {
    // 同步选择状态
    syncBatchProviderSelection()
  })
}

// 处理批量添加 Provider 对话框关闭事件
function handleBatchAddProvidersDialogUpdate(value: boolean) {
  // 只有在不处于提交状态时才允许关闭
  if (!value && submittingBatchProviders.value) {
    return
  }
  batchAddProvidersDialogOpen.value = value
}

// 关闭批量添加对话框
function closeBatchAddProvidersDialog() {
  batchAddProvidersDialogOpen.value = false
  batchProviderSearchQuery.value = ''
  selectedBatchProviderIds.value = new Set()
  initialBatchProviderIds.value = new Set()
  submittingBatchProviders.value = false
}

// 抽屉控制函数
function handleDrawerOpenChange(value: boolean) {
  if (!value && !hasBlockingDialogOpen.value) {
    selectedModel.value = null
  }
}

// 编辑提供商模型
function openEditProviderImplementation(provider: any) {
  editingProvider.value = provider
  editProviderDialogOpen.value = true
}

// 处理编辑 Provider 对话框关闭事件
function handleEditProviderDialogUpdate(value: boolean) {
  editProviderDialogOpen.value = value
  if (!value) {
    editingProvider.value = null
  }
}

// 编辑提供商模型保存成功后刷新列表
async function handleEditProviderSaved() {
  if (selectedModel.value) {
    await loadModelProviders(selectedModel.value.id)
  }
}

// 切换关联提供商状态
async function toggleProviderStatus(provider: any) {
  if (!provider.model_id) {
    showError('缺少模型 ID')
    return
  }

  try {
    const { updateModel } = await import('@/api/endpoints')
    const newStatus = !provider.is_active
    await updateModel(provider.id, provider.model_id, { is_active: newStatus })
    provider.is_active = newStatus
    success(newStatus ? '已启用此关联提供商' : '已停用此关联提供商')
    // 刷新路由数据
    modelDetailDrawerRef.value?.refreshRoutingData?.()
  } catch (err: any) {
    showError(parseApiError(err, '更新状态失败'))
  }
}

// 删除关联提供商
async function confirmDeleteProviderImplementation(provider: any) {
  if (!provider.model_id) {
    showError('缺少模型 ID')
    return
  }

  const confirmed = await confirmDanger(
    `确定要删除 ${provider.name} 的模型关联吗？\n\n此操作不可恢复！`,
    '删除关联提供商'
  )
  if (!confirmed) return

  try {
    const { deleteModel } = await import('@/api/endpoints')
    await deleteModel(provider.id, provider.model_id)
    success(`已删除 ${provider.name} 的模型实现`)
    // 同步更新 selectedModelProviders 确保状态一致
    if (selectedModel.value) {
      await loadModelProviders(selectedModel.value.id)
    }
    // 刷新路由数据
    modelDetailDrawerRef.value?.refreshRoutingData?.()
  } catch (err: any) {
    showError(parseApiError(err, '删除模型失败'))
  }
}

function openCreateModelDialog() {
  editingModel.value = null
  createModelDialogOpen.value = true
}

// 处理模型对话框关闭事件
function handleModelDialogUpdate(value: boolean) {
  createModelDialogOpen.value = value
  if (!value) {
    editingModel.value = null
  }
}

// 处理模型表单提交成功
async function handleModelFormSuccess() {
  createModelDialogOpen.value = false
  editingModel.value = null
  await loadGlobalModels()
}

async function editModel(model: GlobalModelResponse) {
  editingModel.value = model
  createModelDialogOpen.value = true
}

async function deleteModel(model: GlobalModelResponse) {
  const confirmed = await confirmDanger(
    `确定删除模型 "${model.name}" 吗？\n\n此操作不可撤销。`,
    '删除模型'
  )
  if (!confirmed) return

  try {
    await deleteGlobalModel(model.id)
    success('模型删除成功')
    if (selectedModel.value?.id === model.id) {
      selectedModel.value = null
    }
    await loadGlobalModels()
  } catch (err: any) {
    showError(err.response?.data?.detail || err.message, '删除失败')
  }
}

async function toggleModelStatus(model: GlobalModelResponse) {
  try {
    await updateGlobalModel(model.id, { is_active: !model.is_active })
    model.is_active = !model.is_active
    success(model.is_active ? '模型已启用' : '模型已停用')
  } catch (err: any) {
    showError(err.response?.data?.detail || err.message, '操作失败')
  }
}

async function refreshData() {
  await loadGlobalModels()
}

async function loadProviders() {
  try {
    providers.value = await getProvidersSummary()
  } catch (err: any) {
    showError(err.response?.data?.detail || err.message, '加载 Provider 列表失败')
  }
}

async function loadCapabilities() {
  try {
    capabilities.value = await getAllCapabilities()
  } catch (err) {
    log.error('Failed to load capabilities:', err)
  }
}

// 获取 capability 的显示名称
function getCapabilityDisplayName(capName: string): string {
  const cap = capabilities.value.find(c => c.name === capName)
  return cap?.display_name || capName
}

// 获取 capability 的短名称（用于表格展示）
function getCapabilityShortName(capName: string): string {
  const cap = capabilities.value.find(c => c.name === capName)
  return cap?.short_name || cap?.display_name || capName
}

onMounted(async () => {
  await Promise.all([
    refreshData(),
    loadProviders(),
    loadCapabilities(),
  ])
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
