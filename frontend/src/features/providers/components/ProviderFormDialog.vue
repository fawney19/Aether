<template>
  <Dialog
    :model-value="internalOpen"
    :title="isEditMode ? '编辑提供商' : '添加提供商'"
    :description="isEditMode ? '更新提供商配置。API 端点和密钥需在详情页面单独管理。' : '创建新的提供商配置。创建后可以为其添加 API 端点和密钥。'"
    :icon="isEditMode ? SquarePen : Server"
    size="xl"
    @update:model-value="handleDialogUpdate"
  >
    <form
      class="space-y-5"
      @submit.prevent="handleSubmit"
    >
      <!-- 基本信息 -->
      <div class="space-y-3">
        <h3 class="text-sm font-medium border-b pb-2">
          基本信息
        </h3>

        <div class="grid grid-cols-2 gap-4">
          <div class="space-y-1.5">
            <Label for="name">名称 *</Label>
            <Input
              id="name"
              v-model="form.name"
              placeholder="例如: OpenAI 主账号"
            />
          </div>
          <div class="space-y-1.5">
            <Label for="website">主站链接</Label>
            <Input
              id="website"
              v-model="form.website"
              placeholder="https://..."
              type="url"
            />
          </div>
        </div>

        <div class="space-y-1.5">
          <Label for="description">描述</Label>
          <Input
            id="description"
            v-model="form.description"
            placeholder="提供商描述（可选）"
          />
        </div>
      </div>

      <!-- 计费与限流 / 请求配置 -->
      <div class="space-y-3">
        <div class="grid grid-cols-2 gap-4">
          <h3 class="text-sm font-medium border-b pb-2">
            计费与限流
          </h3>
          <h3 class="text-sm font-medium border-b pb-2">
            请求配置
          </h3>
        </div>
        <div class="grid grid-cols-2 gap-4">
          <div class="space-y-1.5">
            <Label>计费类型</Label>
            <Select
              v-model="form.billing_type"
              v-model:open="billingTypeSelectOpen"
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="monthly_quota">
                  月卡额度
                </SelectItem>
                <SelectItem value="pay_as_you_go">
                  按量付费
                </SelectItem>
                <SelectItem value="free_tier">
                  免费套餐
                </SelectItem>
              </SelectContent>
            </Select>
          </div>
          <div class="grid grid-cols-2 gap-4">
            <div class="space-y-1.5">
              <Label>超时时间 (秒)</Label>
              <Input
                :model-value="form.timeout ?? ''"
                type="number"
                min="1"
                max="600"
                placeholder="默认 300"
                @update:model-value="(v) => form.timeout = parseNumberInput(v)"
              />
            </div>
            <div class="space-y-1.5">
              <Label>最大重试次数</Label>
              <Input
                :model-value="form.max_retries ?? ''"
                type="number"
                min="0"
                max="10"
                placeholder="默认 2"
                @update:model-value="(v) => form.max_retries = parseNumberInput(v)"
              />
            </div>
          </div>
        </div>

        <!-- 月卡配置 -->
        <div
          v-if="form.billing_type === 'monthly_quota'"
          class="grid grid-cols-2 gap-4 p-3 border rounded-lg bg-muted/50"
        >
          <div class="space-y-1.5">
            <Label class="text-xs">周期额度 (USD)</Label>
            <Input
              :model-value="form.monthly_quota_usd ?? ''"
              type="number"
              step="0.01"
              min="0"
              @update:model-value="(v) => form.monthly_quota_usd = parseNumberInput(v, { allowFloat: true })"
            />
          </div>
          <div class="space-y-1.5">
            <Label class="text-xs">重置周期 (天)</Label>
            <Input
              :model-value="form.quota_reset_day ?? ''"
              type="number"
              min="1"
              max="365"
              @update:model-value="(v) => form.quota_reset_day = parseNumberInput(v) ?? 30"
            />
          </div>
          <div class="space-y-1.5">
            <Label class="text-xs">
              周期开始时间 <span class="text-red-500">*</span>
            </Label>
            <Input
              v-model="form.quota_last_reset_at"
              type="datetime-local"
            />
          </div>
          <div class="space-y-1.5">
            <Label class="text-xs">过期时间</Label>
            <Input
              v-model="form.quota_expires_at"
              type="datetime-local"
            />
          </div>
        </div>
      </div>

      <!-- 代理配置 -->
      <div class="space-y-3">
        <div class="flex items-center justify-between">
          <h3 class="text-sm font-medium">
            代理配置
          </h3>
          <div class="flex items-center gap-2">
            <Switch
              :model-value="form.proxy_enabled"
              @update:model-value="(v: boolean) => form.proxy_enabled = v"
            />
            <span class="text-sm text-muted-foreground">启用代理</span>
          </div>
        </div>
        <div
          v-if="form.proxy_enabled"
          class="grid grid-cols-2 gap-4 p-3 border rounded-lg bg-muted/50"
        >
          <div class="space-y-1.5">
            <Label class="text-xs">代理地址 *</Label>
            <Input
              v-model="form.proxy_url"
              placeholder="http://proxy:port 或 socks5://proxy:port"
            />
          </div>
          <div class="grid grid-cols-2 gap-3">
            <div class="space-y-1.5">
              <Label class="text-xs">用户名</Label>
              <Input
                v-model="form.proxy_username"
                placeholder="可选"
                autocomplete="off"
                data-form-type="other"
                data-lpignore="true"
                data-1p-ignore="true"
              />
            </div>
            <div class="space-y-1.5">
              <Label class="text-xs">密码</Label>
              <Input
                v-model="form.proxy_password"
                type="password"
                placeholder="可选"
                autocomplete="new-password"
                data-form-type="other"
                data-lpignore="true"
                data-1p-ignore="true"
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
        :disabled="loading || !form.name"
        @click="handleSubmit"
      >
        {{ loading ? (isEditMode ? '保存中...' : '创建中...') : (isEditMode ? '保存' : '创建') }}
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { ref, computed } from 'vue'
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
import { Server, SquarePen } from 'lucide-vue-next'
import { useToast } from '@/composables/useToast'
import { useFormDialog } from '@/composables/useFormDialog'
import { createProvider, updateProvider, type ProviderWithEndpointsSummary } from '@/api/endpoints'
import { parseApiError } from '@/utils/errorParser'
import { parseNumberInput } from '@/utils/form'

