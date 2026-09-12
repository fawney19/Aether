import { beforeEach, describe, expect, it, vi } from 'vitest'
import { revealEndpointRules } from '../endpoints/endpoints'

const client = vi.hoisted(() => ({ get: vi.fn() }))
vi.mock('../client', () => ({ default: client }))

beforeEach(() => {
  client.get.mockReset().mockResolvedValue({ data: { header_rules: [], body_rules: [], response_header_rules: [] } })
})

describe('endpoint rule reveal API', () => {
  it('uses the scoped route and forwards cancellation', async () => {
    const controller = new AbortController()
    await revealEndpointRules('endpoint/with?reserved', controller.signal)
    expect(client.get).toHaveBeenCalledWith(
      '/api/admin/endpoints/endpoint%2Fwith%3Freserved/rules/reveal',
      { signal: controller.signal },
    )
  })

  it('fetches each reveal without retaining a cached response', async () => {
    await revealEndpointRules('endpoint-1')
    await revealEndpointRules('endpoint-1')
    expect(client.get).toHaveBeenCalledTimes(2)
  })
})
