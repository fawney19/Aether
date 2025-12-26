<template>
  <Dialog
    :model-value="open"
    title="批量添加关联模型"
    description="为提供商批量添加模型实现，提供商将自动继承模型的价格和能力，可在添加后单独修改"
    :icon="Layers"
    size="4xl"
    @update:model-value="$emit('update:open', $event)"
  >
    <template #default>
      <div class="space-y-4">
        <!-- 提供商信息头部 -->
        <div class="rounded-lg border bg-muted/30 p-4">
          <div class="flex items-start justify-between">
            <div>
              <p class="font-semibold text-lg">
                {{ providerName }}
              </p>
              <p class="text-sm text-muted-foreground font-mono">
                {{ providerIdentifier }}
              </p>
            </div>
            <Badge
              variant="outline"
              class="text-xs"
            >
              当前 {{ existingModels.length }} 个模型
            </Badge>
          </div>
        </div>

        <!-- 左右对比布局 -->
        <div class="flex gap-2 items-stretch">
          <!-- 左侧：可添加的模型（分组折叠） -->
          <div class="flex-1 space-y-2">
            <div class="flex items-center justify-between gap-2">
              <p class="text-sm font-medium shrink-0">
                可添加
              </p>
              <div class="flex-1 relative">
                <Search class="absolute left-2 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-muted-foreground" />
                <Input
                  v-model="searchQuery"
                  placeholder="搜索模型..."
                  class="pl-7 h-7 text-xs"
                />
              </div>
              <button
                v-if="upstreamModelsLoaded"
                type="button"
                class="p-1.5 hover:bg-muted rounded-md transition-colors shrink-0"
                title="刷新上游模型"
                :disabled="fetchingUpstreamModels"
                @click="fetchUpstreamModels(true)"
              >
                <RefreshCw
                  class="w-3.5 h-3.5"
                  :class="{ 'animate-spin': fetchingUpstreamModels }"
                />
              </button>
              <button
                v-else-if="!fetchingUpstreamModels"
                type="button"
                class="p-1.5 hover:bg-muted rounded-md transition-colors shrink-0"
                title="从提供商获取模型"
                @click="fetchUpstreamModels"
              >
                <Zap class="w-3.5 h-3.5" />
              </button>
              <Loader2
                v-else
                class="w-3.5 h-3.5 animate-spin text-muted-foreground shrink-0"
              />
            </div>
            <div class="border rounded-lg h-80 overflow-y-auto">
              <div
                v-if="loadingGlobalModels"
                class="flex items-center justify-center h-full"
              >
                <Loader2 class="w-6 h-6 animate-spin text-primary" />
              </div>
              <div
                v-else-if="totalAvailableCount === 0 && !upstreamModelsLoaded"
                class="flex flex-col items-center justify-center h-full text-muted-foreground"
              >
                <Layers class="w-10 h-10 mb-2 opacity-30" />
                <p class="text-sm">
                  所有模型均已关联
                </p>
              </div>
              <div
                v-else
                class="p-2 space-y-2"
              >
                <!-- 全局模型折叠组 -->
                <div
                  v-if="availableGlobalModels.length > 0 || !upstreamModelsLoaded"
                  class="border rounded-lg overflow-hidden"
                >
                  <div class="flex items-center gap-2 px-3 py-2 bg-muted/30">
                    <button
                      type="button"
                      class="flex items-center gap-2 flex-1 hover:bg-muted/50 -mx-1 px-1 rounded transition-colors"
                      @click="toggleGroupCollapse('global')"
                    >
                      <ChevronDown
                        class="w-4 h-4 transition-transform shrink-0"
                        :class="collapsedGroups.has('global') ? '-rotate-90' : ''"
                      />
                      <span class="text-xs font-medium">
                        全局模型
                      </span>
                      <span class="text-xs text-muted-foreground">
                        ({{ availableGlobalModels.length }})
                      </span>
                    </button>
                    <button
                      v-if="availableGlobalModels.length > 0"
                      type="button"
                      class="text-xs text-primary hover:underline shrink-0"
                      @click.stop="selectAllGlobalModels"
                    >
                      {{ isAllGlobalModelsSelected ? '取消' : '全选' }}
                    </button>
                  </div>
                  <div
                    v-show="!collapsedGroups.has('global')"
                    class="p-2 space-y-1 border-t"
                  >
                    <div
                      v-if="availableGlobalModels.length === 0"
                      class="py-4 text-center text-xs text-muted-foreground"
                    >
                      所有全局模型均已关联
                    </div>
                    <div
                      v-for="model in availableGlobalModels"
                      v-else
                      :key="model.id"
                      class="flex items-center gap-2 p-2 rounded-lg border transition-colors cursor-pointer"
                      :class="selectedGlobalModelIds.includes(model.id)
                        ? 'border-primary bg-primary/10'
                        : 'hover:bg-muted/50'"
                      @click="toggleGlobalModelSelection(model.id)"
                    >
                      <Checkbox
                        :checked="selectedGlobalModelIds.includes(model.id)"
                        @update:checked="toggleGlobalModelSelection(model.id)"
                        @click.stop
                      />
                      <div class="flex-1 min-w-0">
                        <p class="font-medium text-sm truncate">
                          {{ model.display_name }}
                        </p>
                        <p class="text-xs text-muted-foreground truncate font-mono">
                          {{ model.name }}
                        </p>
                      </div>
                      <Badge
                        :variant="model.is_active ? 'outline' : 'secondary'"
                        :class="model.is_active ? 'text-green-600 border-green-500/60' : ''"
                        class="text-xs shrink-0"
                      >
                        {{ model.is_active ? '活跃' : '停用' }}
                      </Badge>
                    </div>
                  </div>
                </div>

                <!-- 从提供商获取的模型折叠组 -->
                <div
                  v-for="group in upstreamModelGroups"
                  :key="group.api_format"
                  class="border rounded-lg overflow-hidden"
                >
                  <div class="flex items-center gap-2 px-3 py-2 bg-muted/30">
                    <button
                      type="button"
                      class="flex items-center gap-2 flex-1 hover:bg-muted/50 -mx-1 px-1 rounded transition-colors"
                      @click="toggleGroupCollapse(group.api_format)"
                    >
                      <ChevronDown
                        class="w-4 h-4 transition-transform shrink-0"
                        :class="collapsedGroups.has(group.api_format) ? '-rotate-90' : ''"
                      />
                      <span class="text-xs font-medium">
                        {{ API_FORMAT_LABELS[group.api_format] || group.api_format }}
                      </span>
                      <span class="text-xs text-muted-foreground">
                        ({{ group.models.length }})
                      </span>
                    </button>
                    <button
                      type="button"
                      class="text-xs text-primary hover:underline shrink-0"
                      @click.stop="selectAllUpstreamModels(group.api_format)"
                    >
                      {{ isUpstreamGroupAllSelected(group.api_format) ? '取消' : '全选' }}
                    </button>
                  </div>
                  <div
                    v-show="!collapsedGroups.has(group.api_format)"
                    class="p-2 space-y-1 border-t"
                  >
                    <div
                      v-for="model in group.models"
                      :key="model.id"
                      class="flex items-center gap-2 p-2 rounded-lg border transition-colors cursor-pointer"
                      :class="selectedUpstreamModelIds.includes(model.id)
                        ? 'border-primary bg-primary/10'
                        : 'hover:bg-muted/50'"
                      @click="toggleUpstreamModelSelection(model.id)"
                    >
                      <Checkbox
                        :checked="selectedUpstreamModelIds.includes(model.id)"
                        @update:checked="toggleUpstreamModelSelection(model.id)"
                        @click.stop
                      />
                      <div class="flex-1 min-w-0">
                        <div class="flex items-center gap-2">
                          <p class="font-medium text-sm truncate">
                            {{ model.id }}
                          </p>
                          <span
                            v-if="model.mapped"
                            class="shrink-0 px-1.5 py-0.5 text-xs rounded bg-primary/10 text-primary border border-primary/20"
                            title="此模型已被映射"
                          >
                            已映射
                          </span>
                        </div>
                        <p class="text-xs text-muted-foreground truncate font-mono">
                          {{ model.owned_by || model.id }}
                        </p>
                      </div>
                    </div>
                  </div>
                </div>
              </div>
            </div>
          </div>

          <!-- 中间：操作按钮 -->
          <div class="flex flex-col items-center justify-center w-12 shrink-0 gap-2">
            <Button
              variant="outline"
              size="sm"
              class="w-9 h-8"
              :class="totalSelectedCount > 0 && !submittingAdd ? 'border-primary' : ''"
              :disabled="totalSelectedCount === 0 || submittingAdd"
              title="添加选中"
              @click="batchAddSelected"
            >
              <Loader2
                v-if="submittingAdd"
                class="w-4 h-4 animate-spin"
              />
              <ChevronRight
                v-else
                class="w-6 h-6 stroke-[3]"
                :class="totalSelectedCount > 0 && !submittingAdd ? 'text-primary' : ''"
              />
            </Button>
            <Button
              variant="outline"
              size="sm"
              class="w-9 h-8"
              :class="selectedRightIds.length > 0 && !submittingRemove ? 'border-primary' : ''"
              :disabled="selectedRightIds.length === 0 || submittingRemove"
              title="移除选中"
              @click="batchRemoveSelected"
            >
              <Loader2
                v-if="submittingRemove"
                class="w-4 h-4 animate-spin"
              />
              <ChevronLeft
                v-else
                class="w-6 h-6 stroke-[3]"
                :class="selectedRightIds.length > 0 && !submittingRemove ? 'text-primary' : ''"
              />
            </Button>
          </div>

          <!-- 右侧：已添加的模型 -->
          <div class="flex-1 space-y-2">
            <div class="flex items-center justify-between">
              <p class="text-sm font-medium">
                已添加
              </p>
              <Button
                v-if="existingModels.length > 0"
                variant="ghost"
                size="sm"
                class="h-6 px-2 text-xs"
                @click="toggleSelectAllRight"
              >
                {{ isAllRightSelected ? '取消' : '全选' }}
              </Button>
            </div>
            <div class="border rounded-lg h-80 overflow-y-auto">
              <div
                v-if="existingModels.length === 0"
                class="flex flex-col items-center justify-center h-full text-muted-foreground"
              >
                <Layers class="w-10 h-10 mb-2 opacity-30" />
                <p class="text-sm">
                  暂无关联模型
                </p>
              </div>
              <div
                v-else
                class="p-2 space-y-1"
              >
                <div
                  v-for="model in existingModels"
                  :key="'existing-' + model.id"
                  class="flex items-center gap-2 p-2 rounded-lg border transition-colors cursor-pointer"
                  :class="selectedRightIds.includes(model.id)
                    ? 'border-primary bg-primary/10'
                    : 'hover:bg-muted/50'"
                  @click="toggleRightSelection(model.id)"
                >
                  <Checkbox
                    :checked="selectedRightIds.includes(model.id)"
                    @update:checked="toggleRightSelection(model.id)"
                    @click.stop
                  />
                  <div class="flex-1 min-w-0">
                    <p class="font-medium text-sm truncate">
                      {{ model.global_model_display_name || model.provider_model_name }}
                    </p>
                    <p class="text-xs text-muted-foreground truncate font-mono">
                      {{ model.provider_model_name }}
                    </p>
                  </div>
                  <Badge
                    :variant="model.is_active ? 'outline' : 'secondary'"
                    :class="model.is_active ? 'text-green-600 border-green-500/60' : ''"
                    class="text-xs shrink-0"
                  >
                    {{ model.is_active ? '活跃' : '停用' }}
                  </Badge>
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </template>
    <template #footer>
      <Button
        variant="outline"
        @click="$emit('update:open', false)"
      >
        关闭
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { ref, computed, watch } from 'vue'
import { Layers, Loader2, ChevronRight, ChevronLeft, ChevronDown, Zap, RefreshCw, Search } from 'lucide-vue-next'
import Dialog from '@/components/ui/dialog/Dialog.vue'
import Button from '@/components/ui/button.vue'
import Badge from '@/components/ui/badge.vue'
import Checkbox from '@/components/ui/checkbox.vue'
import Input from '@/components/ui/input.vue'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'
import {
  getGlobalModels,
  type GlobalModelResponse
} from '@/api/endpoints/global-models'
import {
  getProviderModels,
  batchAssignModelsToProvider,
  deleteModel,
  importModelsFromUpstream,
  API_FORMAT_LABELS,
  type Model
} from '@/api/endpoints'
import { useUpstreamModelsCache, type UpstreamModel } from '../composables/useUpstreamModelsCache'

