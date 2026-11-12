<template>
  <Dialog
    :model-value="isOpen"
    :title="props.apiKey?.name ? t('ui.modelPermissionsValue', { name: props.apiKey.name }) : t('guide.nav.concepts.modelPermission')"
    :description="t('ui.modelPermissionSelectionHelp')"
    :icon="Shield"
    size="2xl"
    @update:model-value="handleDialogUpdate"
  >
    <template #default>
      <div class="space-y-4">
        <label class="flex items-center gap-2 text-sm cursor-pointer">
          <Checkbox
            :checked="allowAllModels"
            :indeterminate="!allowAllModels && selectedModels.length > 0"
            :aria-label="t('ui.selectAll')"
            @update:checked="toggleAllModels"
          />
          <span>{{ t('ui.selectAll') }}</span>
          <span class="text-muted-foreground">{{ t('ui.allModels') }}</span>
        </label>
        <!-- 搜索栏 -->
        <div class="flex items-center gap-2">
          <div class="flex-1 relative">
            <Search class="absolute left-2.5 top-1/2 -translate-y-1/2 w-4 h-4 text-muted-foreground" />
            <Input
              v-model="searchQuery"
              :placeholder="t('ui.searchModelsOrAddACustomModel')"
              class="pl-8 h-9"
            />
          </div>
          <!-- 已选数量徽章 -->
          <span
            v-if="allowAllModels"
            class="h-7 px-2.5 text-xs rounded-md flex items-center bg-muted text-muted-foreground shrink-0"
          > {{ t('ui.allModels') }} </span>
          <span
            v-else-if="selectedModels.length === 0"
            class="h-7 px-2.5 text-xs rounded-md flex items-center bg-amber-500/10 text-amber-600 dark:text-amber-400 shrink-0"
          > {{ t('ui.noModelsSelected') }} </span>
          <span
            v-else
            class="h-7 px-2.5 text-xs rounded-md flex items-center bg-primary/10 text-primary shrink-0"
          > {{ t('ui.selected') }} {{ selectedModels.length }} {{ t('ui.items3a11681') }} </span>
          <!-- 刷新上游模型按钮 -->
          <button
            v-if="upstreamModelsLoaded"
            type="button"
            class="p-2 hover:bg-muted rounded-md transition-colors shrink-0"
            :disabled="fetchingUpstreamModels"
            :title="t('ui.refreshUpstreamModels')"
            @click="refreshUpstreamModels"
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
            :title="t('ui.fetchModelsFromProvider')"
            @click="refreshUpstreamModels"
          >
            <Zap class="w-4 h-4" />
          </button>
          <Loader2
            v-else
            class="w-4 h-4 animate-spin text-muted-foreground shrink-0"
          />
        </div>

        <!-- 模型列表 -->
        <div class="border rounded-lg overflow-hidden">
          <div class="max-h-96 overflow-y-auto">
            <!-- 加载中 -->
            <div
              v-if="loadingProviderModels || fetchingUpstreamModels"
              class="flex items-center justify-center py-12"
            >
              <Loader2 class="w-6 h-6 animate-spin text-primary" />
            </div>

            <template v-else>
              <!-- 添加自定义模型（搜索内容不在列表中时显示） -->
              <div
                v-if="searchQuery && canAddAsCustom"
                class="px-3 py-2 border-b bg-background sticky top-0 z-30"
              >
                <div
                  class="flex items-center justify-between px-3 py-2 rounded-lg border border-dashed hover:border-primary hover:bg-primary/5 cursor-pointer transition-colors"
                  @click="addCustomModel"
                >
                  <div class="flex items-center gap-2">
                    <Plus class="w-4 h-4 text-muted-foreground" />
                    <span class="text-sm font-mono">{{ searchQuery }}</span>
                  </div>
                  <span class="text-xs text-muted-foreground">{{ t('ui.addCustomModel') }}</span>
                </div>
              </div>

              <!-- 自定义模型 -->
              <div v-if="customModels.length > 0">
                <div
                  class="flex items-center justify-between px-3 py-2 bg-muted sticky top-0 z-20 cursor-pointer hover:bg-muted/80 transition-colors"
                  @click="toggleGroupCollapse('custom')"
                >
                  <div class="flex items-center gap-2">
                    <ChevronDown
                      class="w-4 h-4 transition-transform shrink-0"
                      :class="collapsedGroups.has('custom') ? '-rotate-90' : ''"
                    />
                    <span class="text-xs font-medium">{{ t('ui.customModel') }}</span>
                    <span class="text-xs text-muted-foreground">({{ customModels.length }})</span>
                  </div>
                </div>
                <div
                  v-show="!collapsedGroups.has('custom')"
                  class="space-y-1 p-2"
                >
                  <div
                    v-for="model in sortedCustomModels"
                    :key="model"
                    class="flex items-center gap-2 px-2 py-1.5 rounded hover:bg-muted cursor-pointer"
                    @click="toggleModel(model)"
                  >
                    <div
                      class="w-4 h-4 border rounded flex items-center justify-center shrink-0"
                      :class="isModelSelected(model) ? 'bg-primary border-primary' : ''"
                    >
                      <Check
                        v-if="isModelSelected(model)"
                        class="w-3 h-3 text-primary-foreground"
                      />
                    </div>
                    <span class="text-sm font-mono truncate flex-1">{{ model }}</span>
                    <button
                      v-if="isModelSelected(model)"
                      type="button"
                      class="p-1 rounded hover:bg-muted-foreground/10 transition-colors shrink-0 text-muted-foreground"
                      :title="isLocked(model) ? t('ui.lockedClickToUnlock') : t('ui.lockToRetainDuringRefresh')"
                      @click="toggleLock(model, $event)"
                    >
                      <Lock
                        v-if="isLocked(model)"
                        class="w-3.5 h-3.5"
                      />
                      <LockOpen
                        v-else
                        class="w-3.5 h-3.5"
                      />
                    </button>
                  </div>
                </div>
              </div>

              <!-- 提供商模型 -->
              <template v-if="filteredProviderModels.length > 0">
                <!-- 标题 sticky top -->
                <div
                  class="flex items-center justify-between px-3 py-2 bg-muted sticky top-0 z-20 cursor-pointer hover:bg-muted/80 transition-colors"
                  @click="toggleGroupCollapse('global')"
                >
                  <div class="flex items-center gap-2">
                    <ChevronDown
                      class="w-4 h-4 transition-transform shrink-0"
                      :class="collapsedGroups.has('global') ? '-rotate-90' : ''"
                    />
                    <span class="text-xs font-medium">{{ t('ui.providerModel') }}</span>
                    <span class="text-xs text-muted-foreground">({{ filteredProviderModels.length }})</span>
                  </div>
                  <button
                    type="button"
                    class="text-xs text-primary hover:underline"
                    @click.stop="toggleAllProviderModels"
                  >
                    {{ isAllProviderModelsSelected ? t('ui.deselectAll') : t('ui.selectAll') }}
                  </button>
                </div>
                <!-- 内容 -->
                <div
                  v-show="!collapsedGroups.has('global')"
                  class="space-y-1 p-2"
                >
                  <div
                    v-for="model in filteredProviderModels"
                    :key="model.name"
                    class="flex items-center gap-2 px-2 py-1.5 rounded hover:bg-muted cursor-pointer"
                    @click="toggleModel(model.name)"
                  >
                    <div
                      class="w-4 h-4 border rounded flex items-center justify-center shrink-0"
                      :class="isModelSelected(model.name) ? 'bg-primary border-primary' : ''"
                    >
                      <Check
                        v-if="isModelSelected(model.name)"
                        class="w-3 h-3 text-primary-foreground"
                      />
                    </div>
                    <div class="flex-1 min-w-0">
                      <p class="text-sm font-medium truncate">
                        {{ model.name }}
                      </p>
                      <p
                        v-if="model.global_model_display_name || model.global_model_name"
                        class="text-xs text-muted-foreground truncate font-mono"
                      >
                        {{ model.global_model_display_name || model.global_model_name }}
                      </p>
                    </div>
                    <button
                      v-if="isModelSelected(model.name)"
                      type="button"
                      class="p-1 rounded hover:bg-muted-foreground/10 transition-colors shrink-0 text-muted-foreground"
                      :title="isLocked(model.name) ? t('ui.lockedClickToUnlock') : t('ui.lockToRetainDuringRefresh')"
                      @click="toggleLock(model.name, $event)"
                    >
                      <Lock
                        v-if="isLocked(model.name)"
                        class="w-3.5 h-3.5"
                      />
                      <LockOpen
                        v-else
                        class="w-3.5 h-3.5"
                      />
                    </button>
                  </div>
                </div>
              </template>

              <!-- 上游模型 -->
              <template v-if="filteredUpstreamModels.length > 0">
                <!-- 标题 sticky -->
                <div
                  class="flex items-center justify-between px-3 py-2 bg-muted sticky top-0 z-20 cursor-pointer hover:bg-muted/80 transition-colors"
                  @click="toggleGroupCollapse('upstream')"
                >
                  <div class="flex items-center gap-2">
                    <ChevronDown
                      class="w-4 h-4 transition-transform shrink-0"
                      :class="collapsedGroups.has('upstream') ? '-rotate-90' : ''"
                    />
                    <span class="text-xs font-medium">{{ t('ui.upstreamModels') }}</span>
                    <span class="text-xs text-muted-foreground">({{ upstreamModelNames.length }})</span>
                    <span
                      v-if="isAutoFetchMode"
                      class="text-xs text-muted-foreground"
                    > {{ t('ui.automaticSync') }} </span>
                  </div>
                  <button
                    v-if="!isAutoFetchMode"
                    type="button"
                    class="text-xs text-primary hover:underline"
                    @click.stop="toggleAllUpstreamModels"
                  >
                    {{ isAllUpstreamModelsSelected ? t('ui.deselectAll') : t('ui.selectAll') }}
                  </button>
                </div>
                <!-- 内容 -->
                <div
                  v-show="!collapsedGroups.has('upstream')"
                  class="space-y-1 p-2"
                >
                  <div
                    v-for="model in filteredUpstreamModels"
                    :key="model.id"
                    class="flex items-center gap-2 px-2 py-1.5 rounded"
                    :class="isAutoFetchMode ? 'opacity-80' : 'hover:bg-muted cursor-pointer'"
                    @click="handleUpstreamModelClick(model)"
                  >
                    <div
                      class="w-4 h-4 border rounded flex items-center justify-center shrink-0"
                      :class="[
                        isModelSelected(model.id) ? 'bg-primary border-primary' : '',
                        isAutoFetchMode ? 'opacity-50' : ''
                      ]"
                    >
                      <Check
                        v-if="isModelSelected(model.id)"
                        class="w-3 h-3 text-primary-foreground"
                      />
                    </div>
                    <span class="text-sm font-mono truncate flex-1">{{ model.id }}</span>
                    <!-- API 格式标签 -->
                    <div class="flex items-center gap-1 shrink-0">
                      <span
                        v-for="fmt in model.api_formats"
                        :key="fmt"
                        class="text-[10px] px-1.5 py-0.5 rounded bg-muted text-muted-foreground"
                      >
                        {{ formatApiFormatShort(fmt) }}
                      </span>
                    </div>
                    <button
                      v-if="isModelSelected(model.id)"
                      type="button"
                      class="p-1 rounded hover:bg-muted-foreground/10 transition-colors shrink-0 text-muted-foreground"
                      :title="isLocked(model.id) ? t('ui.lockedClickToUnlock') : t('ui.lockToRetainDuringRefresh')"
                      @click="toggleLock(model.id, $event)"
                    >
                      <Lock
                        v-if="isLocked(model.id)"
                        class="w-3.5 h-3.5"
                      />
                      <LockOpen
                        v-else
                        class="w-3.5 h-3.5"
                      />
                    </button>
                  </div>
                </div>
              </template>

              <!-- 空状态 -->
              <div
                v-if="showEmptyState"
                class="flex flex-col items-center justify-center py-12 text-muted-foreground"
              >
                <Shield class="w-10 h-10 mb-2 opacity-30" />
                <p class="text-sm">
                  {{ searchQuery ? t('ui.noMatchingResults') : t('ui.noModelsAvailableToSelect') }}
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
          {{ hasChanges ? t('ui.unsavedChanges') : '' }}
        </p>
        <div class="flex items-center gap-2">
          <Button
            :disabled="saving || loadingProviderModels || fetchingUpstreamModels || !hasChanges"
            @click="handleSave"
          >
            {{ saving ? t('ui.saving') : t('ui.save') }}
          </Button>
          <Button
            variant="outline"
            @click="handleCancel"
          >
            {{ t('common.cancel') }}
          </Button>
        </div>
      </div>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { useI18n } from '@/i18n'

