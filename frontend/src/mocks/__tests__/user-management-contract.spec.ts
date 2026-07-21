import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@/config/demo', () => ({
  isDemoMode: () => true,
  DEMO_ACCOUNTS: {
    admin: { email: 'admin@demo.aether.io', password: 'demo123' },
    user: { email: 'user@demo.aether.io', password: 'demo123' },
  },
}))

import { handleMockRequest, setMockUserToken } from '../handler'

describe('user management demo contracts', () => {
  beforeEach(() => {
    setMockUserToken('demo-access-token-admin')
  })

  it('returns a list-shaped user group response', async () => {
    const response = await handleMockRequest({
      method: 'GET',
      url: '/api/admin/user-groups',
    })

    expect(response?.data).toEqual({
      items: [],
      default_group_id: null,
    })
  })

  it('creates and lists managed keys only for the selected target user', async () => {
    const aliceId = 'demo-user-uuid-0003'
    const bobId = 'demo-user-uuid-0004'
    const created = await handleMockRequest({
      method: 'POST',
      url: `/api/admin/users/${aliceId}/api-keys`,
      data: JSON.stringify({ name: 'Alice inherited key' }),
    })

    expect(created?.data).toMatchObject({
      name: 'Alice inherited key',
      feature_settings: null,
      is_standalone: false,
    })
    expect(created?.data?.key).toMatch(/^sk-ae-demo-/)

    const aliceKeys = await handleMockRequest({
      method: 'GET',
      url: `/api/admin/users/${aliceId}/api-keys`,
    })
    const bobKeys = await handleMockRequest({
      method: 'GET',
      url: `/api/admin/users/${bobId}/api-keys`,
    })

    expect(aliceKeys?.data).toMatchObject({ total: 1 })
    expect(aliceKeys?.data?.api_keys).toHaveLength(1)
    expect(aliceKeys?.data?.api_keys[0]).toMatchObject({ name: 'Alice inherited key' })
    expect(aliceKeys?.data?.api_keys[0]).not.toHaveProperty('key')
    expect(aliceKeys?.data?.api_keys[0]).not.toHaveProperty('fullKey')
    expect(bobKeys?.data).toEqual({ api_keys: [], total: 0 })
  })
})