const props = defineProps<{
  modelValue: boolean
  provider?: ProviderWithEndpointsSummary | null  // 编辑模式时传入
}>()

const emit = defineEmits<{
  'update:modelValue': [value: boolean]
  'providerCreated': []
  'providerUpdated': []
}>()

const { success, error: showError } = useToast()
const loading = ref(false)
const billingTypeSelectOpen = ref(false)

// 内部状态
const internalOpen = computed(() => props.modelValue)

// 表单数据
const form = ref({
  name: '',
  description: '',
  website: '',
  // 计费配置
  billing_type: 'pay_as_you_go' as 'monthly_quota' | 'pay_as_you_go' | 'free_tier',
  monthly_quota_usd: undefined as number | undefined,
  quota_reset_day: 30,
  quota_last_reset_at: '',  // 周期开始时间
  quota_expires_at: '',
  provider_priority: 999,
  // 状态配置
  is_active: true,
  rate_limit: undefined as number | undefined,
  concurrent_limit: undefined as number | undefined,
  // 请求配置
  timeout: undefined as number | undefined,
  max_retries: undefined as number | undefined,
  // 代理配置（扁平化便于表单绑定）
  proxy_enabled: false,
  proxy_url: '',
  proxy_username: '',
  proxy_password: '',
})

// 重置表单
function resetForm() {
  form.value = {
    name: '',
    description: '',
    website: '',
    billing_type: 'pay_as_you_go',
    monthly_quota_usd: undefined,
    quota_reset_day: 30,
    quota_last_reset_at: '',
    quota_expires_at: '',
    provider_priority: 999,
    is_active: true,
    rate_limit: undefined,
    concurrent_limit: undefined,
    // 请求配置
    timeout: undefined,
    max_retries: undefined,
    // 代理配置
    proxy_enabled: false,
    proxy_url: '',
    proxy_username: '',
    proxy_password: '',
  }
}

