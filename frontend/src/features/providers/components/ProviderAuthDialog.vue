<template>
  <Dialog
    :open="open"
    title="用户认证"
    description="配置提供商的用户认证信息，用于余额查询、签到等操作"
    :icon="KeyRound"
    size="md"
    @update:open="$emit('update:open', $event)"
  >
    <form
      :name="`provider-auth-${Date.now()}`"
      autocomplete="off"
      @submit.prevent
    >
      <!-- 加载状态 -->
      <div
        v-if="isLoadingConfig"
        class="flex items-center justify-center py-8"
      >
        <div class="text-sm text-muted-foreground">
          加载配置中...
        </div>
      </div>
      <div
        v-else
        class="space-y-4"
      >
        <!-- 认证模板 + 认证方式（并排） -->
        <div class="flex gap-3">
          <div
            class="space-y-2"
            :style="{ flex: currentAuthTypes.length > 1 ? 1 : 'auto', width: currentAuthTypes.length > 1 ? undefined : '100%' }"
          >
            <Label>认证模板</Label>
            <Select
              v-model="selectedArchitectureId"
              @update:model-value="handleArchitectureChange"
            >
              <SelectTrigger>
                <SelectValue placeholder="选择认证模板" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem
                  v-for="arch in architectures"
                  :key="arch.architecture_id"
                  :value="arch.architecture_id"
                >
                  {{ arch.display_name }}
                </SelectItem>
              </SelectContent>
            </Select>
          </div>

          <div
            v-if="currentAuthTypes.length > 1"
            class="space-y-2"
            style="flex: 1"
          >
            <Label>认证方式</Label>
            <Select
              v-model="selectedAuthType"
              @update:model-value="handleAuthTypeChange"
            >
              <SelectTrigger>
                <SelectValue placeholder="选择认证方式" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem
                  v-for="authType in currentAuthTypes"
                  :key="authType.type"
                  :value="authType.type"
                >
                  {{ authType.display_name }}
                </SelectItem>
              </SelectContent>
            </Select>
          </div>
        </div>

        <!-- 动态表单字段 -->
        <template v-if="currentSchema">
          <template
            v-for="(group, groupIndex) in fieldGroups"
            :key="groupIndex"
          >
            <!-- 可折叠的分组（代理配置 - 代理节点选择） -->
            <div
              v-if="group.collapsible && group.hasToggle && group.toggleKey"
              class="space-y-2"
            >
              <!-- 标题栏：标题在左，开关在右（在卡片外） -->
              <div class="flex items-center justify-between">
                <span class="text-sm font-medium text-foreground">{{ group.title }}</span>
                <div class="flex items-center gap-2">
                  <span class="text-xs text-muted-foreground">启用代理</span>
                  <Switch
                    :model-value="formData[group.toggleKey] || false"
                    @update:model-value="handleProxyToggle(group.toggleKey, $event)"
                  />
                </div>
              </div>

              <!-- 展开内容（卡片）- 代理节点选择 -->
              <div
                v-if="formData[group.toggleKey]"
                class="rounded-lg border border-border bg-muted/30 px-4 py-3"
              >
                <ProxyNodeSelect
                  ref="proxyNodeSelectRef"
                  :model-value="formData.proxy_node_id || ''"
                  trigger-class="h-8"
                  @update:model-value="(v: string) => { formData.proxy_node_id = v; handleFieldChange('proxy_node_id', v) }"
                />
              </div>
            </div>

            <!-- 普通分组（非折叠） -->
            <template v-else>
              <!-- 分组标题 -->
              <div
                v-if="group.title"
                class="pt-2 text-sm font-medium text-muted-foreground"
              >
                {{ group.title }}
              </div>

              <!-- inline 布局：字段横向排列 -->
              <div
                v-if="group.layout === 'inline'"
                class="flex gap-3"
              >
                <div
                  v-for="field in group.fields"
                  :key="field.key"
                  class="space-y-2"
                  :style="{ flex: field.flex || 1 }"
                >
                  <Label>
                    {{ field.label }}
                    <span
                      v-if="field.required"
                      class="text-muted-foreground/70"
                    >*</span>
                  </Label>

                  <!-- 文本输入 -->
                  <Input
                    v-if="field.type === 'text'"
                    v-model="formData[field.key]"
                    :placeholder="field.sensitive ? (sensitivePlaceholders[field.key] || field.placeholder) : field.placeholder"
                    :masked="field.sensitive"
                    disable-autofill
                    @update:model-value="handleFieldChange(field.key, $event)"
                  />

                  <!-- 密码/敏感输入 -->
                  <Input
                    v-else-if="field.type === 'password'"
                    v-model="formData[field.key]"
                    :placeholder="sensitivePlaceholders[field.key] || field.placeholder"
                    masked
                    @update:model-value="handleFieldChange(field.key, $event)"
                  />
                </div>
              </div>

              <!-- vertical 布局（默认）：字段垂直排列 -->
              <template v-else>
                <div
                  v-for="field in group.fields"
                  :key="field.key"
                  class="space-y-2"
                >
                  <Label>
                    {{ field.label }}
                    <span
                      v-if="field.required"
                      class="text-muted-foreground/70"
                    >*</span>
                  </Label>

                  <!-- 文本输入 -->
                  <Input
                    v-if="field.type === 'text'"
                    v-model="formData[field.key]"
                    :placeholder="field.sensitive ? (sensitivePlaceholders[field.key] || field.placeholder) : field.placeholder"
                    :masked="field.sensitive"
                    disable-autofill
                    @update:model-value="handleFieldChange(field.key, $event)"
                  />

                  <!-- 密码/敏感输入 -->
                  <Input
                    v-else-if="field.type === 'password'"
                    v-model="formData[field.key]"
                    :placeholder="sensitivePlaceholders[field.key] || field.placeholder"
                    masked
                    @update:model-value="handleFieldChange(field.key, $event)"
                  />

                  <!-- 下拉选择 -->
                  <Select
                    v-else-if="field.type === 'select'"
                    v-model="formData[field.key]"
                    @update:model-value="handleFieldChange(field.key, $event)"
                  >
                    <SelectTrigger>
                      <SelectValue :placeholder="field.placeholder || '请选择'" />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem
                        v-for="option in field.options"
                        :key="option.value"
                        :value="option.value"
                      >
                        {{ option.label }}
                      </SelectItem>
                    </SelectContent>
                  </Select>

                  <!-- 多行文本 -->
                  <Textarea
                    v-else-if="field.type === 'textarea'"
                    v-model="formData[field.key]"
                    :placeholder="field.placeholder"
                    rows="3"
                    @update:model-value="handleFieldChange(field.key, $event)"
                  />

                  <!-- 帮助文本 -->
                  <p
                    v-if="field.helpText"
                    class="text-xs text-muted-foreground"
                  >
                    {{ field.helpText }}
                  </p>
                </div>
              </template>
            </template>
          </template>
        </template>

        <div class="rounded-lg border border-border bg-muted/20 px-4 py-3">
          <div class="flex items-center justify-between gap-4">
            <div>
              <Label class="text-sm font-medium">
                额度提醒
              </Label>
              <p class="mt-1 text-xs text-muted-foreground">
                余额低于阈值时通过通知服务发送提醒
              </p>
            </div>
            <Switch v-model="quotaAlert.enabled" />
          </div>

          <div
            v-if="quotaAlert.enabled"
            class="mt-4 grid grid-cols-1 md:grid-cols-2 gap-3"
          >
            <div class="space-y-2">
              <Label>提醒阈值</Label>
              <Input
                v-model.number="quotaAlert.threshold_amount"
                type="number"
                min="0"
                step="0.0001"
                placeholder="0"
              />
            </div>
            <div class="space-y-2">
              <Label>获取频率（秒）</Label>
              <Input
                v-model.number="quotaAlert.fetch_interval_seconds"
                type="number"
                min="30"
                max="86400"
                step="1"
                placeholder="30"
              />
            </div>
          </div>
        </div>
      </div>
    </form>

    <template #footer>
      <div class="flex w-full items-center justify-between">
        <!-- 左侧：清除按钮（仅在已有配置时显示） -->
        <div>
          <Button
            v-if="hasExistingConfig"
            variant="destructive"
            :disabled="isClearing"
            @click="handleClear"
          >
            {{ isClearing ? '清除中...' : '清除' }}
          </Button>
        </div>
        <!-- 右侧：验证、保存、取消按钮 -->
        <div class="flex gap-2">
          <Button
            variant="outline"
            :disabled="isVerifying || !canVerify"
            @click="handleVerify"
          >
            {{ isVerifying ? '验证中...' : '验证' }}
          </Button>
          <Button
            :disabled="isSaving || !canSave"
            @click="handleSave"
          >
            {{ isSaving ? '保存中...' : '保存' }}
          </Button>
          <Button
            variant="outline"
            @click="$emit('update:open', false)"
          >
            取消
          </Button>
        </div>
      </div>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { ref, computed, watch, nextTick } from 'vue'
