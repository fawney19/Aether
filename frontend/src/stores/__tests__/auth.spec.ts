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
})
