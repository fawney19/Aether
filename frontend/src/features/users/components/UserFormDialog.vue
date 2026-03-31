<template>
  <Dialog
    :model-value="isOpen"
    size="2xl"
    @update:model-value="handleDialogUpdate"
  >
    <template #header>
      <div class="border-b border-border px-6 py-4">
        <div class="flex items-center gap-3">
          <div class="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-primary/10">
            <UserPlus
              v-if="!isEditMode"
              class="h-5 w-5 text-primary"
            />
            <SquarePen
              v-else
              class="h-5 w-5 text-primary"
            />
          </div>
          <div class="min-w-0 flex-1">
            <h3 class="text-lg font-semibold leading-tight text-foreground">
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
      <div class="grid grid-cols-1 gap-0 lg:grid-cols-2">
        <div class="space-y-4 lg:pr-6">
          <div class="flex items-center gap-2 border-b border-border/60 pb-2">
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
              {{ isEditMode ? '新密码 (留空保持不变)' : '密码' }}
              <span
                v-if="!isEditMode"
                class="text-muted-foreground"
              >*</span>
            </Label>
            <Input
              :id="`pwd-${formNonce}`"
              v-model="form.password"
              type="text"
              masked
              autocomplete="new-password"
              disable-autofill
              :name="`field-${formNonce}`"
              :required="!isEditMode"
              minlength="6"
              :placeholder="isEditMode ? '留空保持原密码' : getPasswordPolicyPlaceholder(passwordPolicyLevel)"
              class="h-10"
              :class="passwordError ? 'border-destructive' : ''"
            />
            <p
              v-if="passwordError"
              class="text-xs text-destructive"
            >
              {{ passwordError }}
            </p>
            <p
              v-else-if="!isEditMode"
              class="text-xs text-muted-foreground"
            >
              {{ passwordHint }}
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
              type="text"
              masked
              autocomplete="new-password"
              data-form-type="other"
              data-lpignore="true"
              :name="`confirm-${formNonce}`"
              required
              minlength="6"
              placeholder="再次输入新密码"
              class="h-10"
            />
            <p
              v-if="
                form.confirmPassword.length > 0 &&
                  form.password !== form.confirmPassword
              "
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

          <div class="space-y-2">
            <Label class="text-sm font-medium">额度</Label>
            <div class="flex items-center gap-3">
              <div class="min-w-0 flex-1">
                <Input
                  v-if="!isEditMode && !form.unlimited"
                  id="form-initial-gift"
                  :model-value="form.initial_gift_usd ?? ''"
                  type="number"
                  step="0.01"
                  min="0.01"
                  placeholder="初始额度 (USD)"
                  class="h-10"
                  @update:model-value="(v) => form.initial_gift_usd = parseNumberInput(v, { allowFloat: true, min: 0.01 })"
                />
                <span
                  v-else
                  class="flex h-10 w-full items-center rounded-lg border bg-background px-3 text-sm text-muted-foreground opacity-60"
                >{{ form.unlimited ? '无限制' : '按钱包余额限制' }}</span>
              </div>
              <Switch
                v-model="form.unlimited"
                class="shrink-0"
              />
            </div>
          </div>
        </div>

        <div class="space-y-4 border-t border-border pt-4 lg:border-l lg:border-t-0 lg:pl-6 lg:pt-0">
          <div class="flex items-center gap-2 border-b border-border/60 pb-2">
            <span class="text-sm font-medium">访问限制</span>
          </div>

          <div class="space-y-2">
            <Label
              for="form-group"
              class="text-sm font-medium"
            >用户分组</Label>
            <div class="w-full">
              <Select v-model="groupSelectValue">
                <SelectTrigger
                  id="form-group"
                  class="h-10 w-full text-sm"
                >
                  <SelectValue placeholder="选择用户分组" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem
                    v-for="group in userGroups"
                    :key="group.id"
                    :value="group.id"
                  >
                    {{ group.name }}
                  </SelectItem>
                </SelectContent>
              </Select>
            </div>
            <p class="text-xs text-muted-foreground">
              保存后统一按所选分组默认限制生效。
            </p>
          </div>

          <div class="space-y-2">
            <Label class="text-sm font-medium">模型分组</Label>
            <span class="flex h-10 w-full items-center overflow-hidden whitespace-nowrap rounded-lg border bg-background px-3 text-sm text-muted-foreground opacity-80">
              {{ formatModelGroupBindings(currentGroup?.model_group_bindings) }}
            </span>
          </div>

          <div class="space-y-2">
            <Label class="text-sm font-medium">允许的 API 格式</Label>
            <span class="flex h-10 w-full items-center overflow-hidden whitespace-nowrap rounded-lg border bg-background px-3 text-sm text-muted-foreground opacity-80">
              {{ formatApiFormatRestrictions(currentGroup?.allowed_api_formats) }}
            </span>
          </div>

          <div class="space-y-2">
            <Label class="text-sm font-medium">路由与计费</Label>
            <span class="flex h-10 w-full items-center overflow-hidden whitespace-nowrap rounded-lg border bg-background px-3 text-sm text-muted-foreground opacity-80">
              由命中的模型分组决定
            </span>
          </div>

          <div class="space-y-2">
            <Label class="text-sm font-medium">速率限制 (请求/分钟)</Label>
            <span class="flex h-10 w-full items-center overflow-hidden whitespace-nowrap rounded-lg border bg-background px-3 text-sm text-muted-foreground opacity-80">
              {{ formatRateLimitSummary(currentGroup?.rate_limit) }}
            </span>
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
        {{ saving ? '处理中...' : isEditMode ? '更新' : '创建' }}
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import {
  Button,
  Dialog,
  Input,
  Label,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Switch,
} from '@/components/ui'
import { SquarePen, UserPlus } from 'lucide-vue-next'
import { useFormDialog } from '@/composables/useFormDialog'
import { adminApi } from '@/api/admin'
import { formatApiFormat } from '@/api/endpoints/types/api-format'
import { usersApi, type UserGroup } from '@/api/users'
import { log } from '@/utils/logger'
import { parseNumberInput } from '@/utils/form'
import {
  getPasswordPolicyHint,
  getPasswordPolicyPlaceholder,
  normalizePasswordPolicyLevel,
  validatePasswordByPolicy,
  type PasswordPolicyLevel,
} from '@/utils/passwordPolicy'