import { ref, computed, watch, onUnmounted } from 'vue'
import {
  Shield,
  Search,
  Loader2,
  Plus,
  Check,
  ChevronDown,
  Lock,
  LockOpen,
  RefreshCw,
  Zap
} from 'lucide-vue-next'
import { Dialog, Button, Checkbox, Input } from '@/components/ui'
import { useToast } from '@/composables/useToast'
import { useDiscardConfirm } from '@/composables/useDiscardConfirm'
import { parseApiError } from '@/utils/errorParser'
import {
  getProviderModels,
  updateProviderKey,
  type EndpointAPIKey,
  type AllowedModels,
} from '@/api/endpoints'
import { useUpstreamModelsCache } from '../composables/useUpstreamModelsCache'
import { formatApiFormatShort, type UpstreamModel } from '@/api/endpoints/types'

interface AvailableModel {
  name: string
  display_name: string
  global_model_name?: string
  global_model_display_name?: string
}

const props = defineProps<{
  open: boolean
  apiKey: EndpointAPIKey | null
  providerId: string
}>()

const emit = defineEmits<{
  close: []
  saved: []
}>()

const { t } = useI18n()

const { success, error: showError, warning: showWarning } = useToast()
const { confirmDiscard } = useDiscardConfirm(() => hasChanges.value)
const { fetchModels: fetchCachedModels } = useUpstreamModelsCache()

