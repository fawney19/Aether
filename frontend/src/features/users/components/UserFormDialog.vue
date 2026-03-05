<template>
  <Dialog
    :model-value="isOpen"
    size="2xl"
    @update:model-value="handleDialogUpdate"
  >
    <template #header>
      <div class="border-b border-border px-6 py-4">
        <div class="flex items-center gap-3">
          <div class="flex h-9 w-9 items-center justify-center rounded-lg bg-primary/10 flex-shrink-0">
            <UserPlus
              v-if="!isEditMode"
              class="h-5 w-5 text-primary"
            />
            <SquarePen
              v-else
              class="h-5 w-5 text-primary"
            />
          </div>
          <div class="flex-1 min-w-0">
            <h3 class="text-lg font-semibold text-foreground leading-tight">
              {{ isEditMode ? '编辑用户' : '新增用户' }}
            </h3>
            <p class="text-xs text-muted-foreground">
              {{ isEditMode ? '修改用户账户信息' : '创建新的系统用户账户' }}
            </p>
          </div>
        </div>
      </div>
    </template>

    <form
      autocomplete="off"
      @submit.prevent="handleSubmit"
    >
      <div class="grid grid-cols-2 gap-0">
        <!-- 左侧：基础设置 -->
        <div class="pr-6 space-y-4">
          <div class="flex items-center gap-2 pb-2 border-b border-border/60">
            <span class="text-sm font-medium">基础设置</span>
          </div>

          <div class="space-y-2">
            <Label
              for="form-username"
              class="text-sm font-medium"
            >用户名 <span class="text-muted-foreground">*</span></Label>
            <Input
              id="form-username"
              v-model="form.username"
              type="text"
              autocomplete="off"
              data-form-type="other"
              required
              class="h-10"
              :class="usernameError ? 'border-destructive' : ''"
            />
            <p
              v-if="usernameError"
              class="text-xs text-destructive"
            >
              {{ usernameError }}
            </p>
            <p
              v-else
              class="text-xs text-muted-foreground"
            >
              3-30个字符，允许字母、数字、下划线、连字符和点号
            </p>
          </div>

          <div class="space-y-2">
            <Label class="text-sm font-medium">
              {{ isEditMode ? '新密码 (留空保持不变)' : '密码' }} <span
                v-if="!isEditMode"
                class="text-muted-foreground"
              >*</span>
            </Label>
            <Input
              :id="`pwd-${formNonce}`"
              v-model="form.password"
              :type="passwordFocused ? 'password' : 'text'"
              autocomplete="new-password"
              data-form-type="other"
              data-lpignore="true"
              :name="`field-${formNonce}`"
              :required="!isEditMode"
              minlength="8"
              :placeholder="isEditMode ? '留空保持原密码' : '至少8个字符，含大小写字母和数字'"
              :class="!passwordFocused && form.password.length === 0 ? 'h-10 text-transparent' : 'h-10'"
              @focus="passwordFocused = true"
              @blur="passwordFocused = form.password.length > 0"
            />
            <p
              class="text-xs"
              :class="passwordHintIsError ? 'text-destructive' : 'text-muted-foreground'"
            >
              {{ passwordHintText }}
            </p>
          </div>

          <div
            v-if="isEditMode && form.password.length > 0"
            class="space-y-2"
          >
            <Label class="text-sm font-medium">
              确认新密码 <span class="text-muted-foreground">*</span>
            </Label>
            <Input
              :id="`pwd-confirm-${formNonce}`"
              v-model="form.confirmPassword"
              type="password"
              autocomplete="new-password"
              data-form-type="other"
              data-lpignore="true"
              :name="`confirm-${formNonce}`"
              required
              minlength="8"
              placeholder="再次输入新密码"
              class="h-10"
            />
            <p
              v-if="form.confirmPassword.length > 0 && form.password !== form.confirmPassword"
              class="text-xs text-destructive"
            >
              两次输入的密码不一致
            </p>
          </div>

          <div class="space-y-2">
            <Label
              for="form-email"
              class="text-sm font-medium"
            >邮箱</Label>
            <Input
              id="form-email"
              v-model="form.email"
              type="email"
              autocomplete="off"
              data-form-type="other"
              class="h-10"
            />
          </div>

          <div class="space-y-2">
            <Label
              for="form-role"
              class="text-sm font-medium"
            >用户角色</Label>
            <div class="w-full">
              <Select v-model="form.role">
                <SelectTrigger
                  id="form-role"
                  class="h-10 w-full text-sm"
                >
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="user">
                    普通用户
                  </SelectItem>
                  <SelectItem value="admin">
                    管理员
                  </SelectItem>
                </SelectContent>
              </Select>
            </div>
          </div>

          <div
            v-if="!isEditMode"
            class="space-y-2"
          >
            <Label
              for="form-active"
              class="text-sm font-medium"
            >启用用户</Label>
            <div class="flex items-center gap-3">
              <Switch
                id="form-active"
                v-model="form.is_active"
              />
              <div class="flex flex-col">
                <span class="text-sm text-foreground">
                  {{ form.is_active ? '已启用' : '已禁用' }}
                </span>
                <span class="text-xs text-muted-foreground">
                  {{ form.is_active ? '允许登录与请求' : '阻止登录与请求' }}
                </span>
              </div>
            </div>
          </div>
        </div>

        <!-- 右侧：访问限制 -->
        <div class="pl-6 space-y-4 border-l border-border">
          <div class="flex items-center gap-2 pb-2 border-b border-border/60">
            <span class="text-sm font-medium">访问限制</span>
            <span class="text-xs text-muted-foreground">(留空不限)</span>
          </div>

          <!-- Provider 多选下拉框 -->
          <div class="space-y-2">
            <Label class="text-sm font-medium">允许的 Provider</Label>
            <div class="relative">
              <button
                type="button"
                class="w-full h-10 px-3 border rounded-lg bg-background text-left flex items-center justify-between hover:bg-muted/50 transition-colors"
                @click="providerDropdownOpen = !providerDropdownOpen"
              >
                <span :class="form.allowed_providers.length ? 'text-foreground' : 'text-muted-foreground'">
                  {{ form.allowed_providers.length ? `已选择 ${form.allowed_providers.length} 个` : '全部可用' }}
                </span>
                <ChevronDown
                  class="h-4 w-4 text-muted-foreground transition-transform"
                  :class="providerDropdownOpen ? 'rotate-180' : ''"
                />
              </button>
              <div
                v-if="providerDropdownOpen"
                class="fixed inset-0 z-[80]"
                @click.stop="providerDropdownOpen = false"
              />
              <div
                v-if="providerDropdownOpen"
                class="absolute z-[90] w-full mt-1 bg-popover border rounded-lg shadow-lg max-h-48 overflow-y-auto"
              >
                <div
                  v-for="provider in providers"
                  :key="provider.id"
                  class="flex items-center gap-2 px-3 py-2 hover:bg-muted/50 cursor-pointer"
                  @click="toggleSelection('allowed_providers', provider.id)"
                >
                  <input
                    type="checkbox"
                    :checked="form.allowed_providers.includes(provider.id)"
                    class="h-4 w-4 rounded border-gray-300 cursor-pointer"
                    @click.stop
                    @change="toggleSelection('allowed_providers', provider.id)"
                  >
                  <span class="text-sm">{{ provider.name }}</span>
                </div>
                <div
                  v-if="providers.length === 0"
                  class="px-3 py-2 text-sm text-muted-foreground"
                >
                  暂无可用 Provider
                </div>
              </div>
            </div>
          </div>

          <!-- API 格式多选下拉框 -->
          <div class="space-y-2">
            <Label class="text-sm font-medium">允许的 API 格式</Label>
            <div class="relative">
              <button
                type="button"
                class="w-full h-10 px-3 border rounded-lg bg-background text-left flex items-center justify-between hover:bg-muted/50 transition-colors"
                @click="endpointDropdownOpen = !endpointDropdownOpen"
              >
                <span :class="form.allowed_api_formats.length ? 'text-foreground' : 'text-muted-foreground'">
                  {{ form.allowed_api_formats.length ? `已选择 ${form.allowed_api_formats.length} 个` : '全部可用' }}
                </span>
                <ChevronDown
                  class="h-4 w-4 text-muted-foreground transition-transform"
                  :class="endpointDropdownOpen ? 'rotate-180' : ''"
                />
              </button>
              <div
                v-if="endpointDropdownOpen"
                class="fixed inset-0 z-[80]"
                @click.stop="endpointDropdownOpen = false"
              />
              <div
                v-if="endpointDropdownOpen"
                class="absolute z-[90] w-full mt-1 bg-popover border rounded-lg shadow-lg max-h-48 overflow-y-auto"
              >
                <div
                  v-for="format in apiFormats"
                  :key="format.value"
                  class="flex items-center gap-2 px-3 py-2 hover:bg-muted/50 cursor-pointer"
                  @click="toggleSelection('allowed_api_formats', format.value)"
                >
                  <input
                    type="checkbox"
                    :checked="form.allowed_api_formats.includes(format.value)"
                    class="h-4 w-4 rounded border-gray-300 cursor-pointer"
                    @click.stop
                    @change="toggleSelection('allowed_api_formats', format.value)"
                  >
                  <span class="text-sm">{{ format.label }}</span>
                </div>
                <div
                  v-if="apiFormats.length === 0"
                  class="px-3 py-2 text-sm text-muted-foreground"
                >
                  暂无可用 API 格式
                </div>
              </div>
            </div>
          </div>

          <!-- 模型多选下拉框 -->
          <ModelMultiSelect
            v-model="form.allowed_models"
            :models="globalModels"
          />

          <div class="space-y-2">
            <Label class="text-sm font-medium">无限制额度</Label>
            <div class="flex items-center gap-3">
              <Switch v-model="form.unlimited" />
              <div class="flex flex-col">
                <span class="text-sm text-foreground">
                  {{ form.unlimited ? '已启用' : '已关闭' }}
                </span>
                <span class="text-xs text-muted-foreground">
                  {{ form.unlimited ? '无限制：忽略钱包余额校验' : '有限制：按钱包余额校验' }}
                </span>
              </div>
            </div>
          </div>

          <div
            v-if="!isEditMode && !form.unlimited"
            class="space-y-2"
          >
            <Label
              for="form-initial-gift"
              class="text-sm font-medium"
            >初始赠款额度 (USD) <span class="text-muted-foreground">*</span></Label>
            <Input
              id="form-initial-gift"
              :model-value="form.initial_gift_usd ?? ''"
              type="number"
              step="0.01"
              min="0.01"
              placeholder="10.00"
              class="h-10"
              @update:model-value="(v) => form.initial_gift_usd = parseNumberInput(v, { allowFloat: true, min: 0.01 })"
            />
            <p class="text-xs text-muted-foreground">
              最小值 $0.01
            </p>
          </div>
        </div>
      </div>
    </form>

    <template #footer>
      <Button
        variant="outline"
        type="button"
        class="h-10 px-5"
        @click="handleCancel"
      >
        取消
      </Button>
      <Button
        class="h-10 px-5"
        :disabled="saving || !isFormValid"
        @click="handleSubmit"
      >
        {{ saving ? '处理中...' : (isEditMode ? '更新' : '创建') }}
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
  Switch,
  Select,
  SelectTrigger,
  SelectValue,
  SelectContent,
  SelectItem,
} from '@/components/ui'
import { UserPlus, SquarePen, ChevronDown } from 'lucide-vue-next'
import { useFormDialog } from '@/composables/useFormDialog'
import { ModelMultiSelect } from '@/components/common'
import { getProvidersSummary } from '@/api/endpoints/providers'
import { getGlobalModels } from '@/api/global-models'
import { adminApi } from '@/api/admin'
import { log } from '@/utils/logger'
import { parseNumberInput } from '@/utils/form'
import type { ProviderWithEndpointsSummary, GlobalModelResponse } from '@/api/endpoints/types'

