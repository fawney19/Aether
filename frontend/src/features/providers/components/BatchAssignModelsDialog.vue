<template>
  <Dialog
    :model-value="open"
    :title="providerName ? `批量管理模型 - ${providerName}` : '批量管理模型'"
    description="选中的模型将被关联到提供商，取消选中将移除关联"
    :icon="Layers"
    size="2xl"
    @update:model-value="handleDialogUpdate"
  >
    <template #default>
      <div class="space-y-4">
        <!-- 搜索栏 -->
        <div class="flex items-center gap-2">
          <div class="flex-1 relative">
            <Search class="absolute left-2.5 top-1/2 -translate-y-1/2 w-4 h-4 text-muted-foreground" />
            <Input
              v-model="searchQuery"
              placeholder="搜索模型..."
              class="pl-8 h-9"
            />
          </div>
          <button
            v-if="upstreamModelsLoaded"
            type="button"
            class="p-2 hover:bg-muted rounded-md transition-colors shrink-0"
            title="刷新上游模型"
            :disabled="fetchingUpstreamModels"
            @click="fetchUpstreamModels(true)"
          >
            <RefreshCw
              class="w-4 h-4"
              :class="{ 'animate-spin': fetchingUpstreamModels }"
            />
          </button>
          <button
            v-else-if="!fetchingUpstreamModels"
            type="button"
            class="p-2 hover:bg-muted rounded-md transition-colors shrink-0"
            title="从提供商获取模型"
            @click="fetchUpstreamModels()"
          >
            <Zap class="w-4 h-4" />
          </button>
          <Loader2
            v-else
            class="w-4 h-4 animate-spin text-muted-foreground shrink-0"
          />
        </div>

        <!-- 单列模型列表 -->
        <div class="border rounded-lg overflow-hidden">
          <div class="max-h-96 overflow-y-auto">
            <div
              v-if="loadingGlobalModels"
              class="flex items-center justify-center py-12"
            >
              <Loader2 class="w-6 h-6 animate-spin text-primary" />
            </div>

            <template v-else>
              <!-- 全局模型组 -->
              <div v-if="filteredGlobalModels.length > 0 || !upstreamModelsLoaded">
                <div
                  class="flex items-center justify-between px-3 py-2 bg-muted sticky top-0 z-10 cursor-pointer hover:bg-muted/80 transition-colors"
                  @click="toggleGroupCollapse('global')"
                >
                  <div class="flex items-center gap-2">
                    <ChevronDown
                      class="w-4 h-4 transition-transform shrink-0"
                      :class="collapsedGroups.has('global') ? '-rotate-90' : ''"
                    />
                    <span class="text-xs font-medium">全局模型</span>
                    <span class="text-xs text-muted-foreground">({{ filteredGlobalModels.length }})</span>
                  </div>
                  <button
                    v-if="filteredGlobalModels.length > 0"
                    type="button"
                    class="text-xs text-primary hover:underline shrink-0"
                    @click.stop="toggleAllGlobalModels"
                  >
                    {{ isAllGlobalModelsSelected ? '取消全选' : '全选' }}
                  </button>
                </div>
                <div
                  v-show="!collapsedGroups.has('global')"
                  class="space-y-1 p-2"
                >
                  <div
                    v-if="filteredGlobalModels.length === 0"
                    class="py-4 text-center text-xs text-muted-foreground"
                  >
                    暂无可用全局模型
                  </div>
                  <div
                    v-for="model in filteredGlobalModels"
                    :key="model.id"
                    class="flex items-center gap-2 px-2 py-1.5 rounded hover:bg-muted cursor-pointer"
                    @click="toggleGlobalModelSelection(model.id)"
                  >
                    <div
                      class="w-4 h-4 border rounded flex items-center justify-center shrink-0"
                      :class="isGlobalModelSelected(model.id) ? 'bg-primary border-primary' : ''"
                    >
                      <Check
                        v-if="isGlobalModelSelected(model.id)"
                        class="w-3 h-3 text-primary-foreground"
                      />
                    </div>
                    <div class="flex-1 min-w-0">
                      <p class="text-sm font-medium truncate">{{ model.display_name }}</p>
                      <p class="text-xs text-muted-foreground truncate font-mono">{{ model.name }}</p>
                    </div>
                  </div>
                </div>
              </div>

              <!-- 上游模型组 -->
              <div
                v-for="group in filteredUpstreamGroups"
                :key="group.api_format"
              >
                <div
                  class="flex items-center justify-between px-3 py-2 bg-muted sticky top-0 z-10 cursor-pointer hover:bg-muted/80 transition-colors"
                  @click="toggleGroupCollapse(group.api_format)"
                >
                  <div class="flex items-center gap-2">
                    <ChevronDown
                      class="w-4 h-4 transition-transform shrink-0"
                      :class="collapsedGroups.has(group.api_format) ? '-rotate-90' : ''"
                    />
                    <span class="text-xs font-medium">{{ API_FORMAT_LABELS[group.api_format] || group.api_format }}</span>
                    <span class="text-xs text-muted-foreground">({{ group.models.length }})</span>
                  </div>
                  <button
                    type="button"
                    class="text-xs text-primary hover:underline shrink-0"
                    @click.stop="toggleAllUpstreamGroup(group.api_format)"
                  >
                    {{ isUpstreamGroupAllSelected(group.api_format) ? '取消全选' : '全选' }}
                  </button>
                </div>
                <div
                  v-show="!collapsedGroups.has(group.api_format)"
                  class="space-y-1 p-2"
                >
                  <div
                    v-for="model in group.models"
                    :key="model.id"
                    class="flex items-center gap-2 px-2 py-1.5 rounded hover:bg-muted cursor-pointer"
                    @click="toggleUpstreamModelSelection(model.id)"
                  >
                    <div
                      class="w-4 h-4 border rounded flex items-center justify-center shrink-0"
                      :class="isUpstreamModelSelected(model.id) ? 'bg-primary border-primary' : ''"
                    >
                      <Check
                        v-if="isUpstreamModelSelected(model.id)"
                        class="w-3 h-3 text-primary-foreground"
                      />
                    </div>
                    <div class="flex-1 min-w-0">
                      <p class="text-sm font-medium truncate">{{ model.id }}</p>
                      <p class="text-xs text-muted-foreground truncate font-mono">{{ model.owned_by || model.id }}</p>
                    </div>
                  </div>
                </div>
              </div>

              <!-- 空状态 -->
              <div
                v-if="filteredGlobalModels.length === 0 && filteredUpstreamGroups.length === 0"
                class="flex flex-col items-center justify-center py-12 text-muted-foreground"
              >
                <Layers class="w-10 h-10 mb-2 opacity-30" />
                <p class="text-sm">{{ searchQuery ? '无匹配结果' : '暂无可用模型' }}</p>
                <p
                  v-if="!upstreamModelsLoaded"
                  class="text-xs mt-1"
                >
                  点击上方按钮从上游获取模型
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
          {{ hasChanges ? `${pendingChangesCount} 项更改待保存` : '' }}
        </p>
        <div class="flex items-center gap-2">
          <Button
            :disabled="!hasChanges || saving"
            @click="handleSave"
          >
            <Loader2
              v-if="saving"
              class="w-4 h-4 mr-1 animate-spin"
            />
            {{ saving ? '保存中...' : '保存' }}
          </Button>
          <Button
            variant="outline"
            @click="handleClose"
          >
            关闭
          </Button>
        </div>
      </div>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { ref, computed, watch } from 'vue'
