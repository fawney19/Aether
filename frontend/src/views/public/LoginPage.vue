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

    <Card class="w-full max-w-md overflow-hidden border border-border/70 bg-card/85 backdrop-blur shadow-xl">
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
    </Card>
  </div>
</template>

<script setup lang="ts">
import { onMounted } from 'vue'
import { useRouter } from 'vue-router'
import { Moon, Sun, SunMoon } from 'lucide-vue-next'
import { useToast } from '@/composables/useToast'
import { useSiteInfo } from '@/composables/useSiteInfo'
import { useDarkMode } from '@/composables/useDarkMode'
import { getApiUrl } from '@/utils/url'
import { Card } from '@/components/ui'
import LoginForm from '@/features/auth/components/LoginForm.vue'
import { useAuthSettings } from '@/features/auth/composables/useAuthSettings'
import { useLoginForm } from '@/features/auth/composables/useLoginForm'

const router = useRouter()
const { warning: showWarning } = useToast()
const { siteName } = useSiteInfo()
const { themeMode, toggleDarkMode } = useDarkMode()

const {
  allowRegistration,
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

onMounted(() => {
  resetForm()
  loadSettings()
})

async function handleSubmit() {
  const result = await handleLogin()
  if (!result.success) {
    return
  }

  router.push(result.targetPath)
}

function handleSwitchToRegister() {
  if (!allowRegistration.value) {
    showWarning('当前未开放注册，请联系管理员')
    return
  }

  router.push('/register')
}

function handleOAuthLogin(providerType: string) {
  window.location.href = getApiUrl(`/api/oauth/${providerType}/authorize`)
}
</script>
