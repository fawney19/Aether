<template>
  <Dialog
    v-model:open="isOpen"
    size="lg"
  >
    <RegisterForm
      :email-configured="emailConfigured"
      :require-email-verification="requireEmailVerification"
      :is-loading="isLoading"
      :loading-text="loadingText"
      :is-sending-code="isSendingCode"
      :email-verified="emailVerified"
      :verification-error="verificationError"
      :can-send-code="canSendCode"
      :send-code-button-text="sendCodeButtonText"
      :username-error="usernameError"
      :password-hint-text="passwordHintText"
      :password-hint-is-error="passwordHintIsError"
      :can-submit="canSubmit"
      :form-nonce="formNonce"
      :email="formData.email"
      :username="formData.username"
      :password="formData.password"
      :confirm-password="formData.confirmPassword"
      :code-digits="codeDigits"
      @update:email="formData.email = $event"
      @update:username="formData.username = $event"
      @update:password="formData.password = $event"
      @update:confirm-password="formData.confirmPassword = $event"
      @set-code-input-ref="setCodeInputRef"
      @code-input="handleCodeInput"
      @code-keydown="handleCodeKeyDown"
      @code-paste="handleCodePaste"
      @send-code="handleSendCode"
      @submit="handleSubmit"
      @switch-to-login="handleSwitchToLogin"
    />

    <template #footer>
      <Button
        type="button"
        variant="outline"
        class="w-full sm:w-auto border-slate-200 dark:border-slate-600 text-slate-500 dark:text-slate-400 hover:text-primary hover:border-primary/50 hover:bg-primary/5 dark:hover:text-primary dark:hover:border-primary/50 dark:hover:bg-primary/10"
        :disabled="isLoading"
        @click="handleCancel"
      >
        取消
      </Button>
      <Button
        class="w-full sm:w-auto bg-primary hover:bg-primary/90 text-white border-0"
        :disabled="isLoading || !canSubmit"
        @click="handleSubmit"
      >
        {{ isLoading ? loadingText : '注册' }}
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { computed, toRef, watch } from 'vue'
import { Dialog } from '@/components/ui'
import Button from '@/components/ui/button.vue'
import RegisterForm from './RegisterForm.vue'
import { useRegisterForm } from '../composables/useRegisterForm'

interface Props {
  open?: boolean
  requireEmailVerification?: boolean
  emailConfigured?: boolean
}

interface Emits {
  (e: 'update:open', value: boolean): void
  (e: 'success'): void
  (e: 'switch-to-login'): void
}

const props = withDefaults(defineProps<Props>(), {
  open: false,
  requireEmailVerification: false,
  emailConfigured: true
})

const emit = defineEmits<Emits>()

const isOpen = computed({
  get: () => props.open,
  set: (value) => emit('update:open', value)
})

const {
  formNonce,
  formData,
  codeDigits,
  emailVerified,
  isLoading,
  loadingText,
  isSendingCode,
  verificationError,
  canSendCode,
  sendCodeButtonText,
  usernameError,
  passwordHintText,
  passwordHintIsError,
  canSubmit,
  setCodeInputRef,
  handleCodeInput,
  handleCodeKeyDown,
  handleCodePaste,
  handleSendCode,
  handleSubmit: submitRegister,
  resetForm,
} = useRegisterForm({
  requireEmailVerification: toRef(props, 'requireEmailVerification'),
  emailConfigured: toRef(props, 'emailConfigured'),
})

watch(isOpen, (newValue) => {
  if (newValue) {
    resetForm()
  }
})

async function handleSubmit() {
  const success = await submitRegister()
  if (!success) {
    return
  }

  emit('success')
  isOpen.value = false
}

function handleCancel() {
  isOpen.value = false
}

function handleSwitchToLogin() {
  emit('switch-to-login')
  isOpen.value = false
}
</script>
