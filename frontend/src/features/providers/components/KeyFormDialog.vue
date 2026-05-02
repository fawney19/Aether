<template>
  <Dialog
    :model-value="isOpen"
    :title="isEditMode ? '编辑密钥' : '添加密钥'"
    :description="isEditMode ? '修改 API 密钥配置' : '为提供商添加新的 API 密钥'"
    :icon="isEditMode ? SquarePen : Key"
    size="xl"
    @update:model-value="handleDialogUpdate"
  >
    <form
      class="space-y-3"
      autocomplete="off"
      @submit.prevent="handleSave"
    >
      <!-- 基本信息 -->
      <div class="grid grid-cols-2 gap-3">
        <div>
          <Label :for="keyNameInputId">密钥名称 *</Label>
          <Input
            :id="keyNameInputId"
            v-model="form.name"
            :name="keyNameFieldName"
            required
            placeholder="例如：主 Key、备用 Key 1"
            maxlength="100"
            autocomplete="off"
            autocapitalize="none"
            autocorrect="off"
            spellcheck="false"
            data-form-type="other"
            data-lpignore="true"
            data-1p-ignore="true"
          />
        </div>
        <div>
          <Label :for="apiKeyInputId">
            {{ authSecretLabel }}
            {{ authSecretRequiredMark }}
          </Label>
          <template v-if="form.auth_type === 'service_account'">
            <JsonImportInput
              v-model="form.auth_config_text"
              :disabled="saving"
              :reset-key="formNonce"
              accept=".json,.txt,application/json,text/plain"
              :multiple="false"
              drop-title="拖入 Service Account JSON 或点击选择"
              drop-hint="支持 .json / .txt，单文件导入"
              :manual-placeholder="editingKey ? '留空表示不修改，或粘贴完整的 Service Account JSON' : '粘贴完整的 Service Account JSON'"
              :manual-description="serviceAccountDescription"
              textarea-class="min-h-[160px] font-mono text-xs break-all !rounded-xl"
              @error="handleServiceAccountImportError"
            />
          </template>
          <template v-else>
            <Input
              :id="apiKeyInputId"
              v-model="form.api_key"
              :name="apiKeyFieldName"
              masked
              :required="false"
              :placeholder="editingKey ? editingKey.api_key_masked : authSecretPlaceholder"
            />
          </template>
          <p
            v-if="editingKey && isRawSecretAuthType(form.auth_type)"
            class="text-xs text-muted-foreground mt-1"
          >
            留空表示不修改
          </p>
        </div>
      </div>

      <!-- 备注 -->
      <div>
        <Label for="note">备注</Label>
        <Input
          id="note"
          v-model="form.note"
          placeholder="可选的备注信息"
        />
      </div>

      <!-- API 格式 & 认证方式 -->
      <div v-if="visibleApiFormats.length > 0">
        <div class="flex items-center gap-1 mb-1.5">
          <Label>支持的 API 格式 *</Label>
          <span
            class="relative inline-flex"
            @mouseenter="apiFormatHelpHovered = true"
            @mouseleave="apiFormatHelpHovered = false"
          >
            <button
              type="button"
              class="inline-flex items-center justify-center rounded-sm p-0.5 text-muted-foreground transition-colors hover:bg-muted/60 hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
              title="API 格式说明"
              aria-label="API 格式说明"
              :aria-expanded="apiFormatHelpVisible"
              @click.stop="toggleApiFormatHelp"
              @focus="apiFormatHelpHovered = true"
              @blur="apiFormatHelpHovered = false"
              @keydown.escape.stop.prevent="apiFormatHelpOpen = false"
            >
              <CircleHelp class="h-3.5 w-3.5" />
            </button>
            <span
              v-if="apiFormatHelpVisible"
              role="tooltip"
              class="absolute left-0 top-full z-[100] mt-1 w-80 rounded-md border bg-popover px-3 py-2 text-xs font-normal normal-case leading-5 tracking-normal text-popover-foreground shadow-md"
            >
              选择此密钥支持的 API 格式及对应认证方式。OpenAI 格式固定使用 Bearer Token；Claude / Gemini 格式可选 API Key 或 Bearer Token（如 Claude Code 应使用 Bearer Token）。
            </span>
          </span>
        </div>
        <div class="flex flex-col gap-1.5">
          <div
            v-for="format in visibleApiFormats"
            :key="format"
            class="flex items-center justify-between rounded-md border px-2 py-1.5 transition-colors cursor-pointer"
            :class="form.api_formats.includes(format)
              ? 'bg-primary/5 border-primary/30'
              : 'bg-muted/30 border-border hover:border-muted-foreground/30'"
            @click="toggleApiFormat(format)"
          >
            <div class="flex items-center gap-1.5 min-w-0">
              <span
                class="w-4 h-4 rounded border flex items-center justify-center text-xs shrink-0"
                :class="form.api_formats.includes(format)
                  ? 'bg-primary border-primary text-primary-foreground'
                  : 'border-muted-foreground/30'"
              >
                <span v-if="form.api_formats.includes(format)">✓</span>
              </span>
              <span
                class="text-sm"
                :class="form.api_formats.includes(format) ? 'text-primary' : 'text-muted-foreground'"
              >{{ formatApiFormat(format) }}</span>
            </div>
            <!-- 认证方式：已勾选且可覆盖时显示 radio -->
            <div class="flex items-center gap-3">
              <div
                v-if="canOverrideFormatAuth(format)"
                class="flex gap-2"
                @click.stop
              >
                <button
                  v-for="opt in authTypeOptions.filter(o => isRawSecretAuthType(o.value))"
                  :key="opt.value"
                  type="button"
                  class="flex items-center gap-1 text-[10px] leading-none transition-colors"
                  :class="getFormatAuthType(format) === opt.value ? 'text-primary' : 'text-muted-foreground hover:text-foreground'"
                  @click="setFormatAuthType(format, opt.value as RawSecretAuthType)"
                >
                  <span
                    class="w-2.5 h-2.5 rounded-full border flex items-center justify-center shrink-0"
                    :class="getFormatAuthType(format) === opt.value ? 'border-primary' : 'border-muted-foreground/40'"
                  >
                    <span
                      v-if="getFormatAuthType(format) === opt.value"
                      class="w-1 h-1 rounded-full bg-primary"
                    />
                  </span>
                  {{ opt.label }}
                </button>
              </div>
              <div
                v-if="canToggleAuthChannelMismatch(format)"
                class="flex items-center gap-1"
                title="允许客户端认证方式不一致时使用"
                @click.stop
              >
                <Switch
                  :model-value="isAuthChannelMismatchAllowed(format)"
                  class="scale-75"
                  @update:model-value="(value) => setAuthChannelMismatchAllowed(format, value)"
                />
              </div>
            </div>
          </div>
        </div>
      </div>

      <!-- 配置项 -->
      <div class="grid grid-cols-4 gap-3">
        <div>
          <Label
            for="internal_priority"
            class="text-xs"
          >优先级</Label>
          <Input
            id="internal_priority"
            v-model.number="form.internal_priority"
            type="number"
            min="0"
            class="h-8"
          />
          <p class="text-xs text-muted-foreground mt-0.5">
            越小越优先
          </p>
        </div>
        <div>
          <Label
            for="rpm_limit"
            class="text-xs"
          >RPM 限制</Label>
          <Input
            id="rpm_limit"
            :model-value="form.rpm_limit ?? ''"
            type="number"
            min="1"
            max="10000"
            placeholder="自适应"
            class="h-8"
            @update:model-value="(v) => form.rpm_limit = parseNullableNumberInput(v, { min: 1, max: 10000 })"
          />
          <p class="text-xs text-muted-foreground mt-0.5">
            留空自适应
          </p>
        </div>
        <div>
          <Label
            for="concurrent_limit"
            class="text-xs"
          >并发请求上限</Label>
          <Input
            id="concurrent_limit"
            :model-value="form.concurrent_limit ?? ''"
            type="number"
            min="0"
            placeholder="不限制"
            class="h-8"
            @update:model-value="(v) => form.concurrent_limit = parseNullableNumberInput(v, { min: 0 })"
          />
          <p class="text-xs text-muted-foreground mt-0.5">
            同一时间允许使用该 Key 的最大请求数，留空或 0 表示不限制
          </p>
        </div>
        <div>
          <Label
            for="cache_ttl_minutes"
            class="text-xs"
          >缓存 TTL</Label>
          <Input
            id="cache_ttl_minutes"
            :model-value="form.cache_ttl_minutes ?? ''"
            type="number"
            min="0"
            max="60"
            class="h-8"
            @update:model-value="(v) => form.cache_ttl_minutes = parseNumberInput(v, { min: 0, max: 60 }) ?? 5"
          />
          <p class="text-xs text-muted-foreground mt-0.5">
            分钟，0禁用
          </p>
        </div>
        <div>
          <Label
            for="max_probe_interval_minutes"
            class="text-xs"
          >熔断探测</Label>
          <Input
            id="max_probe_interval_minutes"
            :model-value="form.max_probe_interval_minutes ?? ''"
            type="number"
            min="0"
            max="32"
            placeholder="32"
            class="h-8"
            @update:model-value="(v) => form.max_probe_interval_minutes = parseNumberInput(v, { min: 0, max: 32 }) ?? 32"
          />
          <p class="text-xs text-muted-foreground mt-0.5">
            分钟，0-32
          </p>
        </div>
      </div>

      <!-- 能力标签 -->
      <div v-if="availableCapabilities.length > 0">
        <Label class="text-xs mb-1.5 block">能力标签</Label>
        <div class="flex flex-wrap gap-1.5">
          <button
            v-for="cap in availableCapabilities"
            :key="cap.name"
            type="button"
            class="inline-flex items-center gap-1.5 px-2.5 py-1 rounded-md border text-sm transition-colors"
            :class="form.capabilities[cap.name]
              ? 'bg-primary/10 border-primary/50 text-primary'
              : 'bg-card border-border hover:bg-muted/50 text-muted-foreground'"
            @click="form.capabilities[cap.name] = !form.capabilities[cap.name]"
          >
            {{ cap.display_name }}
          </button>
        </div>
      </div>

      <!-- 自动获取模型 -->
      <div class="space-y-3 py-2 px-3 rounded-md border border-border/60 bg-muted/30">
        <div class="flex items-center justify-between">
          <div class="space-y-0.5">
            <Label class="text-sm font-medium">自动获取上游可用模型</Label>
            <p class="text-xs text-muted-foreground">
              定时更新上游模型, 配合模型映射使用
            </p>
            <p
              v-if="showAutoFetchWarning"
              class="text-xs text-amber-600 dark:text-amber-400"
            >
              {{ autoFetchWarningMessage }}
            </p>
          </div>
          <Switch v-model="form.auto_fetch_models" />
        </div>

        <!-- 模型过滤规则（仅当开启自动获取时显示） -->
        <div
          v-if="form.auto_fetch_models"
          class="space-y-2 pt-2 border-t border-border/40"
        >
          <div class="grid grid-cols-2 gap-3">
            <div>
              <Label class="text-xs">包含规则</Label>
              <Input
                v-model="form.model_include_patterns_text"
                placeholder="gpt-*, claude-*, 留空包含全部"
                class="h-8 text-sm"
              />
            </div>
            <div>
              <Label class="text-xs">排除规则</Label>
              <Input
                v-model="form.model_exclude_patterns_text"
                placeholder="*-preview, *-beta"
                class="h-8 text-sm"
              />
            </div>
          </div>
          <p class="text-xs text-muted-foreground">
            逗号分隔，支持 * ? 通配符，不区分大小写
          </p>
        </div>
      </div>
    </form>

    <template #footer>
      <Button
        variant="outline"
        @click="handleCancel"
      >
        取消
      </Button>
      <Button
        :disabled="saving || !canSave"
        @click="handleSave"
      >
        {{ saving ? (isEditMode ? '保存中...' : '添加中...') : (isEditMode ? '保存' : '添加') }}
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { ref, computed, onMounted, watch } from 'vue'
import { Dialog, Button, Input, Label, Switch } from '@/components/ui'
import { Key, SquarePen, CircleHelp } from 'lucide-vue-next'
import { useToast } from '@/composables/useToast'
import { useFormDialog } from '@/composables/useFormDialog'
import { parseApiError } from '@/utils/errorParser'
import { parseNumberInput, parseNullableNumberInput } from '@/utils/form'
import { log } from '@/utils/logger'
import JsonImportInput from '@/components/common/JsonImportInput.vue'
import {
  addProviderKey,
  updateProviderKey,
  getAllCapabilities,
  sortApiFormats,
  type EndpointAPIKey,
  type EndpointAPIKeyUpdate,
  type ProviderEndpoint,
  type CapabilityDefinition,
  type ProviderType
} from '@/api/endpoints'
import { formatApiFormat, normalizeApiFormatAlias, formatSupportsAuthOverride } from '@/api/endpoints/types/api-format'

