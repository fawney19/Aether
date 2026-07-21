import { beforeEach, describe, expect, it, vi } from 'vitest'

const { getMock } = vi.hoisted(() => ({ getMock: vi.fn() }))

vi.mock('@/api/client', () => ({
  default: {
    get: getMock,
  },
}))

import { getProviderKeysPage } from '@/api/endpoints/keys'

describe('getProviderKeysPage', () => {
  beforeEach(() => {
    getMock.mockReset()
  })

  it('normalizes a legacy array response for the provider drawer', async () => {
    getMock.mockResolvedValue({
      data: [{ id: 'key-1' }, { id: 'key-2' }],
    })

    const result = await getProviderKeysPage('provider-demo', { page: 1, page_size: 1 })

    expect(result).toMatchObject({ total: 2, page: 1, page_size: 1 })
    expect(result.keys).toEqual([{ id: 'key-1' }])
  })

  it('normalizes a malformed object without exposing a non-array keys field', async () => {
    getMock.mockResolvedValue({
      data: { total: null, page: null, page_size: null, keys: {} },
    })

    const result = await getProviderKeysPage('provider-demo', { page: 2, page_size: 3 })

    expect(result).toEqual({ total: 0, page: 2, page_size: 3, keys: [] })
  })
})