const { fetchModels: fetchCachedModels, clearCache, getCachedModels } = useUpstreamModelsCache()

const props = defineProps<{
  open: boolean
  providerId: string
  providerName: string
  providerIdentifier: string
}>()

const emit = defineEmits<{
  'update:open': [value: boolean]
  'changed': []
}>()

const { error: showError, success } = useToast()

// 状态
const loadingGlobalModels = ref(false)
const submittingAdd = ref(false)
const submittingRemove = ref(false)
const fetchingUpstreamModels = ref(false)
const upstreamModelsLoaded = ref(false)

// 数据
const allGlobalModels = ref<GlobalModelResponse[]>([])
const existingModels = ref<Model[]>([])
const upstreamModels = ref<UpstreamModel[]>([])

// 选择状态
const selectedGlobalModelIds = ref<string[]>([])
const selectedUpstreamModelIds = ref<string[]>([])
const selectedRightIds = ref<string[]>([])

// 折叠状态
const collapsedGroups = ref<Set<string>>(new Set())

// 搜索状态
const searchQuery = ref('')

// 计算可添加的全局模型（排除已关联的）
const availableGlobalModelsBase = computed(() => {
  const existingGlobalModelIds = new Set(
    existingModels.value
      .filter(m => m.global_model_id)
      .map(m => m.global_model_id)
  )
  return allGlobalModels.value.filter(m => !existingGlobalModelIds.has(m.id))
})

