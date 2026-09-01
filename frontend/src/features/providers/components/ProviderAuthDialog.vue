<template>
  <Dialog
    :open="open"
    title="用户认证"
    description="配置提供商的用户认证信息，用于余额查询、签到等操作"
    :icon="KeyRound"
    :size="selectedArchitectureId === 'sub2api' ? '2xl' : 'md'"
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

        <div
          v-if="selectedArchitectureId === 'sub2api'"
          class="rounded-lg border border-border bg-muted/20 px-4 py-3"
        >
          <div class="flex items-center justify-between gap-4">
            <div>
              <Label class="text-sm font-medium">
                远程套餐额度同步
              </Label>
              <p class="mt-1 text-xs text-muted-foreground">
                一个 Provider 同步一个 Sub2API 订阅套餐；余额分组不使用此功能
              </p>
            </div>
            <Switch v-model="remoteQuota.enabled" />
          </div>

          <div
            v-if="remoteQuota.enabled"
            class="mt-4 space-y-4"
          >
            <div class="space-y-2">
              <div class="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
                <div>
                  <Label>订阅套餐</Label>
                  <p class="mt-1 text-xs text-muted-foreground">
                    读取当前账号的有效订阅，并选择这个 Provider 使用的套餐
                  </p>
                </div>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  class="w-full shrink-0 sm:w-auto"
                  :disabled="isLoadingSub2ApiGroups || !canVerify"
                  @click="handleDiscoverSub2ApiGroups"
                >
                  <RefreshCw
                    class="mr-1 h-3.5 w-3.5"
                    :class="{ 'animate-spin': isLoadingSub2ApiGroups }"
                  />
                  {{ isLoadingSub2ApiGroups ? '读取中...' : '读取套餐列表' }}
                </Button>
              </div>
              <Select v-model="selectedSub2ApiGroupId">
                <SelectTrigger class="h-auto min-h-11 py-3">
                  <SelectValue placeholder="请先读取套餐列表" />
                </SelectTrigger>
                <SelectContent class="w-[var(--radix-select-trigger-width)] max-w-[calc(100vw-2rem)]">
                  <SelectItem
                    v-if="selectedSub2ApiGroupMissing && remoteQuota.group_id"
                    :value="remoteQuota.group_id"
                  >
                    已保存套餐（当前列表中不可用）
                  </SelectItem>
                  <SelectItem
                    v-for="group in sub2ApiGroups"
                    :key="group.group_id"
                    :value="group.group_id"
                    :text-value="formatSub2ApiGroupOption(group)"
                    class="py-2.5"
                  >
                    {{ formatSub2ApiGroupOption(group) }}
                  </SelectItem>
                </SelectContent>
              </Select>
            </div>

            <div
              v-if="selectedSub2ApiGroup"
              class="rounded-lg border border-border/70 bg-background/70 p-3"
            >
              <div class="flex flex-wrap items-center justify-between gap-2">
                <div>
                  <p class="font-medium text-foreground">
                    {{ selectedSub2ApiGroup.group_name || '未命名套餐' }}
                  </p>
                </div>
                <span class="rounded-full bg-primary/10 px-2.5 py-1 text-xs font-medium text-primary">
                  {{ localSyncWindowText(selectedSub2ApiGroup.local_sync_window) }}
                </span>
              </div>
              <div class="mt-3 grid grid-cols-1 gap-2 sm:grid-cols-3">
                <div
                  v-for="window in sub2ApiQuotaWindows"
                  :key="window.key"
                  class="rounded-md border border-border/60 bg-muted/30 px-3 py-2"
                >
                  <p class="text-xs text-muted-foreground">
                    {{ window.label }}额度
                  </p>
                  <p class="mt-1 text-sm font-medium tabular-nums">
                    {{ formatSub2ApiWindow(selectedSub2ApiGroup, window.key) }}
                  </p>
                </div>
              </div>
              <p class="mt-3 text-xs text-muted-foreground">
                Aether 按“日 → 周 → 月”的顺序采用第一个有限额度作为调度上限；其他额度仅用于展示。
              </p>
            </div>
            <div
              v-else-if="remoteQuota.group_id"
              class="rounded-lg border border-dashed border-border bg-muted/20 px-4 py-3"
            >
              <p class="text-sm font-medium">
                已保存套餐
              </p>
              <p class="mt-1 text-xs text-muted-foreground">
                套餐额度不会在打开弹窗时自动联网读取。点击“读取套餐列表”即可查看最新的日、周、月额度。
              </p>
            </div>

            <div class="space-y-2">
              <Label>套餐进度端点</Label>
              <Input
                v-model="remoteQuota.progress_endpoint"
                placeholder="/api/v1/subscriptions/progress"
              />
              <p
                v-if="!remoteQuotaProgressEndpointValid"
                class="text-xs text-destructive"
              >
                必须填写同源、以 / 开头且不含 //、# 或反斜杠的相对路径
              </p>
            </div>

            <div class="flex flex-col gap-2 rounded-md border border-border/70 bg-background/70 p-3 sm:flex-row sm:items-center sm:justify-between">
              <p class="text-xs text-muted-foreground">
                使用已保存配置读取远程套餐，并立即校准 Provider 本地调度额度。
              </p>
              <Button
                type="button"
                variant="outline"
                size="sm"
                class="w-full shrink-0 sm:w-auto"
                :disabled="isSyncingRemoteQuota || remoteQuotaChanged || !remoteQuota.group_id"
                @click="handleSyncRemoteQuota"
              >
                <RefreshCw
                  class="mr-1 h-3.5 w-3.5"
                  :class="{ 'animate-spin': isSyncingRemoteQuota }"
                />
                {{ isSyncingRemoteQuota ? '同步中...' : '立即同步本地额度' }}
              </Button>
            </div>
            <p
              v-if="remoteQuotaChanged"
              class="text-xs text-muted-foreground"
            >
              请先保存远程额度配置，再执行同步。
            </p>
          </div>
          <p
            v-if="remoteQuota.enabled"
            class="mt-3 text-xs text-muted-foreground"
          >
            所选套餐不存在或已失效时会将本地额度标记为耗尽；网络、认证或解析失败不会修改本地额度。单个额度为 0 表示该周期不限额，日、周、月均不限时才视为无限套餐。
          </p>
        </div>

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
import { KeyRound, RefreshCw } from 'lucide-vue-next'
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
  discoverSub2ApiGroups,
  refreshBalance,
  getProviderOpsConfig,
  deleteProviderOpsConfig,
  type ArchitectureInfo,
  type QuotaAlertConfig,
  type RemoteQuotaConfig,
  type Sub2ApiRemoteQuotaGroup,
  type VerifyAuthRequest,
  type VerifyAuthResponse,
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
import {
  formatSub2ApiGroupOption,
  formatSub2ApiWindow,
  localSyncWindowText,
  sub2ApiQuotaWindows,
} from '../utils/sub2ApiQuota'

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