import { KeyRound } from 'lucide-vue-next'
import {
  Dialog,
  Button,
  Input,
  Label,
  Textarea,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Switch,
} from '@/components/ui'
import {
  getArchitectures,
  saveProviderOpsConfig,
  verifyProviderAuth,
  getProviderOpsConfig,
  deleteProviderOpsConfig,
  type ArchitectureInfo,
  type QuotaAlertConfig,
} from '@/api/providerOps'
import { parseApiError } from '@/utils/errorParser'
import { useToast } from '@/composables/useToast'
import { useConfirm } from '@/composables/useConfirm'
import type { AuthTemplateFieldGroup } from '../auth-templates/types'
import {
  schemaToFieldGroups,
  buildRequestFromSchema,
  parseConfigFromSchema,
  validateFromSchema,
  formatQuotaFromSchema,
  handleSchemaFieldChange,
  type CredentialsSchema,
} from '../auth-templates/schema-utils'
import ProxyNodeSelect from './ProxyNodeSelect.vue'
import { useProxyNodesStore } from '@/stores/proxy-nodes'

const props = defineProps<{
  open: boolean
  providerId: string
  providerWebsite?: string
  currentConfig?: Record<string, unknown> | null
}>()

const emit = defineEmits<{
  (e: 'update:open', value: boolean): void
  (e: 'saved'): void
}>()

