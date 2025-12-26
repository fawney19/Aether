<template>
  <Dialog
    :model-value="open"
    :title="editingGroup ? '编辑模型映射' : '添加模型映射'"
    :description="editingGroup ? '修改映射配置' : '为模型添加新的名称映射'"
    :icon="Tag"
    size="4xl"
    @update:model-value="$emit('update:open', $event)"
  >
    <div class="space-y-4">
      <!-- 第一行：目标模型 | 作用域 -->
      <div class="flex gap-4">
        <!-- 目标模型 -->
        <div class="flex-1 space-y-1.5">
          <Label class="text-xs">目标模型</Label>
          <Select
            v-model:open="modelSelectOpen"
            :model-value="formData.modelId"
            :disabled="!!editingGroup"
            @update:model-value="handleModelChange"
          >
            <SelectTrigger class="h-9">
              <SelectValue placeholder="请选择模型" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem
                v-for="model in models"
                :key="model.id"
                :value="model.id"
              >
                {{ model.global_model_display_name || model.provider_model_name }}
              </SelectItem>
            </SelectContent>
          </Select>
        </div>

        <!-- 作用域 -->
        <div class="flex-1 space-y-1.5">
          <Label class="text-xs">作用域 <span class="text-muted-foreground font-normal">(不选则适用全部)</span></Label>
          <div
            v-if="providerApiFormats.length > 0"
            class="flex flex-wrap gap-1.5 p-2 rounded-md border bg-muted/30 min-h-[36px]"
          >
            <button
              v-for="format in providerApiFormats"
              :key="format"
              type="button"
              class="px-2.5 py-0.5 rounded text-xs font-medium transition-colors"
              :class="[
                formData.apiFormats.includes(format)
                  ? 'bg-primary text-primary-foreground'
                  : 'bg-background border border-border hover:bg-muted'
              ]"
              @click="toggleApiFormat(format)"
            >
              {{ API_FORMAT_LABELS[format] || format }}
            </button>
          </div>
          <div
            v-else
            class="h-9 flex items-center text-xs text-muted-foreground"
          >
            无可用格式
          </div>
        </div>
      </div>

      <!-- 第二行：两栏布局 -->
      <div class="flex gap-4 items-stretch">
        <!-- 左侧：上游模型列表 -->
        <div class="flex-1 space-y-2">
          <div class="flex items-center justify-between gap-2">
            <span class="text-sm font-medium shrink-0">
              上游模型
            </span>
            <div class="flex-1 relative">
              <Search class="absolute left-2 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-muted-foreground" />
              <Input
                v-model="upstreamModelSearch"
                placeholder="搜索模型..."
                class="pl-7 h-7 text-xs"
              />
            </div>
            <button
              v-if="upstreamModelsLoaded"
              type="button"
              class="p-1.5 hover:bg-muted rounded-md transition-colors shrink-0"
              title="刷新列表"
              :disabled="refreshingUpstreamModels"
              @click="refreshUpstreamModels"
            >
              <RefreshCw
                class="w-3.5 h-3.5"
                :class="{ 'animate-spin': refreshingUpstreamModels }"
              />
            </button>
            <button
              v-else-if="!fetchingUpstreamModels"
              type="button"
              class="p-1.5 hover:bg-muted rounded-md transition-colors shrink-0"
              title="获取上游模型列表"
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
            <template v-if="upstreamModelsLoaded">
              <div
                v-if="groupedAvailableUpstreamModels.length === 0"
                class="flex flex-col items-center justify-center h-full text-muted-foreground"
              >
                <Zap class="w-10 h-10 mb-2 opacity-30" />
                <p class="text-sm">
                  {{ upstreamModelSearch ? '没有匹配的模型' : '所有模型已添加' }}
                </p>
              </div>
              <div
                v-else
                class="p-2 space-y-2"
              >
                <!-- 按分组显示（可折叠） -->
                <div
                  v-for="group in groupedAvailableUpstreamModels"
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
                  </div>
                  <div
                    v-show="!collapsedGroups.has(group.api_format)"
                    class="p-2 space-y-1 border-t"
                  >
                    <div
                      v-for="model in group.models"
                      :key="model.id"
                      class="flex items-center gap-2 p-2 rounded-lg border transition-colors hover:bg-muted/30"
                      :title="model.id"
                    >
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
                      <button
                        type="button"
                        class="p-1 hover:bg-primary/10 rounded transition-colors shrink-0"
                        title="添加到映射"
                        @click="addUpstreamModel(model.id)"
                      >
                        <ChevronRight class="w-4 h-4 text-muted-foreground hover:text-primary" />
                      </button>
                    </div>
                  </div>
                </div>
              </div>
            </template>

            <!-- 未加载状态 -->
            <div
              v-else
              class="flex flex-col items-center justify-center h-full text-muted-foreground"
            >
              <Zap class="w-10 h-10 mb-2 opacity-30" />
              <p class="text-sm">
                点击右上角按钮
              </p>
              <p class="text-xs mt-1">
                从上游获取可用模型
              </p>
            </div>
          </div>
        </div>

        <!-- 右侧：映射名称列表 -->
        <div class="flex-1 space-y-2">
          <div class="flex items-center justify-between">
            <p class="text-sm font-medium">
              映射名称
            </p>
            <button
              type="button"
              class="p-1.5 hover:bg-muted rounded-md transition-colors"
              title="手动添加"
              @click="addAliasItem"
            >
              <Plus class="w-3.5 h-3.5" />
            </button>
          </div>
          <div class="border rounded-lg h-80 overflow-y-auto">
            <div
              v-if="formData.aliases.length === 0"
              class="flex flex-col items-center justify-center h-full text-muted-foreground"
            >
              <Tag class="w-10 h-10 mb-2 opacity-30" />
              <p class="text-sm">
                从左侧选择模型
              </p>
              <p class="text-xs mt-1">
                或点击上方"手动添加"
              </p>
            </div>
            <div
              v-else
              class="p-2 space-y-1"
            >
              <div
                v-for="(alias, index) in formData.aliases"
                :key="`alias-${index}`"
                class="group flex items-center gap-2 p-2 rounded-lg border transition-colors hover:bg-muted/30"
                :class="[
                  draggedIndex === index ? 'bg-primary/5' : '',
                  dragOverIndex === index ? 'bg-primary/10 border-primary' : ''
                ]"
                draggable="true"
                @dragstart="handleDragStart(index, $event)"
                @dragend="handleDragEnd"
                @dragover.prevent="handleDragOver(index)"
                @dragleave="handleDragLeave"
                @drop="handleDrop(index)"
              >
                <!-- 删除按钮 -->
                <button
                  type="button"
                  class="p-1 hover:bg-destructive/10 rounded transition-colors shrink-0"
                  title="移除"
                  @click="removeAliasItem(index)"
                >
                  <ChevronLeft class="w-4 h-4 text-muted-foreground hover:text-destructive" />
                </button>

                <!-- 优先级 -->
                <div class="shrink-0">
                  <input
                    v-if="editingPriorityIndex === index"
                    type="number"
                    min="1"
                    :value="alias.priority"
                    class="w-7 h-6 rounded bg-background border border-primary text-xs text-center focus:outline-none [appearance:textfield] [&::-webkit-outer-spin-button]:appearance-none [&::-webkit-inner-spin-button]:appearance-none"
                    autofocus
                    @blur="finishEditPriority(index, $event)"
                    @keydown.enter="($event.target as HTMLInputElement).blur()"
                    @keydown.escape="cancelEditPriority"
                  >
                  <div
                    v-else
                    class="w-6 h-6 rounded bg-muted/50 flex items-center justify-center text-xs text-muted-foreground cursor-pointer hover:bg-primary/10 hover:text-primary"
                    title="点击编辑优先级"
                    @click.stop="startEditPriority(index)"
                  >
                    {{ alias.priority }}
                  </div>
                </div>

                <!-- 名称显示/编辑 -->
                <div class="flex-1 min-w-0">
                  <Input
                    v-if="alias.isEditing"
                    v-model="alias.name"
                    placeholder="输入映射名称"
                    class="h-7 text-xs"
                    autofocus
                    @blur="alias.isEditing = false"
                    @keydown.enter="alias.isEditing = false"
                  />
                  <p
                    v-else
                    class="font-medium text-sm truncate cursor-pointer hover:text-primary"
                    title="点击编辑"
                    @click="alias.isEditing = true"
                  >
                    {{ alias.name || '点击输入名称' }}
                  </p>
                </div>

                <!-- 拖拽手柄 -->
                <div class="cursor-grab active:cursor-grabbing text-muted-foreground/30 group-hover:text-muted-foreground shrink-0">
                  <GripVertical class="w-4 h-4" />
                </div>
              </div>
            </div>
            <!-- 拖拽提示 -->
            <div
              v-if="formData.aliases.length > 1"
              class="px-3 py-1.5 bg-muted/30 border-t text-xs text-muted-foreground text-center"
            >
              拖拽调整优先级顺序
            </div>
          </div>
        </div>
      </div>
    </div>

    <template #footer>
      <Button
        variant="outline"
        @click="$emit('update:open', false)"
      >
        取消
      </Button>
      <Button
        :disabled="submitting || !formData.modelId || formData.aliases.length === 0 || !hasValidAliases"
        @click="handleSubmit"
      >
        <Loader2
          v-if="submitting"
          class="w-4 h-4 mr-2 animate-spin"
        />
        {{ editingGroup ? '保存' : '添加' }}
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { ref, computed, watch } from 'vue'
import { Tag, Loader2, GripVertical, Zap, Search, RefreshCw, ChevronDown, ChevronRight, ChevronLeft, Plus } from 'lucide-vue-next'
import {
  Button,
  Input,
  Label,
  Dialog,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui'
import { useToast } from '@/composables/useToast'
import {
  API_FORMAT_LABELS,
  type Model,
  type ProviderModelAlias
} from '@/api/endpoints'
import { updateModel } from '@/api/endpoints/models'
import { useUpstreamModelsCache, type UpstreamModel } from '../composables/useUpstreamModelsCache'

interface FormAlias {
  name: string
  priority: number
  isEditing?: boolean
}

export interface AliasGroup {
  model: Model
  apiFormatsKey: string
  apiFormats: string[]
  aliases: ProviderModelAlias[]
}

const props = defineProps<{
  open: boolean
  providerId: string
  providerApiFormats: string[]
  models: Model[]
  editingGroup?: AliasGroup | null
}>()

const emit = defineEmits<{
  'update:open': [value: boolean]
  'saved': []
}>()

const { error: showError, success: showSuccess } = useToast()
const { fetchModels: fetchCachedModels, clearCache, getCachedModels } = useUpstreamModelsCache()

// 状态
const submitting = ref(false)
const modelSelectOpen = ref(false)

// 拖拽状态
const draggedIndex = ref<number | null>(null)
const dragOverIndex = ref<number | null>(null)

// 优先级编辑状态
const editingPriorityIndex = ref<number | null>(null)

// 快速添加（上游模型）状态
const fetchingUpstreamModels = ref(false)
const refreshingUpstreamModels = ref(false)
const upstreamModelsLoaded = ref(false)
const upstreamModels = ref<UpstreamModel[]>([])
const upstreamModelSearch = ref('')

// 分组折叠状态
const collapsedGroups = ref<Set<string>>(new Set())

// 表单数据
const formData = ref<{
  modelId: string
  apiFormats: string[]
  aliases: FormAlias[]
}>({
  modelId: '',
  apiFormats: [],
  aliases: []
})

// 检查是否有有效的映射
const hasValidAliases = computed(() => {
  return formData.value.aliases.some(a => a.name.trim())
})

// 过滤和排序后的上游模型列表
const filteredUpstreamModels = computed(() => {
  const searchText = upstreamModelSearch.value.toLowerCase().trim()
  let result = [...upstreamModels.value]

  result.sort((a, b) => a.id.localeCompare(b.id))

  if (searchText) {
    const keywords = searchText.split(/\s+/).filter(k => k.length > 0)
    result = result.filter(m => {
      const searchableText = `${m.id} ${m.owned_by || ''} ${m.api_format || ''}`.toLowerCase()
      return keywords.every(keyword => searchableText.includes(keyword))
    })
  }

  return result
})

// 按 API 格式分组的上游模型列表
interface UpstreamModelGroup {
  api_format: string
  models: Array<{ id: string; owned_by?: string; api_format?: string }>
}

const groupedAvailableUpstreamModels = computed<UpstreamModelGroup[]>(() => {
  const addedNames = new Set(formData.value.aliases.map(a => a.name.trim()))
  const availableModels = filteredUpstreamModels.value.filter(m => !addedNames.has(m.id))

  const groups = new Map<string, UpstreamModelGroup>()

  for (const model of availableModels) {
    const format = model.api_format || 'UNKNOWN'
    if (!groups.has(format)) {
      groups.set(format, { api_format: format, models: [] })
    }
    groups.get(format)!.models.push(model)
  }

  const order = Object.keys(API_FORMAT_LABELS)
  return Array.from(groups.values()).sort((a, b) => {
    const aIndex = order.indexOf(a.api_format)
    const bIndex = order.indexOf(b.api_format)
    if (aIndex === -1 && bIndex === -1) return a.api_format.localeCompare(b.api_format)
    if (aIndex === -1) return 1
    if (bIndex === -1) return -1
    return aIndex - bIndex
  })
})

// 监听打开状态
watch(() => props.open, (isOpen) => {
  if (isOpen) {
    initForm()
  }
})

// 初始化表单
function initForm() {
  if (props.editingGroup) {
    formData.value = {
      modelId: props.editingGroup.model.id,
      apiFormats: [...props.editingGroup.apiFormats],
      aliases: props.editingGroup.aliases.map(a => ({ name: a.name, priority: a.priority }))
    }
  } else {
    formData.value = {
      modelId: '',
      apiFormats: [],
      aliases: []
    }
  }
  // 重置状态
  editingPriorityIndex.value = null
  draggedIndex.value = null
  dragOverIndex.value = null
  upstreamModelSearch.value = ''
  collapsedGroups.value = new Set()

  // 检查缓存，如果有缓存数据则直接使用
  const cachedModels = getCachedModels(props.providerId)
  if (cachedModels) {
    upstreamModels.value = cachedModels
    upstreamModelsLoaded.value = true
    // 默认折叠所有分组
    for (const model of cachedModels) {
      if (model.api_format) {
        collapsedGroups.value.add(model.api_format)
      }
    }
  } else {
    upstreamModelsLoaded.value = false
    upstreamModels.value = []
  }
}

// 处理模型选择变更
function handleModelChange(value: string) {
  formData.value.modelId = value
  const selectedModel = props.models.find(m => m.id === value)
  if (selectedModel) {
    upstreamModelSearch.value = selectedModel.provider_model_name
  }
}

// 切换 API 格式
function toggleApiFormat(format: string) {
  const index = formData.value.apiFormats.indexOf(format)
  if (index >= 0) {
    formData.value.apiFormats.splice(index, 1)
  } else {
    formData.value.apiFormats.push(format)
  }
}

// 切换分组折叠状态
function toggleGroupCollapse(apiFormat: string) {
  if (collapsedGroups.value.has(apiFormat)) {
    collapsedGroups.value.delete(apiFormat)
  } else {
    collapsedGroups.value.add(apiFormat)
  }
}

// 添加映射项
function addAliasItem() {
  const maxPriority = formData.value.aliases.length > 0
    ? Math.max(...formData.value.aliases.map(a => a.priority))
    : 0
  formData.value.aliases.push({ name: '', priority: maxPriority + 1, isEditing: true })
}

// 删除映射项
function removeAliasItem(index: number) {
  formData.value.aliases.splice(index, 1)
}

// ===== 拖拽排序 =====
function handleDragStart(index: number, event: DragEvent) {
  draggedIndex.value = index
  if (event.dataTransfer) {
    event.dataTransfer.effectAllowed = 'move'
  }
}

function handleDragEnd() {
  draggedIndex.value = null
  dragOverIndex.value = null
}

function handleDragOver(index: number) {
  if (draggedIndex.value !== null && draggedIndex.value !== index) {
    dragOverIndex.value = index
  }
}

function handleDragLeave() {
  dragOverIndex.value = null
}

function handleDrop(targetIndex: number) {
  const dragIndex = draggedIndex.value
  if (dragIndex === null || dragIndex === targetIndex) {
    dragOverIndex.value = null
    return
  }

  const items = [...formData.value.aliases]
  const draggedItem = items[dragIndex]

  const originalPriorityMap = new Map<number, number>()
  items.forEach((alias, idx) => {
    originalPriorityMap.set(idx, alias.priority)
  })

  items.splice(dragIndex, 1)
  items.splice(targetIndex, 0, draggedItem)

  const groupNewPriority = new Map<number, number>()
  let currentPriority = 1

  items.forEach((alias) => {
    const originalIdx = formData.value.aliases.findIndex(a => a === alias)
    const originalPriority = originalIdx >= 0 ? originalPriorityMap.get(originalIdx)! : alias.priority

    if (alias === draggedItem) {
      alias.priority = currentPriority
      currentPriority++
    } else {
      if (groupNewPriority.has(originalPriority)) {
        alias.priority = groupNewPriority.get(originalPriority)!
      } else {
        groupNewPriority.set(originalPriority, currentPriority)
        alias.priority = currentPriority
        currentPriority++
      }
    }
  })

  formData.value.aliases = items
  draggedIndex.value = null
  dragOverIndex.value = null
}

// ===== 优先级编辑 =====
function startEditPriority(index: number) {
  editingPriorityIndex.value = index
}

function finishEditPriority(index: number, event: FocusEvent) {
  const input = event.target as HTMLInputElement
  const newPriority = parseInt(input.value) || 1
  formData.value.aliases[index].priority = Math.max(1, newPriority)
  editingPriorityIndex.value = null
}

function cancelEditPriority() {
  editingPriorityIndex.value = null
}

// ===== 快速添加（上游模型）=====
async function fetchUpstreamModels() {
  if (!props.providerId) return

  upstreamModelSearch.value = ''
  fetchingUpstreamModels.value = true

  try {
    const result = await fetchCachedModels(props.providerId)
    if (result) {
      if (result.error) {
        showError(result.error, '错误')
      } else {
        upstreamModels.value = result.models
        upstreamModelsLoaded.value = true
        // 默认折叠所有分组
        for (const model of result.models) {
          if (model.api_format) {
            collapsedGroups.value.add(model.api_format)
          }
        }
      }
    }
  } finally {
    fetchingUpstreamModels.value = false
  }
}

function addUpstreamModel(modelId: string) {
  if (formData.value.aliases.some(a => a.name === modelId)) {
    return
  }

  const maxPriority = formData.value.aliases.length > 0
    ? Math.max(...formData.value.aliases.map(a => a.priority))
    : 0

  formData.value.aliases.push({ name: modelId, priority: maxPriority + 1 })
}

async function refreshUpstreamModels() {
  if (!props.providerId || refreshingUpstreamModels.value) return

  refreshingUpstreamModels.value = true
  clearCache(props.providerId)

  try {
    const result = await fetchCachedModels(props.providerId, true)
    if (result) {
      if (result.error) {
        showError(result.error, '错误')
      } else {
        upstreamModels.value = result.models
      }
    }
  } finally {
    refreshingUpstreamModels.value = false
  }
}

// 生成作用域唯一键
function getApiFormatsKey(formats: string[] | undefined): string {
  if (!formats || formats.length === 0) return ''
  return [...formats].sort().join(',')
}

// 提交表单
async function handleSubmit() {
  if (submitting.value) return
  if (!formData.value.modelId || formData.value.aliases.length === 0) return

  const validAliases = formData.value.aliases.filter(a => a.name.trim())
  if (validAliases.length === 0) {
    showError('请至少添加一个有效的映射名称', '错误')
    return
  }

  submitting.value = true
  try {
    const targetModel = props.models.find(m => m.id === formData.value.modelId)
    if (!targetModel) {
      showError('模型不存在', '错误')
      return
    }

    const currentAliases = targetModel.provider_model_mappings || []
    let newAliases: ProviderModelAlias[]

    const buildAlias = (a: FormAlias): ProviderModelAlias => ({
      name: a.name.trim(),
      priority: a.priority,
      ...(formData.value.apiFormats.length > 0 ? { api_formats: formData.value.apiFormats } : {})
    })

    if (props.editingGroup) {
      const oldApiFormatsKey = props.editingGroup.apiFormatsKey
      const oldAliasNames = new Set(props.editingGroup.aliases.map(a => a.name))

      const filteredAliases = currentAliases.filter((a: ProviderModelAlias) => {
        const currentKey = getApiFormatsKey(a.api_formats)
        return !(currentKey === oldApiFormatsKey && oldAliasNames.has(a.name))
      })

      const existingNames = new Set(filteredAliases.map((a: ProviderModelAlias) => a.name))
      const duplicates = validAliases.filter(a => existingNames.has(a.name.trim()))
      if (duplicates.length > 0) {
        showError(`以下映射名称已存在：${duplicates.map(d => d.name).join(', ')}`, '错误')
        return
      }

      newAliases = [
        ...filteredAliases,
        ...validAliases.map(buildAlias)
      ]
    } else {
      const existingNames = new Set(currentAliases.map((a: ProviderModelAlias) => a.name))
      const duplicates = validAliases.filter(a => existingNames.has(a.name.trim()))
      if (duplicates.length > 0) {
        showError(`以下映射名称已存在：${duplicates.map(d => d.name).join(', ')}`, '错误')
        return
      }
      newAliases = [
        ...currentAliases,
        ...validAliases.map(buildAlias)
      ]
    }

    await updateModel(props.providerId, targetModel.id, {
      provider_model_mappings: newAliases
    })

    showSuccess(props.editingGroup ? '映射组已更新' : '映射已添加')
    emit('update:open', false)
    emit('saved')
  } catch (err: any) {
    showError(err.response?.data?.detail || '操作失败', '错误')
  } finally {
    submitting.value = false
  }
}
</script>
