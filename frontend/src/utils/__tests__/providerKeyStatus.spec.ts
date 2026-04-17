import { describe, expect, it } from 'vitest'

import {
  getAccountStatusDisplay,
  getOAuthStatusDisplay,
  getOAuthStatusTitle,
} from '@/utils/providerKeyStatus'

describe('providerKeyStatus', () => {
  it('uses status_snapshot account state as the primary source', () => {
    expect(
      getAccountStatusDisplay({
        status_snapshot: {
          oauth: { code: 'valid' },
          account: {
            code: 'workspace_deactivated',
            label: '工作区停用',
            reason: 'deactivated_workspace',
            blocked: true,
          },
          quota: { code: 'ok', exhausted: false },
        },
      }),
    ).toEqual({
      code: 'workspace_deactivated',
      label: '工作区停用',
      reason: 'deactivated_workspace',
      blocked: true,
    })
  })

  it('shows oauth invalid when refresh failure exists beside account block', () => {
    const status = getOAuthStatusDisplay(
      {
        auth_type: 'oauth',
        oauth_expires_at: 2_000_000_000,
        status_snapshot: {
          oauth: {
            code: 'invalid',
            reason: 'Token 续期失败 (400): refresh_token_reused',
            expires_at: 2_000_000_000,
            requires_reauth: true,
          },
          account: {
            code: 'workspace_deactivated',
            label: '工作区停用',
            reason: 'deactivated_workspace',
            blocked: true,
          },
          quota: { code: 'ok', exhausted: false },
        },
      },
      0,
    )

    expect(status).toEqual({
      text: '已失效',
      isExpired: false,
      isExpiringSoon: false,
      isInvalid: true,
      invalidReason: 'Token 续期失败 (400): refresh_token_reused',
    })
  })

  it('falls back to countdown for account block without oauth invalidation', () => {
    const status = getOAuthStatusDisplay(
      {
        auth_type: 'oauth',
        oauth_expires_at: Math.floor(Date.now() / 1000) + 3 * 24 * 3600,
        status_snapshot: {
          oauth: {
            code: 'valid',
            expires_at: Math.floor(Date.now() / 1000) + 3 * 24 * 3600,
          },
          account: {
            code: 'account_disabled',
            label: '账号停用',
            reason: 'account has been deactivated',
            blocked: true,
          },
          quota: { code: 'ok', exhausted: false },
        },
      },
      0,
    )

    expect(status?.isInvalid).toBe(false)
    expect(status?.isExpired).toBe(false)
    expect(getOAuthStatusTitle({
      auth_type: 'oauth',
      oauth_expires_at: Math.floor(Date.now() / 1000) + 3 * 24 * 3600,
      status_snapshot: {
        oauth: {
          code: 'valid',
          expires_at: Math.floor(Date.now() / 1000) + 3 * 24 * 3600,
        },
        account: {
          code: 'account_disabled',
          label: '账号停用',
          reason: 'account has been deactivated',
          blocked: true,
        },
        quota: { code: 'ok', exhausted: false },
      },
    }, 0)).toContain('Token 剩余有效期:')
  })

  it('treats oauth_managed bearer credentials as oauth for legacy countdown fallback', () => {
    const future = Math.floor(Date.now() / 1000) + 2 * 24 * 3600
    const status = getOAuthStatusDisplay(
      {
        auth_type: 'bearer',
        oauth_managed: true,
        oauth_expires_at: future,
      },
      0,
    )

    expect(status).not.toBeNull()
    expect(status?.isExpired).toBe(false)
    expect(getOAuthStatusTitle({
      auth_type: 'bearer',
      oauth_managed: true,
      oauth_expires_at: future,
    }, 0)).toContain('Token 剩余有效期:')
  })
})
