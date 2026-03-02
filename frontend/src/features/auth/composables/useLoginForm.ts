import { computed, ref, watch, type Ref } from 'vue'
import { useAuthStore } from '@/stores/auth'
import { useToast } from '@/composables/useToast'
import { isDemoMode, DEMO_ACCOUNTS } from '@/config/demo'

const PREFERRED_AUTH_TYPE_KEY = 'aether_preferred_auth_type'

function getStoredAuthType(): 'local' | 'ldap' {
  const stored = localStorage.getItem(PREFERRED_AUTH_TYPE_KEY)
  return stored === 'ldap' || stored === 'local' ? stored : 'local'
}

function consumeRedirectPath(): string | null {
  const redirectPath = sessionStorage.getItem('redirectPath')
  if (!redirectPath) {
    return null
  }

  sessionStorage.removeItem('redirectPath')

  if (redirectPath === '/' || redirectPath === '/login' || redirectPath === '/register') {
    return null
  }

  return redirectPath
}

interface UseLoginFormOptions {
  localEnabled: Ref<boolean>
  ldapEnabled: Ref<boolean>
  ldapExclusive: Ref<boolean>
  resolvedAuthType: Ref<'local' | 'ldap'>
}

export interface LoginResult {
  success: boolean
  targetPath: string
}

export function useLoginForm(options: UseLoginFormOptions) {
  const authStore = useAuthStore()
  const { success: showSuccess, warning: showWarning, error: showError } = useToast()

  const form = ref({
    email: '',
    password: ''
  })

  const authType = ref<'local' | 'ldap'>(getStoredAuthType())
  const isDemo = computed(() => isDemoMode())
  const emailLabel = computed(() => '用户名/邮箱')

  const normalizeAuthType = (preferred: 'local' | 'ldap'): 'local' | 'ldap' => {
    if (options.ldapExclusive.value) {
      return 'ldap'
    }

    if (preferred === 'local' && !options.localEnabled.value) {
      if (options.ldapEnabled.value) return 'ldap'
      return options.resolvedAuthType.value
    }

    if (preferred === 'ldap' && !options.ldapEnabled.value) {
      if (options.localEnabled.value) return 'local'
      return options.resolvedAuthType.value
    }

    return preferred
  }

  authType.value = normalizeAuthType(authType.value)

  watch(authType, (newType) => {
    localStorage.setItem(PREFERRED_AUTH_TYPE_KEY, newType)
  })

  watch(
    [options.localEnabled, options.ldapEnabled, options.ldapExclusive, options.resolvedAuthType],
    () => {
      authType.value = normalizeAuthType(authType.value)
    },
    { immediate: true }
  )

  function fillDemoAccount(type: 'admin' | 'user') {
    const account = DEMO_ACCOUNTS[type]
    form.value.email = account.email
    form.value.password = account.password
  }

  function resetForm() {
    form.value = {
      email: '',
      password: ''
    }
  }

  async function handleLogin(): Promise<LoginResult> {
    if (!form.value.email || !form.value.password) {
      showWarning('请输入邮箱和密码')
      return {
        success: false,
        targetPath: ''
      }
    }

    const loginSuccess = await authStore.login(form.value.email, form.value.password, authType.value)
    if (!loginSuccess) {
      showError(authStore.error || '登录失败，请检查邮箱和密码')
      return {
        success: false,
        targetPath: ''
      }
    }

    showSuccess('登录成功，正在跳转...')

    const redirectPath = consumeRedirectPath()
    const defaultTarget = authStore.user?.role === 'admin' ? '/admin/dashboard' : '/dashboard'

    return {
      success: true,
      targetPath: redirectPath || defaultTarget
    }
  }

  return {
    authStore,
    form,
    authType,
    isDemo,
    emailLabel,
    fillDemoAccount,
    resetForm,
    handleLogin,
  }
}
