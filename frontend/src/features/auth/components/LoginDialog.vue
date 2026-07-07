<template>
  <Dialog
    v-model="isOpen"
    size="md"
    no-padding
  >
    <div class="px-6 py-6 sm:px-8 sm:py-8">
      <!-- Logo 和标题 -->
      <div class="flex flex-col items-center text-center mb-8">
        <img
          src="/aether_adaptive.svg"
          :alt="siteName"
          class="h-16 w-16 mb-4"
        >
        <h2 class="text-2xl font-semibold text-foreground">
          {{ t('auth.login.title', { siteName }) }}
        </h2>
      </div>

      <!-- Demo 模式提示 -->
      <div
        v-if="isDemo"
        class="rounded-lg border border-primary/20 bg-primary/5 p-3 mb-5"
      >
        <p class="text-xs font-medium text-foreground mb-2">
          {{ t('auth.login.demoMode') }}
        </p>
        <div class="space-y-1.5">
          <button
            type="button"
            class="flex items-center gap-2 text-xs text-muted-foreground hover:text-foreground transition-colors w-full"
            @click="fillDemoAccount('admin')"
          >
            <span class="inline-flex items-center justify-center w-4 h-4 rounded bg-primary/20 text-primary text-[10px] font-bold">A</span>
            <span>admin@demo.aether.io / demo123</span>
          </button>
          <button
            type="button"
            class="flex items-center gap-2 text-xs text-muted-foreground hover:text-foreground transition-colors w-full"
            @click="fillDemoAccount('user')"
          >
            <span class="inline-flex items-center justify-center w-4 h-4 rounded bg-muted text-muted-foreground text-[10px] font-bold">U</span>
            <span>user@demo.aether.io / demo123</span>
          </button>
        </div>
      </div>

      <!-- OAuth 登录按钮 -->
      <div
        v-if="oauthProviders.length > 0"
        class="mb-5"
      >
        <!-- 单个 provider: 完整按钮 -->
        <div
          v-if="oauthProviders.length === 1"
          class="space-y-2"
        >
          <button
            type="button"
            class="oauth-btn"
            @click="handleOAuthLogin(oauthProviders[0].provider_type)"
          >
            <!-- eslint-disable vue/no-v-html -->
            <span
              class="oauth-icon"
              v-html="getOAuthIcon(oauthProviders[0].provider_type, oauthProviders[0].icon_url)"
            />
            <!-- eslint-enable vue/no-v-html -->
            <span>{{ t('auth.login.oauthWithProvider', { provider: oauthProviders[0].display_name }) }}</span>
          </button>
        </div>

        <!-- 多个 provider: 图标按钮组 -->
        <div
          v-else
          class="flex flex-col items-center gap-3"
        >
          <span class="text-xs text-muted-foreground">{{ t('auth.login.oauthOptions') }}</span>
          <div class="flex items-center justify-center gap-3">
            <button
              v-for="p in oauthProviders"
              :key="p.provider_type"
              type="button"
              class="oauth-icon-btn"
              :title="p.display_name"
              @click="handleOAuthLogin(p.provider_type)"
            >
              <!-- eslint-disable vue/no-v-html -->
              <span
                class="oauth-icon-lg"
                v-html="getOAuthIcon(p.provider_type, p.icon_url)"
              />
              <!-- eslint-enable vue/no-v-html -->
            </button>
          </div>
        </div>
      </div>

      <!-- 分隔线 -->
      <div
        v-if="oauthProviders.length > 0"
        class="flex items-center gap-3 mb-5"
      >
        <div class="flex-1 h-px bg-border" />
        <span class="text-xs text-muted-foreground px-2">{{ t('auth.login.accountPassword') }}</span>
        <div class="flex-1 h-px bg-border" />
      </div>

      <!-- 认证方式切换 -->
      <div
        v-if="showAuthTypeTabs"
        class="auth-type-tabs mb-4"
      >
        <button
          type="button"
          class="auth-tab"
          :class="[authType === 'local' && 'active']"
          @click="authType = 'local'"
        >
          {{ t('auth.login.local') }}
        </button>
        <button
          type="button"
          class="auth-tab"
          :class="[authType === 'ldap' && 'active']"
          @click="authType = 'ldap'"
        >
          {{ t('auth.login.ldap') }}
        </button>
      </div>

      <!-- 登录表单 -->
      <form
        ref="loginFormEl"
        name="login"
        action="/api/auth/login"
        method="post"
        class="space-y-4"
        autocomplete="on"
        data-form-type="login"
        @submit.prevent="handleLogin"
      >
        <div class="space-y-1.5">
          <div class="flex items-center justify-between">
            <Label
              for="username"
              class="text-sm"
            >
              {{ emailLabel }}
            </Label>
            <button
              v-if="ldapExclusive && authType === 'ldap'"
              type="button"
              class="text-xs text-muted-foreground/60 hover:text-muted-foreground transition-colors"
              @click="authType = 'local'"
            >
              {{ t('auth.login.adminLocal') }}
            </button>
            <button
              v-if="ldapExclusive && authType === 'local'"
              type="button"
              class="text-xs text-muted-foreground/60 hover:text-muted-foreground transition-colors"
              @click="authType = 'ldap'"
            >
              {{ t('auth.login.backToLdap') }}
            </button>
          </div>
          <Input
            id="username"
            v-model="form.email"
            type="text"
            name="username"
            required
            :placeholder="t('auth.login.usernamePlaceholder')"
            autocomplete="username"
            autocapitalize="none"
            spellcheck="false"
            :disable-autofill="false"
          />
        </div>

        <div class="space-y-1.5">
          <Label
            for="password"
            class="text-sm"
          >
            {{ t('auth.login.password') }}
          </Label>
          <Input
            id="password"
            v-model="form.password"
            type="password"
            name="password"
            required
            :placeholder="t('auth.login.passwordPlaceholder')"
            autocomplete="current-password"
            :disable-autofill="false"
          />
        </div>

        <!-- 登录按钮 -->
        <Button
          type="submit"
          :disabled="authStore.loading"
          class="w-full h-12"
        >
          {{ authStore.loading ? t('auth.login.submitting') : t('auth.login.submit') }}
        </Button>

        <!-- 提示信息 -->
        <p
          v-if="!isDemo && !allowRegistration"
          class="text-xs text-muted-foreground text-center"
        >
          {{ t('auth.login.contactAdmin') }}
        </p>
      </form>

      <!-- 注册链接 -->
      <div
        v-if="allowRegistration"
        class="mt-5 pt-5 border-t border-border text-center text-sm text-muted-foreground"
      >
        {{ t('auth.login.noAccount') }}
        <button
          type="button"
          class="text-primary hover:text-primary/80 font-medium transition-colors"
          @click="handleSwitchToRegister"
        >
          {{ t('auth.login.registerNow') }}
        </button>
      </div>
    </div>
  </Dialog>

  <!-- Register Dialog -->
  <RegisterDialog
    v-model:open="showRegisterDialog"
    :require-email-verification="requireEmailVerification"
    :email-configured="emailConfigured"
    :password-policy-level="passwordPolicyLevel"
    :turnstile-enabled="turnstileEnabled"
    :turnstile-site-key="turnstileSiteKey"
    :privacy-policy="privacyPolicy"
    @success="handleRegisterSuccess"
    @switch-to-login="handleSwitchToLogin"
  />
