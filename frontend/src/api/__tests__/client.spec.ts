import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { AxiosAdapter, InternalAxiosRequestConfig } from 'axios'

import apiClient, {
  AUTH_SESSION_SIGNAL_KEY,
  AUTH_STATE_CHANGE_EVENT,
  parseAuthSessionSignal,
} from '@/api/client'
import { cache, cachedRequest } from '@/utils/cache'

describe('apiClient auth state change event', () => {
  beforeEach(() => {
    localStorage.clear()
    apiClient.clearAuth()
  })

  afterEach(() => {
    localStorage.clear()
    apiClient.clearAuth()
  })

  it('dispatches a same-tab auth change event when clearing auth', () => {
    const handler = vi.fn()
    window.addEventListener(AUTH_STATE_CHANGE_EVENT, handler as EventListener)

    apiClient.setToken('access-token')
    localStorage.setItem('access_token', 'legacy-local-token')
    sessionStorage.setItem('access_token', 'legacy-session-token')
    apiClient.clearAuth()

    expect(localStorage.getItem('access_token')).toBeNull()
    expect(sessionStorage.getItem('access_token')).toBeNull()
    expect(handler).toHaveBeenCalledTimes(2)

    const event = handler.mock.calls[1][0] as CustomEvent<{ authenticated: boolean }>
    expect(event.detail).toEqual({ authenticated: false })

    window.removeEventListener(AUTH_STATE_CHANGE_EVENT, handler as EventListener)
  })

  it('keeps access tokens in memory and only stores token-free session metadata', () => {
    apiClient.setToken('sensitive-access-token', true)

    expect(apiClient.getToken()).toBe('sensitive-access-token')
    expect(localStorage.getItem('access_token')).toBeNull()
    expect(sessionStorage.getItem('access_token')).toBeNull()

    const rawSignal = localStorage.getItem(AUTH_SESSION_SIGNAL_KEY)
    expect(rawSignal).not.toContain('sensitive-access-token')
    expect(parseAuthSessionSignal(rawSignal)).toMatchObject({ authenticated: true })
  })

  it('restores a session through the refresh cookie and stores the result in memory only', async () => {
    const rawClient = apiClient['client']
    const previousAdapter = rawClient.defaults.adapter

    rawClient.defaults.adapter = (async (config: InternalAxiosRequestConfig) => ({
      data: { access_token: 'restored-access-token' },
      status: 200,
      statusText: 'OK',
      headers: {},
      config,
    })) as AxiosAdapter

    try {
      await expect(apiClient.restoreSession()).resolves.toBe('restored-access-token')
      expect(apiClient.getToken()).toBe('restored-access-token')
      expect(localStorage.getItem('access_token')).toBeNull()
      expect(sessionStorage.getItem('access_token')).toBeNull()
    } finally {
      rawClient.defaults.adapter = previousAdapter
    }
  })

  it('does not resurrect a session when logout wins an in-flight restore', async () => {
    const rawClient = apiClient['client']
    const previousAdapter = rawClient.defaults.adapter
    let resolveRefresh!: (response: Awaited<ReturnType<AxiosAdapter>>) => void

    rawClient.defaults.adapter = (() => new Promise((resolve) => {
      resolveRefresh = resolve
    })) as AxiosAdapter

    try {
      const restore = apiClient.restoreSession()
      await vi.waitFor(() => expect(resolveRefresh).toBeTypeOf('function'))
      apiClient.clearAuth()
      resolveRefresh({
        data: { access_token: 'stale-access-token' },
        status: 200,
        statusText: 'OK',
        headers: {},
        config: {} as InternalAxiosRequestConfig,
      })

      await expect(restore).rejects.toThrow('Auth state changed')
      expect(apiClient.getToken()).toBeNull()
    } finally {
      rawClient.defaults.adapter = previousAdapter
    }
  })

  it('clears cached API data whenever the authentication identity changes', () => {
    apiClient.setToken('first-token')
    cache.set('dashboard', { owner: 'first-user' }, 30_000)

    apiClient.setToken('second-token')

    expect(cache.get('dashboard')).toBeNull()
  })

  it('does not share or restore an in-flight cached response across token changes', async () => {
    let resolveFirst!: (value: string) => void
    let resolveSecond!: (value: string) => void
    const firstResponse = new Promise<string>((resolve) => {
      resolveFirst = resolve
    })
    const secondResponse = new Promise<string>((resolve) => {
      resolveSecond = resolve
    })
    const secondFetcher = vi.fn(() => secondResponse)

    apiClient.setToken('first-token')
    const firstRequest = cachedRequest('dashboard', () => firstResponse, 30_000)

    apiClient.setToken('second-token')
    const secondRequest = cachedRequest('dashboard', secondFetcher, 30_000)
    expect(secondFetcher).toHaveBeenCalledTimes(1)

    resolveFirst('first-user-data')
    await expect(firstRequest).resolves.toBe('first-user-data')
    expect(cache.get('dashboard')).toBeNull()

    resolveSecond('second-user-data')
    await expect(secondRequest).resolves.toBe('second-user-data')
    expect(cache.get('dashboard')).toBe('second-user-data')
  })

  it('sends auth refresh without a request body', async () => {
    const rawClient = apiClient['client']
    const previousAdapter = rawClient.defaults.adapter
    const requests: InternalAxiosRequestConfig[] = []

    rawClient.defaults.adapter = (async (config: InternalAxiosRequestConfig) => {
      requests.push(config)
      return {
        data: { access_token: 'new-access-token' },
        status: 200,
        statusText: 'OK',
        headers: {},
        config,
      }
    }) as AxiosAdapter

    try {
      const response = await apiClient.refreshToken()

      expect(response.data.access_token).toBe('new-access-token')
      expect(requests).toHaveLength(1)
      expect(requests[0].url).toBe('/api/auth/refresh')
      expect(requests[0].method).toBe('post')
      expect(requests[0].data).toBeUndefined()
    } finally {
      rawClient.defaults.adapter = previousAdapter
    }
  })

  it('authenticates protected gateway operational requests', async () => {
    const rawClient = apiClient['client']
    const previousAdapter = rawClient.defaults.adapter
    const requests: InternalAxiosRequestConfig[] = []

    rawClient.defaults.adapter = (async (config: InternalAxiosRequestConfig) => {
      requests.push(config)
      return {
        data: '',
        status: 200,
        statusText: 'OK',
        headers: {},
        config,
      }
    }) as AxiosAdapter

    try {
      apiClient.setToken('operational-access-token')
      await apiClient.get('/_gateway/metrics')

      expect(requests).toHaveLength(1)
      expect(requests[0].headers.Authorization).toBe('Bearer operational-access-token')
      expect(requests[0].headers['X-Client-Device-Id']).toBeTruthy()
    } finally {
      rawClient.defaults.adapter = previousAdapter
    }
  })
})
