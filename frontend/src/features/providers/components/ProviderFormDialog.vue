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

        <div class="space-y-1.5">
          <Label for="name">名称 *</Label>
          <Input
            id="name"
            v-model="form.name"
            placeholder="例如: OpenAI 主账号"
          />
        </div>

        <div class="grid grid-cols-2 gap-4">
          <div class="space-y-1.5">
            <Label>提供商类型</Label>
            <Select
              v-model="form.provider_type"
              :disabled="isEditMode"
            >
              <SelectTrigger>
                <SelectValue placeholder="请选择" />
              </SelectTrigger>
              <SelectContent>
                <!-- 新建模式：允许自定义及各反代类型 -->
                <template v-if="!isEditMode">
                  <SelectItem value="custom">
                    自定义
                  </SelectItem>
                  <SelectItem value="vertex_ai">
                    Vertex AI
                  </SelectItem>
                  <SelectItem value="claude_code">
                    ClaudeCode
                  </SelectItem>
                  <SelectItem value="codex">
                    Codex
                  </SelectItem>
                  <SelectItem value="gemini_cli">
                    Gemini CLI
                  </SelectItem>
                  <SelectItem value="kiro">
                    Kiro
                  </SelectItem>
                  <SelectItem value="antigravity">
                    Antigravity
                  </SelectItem>
                </template>
                <!-- 编辑模式：显示所有类型（兼容已有数据） -->
                <template v-else>
                  <SelectItem value="custom">
                    自定义
                  </SelectItem>
                  <SelectItem value="vertex_ai">
                    Vertex AI
                  </SelectItem>
                  <SelectItem value="claude_code">
                    ClaudeCode
                  </SelectItem>
                  <SelectItem value="codex">
                    Codex
                  </SelectItem>
                  <SelectItem value="gemini_cli">
                    Gemini CLI
                  </SelectItem>
                  <SelectItem value="kiro">
                    Kiro
                  </SelectItem>
                  <SelectItem value="antigravity">
                    Antigravity
                  </SelectItem>
                </template>
              </SelectContent>
            </Select>
            <p
              v-if="!isEditMode && form.provider_type !== 'custom'"
              class="text-xs text-muted-foreground"
            >
              反代使用固定端点且不可修改
            </p>
          </div>
          <div class="space-y-1.5">
            <Label for="website">主站链接</Label>
            <Input
              id="website"
              v-model="form.website"
              placeholder="https://example.com（可选）"
            />
          </div>
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
          <div class="space-y-1.5">
            <Label>最大重试次数</Label>
            <Input
              :model-value="form.max_retries ?? ''"
              type="number"
              min="0"
              max="999"
              placeholder="默认 2"
              @update:model-value="(v) => form.max_retries = parseNumberInput(v)"
            />
          </div>
        </div>

        <!-- 超时配置 -->
        <div class="grid grid-cols-2 gap-4">
          <div class="space-y-1.5">
            <Label>
              流式首字节超时
              <span class="text-xs text-muted-foreground">(秒)</span>
            </Label>
            <Input
              :model-value="form.stream_first_byte_timeout ?? ''"
              type="number"
              min="1"
              max="300"
              step="1"
              placeholder="30"
              @update:model-value="(v) => form.stream_first_byte_timeout = parseNumberInput(v)"
            />
          </div>
          <div class="space-y-1.5">
            <Label>
              非流式请求超时
              <span class="text-xs text-muted-foreground">(秒)</span>
            </Label>
            <Input
              :model-value="form.request_timeout ?? ''"
              type="number"
              min="1"
              max="600"
              step="1"
              placeholder="300"
              @update:model-value="(v) => form.request_timeout = parseNumberInput(v)"
            />
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

      <!-- 功能开关 -->
      <div class="space-y-3">
        <h3 class="text-sm font-medium border-b pb-2">
          功能开关
        </h3>

        <div class="flex items-center justify-between p-3 border rounded-lg bg-muted/50">
          <div class="space-y-0.5">
            <span class="text-sm font-medium">格式转换保持优先级</span>
            <p class="text-xs text-muted-foreground">
              跨格式请求时保持原优先级排名，不降级到格式匹配的提供商之后
            </p>
          </div>
          <Switch
            :model-value="form.keep_priority_on_conversion"
            @update:model-value="(v: boolean) => form.keep_priority_on_conversion = v"
          />
        </div>

        <div class="flex items-center justify-between p-3 border rounded-lg bg-muted/50">
          <div class="space-y-0.5">
            <span class="text-sm font-medium">号池调度模式</span>
            <p class="text-xs text-muted-foreground">
              启用后该提供商的密钥将由号池统一调度
            </p>
          </div>
          <Switch
            :model-value="form.pool_mode_enabled"
            @update:model-value="(v: boolean) => form.pool_mode_enabled = v"
          />
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
import { ref, computed, watch } from 'vue'
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
  maxPriority?: number  // 当前已有的最大优先级值
}>()

const emit = defineEmits<{
  'update:modelValue': [value: boolean]
  'providerCreated': []
  'providerUpdated': [provider: ProviderWithEndpointsSummary]
}>()