type RawSecretAuthType = 'api_key' | 'bearer'
type ProviderKeyFormAuthType = RawSecretAuthType | 'service_account'

interface AuthTypeOption {
  value: ProviderKeyFormAuthType
  label: string
}

const props = defineProps<{
  open: boolean
  endpoint: ProviderEndpoint | null
  editingKey: EndpointAPIKey | null
  providerId: string | null
  providerType: ProviderType | null
  availableApiFormats: string[]  // Provider 支持的所有 API 格式
}>()

const emit = defineEmits<{
  close: []
  saved: []
}>()

const { success, error: showError } = useToast()

function isRawSecretAuthType(authType: string | null | undefined): authType is RawSecretAuthType {
  return authType === 'api_key' || authType === 'bearer'
}

function normalizeRawSecretAuthType(authType: string | null | undefined): RawSecretAuthType | null {
  const normalized = (authType || '').trim().toLowerCase()
  if (normalized === 'api_key' || normalized === 'apikey' || normalized === 'api-key') return 'api_key'
  if (normalized === 'bearer' || normalized === 'bearer_token' || normalized === 'bearer-token' || normalized === 'authorization') return 'bearer'
  return null
}

function normalizeFormAuthType(authType: string | null | undefined): ProviderKeyFormAuthType {
  const normalized = (authType || '').trim().toLowerCase()
  if (normalized === 'bearer') return 'bearer'
  if (normalized === 'service_account' || normalized === 'vertex_ai') return 'service_account'
  return 'api_key'
}

