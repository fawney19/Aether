import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { authApi, type User } from '@/api/auth'
import apiClient from '@/api/client'
import { log } from '@/utils/logger'
import { parseApiError } from '@/utils/errorParser'
import { getErrorStatus } from '@/types/api-error'

export const useAuthStore = defineStore('auth', () => {
  const CURRENT_USER_FAILURE_BACKOFF_MS = 15_000

  // 初始化时从 localStorage 恢复 token
  const storedToken = apiClient.getToken()

  const user = ref<User | null>(null)
  const token = ref<string | null>(storedToken)
  const loading = ref(false)
  const error = ref<string | null>(null)
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
    // 使用 store 中的 token 状态判断认证状态
    // 如果需要同步 localStorage，应该在 checkAuth 或专门的 syncToken 方法中处理
    return !!token.value
  })

  /**
   * 同步 localStorage 中的 token 到 store
   * 用于处理多标签页或外部 token 变更的情况
   */
  function syncToken() {
    const currentToken = apiClient.getToken()
    if (token.value !== currentToken) {
      token.value = currentToken
      markAuthStateChanged()
    }
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
    await authApi.logout()
  }

  function applyExternalLogout() {
    user.value = null
    token.value = null
    error.value = null
    markAuthStateChanged()
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
      } finally {
        if (fetchCurrentUserPromise === request) {
          fetchCurrentUserPromise = null
          fetchCurrentUserToken = null
        }
      }
    })()

    fetchCurrentUserPromise = request
    return request
  }

  async function checkAuth() {
    syncToken()
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
    fetchCurrentUser,
    checkAuth,
    syncToken
  }
})
