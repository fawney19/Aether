import { describe, expect, it } from 'vitest'

import { hasAuthIdentityChanged, parseAccessTokenIdentity } from '@/utils/authToken'

function buildToken(payload: Record<string, unknown>): string {
  const encoded = btoa(JSON.stringify(payload))
    .replace(/\+/g, '-')
    .replace(/\//g, '_')
    .replace(/=+$/g, '')
  return `header.${encoded}.signature`
}

describe('authToken helpers', () => {
  it('parses user identity from JWT payload', () => {
    const token = buildToken({ user_id: 'user-1', role: 'admin' })

    expect(parseAccessTokenIdentity(token)).toEqual({
      userId: 'user-1',
      role: 'admin',
    })
  })

  it('returns null for non-jwt tokens', () => {
    expect(parseAccessTokenIdentity('demo-access-token')).toBeNull()
  })

  it('detects unchanged identity when only token value rotates', () => {
    const previous = buildToken({ user_id: 'user-1', role: 'user', exp: 1 })
    const next = buildToken({ user_id: 'user-1', role: 'user', exp: 2 })

    expect(
      hasAuthIdentityChanged(previous, next, { id: 'user-1', role: 'user' }),
    ).toBe(false)
  })

  it('detects account switch when user identity changes', () => {
    const previous = buildToken({ user_id: 'user-1', role: 'user' })
    const next = buildToken({ user_id: 'user-2', role: 'admin' })

    expect(
      hasAuthIdentityChanged(previous, next, { id: 'user-1', role: 'user' }),
    ).toBe(true)
  })
})
