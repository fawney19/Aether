import { describe, expect, it } from 'vitest'

import type { TieredPricingConfig } from '@/api/endpoints/types'
import {
  tieredPricingHasCacheTtl,
  tieredPricingHasImageOutputPricing,
} from '../tiered-pricing'

function pricingWithProcessingTier(
  processingTier: NonNullable<TieredPricingConfig['processing_tiers']>[string],
): TieredPricingConfig {
  return {
    tiers: [{ up_to: null, input_price_per_1m: 1, output_price_per_1m: 2 }],
    processing_tiers: { priority: processingTier },
  }
}

describe('tiered pricing capabilities', () => {
  it('detects image prices in the base and processing-tier catalogs', () => {
    expect(tieredPricingHasImageOutputPricing({
      tiers: [],
      image_output_price_default: 0,
    })).toBe(true)
    expect(tieredPricingHasImageOutputPricing(pricingWithProcessingTier({
      image_output_prices: { '1024x1024': { high: 0.08 } },
    }))).toBe(true)
    expect(tieredPricingHasImageOutputPricing(pricingWithProcessingTier({
      image_output_price_ranges: [{
        up_to_pixels: null,
        prices: { medium: 0.04 },
      }],
    }))).toBe(true)
    expect(tieredPricingHasImageOutputPricing(pricingWithProcessingTier({}))).toBe(false)
  })

  it('detects cache TTL prices in the base and processing-tier catalogs', () => {
    expect(tieredPricingHasCacheTtl({
      tiers: [{
        up_to: null,
        input_price_per_1m: 1,
        output_price_per_1m: 2,
        cache_ttl_pricing: [{ ttl_minutes: 60, cache_creation_price_per_1m: 3 }],
      }],
    }, 60)).toBe(true)
    expect(tieredPricingHasCacheTtl(pricingWithProcessingTier({
      tiers: [{
        up_to: null,
        input_price_per_1m: 1,
        output_price_per_1m: 2,
        cache_ttl_pricing: [{ ttl_minutes: 60, cache_creation_price_per_1m: 3 }],
      }],
    }), 60)).toBe(true)
    expect(tieredPricingHasCacheTtl(pricingWithProcessingTier({}), 60)).toBe(false)
  })
})