export interface UserFormData {
  id?: string
  username: string
  email: string
  initial_gift_usd?: number | null
  unlimited?: boolean
  role: 'admin' | 'user'
  is_active?: boolean
  allowed_providers?: string[] | null
  allowed_api_formats?: string[] | null
  allowed_models?: string[] | null
}

const props = defineProps<{
  open: boolean
  user: UserFormData | null
}>()

const emit = defineEmits<{
  close: []
  submit: [data: UserFormData & { password?: string; unlimited?: boolean }]
}>()

const isOpen = computed(() => props.open)
const saving = ref(false)
const formNonce = ref(createFieldNonce())
const passwordFocused = ref(false)

// 下拉框状态
const providerDropdownOpen = ref(false)
const endpointDropdownOpen = ref(false)

// 选项数据
const providers = ref<ProviderWithEndpointsSummary[]>([])
const globalModels = ref<GlobalModelResponse[]>([])
const apiFormats = ref<Array<{ value: string; label: string }>>([])

// 表单数据
const form = ref({
  username: '',
  password: '',
  confirmPassword: '',
  email: '',
  initial_gift_usd: 10 as number | undefined,
  role: 'user' as 'admin' | 'user',
  unlimited: false,
  is_active: true,
  allowed_providers: [] as string[],
  allowed_api_formats: [] as string[],
  allowed_models: [] as string[]
})

