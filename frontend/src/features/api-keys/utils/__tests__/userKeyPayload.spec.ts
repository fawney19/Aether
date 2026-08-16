import { describe, expect, it } from 'vitest'

import { buildUserApiKeyMutationPayload } from '@/features/api-keys/utils/userKeyPayload'

describe('userKeyPayload', () => {
  it('omits concurrent_limit when the field is left blank', () => {
    expect(buildUserApiKeyMutationPayload({
      name: 'writer-key',
      rate_limit: 30,
      daily_usage_limit_usd: 12.5,
      concurrent_limit: undefined,
    })).toEqual({
      name: 'writer-key',
      rate_limit: 30,
      daily_usage_limit_usd: 12.5,
    })
  })

  it('keeps explicit unlimited concurrent_limit values', () => {
    expect(buildUserApiKeyMutationPayload({
      name: 'writer-key',
      rate_limit: undefined,
      daily_usage_limit_usd: undefined,
      concurrent_limit: 0,
    })).toEqual({
      name: 'writer-key',
      rate_limit: 0,
      daily_usage_limit_usd: 0,
      concurrent_limit: 0,
    })
  })

  it('keeps positive concurrent_limit values', () => {
    expect(buildUserApiKeyMutationPayload({
      name: 'writer-key',
      rate_limit: 15,
      daily_usage_limit_usd: 4,
      concurrent_limit: 4,
    })).toEqual({
      name: 'writer-key',
      rate_limit: 15,
      daily_usage_limit_usd: 4,
      concurrent_limit: 4,
    })
  })
})