const { success: showSuccess, error: showError, showToast } = useToast()
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
const isLoadingSub2ApiGroups = ref(false)
const isSyncingRemoteQuota = ref(false)
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
const remoteQuota = ref<RemoteQuotaConfig>({
  enabled: false,
  group_id: null,
  progress_endpoint: '/api/v1/subscriptions/progress',
})
const savedRemoteQuotaSignature = ref(remoteQuotaSignature(remoteQuota.value))
const sub2ApiGroups = ref<Sub2ApiRemoteQuotaGroup[]>([])
const selectedSub2ApiGroupId = computed<string | undefined>({
  get: () => remoteQuota.value.group_id ?? undefined,
  set: (value) => {
    remoteQuota.value.group_id = value?.trim() || null
  },
})
const selectedSub2ApiGroup = computed(() => {
  const groupId = remoteQuota.value.group_id
  return sub2ApiGroups.value.find((group) => group.group_id === groupId) ?? null
})
const selectedSub2ApiGroupMissing = computed(() => {
  const groupId = remoteQuota.value.group_id
  return !!groupId && !sub2ApiGroups.value.some((group) => group.group_id === groupId)
})

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
const remoteQuotaChanged = computed(() => {
  return remoteQuotaSignature(remoteQuota.value) !== savedRemoteQuotaSignature.value
})
const remoteQuotaProgressEndpointValid = computed(() => {
  const endpoint = normalizeRemoteQuota(remoteQuota.value).progress_endpoint
  return endpoint.startsWith('/')
    && !endpoint.startsWith('//')
    && !endpoint.includes('#')
    && !endpoint.includes('\\')
})
const remoteQuotaValid = computed(() => {
  const normalized = normalizeRemoteQuota(remoteQuota.value)
  return !normalized.enabled
    || (
      selectedArchitectureId.value === 'sub2api'
      && normalized.group_id !== null
      && remoteQuotaProgressEndpointValid.value
    )
})

