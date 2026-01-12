<template>
  <Dialog
    :model-value="isOpen"
    title="获取上游模型"
    :description="`使用密钥 ${props.apiKey?.name || props.apiKey?.api_key_masked || ''} 从上游获取模型列表。导入的模型需要关联全局模型后才能参与路由。`"
    :icon="Layers"
    size="2xl"
    @update:model-value="handleDialogUpdate"
  >
    <div class="space-y-4 py-2">
      <!-- 操作区域 -->
      <div class="flex items-center justify-between">
        <div class="text-sm text-muted-foreground">
          <span v-if="!hasQueried">点击获取按钮查询上游可用模型</span>
          <span v-else-if="upstreamModels.length > 0">
            共 {{ upstreamModels.length }} 个模型，已选 {{ selectedModels.length }} 个
          </span>
          <span v-else>未找到可用模型</span>
        </div>
        <Button
          variant="outline"
          size="sm"
          :disabled="loading"
          @click="fetchUpstreamModels"
        >
          <RefreshCw
            class="w-3.5 h-3.5 mr-1.5"
            :class="{ 'animate-spin': loading }"
          />
          {{ hasQueried ? '刷新' : '获取模型' }}
        </Button>
      </div>

      <!-- 加载状态 -->
      <div
        v-if="loading"
        class="flex flex-col items-center justify-center py-12 space-y-3"
      >
        <div class="animate-spin rounded-full h-8 w-8 border-2 border-primary/20 border-t-primary" />
        <span class="text-xs text-muted-foreground">正在从上游获取模型列表...</span>
      </div>

      <!-- 错误状态 -->
      <div
        v-else-if="errorMessage"
        class="flex flex-col items-center justify-center py-12 text-destructive border border-dashed border-destructive/30 rounded-lg bg-destructive/5"
      >
        <AlertCircle class="w-10 h-10 mb-2 opacity-50" />
        <span class="text-sm text-center px-4">{{ errorMessage }}</span>
        <Button
          variant="outline"
          size="sm"
          class="mt-3"
          @click="fetchUpstreamModels"
        >
          重试
        </Button>
      </div>

      <!-- 未查询状态 -->
      <div
        v-else-if="!hasQueried"
        class="flex flex-col items-center justify-center py-12 text-muted-foreground border border-dashed rounded-lg bg-muted/10"
      >
        <Layers class="w-10 h-10 mb-2 opacity-20" />
        <span class="text-sm">点击上方按钮获取模型列表</span>
      </div>

      <!-- 无模型 -->
      <div
        v-else-if="upstreamModels.length === 0"
        class="flex flex-col items-center justify-center py-12 text-muted-foreground border border-dashed rounded-lg bg-muted/10"
      >
        <Box class="w-10 h-10 mb-2 opacity-20" />
        <span class="text-sm">上游 API 未返回可用模型</span>
      </div>

      <!-- 模型列表 -->
      <div
        v-else
        class="space-y-2"
      >
        <!-- 全选/取消 -->
        <div class="flex items-center justify-between px-1">
          <div class="flex items-center gap-2">
            <Checkbox
              :checked="isAllSelected"
              :indeterminate="isPartiallySelected"
              @update:checked="toggleSelectAll"
            />
            <span class="text-xs text-muted-foreground">
              {{ isAllSelected ? '取消全选' : '全选' }}
            </span>
          </div>
          <div class="text-xs text-muted-foreground">
            {{ newModelsCount }} 个新模型（不在本地）
          </div>
        </div>

        <div class="max-h-[320px] overflow-y-auto pr-1 space-y-1 custom-scrollbar">
          <div
            v-for="model in upstreamModels"
            :key="`${model.id}:${model.api_format || ''}`"
            class="group flex items-center gap-3 px-3 py-2.5 rounded-lg border transition-all duration-200 cursor-pointer select-none"
            :class="[
              selectedModels.includes(model.id)
                ? 'border-primary/40 bg-primary/5 shadow-sm'
                : 'border-border/40 bg-background hover:border-primary/20 hover:bg-muted/30'
            ]"
            @click="toggleModel(model.id)"
          >
            <Checkbox
              :checked="selectedModels.includes(model.id)"
              class="data-[state=checked]:bg-primary data-[state=checked]:border-primary"
              @click.stop
              @update:checked="checked => toggleModel(model.id, checked)"
            />
            <div class="flex-1 min-w-0">
              <div class="flex items-center gap-2">
                <span class="text-sm font-medium truncate text-foreground/90">
                  {{ model.display_name || model.id }}
                </span>
                <Badge
                  v-if="model.api_format"
                  variant="outline"
                  class="text-[10px] px-1.5 py-0 shrink-0"
                >
                  {{ API_FORMAT_LABELS[model.api_format] || model.api_format }}
                </Badge>
                <Badge
                  v-if="isModelExisting(model.id)"
                  variant="secondary"
                  class="text-[10px] px-1.5 py-0 shrink-0"
                >
                  已存在
                </Badge>
              </div>
              <div class="text-[11px] text-muted-foreground/60 font-mono truncate mt-0.5">
                {{ model.id }}
              </div>
            </div>
            <div
              v-if="model.owned_by"
              class="text-[10px] text-muted-foreground/50 shrink-0"
            >
              {{ model.owned_by }}
            </div>
          </div>
        </div>
      </div>
    </div>

    <template #footer>
      <div class="flex items-center justify-between w-full pt-2">
        <div class="text-xs text-muted-foreground">
          <span v-if="selectedModels.length > 0 && newSelectedCount > 0">
            将导入 {{ newSelectedCount }} 个新模型
          </span>
        </div>
        <div class="flex items-center gap-2">
          <Button
            variant="outline"
            class="h-9"
            @click="handleCancel"
          >
            取消
          </Button>
          <Button
            :disabled="importing || selectedModels.length === 0 || newSelectedCount === 0"
            class="h-9 min-w-[100px]"
            @click="handleImport"
          >
            <Loader2
              v-if="importing"
              class="w-3.5 h-3.5 mr-1.5 animate-spin"
            />
            {{ importing ? '导入中' : `导入 ${newSelectedCount} 个模型` }}
          </Button>
        </div>
      </div>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { ref, computed, watch } from 'vue'
