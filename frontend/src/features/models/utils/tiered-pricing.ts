import type {
  ProviderTieredPricingConfig,
  ProcessingTierPricingConfig,
  TieredPricingConfig,
} from '@/api/endpoints/types'

type PricingCatalog = ProviderTieredPricingConfig | ProcessingTierPricingConfig
type RootPricingCatalog = TieredPricingConfig | ProviderTieredPricingConfig

export function comparePricingUpperBounds(
  left: number | null,
  right: number | null,
): number {
  if (left === null && right === null) return 0
  if (left === null) return 1
  if (right === null) return -1
  return left - right
}

function pricingCatalogs(pricing: RootPricingCatalog | null | undefined): PricingCatalog[] {
  if (!pricing) return []
  const processingTiers = pricing.processing_tiers
    ? Object.values(pricing.processing_tiers).filter(isRecord)
    : []
  return [pricing, ...processingTiers]
}

export function tieredPricingHasImageOutputPricing(
  pricing: RootPricingCatalog | null | undefined,
): boolean {
  return pricingCatalogs(pricing).some((catalog) => {
    if (toFinitePrice(catalog.image_output_price_default) !== null) return true
    if (Object.values(catalog.image_output_prices || {}).some(prices => (
      isRecord(prices)
      && Object.values(prices).some(price => toFinitePrice(price) !== null)
    ))) return true
    return (catalog.image_output_price_ranges || []).some(range => (
      isRecord(range)
      && isRecord(range.prices)
      && Object.values(range.prices).some(price => toFinitePrice(price) !== null)
    ))
  })
}

export function tieredPricingHasCacheTtl(
  pricing: RootPricingCatalog | null | undefined,
  ttlMinutes: number,
): boolean {
  return pricingCatalogs(pricing).some(catalog => (
    Array.isArray(catalog.tiers)
    && catalog.tiers.some(tier => (
      Array.isArray(tier.cache_ttl_pricing)
      && tier.cache_ttl_pricing.some(entry => entry.ttl_minutes === ttlMinutes)
    ))
  ))
}

function toFinitePrice(value: unknown): number | null {
  if (typeof value === 'number' && Number.isFinite(value)) return value
  if (typeof value === 'string' && value.trim()) {
    const parsed = Number(value)
    return Number.isFinite(parsed) ? parsed : null
  }
  return null
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value)
}