const isOpen = computed(() => props.open)
const saving = ref(false)
const loadingProviderModels = ref(false)
const fetchingUpstreamModels = ref(false)
const upstreamModelsLoaded = ref(false)

// 用于取消异步操作的标志
let loadingCancelled = false

// 搜索
const searchQuery = ref('')

// 可用模型列表（该 Provider 已关联的模型）
const allProviderModels = ref<AvailableModel[]>([])
// 上游模型列表（从 API 查询获取）
const upstreamModels = ref<UpstreamModel[]>([])

// 已选中的模型
const selectedModels = ref<string[]>([])
const initialSelectedModels = ref<string[]>([])
const allowAllModels = ref(false)
const initialAllowAllModels = ref(false)

// 已锁定的模型
const lockedModels = ref<string[]>([])
const initialLockedModels = ref<string[]>([])

// 所有添加过的自定义模型（包括已取消勾选的，保存前不消失）
const allCustomModels = ref<string[]>([])

// 是否为自动获取模式
const isAutoFetchMode = computed(() => props.apiKey?.auto_fetch_models ?? false)

// 空状态判断
const showEmptyState = computed(() => {
  return filteredProviderModels.value.length === 0 &&
         filteredUpstreamModels.value.length === 0 &&
         customModels.value.length === 0
})

