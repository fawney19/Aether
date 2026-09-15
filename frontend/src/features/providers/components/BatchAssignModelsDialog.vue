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
          <DropdownMenu :modal="false">
            <DropdownMenuTrigger as-child>
              <Button
                variant="ghost"
                size="icon"
                class="h-9 w-9 shrink-0"
                :disabled="loadingGlobalModels || loadingProviderKeys || fetchingAutoMatchedModels || providerKeys.length === 0"
                :title="autoMatchButtonTitle"
                aria-label="按密钥匹配"
              >
                <Loader2
                  v-if="loadingProviderKeys || fetchingAutoMatchedModels"
                  class="w-4 h-4 animate-spin"
                />
                <ListChecks
                  v-else
                  class="w-4 h-4"
                />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent
              align="end"
              class="w-72 max-h-80 overflow-y-auto"
            >
              <DropdownMenuItem
                v-for="key in providerKeys"
                :key="key.id"
                class="flex-col items-start gap-0.5"
                :disabled="fetchingAutoMatchedModels"
                @select="applyAutoMatchFromKey(key)"
              >
                <span class="w-full truncate font-medium">
                  {{ getAutoMatchKeyLabel(key) }}
                </span>
                <span
                  v-if="getAutoMatchKeyDetail(key)"
                  class="w-full truncate text-xs text-muted-foreground"
                >
                  {{ getAutoMatchKeyDetail(key) }}
                </span>
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>

        <!-- 模型列表 -->
        <div class="border rounded-lg overflow-hidden">
          <div class="max-h-96 overflow-y-auto">
            <div
              v-if="loadingGlobalModels"
              class="flex items-center justify-center py-12"
            >
              <Loader2 class="w-6 h-6 animate-spin text-primary" />
            </div>

            <template v-else>
              <!-- 全局模型列表 -->
              <div v-if="filteredGlobalModels.length > 0">
                <div
                  class="flex items-center justify-between px-3 py-2 bg-muted sticky top-0 z-10"
                >
                  <div class="flex items-center gap-2">
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
                <div class="space-y-1 p-2">
                  <div
                    v-for="model in filteredGlobalModels"
                    :key="model.id"
                    class="flex items-center gap-2 px-2 py-1.5 rounded hover:bg-muted cursor-pointer"
                    :data-testid="`batch-assign-model-${model.id}`"
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
                      <p class="text-sm font-medium truncate">
                        {{ model.display_name }}
                      </p>
                      <p class="text-xs text-muted-foreground truncate font-mono">
                        {{ model.name }}
                      </p>
                    </div>
                  </div>
                </div>
              </div>

              <!-- 空状态 -->
              <div
                v-if="filteredGlobalModels.length === 0"
                class="flex flex-col items-center justify-center py-12 text-muted-foreground"
              >
                <Layers class="w-10 h-10 mb-2 opacity-30" />
                <p class="text-sm">
                  {{ searchQuery ? '无匹配结果' : '暂无可用全局模型' }}
                </p>
                <p class="text-xs mt-1">
                  请先前往"模型目录"页面创建全局模型
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
import { Layers, Loader2, Search, Check, ListChecks } from 'lucide-vue-next'
import Dialog from '@/components/ui/dialog/Dialog.vue'
import Button from '@/components/ui/button.vue'
import Input from '@/components/ui/input.vue'
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem,
} from '@/components/ui'
import { useToast } from '@/composables/useToast'
import { useConfirm } from '@/composables/useConfirm'
import { parseApiError } from '@/utils/errorParser'
import { useUpstreamModelsCache } from '../composables/useUpstreamModelsCache'
import {
  getGlobalModels,
  type GlobalModelResponse
} from '@/api/endpoints/global-models'
import {
  getProviderModels,
  getProviderKeys,
  batchAssignModelsToProvider,
  deleteModel,
  type Model,
  type EndpointAPIKey
} from '@/api/endpoints'

type AutoMatchKey = Pick<EndpointAPIKey, 'id' | 'name' | 'api_key_masked'>

interface Props {
  open: boolean
  providerId: string
  providerName?: string
}

const props = defineProps<Props>()

const emit = defineEmits<{
  'update:open': [value: boolean]
  'changed': []
}>()

interface AutoMatchKeyLike {
  id: string
  name?: string | null
  api_key_masked?: string | null
}

const { error: showError, success, warning: showWarning } = useToast()
const { confirmWarning } = useConfirm()
const { fetchModels: fetchCachedModels } = useUpstreamModelsCache()

