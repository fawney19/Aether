<template>
  <Dialog
    :model-value="isOpen"
    :title="props.apiKey?.name ? `模型权限 - ${props.apiKey.name}` : '模型权限'"
    description="选中的模型将被允许访问，不选择则允许全部"
    :icon="Shield"
    size="2xl"
    @update:model-value="handleDialogUpdate"
  >
    <template #default>
      <div class="space-y-4">
        <!-- 字典模式警告 -->
        <div
          v-if="isDictMode"
          class="rounded-lg border border-amber-500/50 bg-amber-50 dark:bg-amber-950/30 p-3"
        >
          <p class="text-sm text-amber-700 dark:text-amber-400">
            <strong>注意：</strong>此密钥使用按 API 格式区分的模型权限配置。
            编辑后将转换为统一列表模式，原有的格式区分信息将丢失。
          </p>
        </div>

        <!-- 常驻选择面板 -->
        <div class="border rounded-lg overflow-hidden">
          <!-- 搜索 + 操作栏 -->
          <div class="flex items-center gap-2 p-2 border-b bg-muted/30">
            <div class="relative flex-1">
              <Search class="absolute left-2 top-1/2 -translate-y-1/2 w-4 h-4 text-muted-foreground" />
              <Input
                v-model="searchQuery"
                placeholder="搜索模型或添加自定义模型..."
                class="pl-8 h-8 text-sm"
              />
            </div>
            <!-- 已选数量徽章 -->
            <span
              v-if="selectedModels.length === 0"
              class="h-6 px-2 text-xs rounded flex items-center bg-muted text-muted-foreground shrink-0"
            >
              全部模型
            </span>
            <span
              v-else
              class="h-6 px-2 text-xs rounded flex items-center bg-primary/10 text-primary shrink-0"
            >
              已选 {{ selectedModels.length }} 个
            </span>
            <button
              v-if="upstreamModelsLoaded"
              type="button"
              class="p-1.5 hover:bg-muted rounded-md transition-colors shrink-0"
              title="刷新上游模型"
              :disabled="fetchingUpstreamModels"
              @click="fetchUpstreamModels()"
            >
              <RefreshCw
                class="w-4 h-4"
                :class="{ 'animate-spin': fetchingUpstreamModels }"
              />
            </button>
            <Button
              v-else-if="!fetchingUpstreamModels"
              variant="outline"
              size="sm"
              class="h-8"
              title="从提供���获取模型"
              @click="fetchUpstreamModels()"
            >
              <Zap class="w-4 h-4" />
            </Button>
            <Loader2
              v-else
              class="w-4 h-4 animate-spin text-muted-foreground shrink-0"
            />
          </div>

          <!-- 分组列表 -->
          <div class="max-h-96 overflow-y-auto">
            <!-- 加载中 -->
            <div
              v-if="loadingGlobalModels"
              class="flex items-center justify-center py-12"
            >
              <Loader2 class="w-6 h-6 animate-spin text-primary" />
            </div>

            <template v-else>
              <!-- 添加自定义模型（搜索内容不在列表中时显示，固定在顶部） -->
              <div
                v-if="searchQuery && canAddAsCustom"
                class="px-3 py-2 border-b bg-background sticky top-0 z-10"
              >
                <div
                  class="flex items-center justify-between px-3 py-2 rounded-lg border border-dashed hover:border-primary hover:bg-primary/5 cursor-pointer transition-colors"
                  @click="addCustomModel"
                >
                  <div class="flex items-center gap-2">
                    <Plus class="w-4 h-4 text-muted-foreground" />
                    <span class="text-sm font-mono">{{ searchQuery }}</span>
                  </div>
                  <span class="text-xs text-muted-foreground">添加自定义模型</span>
                </div>
              </div>

              <!-- 自定义模型（手动添加的，始终显示全部，搜索命中的排前面） -->
              <div v-if="customModels.length > 0">
                <div
                  class="flex items-center justify-between px-3 py-2 bg-muted sticky top-0 z-10 cursor-pointer hover:bg-muted/80 transition-colors"
                  @click="toggleGroupCollapse('custom')"
                >
                  <div class="flex items-center gap-2">
                    <ChevronDown
                      class="w-4 h-4 transition-transform shrink-0"
                      :class="collapsedGroups.has('custom') ? '-rotate-90' : ''"
                    />
                    <span class="text-xs font-medium">自定义模型</span>
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
                      :class="selectedModels.includes(model) ? 'bg-primary border-primary' : ''"
                    >
                      <Check v-if="selectedModels.includes(model)" class="w-3 h-3 text-primary-foreground" />
                    </div>
                    <span class="text-sm font-mono truncate">{{ model }}</span>
                  </div>
                </div>
              </div>

              <!-- 全局模型 -->
              <div v-if="filteredGlobalModels.length > 0">
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
                    type="button"
                    class="text-xs text-primary hover:underline"
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
                    v-for="model in filteredGlobalModels"
                    :key="model.name"
                    class="flex items-center gap-2 px-2 py-1.5 rounded hover:bg-muted cursor-pointer"
                    @click="toggleModel(model.name)"
                  >
                    <div
                      class="w-4 h-4 border rounded flex items-center justify-center shrink-0"
                      :class="selectedModels.includes(model.name) ? 'bg-primary border-primary' : ''"
                    >
                      <Check v-if="selectedModels.includes(model.name)" class="w-3 h-3 text-primary-foreground" />
                    </div>
                    <div class="flex-1 min-w-0">
                      <p class="text-sm font-medium truncate">{{ model.display_name }}</p>
                      <p class="text-xs text-muted-foreground truncate font-mono">{{ model.name }}</p>
                    </div>
                  </div>
                </div>
              </div>

              <!-- 上游模型组 -->
              <div v-for="group in filteredUpstreamGroups" :key="group.api_format">
                <div
                  class="flex items-center justify-between px-3 py-2 bg-muted sticky top-0 z-10 cursor-pointer hover:bg-muted/80 transition-colors"
                  @click="toggleGroupCollapse(group.api_format)"
                >
                  <div class="flex items-center gap-2">
                    <ChevronDown
                      class="w-4 h-4 transition-transform shrink-0"
                      :class="collapsedGroups.has(group.api_format) ? '-rotate-90' : ''"
                    />
                    <span class="text-xs font-medium">
                      {{ API_FORMAT_LABELS[group.api_format] || group.api_format }}
                    </span>
                    <span class="text-xs text-muted-foreground">({{ group.models.length }})</span>
                  </div>
                  <button
                    type="button"
                    class="text-xs text-primary hover:underline"
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
                    @click="toggleModel(model.id)"
                  >
                    <div
                      class="w-4 h-4 border rounded flex items-center justify-center shrink-0"
                      :class="selectedModels.includes(model.id) ? 'bg-primary border-primary' : ''"
                    >
                      <Check v-if="selectedModels.includes(model.id)" class="w-3 h-3 text-primary-foreground" />
                    </div>
                    <span class="text-sm font-mono truncate">{{ model.id }}</span>
                  </div>
                </div>
              </div>

              <!-- 空状态 -->
              <div
                v-if="filteredGlobalModels.length === 0 && filteredUpstreamGroups.length === 0 && customModels.length === 0"
                class="flex flex-col items-center justify-center py-12 text-muted-foreground"
              >
                <Shield class="w-10 h-10 mb-2 opacity-30" />
                <p class="text-sm">{{ searchQuery ? '无匹配结果' : '暂无可选模型' }}</p>
                <p v-if="!upstreamModelsLoaded" class="text-xs mt-1">点击闪电按钮从上游获取模型</p>
              </div>
            </template>
          </div>
        </div>
      </div>
    </template>

    <template #footer>
      <div class="flex items-center justify-between w-full">
        <p class="text-xs text-muted-foreground">
          {{ hasChanges ? '有未保存的更改' : '' }}
        </p>
        <div class="flex items-center gap-2">
          <Button :disabled="saving || !hasChanges" @click="handleSave">
            {{ saving ? '保存中...' : '保存' }}
          </Button>
          <Button variant="outline" @click="handleCancel">取消</Button>
        </div>
      </div>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { ref, computed, watch, onUnmounted } from 'vue'