// 折叠状态
const collapsedGroups = ref<Set<string>>(new Set())

// 是否有更改
const hasChanges = computed(() => {
  if (allowAllModels.value !== initialAllowAllModels.value) return true
  // 检查选中模型是否有变化
  if (selectedModels.value.length !== initialSelectedModels.value.length) return true
  const sorted1 = [...selectedModels.value].sort()
  const sorted2 = [...initialSelectedModels.value].sort()
  if (sorted1.some((v, i) => v !== sorted2[i])) return true

  // 检查锁定模型是否有变化
  if (lockedModels.value.length !== initialLockedModels.value.length) return true
  const sortedLocked1 = [...lockedModels.value].sort()
  const sortedLocked2 = [...initialLockedModels.value].sort()
  return sortedLocked1.some((v, i) => v !== sortedLocked2[i])
})

// 所有已知模型的集合（提供商模型 + 上游模型）
const allKnownModels = computed(() => {
  const set = new Set<string>()
  allProviderModels.value.forEach(m => set.add(m.name))
  upstreamModels.value.forEach(m => set.add(m.id))
  return set
})

function isModelSelected(model: string): boolean {
  return allowAllModels.value || selectedModels.value.includes(model)
}

function toggleAllModels(checked: boolean) {
  allowAllModels.value = checked
  selectedModels.value = []
}