import { Box, Layers, Loader2, RefreshCw, AlertCircle } from 'lucide-vue-next'
import { Dialog } from '@/components/ui'
import Button from '@/components/ui/button.vue'
import Badge from '@/components/ui/badge.vue'
import Checkbox from '@/components/ui/checkbox.vue'
import { useToast } from '@/composables/useToast'
import { parseUpstreamModelError } from '@/utils/errorParser'
import { adminApi } from '@/api/admin'
import {
  importModelsFromUpstream,
  getProviderModels,
  type EndpointAPIKey,
  type UpstreamModel,
  API_FORMAT_LABELS,
} from '@/api/endpoints'

const props = defineProps<{
  open: boolean
  apiKey: EndpointAPIKey | null
  providerId: string | null
}>()

const emit = defineEmits<{
  close: []
  saved: []
}>()

const { success, error: showError } = useToast()

const isOpen = computed(() => props.open)
const loading = ref(false)
const importing = ref(false)
const hasQueried = ref(false)
const errorMessage = ref('')
const upstreamModels = ref<UpstreamModel[]>([])
const selectedModels = ref<string[]>([])
const existingModelIds = ref<Set<string>>(new Set())

// 计算属性
const isAllSelected = computed(() =>
  upstreamModels.value.length > 0 &&
  selectedModels.value.length === upstreamModels.value.length
)

const isPartiallySelected = computed(() =>
  selectedModels.value.length > 0 &&
  selectedModels.value.length < upstreamModels.value.length
)

const newModelsCount = computed(() =>
  upstreamModels.value.filter(m => !existingModelIds.value.has(m.id)).length
)

const newSelectedCount = computed(() =>
  selectedModels.value.filter(id => !existingModelIds.value.has(id)).length
)

// 检查模型是否已存在
function isModelExisting(modelId: string): boolean {
  return existingModelIds.value.has(modelId)
}

// 监听对话框打开
watch(() => props.open, (open) => {
  if (open) {
    resetState()
    loadExistingModels()
  }
})

function resetState() {
  hasQueried.value = false
  errorMessage.value = ''
  upstreamModels.value = []
  selectedModels.value = []
}

