import { describe, expect, it } from 'vitest'

import {
  formatPricePerMillion,
  resolveProcessingTierPriceMultiplier,
  resolveSettlementPricingSnapshot,
  resolveSettlementPricingSourceLabel,
  resolveSettlementPricingTiers,
} from '../settlement-pricing'

function buildSource(overrides: Record<string, unknown> = {}) {
  return {
    settlement: {
      rate_multiplier: 9,
      settlement_snapshot: {
        pricing_snapshot: {
          billing_processing_tier: 'priority',
          pricing_source: 'provider_override',
          tiered_pricing_source: 'global_default',
          processing_tier_price_multiplier: 2.5,
          tiered_pricing: {
            tiers: [{ up_to: null, input_price_per_1m: 5, output_price_per_1m: 30 }],
          },
          ...overrides,
        },
      },
    },
    tiered_pricing: {
      source: 'global',
      tiers: [{ up_to: null, input_price_per_1m: 1 }],
    },
  }
}

describe('settlement pricing presentation', () => {
  it('reads the resolved pricing snapshot and prefers its catalog over legacy tiers', () => {
    const source = buildSource()

    expect(resolveSettlementPricingSnapshot(source)?.billing_processing_tier).toBe('priority')
    expect(resolveSettlementPricingTiers(source)).toEqual([
      { up_to: null, input_price_per_1m: 5, output_price_per_1m: 30 },
    ])
  })

  it.each([
    ['provider_override', '提供商定价'],
    ['global_default', '全局定价'],
    ['mixed', '混合定价'],
  ])('maps the resolved %s pricing source', (pricingSource, label) => {
    expect(resolveSettlementPricingSourceLabel(buildSource({
      pricing_source: pricingSource,
    }))).toBe(label)
  })

  it('falls back from pricing_source to tiered_pricing_source and then legacy source', () => {
    expect(resolveSettlementPricingSourceLabel(buildSource({
      pricing_source: null,
      tiered_pricing_source: 'global_default',
    }))).toBe('全局定价')

    expect(resolveSettlementPricingSourceLabel({
      tiered_pricing: { source: 'provider' },
    })).toBe('提供商定价')
  })

  it('uses only processing_tier_price_multiplier, never settlement.rate_multiplier', () => {
    expect(resolveProcessingTierPriceMultiplier(buildSource())).toBe(2.5)
    expect(resolveProcessingTierPriceMultiplier(buildSource({
      processing_tier_price_multiplier: null,
    }))).toBeNull()
    expect(resolveProcessingTierPriceMultiplier({
      settlement: { rate_multiplier: 9 },
    })).toBeNull()
  })

  it('falls back to legacy tiers when the resolved snapshot has no catalog', () => {
    expect(resolveSettlementPricingTiers(buildSource({ tiered_pricing: null }))).toEqual([
      { up_to: null, input_price_per_1m: 1 },
    ])
  })

  it('formats an input-only embedding tier without inventing an output price', () => {
    const tiers = resolveSettlementPricingTiers(buildSource({
      tiered_pricing: {
        tiers: [{ up_to: null, input_price_per_1m: 0.1 }],
      },
    }))

    expect(formatPricePerMillion(tiers?.[0]?.input_price_per_1m)).toBe('$0.1/M')
    expect(formatPricePerMillion(tiers?.[0]?.output_price_per_1m)).toBe('-')
    expect(formatPricePerMillion(null)).toBe('-')
    expect(formatPricePerMillion(0)).toBe('$0/M')
  })
})