function getAuthTypeOptions(providerType: ProviderType | null): AuthTypeOption[] {
  if ((providerType || '').toLowerCase() === 'vertex_ai') {
    return [
      { value: 'api_key', label: 'API Key' },
      { value: 'service_account', label: 'Service Account' },
    ]
  }

  return [
    { value: 'api_key', label: 'API Key' },
    { value: 'bearer', label: 'Bearer Token' },
  ]
}

function getVertexAllowedFormatsByAuth(authType: ProviderKeyFormAuthType): Set<string> {
  if (authType === 'api_key') {
    return new Set(['gemini:generate_content'])
  }
  if (authType === 'service_account') {
    return new Set(['gemini:generate_content', 'claude:messages'])
  }
  return new Set()
}

function normalizeApiFormat(format: string): string {
  return normalizeApiFormatAlias(format).trim().toLowerCase()
}

function getSelectableApiFormats(authType = form.value.auth_type): string[] {
  const sorted = sortApiFormats(props.availableApiFormats)
  if (props.providerType !== 'vertex_ai') {
    return sorted
  }

  const allowed = getVertexAllowedFormatsByAuth(authType)
  return sorted.filter(fmt => allowed.has(normalizeApiFormat(fmt)))
}

function sanitizeApiFormats(formats: string[], authType = form.value.auth_type): string[] {
  const selectable = new Set(getSelectableApiFormats(authType).map(normalizeApiFormat))
  if (selectable.size === 0) {
    return []
  }

  return formats.filter(format => selectable.has(normalizeApiFormat(format)))
}

