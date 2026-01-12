<template>
  <Dialog
    :model-value="internalOpen"
    title="端点管理"
    :description="`管理 ${provider?.name} 的 API 端点`"
    :icon="Settings"
    size="2xl"
    @update:model-value="handleDialogUpdate"
  >
    <div class="space-y-4">
      <!-- 已有端点列表 -->
      <div
        v-if="localEndpoints.length > 0"
        class="space-y-2"
      >
        <Label class="text-muted-foreground">已配置的端点</Label>
        <div class="space-y-2">
          <div
            v-for="endpoint in localEndpoints"
            :key="endpoint.id"
            class="rounded-md border px-3 py-2"
            :class="{ 'opacity-50': !endpoint.is_active }"
          >
            <!-- 编辑模式 -->
            <template v-if="editingEndpointId === endpoint.id">
              <div class="space-y-2">
                <div class="flex items-center gap-2">
                  <span class="text-sm font-medium w-24 shrink-0">{{ API_FORMAT_LABELS[endpoint.api_format] || endpoint.api_format }}</span>
                  <div class="flex items-center gap-1 ml-auto">
                    <Button
                      variant="ghost"
                      size="icon"
                      class="h-7 w-7"
                      title="保存"
                      :disabled="savingEndpointId === endpoint.id"
                      @click="saveEndpointUrl(endpoint)"
                    >
                      <Check class="w-3.5 h-3.5" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon"
                      class="h-7 w-7"
                      title="取消"
                      @click="cancelEdit"
                    >
                      <X class="w-3.5 h-3.5" />
                    </Button>
                  </div>
                </div>
                <div class="grid grid-cols-2 gap-2">
                  <div class="space-y-1">
                    <Label class="text-xs text-muted-foreground">Base URL</Label>
                    <Input
                      v-model="editingUrl"
                      class="h-8 text-sm"
                      placeholder="https://api.example.com"
                      @keyup.escape="cancelEdit"
                    />
                  </div>
                  <div class="space-y-1">
                    <Label class="text-xs text-muted-foreground">自定义路径 (可选)</Label>
                    <Input
                      v-model="editingPath"
                      class="h-8 text-sm"
                      :placeholder="editingDefaultPath || '留空使用默认路径'"
                      @keyup.escape="cancelEdit"
                    />
                  </div>
                </div>
              </div>
            </template>
            <!-- 查看模式 -->
            <template v-else>
              <div class="flex items-center gap-3">
                <div class="w-24 shrink-0">
                  <span class="text-sm font-medium">{{ API_FORMAT_LABELS[endpoint.api_format] || endpoint.api_format }}</span>
                </div>
                <div class="flex-1 min-w-0">
                  <span class="text-sm text-muted-foreground truncate block">
                    {{ endpoint.base_url }}{{ endpoint.custom_path ? endpoint.custom_path : '' }}
                  </span>
                </div>
                <div class="flex items-center gap-1 shrink-0">
                  <Button
                    variant="ghost"
                    size="icon"
                    class="h-7 w-7"
                    title="编辑"
                    @click="startEdit(endpoint)"
                  >
                    <Edit class="w-3.5 h-3.5" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    class="h-7 w-7"
                    :title="endpoint.is_active ? '停用' : '启用'"
                    :disabled="togglingEndpointId === endpoint.id"
                    @click="handleToggleEndpoint(endpoint)"
                  >
                    <Power class="w-3.5 h-3.5" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    class="h-7 w-7 text-destructive hover:text-destructive"
                    title="删除"
                    :disabled="deletingEndpointId === endpoint.id"
                    @click="handleDeleteEndpoint(endpoint)"
                  >
                    <Trash2 class="w-3.5 h-3.5" />
                  </Button>
                </div>
              </div>
            </template>
          </div>
        </div>
      </div>

      <!-- 添加新端点 -->
      <div
        v-if="availableFormats.length > 0"
        class="space-y-3 pt-3 border-t"
      >
        <Label class="text-muted-foreground">添加新端点</Label>
        <div class="flex items-end gap-3">
          <div class="w-32 shrink-0 space-y-1.5">
            <Label class="text-xs">API 格式</Label>
            <Select
              v-model="newEndpoint.api_format"
              v-model:open="formatSelectOpen"
            >
              <SelectTrigger class="h-9">
                <SelectValue placeholder="选择格式" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem
                  v-for="format in availableFormats"
                  :key="format.value"
                  :value="format.value"
                >
                  {{ format.label }}
                </SelectItem>
              </SelectContent>
            </Select>
          </div>
          <div class="flex-1 space-y-1.5">
            <Label class="text-xs">Base URL</Label>
            <Input
              v-model="newEndpoint.base_url"
              placeholder="https://api.example.com"
              class="h-9"
            />
          </div>
          <div class="w-40 shrink-0 space-y-1.5">
            <Label class="text-xs">自定义路径</Label>
            <Input
              v-model="newEndpoint.custom_path"
              :placeholder="newEndpointDefaultPath || '可选'"
              class="h-9"
            />
          </div>
          <Button
            variant="outline"
            size="sm"
            class="h-9 shrink-0"
            :disabled="!newEndpoint.api_format || !newEndpoint.base_url || addingEndpoint"
            @click="handleAddEndpoint"
          >
            {{ addingEndpoint ? '添加中...' : '添加' }}
          </Button>
        </div>
      </div>

      <!-- 空状态 -->
      <div
        v-if="localEndpoints.length === 0 && availableFormats.length === 0"
        class="text-center py-8 text-muted-foreground"
      >
        <p>所有 API 格式都已配置</p>
      </div>
    </div>

    <template #footer>
      <Button
        variant="outline"
        @click="handleClose"
      >
        关闭
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { ref, computed, onMounted, watch } from 'vue'
import {
  Dialog,
  Button,
  Input,
  Label,
  Select,
  SelectTrigger,
  SelectValue,
  SelectContent,
  SelectItem,
} from '@/components/ui'
import { Settings, Edit, Trash2, Check, X, Power } from 'lucide-vue-next'
import { useToast } from '@/composables/useToast'
import { log } from '@/utils/logger'
import {
  createEndpoint,
  updateEndpoint,
  deleteEndpoint,
  API_FORMAT_LABELS,
  type ProviderEndpoint,
  type ProviderWithEndpointsSummary
} from '@/api/endpoints'
import { adminApi } from '@/api/admin'