// 状态
const loadingGlobalModels = ref(false)
const loadingProviderKeys = ref(false)
const saving = ref(false)
const fetchingAutoMatchedModels = ref(false)

// 数据
const allGlobalModels = ref<GlobalModelResponse[]>([])
const existingModels = ref<Model[]>([])
const providerKeys = ref<AutoMatchKey[]>([])

// 选择状态（本地状态，保存时才提交）
const selectedGlobalModelIds = ref<Set<string>>(new Set())

// 初始状态（用于计算变更）
const initialGlobalModelIds = ref<Set<string>>(new Set())

// 搜索状态
const searchQuery = ref('')

const autoMatchButtonTitle = computed(() => {
  if (loadingProviderKeys.value) return '正在加载密钥'
  if (providerKeys.value.length === 0) return '暂无可用于匹配的密钥'
  return '选择密钥，并按该密钥的上游模型自动勾选同名模型'
})

// 已关联的全局模型 ID 集合（从已有数据计算）
const existingGlobalModelIds = computed(() => {
  return new Set(
    existingModels.value
      .map(m => m.global_model_id)
  )
})

function globalModelMatchesQuery(model: GlobalModelResponse, query: string): boolean {
  if (!query) return true
  return model.name.toLowerCase().includes(query) || model.display_name.toLowerCase().includes(query)
}

function compareGlobalModelsByName(left: GlobalModelResponse, right: GlobalModelResponse): number {
  const nameA = (left.display_name || left.name || '').toLowerCase()
  const nameB = (right.display_name || right.name || '').toLowerCase()
  return nameA.localeCompare(nameB)
}

// 过滤后的全局模型：当前已勾选/已关联的排在可见结果顶部，便于取消关联
const filteredGlobalModels = computed(() => {
  const query = searchQuery.value.toLowerCase().trim()
  const selectedIds = selectedGlobalModelIds.value
  const matched = allGlobalModels.value.filter(model => globalModelMatchesQuery(model, query))
  const pinned = matched
    .filter(model => selectedIds.has(model.id))
    .sort(compareGlobalModelsByName)
  const rest = matched
    .filter(model => !selectedIds.has(model.id))
    .sort(compareGlobalModelsByName)
  return [...pinned, ...rest]
})

// 全局模型是否全选
const isAllGlobalModelsSelected = computed(() => {
  if (filteredGlobalModels.value.length === 0) return false
  return filteredGlobalModels.value.every(m => isGlobalModelSelected(m.id))
})

