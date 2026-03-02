<template>
  <Dialog
    v-model="isOpen"
    size="md"
    no-padding
  >
    <LoginForm
      :site-name="siteName"
      :is-demo="isDemo"
      :allow-registration="allowRegistration"
      :oauth-providers="oauthProviders"
      :show-auth-type-tabs="showAuthTypeTabs"
      :auth-type="authType"
      :ldap-exclusive="ldapExclusive"
      :email-label="emailLabel"
      :loading="authStore.loading"
      :form-email="form.email"
      :form-password="form.password"
      @update:auth-type="authType = $event"
      @update:form-email="form.email = $event"
      @update:form-password="form.password = $event"
      @fill-demo-account="fillDemoAccount"
      @submit="handleSubmit"
      @switch-to-register="handleSwitchToRegister"
      @oauth-login="handleOAuthLogin"
    />
  </Dialog>

  <RegisterDialog
    v-model:open="showRegisterDialog"
    :require-email-verification="requireEmailVerification"
    :email-configured="emailConfigured"
    @success="handleRegisterSuccess"
    @switch-to-login="handleSwitchToLogin"
  />
</template>

<script setup lang="ts">
import { onMounted, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { Dialog } from '@/components/ui'
import { useToast } from '@/composables/useToast'
import { useSiteInfo } from '@/composables/useSiteInfo'
import { getApiUrl } from '@/utils/url'
import RegisterDialog from './RegisterDialog.vue'
import LoginForm from './LoginForm.vue'
import { useAuthSettings } from '../composables/useAuthSettings'
import { useLoginForm } from '../composables/useLoginForm'

const props = defineProps<{
  modelValue: boolean
}>()

const emit = defineEmits<{
  'update:modelValue': [value: boolean]
}>()

const router = useRouter()
const { success: showSuccess } = useToast()
const { siteName } = useSiteInfo()

const isOpen = ref(props.modelValue)
const showRegisterDialog = ref(false)

const {
  allowRegistration,
  requireEmailVerification,
  emailConfigured,
  localEnabled,
  ldapEnabled,
  ldapExclusive,
  oauthProviders,
  loadSettings,
  showAuthTypeTabs,
  resolvedAuthType,
} = useAuthSettings()

const {
  authStore,
  form,
  authType,
  isDemo,
  emailLabel,
  fillDemoAccount,
  resetForm,
  handleLogin,
} = useLoginForm({
  localEnabled,
  ldapEnabled,
  ldapExclusive,
  resolvedAuthType,
})

watch(
  () => props.modelValue,
  (val) => {
    isOpen.value = val
    if (!val) return

    resetForm()
    loadSettings()
  }
)

watch(isOpen, (val) => {
  emit('update:modelValue', val)
})

onMounted(() => {
  loadSettings()
})

async function handleSubmit() {
  const result = await handleLogin()
  if (!result.success) {
    return
  }

  isOpen.value = false

  setTimeout(() => {
    router.push(result.targetPath)
  }, 1000)
}

function handleOAuthLogin(providerType: string) {
  window.location.href = getApiUrl(`/api/oauth/${providerType}/authorize`)
}

function handleSwitchToRegister() {
  if (!allowRegistration.value) {
    return
  }

  isOpen.value = false
  showRegisterDialog.value = true
}

function handleRegisterSuccess() {
  showRegisterDialog.value = false
  showSuccess('注册成功！请登录')
  isOpen.value = true
}

function handleSwitchToLogin() {
  showRegisterDialog.value = false
  isOpen.value = true
}
</script>
