import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'

const { logoutMock, getTokenMock, getCurrentUserMock } = vi.hoisted(() => ({
  logoutMock: vi.fn(),
  getTokenMock: vi.fn(() => null),
  getCurrentUserMock: vi.fn(),
}))

vi.mock('@/api/auth', () => ({
  authApi: {
    logout: logoutMock,
    getCurrentUser: getCurrentUserMock,
  },
}))

vi.mock('@/api/client', () => ({
  default: {
    getToken: getTokenMock,
  },
}))

import { useAuthStore } from '@/stores/auth'

describe('auth store logout', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    logoutMock.mockReset()
    getTokenMock.mockReset()
    getCurrentUserMock.mockReset()
    getTokenMock.mockReturnValue(null)
  })

  it('waits for backend logout before resolving', async () => {
    let resolveLogout: (() => void) | null = null
    logoutMock.mockImplementation(
      () =>
        new Promise<void>((resolve) => {
          resolveLogout = resolve
        })
    )

    const store = useAuthStore()
    store.user = {
      id: 'user-1',
      username: 'tester',
      role: 'user',
      is_active: true,
      created_at: '2026-03-16T00:00:00Z',
    }
    store.token = 'access-token'

    let settled = false
    const logoutPromise = store.logout().then(() => {
      settled = true
    })

    await Promise.resolve()

    expect(logoutMock).toHaveBeenCalledTimes(1)
    expect(store.user).toBeNull()
    expect(store.token).toBeNull()
    expect(settled).toBe(false)

    resolveLogout?.()
    await logoutPromise

    expect(settled).toBe(true)
  })

  it('clears local auth state for external logout without calling backend', () => {
    const store = useAuthStore()
    store.user = {
      id: 'user-1',
      username: 'tester',
      role: 'user',
      is_active: true,
      created_at: '2026-03-16T00:00:00Z',
    }
    store.token = 'access-token'

    store.applyExternalLogout()

    expect(store.user).toBeNull()
    expect(store.token).toBeNull()
    expect(logoutMock).not.toHaveBeenCalled()
  })

  it('clears stale store auth when fetchCurrentUser fails after token was removed', async () => {
    const store = useAuthStore()
    store.user = {
      id: 'user-1',
      username: 'tester',
      role: 'user',
      is_active: true,
      created_at: '2026-03-16T00:00:00Z',
    }
    store.token = 'access-token'
    getCurrentUserMock.mockRejectedValue(new Error('unauthorized'))
    getTokenMock.mockReturnValue(null)

    const result = await store.fetchCurrentUser()

    expect(result).toBeNull()
    expect(store.user).toBeNull()
    expect(store.token).toBeNull()
  })

  it('deduplicates concurrent current-user requests', async () => {
    let resolveCurrentUser: ((user: {
      id: string
      username: string
      role: string
      is_active: boolean
      created_at: string
    }) => void) | null = null
    getTokenMock.mockReturnValue('access-token')
    getCurrentUserMock.mockImplementation(
      () => new Promise((resolve) => {
        resolveCurrentUser = resolve
      })
    )

    const store = useAuthStore()
    const firstRequest = store.fetchCurrentUser()
    const secondRequest = store.fetchCurrentUser()

    expect(getCurrentUserMock).toHaveBeenCalledTimes(1)

    const currentUser = {
      id: 'user-1',
      username: 'tester',
      role: 'user',
      is_active: true,
      created_at: '2026-03-16T00:00:00Z',
    }
    resolveCurrentUser?.(currentUser)

    await expect(firstRequest).resolves.toEqual(currentUser)
    await expect(secondRequest).resolves.toEqual(currentUser)
  })

  it('does not repeat a failed current-user request on every navigation', async () => {
    getTokenMock.mockReturnValue('access-token')
    getCurrentUserMock.mockRejectedValue(new Error('network unavailable'))

    const store = useAuthStore()
    await store.fetchCurrentUser()
    await store.fetchCurrentUser()

    expect(store.token).toBe('access-token')
    expect(store.user).toBeNull()
    expect(getCurrentUserMock).toHaveBeenCalledTimes(1)
  })

  it('does not restore a stale user after logout while the request is in flight', async () => {
    let resolveCurrentUser: ((user: {
      id: string
      username: string
      role: string
      is_active: boolean
      created_at: string
    }) => void) | null = null
    getTokenMock.mockReturnValue('access-token')
    getCurrentUserMock.mockImplementation(
      () => new Promise((resolve) => {
        resolveCurrentUser = resolve
      })
    )

    const store = useAuthStore()
    const currentUserRequest = store.fetchCurrentUser()
    await store.logout()
    resolveCurrentUser?.({
      id: 'stale-user',
      username: 'stale',
      role: 'user',
      is_active: true,
      created_at: '2026-03-16T00:00:00Z',
    })

    await expect(currentUserRequest).resolves.toBeNull()
    expect(store.user).toBeNull()
    expect(store.token).toBeNull()
  })

  it('does not restore a stale token when an in-flight request fails after logout', async () => {
    let rejectCurrentUser: ((error: Error) => void) | null = null
    getTokenMock.mockReturnValue('access-token')
    getCurrentUserMock.mockImplementation(
      () => new Promise((_resolve, reject) => {
        rejectCurrentUser = reject
      })
    )

    const store = useAuthStore()
    const currentUserRequest = store.fetchCurrentUser()
    await store.logout()
    rejectCurrentUser?.(new Error('stale request failed'))

    await expect(currentUserRequest).resolves.toBeNull()
    expect(store.user).toBeNull()
    expect(store.token).toBeNull()
  })

  it('skips the delayed auth check when the router already loaded the user', async () => {
    getTokenMock.mockReturnValue('access-token')
    const store = useAuthStore()
    store.user = {
      id: 'user-1',
      username: 'tester',
      role: 'user',
      is_active: true,
      created_at: '2026-03-16T00:00:00Z',
    }

    await store.checkAuth()

    expect(getCurrentUserMock).not.toHaveBeenCalled()
  })

  it('separates admin access from admin operations for audit administrators', () => {
    const store = useAuthStore()

    store.user = {
      id: 'audit-1',
      username: 'auditor',
      role: 'audit_admin',
      is_active: true,
      created_at: '2026-03-16T00:00:00Z',
    }

    expect(store.isAdmin).toBe(false)
    expect(store.isAuditAdmin).toBe(true)
    expect(store.canAccessAdmin).toBe(true)
    expect(store.canOperateAdmin).toBe(false)
  })
})