function sanitizeAuthTypeByFormat(
  authTypeByFormat: Record<string, string> | null | undefined,
  formats = form.value.api_formats,
  authType = form.value.auth_type
): Record<string, RawSecretAuthType> {
  if (!isRawSecretAuthType(authType) || !authTypeByFormat) {
    return {}
  }

  const selected = new Set(formats.map(normalizeApiFormat))
  const sanitized: Record<string, RawSecretAuthType> = {}
  for (const [format, rawAuthType] of Object.entries(authTypeByFormat)) {
    const normalizedFormat = normalizeApiFormat(format)
    if (!selected.has(normalizedFormat)) continue
    const normalizedAuthType = normalizeRawSecretAuthType(rawAuthType)
    if (!normalizedAuthType || normalizedAuthType === authType) continue
    sanitized[normalizedFormat] = normalizedAuthType
  }
  return sanitized
}

function sanitizeAllowAuthChannelMismatchFormats(
  formats: string[] | null | undefined,
  selectedFormats = form.value.api_formats
): string[] {
  if (!formats) return []
  const selected = new Set(selectedFormats.map(normalizeApiFormat))
  const seen = new Set<string>()
  const sanitized: string[] = []
  for (const format of formats) {
    const normalizedFormat = normalizeApiFormat(format)
    if (!normalizedFormat || !selected.has(normalizedFormat) || seen.has(normalizedFormat)) {
      continue
    }
    seen.add(normalizedFormat)
    sanitized.push(normalizedFormat)
  }
  return sanitized
}

function getDefaultApiFormats(): string[] {
  const endpointFormat = props.endpoint?.api_format
  if (endpointFormat) {
    const endpointFormats = sanitizeApiFormats([endpointFormat])
    if (endpointFormats.length > 0) {
      return endpointFormats
    }
  }

  const firstAvailableFormat = getSelectableApiFormats()[0]
  return firstAvailableFormat ? [firstAvailableFormat] : []
}

// 按 provider/auth_type 过滤后的可用 API 格式列表
const visibleApiFormats = computed(() => getSelectableApiFormats())

const authTypeOptions = computed(() => getAuthTypeOptions(props.providerType))

const apiFormatHelpOpen = ref(false)
const apiFormatHelpHovered = ref(false)
const apiFormatHelpVisible = computed(() => apiFormatHelpOpen.value || apiFormatHelpHovered.value)

function toggleApiFormatHelp() {
  apiFormatHelpOpen.value = !apiFormatHelpOpen.value
  if (!apiFormatHelpOpen.value) {
    apiFormatHelpHovered.value = false
  }
}

const authSecretLabel = computed(() => {
  if (form.value.auth_type === 'service_account') return 'Service Account JSON'
  if (form.value.auth_type === 'bearer') return 'Bearer Token'
  return 'API 密钥'
})

const authSecretPlaceholder = computed(() =>
  form.value.auth_type === 'bearer' ? 'token-...' : 'sk-...'
)

const authSecretRequiredMark = computed(() => {
  if (form.value.auth_type === 'service_account' && (!props.editingKey || switchingToServiceAccount.value)) {
    return '*'
  }
  return ''
})



