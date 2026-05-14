<template>
  <Dialog
    :model-value="isOpen"
    size="xl"
    @update:model-value="handleDialogUpdate"
  >
    <template #header>
      <div class="border-b border-border px-6 py-4">
        <div class="flex items-center gap-3">
          <div
            class="flex h-9 w-9 items-center justify-center rounded-lg bg-primary/10 flex-shrink-0"
          >
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
      <div class="space-y-5">
        <div class="grid gap-4 sm:grid-cols-2">
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
            :class="[
              passwordError ? 'border-destructive' : '',
            ]"
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
          <Label class="text-sm font-medium">所属分组</Label>
          <MultiSelect
            v-model="form.group_ids"
            :options="groupOptions"
            :search-threshold="0"
            placeholder="可选择多个分组"
            empty-text="暂无分组"
            no-results-text="未找到匹配的分组"
          />
        </div>

        <div class="space-y-2">
          <Label class="text-sm font-medium">额度</Label>
          <div class="flex items-center gap-3">
            <div class="flex-1 min-w-0">
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

        <div class="rounded-lg border border-border bg-muted/30 p-3">
          <div class="flex items-center justify-between gap-3">
            <Label class="text-sm font-medium">敏感信息保护</Label>
            <Switch v-model="form.chat_pii_redaction_enabled" />
          </div>
          <div class="mt-3 flex items-center justify-between gap-3">
            <Label class="text-sm font-medium">占位符说明</Label>
            <Switch
              v-model="form.chat_pii_redaction_placeholder_notice"
              :disabled="!form.chat_pii_redaction_enabled"
            />
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
import { UserPlus, SquarePen } from 'lucide-vue-next'
import { useFormDialog } from '@/composables/useFormDialog'
import { MultiSelect } from '@/components/common'
import { adminApi } from '@/api/admin'
import { log } from '@/utils/logger'
import { parseNumberInput } from '@/utils/form'
import {
  mergeChatPiiRedactionFeatureSettings,
  readChatPiiRedactionFeatureSettings,
} from '@/utils/featureSettings'
import {
  getPasswordPolicyHint,
  getPasswordPolicyPlaceholder,
  normalizePasswordPolicyLevel,
  validatePasswordByPolicy,
  type PasswordPolicyLevel,
} from '@/utils/passwordPolicy'
import type { UserGroup } from '@/api/users'

export interface UserFormData {
  id?: string
  username: string
  email: string
  initial_gift_usd?: number | null
  unlimited?: boolean
  role: 'admin' | 'user'
  is_active?: boolean
  group_ids?: string[]
  feature_settings?: Record<string, unknown> | null
}

const props = defineProps<{
  open: boolean
  user: UserFormData | null
  groups?: UserGroup[]
}>()

const emit = defineEmits<{
  close: []
  submit: [data: UserFormData & { password?: string; unlimited?: boolean }]
}>()

const isOpen = computed(() => props.open)
const saving = ref(false)
const formNonce = ref(createFieldNonce())
const passwordPolicyLevel = ref<PasswordPolicyLevel>('weak')

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
  group_ids: [] as string[],
  chat_pii_redaction_enabled: false,
  chat_pii_redaction_placeholder_notice: true,
})

const groupOptions = computed(() => (props.groups || []).map((group) => ({
  label: group.name,
  value: group.id,
})))

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
    group_ids: [],
    chat_pii_redaction_enabled: false,
    chat_pii_redaction_placeholder_notice: true,
  }
}

function loadUserData() {
  if (!props.user) return
  formNonce.value = createFieldNonce()
  const redactionFeature = readChatPiiRedactionFeatureSettings(props.user.feature_settings)
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
    group_ids: props.user.group_ids ? [...props.user.group_ids] : [],
    chat_pii_redaction_enabled: redactionFeature.enabled,
    chat_pii_redaction_placeholder_notice: redactionFeature.inject_model_instruction,
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
const usernameError = computed(() => {
  const username = form.value.username.trim()
  if (!username) return ''
  if (username.length < 3) return '用户名长度至少为3个字符'
  if (username.length > 30) return '用户名长度不能超过30个字符'
  if (!usernameRegex.test(username))
    return '用户名只能包含字母、数字、下划线、连字符和点号'
  return ''
})

const passwordHint = computed(() => getPasswordPolicyHint(passwordPolicyLevel.value))

const passwordError = computed(() => {
  if (!form.value.password) {
    return ''
  }
  return validatePasswordByPolicy(form.value.password, passwordPolicyLevel.value)
})

// 表单验证
const isFormValid = computed(() => {
  const hasUsername = form.value.username.trim().length > 0
  const usernameValid = !usernameError.value
  const passwordFilled = form.value.password.length > 0
  const passwordValid = passwordFilled
    ? !passwordError.value
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


async function loadPasswordPolicy(): Promise<void> {
  try {
    const passwordPolicyResponse = await adminApi
      .getSystemConfig('password_policy_level')
      .catch(() => ({ value: 'weak' }))
    passwordPolicyLevel.value = normalizePasswordPolicyLevel(passwordPolicyResponse.value)
  } catch (err) {
    log.error('加载密码策略失败:', err)
    passwordPolicyLevel.value = 'weak'
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
      group_ids: [...form.value.group_ids],
      feature_settings: mergeChatPiiRedactionFeatureSettings(props.user?.feature_settings, {
        enabled: form.value.chat_pii_redaction_enabled,
        inject_model_instruction: form.value.chat_pii_redaction_placeholder_notice,
      }),
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
    loadPasswordPolicy()
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
  setSaving,
})
</script>
