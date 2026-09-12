import axios, { getAdapter } from 'axios'
import type { AxiosInstance, AxiosRequestConfig, AxiosResponse, InternalAxiosRequestConfig, AxiosAdapter } from 'axios'
import { NETWORK_CONFIG, AUTH_CONFIG } from '@/config/constants'
import { isDemoMode } from '@/config/demo'
import { getClientDeviceId } from '@/utils/deviceId'
import { CrossTabRefreshCoordinator } from '@/utils/crossTabRefresh'
import { log } from '@/utils/logger'
import { cache } from '@/utils/cache'

// 在开发环境下使用代理,生产环境使用环境变量
const API_BASE_URL = import.meta.env.VITE_API_URL || ''
export const AUTH_STATE_CHANGE_EVENT = 'aether-auth-state-change'
export const AUTH_SESSION_SIGNAL_KEY = 'aether_auth_session_signal'

export type AuthStateChangeDetail = {
  authenticated: boolean
}

export type AuthSessionSignal = AuthStateChangeDetail & {
  eventId: string
  emittedAt: number
}

type MockRuntime = typeof import('@/mocks')

let mockRuntimePromise: Promise<MockRuntime> | null = null
let currentMockUserToken: string | null = null

function loadMockRuntime(): Promise<MockRuntime> {
  if (!mockRuntimePromise) {
    mockRuntimePromise = import('@/mocks').catch((error) => {
      mockRuntimePromise = null
      throw error
    })
  }
  return mockRuntimePromise
}

/**
 * 判断请求是否为公共端点
 */
function isPublicEndpoint(url?: string, method?: string): boolean {
  if (!url) return false

  const isHealthCheck = url.includes('/health') &&
                       method?.toLowerCase() === 'get' &&
                       !url.includes('/api/admin')

  return url.includes('/public') ||
         url.includes('.json') ||
         isHealthCheck
}

/**
 * 判断是否为认证相关请求
 */
function isAuthRequest(url?: string): boolean {
  return url?.includes('/auth/login') || url?.includes('/auth/refresh') || url?.includes('/auth/logout') || false
}

function isProtectedOperationalEndpoint(url?: string): boolean {
  if (!url) return false
  const path = url.split('?', 1)[0]
  return path === '/_gateway/metrics' ||
         path.startsWith('/_gateway/audit/') ||
         path.startsWith('/_gateway/async-tasks/')
}

/**
 * 判断 403 错误是否表示用户账号级别的问题（需要清除认证并跳转）
 */
function isAccountLevelForbidden(status: number, errorDetail: string): boolean {
  if (status !== 403) return false
  const accountErrors = [
    '用户不存在或已禁用',
    '用户已禁用',
  ]
  return accountErrors.some((msg) => errorDetail.includes(msg))
}

/**
 * 创建 Demo 模式的自定义 adapter
 * 在 Demo 模式下拦截请求并返回 mock 数据
 */
function createDemoAdapter(defaultAdapter: AxiosAdapter) {
  return async (config: InternalAxiosRequestConfig): Promise<AxiosResponse> => {
    if (isDemoMode()) {
      try {
        const mockRuntime = await loadMockRuntime()
        mockRuntime.setMockUserToken(currentMockUserToken)
        const mockResponse = await mockRuntime.handleMockRequest({
          method: config.method?.toUpperCase(),
          url: config.url,
          data: config.data,
          params: config.params,
        })
        if (mockResponse) {
          // 确保响应包含 config
          mockResponse.config = config
          return mockResponse
        }
      } catch (error: unknown) {
        // Mock 错误需要附加 config，否则 handleResponseError 会崩溃
        if (axios.isAxiosError(error)) {
          error.config = config
          if (error.response) {
            error.response.config = config
          }
        }
        throw error
      }
    }
    // 非 Demo 模式或没有 mock 响应时，使用默认 adapter
    return defaultAdapter(config)
  }
}

class ApiClient {
  private client: AxiosInstance
  private token: string | null = null
  private authStateVersion = 0
  private isRefreshing = false
  private refreshPromise: Promise<string> | null = null
  private readonly refreshCoordinator = new CrossTabRefreshCoordinator()

  constructor() {
    this.client = axios.create({
      baseURL: API_BASE_URL,
      timeout: NETWORK_CONFIG.API_TIMEOUT,
      withCredentials: true,
      headers: {
        'Content-Type': 'application/json',
      },
    })

    // 设置自定义 adapter 处理 Demo 模式
    const defaultAdapter = getAdapter(this.client.defaults.adapter)
    this.client.defaults.adapter = createDemoAdapter(defaultAdapter)

    this.setupInterceptors()
    this.purgeLegacyStoredTokens()
  }