// 敏感字段检测：根据 schema 动态判断
function isSensitiveField(key: string): boolean {
  if (!currentSchema.value) return false
  const prop = currentSchema.value.properties[key]
  return prop?.['x-sensitive'] === true
}

const { success: showSuccess, error: showError } = useToast()
const { confirmDanger } = useConfirm()
const proxyNodeSelectRef = ref<InstanceType<typeof ProxyNodeSelect> | null>(null)
const proxyNodesStore = useProxyNodesStore()

/** 启用代理时加载节点列表 */
function handleProxyToggle(toggleKey: string, value: boolean) {
  formData.value[toggleKey] = value
  if (value) {
    proxyNodesStore.ensureLoaded()
  }
}

// State
const isSaving = ref(false)
const isVerifying = ref(false)
const isLoadingConfig = ref(false)
const isClearing = ref(false)
const verifyStatus = ref<'success' | 'error' | null>(null)
const formChanged = ref(false)

// 敏感字段的 placeholder（存储脱敏后的已保存值）
const sensitivePlaceholders = ref<Record<string, string>>({})
// 是否有已保存的配置（编辑模式）
const hasExistingConfig = ref(false)

// 架构列表（从后端获取）
const architectures = ref<ArchitectureInfo[]>([])
const architecturesLoaded = ref(false)

// 当前选择
const selectedArchitectureId = ref('new_api')
const selectedAuthType = ref('')
const formData = ref<Record<string, unknown>>({})
const quotaAlert = ref<QuotaAlertConfig>({
  enabled: false,
  threshold_amount: 0,
  fetch_interval_seconds: 30,
})
const savedQuotaAlertSignature = ref(quotaAlertSignature(quotaAlert.value))

// 当前架构支持的认证方式
const currentAuthTypes = computed(() => {
  const arch = architectures.value.find((a) => a.architecture_id === selectedArchitectureId.value)
  return arch?.supported_auth_types ?? []
})

// 当前架构的 schema（优先从选中的 auth_type 获取）
const currentSchema = computed<CredentialsSchema | null>(() => {
  const arch = architectures.value.find((a) => a.architecture_id === selectedArchitectureId.value)
  if (!arch) return null

  // 如果有选中的 auth_type 且架构有多个 connector，从对应的 auth_type 获取 schema
  if (selectedAuthType.value && arch.supported_auth_types.length > 1) {
    const authType = arch.supported_auth_types.find((t) => t.type === selectedAuthType.value)
    if (authType?.credentials_schema) {
      return authType.credentials_schema as CredentialsSchema
    }
  }

  return (arch?.credentials_schema as CredentialsSchema) ?? null
})

