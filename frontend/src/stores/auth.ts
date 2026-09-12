import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { authApi, type User } from '@/api/auth'
import apiClient from '@/api/client'
import { log } from '@/utils/logger'
import { parseApiError } from '@/utils/errorParser'
import { getErrorStatus } from '@/types/api-error'

export const useAuthStore = defineStore('auth', () => {
  const CURRENT_USER_FAILURE_BACKOFF_MS = 15_000

  const user = ref<User | null>(null)
  const token = ref<string | null>(apiClient.getToken())
  const loading = ref(false)
  const error = ref<string | null>(null)
  let sessionRestoreAttempted = false
  let sessionRestorePromise: Promise<boolean> | null = null
  let fetchCurrentUserPromise: Promise<User | null> | null = null
  let fetchCurrentUserToken: string | null = null
  let lastCurrentUserFailureAt = 0
  let lastCurrentUserFailureToken: string | null = null
  let authStateVersion = 0

  function resetCurrentUserFailure() {
    lastCurrentUserFailureAt = 0
    lastCurrentUserFailureToken = null
  }

  function markAuthStateChanged() {
    authStateVersion += 1
    resetCurrentUserFailure()
  }

  const isAuthenticated = computed(() => {
    // The access token only exists in this tab's memory.
    return !!token.value
  })

  /** Synchronize the store with the access token held in this tab's memory. */
  function syncToken() {
    const currentToken = apiClient.getToken()
    if (token.value !== currentToken) {
      token.value = currentToken
      markAuthStateChanged()
    }
  }

  async function restoreSession(force = false, notifyOtherTabs = false): Promise<boolean> {
    syncToken()
    if (token.value && !force) {
      return true
    }
    if (sessionRestorePromise) {
      return sessionRestorePromise
    }
    if (sessionRestoreAttempted && !force) {
      return false
    }

    sessionRestoreAttempted = true
    const requestAuthStateVersion = authStateVersion
    const requestToken = token.value
    const request = (async () => {
      try {
        const accessToken = await apiClient.restoreSession(notifyOtherTabs)
        if (requestAuthStateVersion !== authStateVersion) {
          return token.value === accessToken
        }
        token.value = accessToken
        markAuthStateChanged()
        return true
      } catch {
        if (requestAuthStateVersion === authStateVersion && token.value === requestToken) {
          // A forced refresh can race with another tab or fail transiently.
          // Preserve an already-issued in-memory access token; its next
          // authenticated request remains the authority on whether it is valid.
          if (!requestToken) {
            apiClient.clearAuth(false, false)
            token.value = null
            user.value = null
          }
        }
        return false
      }
    })().finally(() => {
      if (sessionRestorePromise === request) {
        sessionRestorePromise = null
      }
    })

    sessionRestorePromise = request
    return request
  }
  const isAdmin = computed(() => user.value?.role === 'admin')
  const isAuditAdmin = computed(() => user.value?.role === 'audit_admin')
  const canAccessAdmin = computed(() => isAdmin.value || isAuditAdmin.value)
  const canOperateAdmin = computed(() => isAdmin.value)

  async function login(email: string, password: string, authType: 'local' | 'ldap' = 'local') {
    loading.value = true
    error.value = null

    try {
      const response = await authApi.login({ email, password, auth_type: authType })
      token.value = response.access_token
      sessionRestoreAttempted = true
      markAuthStateChanged()

      // 获取用户信息
      const userInfo = await authApi.getCurrentUser()
      user.value = userInfo
      resetCurrentUserFailure()

      return true
    } catch (err: unknown) {
      // 不要暴露后端的详细错误信息
      const status = getErrorStatus(err)
      if (status === 401) {
        error.value = '邮箱或密码错误'
      } else if (status === 422) {
        error.value = '请输入有效的邮箱地址'
      } else if (status === 429) {
        // 限流错误，显示后端返回的具体信息
        error.value = parseApiError(err, '请求过于频繁,请稍后重试')
      } else if (status === 500) {
        error.value = '服务器错误,请稍后重试'
      } else {
        error.value = '登录失败,请检查网络连接'
      }
      return false
    } finally {
      loading.value = false
    }
  }

  async function logout() {
    user.value = null
    token.value = null
    markAuthStateChanged()
    sessionRestoreAttempted = true
    await authApi.logout()
  }

  function applyExternalLogout() {
    user.value = null
    token.value = null
    error.value = null
    markAuthStateChanged()
    sessionRestoreAttempted = true
    apiClient.clearAuth(false, false)
  }

  async function applyExternalLogin(): Promise<boolean> {
    user.value = null
    token.value = null
    error.value = null
    markAuthStateChanged()
    apiClient.clearAuth(false, false)
    sessionRestoreAttempted = false
    const restored = await restoreSession(true)
    if (!restored) {
      return false
    }
    await fetchCurrentUser()
    return !!user.value
  }

  function fetchCurrentUser(): Promise<User | null> {
    const requestToken = token.value || apiClient.getToken()
    if (!requestToken) {
      user.value = null
      return Promise.resolve(null)
    }

    // 路由守卫、App 初始化和认证同步可能同时触发，复用同一个请求。
    if (fetchCurrentUserPromise && fetchCurrentUserToken === requestToken) {
      return fetchCurrentUserPromise
    }

    // 后端暂时不可用时，不要让每一次导航都重新等待全局请求超时。
    if (
      lastCurrentUserFailureToken === requestToken &&
      Date.now() - lastCurrentUserFailureAt < CURRENT_USER_FAILURE_BACKOFF_MS
    ) {
      return Promise.resolve(null)
    }

    fetchCurrentUserToken = requestToken
    const requestAuthStateVersion = authStateVersion
    const request = (async () => {
      try {
        const userInfo = await authApi.getCurrentUser()
        if (requestAuthStateVersion !== authStateVersion || !token.value) {
          return null
        }
        user.value = userInfo
        resetCurrentUserFailure()
        return userInfo
      } catch (err: unknown) {
        log.error('Failed to fetch user info', err)
        if (requestAuthStateVersion !== authStateVersion) {
          return null
        }
        syncToken()
        if (requestAuthStateVersion !== authStateVersion) {
          if (!token.value) user.value = null
          return null
        }
        if (!token.value) {
          user.value = null
        } else {
          lastCurrentUserFailureAt = Date.now()
          lastCurrentUserFailureToken = token.value
        }
        // 保留登录状态；短暂退避后允许再次校验。
        log.info('Keeping session despite error, as per user requirement')
        return null
      }
    })().finally(() => {
      if (fetchCurrentUserPromise === request) {
        fetchCurrentUserPromise = null
        fetchCurrentUserToken = null
      }
    })

    fetchCurrentUserPromise = request
    return request
  }

  async function checkAuth() {
    await restoreSession()
    if (token.value && !user.value) {
      // 即使获取用户信息失败,也保留 token。
      await fetchCurrentUser()
    }
  }

  return {
    user,
    token,
    loading,
    error,
    isAuthenticated,
    isAdmin,
    isAuditAdmin,
    canAccessAdmin,
    canOperateAdmin,
    login,
    logout,
    applyExternalLogout,
    applyExternalLogin,
    fetchCurrentUser,
    checkAuth,
    restoreSession,
    syncToken
  }
})