// 搜索过滤后的全局模型
const availableGlobalModels = computed(() => {
  if (!searchQuery.value.trim()) return availableGlobalModelsBase.value
  const query = searchQuery.value.toLowerCase()
  return availableGlobalModelsBase.value.filter(m =>
    m.name.toLowerCase().includes(query) ||
    m.display_name.toLowerCase().includes(query)
  )
})

// 计算可添加的上游模型（排除已关联的）
const availableUpstreamModelsBase = computed(() => {
  const existingModelNames = new Set(
    existingModels.value.map(m => m.provider_model_name)
  )
  return upstreamModels.value.filter(m => !existingModelNames.has(m.id))
})

// 搜索过滤后的上游模型
const availableUpstreamModels = computed(() => {
  if (!searchQuery.value.trim()) return availableUpstreamModelsBase.value
  const query = searchQuery.value.toLowerCase()
  return availableUpstreamModelsBase.value.filter(m =>
    m.id.toLowerCase().includes(query) ||
    (m.owned_by && m.owned_by.toLowerCase().includes(query))
  )
})

// 按 API 格式分组的上游模型
const upstreamModelGroups = computed(() => {
  const groups: Record<string, UpstreamModel[]> = {}

  for (const model of availableUpstreamModels.value) {
    const format = model.api_format || 'unknown'
    if (!groups[format]) {
      groups[format] = []
    }
    groups[format].push(model)
  }

  // 按 API_FORMAT_LABELS 的顺序排序
  const order = Object.keys(API_FORMAT_LABELS)
  return Object.entries(groups)
    .map(([api_format, models]) => ({ api_format, models }))
    .sort((a, b) => {
      const aIndex = order.indexOf(a.api_format)
      const bIndex = order.indexOf(b.api_format)
      if (aIndex === -1 && bIndex === -1) return a.api_format.localeCompare(b.api_format)
      if (aIndex === -1) return 1
      if (bIndex === -1) return -1
      return aIndex - bIndex
    })
})

