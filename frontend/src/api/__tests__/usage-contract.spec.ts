import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const { getMock, cachedRequestMock, dedupedRequestMock, buildCacheKeyMock } = vi.hoisted(() => ({
  getMock: vi.fn(),
  cachedRequestMock: vi.fn(async (_key: string, fn: () => Promise<unknown>) => fn()),
  dedupedRequestMock: vi.fn(async (_key: string, fn: () => Promise<unknown>) => fn()),
  buildCacheKeyMock: vi.fn(() => 'cache-key'),
}))

vi.mock('@/api/client', () => ({
  default: {
    get: getMock,
  },
}))

vi.mock('@/utils/cache', () => ({
  cachedRequest: cachedRequestMock,
  dedupedRequest: dedupedRequestMock,
  buildCacheKey: buildCacheKeyMock,
}))

import { usageApi } from '@/api/usage'

describe('usageApi contract alignment', () => {
  beforeEach(() => {
    getMock.mockReset()
    cachedRequestMock.mockClear()
    dedupedRequestMock.mockClear()
    buildCacheKeyMock.mockClear()
  })

  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('loads current-user usage records from the Rust usage endpoint and normalizes pagination', async () => {
    getMock.mockResolvedValueOnce({
      data: {
        records: [{ id: 'record-1' }],
        pagination: {
          total: 42,
          limit: 10,
          offset: 10,
          has_more: true,
        },
      },
    })

    const result = await usageApi.getUsageRecords({
      page: 2,
      page_size: 10,
      start_date: '2026-05-01',
      end_date: '2026-05-16',
    })

    expect(getMock).toHaveBeenCalledWith('/api/users/me/usage', {
      params: {
        limit: 10,
        offset: 10,
        start_date: '2026-05-01',
        end_date: '2026-05-16',
      },
    })
    expect(result).toEqual({
      records: [{ id: 'record-1' }],
      total: 42,
      page: 2,
      page_size: 10,
    })
  })

  it('loads admin usage for a specific user from admin usage endpoints', async () => {
    getMock
      .mockResolvedValueOnce({
        data: {
          total_requests: 7,
          total_tokens: 99,
          total_cost: 12.34,
          avg_response_time: 456,
        },
      })
      .mockResolvedValueOnce({
        data: {
          records: [{ id: 'record-2' }],
          total: 1,
          limit: 25,
          offset: 0,
        },
      })

    const result = await usageApi.getUserUsage('user-123', {
      page: 1,
      page_size: 25,
      model: 'gpt-5.5',
    })

    expect(getMock).toHaveBeenNthCalledWith(1, '/api/admin/usage/stats', {
      params: {
        user_id: 'user-123',
        model: 'gpt-5.5',
      },
    })
    expect(getMock).toHaveBeenNthCalledWith(2, '/api/admin/usage/records', {
      params: {
        user_id: 'user-123',
        limit: 25,
        offset: 0,
        model: 'gpt-5.5',
      },
    })
    expect(result).toEqual({
      records: [{ id: 'record-2' }],
      stats: {
        total_requests: 7,
        total_tokens: 99,
        total_cost: 12.34,
        avg_response_time: 456,
      },
    })
  })

  it('captures admin user usage server timing when the records request resolves', async () => {
    let now = 1_000
    vi.spyOn(Date, 'now').mockImplementation(() => now)

    let resolveStats: ((value: unknown) => void) | null = null
    getMock.mockImplementation((url: string) => {
      if (url === '/api/admin/usage/stats') {
        return new Promise(resolve => {
          resolveStats = resolve
        })
      }
      if (url === '/api/admin/usage/records') {
        return Promise.resolve({
          headers: {
            'x-aether-server-now-unix-ms': '10050',
          },
          data: {
            records: [{ id: 'record-3' }],
            total: 1,
            limit: 25,
            offset: 0,
          },
        })
      }
      return Promise.reject(new Error(`unexpected url: ${url}`))
    })

    const resultPromise = usageApi.getUserUsage('user-123', { page: 1, page_size: 25 })
    now = 1_100
    await Promise.resolve()
    now = 20_000
    resolveStats?.({
      data: {
        total_requests: 1,
        total_tokens: 10,
        total_cost: 0.1,
        avg_response_time: 500,
      },
    })

    const result = await resultPromise

    expect(result.server_timing).toEqual({
      server_now_unix_ms: 10_050,
      client_send_unix_ms: 1_000,
      client_receive_unix_ms: 1_100,
      round_trip_ms: 100,
    })
  })

  it('uses an extended timeout and cache bypass option for admin analytics', async () => {
    getMock
      .mockResolvedValueOnce({
        data: {
          total_requests: 7,
          total_tokens: 99,
          total_cost: 12.34,
          avg_response_time: 456,
        },
      })
      .mockResolvedValueOnce({
        data: [{ provider: 'OpenAI', request_count: 7 }],
      })

    await usageApi.getUsageStats({ preset: 'last30days' }, { skipCache: true })
    await usageApi.getUsageByProvider({ preset: 'last30days' }, { skipCache: true })

    expect(cachedRequestMock).toHaveBeenNthCalledWith(
      1,
      expect.stringContaining(':fresh'),
      expect.any(Function),
      0
    )
    expect(cachedRequestMock).toHaveBeenNthCalledWith(
      2,
      expect.stringContaining(':fresh'),
      expect.any(Function),
      0
    )
    expect(getMock).toHaveBeenNthCalledWith(1, '/api/admin/usage/stats', {
      params: { preset: 'last30days' },
      timeout: 120000,
    })
    expect(getMock).toHaveBeenNthCalledWith(2, '/api/admin/usage/aggregation/stats', {
      params: { group_by: 'provider', preset: 'last30days' },
      timeout: 120000,
    })
  })
})
