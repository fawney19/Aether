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
                  <SelectItem value="claude_code">
                    ClaudeCode
                  </SelectItem>
                  <SelectItem value="codex">
                    Codex
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
                  <SelectItem value="claude_code">
                    ClaudeCode
                  </SelectItem>
                  <SelectItem value="codex">
                    Codex
                  </SelectItem>
                  <SelectItem value="gemini_cli">
                    GeminiCli
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
              max="10"
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

      <!-- 格式转换配置 -->
      <div class="space-y-3">
        <h3 class="text-sm font-medium border-b pb-2">
          格式转换
        </h3>
        <div class="flex items-center justify-between p-3 border rounded-lg bg-muted/50">
          <div class="space-y-0.5">
            <span class="text-sm font-medium">保持优先级</span>
            <p class="text-xs text-muted-foreground">
              跨格式请求时保持原优先级排名，不降级到格式匹配的提供商之后
            </p>
          </div>
          <Switch
            :model-value="form.keep_priority_on_conversion"
            @update:model-value="(v: boolean) => form.keep_priority_on_conversion = v"
          />
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
              @update:model-value="handleProxyToggle"
            />
            <span class="text-sm text-muted-foreground">启用代理</span>
          </div>
        </div>
        <div
          v-if="form.proxy_enabled"
          class="space-y-1.5 p-3 border rounded-lg bg-muted/50"
        >
          <Label class="text-xs">代理节点 *</Label>
          <ProxyNodeSelect
            ref="proxyNodeSelectRef"
            v-model="form.proxy_node_id"
          />
        </div>
      </div>

      <!-- Claude Code 高级配置 -->
      <Collapsible
        v-if="form.provider_type === 'claude_code'"
        v-model:open="claudeAdvancedOpen"
        class="space-y-3"
      >
        <CollapsibleTrigger as-child>
          <button
            type="button"
            class="w-full flex items-center justify-between py-1.5 px-2 -mx-2 rounded-md hover:bg-muted/50 transition-colors border-b"
          >
            <span class="text-sm font-medium">高级配置</span>
            <ChevronDown
              class="w-4 h-4 text-muted-foreground transition-transform"
              :class="{ '-rotate-90': !claudeAdvancedOpen }"
            />
          </button>
        </CollapsibleTrigger>

        <CollapsibleContent class="space-y-3">
          <div class="space-y-3 p-3 border rounded-lg bg-muted/50">
            <div class="flex items-center justify-between">
              <div class="space-y-0.5">
                <span class="text-sm font-medium">会话数量控制</span>
                <p class="text-xs text-muted-foreground">
                  限制同时活跃的会话数量
                </p>
              </div>
              <Switch
                :model-value="form.claude_session_control_enabled"
                @update:model-value="(v: boolean) => form.claude_session_control_enabled = v"
              />
            </div>

            <div
              v-if="form.claude_session_control_enabled"
              class="grid grid-cols-2 gap-4"
            >
              <div class="space-y-1.5">
                <Label class="text-xs">最大会话数</Label>
                <Input
                  :model-value="form.claude_max_sessions ?? ''"
                  type="number"
                  min="1"
                  max="1000"
                  placeholder="例如 20"
                  @update:model-value="(v) => form.claude_max_sessions = parseNumberInput(v)"
                />
              </div>
              <div class="space-y-1.5">
                <Label class="text-xs">会话空闲超时 (分钟)</Label>
                <Input
                  :model-value="form.claude_session_idle_timeout_minutes ?? ''"
                  type="number"
                  min="1"
                  max="1440"
                  placeholder="默认 5"
                  @update:model-value="(v) => form.claude_session_idle_timeout_minutes = parseNumberInput(v) ?? 5"
                />
              </div>
            </div>
          </div>

          <div class="flex items-center justify-between p-3 border rounded-lg bg-muted/50">
            <div class="space-y-0.5">
              <span class="text-sm font-medium">TLS 指纹模拟</span>
              <p class="text-xs text-muted-foreground">
                模拟 Node.js / Claude Code 客户端的 TLS 指纹
              </p>
            </div>
            <Switch
              :model-value="form.claude_enable_tls_fingerprint"
              @update:model-value="(v: boolean) => form.claude_enable_tls_fingerprint = v"
            />
          </div>

          <div class="flex items-center justify-between p-3 border rounded-lg bg-muted/50">
            <div class="space-y-0.5">
              <span class="text-sm font-medium">会话 ID 伪装</span>
              <p class="text-xs text-muted-foreground">
                启用后在 15 分钟内固定 metadata.user_id 中的 session ID
              </p>
            </div>
            <Switch
              :model-value="form.claude_session_id_masking_enabled"
              @update:model-value="(v: boolean) => form.claude_session_id_masking_enabled = v"
            />
          </div>
        </CollapsibleContent>
      </Collapsible>
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
  Collapsible,
  CollapsibleTrigger,
  CollapsibleContent,
} from '@/components/ui'
import { Server, SquarePen, ChevronDown } from 'lucide-vue-next'
import { useToast } from '@/composables/useToast'
import { useFormDialog } from '@/composables/useFormDialog'
import { createProvider, updateProvider, type ProviderWithEndpointsSummary } from '@/api/endpoints'
import { parseApiError } from '@/utils/errorParser'
import { parseNumberInput } from '@/utils/form'
import ProxyNodeSelect from './ProxyNodeSelect.vue'
import { useProxyNodesStore } from '@/stores/proxy-nodes'