// 总可添加数量
const totalAvailableCount = computed(() => {
  return availableGlobalModels.value.length + availableUpstreamModels.value.length
})

// 总选中数量
const totalSelectedCount = computed(() => {
  return selectedGlobalModelIds.value.length + selectedUpstreamModelIds.value.length
})

// 全选状态
const isAllRightSelected = computed(() =>
  existingModels.value.length > 0 &&
  selectedRightIds.value.length === existingModels.value.length
)

// 全局模型是否全选
const isAllGlobalModelsSelected = computed(() => {
  if (availableGlobalModels.value.length === 0) return false
  return availableGlobalModels.value.every(m => selectedGlobalModelIds.value.includes(m.id))
})

// 检查某个上游组是否全选
function isUpstreamGroupAllSelected(apiFormat: string): boolean {
  const group = upstreamModelGroups.value.find(g => g.api_format === apiFormat)
  if (!group || group.models.length === 0) return false
  return group.models.every(m => selectedUpstreamModelIds.value.includes(m.id))
}

// 监听打开状态
watch(() => props.open, async (isOpen) => {
  if (isOpen && props.providerId) {
    await loadData()
  } else {
    // 重置状态
    selectedGlobalModelIds.value = []
    selectedUpstreamModelIds.value = []
    selectedRightIds.value = []
    upstreamModels.value = []
    upstreamModelsLoaded.value = false
    collapsedGroups.value = new Set()
    searchQuery.value = ''
  }
})