const props = defineProps<{
  modelValue: boolean
  provider: ProviderWithEndpointsSummary | null
  endpoints?: ProviderEndpoint[]
}>()

const emit = defineEmits<{
  'update:modelValue': [value: boolean]
  'endpointCreated': []
  'endpointUpdated': []
}>()

const { success, error: showError } = useToast()

// 状态
const addingEndpoint = ref(false)
const editingEndpointId = ref<string | null>(null)
const editingUrl = ref('')
const editingPath = ref('')
const savingEndpointId = ref<string | null>(null)
const deletingEndpointId = ref<string | null>(null)
const togglingEndpointId = ref<string | null>(null)
const formatSelectOpen = ref(false)

// 内部状态
const internalOpen = computed(() => props.modelValue)

// 新端点表单
const newEndpoint = ref({
  api_format: '',
  base_url: '',
  custom_path: '',
})

// API 格式列表
const apiFormats = ref<Array<{ value: string; label: string; default_path: string }>>([])

// 本地端点列表
const localEndpoints = ref<ProviderEndpoint[]>([])

// 可用的格式（未添加的）
const availableFormats = computed(() => {
  const existingFormats = localEndpoints.value.map(e => e.api_format)
  return apiFormats.value.filter(f => !existingFormats.includes(f.value))
})

// 获取指定 API 格式的默认路径
function getDefaultPath(apiFormat: string): string {
  const format = apiFormats.value.find(f => f.value === apiFormat)
  return format?.default_path || ''
}

// 当前编辑端点的默认路径
const editingDefaultPath = computed(() => {
  const endpoint = localEndpoints.value.find(e => e.id === editingEndpointId.value)
  return endpoint ? getDefaultPath(endpoint.api_format) : ''
})

