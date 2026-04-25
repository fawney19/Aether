import { describe, expect, it } from 'vitest'

import { getCacheCreationTokens, getCacheReadTokens, getEffectiveInputTokens } from '../token-normalization'

describe('usage token normalization', () => {
  it('prefers explicit cache creation totals when present', () => {
    expect(getCacheCreationTokens({
      cache_creation_input_tokens: 18,
      cache_creation_ephemeral_5m_input_tokens: 5,
      cache_creation_ephemeral_1h_input_tokens: 7,
    })).toBe(18)
  })

  it('rehydrates cache creation totals from classified fields when legacy total is zero', () => {
    expect(getCacheCreationTokens({
      cache_creation_input_tokens: 0,
      cache_creation_ephemeral_5m_input_tokens: 12,
      cache_creation_ephemeral_1h_input_tokens: 8,
    })).toBe(20)
  })

  it('keeps cache read and cache creation as separate display values', () => {
    const usage = {
      input_tokens: 1,
      cache_creation_input_tokens: 1530,
      cache_read_input_tokens: 104026,
      output_tokens: 591,
      api_format: 'claude:chat',
    }

    expect(getEffectiveInputTokens(usage)).toBe(1)
    expect(getCacheReadTokens(usage)).toBe(104026)
    expect(getCacheCreationTokens(usage)).toBe(1530)
  })

  it('keeps effective input token normalization unchanged', () => {
    expect(getEffectiveInputTokens({
      input_tokens: 100,
      cache_read_input_tokens: 20,
      api_format: 'openai:chat',
    })).toBe(80)
  })

  it('does not subtract cache read tokens for Claude usage', () => {
    expect(getEffectiveInputTokens({
      input_tokens: 4941,
      cache_creation_input_tokens: 687,
      cache_read_input_tokens: 52873,
      output_tokens: 973,
      api_format: 'claude:chat',
    })).toBe(4941)
  })
})
