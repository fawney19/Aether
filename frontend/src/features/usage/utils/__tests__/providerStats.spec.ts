import { describe, expect, it } from 'vitest'

import { isUsageProviderVisible, normalizeUsageProviderStats } from '../providerStats'

describe('usage provider stats normalization', () => {
  it('maps admin provider aggregation fields to table fields', () => {
    const rows = normalizeUsageProviderStats([
      {
        provider_id: 'provider-openai',
        provider_key: 'provider-openai',
        provider_identity_source: 'provider_id',
        provider: 'OpenAI',
        request_count: 12,
        total_tokens: 3456,
        effective_input_tokens: 1200,
        total_input_context: 1600,
        output_tokens: 2256,
        cache_read_tokens: 240,
        cache_creation_tokens: 60,
        cache_hit_rate: 15,
        total_cost: 0.123456,
        actual_cost: 0.2,
        avg_response_time_ms: 1250,
        success_rate: 91.67,
        error_count: 1,
      },
    ])

    expect(rows).toEqual([
      {
        providerId: 'provider-openai',
        providerKey: 'provider-openai',
        providerIdentitySource: 'provider_id',
        provider: 'OpenAI',
        requests: 12,
        totalTokens: 3456,
        effectiveInputTokens: 1200,
        totalInputContext: 1600,
        outputTokens: 2256,
        cacheReadTokens: 240,
        cacheCreationTokens: 60,
        cacheHitRate: 15,
        totalCost: 0.123456,
        actualCost: 0.2,
        successRate: 91.67,
        avgResponseTime: '1.25s',
      },
    ])
  })

  it('filters placeholder providers', () => {
    expect(isUsageProviderVisible('OpenAI')).toBe(true)
    expect(isUsageProviderVisible('unknown')).toBe(false)
    expect(isUsageProviderVisible('unknow')).toBe(false)
    expect(isUsageProviderVisible('pending')).toBe(false)
    expect(isUsageProviderVisible(' ')).toBe(false)
  })
})