import { Layers, Loader2, ChevronDown, Zap, RefreshCw, Search, Check } from 'lucide-vue-next'
import Dialog from '@/components/ui/dialog/Dialog.vue'
import Button from '@/components/ui/button.vue'
import Input from '@/components/ui/input.vue'
import { useToast } from '@/composables/useToast'
import { useConfirm } from '@/composables/useConfirm'
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

const props = defineProps<{
  open: boolean
  providerId: string
  providerName?: string
}>()

const emit = defineEmits<{
  'update:open': [value: boolean]
  'changed': []
}>()

const { fetchModels: fetchCachedModels, clearCache, getCachedModels } = useUpstreamModelsCache()

const { error: showError, success } = useToast()
const { confirmWarning } = useConfirm()

// 状态
const loadingGlobalModels = ref(false)
const fetchingUpstreamModels = ref(false)
const upstreamModelsLoaded = ref(false)
const saving = ref(false)

// 数据
const allGlobalModels = ref<GlobalModelResponse[]>([])
const existingModels = ref<Model[]>([])
const upstreamModels = ref<UpstreamModel[]>([])

// 选择状态（本地状态，保存时才提交）
const selectedGlobalModelIds = ref<Set<string>>(new Set())
const selectedUpstreamModelIds = ref<Set<string>>(new Set())