// 表单是否可以验证（必填字段已填写）
const canVerify = computed(() => {
  const schema = currentSchema.value
  if (!schema) return false

  // 编辑模式下，敏感字段可以为空（使用已保存的值）
  let dataToValidate = formData.value
  if (hasExistingConfig.value) {
    dataToValidate = { ...formData.value }
    for (const key of Object.keys(schema.properties)) {
      if (isSensitiveField(key) && !dataToValidate[key] && sensitivePlaceholders.value[key]) {
        dataToValidate[key] = 'placeholder'
      }
    }
  }
  const error = validateFromSchema(schema, dataToValidate)
  if (error) return false

  const effectiveBaseUrl = formData.value.base_url || props.providerWebsite
  return !!effectiveBaseUrl
})

// 保存按钮是否可用：验证成功且表单未变动
const quotaAlertChanged = computed(() => {
  return quotaAlertSignature(quotaAlert.value) !== savedQuotaAlertSignature.value
})

const canSave = computed(() => {
  return (
    (verifyStatus.value === 'success' && !formChanged.value)
    || (hasExistingConfig.value && quotaAlertChanged.value && !formChanged.value)
  )
})

// 字段分组
const fieldGroups = computed<AuthTemplateFieldGroup[]>(() => {
  if (!currentSchema.value) return []
  return schemaToFieldGroups(currentSchema.value, props.providerWebsite)
})

// Methods
function handleArchitectureChange() {
  // 切换架构时，默认选中第一个认证方式
  const authTypes = currentAuthTypes.value
  selectedAuthType.value = authTypes.length > 0 ? authTypes[0].type : ''
  resetFormData()
  verifyStatus.value = null
  formChanged.value = true
}

function handleAuthTypeChange() {
  resetFormData()
  verifyStatus.value = null
  formChanged.value = true
}

function handleFieldChange(fieldKey: string, value: unknown) {
  formChanged.value = true

  // 执行 schema 定义的字段钩子
  const schema = currentSchema.value
  if (schema) {
    handleSchemaFieldChange(schema, fieldKey, value, formData.value)
  }
}

// 监听 formData 变化，验证成功后的修改需要重新验证
watch(
  formData,
  () => {
    if (verifyStatus.value === 'success') {
      formChanged.value = true
    }
  },
  { deep: true }
)

function resetFormData() {
  const schema = currentSchema.value
  if (!schema) {
    formData.value = {}
    return
  }

  // 初始化表单数据
  const data: Record<string, unknown> = {}
  for (const [key, prop] of Object.entries(schema.properties)) {
    data[key] = (prop as Record<string, unknown>)['x-default-value'] ?? ''
  }
  // 代理相关默认值
  data.proxy_enabled = false
  data.proxy_node_id = ''

  formData.value = data
}

function formatQuota(quota: number): string {
  const schema = currentSchema.value
  if (schema) {
    return formatQuotaFromSchema(schema, quota)
  }
  return quota.toLocaleString()
}

function finiteNumber(value: unknown): number | null {
  const numberValue = Number(value)
  return Number.isFinite(numberValue) ? numberValue : null
}

