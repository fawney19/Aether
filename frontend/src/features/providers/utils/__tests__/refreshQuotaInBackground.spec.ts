import { afterEach, describe, expect, it, vi } from 'vitest'
import type { RefreshQuotaResult } from '@/api/endpoints/keys'
import { refreshQuotaInBackground } from '../refreshQuotaInBackground'

function emptyQuota(status: 'error' | 'no_metadata' | 'forbidden' = 'no_metadata'): RefreshQuotaResult {
  return {
    success: 0,
    failed: 1,
    total: 1,
    results: [{ key_id: 'key-1', key_name: 'account', status }],
  }
}

const readyQuota: RefreshQuotaResult = {
  success: 1,
  failed: 0,
  total: 1,
  results: [{ key_id: 'key-1', key_name: 'account', status: 'success' }],
}

afterEach(() => vi.useRealTimers())

describe('initial background quota refresh', () => {
  it('waits and retries an initial empty response once', async () => {
    vi.useFakeTimers()
    const refresh = vi.fn().mockResolvedValueOnce(emptyQuota()).mockResolvedValueOnce(readyQuota)
    const pending = refreshQuotaInBackground({ refresh, isCurrent: () => true, retryInitialEmptyQuota: true })
    await vi.advanceTimersByTimeAsync(999)
    expect(refresh).toHaveBeenCalledTimes(1)
    await vi.advanceTimersByTimeAsync(1)
    expect(await pending).toBe(readyQuota)
    expect(refresh).toHaveBeenCalledTimes(2)
  })

  it('returns persistent failures after at most one retry', async () => {
    vi.useFakeTimers()
    const result = emptyQuota('error')
    const refresh = vi.fn().mockResolvedValue(result)
    const pending = refreshQuotaInBackground({ refresh, isCurrent: () => true, retryInitialEmptyQuota: true })
    await vi.runAllTimersAsync()
    expect(await pending).toBe(result)
    expect(refresh).toHaveBeenCalledTimes(2)
  })

  it.each([readyQuota, emptyQuota('forbidden'), {
    ...emptyQuota('error'),
    results: [{ ...emptyQuota('error').results[0]!, status_code: 401 }],
  }])('does not retry successful or rejected authorization results', async (result) => {
    const refresh = vi.fn().mockResolvedValue(result)
    expect(await refreshQuotaInBackground({ refresh, isCurrent: () => true, retryInitialEmptyQuota: true })).toBe(result)
    expect(refresh).toHaveBeenCalledTimes(1)
  })

  it('does not retry when initial quota retry is disabled', async () => {
    const refresh = vi.fn().mockResolvedValue(emptyQuota())
    await refreshQuotaInBackground({ refresh, isCurrent: () => true, retryInitialEmptyQuota: false })
    expect(refresh).toHaveBeenCalledTimes(1)
  })

  it('cancels the retry when the drawer closes or switches providers', async () => {
    vi.useFakeTimers()
    let current = true
    const refresh = vi.fn().mockResolvedValue(emptyQuota())
    const pending = refreshQuotaInBackground({ refresh, isCurrent: () => current, retryInitialEmptyQuota: true })
    await vi.advanceTimersByTimeAsync(1)
    current = false
    await vi.runAllTimersAsync()
    expect(await pending).toBeNull()
    expect(refresh).toHaveBeenCalledTimes(1)
  })

  it('discards an in-flight response from a closed drawer', async () => {
    let current = true
    const refresh = vi.fn(async () => {
      current = false
      return readyQuota
    })
    expect(await refreshQuotaInBackground({ refresh, isCurrent: () => current, retryInitialEmptyQuota: true })).toBeNull()
  })
})
