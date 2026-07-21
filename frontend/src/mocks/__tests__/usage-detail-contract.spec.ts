import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@/config/demo', () => ({
  isDemoMode: () => true,
  DEMO_ACCOUNTS: {
    admin: { email: 'admin@demo.aether.io', password: 'demo123' },
    user: { email: 'user@demo.aether.io', password: 'demo123' },
  },
}))

import { handleMockRequest, setMockUserToken } from '../handler'

describe('usage detail demo contracts', () => {
  beforeEach(() => {
    setMockUserToken('demo-access-token-admin')
  })

  it('keeps body availability while omitting bodies from lightweight detail', async () => {
    const response = await handleMockRequest({
      method: 'GET',
      url: '/api/admin/usage/usage-cyber-risk-demo',
      params: { include_bodies: false },
    })

    expect(response?.data).toMatchObject({
      has_request_body: true,
      has_provider_request_body: true,
      has_response_body: true,
      request_body: null,
      provider_request_body: null,
      response_body: null,
      client_response_body: null,
      body_load_errors: null,
    })
  })

  it('returns the exact Cyber error body when bodies are requested', async () => {
    const response = await handleMockRequest({
      method: 'GET',
      url: '/api/admin/usage/usage-cyber-risk-demo',
      params: { include_bodies: true },
    })

    expect(response?.data?.response_body).toEqual({
      error: {
        type: 'invalid_request',
        message: 'This content was flagged for possible cybersecurity risk. If this seems wrong, try rephrasing your request. To get authorized for security work, join the Trusted Access for Cyber program: https://chatgpt.com/cyber',
        code: 400,
      },
    })
  })
})
