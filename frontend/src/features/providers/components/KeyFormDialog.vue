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
      class="space-y-4"
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
          <Label :for="apiKeyInputId">API 密钥 {{ editingKey ? '' : '*' }}</Label>
          <Input
            :id="apiKeyInputId"
            v-model="form.api_key"
            :name="apiKeyFieldName"
            :type="apiKeyInputType"
            :required="!editingKey"
            :placeholder="editingKey ? editingKey.api_key_masked : 'sk-...'"
            :class="getApiKeyInputClass()"
            autocomplete="new-password"
            autocapitalize="none"
            autocorrect="off"
            spellcheck="false"
            data-form-type="other"
            data-lpignore="true"
            data-1p-ignore="true"
            @focus="apiKeyFocused = true"
            @blur="apiKeyFocused = form.api_key.trim().length > 0"
          />
          <p
            v-if="apiKeyError"
            class="text-xs text-destructive mt-1"
          >
            {{ apiKeyError }}
          </p>
          <p
            v-else-if="editingKey"
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

      <!-- API 格式选择 -->
      <div v-if="sortedApiFormats.length > 0">
        <Label class="mb-1.5 block">支持的 API 格式 *</Label>
        <div class="grid grid-cols-2 gap-2">
          <div
            v-for="format in sortedApiFormats"
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
                class="text-sm whitespace-nowrap"
                :class="form.api_formats.includes(format) ? 'text-primary' : 'text-muted-foreground'"
              >{{ API_FORMAT_LABELS[format] || format }}</span>
            </div>
            <div
              class="flex items-center shrink-0 ml-2 text-xs text-muted-foreground gap-1"
              @click.stop
            >
              <span>×</span>
              <input
                :value="form.rate_multipliers[format] ?? ''"
                type="number"
                step="0.01"
                min="0.01"
                placeholder="1"
                class="w-9 bg-transparent text-right outline-none [appearance:textfield] [&::-webkit-outer-spin-button]:appearance-none [&::-webkit-inner-spin-button]:appearance-none"
                :class="form.api_formats.includes(format) ? 'text-primary' : 'text-muted-foreground'"
                title="成本倍率"
                @input="(e) => updateRateMultiplier(format, (e.target as HTMLInputElement).value)"
              >
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
            min="2"
            max="32"
            placeholder="32"
            class="h-8"
            @update:model-value="(v) => form.max_probe_interval_minutes = parseNumberInput(v, { min: 2, max: 32 }) ?? 32"
          />
          <p class="text-xs text-muted-foreground mt-0.5">
            分钟，2-32
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
    </form>

    <template #footer>
      <Button
        variant="outline"
        @click="handleCancel"
      >
        取消
      </Button>
      <Button
        :disabled="saving"
        @click="handleSave"
      >
        {{ saving ? (isEditMode ? '保存中...' : '添加中...') : (isEditMode ? '保存' : '添加') }}
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { ref, computed, onMounted, watch } from 'vue'
import { Dialog, Button, Input, Label } from '@/components/ui'
import { Key, SquarePen } from 'lucide-vue-next'
import { useToast } from '@/composables/useToast'
import { useFormDialog } from '@/composables/useFormDialog'
import { parseApiError } from '@/utils/errorParser'
import { parseNumberInput, parseNullableNumberInput } from '@/utils/form'
import { log } from '@/utils/logger'
import {
  addProviderKey,
  updateProviderKey,
  getAllCapabilities,
  API_FORMAT_LABELS,
  sortApiFormats,
  type EndpointAPIKey,
  type EndpointAPIKeyUpdate,
  type ProviderEndpoint,
  type CapabilityDefinition
} from '@/api/endpoints'

const props = defineProps<{
  open: boolean
  endpoint: ProviderEndpoint | null
  editingKey: EndpointAPIKey | null
  providerId: string | null
  availableApiFormats: string[]  // Provider 支持的所有 API 格式
}>()

const emit = defineEmits<{
  close: []
  saved: []
}>()

const { success, error: showError } = useToast()

// 排序后的可用 API 格式列表
const sortedApiFormats = computed(() => sortApiFormats(props.availableApiFormats))

const isOpen = computed(() => props.open)
const saving = ref(false)
const formNonce = ref(createFieldNonce())
const keyNameInputId = computed(() => `key-name-${formNonce.value}`)
const apiKeyInputId = computed(() => `api-key-${formNonce.value}`)
const keyNameFieldName = computed(() => `key-name-field-${formNonce.value}`)
const apiKeyFieldName = computed(() => `api-key-field-${formNonce.value}`)
const apiKeyFocused = ref(false)
const apiKeyInputType = computed(() =>
  apiKeyFocused.value || form.value.api_key.trim().length > 0 ? 'password' : 'text'
)

// 可用的能力列表
const availableCapabilities = ref<CapabilityDefinition[]>([])

const form = ref({
  name: '',
  api_key: '',
  api_formats: [] as string[],  // 支持的 API 格式列表
  rate_multipliers: {} as Record<string, number>,  // 按 API 格式的成本倍率
  internal_priority: 10,
  rpm_limit: undefined as number | null | undefined,  // RPM 限制（null=自适应，undefined=保持原值）
  cache_ttl_minutes: 5,
  max_probe_interval_minutes: 32,
  note: '',
  is_active: true,
  capabilities: {} as Record<string, boolean>
})

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
  } else {
    // 移除格式前检查：至少保留一个格式
    if (form.value.api_formats.length <= 1) {
      showError('至少需要选择一个 API 格式', '验证失败')
      return
    }
    // 移除格式，但保留倍率配置（用户可能只是临时取消）
    form.value.api_formats.splice(index, 1)
  }
}