</template>

<script setup lang="ts">
import { ref, watch, computed, onMounted } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { Dialog } from '@/components/ui'
import Button from '@/components/ui/button.vue'
import Input from '@/components/ui/input.vue'
import Label from '@/components/ui/label.vue'
import { useAuthStore } from '@/stores/auth'
import { useToast } from '@/composables/useToast'
import { useSiteInfo } from '@/composables/useSiteInfo'
import { normalizePasswordPolicyLevel, type PasswordPolicyLevel } from '@/utils/passwordPolicy'
import { isDemoMode, DEMO_ACCOUNTS } from '@/config/demo'
import RegisterDialog from './RegisterDialog.vue'
import { authApi, type RegistrationPrivacyPolicySettings } from '@/api/auth'
import { oauthApi, type OAuthProviderInfo } from '@/api/oauth'
import { getClientDeviceId } from '@/utils/deviceId'
import { getApiUrl } from '@/utils/url'
import { getOAuthIcon } from '@/utils/oauth-icons'
import { navigateAfterLogin } from '@/features/auth/utils/loginRedirect'
import { useI18n } from '@/i18n'

const props = defineProps<{
  modelValue: boolean
}>()

const emit = defineEmits<{
  'update:modelValue': [value: boolean]
}>()

const router = useRouter()
const route = useRoute()
const authStore = useAuthStore()
const { success: showSuccess, warning: showWarning, error: showError } = useToast()
const { siteName } = useSiteInfo()
const { t } = useI18n()