const canSave = computed(() => {
  return remoteQuotaValid.value && (
    (verifyStatus.value === 'success' && !formChanged.value)
    || (
      hasExistingConfig.value
      && (quotaAlertChanged.value || remoteQuotaChanged.value)
      && !formChanged.value
    )
  )
})

// 字段分组
const fieldGroups = computed<AuthTemplateFieldGroup[]>(() => {
  if (!currentSchema.value) return []
  return schemaToFieldGroups(currentSchema.value, props.providerWebsite)
})

// Methods
function handleArchitectureChange() {
  sub2ApiGroups.value = []
  if (selectedArchitectureId.value !== 'sub2api') {
    remoteQuota.value.enabled = false
  }
  // 切换架构时，默认选中第一个认证方式
  const authTypes = currentAuthTypes.value
  selectedAuthType.value = authTypes.length > 0 ? authTypes[0].type : ''
  resetFormData()
  verifyStatus.value = null
  formChanged.value = true
}

function handleAuthTypeChange() {
  sub2ApiGroups.value = []
  resetFormData()
  verifyStatus.value = null
  formChanged.value = true
}

function handleFieldChange(fieldKey: string, value: unknown) {
  formChanged.value = true
  sub2ApiGroups.value = []

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

function buildVerifyRequest(): VerifyAuthRequest | null {
  const schema = currentSchema.value
  if (!schema) return null

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
    return null
  }

  const effectiveBaseUrl = formData.value.base_url || props.providerWebsite
  if (!effectiveBaseUrl) {
    showError('请填写 API 地址')
    return null
  }
  const request = buildRequestFromSchema(
    schema,
    selectedArchitectureId.value,
    formData.value,
    props.providerWebsite,
  )
  return {
    ...request,
    base_url: request.base_url || String(effectiveBaseUrl),
  }
}

function applyUpdatedCredentials(result: VerifyAuthResponse, keepSaveable: boolean) {
  if (!result.updated_credentials) return
  for (const [key, value] of Object.entries(result.updated_credentials)) {
    formData.value[key] = value
  }
  if (keepSaveable) {
    nextTick(() => {
      formChanged.value = false
    })
  }
}

async function handleDiscoverSub2ApiGroups() {
  const verifyRequest = buildVerifyRequest()
  if (!verifyRequest) return

  isLoadingSub2ApiGroups.value = true
  try {
    const result = await discoverSub2ApiGroups(props.providerId, verifyRequest)
    const username = result.data?.username?.trim() || result.data?.display_name?.trim()
    const quota = result.data?.quota
    const accountVerified = result.success && !!username && quota !== undefined && quota !== null
    applyUpdatedCredentials(result, accountVerified)
    if (!accountVerified) {
      verifyStatus.value = 'error'
      const fallbackMessage = result.success
        ? '读取套餐成功，但 Sub2API 账号验证响应不完整'
        : '读取 Sub2API 套餐列表失败'
      showError(result.message || fallbackMessage)
      return
    }

    verifyStatus.value = 'success'
    formChanged.value = false
    const groups = result.data?.extra?.sub2api_groups ?? []
    sub2ApiGroups.value = groups
    const selectedExists = groups.some((group) => group.group_id === remoteQuota.value.group_id)
    if (!selectedExists) {
      remoteQuota.value.group_id = groups.length === 1 ? groups[0].group_id : null
    }
    if (groups.length === 0) {
      showError('当前 Sub2API 账号没有活跃套餐')
    } else if (groups.length === 1) {
      showSuccess(`已读取并选择套餐：${groups[0].group_name || '未命名套餐'}`, '读取成功')
    } else {
      showSuccess(`已读取 ${groups.length} 个有效订阅，请选择套餐`, '读取成功')
    }
  } catch (error: unknown) {
    verifyStatus.value = 'error'
    showError(parseApiError(error, '读取 Sub2API 套餐列表失败'))
  } finally {
    isLoadingSub2ApiGroups.value = false
  }
}