const { success, error: showError } = useToast()
const loading = ref(false)

// 内部状态
const internalOpen = computed(() => props.modelValue)

// 计算新建时的默认优先级
const defaultPriority = computed(() => {
  if (props.maxPriority != null) {
    return Math.min(props.maxPriority + 10, 10000)
  }
  return 100
})

// 表单数据
const form = ref({
  name: '',
  provider_type: 'custom' as 'custom' | 'vertex_ai' | 'claude_code' | 'codex' | 'gemini_cli' | 'antigravity' | 'kiro',
  description: '',
  website: '',
  // 计费配置
  billing_type: 'pay_as_you_go' as 'monthly_quota' | 'pay_as_you_go' | 'free_tier',
  monthly_quota_usd: undefined as number | undefined,
  quota_reset_day: 30,
  quota_last_reset_at: '',  // 周期开始时间
  quota_expires_at: '',
  provider_priority: 100,
  keep_priority_on_conversion: false,  // 格式转换时是否保持优先级
  // 状态配置
  is_active: true,
  rate_limit: undefined as number | undefined,
  concurrent_limit: undefined as number | undefined,
  // 请求配置
  max_retries: undefined as number | undefined,
  // 超时配置（秒）
  stream_first_byte_timeout: undefined as number | undefined,
  request_timeout: undefined as number | undefined,
  // 号池模式
  pool_mode_enabled: false,
})

// 重置表单
function resetForm() {
  form.value = {
    name: '',
    provider_type: 'custom',
    description: '',
    website: '',
    billing_type: 'pay_as_you_go',
    monthly_quota_usd: undefined,
    quota_reset_day: 30,
    quota_last_reset_at: '',
    quota_expires_at: '',
    provider_priority: defaultPriority.value,
    keep_priority_on_conversion: false,
    is_active: true,
    rate_limit: undefined,
    concurrent_limit: undefined,
    // 请求配置
    max_retries: undefined,
    // 超时配置
    stream_first_byte_timeout: undefined,
    request_timeout: undefined,
    // 号池模式
    pool_mode_enabled: false,
  }
}

// 加载提供商数据（编辑模式）
function loadProviderData() {
  if (!props.provider) return

  form.value = {
    name: props.provider.name,
    provider_type: props.provider.provider_type || 'custom',
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
    keep_priority_on_conversion: props.provider.keep_priority_on_conversion ?? false,
    is_active: props.provider.is_active,
    rate_limit: undefined,
    concurrent_limit: undefined,
    // 请求配置
    max_retries: props.provider.max_retries ?? undefined,
    // 超时配置
    stream_first_byte_timeout: props.provider.stream_first_byte_timeout ?? undefined,
    request_timeout: props.provider.request_timeout ?? undefined,
    // 号池模式
    pool_mode_enabled: !!props.provider.pool_advanced,
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

// 新建模式下切换 provider_type 时不自动开启号池模式
watch(() => form.value.provider_type, () => {
  if (!isEditMode.value) {
    form.value.pool_mode_enabled = false
  }
})

// 提交表单
const handleSubmit = async () => {
  // 月卡类型必须设置周期开始时间
  if (form.value.billing_type === 'monthly_quota' && !form.value.quota_last_reset_at) {
    showError('月卡类型必须设置周期开始时间', '验证失败')
    return
  }

  loading.value = true
  try {
    const basePayload = {
      name: form.value.name,
      provider_type: form.value.provider_type,
      description: form.value.description || undefined,
      website: form.value.website || undefined,
      billing_type: form.value.billing_type,
      monthly_quota_usd: form.value.monthly_quota_usd,
      quota_reset_day: form.value.quota_reset_day,
      quota_last_reset_at: form.value.quota_last_reset_at || undefined,
      quota_expires_at: form.value.quota_expires_at || undefined,
      keep_priority_on_conversion: form.value.keep_priority_on_conversion,
      is_active: form.value.is_active,
      // 请求配置
      max_retries: form.value.max_retries ?? undefined,
      // 超时配置（null 表示清除，使用全局配置）
      stream_first_byte_timeout: form.value.stream_first_byte_timeout ?? null,
      request_timeout: form.value.request_timeout ?? null,
      pool_advanced: form.value.pool_mode_enabled
        ? (props.provider?.pool_advanced ?? {})
        : null,
    }

    if (isEditMode.value && props.provider) {
      // 更新提供商
      const updated = await updateProvider(props.provider.id, {
        ...basePayload,
        provider_priority: form.value.provider_priority,
      })
      success('提供商更新成功')
      emit('providerUpdated', updated)
    } else {
      // 创建提供商（优先级由后端自动置顶）
      await createProvider(basePayload)
      success('提供商已创建，请继续添加端点和密钥，或在优先级管理中调整顺序', '创建成功')
      emit('providerCreated')
    }

    emit('update:modelValue', false)
  } catch (error: unknown) {
    const action = isEditMode.value ? '更新' : '创建'
    showError(parseApiError(error, `${action}提供商失败`), `${action}失败`)
  } finally {
    loading.value = false
  }
}
</script>