// 更新指定格式的成本倍率
function updateRateMultiplier(format: string, value: string | number) {
  // 使用对象替换以确保 Vue 3 响应性
  const newMultipliers = { ...form.value.rate_multipliers }

  if (value === '' || value === null || value === undefined) {
    // 清空时删除该格式的配置（使用默认倍率）
    delete newMultipliers[format]
  } else {
    const numValue = typeof value === 'string' ? parseFloat(value) : value
    // 限制倍率范围：0.01 - 100
    if (!isNaN(numValue) && numValue >= 0.01 && numValue <= 100) {
      newMultipliers[format] = numValue
    }
  }

  // 替换整个对象以触发响应式更新
  form.value.rate_multipliers = newMultipliers
}

// API 密钥输入框样式计算
function getApiKeyInputClass(): string {
  const classes = []
  if (apiKeyError.value) {
    classes.push('border-destructive')
  }
  if (!apiKeyFocused.value && !form.value.api_key) {
    classes.push('text-transparent caret-transparent selection:bg-transparent selection:text-transparent')
  }
  return classes.join(' ')
}


// API 密钥验证错误信息
const apiKeyError = computed(() => {
  const apiKey = form.value.api_key.trim()
  if (!apiKey) {
    // 新增模式下必填
    if (!props.editingKey) {
      return ''  // 空值由 required 属性处理
    }
    // 编辑模式下可以为空（表示不修改）
    return ''
  }

  // 如果输入了值，检查长度
  if (apiKey.length < 3) {
    return 'API 密钥至少需要 3 个字符'
  }

  return ''
})

// 重置表单
function resetForm() {
  formNonce.value = createFieldNonce()
  apiKeyFocused.value = false
  form.value = {
    name: '',
    api_key: '',
    api_formats: [],  // 默认不选中任何格式
    rate_multipliers: {},
    internal_priority: 10,
    rpm_limit: undefined,
    cache_ttl_minutes: 5,
    max_probe_interval_minutes: 32,
    note: '',
    is_active: true,
    capabilities: {}
  }
}

// 添加成功后清除部分字段以便继续添加
function clearForNextAdd() {
  formNonce.value = createFieldNonce()
  apiKeyFocused.value = false
  form.value.name = ''
  form.value.api_key = ''
}

// 加载密钥数据（编辑模式）
function loadKeyData() {
  if (!props.editingKey) return
  formNonce.value = createFieldNonce()
  apiKeyFocused.value = false
  form.value = {
    name: props.editingKey.name,
    api_key: '',
    api_formats: props.editingKey.api_formats?.length > 0
      ? [...props.editingKey.api_formats]
      : [],  // 编辑模式下保持原有选择，不默认全选
    rate_multipliers: { ...(props.editingKey.rate_multipliers || {}) },
    internal_priority: props.editingKey.internal_priority ?? 10,
    // 保留原始的 null/undefined 状态，null 表示自适应模式
    rpm_limit: props.editingKey.rpm_limit ?? undefined,
    cache_ttl_minutes: props.editingKey.cache_ttl_minutes ?? 5,
    max_probe_interval_minutes: props.editingKey.max_probe_interval_minutes ?? 32,
    note: props.editingKey.note || '',
    is_active: props.editingKey.is_active,
    capabilities: { ...(props.editingKey.capabilities || {}) }
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

async function handleSave() {
  // 必须有 providerId
  if (!props.providerId) {
    showError('无法保存：缺少提供商信息', '错误')
    return
  }

  // 提交前验证
  if (apiKeyError.value) {
    showError(apiKeyError.value, '验证失败')
    return
  }

  // 新增模式下，API 密钥必填
  if (!props.editingKey && !form.value.api_key.trim()) {
    showError('请输入 API 密钥', '验证失败')
    return
  }

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

    if (props.editingKey) {
      // 更新模式
      // 注意：rpm_limit 使用 null 表示自适应模式
      // undefined 表示"保持原值不变"（会在 JSON 序列化时被忽略）
      const updateData: EndpointAPIKeyUpdate = {
        api_formats: form.value.api_formats,
        name: form.value.name,
        rate_multipliers: rateMultipliersData,
        internal_priority: form.value.internal_priority,
        rpm_limit: form.value.rpm_limit,
        cache_ttl_minutes: form.value.cache_ttl_minutes,
        max_probe_interval_minutes: form.value.max_probe_interval_minutes,
        note: form.value.note,
        is_active: form.value.is_active,
        capabilities: capabilitiesData
      }

      if (form.value.api_key.trim()) {
        updateData.api_key = form.value.api_key
      }

      await updateProviderKey(props.editingKey.id, updateData)
      success('密钥已更新', '成功')
    } else {
      // 新增模式
      await addProviderKey(props.providerId, {
        api_formats: form.value.api_formats,
        api_key: form.value.api_key,
        name: form.value.name,
        rate_multipliers: rateMultipliersData,
        internal_priority: form.value.internal_priority,
        rpm_limit: form.value.rpm_limit,
        cache_ttl_minutes: form.value.cache_ttl_minutes,
        max_probe_interval_minutes: form.value.max_probe_interval_minutes,
        note: form.value.note,
        capabilities: capabilitiesData || undefined
      })
      success('密钥已添加', '成功')
      // 添加模式：不关闭对话框，只清除名称和密钥以便继续添加
      emit('saved')
      clearForNextAdd()
      return
    }

    emit('saved')
    emit('close')
  } catch (err: any) {
    const errorMessage = parseApiError(err, '保存密钥失败')
    showError(errorMessage, '错误')
  } finally {
    saving.value = false
  }
}
</script>