import {
  Shield,
  Search,
  RefreshCw,
  Loader2,
  Zap,
  Plus,
  Check,
  ChevronDown
} from 'lucide-vue-next'
import { Dialog, Button, Input } from '@/components/ui'
import { useToast } from '@/composables/useToast'
import { useConfirm } from '@/composables/useConfirm'
import { parseApiError, parseUpstreamModelError } from '@/utils/errorParser'
import {
  updateProviderKey,
  API_FORMAT_LABELS,
  type EndpointAPIKey,
  type AllowedModels,
} from '@/api/endpoints'
import { getGlobalModels, type GlobalModelResponse } from '@/api/global-models'
import { adminApi } from '@/api/admin'
import type { UpstreamModel } from '@/api/endpoints/types'

interface AvailableModel {
  name: string
  display_name: string
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

const { success, error: showError } = useToast()
const { confirmWarning } = useConfirm()

const isOpen = computed(() => props.open)
const saving = ref(false)
const loadingGlobalModels = ref(false)
const fetchingUpstreamModels = ref(false)
const upstreamModelsLoaded = ref(false)

// 用于取消异步操作的标志
let loadingCancelled = false

// 搜索
const searchQuery = ref('')

// 可用模型列表（全局模型）
const allGlobalModels = ref<AvailableModel[]>([])
// 上游模型列表
const upstreamModels = ref<UpstreamModel[]>([])

// 已选中的模型
const selectedModels = ref<string[]>([])
const initialSelectedModels = ref<string[]>([])

// 所有添加过的自定义模型（包括已取消勾选的，保存前不消失）
const allCustomModels = ref<string[]>([])

// 是否为字典模式（按 API 格式区分）
const isDictMode = ref(false)

// 折叠状态
const collapsedGroups = ref<Set<string>>(new Set())

// 是否有更改
const hasChanges = computed(() => {
  if (selectedModels.value.length !== initialSelectedModels.value.length) return true
  const sorted1 = [...selectedModels.value].sort()
  const sorted2 = [...initialSelectedModels.value].sort()
  return sorted1.some((v, i) => v !== sorted2[i])
})

// 所有已知模型的集合（全局 + 上游）
const allKnownModels = computed(() => {
  const set = new Set<string>()
  allGlobalModels.value.forEach(m => set.add(m.name))
  upstreamModels.value.forEach(m => set.add(m.id))
  return set
})

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
  // 精确匹配全局模型就不显示
  if (allGlobalModels.value.some(m => m.name === search)) return false
  // 精确匹配上游模型就不显示
  if (upstreamModels.value.some(m => m.id === search)) return false
  return true
})

