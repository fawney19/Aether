import { beforeEach, describe, expect, it, vi } from 'vitest'

import { cache, cachedRequest } from '@/utils/cache'

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise
  })
  return { promise, resolve }
}

describe('request cache invalidation', () => {
  beforeEach(() => {
    cache.clear()
  })

  it('does not let an invalidated request overwrite a newer in-flight request', async () => {
    const oldResponse = deferred<string>()
    const newResponse = deferred<string>()
    const staleFetcher = vi.fn(() => oldResponse.promise)
    const freshFetcher = vi.fn(() => newResponse.promise)
    const unexpectedFetcher = vi.fn(async () => 'unexpected')

    const staleRequest = cachedRequest('config', staleFetcher, 30_000)
    cache.delete('config')
    const freshRequest = cachedRequest('config', freshFetcher, 30_000)

    oldResponse.resolve('stale')
    await expect(staleRequest).resolves.toBe('stale')
    expect(cache.get('config')).toBeNull()

    const deduplicatedRequest = cachedRequest('config', unexpectedFetcher, 30_000)
    expect(unexpectedFetcher).not.toHaveBeenCalled()

    newResponse.resolve('fresh')
    await expect(freshRequest).resolves.toBe('fresh')
    await expect(deduplicatedRequest).resolves.toBe('fresh')
    expect(cache.get('config')).toBe('fresh')
  })

  it('does not refill the cache from a request started before a global clear', async () => {
    const response = deferred<string>()
    const request = cachedRequest('dashboard', () => response.promise, 30_000)

    cache.clear()
    response.resolve('previous-user-data')

    await expect(request).resolves.toBe('previous-user-data')
    expect(cache.get('dashboard')).toBeNull()
  })
})