function materializeModelSelection() {
  if (!allowAllModels.value) return
  selectedModels.value = [...new Set([...allKnownModels.value, ...allCustomModels.value])]
  allowAllModels.value = false
}

// 提供商模型名称集合（用于判断模型是否为“提供商模型”分组项）
const providerModelNamesSet = computed(() => {
  return new Set(allProviderModels.value.map(m => m.name))
})

// 判断模型是否为“提供商模型”分组项
function isProviderModel(modelId: string): boolean {
  return providerModelNamesSet.value.has(modelId)
}

// 上游模型列表（后端已按 id 聚合，包含 api_formats 数组）
// 这里只做排序
const upstreamModelList = computed(() => {
  return [...upstreamModels.value].sort((a, b) => a.id.localeCompare(b.id))
})

// 上游模型名称列表（用于计数和全选判断）
const upstreamModelNames = computed(() => upstreamModelList.value.map(m => m.id))

// 过滤后的上游模型
const filteredUpstreamModels = computed(() => {
  if (!searchQuery.value.trim()) return upstreamModelList.value
  const query = searchQuery.value.toLowerCase()
  return upstreamModelList.value.filter(m => m.id.toLowerCase().includes(query))
})

// 上游模型是否全选
const isAllUpstreamModelsSelected = computed(() => {
  if (filteredUpstreamModels.value.length === 0) return false
  return filteredUpstreamModels.value.every(m => isModelSelected(m.id))
})

// 全选/取消全选上游模型
function toggleAllUpstreamModels() {
  materializeModelSelection()
  const allIds = filteredUpstreamModels.value.map(m => m.id)
  if (isAllUpstreamModelsSelected.value) {
    selectedModels.value = selectedModels.value.filter(id => !allIds.includes(id))
  } else {
    allIds.forEach(id => {
      if (!selectedModels.value.includes(id)) {
        selectedModels.value.push(id)
      }
    })
  }
}

// 处理上游模型点击（自动同步模式下禁用）
function handleUpstreamModelClick(model: UpstreamModel) {
  if (!isAutoFetchMode.value) {
    toggleModel(model.id)
  }
}

// 自定义模型列表（显示所有添加过的，不因取消勾选而消失）
const customModels = computed(() => {
  return allCustomModels.value
})

// 排序后的自定义模型（搜索命中的排前面）
const sortedCustomModels = computed(() => {
  const search = searchQuery.value.toLowerCase().trim()
  if (!search) return customModels.value

  const matched: string[] = []
  const unmatched: string[] = []
  for (const m of customModels.value) {
    if (m.toLowerCase().includes(search)) {
      matched.push(m)
    } else {
      unmatched.push(m)
    }
  }
  return [...matched, ...unmatched]
})

// 判断搜索内容是否可以作为自定义模型添加
const canAddAsCustom = computed(() => {
  const search = searchQuery.value.trim()
  if (!search) return false
  // 已经选中了就不显示
  if (selectedModels.value.includes(search)) return false
  // 已经在自定义模型列表中就不显示
  if (allCustomModels.value.includes(search)) return false
  // 精确匹配提供商模型就不显示
  if (allProviderModels.value.some(m => m.name === search)) return false
  // 精确匹配上游模型就不显示
  if (upstreamModelNames.value.includes(search)) return false
  return true
})

