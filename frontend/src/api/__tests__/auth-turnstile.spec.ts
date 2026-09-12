import { beforeEach, describe, expect, it, vi } from 'vitest'

const { postMock, setTokenMock } = vi.hoisted(() => ({
  postMock: vi.fn(),
  setTokenMock: vi.fn(),
}))

vi.mock('@/api/client', () => ({
  default: {
    post: postMock,
    setToken: setTokenMock,
  },
}))

import { authApi } from '@/api/auth'

describe('authApi turnstile payloads', () => {
  beforeEach(() => {
    postMock.mockReset()
    setTokenMock.mockReset()
    postMock.mockResolvedValue({ data: {} })
  })

  it('includes turnstile token when sending email verification code', async () => {
    await authApi.sendVerificationCode('alice@example.com', 'turnstile-token')

    expect(postMock).toHaveBeenCalledWith('/api/auth/send-verification-code', {
      email: 'alice@example.com',
      turnstile_token: 'turnstile-token',
    })
  })

  it('includes turnstile token when registering', async () => {
    await authApi.register({
      email: 'alice@example.com',
      username: 'alice',
      password: 'secret123',
      turnstile_token: 'turnstile-token',
    })

    expect(postMock).toHaveBeenCalledWith('/api/auth/register', {
      email: 'alice@example.com',
      username: 'alice',
      password: 'secret123',
      turnstile_token: 'turnstile-token',
    })
  })

  it('binds verification and status requests to the verification session token', async () => {
    await authApi.verifyEmail('alice@example.com', '123456', 'verification-session-token')
    await authApi.getVerificationStatus('alice@example.com', 'verification-session-token')

    expect(postMock).toHaveBeenNthCalledWith(1, '/api/auth/verify-email', {
      email: 'alice@example.com',
      code: '123456',
      verification_token: 'verification-session-token',
    })
    expect(postMock).toHaveBeenNthCalledWith(2, '/api/auth/verification-status', {
      email: 'alice@example.com',
      verification_token: 'verification-session-token',
    })
  })

  it('refreshes auth token without a request body', async () => {
    postMock.mockResolvedValue({ data: { access_token: 'new-access-token' } })

    await authApi.refreshToken()

    expect(postMock).toHaveBeenCalledWith('/api/auth/refresh')
    expect(setTokenMock).toHaveBeenCalledWith('new-access-token')
  })

  it('publishes login session availability without changing the token payload', async () => {
    postMock.mockResolvedValue({ data: { access_token: 'login-access-token' } })

    await authApi.login({ email: 'alice@example.com', password: 'secret123' })

    expect(setTokenMock).toHaveBeenCalledWith('login-access-token', true)
  })
})
