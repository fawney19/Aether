import { describe, expect, it } from 'vitest'

import {
  buildModelsDevTieredPricing,
  resolveModelsDevTieredPricing,
} from '@/api/models-dev-pricing'

describe('buildModelsDevTieredPricing', () => {
  it('maps context bands and cache prices without flattening them', () => {
    expect(buildModelsDevTieredPricing({
      input: 5,
      output: 30,
      cache_read: 0.5,
      cache_write: 6.25,
      tiers: [{
        input: 10,
        output: 45,
        cache_read: 1,
        cache_write: 12.5,
        tier: { type: 'context', size: 272_000 },
      }],
    })).toEqual({
      tiers: [
        {
          up_to: 271_999,
          input_price_per_1m: 5,
          output_price_per_1m: 30,
          cache_creation_price_per_1m: 6.25,
          cache_read_price_per_1m: 0.5,
        },
        {
          up_to: null,
          input_price_per_1m: 10,
          output_price_per_1m: 45,
          cache_creation_price_per_1m: 12.5,
          cache_read_price_per_1m: 1,
        },
      ],
    })
  })

  it('sorts multiple context boundaries into contiguous Aether bands', () => {
    const cost = {
      input: 1,
      output: 2,
      tiers: [
        { input: 5, output: 6, tier: { type: 'context' as const, size: 200_000 } },
        { input: 3, output: 4, tier: { type: 'context' as const, size: 100_000 } },
      ],
    }

    expect(buildModelsDevTieredPricing(cost)?.tiers).toEqual([
      { up_to: 99_999, input_price_per_1m: 1, output_price_per_1m: 2 },
      { up_to: 199_999, input_price_per_1m: 3, output_price_per_1m: 4 },
      { up_to: null, input_price_per_1m: 5, output_price_per_1m: 6 },
    ])
    expect(cost.tiers.map(tier => tier.tier.size)).toEqual([200_000, 100_000])
  })

  it('keeps flat token pricing as one unbounded band', () => {
    expect(buildModelsDevTieredPricing({ input: 0, output: 0.1 })).toEqual({
      tiers: [{ up_to: null, input_price_per_1m: 0, output_price_per_1m: 0.1 }],
    })
  })

  it('omits an empty base band when context pricing starts at zero', () => {
    expect(buildModelsDevTieredPricing({
      input: 1,
      output: 2,
      tiers: [
        { input: 3, output: 4, tier: { type: 'context', size: 0 } },
        { input: 5, output: 6, tier: { type: 'context', size: 100_000 } },
      ],
    })?.tiers).toEqual([
      { up_to: 99_999, input_price_per_1m: 3, output_price_per_1m: 4 },
      { up_to: null, input_price_per_1m: 5, output_price_per_1m: 6 },
    ])
  })

  it.each([
    { input: -1, output: 2 },
    { input: 1, output: Number.POSITIVE_INFINITY },
    {
      input: 1,
      output: 2,
      tiers: [{ input: 3, output: 4, tier: { type: 'context', size: Number.MAX_SAFE_INTEGER + 1 } }],
    },
    {
      input: 1,
      output: 2,
      tiers: [{ input: 3, output: 4, tier: { type: 'context', size: -1 } }],
    },
    {
      input: 1,
      output: 2,
      tiers: [
        { input: 3, output: 4, tier: { type: 'context', size: 100 } },
        { input: 5, output: 6, tier: { type: 'context', size: 100 } },
      ],
    },
  ])('fails closed for malformed structured pricing', (cost) => {
    expect(buildModelsDevTieredPricing(cost)).toBeNull()
  })
})

