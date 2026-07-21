import { beforeEach, describe, expect, it, vi } from 'vitest'

const { getMock, cachedRequestMock, buildCacheKeyMock } = vi.hoisted(() => ({
  getMock: vi.fn(),
  cachedRequestMock: vi.fn(async (_key: string, fetcher: () => Promise<unknown>) => fetcher()),
  buildCacheKeyMock: vi.fn((prefix: string) => prefix),
}))

vi.mock('@/api/client', () => ({
  default: {
    get: getMock,
  },
}))

vi.mock('@/utils/cache', () => ({
  cache: {
    clear: vi.fn(),
    delete: vi.fn(),
  },
  cachedRequest: cachedRequestMock,
  buildCacheKey: buildCacheKeyMock,
}))

import { adminApi } from '@/api/admin'

describe('adminApi analytics cache options', () => {
  const params = {
    start_date: '2026-07-01',
    end_date: '2026-07-15',
    preset: 'custom',
    timezone: 'Asia/Shanghai',
    tz_offset_minutes: 480,
  }

  beforeEach(() => {
    getMock.mockReset()
    getMock.mockResolvedValue({ data: {} })
    cachedRequestMock.mockClear()
    buildCacheKeyMock.mockClear()
  })

  it('keeps the existing 20-second cache TTL by default', async () => {
    await adminApi.getTimeSeries(params)
    await adminApi.getPercentiles(params)
    await adminApi.getProviderPerformance(params)
    await adminApi.getErrorDistribution(params)

    for (let call = 1; call <= 4; call += 1) {
      expect(cachedRequestMock).toHaveBeenNthCalledWith(
        call,
        expect.any(String),
        expect.any(Function),
        20 * 1000
      )
    }
  })

  it('uses a zero TTL when an analytics request skips the cache', async () => {
    const options = { skipCache: true }
    const providerParams = { ...params, include_timeline: false }

    await adminApi.getTimeSeries(params, options)
    await adminApi.getPercentiles(params, options)
    await adminApi.getProviderPerformance(providerParams, options)
    await adminApi.getErrorDistribution(params, options)

    for (let call = 1; call <= 4; call += 1) {
      expect(cachedRequestMock).toHaveBeenNthCalledWith(
        call,
        expect.any(String),
        expect.any(Function),
        0
      )
    }

    expect(getMock).toHaveBeenNthCalledWith(1, '/api/admin/stats/time-series', { params })
    expect(getMock).toHaveBeenNthCalledWith(2, '/api/admin/stats/performance/percentiles', {
      params,
    })
    expect(getMock).toHaveBeenNthCalledWith(3, '/api/admin/stats/performance/providers', {
      params: providerParams,
    })
    expect(getMock).toHaveBeenNthCalledWith(4, '/api/admin/stats/errors/distribution', { params })
  })
})
