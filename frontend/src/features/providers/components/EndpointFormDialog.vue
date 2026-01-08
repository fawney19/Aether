<template>
  <Dialog
    :model-value="internalOpen"
    :title="isEditMode ? '编辑 API 端点' : '添加 API 端点'"
    :description="isEditMode ? `修改 ${provider?.display_name} 的端点配置` : '为提供商添加新的 API 端点'"
    :icon="isEditMode ? SquarePen : Link"
    size="xl"
    @update:model-value="handleDialogUpdate"
  >
    <form
      class="space-y-6"
      @submit.prevent="handleSubmit()"
    >
      <!-- API 配置 -->
      <div class="space-y-4">
        <h3
          v-if="isEditMode"
          class="text-sm font-medium"
        >
          API 配置
        </h3>

        <!-- API 格式 -->
        <div class="space-y-2">
          <Label for="api_format">API 格式 *</Label>
          <template v-if="isEditMode">
            <Input
              id="api_format"
              v-model="form.api_format"
              disabled
              class="bg-muted"
            />
            <p class="text-xs text-muted-foreground">
              API 格式创建后不可修改
            </p>
          </template>
          <template v-else>
            <div class="grid grid-cols-2 gap-3">
              <label
                v-for="format in apiFormats"
                :key="format.value"
                class="flex items-center gap-3 rounded-lg border p-4 cursor-pointer transition-colors hover:bg-accent"
                :class="selectedFormats.includes(format.value) ? 'border-primary bg-accent' : 'border-border'"
              >
                <input
                  type="checkbox"
                  :value="format.value"
                  v-model="selectedFormats"
                  class="h-4 w-4 text-primary focus:ring-2 focus:ring-primary rounded"
                />
                <span class="text-sm font-medium">{{ format.label }}</span>
              </label>
            </div>
            <p class="text-xs text-muted-foreground">
              选择一个或多个 API 格式，将为每个格式创建独立的端点
            </p>
          </template>
        </div>

        <!-- API URL 和自定义路径 -->
        <div class="grid grid-cols-2 gap-4">
          <div class="space-y-2">
            <Label for="base_url">API URL *</Label>
            <Input
              id="base_url"
              v-model="form.base_url"
              placeholder="https://api.example.com"
              required
            />
          </div>

          <div class="space-y-2">
            <Label for="custom_path">自定义请求路径（可选）</Label>
            <Input
              id="custom_path"
              v-model="form.custom_path"
              :placeholder="isEditMode ? defaultPathPlaceholder : '留空使用各格式的默认路径'"
            />
            <p v-if="!isEditMode && selectedFormats.length > 0" class="text-xs text-muted-foreground">
              将为所有选中的格式使用相同的 URL 和路径配置
            </p>
          </div>
        </div>
      </div>

      <!-- 请求配置 -->
      <div class="space-y-4">
        <h3 class="text-sm font-medium">
          请求配置
        </h3>

        <div class="grid grid-cols-3 gap-4">
          <div class="space-y-2">
            <Label for="timeout">超时（秒）</Label>
            <Input
              id="timeout"
              v-model.number="form.timeout"
              type="number"
              placeholder="300"
            />
          </div>

          <div class="space-y-2">
            <Label for="max_retries">最大重试</Label>
            <Input
              id="max_retries"
              v-model.number="form.max_retries"
              type="number"
              placeholder="3"
            />
          </div>

          <div class="space-y-2">
            <Label for="max_concurrent">最大并发</Label>
            <Input
              id="max_concurrent"
              :model-value="form.max_concurrent ?? ''"
              type="number"
              placeholder="无限制"
              @update:model-value="(v) => form.max_concurrent = parseNumberInput(v)"
            />
          </div>
        </div>

        <div class="grid grid-cols-2 gap-4">
          <div class="space-y-2">
            <Label for="rate_limit">速率限制（请求/分钟）</Label>
            <Input
              id="rate_limit"
              :model-value="form.rate_limit ?? ''"
              type="number"
              placeholder="无限制"
              @update:model-value="(v) => form.rate_limit = parseNumberInput(v)"
            />
          </div>
        </div>
      </div>

      <!-- 代理配置 -->
      <div class="space-y-4">
        <div class="flex items-center justify-between">
          <h3 class="text-sm font-medium">
            代理配置
          </h3>
          <div class="flex items-center gap-2">
            <Switch v-model="proxyEnabled" />
            <span class="text-sm text-muted-foreground">启用代理</span>
          </div>
        </div>

        <div
          v-if="proxyEnabled"
          class="space-y-4 rounded-lg border p-4"
        >
          <div class="space-y-2">
            <Label for="proxy_url">代理 URL *</Label>
            <Input
              id="proxy_url"
              v-model="form.proxy_url"
              placeholder="http://host:port 或 socks5://host:port"
              required
              :class="proxyUrlError ? 'border-red-500' : ''"
            />
            <p
              v-if="proxyUrlError"
              class="text-xs text-red-500"
            >
              {{ proxyUrlError }}
            </p>
            <p
              v-else
              class="text-xs text-muted-foreground"
            >
              支持 HTTP、HTTPS、SOCKS5 代理
            </p>
          </div>

          <div class="grid grid-cols-2 gap-4">
            <div class="space-y-2">
              <Label for="proxy_user">用户名（可选）</Label>
              <Input
                :id="`proxy_user_${formId}`"
                v-model="form.proxy_username"
                :name="`proxy_user_${formId}`"
                placeholder="代理认证用户名"
                autocomplete="off"
                data-form-type="other"
                data-lpignore="true"
                data-1p-ignore="true"
              />
            </div>

            <div class="space-y-2">
              <Label :for="`proxy_pass_${formId}`">密码（可选）</Label>
              <Input
                :id="`proxy_pass_${formId}`"
                v-model="form.proxy_password"
                :name="`proxy_pass_${formId}`"
                type="text"
                :placeholder="passwordPlaceholder"
                autocomplete="off"
                data-form-type="other"
                data-lpignore="true"
                data-1p-ignore="true"
                :style="{ '-webkit-text-security': 'disc', 'text-security': 'disc' }"
              />
            </div>
          </div>
        </div>
      </div>
    </form>

    <template #footer>
      <Button
        type="button"
        variant="outline"
        :disabled="loading"
        @click="handleCancel"
      >
        取消
      </Button>
      <Button
        :disabled="loading || !form.base_url || (!isEditMode && selectedFormats.length === 0)"
        @click="handleSubmit()"
      >
        {{ loading ? (isEditMode ? '保存中...' : '创建中...') : (isEditMode ? '保存修改' : `创建 ${selectedFormats.length} 个端点`) }}
      </Button>
    </template>
  </Dialog>

  <!-- 确认清空凭据对话框 -->
  <AlertDialog
    v-model="showClearCredentialsDialog"
    title="清空代理凭据"
    description="代理 URL 为空，但用户名和密码仍有值。是否清空这些凭据并继续保存？"
    type="warning"
    confirm-text="清空并保存"
    cancel-text="返回编辑"
    @confirm="confirmClearCredentials"
    @cancel="showClearCredentialsDialog = false"
  />
