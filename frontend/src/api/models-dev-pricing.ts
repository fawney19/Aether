import type { PricingTier, TieredPricingConfig } from './endpoints/types'

export interface ModelsDevTokenCost {
  input: number
  output: number
  reasoning?: number
  cache_read?: number
  cache_write?: number
  input_audio?: number
  output_audio?: number
}

export interface ModelsDevCostTier extends ModelsDevTokenCost {
  tier: {
    type: 'context'
    size: number
  }
}

export interface ModelsDevCost extends ModelsDevTokenCost {
  tiers?: ModelsDevCostTier[]
}

const TOKEN_PRICE_FIELDS = [
  'input_price_per_1m',
  'output_price_per_1m',
  'cache_creation_price_per_1m',
  'cache_read_price_per_1m',
] as const
const PROCESSING_MODE_FALLBACK_KEYS = new Set(['fast', 'priority', 'flex', 'batch'])
const DEFAULT_PROCESSING_TIER_MULTIPLIER = 1

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function isPrice(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0
}

function parseTokenPrices(value: unknown): Omit<PricingTier, 'up_to'> | null {
  if (!isRecord(value) || !isPrice(value.input) || !isPrice(value.output)) return null
  if (value.cache_write !== undefined && !isPrice(value.cache_write)) return null
  if (value.cache_read !== undefined && !isPrice(value.cache_read)) return null

  return {
    input_price_per_1m: value.input,
    output_price_per_1m: value.output,
    ...(value.cache_write === undefined
      ? {}
      : { cache_creation_price_per_1m: value.cache_write }),
    ...(value.cache_read === undefined
      ? {}
      : { cache_read_price_per_1m: value.cache_read }),
  }
}

function parseContextTier(value: unknown): { size: number; prices: Omit<PricingTier, 'up_to'> } | null {
  if (!isRecord(value) || !isRecord(value.tier)) return null
  if (
    value.tier.type !== 'context'
    || typeof value.tier.size !== 'number'
    || !Number.isSafeInteger(value.tier.size)
    || value.tier.size < 0
  ) {
    return null
  }
  const prices = parseTokenPrices(value)
  return prices ? { size: value.tier.size, prices } : null
}

export function buildModelsDevTieredPricing(cost: unknown): TieredPricingConfig | null {
  const basePrices = parseTokenPrices(cost)
  if (!basePrices || !isRecord(cost)) return null

  const rawTiers = cost.tiers
  if (rawTiers !== undefined && !Array.isArray(rawTiers)) return null
  const contextTiers = (rawTiers ?? []).map(parseContextTier)
  if (contextTiers.some(tier => tier === null)) return null

  const sortedTiers = contextTiers
    .filter((tier): tier is NonNullable<typeof tier> => tier !== null)
    .sort((a, b) => a.size - b.size)
  if (sortedTiers.some((tier, index) => index > 0 && tier.size === sortedTiers[index - 1].size)) {
    return null
  }

  const tiers: PricingTier[] = []
  if (sortedTiers[0]?.size !== 0) {
    tiers.push({
      ...basePrices,
      up_to: sortedTiers[0] ? sortedTiers[0].size - 1 : null,
    })
  }
  tiers.push(...sortedTiers.map((tier, index) => ({
    ...tier.prices,
    up_to: sortedTiers[index + 1] ? sortedTiers[index + 1].size - 1 : null,
  })))

  return { tiers }
}

function uniformPriceMultiplier(
  standard: TieredPricingConfig,
  processing: TieredPricingConfig,
): number | null {
  if (standard.tiers.length !== processing.tiers.length) return null

  let candidate: number | null = null
  for (const [index, standardTier] of standard.tiers.entries()) {
    const processingTier = processing.tiers[index]
    if (standardTier.up_to !== processingTier?.up_to) return null

    for (const field of TOKEN_PRICE_FIELDS) {
      const standardPrice = standardTier[field]
      const processingPrice = processingTier[field]
      if (standardPrice === undefined || processingPrice === undefined) {
        if (standardPrice !== processingPrice) return null
        continue
      }
      if (standardPrice === 0) {
        if (processingPrice !== 0) return null
        continue
      }

      const ratio = processingPrice / standardPrice
      if (!Number.isFinite(ratio) || ratio < 0) return null
      if (candidate === null) candidate = ratio
      if (Math.abs(processingPrice - standardPrice * candidate) > 1e-9) return null
    }
  }
  // A zero ratio from an imported experimental mode is a missing/default price marker,
  // not a free processing tier. Keep the tier on the Standard catalog so it remains billable.
  return candidate === 0 ? DEFAULT_PROCESSING_TIER_MULTIPLIER : candidate
}

export function resolveModelsDevTieredPricing(
  _providerId: string,
  _modelId: string,
  cost: unknown,
  experimentalModes?: unknown,
): TieredPricingConfig | null {
  // Provider/model identities must never inject local prices over the fetched catalog.
  const standard = buildModelsDevTieredPricing(cost)
  if (!standard || !isRecord(experimentalModes)) return standard

  const processingTierEntries: Array<[string, NonNullable<TieredPricingConfig['processing_tiers']>[string]]> = []
  const seenProcessingTiers = new Set<string>()
  for (const [modeKey, rawMode] of Object.entries(experimentalModes)) {
    if (!isRecord(rawMode)) continue
    const modePricing = buildModelsDevTieredPricing(rawMode.cost)
    if (!modePricing) continue

    const provider = isRecord(rawMode.provider) ? rawMode.provider : null
    const body = provider && isRecord(provider.body) ? provider.body : null
    // Anthropic Fast is expressed with `speed=fast`. A provider body may also carry a
    // `service_tier` fact (commonly `default`/`standard`), but runtime settlement deliberately
    // gives Fast speed precedence, so catalog import must resolve the same processing-tier key.
    const mappedProcessingTier = typeof body?.speed === 'string'
      && body.speed.trim().toLowerCase() === 'fast'
      ? body.speed
      : typeof body?.service_tier === 'string'
        ? body.service_tier
        : null
    const normalizedModeKey = modeKey.trim().toLowerCase()
    const rawProcessingTier = mappedProcessingTier
      ?? (PROCESSING_MODE_FALLBACK_KEYS.has(normalizedModeKey) ? normalizedModeKey : '')
    const processingTier = rawProcessingTier.trim().toLowerCase()
    if (
      !processingTier
      || processingTier.length > 64
      || ['auto', 'default', 'standard'].includes(processingTier)
      || seenProcessingTiers.has(processingTier)
    ) {
      continue
    }

    const multiplier = uniformPriceMultiplier(standard, modePricing)
    seenProcessingTiers.add(processingTier)
    processingTierEntries.push([processingTier, multiplier === null
      ? { tiers: modePricing.tiers }
      : { price_multiplier: multiplier }])
  }
  if (processingTierEntries.length === 0) return standard
  return {
    ...standard,
    processing_tiers: Object.fromEntries(processingTierEntries),
  }
}
