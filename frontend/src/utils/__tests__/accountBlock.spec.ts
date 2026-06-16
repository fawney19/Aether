import { describe, expect, it } from 'vitest'

import {
  classifyAccountBlockLabel,
  cleanAccountBlockReason,
  isAccountLevelBlockReason,
  isRefreshFailedReason,
} from '@/utils/accountBlock'

describe('accountBlock helpers', () => {
  it('detects refresh failure markers even when account block is also present', () => {
    expect(
      isRefreshFailedReason(
        '[ACCOUNT_BLOCK] 工作区已停用 (deactivated_workspace)\n[REFRESH_FAILED] Token 续期失败',
      ),
    ).toBe(true)
  })

  it('keeps account block reason clean when refresh failure is appended', () => {
    expect(
      cleanAccountBlockReason(
        '[ACCOUNT_BLOCK] 工作区已停用 (deactivated_workspace)\n[REFRESH_FAILED] Token 续期失败',
      ),
    ).toBe('工作区已停用 (deactivated_workspace)')
  })

  it('does not treat refresh failure text as an account block by itself', () => {
    expect(
      isAccountLevelBlockReason(
        '[REFRESH_FAILED] Token 续期失败 (401): token has been invalidated',
      ),
    ).toBe(false)
  })

  it('labels invalidated and expired oauth markers separately', () => {
    expect(classifyAccountBlockLabel('[OAUTH_EXPIRED] token invalidated')).toBe('Token 失效')
    expect(classifyAccountBlockLabel('[OAUTH_EXPIRED] session expired')).toBe('Token 过期')
  })
})