</template>

<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
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
  Switch,
} from '@/components/ui'
import AlertDialog from '@/components/common/AlertDialog.vue'
import { Link, SquarePen } from 'lucide-vue-next'
import { useToast } from '@/composables/useToast'
import { useFormDialog } from '@/composables/useFormDialog'
import { parseNumberInput } from '@/utils/form'
import { log } from '@/utils/logger'
import {
  createEndpoint,
  updateEndpoint,
  type ProviderEndpoint,
  type ProviderWithEndpointsSummary
} from '@/api/endpoints'
import { adminApi } from '@/api/admin'

const props = defineProps<{
  modelValue: boolean
  provider: ProviderWithEndpointsSummary | null
  endpoint?: ProviderEndpoint | null  // 编辑模式时传入
}>()

const emit = defineEmits<{
  'update:modelValue': [value: boolean]
  'endpointCreated': []
  'endpointUpdated': []
}>()

const { success, error: showError } = useToast()
const loading = ref(false)
const selectOpen = ref(false)
const proxyEnabled = ref(false)
const showClearCredentialsDialog = ref(false)  // 确认清空凭据对话框

// 生成随机 ID 防止浏览器自动填充
const formId = Math.random().toString(36).substring(2, 10)

// 内部状态
const internalOpen = computed(() => props.modelValue)