// 初始状态（用于计算变更）
const initialGlobalModelIds = ref<Set<string>>(new Set())
const initialUpstreamModelNames = ref<Set<string>>(new Set())

// 折叠状态
const collapsedGroups = ref<Set<string>>(new Set())

// 搜索状态
const searchQuery = ref('')

// 已关联的全局模型 ID 集合（从已有数据计算）
const existingGlobalModelIds = computed(() => {
  return new Set(
    existingModels.value
      .filter(m => m.global_model_id)
      .map(m => m.global_model_id)
  )
})

// 已关联的上游模型名称集合
const existingUpstreamModelNames = computed(() => {
  const names = new Set<string>()
  for (const m of existingModels.value) {
    names.add(m.provider_model_name)
    for (const mapping of m.provider_model_mappings ?? []) {
      if (mapping.name) names.add(mapping.name)
    }
  }
  return names
})

// 过滤后的全局模型
const filteredGlobalModels = computed(() => {
  const query = searchQuery.value.toLowerCase().trim()
  return allGlobalModels.value.filter(m => {
    if (query && !m.name.toLowerCase().includes(query) && !m.display_name.toLowerCase().includes(query)) {
      return false
    }
    return true
  })
})

// 过滤后的上游模型（按 API 格式分组）
const filteredUpstreamGroups = computed(() => {
  if (!upstreamModelsLoaded.value) return []

  const query = searchQuery.value.toLowerCase().trim()
  const groups: Record<string, UpstreamModel[]> = {}

  for (const model of upstreamModels.value) {
    if (query && !model.id.toLowerCase().includes(query)) continue

    const format = model.api_format || 'unknown'
    if (!groups[format]) groups[format] = []
    groups[format].push(model)
  }

  const order = Object.keys(API_FORMAT_LABELS)
  return Object.entries(groups)
    .map(([api_format, models]) => ({ api_format, models }))
    .filter(g => g.models.length > 0)
    .sort((a, b) => {
      const aIndex = order.indexOf(a.api_format)
      const bIndex = order.indexOf(b.api_format)
      if (aIndex === -1 && bIndex === -1) return a.api_format.localeCompare(b.api_format)
      if (aIndex === -1) return 1
      if (bIndex === -1) return -1
      return aIndex - bIndex
    })
})

// 检查全局模型是否已选中
function isGlobalModelSelected(globalModelId: string): boolean {
  return selectedGlobalModelIds.value.has(globalModelId)
}

// 检查上游模型是否已选中
function isUpstreamModelSelected(modelId: string): boolean {
  return selectedUpstreamModelIds.value.has(modelId)
}

// 全局模型是否全选
const isAllGlobalModelsSelected = computed(() => {
  if (filteredGlobalModels.value.length === 0) return false
  return filteredGlobalModels.value.every(m => isGlobalModelSelected(m.id))
})

// 检查某个上游组是否全选
function isUpstreamGroupAllSelected(apiFormat: string): boolean {
  const group = filteredUpstreamGroups.value.find(g => g.api_format === apiFormat)
  if (!group || group.models.length === 0) return false
  return group.models.every(m => isUpstreamModelSelected(m.id))
}

// 计算待添加的全局模型
const globalModelsToAdd = computed(() => {
  const toAdd: string[] = []
  for (const id of selectedGlobalModelIds.value) {
    if (!initialGlobalModelIds.value.has(id)) {
      toAdd.push(id)
    }
  }
  return toAdd
})

// 计算待移除的全局模型
const globalModelsToRemove = computed(() => {
  const toRemove: string[] = []
  for (const id of initialGlobalModelIds.value) {
    if (!selectedGlobalModelIds.value.has(id)) {
      toRemove.push(id)
    }
  }
  return toRemove
})