const isOpen = ref(props.modelValue)
const isDemo = computed(() => isDemoMode())
const showRegisterDialog = ref(false)
const requireEmailVerification = ref(false)
const emailConfigured = ref(true) // 邮箱服务是否已配置
const passwordPolicyLevel = ref<PasswordPolicyLevel>('weak')
const turnstileEnabled = ref(false)
const turnstileSiteKey = ref<string | null>(null)
const allowRegistration = ref(false) // 由系统配置控制，默认关闭
const privacyPolicy = ref<RegistrationPrivacyPolicySettings>({
  enabled: false,
  format: 'markdown',
  content: '',
  version: ''
})

// LDAP authentication settings
const PREFERRED_AUTH_TYPE_KEY = 'aether_preferred_auth_type'
function getStoredAuthType(): 'local' | 'ldap' {
  const stored = localStorage.getItem(PREFERRED_AUTH_TYPE_KEY)
  return (stored === 'ldap' || stored === 'local') ? stored : 'local'
}
const authType = ref<'local' | 'ldap'>(getStoredAuthType())
const localEnabled = ref(true)
const ldapEnabled = ref(false)
const ldapExclusive = ref(false)

const oauthProviders = ref<OAuthProviderInfo[]>([])
const loginFormEl = ref<HTMLFormElement | null>(null)

// 保存用户的认证类型偏好
watch(authType, (newType) => {
  localStorage.setItem(PREFERRED_AUTH_TYPE_KEY, newType)
})

const showAuthTypeTabs = computed(() => {
  return localEnabled.value && ldapEnabled.value && !ldapExclusive.value
})

const emailLabel = computed(() => {
  return t('auth.login.usernameEmail')
})

watch(() => props.modelValue, (val) => {
  isOpen.value = val
  // 打开对话框时重置表单
  if (val) {
    form.value = {
      email: '',
      password: ''
    }
  }
})

watch(isOpen, (val) => {
  emit('update:modelValue', val)
})

const form = ref({
  email: '',
  password: ''
})

function fillDemoAccount(type: 'admin' | 'user') {
  const account = DEMO_ACCOUNTS[type]
  form.value.email = account.email
  form.value.password = account.password
}

async function handleLogin(event?: Event) {
  const { email, password } = readCurrentLoginCredentials(event)

  if (!email || !password) {
    showWarning(t('auth.login.required'))
    return
  }

  const success = await authStore.login(email, password, authType.value)
  if (success) {
    const targetPath = consumeStoredRedirectPath() ?? (authStore.canAccessAdmin ? '/admin/dashboard' : '/dashboard')

    await navigateAfterLogin(router, targetPath)

    showSuccess(t('auth.login.successRedirecting'))

    // 关闭对话框
    isOpen.value = false
  } else {
    showError(authStore.error || t('auth.login.failed'))
  }
}

function readCurrentLoginCredentials(event?: Event): { email: string; password: string } {
  const formElement = event?.currentTarget instanceof HTMLFormElement
    ? event.currentTarget
    : loginFormEl.value

  const emailInput = formElement?.elements.namedItem('username')
  const passwordInput = formElement?.elements.namedItem('password')

  const email = emailInput instanceof HTMLInputElement
    ? emailInput.value.trim()
    : form.value.email.trim()
  const password = passwordInput instanceof HTMLInputElement
    ? passwordInput.value
    : form.value.password

  form.value.email = email
  form.value.password = password

  return { email, password }
}

function consumeStoredRedirectPath(): string | null {
  const redirectPath = sessionStorage.getItem('redirectPath')
  if (redirectPath) {
    sessionStorage.removeItem('redirectPath')
  }
  if (!redirectPath || redirectPath === '/' || !redirectPath.startsWith('/') || redirectPath.startsWith('//')) {
    return null
  }
  return redirectPath
}

function handleOAuthLogin(providerType: string) {
  // 如果 sessionStorage 中没有 redirectPath（用户直接点击登录而非被守卫拦截），
  // 则不设置，让 AuthCallback 使用默认跳转逻辑
  const authorizeUrl = new URL(
    getApiUrl(`/api/oauth/${providerType}/authorize`),
    window.location.origin,
  )
  authorizeUrl.searchParams.set('client_device_id', getClientDeviceId())
  window.location.href = authorizeUrl.toString()
}

function handleSwitchToRegister() {
  isOpen.value = false
  showRegisterDialog.value = true
}

function handleRegisterSuccess() {
  showRegisterDialog.value = false
  showSuccess(t('auth.register.successLogin'))
  isOpen.value = true
}

function handleSwitchToLogin() {
  showRegisterDialog.value = false
  isOpen.value = true
}