// 搜索过滤后的全局模型
const filteredGlobalModels = computed(() => {
  if (!searchQuery.value.trim()) return allGlobalModels.value
  const query = searchQuery.value.toLowerCase()
  return allGlobalModels.value.filter(m =>
    m.name.toLowerCase().includes(query) ||
    m.display_name.toLowerCase().includes(query)
  )
})

// 按 API 格式分组的上游模型（过滤后）
const filteredUpstreamGroups = computed(() => {
  if (!upstreamModelsLoaded.value) return []

  const query = searchQuery.value.toLowerCase().trim()
  const groups: Record<string, UpstreamModel[]> = {}

  for (const model of upstreamModels.value) {
    // 搜索过滤
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

// 全局模型是否全选
const isAllGlobalModelsSelected = computed(() => {
  if (filteredGlobalModels.value.length === 0) return false
  return filteredGlobalModels.value.every(m => selectedModels.value.includes(m.name))
})

// 检查某个上游组是否全选
function isUpstreamGroupAllSelected(apiFormat: string): boolean {
  const group = filteredUpstreamGroups.value.find(g => g.api_format === apiFormat)
  if (!group || group.models.length === 0) return false
  return group.models.every(m => selectedModels.value.includes(m.id))
}

// 切换模型选中状态
function toggleModel(modelId: string) {
  const idx = selectedModels.value.indexOf(modelId)
  if (idx === -1) {
    selectedModels.value.push(modelId)
  } else {
    selectedModels.value.splice(idx, 1)
  }
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

// 全选/取消全选全局模型
function toggleAllGlobalModels() {
  const allNames = filteredGlobalModels.value.map(m => m.name)
  if (isAllGlobalModelsSelected.value) {
    // 取消全选
    selectedModels.value = selectedModels.value.filter(id => !allNames.includes(id))
  } else {
    // 全选
    allNames.forEach(name => {
      if (!selectedModels.value.includes(name)) {
        selectedModels.value.push(name)
      }
    })
  }
}

// 全选/取消全选某个上游组
function toggleAllUpstreamGroup(apiFormat: string) {
  const group = filteredUpstreamGroups.value.find(g => g.api_format === apiFormat)
  if (!group) return

  const allIds = group.models.map(m => m.id)
  if (isUpstreamGroupAllSelected(apiFormat)) {
    // 取消全选
    selectedModels.value = selectedModels.value.filter(id => !allIds.includes(id))
  } else {
    // 全选
    allIds.forEach(id => {
      if (!selectedModels.value.includes(id)) {
        selectedModels.value.push(id)
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

// 加载全局模型
async function loadGlobalModels() {
  loadingGlobalModels.value = true
  try {
    const response = await getGlobalModels({ limit: 1000 })
    if (loadingCancelled) return
    allGlobalModels.value = response.models.map((m: GlobalModelResponse) => ({
      name: m.name,
      display_name: m.display_name
    }))
  } catch (err) {
    if (loadingCancelled) return
    showError('加载全局模型失败', '错误')
  } finally {
    loadingGlobalModels.value = false
  }
}

// 从提供商获取模型
async function fetchUpstreamModels() {
  if (!props.providerId || !props.apiKey) return
  try {
    fetchingUpstreamModels.value = true
    const response = await adminApi.queryProviderModels(props.providerId, props.apiKey.id)
    if (loadingCancelled) return
    if (response.success && response.data?.models) {
      upstreamModels.value = response.data.models
      upstreamModelsLoaded.value = true
      // 获取上游模型后，从自定义模型列表中移除已变成已知的模型
      const upstreamIds = new Set(response.data.models.map((m: UpstreamModel) => m.id))
      allCustomModels.value = allCustomModels.value.filter(m => !upstreamIds.has(m))
    } else {
      const errorMsg = response.data?.error
        ? parseUpstreamModelError(response.data.error)
        : '获取上游模型失败'
      showError(errorMsg, '获取上游模型失败')
    }
  } catch (err: any) {
    if (loadingCancelled) return
    const rawError = err.response?.data?.detail || err.message || '获取上游模型失败'
    showError(parseUpstreamModelError(rawError), '获取上游模型失败')
  } finally {
    fetchingUpstreamModels.value = false
  }
}

// 解析 allowed_models
function parseAllowedModels(allowed: AllowedModels): string[] {
  if (allowed === null || allowed === undefined) {
    isDictMode.value = false
    return []
  }
  if (Array.isArray(allowed)) {
    isDictMode.value = false
    return [...allowed]
  }
  // 字典模式：合并所有格式的模型，并设置警告标志
  isDictMode.value = true
  const all = new Set<string>()
  for (const models of Object.values(allowed)) {
    models.forEach(m => all.add(m))
  }
  return Array.from(all)
}

// 监听对话框打开
watch(() => props.open, async (open) => {
  if (open && props.apiKey) {
    loadingCancelled = false

    const parsed = parseAllowedModels(props.apiKey.allowed_models ?? null)
    selectedModels.value = [...parsed]
    initialSelectedModels.value = [...parsed]
    searchQuery.value = ''
    upstreamModels.value = []
    upstreamModelsLoaded.value = false
    allCustomModels.value = []

    await loadGlobalModels()

    // 加载全局模型后，从已选中的模型中提取自定义模型（不在全局模型中的）
    const globalModelNames = new Set(allGlobalModels.value.map(m => m.name))
    allCustomModels.value = selectedModels.value.filter(m => !globalModelNames.has(m))
  } else {
    loadingCancelled = true
  }
})

// 组件卸载时取消所有异步操作
onUnmounted(() => {
  loadingCancelled = true
})

async function handleDialogUpdate(value: boolean) {
  if (!value && hasChanges.value) {
    const confirmed = await confirmWarning('有未保存的更改，确定要关闭吗？', '放弃更改')
    if (!confirmed) return
  }
  if (!value) emit('close')
}

async function handleCancel() {
  if (hasChanges.value) {
    const confirmed = await confirmWarning('有未保存的更改，确定要关闭吗？', '放弃更改')
    if (!confirmed) return
  }
  emit('close')
}

async function handleSave() {
  if (!props.apiKey) return

  saving.value = true
  try {
    const newAllowed: AllowedModels = selectedModels.value.length > 0
      ? [...selectedModels.value]
      : null

    await updateProviderKey(props.apiKey.id, { allowed_models: newAllowed })
    success('模型权限已更新', '成功')
    emit('saved')
    emit('close')
  } catch (err: any) {
    showError(parseApiError(err, '保存失败'), '错误')
  } finally {
    saving.value = false
  }
}
</script>
