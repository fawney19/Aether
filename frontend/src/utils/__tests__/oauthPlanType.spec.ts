import { describe, expect, it } from 'vitest'

import { formatOAuthPlanType } from '../oauthPlanType'

describe('formatOAuthPlanType', () => {
  it('uses the compact Codex label for the usage-based business plan', () => {
    expect(formatOAuthPlanType('self_serve_business_usage_based')).toBe('Codex')
    expect(formatOAuthPlanType(' SELF_SERVE_BUSINESS_USAGE_BASED ')).toBe('Codex')
  })

  it('keeps existing known plan labels intact', () => {
    expect(formatOAuthPlanType('plus')).toBe('Plus')
    expect(formatOAuthPlanType('team')).toBe('Team')
  })
})