async function handleVerify() {
  const schema = currentSchema.value
  if (!schema) return

  // 验证表单（编辑模式下敏感字段可以为空）
  let dataToValidate = formData.value
  if (hasExistingConfig.value) {
    dataToValidate = { ...formData.value }
    for (const key of Object.keys(schema.properties)) {
      if (isSensitiveField(key) && !dataToValidate[key] && sensitivePlaceholders.value[key]) {
        dataToValidate[key] = 'placeholder'
      }
    }
  }
  const error = validateFromSchema(schema, dataToValidate)
  if (error) {
    showError(error)
    return
  }

  const effectiveBaseUrl = formData.value.base_url || props.providerWebsite
  if (!effectiveBaseUrl) {
    showError('请填写 API 地址')
    return
  }

  isVerifying.value = true

  try {
    const request = buildRequestFromSchema(
      schema,
      selectedArchitectureId.value,
      formData.value,
      props.providerWebsite,
    )
    const verifyRequest = {
      ...request,
      base_url: request.base_url || effectiveBaseUrl,
    }
    const result = await verifyProviderAuth(props.providerId, verifyRequest)

    if (result.success) {
      const username = result.data?.username?.trim() || result.data?.display_name?.trim()
      const quota = result.data?.quota

      if (!username || quota === undefined || quota === null) {
        verifyStatus.value = 'error'
        const missing: string[] = []
        if (!username) missing.push('用户信息')
        if (quota === undefined || quota === null) missing.push('余额')
        showError(`验证响应缺少: ${missing.join('、')}`)
      } else {
        verifyStatus.value = 'success'
        formChanged.value = false

        // Token Rotation: 验证过程中 refresh_token 可能已被轮换，用新值更新表单
        // 必须在 formChanged=false 之后执行，避免 watch 将 formChanged 重新设为 true
        if (result.updated_credentials) {
          for (const [key, value] of Object.entries(result.updated_credentials)) {
            formData.value[key] = value
          }
          // 凭据更新不算用户修改，保持可保存状态
          nextTick(() => {
            formChanged.value = false
          })
        }

        const displayName = result.data?.display_name || result.data?.username
        const extra = result.data?.extra
        let balanceText = `余额: ${formatQuota(quota)}`
        const extraBalance = finiteNumber(extra?.balance)
        const extraPoints = finiteNumber(extra?.points)
        if (extraBalance !== null && extraPoints !== null) {
          balanceText = `余额: ${formatQuota(extraBalance)} | 积分: ${formatQuota(extraPoints)}`
        }
        showSuccess(`用户: ${displayName} | ${balanceText}`, '验证成功')
      }
    } else {
      verifyStatus.value = 'error'

      // Token Rotation: 即使验证失败，refresh_token 可能已被轮换（旧 token 已失效）
      if (result.updated_credentials) {
        for (const [key, value] of Object.entries(result.updated_credentials)) {
          formData.value[key] = value
        }
      }

      showError(result.message || '验证失败')
    }
  } catch (error: unknown) {
    verifyStatus.value = 'error'
    showError(parseApiError(error, '验证失败'))
  } finally {
    isVerifying.value = false
  }
}

async function handleSave() {
  const schema = currentSchema.value
  if (!schema) return

  // 验证表单
  let dataToValidate = formData.value
  if (hasExistingConfig.value) {
    dataToValidate = { ...formData.value }
    for (const key of Object.keys(schema.properties)) {
      if (isSensitiveField(key) && !dataToValidate[key] && sensitivePlaceholders.value[key]) {
        dataToValidate[key] = 'placeholder'
      }
    }
  }
  const error = validateFromSchema(schema, dataToValidate)
  if (error) {
    showError(error)
    return
  }

  const effectiveBaseUrl = formData.value.base_url || props.providerWebsite
  if (!effectiveBaseUrl) {
    showError('请填写 API 地址')
    return
  }

  isSaving.value = true
  try {
    const request = buildRequestFromSchema(
      schema,
      selectedArchitectureId.value,
      formData.value,
      props.providerWebsite,
    )
    request.quota_alert = normalizedQuotaAlert()
    const result = await saveProviderOpsConfig(props.providerId, request)
    if (result.success) {
      savedQuotaAlertSignature.value = quotaAlertSignature(quotaAlert.value)
      showSuccess(result.message || '配置已保存', '保存成功')
      emit('saved')
      emit('update:open', false)
    } else {
      showError(result.message || '保存失败')
    }
  } catch (error: unknown) {
    showError(parseApiError(error, '保存失败'), '保存失败')
  } finally {
    isSaving.value = false
  }
}

