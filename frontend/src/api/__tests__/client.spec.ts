import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import apiClient, { AUTH_STATE_CHANGE_EVENT } from '@/api/client'

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
    apiClient.clearAuth()

    expect(localStorage.getItem('access_token')).toBeNull()
    expect(handler).toHaveBeenCalledTimes(1)

    const event = handler.mock.calls[0][0] as CustomEvent<{ token: string | null }>
    expect(event.detail).toEqual({ token: null })

    window.removeEventListener(AUTH_STATE_CHANGE_EVENT, handler as EventListener)
  })
})