// 计算待添加的上游模型
const upstreamModelsToAdd = computed(() => {
  const toAdd: string[] = []
  for (const id of selectedUpstreamModelIds.value) {
    if (!initialUpstreamModelNames.value.has(id)) {
      toAdd.push(id)
    }
  }
  return toAdd
})

// 计算待移除的上游模型
const upstreamModelsToRemove = computed(() => {
  const toRemove: string[] = []
  for (const id of initialUpstreamModelNames.value) {
    if (!selectedUpstreamModelIds.value.has(id)) {
      toRemove.push(id)
    }
  }
  return toRemove
})

// 是否有变更
const hasChanges = computed(() => {
  return globalModelsToAdd.value.length > 0 ||
    globalModelsToRemove.value.length > 0 ||
    upstreamModelsToAdd.value.length > 0 ||
    upstreamModelsToRemove.value.length > 0
})

// 待变更数量
const pendingChangesCount = computed(() => {
  return globalModelsToAdd.value.length +
    globalModelsToRemove.value.length +
    upstreamModelsToAdd.value.length +
    upstreamModelsToRemove.value.length
})

// 切换全局模型选择
function toggleGlobalModelSelection(id: string) {
  if (selectedGlobalModelIds.value.has(id)) {
    selectedGlobalModelIds.value.delete(id)
  } else {
    selectedGlobalModelIds.value.add(id)
  }
  selectedGlobalModelIds.value = new Set(selectedGlobalModelIds.value)
}

// 切换上游模型选择
function toggleUpstreamModelSelection(id: string) {
  if (selectedUpstreamModelIds.value.has(id)) {
    selectedUpstreamModelIds.value.delete(id)
  } else {
    selectedUpstreamModelIds.value.add(id)
  }
  selectedUpstreamModelIds.value = new Set(selectedUpstreamModelIds.value)
}

// 全选/取消全选全局模型
function toggleAllGlobalModels() {
  const allIds = filteredGlobalModels.value.map(m => m.id)
  if (isAllGlobalModelsSelected.value) {
    // 取消全选
    for (const id of allIds) {
      selectedGlobalModelIds.value.delete(id)
    }
  } else {
    // 全选
    for (const id of allIds) {
      selectedGlobalModelIds.value.add(id)
    }
  }
  selectedGlobalModelIds.value = new Set(selectedGlobalModelIds.value)
}

// 全选/取消全选某个上游组
function toggleAllUpstreamGroup(apiFormat: string) {
  const group = filteredUpstreamGroups.value.find(g => g.api_format === apiFormat)
  if (!group) return

  const allIds = group.models.map(m => m.id)
  if (isUpstreamGroupAllSelected(apiFormat)) {
    // 取消全选
    for (const id of allIds) {
      selectedUpstreamModelIds.value.delete(id)
    }
  } else {
    // 全选
    for (const id of allIds) {
      selectedUpstreamModelIds.value.add(id)
    }
  }
  selectedUpstreamModelIds.value = new Set(selectedUpstreamModelIds.value)
}

// 切换折叠状态
function toggleGroupCollapse(group: string) {
  if (collapsedGroups.value.has(group)) {
    collapsedGroups.value.delete(group)
  } else {
    collapsedGroups.value.add(group)
  }
  collapsedGroups.value = new Set(collapsedGroups.value)
}

// 处理关闭
async function handleClose() {
  if (hasChanges.value) {
    const confirmed = await confirmWarning('有未保存的更改，确定要关闭吗？', '放弃更改')
    if (!confirmed) return
  }
  emit('update:open', false)
}

// 处理对话框状态变更
async function handleDialogUpdate(value: boolean) {
  if (!value && hasChanges.value) {
    const confirmed = await confirmWarning('有未保存的更改，确定要关闭吗？', '放弃更改')
    if (!confirmed) return
  }
  emit('update:open', value)
}

