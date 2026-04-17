import { describe, expect, it } from 'vitest'

import { buildUserApiKeyMutationPayload } from '@/features/api-keys/utils/userKeyPayload'

describe('userKeyPayload', () => {
  it('omits concurrent_limit when the field is left blank', () => {
    expect(buildUserApiKeyMutationPayload({
      name: 'writer-key',
      rate_limit: 30,
      concurrent_limit: undefined,
    })).toEqual({
      name: 'writer-key',
      rate_limit: 30,
    })
  })

  it('keeps explicit unlimited concurrent_limit values', () => {
    expect(buildUserApiKeyMutationPayload({
      name: 'writer-key',
      rate_limit: undefined,
      concurrent_limit: 0,
    })).toEqual({
      name: 'writer-key',
      rate_limit: 0,
      concurrent_limit: 0,
    })
  })

  it('keeps positive concurrent_limit values', () => {
    expect(buildUserApiKeyMutationPayload({
      name: 'writer-key',
      rate_limit: 15,
      concurrent_limit: 4,
    })).toEqual({
      name: 'writer-key',
      rate_limit: 15,
      concurrent_limit: 4,
    })
  })
})
