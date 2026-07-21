import { describe, expect, it } from 'vitest'
import {
  cacheMultiplierFromPrice,
  cachePriceFromInputMultiplier,
} from '../tiered-pricing-multipliers'

describe('cache input multipliers', () => {
  it('converts an input multiplier to an actual cache price', () => {
    expect(cachePriceFromInputMultiplier(4, 1.25)).toBe(5)
    expect(cachePriceFromInputMultiplier(4, 0.1)).toBe(0.4)
    expect(cachePriceFromInputMultiplier(4, 0)).toBe(0)
  })

  it('derives a multiplier from an existing cache price', () => {
    expect(cacheMultiplierFromPrice(4, 5, 1.25)).toBe(1.25)
    expect(cacheMultiplierFromPrice(4, 0.4, 0.1)).toBe(0.1)
  })

  it('uses the fallback when a multiplier cannot be derived', () => {
    expect(cacheMultiplierFromPrice(0, 5, 1.25)).toBe(1.25)
    expect(cacheMultiplierFromPrice(4, undefined, 0.1)).toBe(0.1)
  })
})