// Load authentication and registration settings on mount
onMounted(async () => {
  try {
    const [regSettings, authSettings, providers] = await Promise.all([
      authApi.getRegistrationSettings(),
      authApi.getAuthSettings(),
      oauthApi.getProviders().catch(() => []),
    ])

    allowRegistration.value = !!regSettings.enable_registration
    requireEmailVerification.value = !!regSettings.require_email_verification
    emailConfigured.value = !!regSettings.email_configured
    passwordPolicyLevel.value = normalizePasswordPolicyLevel(regSettings.password_policy_level)
    turnstileEnabled.value = !!regSettings.turnstile_enabled
    turnstileSiteKey.value = regSettings.turnstile_site_key || null
    privacyPolicy.value = regSettings.privacy_policy ?? {
      enabled: false,
      format: 'markdown',
      content: '',
      version: ''
    }

    localEnabled.value = authSettings.local_enabled
    ldapEnabled.value = authSettings.ldap_enabled
    ldapExclusive.value = authSettings.ldap_exclusive
    // 若仅允许 LDAP 登录，则禁用本地注册入口
    if (ldapExclusive.value) {
      allowRegistration.value = false
    }

    // Set default auth type based on settings
    if (authSettings.ldap_exclusive) {
      authType.value = 'ldap'
    } else if (!authSettings.local_enabled && authSettings.ldap_enabled) {
      authType.value = 'ldap'
    } else {
      authType.value = 'local'
    }

    oauthProviders.value = providers
    if (allowRegistration.value && (route.path === '/register' || typeof route.query.invite === 'string')) {
      isOpen.value = false
      showRegisterDialog.value = true
    }
  } catch {
    // If获取失败，保持默认：关闭注册 & 关闭邮箱验证 & 使用本地认证
    allowRegistration.value = false
    requireEmailVerification.value = false
    emailConfigured.value = false
    passwordPolicyLevel.value = 'weak'
    turnstileEnabled.value = false
    turnstileSiteKey.value = null
    privacyPolicy.value = {
      enabled: false,
      format: 'markdown',
      content: '',
      version: ''
    }
    localEnabled.value = true
    ldapEnabled.value = false
    ldapExclusive.value = false
    authType.value = 'local'
    oauthProviders.value = []
  }
})
</script>

<style scoped>
.oauth-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 0.75rem;
  width: 100%;
  padding: 0.625rem 1rem;
  font-size: 0.875rem;
  font-weight: 500;
  color: hsl(var(--foreground));
  background: hsl(var(--muted) / 0.5);
  border: 1px solid hsl(var(--border) / 0.6);
  border-radius: 0.75rem;
  cursor: pointer;
  transition: all 0.15s ease;
}

.oauth-btn:hover {
  background: hsl(var(--muted));
  border-color: hsl(var(--primary) / 0.5);
}

.oauth-icon {
  width: 1.25rem;
  height: 1.25rem;
  flex-shrink: 0;
}

.oauth-icon :deep(svg) {
  width: 100%;
  height: 100%;
}

.oauth-icon-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 3rem;
  height: 3rem;
  background: hsl(var(--muted) / 0.5);
  border: 1px solid hsl(var(--border) / 0.6);
  border-radius: 0.75rem;
  cursor: pointer;
  transition: all 0.15s ease;
}

.oauth-icon-btn:hover {
  background: hsl(var(--muted));
  border-color: hsl(var(--primary) / 0.5);
  transform: translateY(-1px);
}

.oauth-icon-lg {
  width: 1.5rem;
  height: 1.5rem;
}

.oauth-icon-lg :deep(svg) {
  width: 100%;
  height: 100%;
}

.auth-type-tabs {
  display: flex;
  border-bottom: 1px solid hsl(var(--border));
}

.auth-tab {
  flex: 1;
  padding: 0.5rem 1rem;
  font-size: 0.875rem;
  font-weight: 500;
  color: hsl(var(--muted-foreground));
  background: transparent;
  border: none;
  cursor: pointer;
  transition: color 0.15s ease;
  position: relative;
}

.auth-tab::after {
  content: '';
  position: absolute;
  bottom: -1px;
  left: 0;
  right: 0;
  height: 2px;
  background: transparent;
  transition: background 0.15s ease;
}

.auth-tab:hover:not(.active) {
  color: hsl(var(--foreground));
}

.auth-tab.active {
  color: hsl(var(--primary));
  font-weight: 600;
}

.auth-tab.active::after {
  background: hsl(var(--primary));
}
</style>