function getFormatAuthType(format: string): ProviderKeyFormAuthType {
  if (!isRawSecretAuthType(form.value.auth_type)) {
    return form.value.auth_type
  }
  return form.value.auth_type_by_format[normalizeApiFormat(format)] || form.value.auth_type
}

function canOverrideFormatAuth(format: string): boolean {
  return isRawSecretAuthType(form.value.auth_type) && form.value.api_formats.includes(format) && formatSupportsAuthOverride(format)
}

function setFormatAuthType(format: string, authType: RawSecretAuthType) {
  if (!isRawSecretAuthType(form.value.auth_type)) return
  const normalizedFormat = normalizeApiFormat(format)
  const next = { ...form.value.auth_type_by_format }
  if (authType === form.value.auth_type) {
    delete next[normalizedFormat]
  } else {
    next[normalizedFormat] = authType
  }
  form.value.auth_type_by_format = sanitizeAuthTypeByFormat(next)
}

function canToggleAuthChannelMismatch(format: string): boolean {
  return canOverrideFormatAuth(format)
}

function isAuthChannelMismatchAllowed(format: string): boolean {
  return form.value.allow_auth_channel_mismatch_formats.includes(normalizeApiFormat(format))
}

function setAuthChannelMismatchAllowed(format: string, allowed: boolean) {
  const normalizedFormat = normalizeApiFormat(format)
  const next = new Set(form.value.allow_auth_channel_mismatch_formats.map(normalizeApiFormat))
  if (allowed) {
    next.add(normalizedFormat)
  } else {
    next.delete(normalizedFormat)
  }
  form.value.allow_auth_channel_mismatch_formats = sanitizeAllowAuthChannelMismatchFormats([...next])
}

function buildAuthTypeByFormatPayload(): Record<string, RawSecretAuthType> | null {
  const sanitized = sanitizeAuthTypeByFormat(form.value.auth_type_by_format)
  return Object.keys(sanitized).length > 0 ? sanitized : null
}

function buildAllowAuthChannelMismatchFormatsPayload(): string[] {
  const sanitized = sanitizeAllowAuthChannelMismatchFormats(form.value.allow_auth_channel_mismatch_formats)
  return sanitized
}

const serviceAccountDescription = computed(() => (
  props.editingKey
    ? '留空表示不修改；JSON 格式，包含 project_id、private_key 等字段'
    : 'JSON 格式，包含 project_id、private_key 等字段'
))

// 默认认证类型
function getDefaultAuthType(): ProviderKeyFormAuthType {
  return authTypeOptions.value[0]?.value || 'api_key'
}

function getDefaultAllowAuthChannelMismatchFormats(formats = getDefaultApiFormats()): string[] {
  return sanitizeAllowAuthChannelMismatchFormats(formats, formats)
}

// 显示自动获取模型警告：编辑模式下，原本未启用但现在启用，且已有 allowed_models
const showAutoFetchWarning = computed(() => {
  if (!props.editingKey) return false
  // 原本已启用，不需要警告
  if (props.editingKey.auto_fetch_models) return false
  // 现在未启用，不需要警告
  if (!form.value.auto_fetch_models) return false
  // 检查是否有已配置的模型权限
  const allowedModels = props.editingKey.allowed_models
  if (!allowedModels) return false
  if (Array.isArray(allowedModels) && allowedModels.length === 0) return false
  if (typeof allowedModels === 'object' && Object.keys(allowedModels).length === 0) return false
  return true
})

const autoFetchWarningMessage = computed(() => {
  if (!showAutoFetchWarning.value || !props.editingKey?.allowed_models) return ''
  const models = Array.isArray(props.editingKey.allowed_models)
    ? props.editingKey.allowed_models
    : []
  if (models.length === 0) return ''
  return `当前 Key 模型权限存在以下模型：${models.map(model => `“${model}”`).join('、')}，开启自动获取后将被覆盖`
})

// 检查是否正在切换认证类型
const switchingToServiceAccount = computed(() =>
  !!props.editingKey &&
  props.editingKey.auth_type !== 'service_account' &&
  form.value.auth_type === 'service_account'
)

// 表单是否可以保存
const canSave = computed(() => {
  // 必须填写密钥名称
  if (!form.value.name.trim()) return false
  // 新增模式下根据认证类型判断必填字段
  if (!props.editingKey) {
    if (form.value.auth_type === 'service_account' && !form.value.auth_config_text.trim()) return false
  } else {
    // 编辑模式下切换认证类型时，必须填写对应字段
    if (switchingToServiceAccount.value && !form.value.auth_config_text.trim()) return false
  }
  // 必须至少选择一个 API 格式
  if (form.value.api_formats.length === 0) return false
  return true
})

