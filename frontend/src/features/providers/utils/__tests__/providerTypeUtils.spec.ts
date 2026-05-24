import { describe, expect, it } from 'vitest'

import { normalizeBatchImportCredentials } from '@/api/endpoints/provider_oauth'
import { isKeyManagedProviderType, isOAuthAccountProviderType } from '../providerTypeUtils'

describe('providerTypeUtils', () => {
  it('treats ChatGPT-Web as an OAuth account provider', () => {
    expect(isOAuthAccountProviderType('chatgpt_web')).toBe(true)
    expect(isOAuthAccountProviderType('ChatGPT_Web')).toBe(true)
    expect(isKeyManagedProviderType('chatgpt_web')).toBe(false)
  })

  it('treats Grok as an OAuth account provider', () => {
    expect(isOAuthAccountProviderType('grok')).toBe(true)
    expect(isOAuthAccountProviderType('GROK')).toBe(true)
    expect(isKeyManagedProviderType('grok')).toBe(false)
  })

  it('treats Windsurf as an OAuth account provider', () => {
    expect(isOAuthAccountProviderType('windsurf')).toBe(true)
    expect(isOAuthAccountProviderType('Windsurf')).toBe(true)
    expect(isKeyManagedProviderType('windsurf')).toBe(false)
  })
})

describe('normalizeBatchImportCredentials', () => {
  it('converts JSON Lines objects into a JSON array payload', () => {
    const result = normalizeBatchImportCredentials([
      '{"refresh_token":"rt-1","email":"one@example.com"}',
      '{"token":"token-2","email":"two@example.com"}',
    ].join('\n'))

    expect(result).toEqual({
      ok: true,
      isBatch: true,
      credentials: JSON.stringify([
        { refresh_token: 'rt-1', email: 'one@example.com' },
        { token: 'token-2', email: 'two@example.com' },
      ]),
    })
  })

  it('rejects malformed JSON Lines instead of treating them as raw tokens', () => {
    const result = normalizeBatchImportCredentials('{"refresh_token":"rt-1"}\n{"refresh_token":')

    expect(result.ok).toBe(false)
    if (!result.ok) {
      expect(result.message).toContain('第 2 行')
    }
  })

  it('converts multiple raw token lines into a JSON array payload', () => {
    const result = normalizeBatchImportCredentials('token-a\n# comment\ntoken-b')

    expect(result).toEqual({
      ok: true,
      isBatch: true,
      credentials: JSON.stringify(['token-a', 'token-b']),
    })
  })
})