const props = defineProps<{
  modelValue: boolean
  provider?: ProviderWithEndpointsSummary | null  // 编辑模式时传入
  maxPriority?: number  // 当前已有的最大优先级值
}>()

const emit = defineEmits<{
  'update:modelValue': [value: boolean]
  'providerCreated': []
  'providerUpdated': []
}>()

const { success, error: showError } = useToast()
const loading = ref(false)
const claudeAdvancedOpen = ref(true)
const proxyNodeSelectRef = ref<InstanceType<typeof ProxyNodeSelect> | null>(null)
const proxyNodesStore = useProxyNodesStore()

/** 启用代理时懒加载节点列表 */
function handleProxyToggle(v: boolean) {
  form.value.proxy_enabled = v
  if (v) {
    proxyNodesStore.ensureLoaded()
  }
}

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
  provider_type: 'custom' as 'custom' | 'claude_code' | 'codex' | 'gemini_cli' | 'antigravity' | 'kiro',
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
  // 代理配置
  proxy_enabled: false,
  proxy_node_id: '',
  // Claude Code 高级配置
  claude_session_control_enabled: false,
  claude_max_sessions: undefined as number | undefined,
  claude_session_idle_timeout_minutes: 5,
  claude_enable_tls_fingerprint: false,
  claude_session_id_masking_enabled: false,
})

// 重置表单
function resetForm() {
  claudeAdvancedOpen.value = true
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
    // 代理配置
    proxy_enabled: false,
    proxy_node_id: '',
    // Claude Code 高级配置
    claude_session_control_enabled: false,
    claude_max_sessions: undefined,
    claude_session_idle_timeout_minutes: 5,
    claude_enable_tls_fingerprint: false,
    claude_session_id_masking_enabled: false,
  }
}

// 加载提供商数据（编辑模式）
function loadProviderData() {
  if (!props.provider) return

  claudeAdvancedOpen.value = true
  const proxy = props.provider.proxy
  const claudeAdvanced = props.provider.claude_code_advanced || null
  const claudeMaxSessions = claudeAdvanced?.max_sessions
  const claudeSessionControlEnabled = typeof claudeMaxSessions === 'number' && claudeMaxSessions > 0
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
    // 代理配置
    proxy_enabled: proxy?.enabled ?? false,
    proxy_node_id: proxy?.node_id || '',
    // Claude Code 高级配置
    claude_session_control_enabled: claudeSessionControlEnabled,
    claude_max_sessions: claudeSessionControlEnabled ? claudeMaxSessions : undefined,
    claude_session_idle_timeout_minutes: claudeAdvanced?.session_idle_timeout_minutes ?? 5,
    claude_enable_tls_fingerprint: claudeAdvanced?.enable_tls_fingerprint ?? false,
    claude_session_id_masking_enabled: claudeAdvanced?.session_id_masking_enabled ?? false,
  }

  // 如果有代理配置，确保加载节点列表（直接调用 store，避免 ref 未挂载时静默失败）
  if (proxy?.enabled) {
    proxyNodesStore.ensureLoaded()
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

  // 启用代理时的验证
  if (form.value.proxy_enabled && !form.value.proxy_node_id) {
    showError('请选择代理节点', '验证失败')
    return
  }

  // Claude Code 高级配置验证
  if (form.value.provider_type === 'claude_code' && form.value.claude_session_control_enabled) {
    if (!form.value.claude_max_sessions || form.value.claude_max_sessions < 1) {
      showError('请填写有效的最大会话数（>=1）', '验证失败')
      return
    }
  }

  loading.value = true
  try {
    // 构建代理配置
    const proxy = form.value.proxy_enabled && form.value.proxy_node_id ? {
      node_id: form.value.proxy_node_id,
      enabled: true,
    } : null
    const claudeCodeAdvanced = form.value.provider_type === 'claude_code'
      ? {
          max_sessions: form.value.claude_session_control_enabled
            ? (form.value.claude_max_sessions ?? undefined)
            : null,
          session_idle_timeout_minutes: form.value.claude_session_control_enabled
            ? (form.value.claude_session_idle_timeout_minutes ?? 5)
            : null,
          enable_tls_fingerprint: form.value.claude_enable_tls_fingerprint,
          session_id_masking_enabled: form.value.claude_session_id_masking_enabled,
        }
      : undefined

    const payload = {
      name: form.value.name,
      provider_type: form.value.provider_type,
      description: form.value.description || undefined,
      website: form.value.website || undefined,
      billing_type: form.value.billing_type,
      monthly_quota_usd: form.value.monthly_quota_usd,
      quota_reset_day: form.value.quota_reset_day,
      quota_last_reset_at: form.value.quota_last_reset_at || undefined,
      quota_expires_at: form.value.quota_expires_at || undefined,
      provider_priority: form.value.provider_priority,
      keep_priority_on_conversion: form.value.keep_priority_on_conversion,
      is_active: form.value.is_active,
      // 请求配置
      max_retries: form.value.max_retries ?? undefined,
      // 超时配置（null 表示清除，使用全局配置）
      stream_first_byte_timeout: form.value.stream_first_byte_timeout ?? null,
      request_timeout: form.value.request_timeout ?? null,
      proxy,
      claude_code_advanced: claudeCodeAdvanced,
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
  } catch (error: unknown) {
    const action = isEditMode.value ? '更新' : '创建'
    showError(parseApiError(error, `${action}提供商失败`), `${action}失败`)
  } finally {
    loading.value = false
  }
}
</script>
