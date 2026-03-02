<template>
  <div class="space-y-6">
    <div class="flex flex-col items-center text-center">
      <div class="mb-4 rounded-3xl border border-primary/30 dark:border-[#cc785c]/30 bg-primary/5 dark:bg-transparent p-4 shadow-inner shadow-white/40 dark:shadow-[#cc785c]/10">
        <img
          src="/aether_adaptive.svg"
          alt="Logo"
          class="h-16 w-16"
        >
      </div>
      <h2 class="text-2xl font-semibold text-slate-900 dark:text-white">
        注册新账户
      </h2>
      <p class="mt-1 text-sm text-muted-foreground">
        {{ emailConfigured ? '请填写您的信息完成注册' : '请填写用户名和密码完成注册' }}
      </p>
    </div>

    <form
      class="space-y-4"
      autocomplete="off"
      data-form-type="other"
      @submit.prevent="emit('submit')"
    >
      <div
        v-if="emailConfigured"
        class="space-y-2"
      >
        <Label for="reg-email">
          邮箱
          <span
            v-if="requireEmailVerification"
            class="text-destructive"
          >*</span>
          <span
            v-else
            class="text-muted-foreground text-xs"
          >（可选）</span>
        </Label>
        <Input
          id="reg-email"
          :model-value="email"
          type="email"
          placeholder="hello@example.com"
          :required="requireEmailVerification"
          disable-autofill
          :disabled="isLoading || emailVerified"
          @update:model-value="emit('update:email', $event)"
        />
      </div>

      <div
        v-if="emailConfigured && requireEmailVerification"
        class="space-y-3"
      >
        <div class="flex items-center justify-between">
          <Label>验证码 <span class="text-destructive">*</span></Label>
          <Button
            type="button"
            variant="link"
            size="sm"
            class="h-auto p-0 text-xs"
            :disabled="isSendingCode || !canSendCode || emailVerified"
            @click="emit('send-code')"
          >
            {{ sendCodeButtonText }}
          </Button>
        </div>
        <div class="flex justify-center gap-2">
          <div
            v-if="isSendingCode"
            class="flex items-center justify-center gap-2 h-14 text-muted-foreground"
          >
            <svg
              class="animate-spin h-5 w-5"
              xmlns="http://www.w3.org/2000/svg"
              fill="none"
              viewBox="0 0 24 24"
            >
              <circle
                class="opacity-25"
                cx="12"
                cy="12"
                r="10"
                stroke="currentColor"
                stroke-width="4"
              />
              <path
                class="opacity-75"
                fill="currentColor"
                d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"
              />
            </svg>
            <span class="text-sm">正在发送验证码...</span>
          </div>
          <template v-else>
            <input
              v-for="(_, index) in 6"
              :key="index"
              :ref="(el) => emit('set-code-input-ref', index, el as HTMLInputElement | null)"
              :value="codeDigits[index]"
              type="text"
              inputmode="numeric"
              maxlength="1"
              autocomplete="off"
              data-form-type="other"
              class="w-12 h-14 text-center text-xl font-semibold border-2 rounded-lg bg-background transition-all focus:outline-none focus:ring-2 focus:ring-primary/20"
              :class="verificationError ? 'border-destructive' : 'border-border focus:border-primary'"
              :disabled="emailVerified"
              @input="emit('code-input', index, $event)"
              @keydown="emit('code-keydown', index, $event)"
              @paste="emit('code-paste', $event)"
            >
          </template>
        </div>
      </div>

      <div class="space-y-2">
        <Label for="reg-uname">用户名 <span class="text-destructive">*</span></Label>
        <Input
          id="reg-uname"
          :model-value="username"
          type="text"
          placeholder="请输入用户名"
          required
          disable-autofill
          :disabled="isLoading"
          :class="usernameError ? 'border-destructive' : ''"
          @update:model-value="emit('update:username', $event)"
        />
        <p
          v-if="usernameError"
          class="text-xs text-destructive"
        >
          {{ usernameError }}
        </p>
      </div>

      <div class="space-y-2">
        <Label :for="`pwd-${formNonce}`">密码 <span class="text-destructive">*</span></Label>
        <Input
          :id="`pwd-${formNonce}`"
          :model-value="password"
          type="text"
          autocomplete="one-time-code"
          data-form-type="other"
          data-lpignore="true"
          data-1p-ignore="true"
          :name="`pwd-${formNonce}`"
          placeholder="至少 8 个字符，包含大小写字母和数字"
          required
          class="-webkit-text-security-disc"
          :disabled="isLoading"
          @update:model-value="emit('update:password', $event)"
        />
        <p
          class="text-xs"
          :class="passwordHintIsError ? 'text-destructive' : 'text-muted-foreground'"
        >
          {{ passwordHintText }}
        </p>
      </div>

      <div class="space-y-2">
        <Label :for="`pwd-confirm-${formNonce}`">确认密码 <span class="text-destructive">*</span></Label>
        <Input
          :id="`pwd-confirm-${formNonce}`"
          :model-value="confirmPassword"
          type="text"
          autocomplete="one-time-code"
          data-form-type="other"
          data-lpignore="true"
          data-1p-ignore="true"
          :name="`pwd-confirm-${formNonce}`"
          placeholder="再次输入密码"
          required
          class="-webkit-text-security-disc"
          :disabled="isLoading"
          @update:model-value="emit('update:confirmPassword', $event)"
        />
      </div>

      <div
        v-if="showFooterActions"
        class="flex flex-col-reverse sm:flex-row sm:justify-end gap-3 pt-2"
      >
        <Button
          type="button"
          variant="outline"
          class="w-full sm:w-auto border-slate-200 dark:border-slate-600 text-slate-500 dark:text-slate-400 hover:text-primary hover:border-primary/50 hover:bg-primary/5 dark:hover:text-primary dark:hover:border-primary/50 dark:hover:bg-primary/10"
          :disabled="isLoading"
          @click="emit('cancel')"
        >
          取消
        </Button>
        <Button
          type="submit"
          class="w-full sm:w-auto bg-primary hover:bg-primary/90 text-white border-0"
          :disabled="isLoading || !canSubmit"
        >
          {{ isLoading ? loadingText : '注册' }}
        </Button>
      </div>
    </form>

    <div class="text-center text-sm">
      已有账户？
      <Button
        variant="link"
        class="h-auto p-0"
        @click="emit('switch-to-login')"
      >
        立即登录
      </Button>
    </div>
  </div>
</template>

<script setup lang="ts">
import Button from '@/components/ui/button.vue'
import Input from '@/components/ui/input.vue'
import Label from '@/components/ui/label.vue'

interface Props {
  emailConfigured: boolean
  requireEmailVerification: boolean
  showFooterActions?: boolean
  isLoading: boolean
  loadingText: string
  isSendingCode: boolean
  emailVerified: boolean
  verificationError: boolean
  canSendCode: boolean
  sendCodeButtonText: string
  usernameError: string
  passwordHintText: string
  passwordHintIsError: boolean
  canSubmit: boolean
  formNonce: string
  email: string
  username: string
  password: string
  confirmPassword: string
  codeDigits: string[]
}

withDefaults(defineProps<Props>(), {
  showFooterActions: false
})

const emit = defineEmits<{
  'update:email': [value: string]
  'update:username': [value: string]
  'update:password': [value: string]
  'update:confirmPassword': [value: string]
  'set-code-input-ref': [index: number, element: HTMLInputElement | null]
  'code-input': [index: number, event: Event]
  'code-keydown': [index: number, event: KeyboardEvent]
  'code-paste': [event: ClipboardEvent]
  'send-code': []
  'submit': []
  'cancel': []
  'switch-to-login': []
}>()
</script>