export interface UserFormData {
  id?: string
  username: string
  email: string
  initial_gift_usd?: number | null
  unlimited?: boolean
  role: 'admin' | 'user'
  is_active?: boolean
  group_id?: string | null
  group_name?: string | null
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
const passwordPolicyLevel = ref<PasswordPolicyLevel>('weak')
const userGroups = ref<UserGroup[]>([])
const apiFormats = ref<Array<{ value: string; label: string }>>([])

const form = ref({
  username: '',
  password: '',
  confirmPassword: '',
  email: '',
  initial_gift_usd: 10 as number | undefined,
  role: 'user' as 'admin' | 'user',
  unlimited: false,
  is_active: true,
  group_id: null as string | null,
})

const defaultGroup = computed(() => {
  return userGroups.value.find(group => group.is_default) || userGroups.value[0] || null
})

const currentGroup = computed(() => {
  return userGroups.value.find(group => group.id === form.value.group_id) || defaultGroup.value
})

const apiFormatLabelMap = computed(() => new Map(
  apiFormats.value.map(format => [format.value, format.label])
))

const groupSelectValue = computed({
  get: () => form.value.group_id ?? defaultGroup.value?.id ?? '',
  set: (value: string) => {
    form.value.group_id = value || null
  },
})

function createFieldNonce(): string {
  return Math.random().toString(36).slice(2, 10)
}

function resetForm() {
  formNonce.value = createFieldNonce()
  form.value = {
    username: '',
    password: '',
    confirmPassword: '',
    email: '',
    initial_gift_usd: 10,
    role: 'user',
    unlimited: false,
    is_active: true,
    group_id: null,
  }
}

function syncFormGroupSelection() {
  const fallbackGroupId = defaultGroup.value?.id ?? null
  if (!fallbackGroupId) {
    return
  }
  const hasSelectedGroup = !!form.value.group_id && userGroups.value.some(group => group.id === form.value.group_id)
  if (!hasSelectedGroup) {
    form.value.group_id = fallbackGroupId
  }
}

function loadUserData() {
  if (!props.user) return
  formNonce.value = createFieldNonce()
  form.value = {
    username: props.user.username,
    password: '',
    confirmPassword: '',
    email: props.user.email || '',
    initial_gift_usd: undefined,
    role: props.user.role,
    unlimited: props.user.unlimited ?? false,
    is_active: props.user.is_active ?? true,
    group_id: props.user.group_id ?? null,
  }
  syncFormGroupSelection()
}

const { isEditMode, handleDialogUpdate, handleCancel } = useFormDialog({
  isOpen: () => props.open,
  entity: () => props.user,
  isLoading: saving,
  onClose: () => emit('close'),
  loadData: loadUserData,
  resetForm,
})

const usernameRegex = /^[a-zA-Z0-9_.-]+$/
const usernameError = computed(() => {
  const username = form.value.username.trim()
  if (!username) return ''
  if (username.length < 3) return '用户名长度至少为3个字符'
  if (username.length > 30) return '用户名长度不能超过30个字符'
  if (!usernameRegex.test(username)) {
    return '用户名只能包含字母、数字、下划线、连字符和点号'
  }
  return ''
})

const passwordHint = computed(() => getPasswordPolicyHint(passwordPolicyLevel.value))

const passwordError = computed(() => {
  if (!form.value.password) {
    return ''
  }
  return validatePasswordByPolicy(form.value.password, passwordPolicyLevel.value)
})

const isFormValid = computed(() => {
  const hasUsername = form.value.username.trim().length > 0
  const usernameValid = !usernameError.value
  const passwordFilled = form.value.password.length > 0
  const passwordValid = passwordFilled ? !passwordError.value : isEditMode.value
  const passwordConfirmed = isEditMode.value
    ? !passwordFilled || form.value.password === form.value.confirmPassword
    : true
  const initialGiftValid = isEditMode.value ||
    form.value.unlimited ||
    (typeof form.value.initial_gift_usd === 'number' && form.value.initial_gift_usd >= 0.01)
  return hasUsername && usernameValid && passwordValid && passwordConfirmed && initialGiftValid
})

function formatRestrictionDisplay(
  list: string[] | null | undefined,
  unitLabel: string,
  itemFormatter: (value: string) => string = value => value,
): string {
  if (list == null) return '不限制'
  if (list.length === 0) return '未开放'
  if (list.length <= 2) return list.map(itemFormatter).join('、')
  return `${list.length} ${unitLabel}`
}

function formatApiFormatName(apiFormat: string): string {
  return apiFormatLabelMap.value.get(apiFormat) || formatApiFormat(apiFormat)
}

function formatApiFormatRestrictions(list: string[] | null | undefined): string {
  return formatRestrictionDisplay(list, '个格式', formatApiFormatName)
}

function formatModelGroupBindings(bindings: UserGroup['model_group_bindings'] | null | undefined): string {
  if (!bindings || bindings.length === 0) return '默认模型分组'
  const activeBindings = bindings.filter((binding) => binding.is_active)
  if (activeBindings.length === 0) return '全部停用'
  if (activeBindings.length <= 2) {
    return activeBindings
      .map((binding) => binding.model_group_display_name || binding.model_group_name || binding.model_group_id)
      .join('、')
  }
  return `${activeBindings.length} 个模型分组`
}

function formatRateLimitSummary(rateLimit: number | null | undefined): string {
  if (rateLimit == null) return '跟随系统默认'
  if (rateLimit === 0) return '不限速'
  return `${rateLimit} RPM`
}

async function loadFormOptions(): Promise<void> {
  try {
    const [passwordPolicyResponse, groupsData, formatsData] = await Promise.all([
      adminApi.getSystemConfig('password_policy_level').catch(() => ({ value: 'weak' })),
      usersApi.getAllUserGroups(),
      adminApi.getApiFormats().catch(() => ({ formats: [] } as { formats: Array<{ value: string; label: string }> })),
    ])
    passwordPolicyLevel.value = normalizePasswordPolicyLevel(passwordPolicyResponse.value)
    userGroups.value = groupsData
    apiFormats.value = formatsData.formats || []
    syncFormGroupSelection()
  } catch (err) {
    log.error('加载用户表单选项失败:', err)
    passwordPolicyLevel.value = 'weak'
    userGroups.value = []
    apiFormats.value = []
  }
}

async function handleSubmit() {
  saving.value = true
  try {
    const data: UserFormData & { password?: string; unlimited: boolean } = {
      username: form.value.username,
      email: form.value.email.trim() || '',
      unlimited: form.value.unlimited,
      role: form.value.role,
      group_id: form.value.group_id ?? defaultGroup.value?.id ?? null,
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
      return
    }

    emit('submit', data)
  } finally {
    saving.value = false
  }
}

function setSaving(value: boolean) {
  saving.value = value
}

watch(isOpen, (val) => {
  if (val) {
    void loadFormOptions()
  }
})

watch(
  [userGroups, () => props.open],
  () => {
    if (props.open) {
      syncFormGroupSelection()
    }
  },
  { deep: true }
)

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
  setSaving,
})
</script>