function createFieldNonce(): string {
  return Math.random().toString(36).slice(2, 10)
}

function resetForm() {
  formNonce.value = createFieldNonce()
  passwordFocused.value = false
  form.value = {
    username: '',
    password: '',
    confirmPassword: '',
    email: '',
    initial_gift_usd: 10,
    role: 'user',
    unlimited: false,
    is_active: true,
    allowed_providers: [],
    allowed_api_formats: [],
    allowed_models: []
  }
}

function loadUserData() {
  if (!props.user) return
  formNonce.value = createFieldNonce()
  passwordFocused.value = false
  // 创建数组副本，避免与 props 数据共享引用
  form.value = {
    username: props.user.username,
    password: '',
    confirmPassword: '',
    email: props.user.email || '',
    initial_gift_usd: undefined,
    role: props.user.role,
    unlimited: props.user.unlimited ?? false,
    is_active: props.user.is_active ?? true,
    allowed_providers: [...(props.user.allowed_providers || [])],
    allowed_api_formats: [...(props.user.allowed_api_formats || [])],
    allowed_models: [...(props.user.allowed_models || [])]
  }
}

const { isEditMode, handleDialogUpdate, handleCancel } = useFormDialog({
  isOpen: () => props.open,
  entity: () => props.user,
  isLoading: saving,
  onClose: () => emit('close'),
  loadData: loadUserData,
  resetForm,
})