// 表单数据
const form = ref({
  api_format: '',
  base_url: '',
  custom_path: '',
  timeout: 300,
  max_retries: 2,
  max_concurrent: undefined as number | undefined,
  rate_limit: undefined as number | undefined,
  is_active: true,
  // 代理配置
  proxy_url: '',
  proxy_username: '',
  proxy_password: '',
})

// 选中的 API 格式（多选）
const selectedFormats = ref<string[]>([])

// API 格式列表
const apiFormats = ref<Array<{ value: string; label: string; default_path: string; aliases: string[] }>>([])

// 加载API格式列表
const loadApiFormats = async () => {
  try {
    const response = await adminApi.getApiFormats()
    apiFormats.value = response.formats
  } catch (error) {
    log.error('加载API格式失败:', error)
    if (!isEditMode.value) {
      showError('加载API格式失败', '错误')
    }
  }
}

// 根据选择的 API 格式计算默认路径
const defaultPath = computed(() => {
  const format = apiFormats.value.find(f => f.value === form.value.api_format)
  return format?.default_path || '/'
})

// 动态 placeholder
const defaultPathPlaceholder = computed(() => {
  return `留空使用默认路径：${defaultPath.value}`
})

// 检查是否有已保存的密码（后端返回 *** 表示有密码）
const hasExistingPassword = computed(() => {
  if (!props.endpoint?.proxy) return false
  const proxy = props.endpoint.proxy as { password?: string }
  return proxy?.password === MASKED_PASSWORD
})

// 密码输入框的 placeholder
const passwordPlaceholder = computed(() => {
  if (hasExistingPassword.value) {
    return '已保存密码，留空保持不变'
  }
  return '代理认证密码'
})

// 代理 URL 验证
const proxyUrlError = computed(() => {
  // 只有启用代理且填写了 URL 时才验证
  if (!proxyEnabled.value || !form.value.proxy_url) {
    return ''
  }
  const url = form.value.proxy_url.trim()

  // 检查禁止的特殊字符
  if (/[\n\r]/.test(url)) {
    return '代理 URL 包含非法字符'
  }

  // 验证协议（不支持 SOCKS4）
  if (!/^(http|https|socks5):\/\//i.test(url)) {
    return '代理 URL 必须以 http://, https:// 或 socks5:// 开头'
  }
  try {
    const parsed = new URL(url)
    if (!parsed.host) {
      return '代理 URL 必须包含有效的 host'
    }
    // 禁止 URL 中内嵌认证信息
    if (parsed.username || parsed.password) {
      return '请勿在 URL 中包含用户名和密码，请使用独立的认证字段'
    }
  } catch {
    return '代理 URL 格式无效'
  }
  return ''
})

// 组件挂载时加载API格式
onMounted(() => {
  loadApiFormats()
})

// 重置表单
function resetForm() {
  form.value = {
    api_format: '',
    base_url: '',
    custom_path: '',
    timeout: 300,
    max_retries: 2,
    max_concurrent: undefined,
    rate_limit: undefined,
    is_active: true,
    proxy_url: '',
    proxy_username: '',
    proxy_password: '',
  }
  selectedFormats.value = []
  proxyEnabled.value = false
}

// 原始密码占位符（后端返回的脱敏标记）
const MASKED_PASSWORD = '***'

// 加载端点数据（编辑模式）
function loadEndpointData() {
  if (!props.endpoint) return

  const proxy = props.endpoint.proxy as { url?: string; username?: string; password?: string; enabled?: boolean } | null

  form.value = {
    api_format: props.endpoint.api_format,
    base_url: props.endpoint.base_url,
    custom_path: props.endpoint.custom_path || '',
    timeout: props.endpoint.timeout,
    max_retries: props.endpoint.max_retries,
    max_concurrent: props.endpoint.max_concurrent || undefined,
    rate_limit: props.endpoint.rate_limit || undefined,
    is_active: props.endpoint.is_active,
    proxy_url: proxy?.url || '',
    proxy_username: proxy?.username || '',
    // 如果密码是脱敏标记，显示为空（让用户知道有密码但看不到）
    proxy_password: proxy?.password === MASKED_PASSWORD ? '' : (proxy?.password || ''),
  }

  // 根据 enabled 字段或 url 存在判断是否启用代理
  proxyEnabled.value = proxy?.enabled ?? !!proxy?.url
}