// 检查全局模型是否已选中
function isGlobalModelSelected(globalModelId: string): boolean {
  return selectedGlobalModelIds.value.has(globalModelId)
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

// 是否有变更
const hasChanges = computed(() => {
  return globalModelsToAdd.value.length > 0 ||
    globalModelsToRemove.value.length > 0
})

// 待变更数量
const pendingChangesCount = computed(() => {
  return globalModelsToAdd.value.length +
    globalModelsToRemove.value.length
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

// 全选/取消全选全局模型
function toggleAllGlobalModels() {
  const allIds = filteredGlobalModels.value.map(m => m.id)
  if (isAllGlobalModelsSelected.value) {
    for (const id of allIds) {
      selectedGlobalModelIds.value.delete(id)
    }
  } else {
    for (const id of allIds) {
      selectedGlobalModelIds.value.add(id)
    }
  }
  selectedGlobalModelIds.value = new Set(selectedGlobalModelIds.value)
}

function normalizeModelName(name: string | null | undefined): string {
  return (name || '').trim()
}

function getAutoMatchKeyLabel(key: AutoMatchKeyLike): string {
  return key.name || key.api_key_masked || key.id.slice(0, 8)
}

function getAutoMatchKeyDetail(key: AutoMatchKeyLike): string {
  if (key.name && key.api_key_masked) return key.api_key_masked
  return key.name ? key.id.slice(0, 8) : ''
}

async function applyAutoMatchFromKey(key: AutoMatchKey) {
  if (!props.providerId || !key || fetchingAutoMatchedModels.value) return

  fetchingAutoMatchedModels.value = true
  try {
    const result = await fetchCachedModels(props.providerId, key.id, true)
    if (!props.open) return

    if (result.warning) {
      showWarning(`部分格式获取失败: ${result.warning}`)
    }

    if (result.models.length === 0) {
      if (result.error) {
        showError(result.error, '获取上游模型失败')
      } else {
        showWarning('此 Key 未返回可用模型')
      }
      return
    }

    const upstreamModelIds = new Set(
      result.models
        .map(model => normalizeModelName(model.id))
        .filter(Boolean)
    )
    const matchedGlobalModelIds = allGlobalModels.value
      .filter(model => upstreamModelIds.has(normalizeModelName(model.name)))
      .map(model => model.id)

    if (matchedGlobalModelIds.length === 0) {
      showWarning('未找到与此 Key 上游模型 ID 同名的全局模型')
      return
    }

    const nextSelected = new Set(selectedGlobalModelIds.value)
    let newlySelectedCount = 0
    for (const id of matchedGlobalModelIds) {
      if (!nextSelected.has(id)) {
        newlySelectedCount++
      }
      nextSelected.add(id)
    }
    selectedGlobalModelIds.value = nextSelected
    searchQuery.value = ''

    if (newlySelectedCount > 0) {
      success(`已按 ${getAutoMatchKeyLabel(key)} 勾选 ${matchedGlobalModelIds.length} 个同名模型`)
    } else {
      success(`${matchedGlobalModelIds.length} 个同名模型已在选中列表中`)
    }
  } catch (err: unknown) {
    showError(parseApiError(err, '自动匹配模型失败'), '错误')
  } finally {
    fetchingAutoMatchedModels.value = false
  }
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
  let hasAnyOperation = false
  try {
    let totalSuccess = 0
    const allErrors: string[] = []

    // 移除全局模型
    for (const globalModelId of globalModelsToRemove.value) {
      const existingModel = existingModels.value.find(m => m.global_model_id === globalModelId)
      if (existingModel) {
        hasAnyOperation = true
        try {
          await deleteModel(props.providerId, existingModel.id)
          totalSuccess++
        } catch (err: unknown) {
          allErrors.push(parseApiError(err, '移除失败'))
        }
      }
    }

    // 添加全局模型
    if (globalModelsToAdd.value.length > 0) {
      hasAnyOperation = true
      try {
        const result = await batchAssignModelsToProvider(props.providerId, globalModelsToAdd.value)
        totalSuccess += result.success.length
        if (result.errors.length > 0) {
          allErrors.push(...result.errors.map(e => e.error))
        }
      } catch (err: unknown) {
        allErrors.push(parseApiError(err, '批量添加全局模型失败'))
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
  } catch (err: unknown) {
    showError(parseApiError(err, '保存失败'), '错误')
    if (hasAnyOperation) {
      emit('changed')
    }
  } finally {
    saving.value = false
  }
}

// 从已有数据同步选择状态
function syncGlobalModelSelection() {
  const globalIds = [...existingGlobalModelIds.value].filter((id): id is string => id !== undefined)
  selectedGlobalModelIds.value = new Set(globalIds)
  initialGlobalModelIds.value = new Set(globalIds)
}

// 监听打开状态
watch(
  () => props.open,
  async (isOpen) => {
    if (isOpen && props.providerId) {
      await loadData()
    } else {
      searchQuery.value = ''
      selectedGlobalModelIds.value = new Set()
      initialGlobalModelIds.value = new Set()
      providerKeys.value = []
      fetchingAutoMatchedModels.value = false
    }
  },
  { immediate: true },
)

// 加载数据
async function loadData() {
  await Promise.all([loadGlobalModels(), loadExistingModels(), loadProviderKeys()])
  syncGlobalModelSelection()
}

// 加载全局模型列表
async function loadGlobalModels() {
  try {
    loadingGlobalModels.value = true
    const response = await getGlobalModels({ limit: 1000 })
    allGlobalModels.value = response.models
  } catch (err: unknown) {
    showError(parseApiError(err, '加载全局模型失败'), '错误')
  } finally {
    loadingGlobalModels.value = false
  }
}

// 加载已关联的模型
async function loadExistingModels() {
  try {
    existingModels.value = await getProviderModels(props.providerId)
  } catch (err: unknown) {
    showError(parseApiError(err, '加载已关联模型失败'), '错误')
  }
}

// 加载密钥列表
async function loadProviderKeys() {
  try {
    loadingProviderKeys.value = true
    providerKeys.value = await getProviderKeys(props.providerId)
  } catch (err: unknown) {
    providerKeys.value = []
    showError(parseApiError(err, '加载密钥失败'), '错误')
  } finally {
    loadingProviderKeys.value = false
  }
}
</script>