async function handleClear() {
  if (!props.providerId) return

  const confirmed = await confirmDanger(
    '确定要清除该提供商的认证配置吗？清除后将无法进行余额查询、签到等操作。',
    '清除认证',
    '清除'
  )
  if (!confirmed) return

  isClearing.value = true
  try {
    const result = await deleteProviderOpsConfig(props.providerId)
    if (result.success) {
      showSuccess(result.message || '认证信息已清除', '清除成功')
      hasExistingConfig.value = false
      sensitivePlaceholders.value = {}
      verifyStatus.value = null
      formChanged.value = false
      selectedArchitectureId.value = 'new_api'
      selectedAuthType.value = ''
      loadQuotaAlert(null)
      resetFormData()
      emit('saved')
      emit('update:open', false)
    } else {
      showError(result.message || '清除失败')
    }
  } catch (error: unknown) {
    showError(parseApiError(error, '清除失败'), '清除失败')
  } finally {
    isClearing.value = false
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function stringOrDefault(value: unknown, fallback: string): string {
  return typeof value === 'string' && value.trim() ? value : fallback
}

function loadFromConfig(config: Record<string, unknown>) {
  const connector = isRecord(config.connector) ? config.connector : null
  if (!connector) return

  hasExistingConfig.value = true

  // 根据已保存的 architecture_id 选择对应架构
  const architectureId = stringOrDefault(config.architecture_id, 'new_api')
  const archExists = architectures.value.some((a) => a.architecture_id === architectureId)
  selectedArchitectureId.value = archExists ? architectureId : 'new_api'

  // 从已保存的 connector auth_type 恢复认证方式选择
  const savedAuthType = stringOrDefault(connector.auth_type, '')
  const authTypes = currentAuthTypes.value
  if (savedAuthType && authTypes.some((t) => t.type === savedAuthType)) {
    selectedAuthType.value = savedAuthType
  } else {
    selectedAuthType.value = authTypes.length > 0 ? authTypes[0].type : ''
  }

  const schema = currentSchema.value
  if (schema) {
    const parsedData = parseConfigFromSchema(schema, {
      ...config,
      connector,
    })

    // 敏感字段：脱敏值放到 placeholder，表单值设为空
    sensitivePlaceholders.value = {}
    for (const key of Object.keys(schema.properties)) {
      if (isSensitiveField(key) && parsedData[key]) {
        sensitivePlaceholders.value[key] = `${parsedData[key]}`
        parsedData[key] = ''
      }
    }

    formData.value = parsedData
  }
  loadQuotaAlert(config.quota_alert)
}

function defaultQuotaAlert(): QuotaAlertConfig {
  return {
    enabled: false,
    threshold_amount: 0,
    fetch_interval_seconds: 30,
  }
}

function normalizeQuotaAlert(value: unknown): QuotaAlertConfig {
  if (!value || typeof value !== 'object') return defaultQuotaAlert()
  const item = value as Record<string, unknown>
  const threshold = Number(item.threshold_amount)
  const interval = Number(item.fetch_interval_seconds)
  return {
    enabled: item.enabled === true,
    threshold_amount: Number.isFinite(threshold) && threshold >= 0 ? threshold : 0,
    fetch_interval_seconds: Number.isFinite(interval) && interval >= 30 ? Math.min(Math.floor(interval), 86400) : 30,
  }
}

function normalizedQuotaAlert(): QuotaAlertConfig {
  return normalizeQuotaAlert(quotaAlert.value)
}

function quotaAlertSignature(value: QuotaAlertConfig): string {
  const normalized = normalizeQuotaAlert(value)
  return JSON.stringify([
    normalized.enabled,
    normalized.threshold_amount,
    normalized.fetch_interval_seconds,
  ])
}

function loadQuotaAlert(value: unknown) {
  const normalized = normalizeQuotaAlert(value)
  quotaAlert.value = normalized
  savedQuotaAlertSignature.value = quotaAlertSignature(normalized)
}

/** 确保架构列表已加载 */
async function ensureArchitecturesLoaded(): Promise<void> {
  if (architecturesLoaded.value) return
  try {
    architectures.value = await getArchitectures()
    architecturesLoaded.value = true
  } catch {
    architectures.value = []
  }
}

// 打开对话框时初始化
watch(
  () => props.open,
  async (newVal) => {
    if (newVal) {
      verifyStatus.value = null
      formChanged.value = false

      // 确保架构列表已加载
      await ensureArchitecturesLoaded()

      // 如果传入了 currentConfig，直接使用
      if (props.currentConfig?.connector) {
        loadFromConfig(props.currentConfig)
        return
      }

      // 否则尝试从后端加载现有配置
      if (props.providerId) {
        isLoadingConfig.value = true
        try {
          const config = await getProviderOpsConfig(props.providerId)
          if (config.is_configured && config.architecture_id) {
            const configData = {
              architecture_id: config.architecture_id,
              base_url: config.base_url,
              connector: config.connector,
              quota_alert: config.quota_alert,
            }
            loadFromConfig(configData)
          } else {
            hasExistingConfig.value = false
            sensitivePlaceholders.value = {}
            loadQuotaAlert(null)
            selectedArchitectureId.value = 'new_api'
            selectedAuthType.value = ''
            resetFormData()
          }
        } catch {
          hasExistingConfig.value = false
          sensitivePlaceholders.value = {}
          loadQuotaAlert(null)
          selectedArchitectureId.value = 'new_api'
          selectedAuthType.value = ''
          resetFormData()
        } finally {
          isLoadingConfig.value = false
        }
      } else {
        hasExistingConfig.value = false
        sensitivePlaceholders.value = {}
        loadQuotaAlert(null)
        selectedArchitectureId.value = 'new_api'
        selectedAuthType.value = ''
        resetFormData()
      }
    }
  }
)
</script>