// 新端点选择的格式的默认路径
const newEndpointDefaultPath = computed(() => {
  return getDefaultPath(newEndpoint.value.api_format)
})

// 加载 API 格式列表
const loadApiFormats = async () => {
  try {
    const response = await adminApi.getApiFormats()
    apiFormats.value = response.formats
  } catch (error) {
    log.error('加载API格式失败:', error)
  }
}

onMounted(() => {
  loadApiFormats()
})

// 监听 props 变化
watch(() => props.modelValue, (open) => {
  if (open) {
    localEndpoints.value = [...(props.endpoints || [])]
    // 重置编辑状态
    editingEndpointId.value = null
    editingUrl.value = ''
    editingPath.value = ''
  } else {
    // 关闭对话框时完全清空新端点表单
    newEndpoint.value = { api_format: '', base_url: '', custom_path: '' }
  }
}, { immediate: true })

watch(() => props.endpoints, (endpoints) => {
  if (props.modelValue) {
    localEndpoints.value = [...(endpoints || [])]
  }
}, { deep: true })

// 开始编辑
function startEdit(endpoint: ProviderEndpoint) {
  editingEndpointId.value = endpoint.id
  editingUrl.value = endpoint.base_url
  editingPath.value = endpoint.custom_path || ''
}

// 取消编辑
function cancelEdit() {
  editingEndpointId.value = null
  editingUrl.value = ''
  editingPath.value = ''
}

// 保存端点
async function saveEndpointUrl(endpoint: ProviderEndpoint) {
  if (!editingUrl.value) return

  savingEndpointId.value = endpoint.id
  try {
    await updateEndpoint(endpoint.id, {
      base_url: editingUrl.value,
      custom_path: editingPath.value || null,  // 空字符串时传 null 清空
    })
    success('端点已更新')
    emit('endpointUpdated')
    cancelEdit()
  } catch (error: any) {
    showError(error.response?.data?.detail || '更新失败', '错误')
  } finally {
    savingEndpointId.value = null
  }
}

// 添加端点
async function handleAddEndpoint() {
  if (!props.provider || !newEndpoint.value.api_format || !newEndpoint.value.base_url) return

  addingEndpoint.value = true
  try {
    await createEndpoint(props.provider.id, {
      provider_id: props.provider.id,
      api_format: newEndpoint.value.api_format,
      base_url: newEndpoint.value.base_url,
      custom_path: newEndpoint.value.custom_path || undefined,
      is_active: true,
    })
    success(`已添加 ${API_FORMAT_LABELS[newEndpoint.value.api_format] || newEndpoint.value.api_format} 端点`)
    // 重置表单，保留 URL
    const url = newEndpoint.value.base_url
    newEndpoint.value = { api_format: '', base_url: url, custom_path: '' }
    emit('endpointCreated')
  } catch (error: any) {
    showError(error.response?.data?.detail || '添加失败', '错误')
  } finally {
    addingEndpoint.value = false
  }
}

// 切换端点启用状态
async function handleToggleEndpoint(endpoint: ProviderEndpoint) {
  togglingEndpointId.value = endpoint.id
  try {
    const newStatus = !endpoint.is_active
    await updateEndpoint(endpoint.id, { is_active: newStatus })
    success(newStatus ? '端点已启用' : '端点已停用')
    emit('endpointUpdated')
  } catch (error: any) {
    showError(error.response?.data?.detail || '操作失败', '错误')
  } finally {
    togglingEndpointId.value = null
  }
}

// 删除端点
async function handleDeleteEndpoint(endpoint: ProviderEndpoint) {
  deletingEndpointId.value = endpoint.id
  try {
    await deleteEndpoint(endpoint.id)
    success(`已删除 ${API_FORMAT_LABELS[endpoint.api_format] || endpoint.api_format} 端点`)
    emit('endpointUpdated')
  } catch (error: any) {
    showError(error.response?.data?.detail || '删除失败', '错误')
  } finally {
    deletingEndpointId.value = null
  }
}

// 关闭对话框
function handleDialogUpdate(value: boolean) {
  emit('update:modelValue', value)
}

function handleClose() {
  emit('update:modelValue', false)
}
</script>