// 用户名验证
const usernameRegex = /^[a-zA-Z0-9_.-]+$/
const uppercaseRegex = /[A-Z]/
const lowercaseRegex = /[a-z]/
const digitRegex = /\d/
const usernameError = computed(() => {
  const username = form.value.username.trim()
  if (!username) return ''
  if (username.length < 3) return '用户名长度至少为3个字符'
  if (username.length > 30) return '用户名长度不能超过30个字符'
  if (!usernameRegex.test(username)) return '用户名只能包含字母、数字、下划线、连字符和点号'
  return ''
})

function formatMissingItems(items: string[]): string {
  if (items.length === 0) return ''
  if (items.length === 1) return items[0]
  if (items.length === 2) return `${items[0]}和${items[1]}`
  return `${items.slice(0, -1).join('、')}和${items[items.length - 1]}`
}

function getPasswordValidationError(password: string): string | null {
  const missingLength = password.length < 8
  const missingTypes: string[] = []

  if (!uppercaseRegex.test(password)) {
    missingTypes.push('大写字母')
  }
  if (!lowercaseRegex.test(password)) {
    missingTypes.push('小写字母')
  }
  if (!digitRegex.test(password)) {
    missingTypes.push('数字')
  }

  const missingComplexityText =
    missingTypes.length > 0 ? `必须包含${formatMissingItems(missingTypes)}` : ''

  if (missingLength && missingComplexityText) {
    return `至少 8 位，且${missingComplexityText}`
  }
  if (missingLength) {
    return '至少 8 位'
  }
  if (missingComplexityText) {
    return missingComplexityText
  }
  return null
}