describe('resolveModelsDevTieredPricing', () => {
  it('uses the GPT-5.5 Pro context tier declared by models.dev without inventing cache prices', () => {
    expect(resolveModelsDevTieredPricing('openai', 'gpt-5.5-pro', {
      input: 30,
      output: 180,
      tiers: [{
        input: 60,
        output: 270,
        tier: { type: 'context', size: 272_000 },
      }],
      context_over_200k: {
        input: 60,
        output: 270,
      },
    })).toEqual({
      tiers: [
        {
          up_to: 271_999,
          input_price_per_1m: 30,
          output_price_per_1m: 180,
        },
        {
          up_to: null,
          input_price_per_1m: 60,
          output_price_per_1m: 270,
        },
      ],
    })
  })

  it.each([
    'gpt-5.6-sol',
    'gpt-5.6-terra',
    'gpt-5.6-luna',
  ])('uses the fetched models.dev cost for OpenAI model %s', (modelId) => {
    const fetchedCost = {
      input: 7,
      output: 11,
      cache_read: 0.7,
      cache_write: 8.75,
      tiers: [{
        input: 13,
        output: 17,
        cache_read: 1.3,
        cache_write: 16.25,
        tier: { type: 'context' as const, size: 123_000 },
      }],
    }

    expect(resolveModelsDevTieredPricing('openai', modelId, fetchedCost)).toEqual({
      tiers: [
        {
          up_to: 122_999,
          input_price_per_1m: 7,
          output_price_per_1m: 11,
          cache_creation_price_per_1m: 8.75,
          cache_read_price_per_1m: 0.7,
        },
        {
          up_to: null,
          input_price_per_1m: 13,
          output_price_per_1m: 17,
          cache_creation_price_per_1m: 16.25,
          cache_read_price_per_1m: 1.3,
        },
      ],
    })
  })

  it('uses the same fetched-cost conversion for every provider and model identity', () => {
    expect(resolveModelsDevTieredPricing('openai', 'other-model', {
      input: 1,
      output: 2,
      tiers: [{ input: 3, output: 4, tier: { type: 'context', size: 272_000 } }],
    })?.tiers.map(tier => tier.up_to)).toEqual([271_999, null])
  })

  it('does not synthesize pricing when the fetched cost is absent', () => {
    expect(resolveModelsDevTieredPricing('openai', 'gpt-5.6-sol', undefined)).toBeNull()
  })

  it('keeps a models.dev fast cost as an explicit Priority catalog when bands differ', () => {
    expect(resolveModelsDevTieredPricing('openai', 'gpt-5.6-sol', {
      input: 5,
      output: 30,
      tiers: [{
        input: 10,
        output: 45,
        tier: { type: 'context', size: 272_000 },
      }],
    }, {
      fast: {
        cost: { input: 10, output: 60 },
        provider: { body: { service_tier: 'priority' } },
      },
    })).toEqual({
      tiers: [
        { up_to: 271_999, input_price_per_1m: 5, output_price_per_1m: 30 },
        { up_to: null, input_price_per_1m: 10, output_price_per_1m: 45 },
      ],
      processing_tiers: {
        priority: {
          tiers: [{ up_to: null, input_price_per_1m: 10, output_price_per_1m: 60 }],
        },
      },
    })
  })

  it('uses a multiplier only when every fast price has the same ratio', () => {
    expect(resolveModelsDevTieredPricing('anthropic', 'claude-opus-4.8', {
      input: 5,
      output: 25,
      cache_read: 0.5,
      cache_write: 6.25,
    }, {
      fast: {
        cost: { input: 10, output: 50, cache_read: 1, cache_write: 12.5 },
        provider: { body: { speed: 'fast' } },
      },
    })?.processing_tiers).toEqual({
      fast: { price_multiplier: 2 },
    })

    expect(resolveModelsDevTieredPricing('anthropic', 'claude-opus-4.7', {
      input: 5,
      output: 25,
    }, {
      fast: {
        cost: { input: 30, output: 150 },
        provider: { body: { speed: 'fast' } },
      },
    })?.processing_tiers).toEqual({
      fast: { price_multiplier: 6 },
    })
  })

  it('uses the standard catalog when an imported tier has a zero default ratio', () => {
    expect(resolveModelsDevTieredPricing('openai', 'gpt-5.6-sol', {
      input: 5,
      output: 30,
    }, {
      fast: {
        cost: { input: 0, output: 0 },
        provider: { body: { service_tier: 'priority' } },
      },
    })?.processing_tiers).toEqual({
      priority: { price_multiplier: 1 },
    })
  })

  it('prefers Anthropic speed=fast when the mode body also carries a standard service tier', () => {
    expect(resolveModelsDevTieredPricing('anthropic', 'claude-opus-fast', {
      input: 5,
      output: 25,
    }, {
      fast: {
        cost: { input: 10, output: 50 },
        provider: { body: { speed: ' FAST ', service_tier: 'default' } },
      },
    })?.processing_tiers).toEqual({
      fast: { price_multiplier: 2 },
    })
  })

  it('falls back to the mode key and keeps non-uniform prices explicit', () => {
    expect(resolveModelsDevTieredPricing('vendor', 'model', {
      input: 2,
      output: 4,
    }, {
      flex: { cost: { input: 1, output: 3 } },
    })?.processing_tiers).toEqual({
      flex: {
        tiers: [{ up_to: null, input_price_per_1m: 1, output_price_per_1m: 3 }],
      },
    })
  })

  it('does not reinterpret unrelated or special experimental modes as processing tiers', () => {
    const modes = JSON.parse(
      '{"pro":{"cost":{"input":2,"output":4}},"__proto__":{"cost":{"input":2,"output":4}}}',
    )
    const pricing = resolveModelsDevTieredPricing('vendor', 'model', {
      input: 1,
      output: 2,
    }, modes)

    expect(pricing?.processing_tiers).toBeUndefined()
  })
})
