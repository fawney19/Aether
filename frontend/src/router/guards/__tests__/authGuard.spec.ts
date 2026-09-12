import { describe, expect, it, vi } from 'vitest'

import { ensureUserLoaded } from '@/router/guards/authGuard'

describe('ensureUserLoaded', () => {
  it('restores a page-load session before deciding a protected route is unauthenticated', async () => {
    const store = {
      token: null as string | null,
      user: null,
      restoreSession: vi.fn(async function (this: { token: string | null }) {
        this.token = 'restored-access-token'
        return true
      }),
      fetchCurrentUser: vi.fn(async function (this: { user: unknown }) {
        this.user = { id: 'user-1' }
      }),
      logout: vi.fn(),
    }

    await expect(ensureUserLoaded(store as never)).resolves.toBe(true)
    expect(store.restoreSession).toHaveBeenCalledTimes(1)
    expect(store.fetchCurrentUser).toHaveBeenCalledTimes(1)
  })

  it('denies a protected route when the refresh cookie cannot restore a session', async () => {
    const store = {
      token: null,
      user: null,
      restoreSession: vi.fn(async () => false),
      fetchCurrentUser: vi.fn(),
      logout: vi.fn(),
    }

    await expect(ensureUserLoaded(store as never)).resolves.toBe(false)
    expect(store.fetchCurrentUser).not.toHaveBeenCalled()
  })
})