  /**
   * 配置请求和响应拦截器
   */
  private setupInterceptors(): void {
    // 请求拦截器 - 仅处理认证
    this.client.interceptors.request.use(
      (config) => {
        const carriesSessionCredentials = config.url?.includes('/api/') ||
          isProtectedOperationalEndpoint(config.url)

        if (carriesSessionCredentials) {
          config.headers['X-Client-Device-Id'] = getClientDeviceId()
        }

        const requiresAuth = !isPublicEndpoint(config.url, config.method) &&
                           carriesSessionCredentials

        if (requiresAuth) {
          const token = this.getToken()
          if (token) {
            config.headers.Authorization = `Bearer ${token}`
          }
        }
        return config
      },
      (error) => Promise.reject(error)
    )

    // 响应拦截器
    this.client.interceptors.response.use(
      (response) => response,
      async (error) => this.handleResponseError(error)
    )
  }

  private emitAuthStateChange(authenticated: boolean): void {
    if (typeof window === 'undefined') {
      return
    }
    window.dispatchEvent(
      new CustomEvent<AuthStateChangeDetail>(AUTH_STATE_CHANGE_EVENT, {
        detail: { authenticated },
      })
    )
  }

  private publishAuthSessionSignal(authenticated: boolean): void {
    if (typeof window === 'undefined') {
      return
    }

    const signal: AuthSessionSignal = {
      authenticated,
      eventId: typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function'
        ? crypto.randomUUID()
        : `${Date.now()}-${Math.random().toString(36).slice(2)}`,
      emittedAt: Date.now(),
    }

    try {
      window.localStorage.setItem(AUTH_SESSION_SIGNAL_KEY, JSON.stringify(signal))
    } catch {
      // Cross-tab notification is best effort. The HttpOnly cookie remains the
      // source of truth when another tab starts or makes its next request.
    }
  }

  private purgeLegacyStoredTokens(): void {
    if (typeof window === 'undefined') {
      return
    }
    for (const storage of [window.localStorage, window.sessionStorage]) {
      try {
        storage.removeItem('access_token')
      } catch {
        // Storage may be disabled; the token still only lives in memory.
      }
    }
  }

  /**
   * 处理响应错误
   */
  private async handleResponseError(error: unknown): Promise<AxiosResponse> {
    // 请求被取消
    if (axios.isCancel(error)) {
      return Promise.reject(error)
    }

    if (!axios.isAxiosError(error)) {
      return Promise.reject(error)
    }

    const originalRequest = error.config

    // 网络错误或服务器不可达
    if (!error.response) {
      log.warn('Network error or server unreachable', error.message)
      return Promise.reject(error)
    }

    // 认证请求错误,直接返回
    if (isAuthRequest(originalRequest?.url)) {
      return Promise.reject(error)
    }

    const status = error.response?.status ?? 0

    // 处理 403 用户账号级别错误（被禁用/删除）
    if (status === 403) {
      const rawDetail = (error.response?.data as Record<string, unknown>)?.detail
      const errorDetail = typeof rawDetail === 'string' ? rawDetail : ''
      if (isAccountLevelForbidden(status, errorDetail)) {
        log.info('User account issue detected, clearing auth', { errorDetail })
        this.clearAuth()
        window.location.href = '/'
        return Promise.reject(error)
      }
    }

    // 处理401错误
    if (status === 401) {
      return this.handle401Error(error, originalRequest)
    }

    return Promise.reject(error)
  }

  /**
   * 处理401认证错误
   */
  private async handle401Error(error: import('axios').AxiosError, originalRequest: InternalAxiosRequestConfig & { _retry?: boolean; _retryCount?: number } | undefined): Promise<AxiosResponse> {
    // 如果不需要认证,直接返回错误
    if (isPublicEndpoint(originalRequest?.url, originalRequest?.method)) {
      return Promise.reject(error)
    }

    // 如果已经重试过,不再重试
    if (!originalRequest || originalRequest._retry) {
      return Promise.reject(error)
    }

    log.debug('Got 401 error, attempting token refresh')

    // 标记为已重试
    originalRequest._retry = true
    originalRequest._retryCount = (originalRequest._retryCount || 0) + 1

    // 超过最大重试次数
    if (originalRequest._retryCount > AUTH_CONFIG.MAX_RETRY_COUNT) {
      log.error('Max retry attempts reached')
      return Promise.reject(error)
    }

    // 如果正在刷新,等待刷新完成
    if (this.isRefreshing) {
      try {
        const accessToken = await this.refreshPromise
        originalRequest.headers.Authorization = `Bearer ${accessToken}`
        return this.client.request(originalRequest)
      } catch {
        return Promise.reject(error)
      }
    }

    // 开始刷新token
    return this.refreshTokenAndRetry(originalRequest, error)
  }