// 保存变更
async function handleSave() {
  if (!hasChanges.value || saving.value) return

  saving.value = true
  try {
    let totalSuccess = 0
    const allErrors: string[] = []

    // 移除全局模型
    for (const globalModelId of globalModelsToRemove.value) {
      const existingModel = existingModels.value.find(m => m.global_model_id === globalModelId)
      if (existingModel) {
        try {
          await deleteModel(props.providerId, existingModel.id)
          totalSuccess++
        } catch (err: any) {
          allErrors.push(parseApiError(err, '移除失败'))
        }
      }
    }

    // 移除上游模型
    for (const modelId of upstreamModelsToRemove.value) {
      const existingModel = existingModels.value.find(m =>
        m.provider_model_name === modelId ||
        m.provider_model_mappings?.some(mapping => mapping.name === modelId)
      )
      if (existingModel) {
        try {
          await deleteModel(props.providerId, existingModel.id)
          totalSuccess++
        } catch (err: any) {
          allErrors.push(parseApiError(err, '移除失败'))
        }
      }
    }

    // 添加全局模型
    if (globalModelsToAdd.value.length > 0) {
      const result = await batchAssignModelsToProvider(props.providerId, globalModelsToAdd.value)
      totalSuccess += result.success.length
      if (result.errors.length > 0) {
        allErrors.push(...result.errors.map(e => e.error))
      }
    }

    // 添加上游模型
    if (upstreamModelsToAdd.value.length > 0) {
      const result = await importModelsFromUpstream(props.providerId, upstreamModelsToAdd.value)
      totalSuccess += result.success.length
      if (result.errors.length > 0) {
        allErrors.push(...result.errors.map(e => e.error))
      }
    }

    if (totalSuccess > 0) {
      success(`成功处理 ${totalSuccess} 个模型`)
    }

    if (allErrors.length > 0) {
      showError(`部分操作失败: ${allErrors.slice(0, 3).join(', ')}${allErrors.length > 3 ? '...' : ''}`, '警告')
    }

    emit('changed')
    emit('update:open', false)
  } catch (err: any) {
    showError(parseApiError(err, '保存失败'), '错误')
  } finally {
    saving.value = false
  }
}

// 从已有数据同步选择状态（全局模型）
function syncGlobalModelSelection() {
  const globalIds = [...existingGlobalModelIds.value].filter((id): id is string => id !== undefined)
  selectedGlobalModelIds.value = new Set(globalIds)
  initialGlobalModelIds.value = new Set(globalIds)
}

// 从已有数据同步选择状态（上游模型）
function syncUpstreamModelSelection() {
  // 只同步当前已加载的上游模型中，与已关联模型匹配的部分
  const selected = new Set<string>()
  for (const model of upstreamModels.value) {
    if (existingUpstreamModelNames.value.has(model.id)) {
      selected.add(model.id)
    }
  }
  selectedUpstreamModelIds.value = selected
  initialUpstreamModelNames.value = new Set(selected)
}

// 监听打开状态
watch(() => props.open, async (isOpen) => {
  if (isOpen && props.providerId) {
    await loadData()
  } else {
    // 重置状态
    upstreamModels.value = []
    upstreamModelsLoaded.value = false
    collapsedGroups.value = new Set()
    searchQuery.value = ''
    selectedGlobalModelIds.value = new Set()
    selectedUpstreamModelIds.value = new Set()
    initialGlobalModelIds.value = new Set()
    initialUpstreamModelNames.value = new Set()
  }
})

// 加载数据
async function loadData() {
  await Promise.all([loadGlobalModels(), loadExistingModels()])

  // 同步全局模型选择状态
  syncGlobalModelSelection()

  // 检查缓存
  const cachedModels = getCachedModels(props.providerId)
  if (cachedModels && cachedModels.length > 0) {
    upstreamModels.value = cachedModels
    upstreamModelsLoaded.value = true
    // 同步上游模型选择状态
    syncUpstreamModelSelection()
    // 有多个分组时全部折叠
    const allGroups = new Set(['global'])
    for (const model of cachedModels) {
      if (model.api_format) {
        allGroups.add(model.api_format)
      }
    }
    collapsedGroups.value = allGroups
  } else {
    collapsedGroups.value = new Set()
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
        // 同步上游模型选择状态
        syncUpstreamModelSelection()
        // 有多个分组时全部折叠
        const allGroups = new Set(['global'])
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
</script>
