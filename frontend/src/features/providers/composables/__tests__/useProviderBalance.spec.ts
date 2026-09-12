import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, type App } from 'vue'
import type { ActionResultResponse } from '@/api/providerOps'
import { useProviderBalance } from '../useProviderBalance'

const api = vi.hoisted(() => ({
  batchQueryBalance: vi.fn<() => Promise<Record<string, ActionResultResponse>>>(),
  getArchitectures: vi.fn().mockResolvedValue([]),
}))

vi.mock('@/api/providerOps', () => api)

let app: App | undefined
let root: HTMLDivElement

function mountBalance() {
  let balance!: ReturnType<typeof useProviderBalance>
  root = document.createElement('div')
  app = createApp({
    setup() {
      balance = useProviderBalance()
      return () => null
    },
  })
  app.mount(root)
  return balance
}

function result(status: ActionResultResponse['status'], available: number): ActionResultResponse {
  return {
    status,
    action_type: 'query_balance',
    data: { total_available: available, currency: 'USD', extra: {} },
    message: null,
    executed_at: '2026-09-07T00:00:00Z',
    response_time_ms: 0,
    cache_ttl_seconds: 0,
  }
}

beforeEach(() => {
  vi.useFakeTimers()
  api.batchQueryBalance.mockReset()
})

afterEach(() => {
  app?.unmount()
  app = undefined
  root?.remove()
  vi.useRealTimers()
})

describe('provider balance refresh', () => {
  it('does not overwrite a newer refresh with an older pending retry', async () => {
    const balance = mountBalance()
    const providers = [{ id: 'provider-1', ops_configured: true }]
    let resolveRetry!: (value: Record<string, ActionResultResponse>) => void
    api.batchQueryBalance
      .mockResolvedValueOnce({ 'provider-1': result('pending', 0) })
      .mockImplementationOnce(() => new Promise(resolve => { resolveRetry = resolve }))
      .mockResolvedValueOnce({ 'provider-1': result('success', 20) })

    await balance.loadBalances(providers)
    await vi.advanceTimersByTimeAsync(12_000)
    await balance.loadBalances(providers)
    resolveRetry({ 'provider-1': result('success', 10) })
    await Promise.resolve()

    expect(balance.getProviderBalance('provider-1')).toEqual({ available: 20, currency: 'USD' })
  })

  it('ignores a pending response after unmount', async () => {
    const balance = mountBalance()
    let resolveLoad!: (value: Record<string, ActionResultResponse>) => void
    api.batchQueryBalance.mockImplementationOnce(() => new Promise(resolve => { resolveLoad = resolve }))
    const loading = balance.loadBalances([{ id: 'provider-1', ops_configured: true }])
    app?.unmount()
    app = undefined
    resolveLoad({ 'provider-1': result('pending', 10) })
    await loading

    expect(balance.balanceCache.value).toEqual({})
    expect(vi.getTimerCount()).toBe(0)
  })

  it('preserves zero balances and false check-in results', async () => {
    const balance = mountBalance()
    api.batchQueryBalance.mockResolvedValueOnce({
      'provider-1': {
        ...result('success', 0),
        data: {
          total_available: 0,
          currency: 'USD',
          extra: { balance: 0, points: 0, checkin_success: false, checkin_message: 'try again' },
        },
      },
    })
    await balance.loadBalances([{ id: 'provider-1', ops_configured: true }])

    expect(balance.getProviderBalanceBreakdown('provider-1')).toEqual({ balance: 0, points: 0, currency: 'USD' })
    expect(balance.getProviderCheckin('provider-1')).toEqual({ success: false, message: 'try again' })
  })
})