// 加载数据
async function loadData() {
  await Promise.all([loadGlobalModels(), loadExistingModels()])
  // 默认折叠全局模型组
  collapsedGroups.value = new Set(['global'])

  // 检查缓存，如果有缓存数据则直接使用
  const cachedModels = getCachedModels(props.providerId)
  if (cachedModels) {
    upstreamModels.value = cachedModels
    upstreamModelsLoaded.value = true
    // 折叠所有上游模型组
    for (const model of cachedModels) {
      if (model.api_format) {
        collapsedGroups.value.add(model.api_format)
      }
    }
  }
}

// 加载全局模型列表
async function loadGlobalModels() {
  try {
    loadingGlobalModels.value = true
    const response = await getGlobalModels({ limit: 1000 })
    allGlobalModels.value = response.models
  } catch (err: any) {
    showError(parseApiError(err, '加载全局模型失败'), '错误')
  } finally {
    loadingGlobalModels.value = false
  }
}

// 加载已关联的模型
async function loadExistingModels() {
  try {
    existingModels.value = await getProviderModels(props.providerId)
  } catch (err: any) {
    showError(parseApiError(err, '加载已关联模型失败'), '错误')
  }
}

// 从提供商获取模型
async function fetchUpstreamModels(forceRefresh = false) {
  if (forceRefresh) {
    clearCache(props.providerId)
  }

  try {
    fetchingUpstreamModels.value = true
    const result = await fetchCachedModels(props.providerId, forceRefresh)
    if (result) {
      if (result.error) {
        showError(result.error, '错误')
      } else {
        upstreamModels.value = result.models
        upstreamModelsLoaded.value = true
        // 折叠所有上游模型组
        const allGroups = new Set(collapsedGroups.value)
        for (const model of result.models) {
          if (model.api_format) {
            allGroups.add(model.api_format)
          }
        }
        collapsedGroups.value = allGroups
      }
    }
  } finally {
    fetchingUpstreamModels.value = false
  }
}

// 切换折叠状态
function toggleGroupCollapse(group: string) {
  if (collapsedGroups.value.has(group)) {
    collapsedGroups.value.delete(group)
  } else {
    collapsedGroups.value.add(group)
  }
  // 触发响应式更新
  collapsedGroups.value = new Set(collapsedGroups.value)
}

// 切换全局模型选择
function toggleGlobalModelSelection(id: string) {
  const index = selectedGlobalModelIds.value.indexOf(id)
  if (index === -1) {
    selectedGlobalModelIds.value.push(id)
  } else {
    selectedGlobalModelIds.value.splice(index, 1)
  }
}

// 切换上游模型选择
function toggleUpstreamModelSelection(id: string) {
  const index = selectedUpstreamModelIds.value.indexOf(id)
  if (index === -1) {
    selectedUpstreamModelIds.value.push(id)
  } else {
    selectedUpstreamModelIds.value.splice(index, 1)
  }
}