// 搜索过滤后的提供商模型
const filteredProviderModels = computed(() => {
  if (!searchQuery.value.trim()) return allProviderModels.value
  const query = searchQuery.value.toLowerCase()
  return allProviderModels.value.filter(m =>
    m.name.toLowerCase().includes(query) ||
    m.display_name.toLowerCase().includes(query) ||
    (m.global_model_name || '').toLowerCase().includes(query) ||
    (m.global_model_display_name || '').toLowerCase().includes(query)
  )
})

// 提供商模型是否全选
const isAllProviderModelsSelected = computed(() => {
  if (filteredProviderModels.value.length === 0) return false
  return filteredProviderModels.value.every(m => isModelSelected(m.name))
})

// 切换模型选中状态
function toggleModel(modelId: string) {
  materializeModelSelection()
  const idx = selectedModels.value.indexOf(modelId)
  if (idx === -1) {
    selectedModels.value.push(modelId)
    // 自动获取模式下，勾选提供商模型时自动锁定
    // 防止下次刷新时被覆盖（即使全局模型与上游模型同名）
    if (isAutoFetchMode.value && isProviderModel(modelId)) {
      if (!lockedModels.value.includes(modelId)) {
        lockedModels.value.push(modelId)
      }
    }
  } else {
    selectedModels.value.splice(idx, 1)
    // 取消选中时也取消锁定
    const lockIdx = lockedModels.value.indexOf(modelId)
    if (lockIdx !== -1) {
      lockedModels.value.splice(lockIdx, 1)
    }
  }
}

// 切换模型锁定状态
function toggleLock(modelId: string, event: Event) {
  event.stopPropagation()
  // 只有已选中的模型才能锁定
  if (!isModelSelected(modelId)) return

  const idx = lockedModels.value.indexOf(modelId)
  if (idx === -1) {
    lockedModels.value.push(modelId)
  } else {
    lockedModels.value.splice(idx, 1)
  }
}

// 检查模型是否被锁定
function isLocked(modelId: string): boolean {
  return lockedModels.value.includes(modelId)
}

// 添加自定义模型
function addCustomModel() {
  const model = searchQuery.value.trim()
  if (model && !selectedModels.value.includes(model)) {
    selectedModels.value.push(model)
    // 同时添加到自定义模型列表（如果不在已知模型中）
    if (!allKnownModels.value.has(model) && !allCustomModels.value.includes(model)) {
      allCustomModels.value.push(model)
    }
    searchQuery.value = ''
  }
}