  /**
   * 刷新token并重试原始请求
   */
  private async refreshTokenAndRetry(
    originalRequest: InternalAxiosRequestConfig,
    originalError: import('axios').AxiosError
  ): Promise<AxiosResponse> {
    try {
      const accessToken = await this.restoreSession()

      // 重试原始请求
      originalRequest.headers.Authorization = `Bearer ${accessToken}`
      return this.client.request(originalRequest)
    } catch (refreshError: unknown) {
      log.error('Token refresh failed', refreshError instanceof Error ? refreshError.message : String(refreshError))
      const status = axios.isAxiosError(refreshError) ? refreshError.response?.status : undefined
      // Network errors and refresh-rotation conflicts do not prove that the
      // current access token or another tab's newly rotated session is invalid.
      // Only an authoritative refresh rejection signs the browser out.
      if (status === 401 || status === 403) {
        this.clearAuth()
      }
      return Promise.reject(originalError)
    }
  }

  private async coordinatedRefresh(): Promise<string> {
    return this.refreshCoordinator.run(async () => {
      const response = await this.refreshToken()
      const accessToken = response.data.access_token
      if (!accessToken) {
        throw new Error('Refresh response missing access token')
      }
      return accessToken
    })
  }

  private syncTokenState(token: string | null): void {
    if (this.token !== token) {
      cache.clear()
    }
    this.token = token
    currentMockUserToken = token
  }

  setToken(token: string, notifyOtherTabs = false): void {
    this.purgeLegacyStoredTokens()
    this.authStateVersion += 1
    if (this.token === token) {
      cache.clear()
    }
    this.syncTokenState(token)
    this.emitAuthStateChange(true)
    if (notifyOtherTabs) {
      this.publishAuthSessionSignal(true)
    }
  }

  getToken(): string | null {
    return this.token
  }

  clearAuth(notifyOtherTabs = true, emitLocalEvent = true): void {
    const hadAuth = this.token !== null
    this.authStateVersion += 1
    this.syncTokenState(null)
    this.purgeLegacyStoredTokens()
    if (emitLocalEvent && hadAuth) {
      this.emitAuthStateChange(false)
    }
    if (notifyOtherTabs) {
      this.publishAuthSessionSignal(false)
    }
  }

  async restoreSession(notifyOtherTabs = false): Promise<string> {
    if (this.refreshPromise) {
      return this.refreshPromise
    }

    this.isRefreshing = true
    const requestAuthStateVersion = this.authStateVersion
    const restorePromise = (async () => {
      const accessToken = await this.coordinatedRefresh()
      if (requestAuthStateVersion !== this.authStateVersion) {
        throw new Error('Auth state changed during session restore')
      }
      this.setToken(accessToken, notifyOtherTabs)
      return accessToken
    })().finally(() => {
      if (this.refreshPromise === restorePromise) {
        this.refreshPromise = null
        this.isRefreshing = false
      }
    })
    this.refreshPromise = restorePromise
    return restorePromise
  }

  async refreshToken(): Promise<AxiosResponse> {
    return this.client.post('/api/auth/refresh')
  }

  // 以下方法直接委托给 axios client，Demo 模式由 adapter 统一处理
  async request<T = unknown>(config: AxiosRequestConfig): Promise<AxiosResponse<T>> {
    return this.client.request<T>(config)
  }

  async get<T = unknown>(url: string, config?: AxiosRequestConfig): Promise<AxiosResponse<T>> {
    return this.client.get<T>(url, config)
  }

  async post<T = unknown>(url: string, data?: unknown, config?: AxiosRequestConfig): Promise<AxiosResponse<T>> {
    return this.client.post<T>(url, data, config)
  }

  async put<T = unknown>(url: string, data?: unknown, config?: AxiosRequestConfig): Promise<AxiosResponse<T>> {
    return this.client.put<T>(url, data, config)
  }

  async patch<T = unknown>(url: string, data?: unknown, config?: AxiosRequestConfig): Promise<AxiosResponse<T>> {
    return this.client.patch<T>(url, data, config)
  }

  async delete<T = unknown>(url: string, config?: AxiosRequestConfig): Promise<AxiosResponse<T>> {
    return this.client.delete<T>(url, config)
  }
}

export function parseAuthSessionSignal(raw: string | null): AuthSessionSignal | null {
  if (!raw) return null
  try {
    const signal = JSON.parse(raw) as Partial<AuthSessionSignal>
    if (
      typeof signal.authenticated !== 'boolean' ||
      typeof signal.eventId !== 'string' ||
      typeof signal.emittedAt !== 'number'
    ) {
      return null
    }
    return signal as AuthSessionSignal
  } catch {
    return null
  }
}

export default new ApiClient()
