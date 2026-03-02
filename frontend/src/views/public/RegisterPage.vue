<template>
  <div class="min-h-screen literary-grid literary-paper flex items-center justify-center px-4 py-16 sm:px-6">
    <div class="fixed right-4 top-4 z-20 flex items-center gap-2 sm:right-6 sm:top-6">
      <RouterLink
        to="/"
        class="inline-flex h-10 items-center rounded-xl border border-border/60 bg-card/70 px-4 text-sm text-foreground backdrop-blur hover:border-primary/40 hover:text-primary transition"
      >
        返回首页
      </RouterLink>
      <button
        class="flex h-10 w-10 items-center justify-center rounded-xl border border-border/60 bg-card/70 text-muted-foreground backdrop-blur hover:text-foreground hover:border-primary/40 transition"
        :title="themeMode === 'system' ? '跟随系统' : themeMode === 'dark' ? '深色模式' : '浅色模式'"
        @click="toggleDarkMode"
      >
        <SunMoon
          v-if="themeMode === 'system'"
          class="h-4 w-4"
        />
        <Sun
          v-else-if="themeMode === 'light'"
          class="h-4 w-4"
        />
        <Moon
          v-else
          class="h-4 w-4"
        />
      </button>
    </div>

    <Card class="w-full max-w-lg overflow-hidden border border-border/70 bg-card/85 backdrop-blur shadow-xl p-6 sm:p-8">
      <RegisterForm
        :email-configured="emailConfigured"
        :require-email-verification="requireEmailVerification"
        :show-footer-actions="true"
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
        @cancel="router.push('/login')"
        @switch-to-login="router.push('/login')"
      />
    </Card>
  </div>
</template>

<script setup lang="ts">
import { onMounted } from 'vue'
import { useRouter } from 'vue-router'
import { Moon, Sun, SunMoon } from 'lucide-vue-next'
import { Card } from '@/components/ui'
import { useDarkMode } from '@/composables/useDarkMode'
import { useToast } from '@/composables/useToast'
import RegisterForm from '@/features/auth/components/RegisterForm.vue'
import { useAuthSettings } from '@/features/auth/composables/useAuthSettings'
import { useRegisterForm } from '@/features/auth/composables/useRegisterForm'

const router = useRouter()
const { warning: showWarning } = useToast()
const { themeMode, toggleDarkMode } = useDarkMode()

const {
  allowRegistration,
  requireEmailVerification,
  emailConfigured,
  loadSettings,
} = useAuthSettings()

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
  requireEmailVerification,
  emailConfigured,
})

onMounted(async () => {
  resetForm()
  await loadSettings()

  if (!allowRegistration.value) {
    showWarning('当前未开放注册，请先登录')
    router.replace('/login')
  }
})

async function handleSubmit() {
  const success = await submitRegister()
  if (!success) {
    return
  }

  router.push('/login')
}
</script>