// 全选/取消全选提供商模型
function toggleAllProviderModels() {
  materializeModelSelection()
  const allNames = filteredProviderModels.value.map(m => m.name)
  if (isAllProviderModelsSelected.value) {
    // 取消全选
    selectedModels.value = selectedModels.value.filter(id => !allNames.includes(id))
    // 同时取消锁定
    lockedModels.value = lockedModels.value.filter(id => !allNames.includes(id))
  } else {
    // 全选
    allNames.forEach(name => {
      if (!selectedModels.value.includes(name)) {
        selectedModels.value.push(name)
        // 自动获取模式下，勾选提供商模型时自动锁定
        if (isAutoFetchMode.value && !lockedModels.value.includes(name)) {
          lockedModels.value.push(name)
        }
      }
    })
  }
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

// 加载该 Provider 已关联的模型
async function loadProviderModels() {
  loadingProviderModels.value = true
  try {
    const response = await getProviderModels(props.providerId, { limit: 1000 })
    if (loadingCancelled) return
    allProviderModels.value = response.map(m => ({
      name: m.provider_model_name,
      display_name: m.global_model_display_name || m.global_model_name || m.provider_model_name,
      global_model_name: m.global_model_name,
      global_model_display_name: m.global_model_display_name
    }))
  } catch {
    if (loadingCancelled) return
    showError(t('ui.failedToLoadProviderModels'), t('ui.error'))
  } finally {
    loadingProviderModels.value = false
  }
}

// 从提供商获取模型（使用缓存）
// 传 apiKeyId，获取当前 Key 自己能访问的上游模型
async function fetchUpstreamModels(forceRefresh = false) {
  if (!props.providerId || !props.apiKey) return
  try {
    fetchingUpstreamModels.value = true
    const result = await fetchCachedModels(props.providerId, props.apiKey.id, forceRefresh)
    if (loadingCancelled) return
    if (result.models.length > 0) {
      upstreamModels.value = result.models
      upstreamModelsLoaded.value = true
      // 获取上游模型后，从自定义模型列表中移除已变成已知的模型
      const upstreamIds = new Set(result.models.map((m: UpstreamModel) => m.id))
      allCustomModels.value = allCustomModels.value.filter(m => !upstreamIds.has(m))
      if (result.warning) {
        showWarning(result.warning, t('ui.failedToFetchSomeFormats'))
      }
    } else if (result.error) {
      showError(result.error, t('ui.failedToFetchUpstreamModels'))
    }
  } finally {
    fetchingUpstreamModels.value = false
  }
}

// 手动刷新上游模型（强制跳过缓存）
async function refreshUpstreamModels() {
  await fetchUpstreamModels(true)
  if (upstreamModels.value.length > 0) {
    success(t('ui.upstreamModelsRefreshed'))
    // 获取成功后收缩所有分组
    collapsedGroups.value = new Set(['global', 'upstream', 'custom'])
  }
}

// 解析 allowed_models
function parseAllowedModels(allowed: AllowedModels): string[] {
  if (allowed === null || allowed === undefined) {
    return []
  }
  return [...allowed]
}

async function initializeDialogState(apiKey: EndpointAPIKey) {
  loadingCancelled = false

  const parsed = parseAllowedModels(apiKey.allowed_models ?? null)
  allowAllModels.value = apiKey.allowed_models == null
  initialAllowAllModels.value = allowAllModels.value
  selectedModels.value = [...parsed]
  initialSelectedModels.value = [...parsed]

  const locked = apiKey.locked_models ?? []
  lockedModels.value = [...locked]
  initialLockedModels.value = [...locked]

  searchQuery.value = ''
  upstreamModels.value = []
  upstreamModelsLoaded.value = false
  allCustomModels.value = []

  if (apiKey.auto_fetch_models) {
    collapsedGroups.value = new Set(['global', 'custom'])
  } else {
    collapsedGroups.value = new Set()
  }

  await loadProviderModels()

  if (apiKey.auto_fetch_models) {
    await fetchUpstreamModels()
  }

  const upstreamModelIdsSet = new Set(upstreamModels.value.map(m => m.id))
  allCustomModels.value = selectedModels.value.filter(m =>
    !providerModelNamesSet.value.has(m) && !upstreamModelIdsSet.has(m)
  )
}

// 监听对话框打开 / 当前 key 变化
watch([() => props.open, () => props.apiKey], async ([open, apiKey]) => {
  if (open && apiKey) {
    await initializeDialogState(apiKey)
  } else {
    loadingCancelled = true
  }
}, { immediate: true })

// 组件卸载时取消所有异步操作
onUnmounted(() => {
  loadingCancelled = true
})

async function handleDialogUpdate(value: boolean) {
  if (!value && !(await confirmDiscard())) return
  if (!value) emit('close')
}

async function handleCancel() {
  if (!(await confirmDiscard())) return
  emit('close')
}

async function handleSave() {
  if (!props.apiKey) return

  saving.value = true
  try {
    const newAllowed: AllowedModels = allowAllModels.value ? null : [...selectedModels.value]

    // 只保存已选中且被锁定的模型
    const newLocked = lockedModels.value.filter(m => isModelSelected(m))

    await updateProviderKey(props.apiKey.id, {
      allowed_models: newAllowed,
      locked_models: newLocked
    })
    success(t('ui.modelPermissionsUpdated'), t('ui.success'))
    emit('saved')
    emit('close')
  } catch (err: unknown) {
    showError(parseApiError(err, t('ui.failedToSave')), t('ui.error'))
  } finally {
    saving.value = false
  }
}

</script>