const isOpen = computed(() => props.open)
const saving = ref(false)
const formNonce = ref(createFieldNonce())
const keyNameInputId = computed(() => `key-name-${formNonce.value}`)
const apiKeyInputId = computed(() => `api-key-${formNonce.value}`)
const keyNameFieldName = computed(() => `key-name-field-${formNonce.value}`)
const apiKeyFieldName = computed(() => `api-key-field-${formNonce.value}`)

// 可用的能力列表
const availableCapabilities = ref<CapabilityDefinition[]>([])

// 新增密钥时默认不自动开启上游模型获取
const defaultAutoFetchModels = computed(() => false)

const form = ref({
  name: '',
  api_key: '',  // 标准 API Key
  auth_type: 'api_key' as ProviderKeyFormAuthType,  // 认证类型
  auth_type_by_format: {} as Record<string, RawSecretAuthType>,
  allow_auth_channel_mismatch_formats: [] as string[],
  auth_config_text: '',  // Service Account JSON 文本（用于表单输入）
  api_formats: [] as string[],  // 支持的 API 格式列表
  rate_multipliers: {} as Record<string, number>,  // 按 API 格式的成本倍率
  internal_priority: 10,
  rpm_limit: undefined as number | null | undefined,  // RPM 限制（null=自适应，undefined=保持原值）
  concurrent_limit: undefined as number | null | undefined,  // 并发请求上限（null/0=不限制，undefined=保持原值）
  cache_ttl_minutes: 5,
  max_probe_interval_minutes: 32,
  note: '',
  is_active: true,
  capabilities: {} as Record<string, boolean>,
  auto_fetch_models: false,
  model_include_patterns_text: '',  // 包含规则文本（逗号分隔）
  model_exclude_patterns_text: ''   // 排除规则文本（逗号分隔）
})

watch(
  [() => form.value.auth_type, () => props.providerType, () => props.availableApiFormats],
  () => {
    const allowedAuthTypes = new Set(authTypeOptions.value.map(option => option.value))
    if (!allowedAuthTypes.has(form.value.auth_type)) {
      form.value.auth_type = getDefaultAuthType()
      return
    }

    const filtered = sanitizeApiFormats(form.value.api_formats)
    if (filtered.length !== form.value.api_formats.length) {
      form.value.api_formats = [...filtered]
    }
    form.value.auth_type_by_format = sanitizeAuthTypeByFormat(form.value.auth_type_by_format)
    form.value.allow_auth_channel_mismatch_formats = sanitizeAllowAuthChannelMismatchFormats(
      form.value.allow_auth_channel_mismatch_formats
    )
  },
  { immediate: true }
)

watch(
  [() => props.availableApiFormats, () => props.open, () => props.editingKey],
  ([, open, editingKey]) => {
    if (!open) {
      return
    }

    const filtered = sanitizeApiFormats(form.value.api_formats)
    if (filtered.length !== form.value.api_formats.length) {
      form.value.api_formats = [...filtered]
      form.value.auth_type_by_format = sanitizeAuthTypeByFormat(form.value.auth_type_by_format, filtered)
      form.value.allow_auth_channel_mismatch_formats = sanitizeAllowAuthChannelMismatchFormats(
        form.value.allow_auth_channel_mismatch_formats,
        filtered
      )
      return
    }

    if (!editingKey && form.value.api_formats.length === 0) {
      const defaults = getDefaultApiFormats()
      if (defaults.length > 0) {
        form.value.api_formats = defaults
        form.value.allow_auth_channel_mismatch_formats =
          getDefaultAllowAuthChannelMismatchFormats(defaults)
      }
    }
  },
  { deep: true, immediate: true }
)

// 加载能力列表
async function loadCapabilities() {
  try {
    availableCapabilities.value = await getAllCapabilities()
  } catch (err) {
    log.error('Failed to load capabilities:', err)
  }
}

onMounted(() => {
  loadCapabilities()
})

// API 格式切换
function toggleApiFormat(format: string) {
  const index = form.value.api_formats.indexOf(format)
  if (index === -1) {
    // 添加格式
    form.value.api_formats.push(format)
    setAuthChannelMismatchAllowed(format, true)
  } else {
    // 移除格式，但保留隐藏配置（用户可能只是临时取消）
    form.value.api_formats.splice(index, 1)
  }
  form.value.allow_auth_channel_mismatch_formats = sanitizeAllowAuthChannelMismatchFormats(
    form.value.allow_auth_channel_mismatch_formats
  )
}


// 重置表单
function resetForm() {
  formNonce.value = createFieldNonce()
  const defaultApiFormats = getDefaultApiFormats()
  form.value = {
    name: '',
    api_key: '',
    auth_type: getDefaultAuthType(),
    auth_type_by_format: {},
    allow_auth_channel_mismatch_formats:
      getDefaultAllowAuthChannelMismatchFormats(defaultApiFormats),
    auth_config_text: '',
    api_formats: defaultApiFormats,
    rate_multipliers: {},
    internal_priority: 10,
    rpm_limit: undefined,
    concurrent_limit: undefined,
    cache_ttl_minutes: 5,
    max_probe_interval_minutes: 32,
    note: '',
    is_active: true,
    capabilities: {},
    auto_fetch_models: defaultAutoFetchModels.value,
    model_include_patterns_text: '',
    model_exclude_patterns_text: ''
  }
}