// 表单验证
const isFormValid = computed(() => {
  const hasUsername = form.value.username.trim().length > 0
  const usernameValid = !usernameError.value
  const passwordFilled = form.value.password.length > 0
  const passwordValid = passwordFilled
    ? !getPasswordValidationError(form.value.password)
    : isEditMode.value
  // 编辑模式下可留空；填写时必须确认一致。创建模式不展示确认输入框。
  const passwordConfirmed = isEditMode.value
    ? !passwordFilled || form.value.password === form.value.confirmPassword
    : true
  const initialGiftValid = isEditMode.value ||
    form.value.unlimited ||
    (typeof form.value.initial_gift_usd === 'number' && form.value.initial_gift_usd >= 0.01)
  return hasUsername && usernameValid && passwordValid && passwordConfirmed && initialGiftValid
})

const passwordHintText = computed(() => {
  const password = form.value.password
  if (isEditMode.value && !password) {
    return '留空保持原密码'
  }
  if (!password) {
    return '至少 8 位，且必须包含大写字母、小写字母和数字'
  }
  return getPasswordValidationError(password) ?? '密码符合要求'
})

const passwordHintIsError = computed(() => {
  const password = form.value.password
  if (!password) return false
  return !!getPasswordValidationError(password)
})

// 加载访问控制选项
async function loadAccessControlOptions(): Promise<void> {
  try {
    const [providersData, modelsData, formatsData] = await Promise.all([
      getProvidersSummary(),
      getGlobalModels({ limit: 1000, is_active: true }),
      adminApi.getApiFormats()
    ])
    providers.value = providersData
    globalModels.value = modelsData.models || []
    apiFormats.value = formatsData.formats || []
  } catch (err) {
    log.error('加载访问限制选项失败:', err)
  }
}

// 切换选择
function toggleSelection(field: 'allowed_providers' | 'allowed_api_formats' | 'allowed_models', value: string) {
  const arr = form.value[field]
  const index = arr.indexOf(value)
  if (index === -1) {
    arr.push(value)
  } else {
    arr.splice(index, 1)
  }
}

// 提交表单
async function handleSubmit() {
  saving.value = true
  try {
    const data: UserFormData & { password?: string; unlimited: boolean } = {
      username: form.value.username,
      email: form.value.email.trim() || '',
      unlimited: form.value.unlimited,
      role: form.value.role,
      allowed_providers: form.value.allowed_providers.length > 0 ? form.value.allowed_providers : null,
      allowed_api_formats: form.value.allowed_api_formats.length > 0 ? form.value.allowed_api_formats : null,
      allowed_models: form.value.allowed_models.length > 0 ? form.value.allowed_models : null
    }

    if (isEditMode.value && props.user?.id) {
      data.id = props.user.id
    }

    if (!isEditMode.value) {
      data.is_active = form.value.is_active
      if (!form.value.unlimited && form.value.initial_gift_usd != null) {
        data.initial_gift_usd = form.value.initial_gift_usd
      }
    }

    if (form.value.password) {
      data.password = form.value.password
    } else if (!isEditMode.value) {
      // 创建模式必须有密码
      return
    }

    emit('submit', data)
  } finally {
    saving.value = false
  }
}

// 设置保存状态（供父组件调用）
function setSaving(value: boolean) {
  saving.value = value
}

// 监听打开状态，加载选项数据
watch(isOpen, (val) => {
  if (val) {
    loadAccessControlOptions()
  }
})

watch(
  () => form.value.unlimited,
  (unlimited) => {
    if (isEditMode.value) {
      return
    }
    if (unlimited) {
      form.value.initial_gift_usd = undefined
    } else if (form.value.initial_gift_usd == null) {
      form.value.initial_gift_usd = 10
    }
  }
)

defineExpose({
  setSaving
})
</script>