async function handleSyncRemoteQuota() {
  if (remoteQuotaChanged.value || !remoteQuota.value.group_id) return

  isSyncingRemoteQuota.value = true
  try {
    const result = await refreshBalance(props.providerId)
    const data = result.data as {
      extra?: {
        remote_quota_sync?: {
          status?: string
          message?: string
        }
      }
    } | null
    const sync = data?.extra?.remote_quota_sync
    if (sync?.status === 'applied') {
      showSuccess('远程套餐已同步到 Provider 本地额度', '同步成功')
      emit('saved')
      return
    }
    showToast({
      title: '本地额度未修改',
      message: sync?.message || result.message || '远程额度同步未应用，请检查上游响应后重试。',
      variant: 'error',
      duration: 10_000,
    })
  } catch (error: unknown) {
    showError(parseApiError(error, '同步远程额度失败'), '同步失败')
  } finally {
    isSyncingRemoteQuota.value = false
  }
}

async function handleVerify() {
  const verifyRequest = buildVerifyRequest()
  if (!verifyRequest) return

  isVerifying.value = true

  try {
    const result = await verifyProviderAuth(props.providerId, verifyRequest)

    if (result.success) {
      const username = result.data?.username?.trim() || result.data?.display_name?.trim()
      const quota = result.data?.quota

      if (!username || quota === undefined || quota === null) {
        verifyStatus.value = 'error'
        applyUpdatedCredentials(result, false)
        const missing: string[] = []
        if (!username) missing.push('用户信息')
        if (quota === undefined || quota === null) missing.push('余额')
        showError(`验证响应缺少: ${missing.join('、')}`)
      } else {
        verifyStatus.value = 'success'
        formChanged.value = false

        // Token Rotation: 验证过程中 refresh_token 可能已被轮换，用新值更新表单
        // 必须在 formChanged=false 之后执行，避免 watch 将 formChanged 重新设为 true
        applyUpdatedCredentials(result, true)

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
      applyUpdatedCredentials(result, false)

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
    request.remote_quota = normalizedRemoteQuota()
    const result = await saveProviderOpsConfig(props.providerId, request)
    if (result.success) {
      savedQuotaAlertSignature.value = quotaAlertSignature(quotaAlert.value)
      savedRemoteQuotaSignature.value = remoteQuotaSignature(remoteQuota.value)
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
      sub2ApiGroups.value = []
      loadQuotaAlert(null)
      loadRemoteQuota(null)
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
  loadRemoteQuota(config.remote_quota)
}

function defaultRemoteQuota(): RemoteQuotaConfig {
  return {
    enabled: false,
    group_id: null,
    progress_endpoint: '/api/v1/subscriptions/progress',
  }
}

function normalizeRemoteQuota(value: unknown): RemoteQuotaConfig {
  if (!isRecord(value)) return defaultRemoteQuota()
  const groupId = typeof value.group_id === 'string'
    ? value.group_id.trim()
    : typeof value.group_id === 'number'
      ? String(value.group_id)
      : ''
  const progressEndpoint = typeof value.progress_endpoint === 'string'
    ? value.progress_endpoint.trim()
    : ''
  return {
    enabled: value.enabled === true,
    group_id: groupId || null,
    progress_endpoint: progressEndpoint || '/api/v1/subscriptions/progress',
  }
}

function normalizedRemoteQuota(): RemoteQuotaConfig {
  const normalized = normalizeRemoteQuota(remoteQuota.value)
  if (selectedArchitectureId.value !== 'sub2api') {
    normalized.enabled = false
  }
  return normalized
}

function remoteQuotaSignature(value: RemoteQuotaConfig): string {
  const normalized = normalizeRemoteQuota(value)
  return JSON.stringify([
    normalized.enabled,
    normalized.group_id,
    normalized.progress_endpoint,
  ])
}

function loadRemoteQuota(value: unknown) {
  const normalized = normalizeRemoteQuota(value)
  remoteQuota.value = normalized
  savedRemoteQuotaSignature.value = remoteQuotaSignature(normalized)
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
      sub2ApiGroups.value = []

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
              remote_quota: config.remote_quota,
            }
            loadFromConfig(configData)
          } else {
            hasExistingConfig.value = false
            sensitivePlaceholders.value = {}
            loadQuotaAlert(null)
            loadRemoteQuota(null)
            selectedArchitectureId.value = 'new_api'
            selectedAuthType.value = ''
            resetFormData()
          }
        } catch {
          hasExistingConfig.value = false
          sensitivePlaceholders.value = {}
          loadQuotaAlert(null)
          loadRemoteQuota(null)
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
        loadRemoteQuota(null)
        selectedArchitectureId.value = 'new_api'
        selectedAuthType.value = ''
        resetFormData()
      }
    }
  }
)
</script>