// 加载已存在的模型列表
async function loadExistingModels() {
  if (!props.providerId) return
  try {
    const models = await getProviderModels(props.providerId)
    existingModelIds.value = new Set(
      models.map((m: { provider_model_name: string }) => m.provider_model_name)
    )
  } catch {
    existingModelIds.value = new Set()
  }
}

// 获取上游模型
async function fetchUpstreamModels() {
  if (!props.providerId || !props.apiKey) return

  loading.value = true
  errorMessage.value = ''

  try {
    const response = await adminApi.queryProviderModels(props.providerId, props.apiKey.id)

    if (response.success && response.data?.models) {
      upstreamModels.value = response.data.models
      // 默认选中所有新模型
      selectedModels.value = response.data.models
        .filter((m: UpstreamModel) => !existingModelIds.value.has(m.id))
        .map((m: UpstreamModel) => m.id)
      hasQueried.value = true
      // 如果有部分失败，显示警告提示
      if (response.data.error) {
        // 使用友好的错误解析
        showError(`部分格式获取失败: ${parseUpstreamModelError(response.data.error)}`, '警告')
      }
    } else {
      // 使用友好的错误解析
      const rawError = response.data?.error || '获取上游模型失败'
      errorMessage.value = parseUpstreamModelError(rawError)
    }
  } catch (err: any) {
    // 使用友好的错误解析
    const rawError = err.response?.data?.detail || err.message || '获取上游模型失败'
    errorMessage.value = parseUpstreamModelError(rawError)
  } finally {
    loading.value = false
  }
}

// 切换模型选择
function toggleModel(modelId: string, checked?: boolean) {
  const shouldSelect = checked !== undefined ? checked : !selectedModels.value.includes(modelId)
  if (shouldSelect) {
    if (!selectedModels.value.includes(modelId)) {
      selectedModels.value = [...selectedModels.value, modelId]
    }
  } else {
    selectedModels.value = selectedModels.value.filter(id => id !== modelId)
  }
}

// 全选/取消全选
function toggleSelectAll(checked: boolean) {
  if (checked) {
    selectedModels.value = upstreamModels.value.map(m => m.id)
  } else {
    selectedModels.value = []
  }
}

function handleDialogUpdate(value: boolean) {
  if (!value) {
    emit('close')
  }
}

function handleCancel() {
  emit('close')
}

// 导入选中的模型
async function handleImport() {
  if (!props.providerId || selectedModels.value.length === 0) return

  // 过滤出新模型（不在已存在列表中的）
  const modelsToImport = selectedModels.value.filter(id => !existingModelIds.value.has(id))
  if (modelsToImport.length === 0) {
    showError('所选模型都已存在', '提示')
    return
  }

  importing.value = true
  try {
    const response = await importModelsFromUpstream(props.providerId, modelsToImport)

    const successCount = response.success?.length || 0
    const errorCount = response.errors?.length || 0

    if (successCount > 0 && errorCount === 0) {
      success(`成功导入 ${successCount} 个模型`, '导入成功')
      emit('saved')
      emit('close')
    } else if (successCount > 0 && errorCount > 0) {
      success(`成功导入 ${successCount} 个模型，${errorCount} 个失败`, '部分成功')
      emit('saved')
      // 刷新列表以更新已存在状态
      await loadExistingModels()
      // 更新选中列表，移除已成功导入的
      const successIds = new Set(response.success?.map((s: { model_id: string }) => s.model_id) || [])
      selectedModels.value = selectedModels.value.filter(id => !successIds.has(id))
    } else {
      const errorMsg = response.errors?.[0]?.error || '导入失败'
      showError(errorMsg, '导入失败')
    }
  } catch (err: any) {
    showError(err.response?.data?.detail || '导入失败', '错误')
  } finally {
    importing.value = false
  }
}
</script>

<style scoped>
.custom-scrollbar::-webkit-scrollbar {
  width: 4px;
}
.custom-scrollbar::-webkit-scrollbar-track {
  background: transparent;
}
.custom-scrollbar::-webkit-scrollbar-thumb {
  background-color: hsl(var(--muted-foreground) / 0.2);
  border-radius: 4px;
}
.custom-scrollbar::-webkit-scrollbar-thumb:hover {
  background-color: hsl(var(--muted-foreground) / 0.4);
}
</style>