// 添加成功后清除部分字段以便继续添加
function clearForNextAdd() {
  formNonce.value = createFieldNonce()
  form.value.name = ''
  form.value.api_key = ''
  form.value.auth_config_text = ''
  form.value.auth_type_by_format = sanitizeAuthTypeByFormat(form.value.auth_type_by_format)
  form.value.allow_auth_channel_mismatch_formats = sanitizeAllowAuthChannelMismatchFormats(
    form.value.allow_auth_channel_mismatch_formats
  )
}

// 加载密钥数据（编辑模式）
function loadKeyData() {
  if (!props.editingKey) return
  formNonce.value = createFieldNonce()
  form.value = {
    name: props.editingKey.name,
    api_key: '',
    auth_type: normalizeFormAuthType(props.editingKey.auth_type),
    auth_type_by_format: sanitizeAuthTypeByFormat(
      props.editingKey.auth_type_by_format || {},
      props.editingKey.api_formats || [],
      normalizeFormAuthType(props.editingKey.auth_type)
    ),
    allow_auth_channel_mismatch_formats: sanitizeAllowAuthChannelMismatchFormats(
      props.editingKey.allow_auth_channel_mismatch_formats || [],
      props.editingKey.api_formats || []
    ),
    auth_config_text: '',  // auth_config 不返回给前端，编辑时需要重新输入
    api_formats: props.editingKey.api_formats?.length > 0
      ? sanitizeApiFormats(
        props.editingKey.api_formats,
        normalizeFormAuthType(props.editingKey.auth_type)
      )
      : [],  // 编辑模式下保持原有选择，不默认全选
    rate_multipliers: { ...(props.editingKey.rate_multipliers || {}) },
    internal_priority: props.editingKey.internal_priority ?? 10,
    // 保留原始的 null/undefined 状态，null 表示自适应模式
    rpm_limit: props.editingKey.rpm_limit ?? undefined,
    concurrent_limit: props.editingKey.concurrent_limit ?? undefined,
    cache_ttl_minutes: props.editingKey.cache_ttl_minutes ?? 5,
    max_probe_interval_minutes: props.editingKey.max_probe_interval_minutes ?? 32,
    note: props.editingKey.note || '',
    is_active: props.editingKey.is_active,
    capabilities: { ...(props.editingKey.capabilities || {}) },
    auto_fetch_models: props.editingKey.auto_fetch_models ?? false,
    model_include_patterns_text: (props.editingKey.model_include_patterns || []).join(', '),
    model_exclude_patterns_text: (props.editingKey.model_exclude_patterns || []).join(', ')
  }
}

// 使用 useFormDialog 统一处理对话框逻辑
const { isEditMode, handleDialogUpdate, handleCancel } = useFormDialog({
  isOpen: () => props.open,
  entity: () => props.editingKey,
  isLoading: saving,
  onClose: () => emit('close'),
  loadData: loadKeyData,
  resetForm,
})

function createFieldNonce(): string {
  return Math.random().toString(36).slice(2, 10)
}

// 将逗号分隔的文本解析为数组（去空、去重）
// 返回空数组而非 undefined，以便后端能正确清除已有规则
function parsePatternText(text: string): string[] {
  if (!text.trim()) return []
  const patterns = text
    .split(',')
    .map(s => s.trim())
    .filter(s => s.length > 0)
  return [...new Set(patterns)]
}

// 解析 Service Account JSON 文本
function parseAuthConfig(): Record<string, unknown> | null {
  if (form.value.auth_type !== 'service_account') return null
  const text = form.value.auth_config_text.trim()
  if (!text) return null
  try {
    return JSON.parse(text)
  } catch {
    return null
  }
}

function handleServiceAccountImportError(payload: { message: string, title?: string }) {
  showError(payload.message, payload.title || '错误')
}

