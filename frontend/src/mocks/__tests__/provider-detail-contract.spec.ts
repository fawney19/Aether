import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@/config/demo', () => ({
  isDemoMode: () => true,
  DEMO_ACCOUNTS: {
    admin: { email: 'admin@demo.aether.io', password: 'demo123' },
    user: { email: 'user@demo.aether.io', password: 'demo123' },
  },
}))

import { handleMockRequest, setMockUserToken } from '../handler'

describe('provider detail demo contracts', () => {
  beforeEach(() => {
    setMockUserToken('demo-access-token-admin')
  })

  it('returns the paginated key contract used by the provider drawer', async () => {
    const firstPageResponse = await handleMockRequest({
      method: 'GET',
      url: '/api/admin/endpoints/providers/provider-004/keys',
      params: { page: 1, page_size: 1 },
    })
    const secondPageResponse = await handleMockRequest({
      method: 'GET',
      url: '/api/admin/endpoints/providers/provider-004/keys',
      params: { page: 2, page_size: 1 },
    })
    const firstPage = firstPageResponse?.data as {
      total: number
      page: number
      page_size: number
      keys: Array<{ id: string }>
    }
    const secondPage = secondPageResponse?.data as typeof firstPage

    expect(firstPage).toMatchObject({ total: 2, page: 1, page_size: 1 })
    expect(firstPage.keys).toHaveLength(1)
    expect(secondPage).toMatchObject({ total: 2, page: 2, page_size: 1 })
    expect(secondPage.keys).toHaveLength(1)
    expect(secondPage.keys[0]?.id).not.toBe(firstPage.keys[0]?.id)
  })

  it('preserves the legacy array contract for skip/limit callers', async () => {
    const response = await handleMockRequest({
      method: 'GET',
      url: '/api/admin/endpoints/providers/provider-004/keys',
      params: { skip: 1, limit: 1 },
    })

    expect(Array.isArray(response?.data)).toBe(true)
    expect(response?.data).toHaveLength(1)
  })

  it('returns a complete mapping-preview envelope', async () => {
    const response = await handleMockRequest({
      method: 'GET',
      url: '/api/admin/providers/provider-004/mapping-preview',
    })

    expect(response?.data).toEqual({
      provider_id: 'provider-004',
      provider_name: 'IKunCode',
      keys: [],
      total_keys: 0,
      total_matches: 0,
      truncated: false,
      truncated_keys: 0,
      truncated_models: 0,
    })
  })
})