// 全选全局模型
function selectAllGlobalModels() {
  const allIds = availableGlobalModels.value.map(m => m.id)
  const allSelected = allIds.every(id => selectedGlobalModelIds.value.includes(id))
  if (allSelected) {
    selectedGlobalModelIds.value = selectedGlobalModelIds.value.filter(id => !allIds.includes(id))
  } else {
    const newIds = allIds.filter(id => !selectedGlobalModelIds.value.includes(id))
    selectedGlobalModelIds.value.push(...newIds)
  }
}

// 全选某个 API 格式的上游模型
function selectAllUpstreamModels(apiFormat: string) {
  const group = upstreamModelGroups.value.find(g => g.api_format === apiFormat)
  if (!group) return

  const allIds = group.models.map(m => m.id)
  const allSelected = allIds.every(id => selectedUpstreamModelIds.value.includes(id))
  if (allSelected) {
    selectedUpstreamModelIds.value = selectedUpstreamModelIds.value.filter(id => !allIds.includes(id))
  } else {
    const newIds = allIds.filter(id => !selectedUpstreamModelIds.value.includes(id))
    selectedUpstreamModelIds.value.push(...newIds)
  }
}

// 切换右侧选择
function toggleRightSelection(id: string) {
  const index = selectedRightIds.value.indexOf(id)
  if (index === -1) {
    selectedRightIds.value.push(id)
  } else {
    selectedRightIds.value.splice(index, 1)
  }
}

// 全选/取消全选右侧
function toggleSelectAllRight() {
  if (isAllRightSelected.value) {
    selectedRightIds.value = []
  } else {
    selectedRightIds.value = existingModels.value.map(m => m.id)
  }
}

// 批量添加选中的模型
async function batchAddSelected() {
  if (totalSelectedCount.value === 0) return

  try {
    submittingAdd.value = true
    let totalSuccess = 0
    const allErrors: string[] = []

    // 处理全局模型
    if (selectedGlobalModelIds.value.length > 0) {
      const result = await batchAssignModelsToProvider(props.providerId, selectedGlobalModelIds.value)
      totalSuccess += result.success.length
      if (result.errors.length > 0) {
        allErrors.push(...result.errors.map(e => e.error))
      }
    }

    // 处理上游模型（调用 import-from-upstream API）
    if (selectedUpstreamModelIds.value.length > 0) {
      const result = await importModelsFromUpstream(props.providerId, selectedUpstreamModelIds.value)
      totalSuccess += result.success.length
      if (result.errors.length > 0) {
        allErrors.push(...result.errors.map(e => e.error))
      }
    }

    if (totalSuccess > 0) {
      success(`成功添加 ${totalSuccess} 个模型`)
    }

    if (allErrors.length > 0) {
      showError(`部分模型添加失败: ${allErrors.slice(0, 3).join(', ')}${allErrors.length > 3 ? '...' : ''}`, '警告')
    }

    selectedGlobalModelIds.value = []
    selectedUpstreamModelIds.value = []
    await loadExistingModels()
    emit('changed')
  } catch (err: any) {
    showError(parseApiError(err, '批量添加失败'), '错误')
  } finally {
    submittingAdd.value = false
  }
}

// 批量移除选中的模型
async function batchRemoveSelected() {
  if (selectedRightIds.value.length === 0) return

  try {
    submittingRemove.value = true
    let successCount = 0
    const errors: string[] = []

    for (const modelId of selectedRightIds.value) {
      try {
        await deleteModel(props.providerId, modelId)
        successCount++
      } catch (err: any) {
        errors.push(parseApiError(err, '删除失败'))
      }
    }

    if (successCount > 0) {
      success(`成功移除 ${successCount} 个模型`)
    }

    if (errors.length > 0) {
      showError(`部分模型移除失败: ${errors.join(', ')}`, '警告')
    }

    selectedRightIds.value = []
    await loadExistingModels()
    emit('changed')
  } catch (err: any) {
    showError(parseApiError(err, '批量移除失败'), '错误')
  } finally {
    submittingRemove.value = false
  }
}
</script>