async function handleSave() {
  // 必须有 providerId
  if (!props.providerId) {
    showError('无法保存：缺少提供商信息', '错误')
    return
  }

  // 验证认证信息
  if (form.value.auth_type === 'service_account') {
    if (!props.editingKey && !form.value.auth_config_text.trim()) {
      showError('请输入 Service Account JSON', '验证失败')
      return
    }
    // 验证 JSON 格式
    if (form.value.auth_config_text.trim()) {
      const parsed = parseAuthConfig()
      if (!parsed) {
        showError('Service Account JSON 格式无效', '验证失败')
        return
      }
      // 验证必要字段
      if (!parsed.client_email || !parsed.private_key || !parsed.project_id) {
        showError('Service Account JSON 缺少必要字段 (client_email, private_key, project_id)', '验证失败')
        return
      }
    }
  }

  form.value.api_formats = sanitizeApiFormats(form.value.api_formats)
  form.value.allow_auth_channel_mismatch_formats = sanitizeAllowAuthChannelMismatchFormats(
    form.value.allow_auth_channel_mismatch_formats
  )

  // 验证至少选择一个 API 格式
  if (form.value.api_formats.length === 0) {
    showError('请至少选择一个 API 格式', '验证失败')
    return
  }

  // 过滤出有效的能力配置（只包含值为 true 的）
  const activeCapabilities: Record<string, boolean> = {}
  for (const [key, value] of Object.entries(form.value.capabilities)) {
    if (value) {
      activeCapabilities[key] = true
    }
  }
  const capabilitiesData = Object.keys(activeCapabilities).length > 0 ? activeCapabilities : null

  saving.value = true
  try {
    // 准备 rate_multipliers 数据：只保留已选中格式的倍率配置
    const filteredMultipliers: Record<string, number> = {}
    for (const format of form.value.api_formats) {
      if (form.value.rate_multipliers[format] !== undefined) {
        filteredMultipliers[format] = form.value.rate_multipliers[format]
      }
    }
    const rateMultipliersData = Object.keys(filteredMultipliers).length > 0
      ? filteredMultipliers
      : null

    // 准备认证相关数据
    const authConfig = parseAuthConfig()
    const authTypeByFormat = buildAuthTypeByFormatPayload()
    const allowAuthChannelMismatchFormats = buildAllowAuthChannelMismatchFormatsPayload()

    if (props.editingKey) {
      const shouldClearAllowedModels = !!props.editingKey.auto_fetch_models && !form.value.auto_fetch_models
      // 更新模式
      // 注意：rpm_limit 使用 null 表示自适应模式
      // undefined 表示"保持原值不变"（会在 JSON 序列化时被忽略）
      const updateData: EndpointAPIKeyUpdate = {
        api_formats: form.value.api_formats,
        name: form.value.name,
        auth_type: form.value.auth_type,
        auth_type_by_format: authTypeByFormat,
        allow_auth_channel_mismatch_formats: allowAuthChannelMismatchFormats,
        rate_multipliers: rateMultipliersData,
        internal_priority: form.value.internal_priority,
        rpm_limit: form.value.rpm_limit,
        concurrent_limit: form.value.concurrent_limit,
        cache_ttl_minutes: form.value.cache_ttl_minutes,
        max_probe_interval_minutes: form.value.max_probe_interval_minutes,
        note: form.value.note,
        is_active: form.value.is_active,
        capabilities: capabilitiesData,
        allowed_models: shouldClearAllowedModels ? null : undefined,
        auto_fetch_models: form.value.auto_fetch_models,
        model_include_patterns: parsePatternText(form.value.model_include_patterns_text),
        model_exclude_patterns: parsePatternText(form.value.model_exclude_patterns_text)
      }

      // 根据认证类型设置对应字段
      if (isRawSecretAuthType(form.value.auth_type) && form.value.api_key.trim()) {
        updateData.api_key = form.value.api_key
      }
      if (form.value.auth_type === 'service_account' && authConfig) {
        updateData.auth_config = authConfig
      }

      await updateProviderKey(props.editingKey.id, updateData)
      success('密钥已更新', '成功')
    } else {
      // 新增模式
      await addProviderKey(props.providerId, {
        api_formats: form.value.api_formats,
        api_key: form.value.api_key,
        auth_type: form.value.auth_type,
        auth_type_by_format: authTypeByFormat,
        allow_auth_channel_mismatch_formats: allowAuthChannelMismatchFormats,
        auth_config: authConfig || undefined,
        name: form.value.name,
        rate_multipliers: rateMultipliersData,
        internal_priority: form.value.internal_priority,
        rpm_limit: form.value.rpm_limit,
        concurrent_limit: form.value.concurrent_limit,
        cache_ttl_minutes: form.value.cache_ttl_minutes,
        max_probe_interval_minutes: form.value.max_probe_interval_minutes,
        note: form.value.note,
        capabilities: capabilitiesData || undefined,
        auto_fetch_models: form.value.auto_fetch_models,
        model_include_patterns: parsePatternText(form.value.model_include_patterns_text),
        model_exclude_patterns: parsePatternText(form.value.model_exclude_patterns_text)
      })

      success('密钥已添加', '成功')
      // 添加模式：不关闭对话框，只清除名称和密钥以便继续添加
      emit('saved')
      clearForNextAdd()
      return
    }

    emit('saved')
    emit('close')
  } catch (err: unknown) {
    const errorMessage = parseApiError(err, '保存密钥失败')
    showError(errorMessage, '错误')
  } finally {
    saving.value = false
  }
}
</script>