// 加载提供商数据（编辑模式）
function loadProviderData() {
  if (!props.provider) return

  const proxy = props.provider.proxy
  form.value = {
    name: props.provider.name,
    description: props.provider.description || '',
    website: props.provider.website || '',
    billing_type: (props.provider.billing_type as 'monthly_quota' | 'pay_as_you_go' | 'free_tier') || 'pay_as_you_go',
    monthly_quota_usd: props.provider.monthly_quota_usd || undefined,
    quota_reset_day: props.provider.quota_reset_day || 30,
    quota_last_reset_at: props.provider.quota_last_reset_at ?
      new Date(props.provider.quota_last_reset_at).toISOString().slice(0, 16) : '',
    quota_expires_at: props.provider.quota_expires_at ?
      new Date(props.provider.quota_expires_at).toISOString().slice(0, 16) : '',
    provider_priority: props.provider.provider_priority || 999,
    is_active: props.provider.is_active,
    rate_limit: undefined,
    concurrent_limit: undefined,
    // 请求配置
    timeout: props.provider.timeout ?? undefined,
    max_retries: props.provider.max_retries ?? undefined,
    // 代理配置
    proxy_enabled: proxy?.enabled ?? false,
    proxy_url: proxy?.url || '',
    proxy_username: proxy?.username || '',
    proxy_password: proxy?.password || '',
  }
}

// 使用 useFormDialog 统一处理对话框逻辑
const { isEditMode, handleDialogUpdate, handleCancel } = useFormDialog({
  isOpen: () => props.modelValue,
  entity: () => props.provider,
  isLoading: loading,
  onClose: () => emit('update:modelValue', false),
  loadData: loadProviderData,
  resetForm,
})

// 提交表单
const handleSubmit = async () => {
  // 月卡类型必须设置周期开始时间
  if (form.value.billing_type === 'monthly_quota' && !form.value.quota_last_reset_at) {
    showError('月卡类型必须设置周期开始时间', '验证失败')
    return
  }

  // 启用代理时必须填写代理地址
  if (form.value.proxy_enabled && !form.value.proxy_url) {
    showError('启用代理时必须填写代理地址', '验证失败')
    return
  }

  loading.value = true
  try {
    // 构建代理配置
    const proxy = form.value.proxy_enabled ? {
      url: form.value.proxy_url,
      username: form.value.proxy_username || undefined,
      password: form.value.proxy_password || undefined,
      enabled: true,
    } : null

    const payload = {
      name: form.value.name,
      description: form.value.description || undefined,
      website: form.value.website || undefined,
      billing_type: form.value.billing_type,
      monthly_quota_usd: form.value.monthly_quota_usd,
      quota_reset_day: form.value.quota_reset_day,
      quota_last_reset_at: form.value.quota_last_reset_at || undefined,
      quota_expires_at: form.value.quota_expires_at || undefined,
      provider_priority: form.value.provider_priority,
      is_active: form.value.is_active,
      // 请求配置
      timeout: form.value.timeout ?? undefined,
      max_retries: form.value.max_retries ?? undefined,
      proxy,
    }

    if (isEditMode.value && props.provider) {
      // 更新提供商
      await updateProvider(props.provider.id, payload)
      success('提供商更新成功')
      emit('providerUpdated')
    } else {
      // 创建提供商
      await createProvider(payload)
      success('提供商已创建，请继续添加端点和密钥，或在优先级管理中调整顺序', '创建成功')
      emit('providerCreated')
    }

    emit('update:modelValue', false)
  } catch (error: any) {
    const action = isEditMode.value ? '更新' : '创建'
    showError(parseApiError(error, `${action}提供商失败`), `${action}失败`)
  } finally {
    loading.value = false
  }
}
</script>