// 使用 useFormDialog 统一处理对话框逻辑
const { isEditMode, handleDialogUpdate, handleCancel } = useFormDialog({
  isOpen: () => props.modelValue,
  entity: () => props.endpoint,
  isLoading: loading,
  onClose: () => emit('update:modelValue', false),
  loadData: loadEndpointData,
  resetForm,
})

// 构建代理配置
// - 有 URL 时始终保存配置，通过 enabled 字段控制是否启用
// - 无 URL 时返回 null
function buildProxyConfig(): { url: string; username?: string; password?: string; enabled: boolean } | null {
  if (!form.value.proxy_url) {
    // 没填 URL，无代理配置
    return null
  }
  return {
    url: form.value.proxy_url,
    username: form.value.proxy_username || undefined,
    password: form.value.proxy_password || undefined,
    enabled: proxyEnabled.value,  // 开关状态决定是否启用
  }
}

// 提交表单
const handleSubmit = async (skipCredentialCheck = false) => {
  if (!props.provider && !props.endpoint) return

  // 只在开关开启且填写了 URL 时验证
  if (proxyEnabled.value && form.value.proxy_url && proxyUrlError.value) {
    showError(proxyUrlError.value, '代理配置错误')
    return
  }

  // 检查：开关开启但没有 URL，却有用户名或密码
  const hasOrphanedCredentials = proxyEnabled.value
    && !form.value.proxy_url
    && (form.value.proxy_username || form.value.proxy_password)

  if (hasOrphanedCredentials && !skipCredentialCheck) {
    // 弹出确认对话框
    showClearCredentialsDialog.value = true
    return
  }

  loading.value = true
  let successCount = 0
  
  try {
    const proxyConfig = buildProxyConfig()

    if (isEditMode.value && props.endpoint) {
      // 更新端点
      await updateEndpoint(props.endpoint.id, {
        base_url: form.value.base_url,
        custom_path: form.value.custom_path || undefined,
        timeout: form.value.timeout,
        max_retries: form.value.max_retries,
        max_concurrent: form.value.max_concurrent,
        rate_limit: form.value.rate_limit,
        is_active: form.value.is_active,
        proxy: proxyConfig,
      })

      success('端点已更新', '保存成功')
      emit('endpointUpdated')
      emit('update:modelValue', false)
    } else if (props.provider) {
      // 批量创建端点
      let failCount = 0
      const errors: string[] = []

      for (const apiFormat of selectedFormats.value) {
        try {
          await createEndpoint(props.provider.id, {
            provider_id: props.provider.id,
            api_format: apiFormat,
            base_url: form.value.base_url,
            custom_path: form.value.custom_path || undefined,
            timeout: form.value.timeout,
            max_retries: form.value.max_retries,
            max_concurrent: form.value.max_concurrent,
            rate_limit: form.value.rate_limit,
            is_active: form.value.is_active,
            proxy: proxyConfig,
          })
          successCount++
        } catch (error: any) {
          failCount++
          const formatLabel = apiFormats.value.find((f: any) => f.value === apiFormat)?.label || apiFormat
          errors.push(`${formatLabel}: ${error.response?.data?.detail || '创建失败'}`)
        }
      }

      // 显示结果
      if (successCount > 0 && failCount === 0) {
        success(`成功创建 ${successCount} 个端点`, '创建成功')
      } else if (successCount > 0 && failCount > 0) {
        success(`成功创建 ${successCount} 个端点，${failCount} 个失败`, '部分成功')
        if (errors.length > 0) {
          log.error('创建端点失败:', errors)
        }
      } else {
        showError(errors.join('\n') || '创建端点失败', '创建失败')
      }

      if (successCount > 0) {
        emit('endpointCreated')
        resetForm()
        emit('update:modelValue', false)
      }
    }
  } catch (error: any) {
    const action = isEditMode.value ? '更新' : '创建'
    showError(error.response?.data?.detail || `${action}端点失败`, '错误')
  } finally {
    loading.value = false
  }
}

// 确认清空凭据并继续保存
const confirmClearCredentials = () => {
  form.value.proxy_username = ''
  form.value.proxy_password = ''
  showClearCredentialsDialog.value = false
  handleSubmit(true)  // 跳过凭据检查，直接提交
}
</script>
