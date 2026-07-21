import type {
  ModelCreate,
  ModelUpdate,
  ProviderTieredPricingConfig,
  TieredPricingConfig,
} from '@/api/endpoints'

interface EmbeddingMetadataCarrier {
  supported_capabilities?: string[] | null
  supports_embedding?: boolean | null
  effective_supports_embedding?: boolean | null
  config?: Record<string, unknown> | null
}

function isEmbeddingApiFormat(format: unknown): boolean {
  const value = String(format).trim().toLowerCase()
  return value.endsWith(':embedding') || value === 'aliyun:multimodal_embedding'
}

export interface ProviderModelCreatePayloadInput {
  globalModelId: string
  providerModelName: string
  finalTieredPricing: ProviderTieredPricingConfig | null
  tieredPricingModified: boolean
  pricePerRequest?: number
  pricePerRequestModified: boolean
  cleanConfig?: Record<string, unknown>
  configTouched: boolean
  supportsVision?: boolean
  supportsFunctionCalling?: boolean
  supportsStreaming?: boolean
  supportsExtendedThinking?: boolean
  supportsImageGeneration?: boolean
  isActive: boolean
}

export interface ProviderModelUpdatePayloadInput {
  finalTieredPricing: ProviderTieredPricingConfig | null
  tieredPricingModified: boolean
  pricePerRequest?: number
  pricePerRequestModified: boolean
  cleanConfig?: Record<string, unknown>
  configTouched: boolean
  supportsVision?: boolean
  supportsFunctionCalling?: boolean
  supportsStreaming?: boolean
  supportsExtendedThinking?: boolean
  supportsImageGeneration?: boolean
  isActive: boolean
}

export function modelSupportsEmbedding(model: EmbeddingMetadataCarrier | null | undefined): boolean {
  if (!model) return false
  if ('effective_supports_embedding' in model && model.effective_supports_embedding === true) return true
  if ('supports_embedding' in model && model.supports_embedding === true) return true

  const supportedCapabilities = 'supported_capabilities' in model ? model.supported_capabilities : null
  const config = model.config || {}
  return supportedCapabilities?.includes('embedding') === true
    || config.embedding === true
    || config.model_type === 'embedding'
    || (Array.isArray(config.api_formats) && config.api_formats.some(isEmbeddingApiFormat))
}

const STANDARD_PRICING_KEYS = new Set([
  'tiers',
  'image_output_prices',
  'image_output_price_default',
  'image_output_price_ranges',
  'image_output_price_per_image',
  'image_output_price_matrix',
  'image_prices',
])

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}

function cloneJson<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T
}

function hasOwn(object: object, key: PropertyKey): boolean {
  return Object.prototype.hasOwnProperty.call(object, key)
}

function valueHasEntries(value: unknown): boolean {
  return (Array.isArray(value) && value.length > 0)
    || (isRecord(value) && Object.keys(value).length > 0)
}

function hasStandardPricingData(pricing: ProviderTieredPricingConfig): boolean {
  return (Array.isArray(pricing.tiers) && pricing.tiers.length > 0)
    || (typeof pricing.image_output_price_default === 'number'
      && Number.isFinite(pricing.image_output_price_default))
    || [
      'image_output_prices',
      'image_output_price_ranges',
      'image_output_price_per_image',
      'image_output_price_matrix',
      'image_prices',
    ].some(key => valueHasEntries(pricing[key]))
}

function pricingRoot(pricing: ProviderTieredPricingConfig): Record<string, unknown> {
  return Object.fromEntries(
    Object.entries(pricing).filter(([key]) => key !== 'processing_tiers'),
  )
}

function processingTierEntries(pricing: ProviderTieredPricingConfig | null | undefined) {
  return isRecord(pricing?.processing_tiers)
    ? Object.entries(pricing.processing_tiers)
    : []
}

function jsonValuesEqual(left: unknown, right: unknown): boolean {
  if (Object.is(left, right)) return true
  if (Array.isArray(left) || Array.isArray(right)) {
    return Array.isArray(left)
      && Array.isArray(right)
      && left.length === right.length
      && left.every((value, index) => jsonValuesEqual(value, right[index]))
  }
  if (!isRecord(left) || !isRecord(right)) return false
  const leftKeys = Object.keys(left).sort()
  const rightKeys = Object.keys(right).sort()
  return leftKeys.length === rightKeys.length
    && leftKeys.every((key, index) => (
      key === rightKeys[index]
      && jsonValuesEqual(left[key], right[key])
    ))
}

/**
 * Build the complete catalog shown by the editor from the two independent
 * runtime sources: GlobalModel Standard/default overlays and Provider overrides.
 */
export function mergeProviderTieredPricingForEditing(
  globalDefault: ProviderTieredPricingConfig | null | undefined,
  providerOverride: ProviderTieredPricingConfig | null | undefined,
): TieredPricingConfig | null {
  if (!providerOverride) {
    return Array.isArray(globalDefault?.tiers)
      ? cloneJson(globalDefault) as TieredPricingConfig
      : null
  }

  const providerHasStandard = hasStandardPricingData(providerOverride)
  const providerRoot = pricingRoot(providerOverride)
  let mergedRoot: Record<string, unknown>
  if (providerHasStandard) {
    mergedRoot = providerRoot
  } else if (globalDefault) {
    const providerMetadata = Object.fromEntries(
      Object.entries(providerRoot).filter(([key]) => !STANDARD_PRICING_KEYS.has(key)),
    )
    mergedRoot = {
      ...pricingRoot(globalDefault),
      ...providerMetadata,
    }
  } else {
    return null
  }

  if (!Array.isArray(mergedRoot.tiers)) return null

  const mergedProcessingTiers = Object.fromEntries([
    ...processingTierEntries(globalDefault),
    ...processingTierEntries(providerOverride),
  ])
  if (Object.keys(mergedProcessingTiers).length > 0) {
    mergedRoot.processing_tiers = mergedProcessingTiers
  }
  return cloneJson(mergedRoot) as TieredPricingConfig
}

/**
 * Project the complete editor catalog back to the smallest Provider override.
 * Unchanged Standard and processing-tier values keep inheriting from GlobalModel.
 */
export function buildProviderTieredPricingOverride(
  finalPricing: TieredPricingConfig | null,
  originalEditorPricing: TieredPricingConfig | null,
  originalProviderOverride: ProviderTieredPricingConfig | null | undefined,
): ProviderTieredPricingConfig | null {
  if (!finalPricing) return null
  if (!originalEditorPricing) return cloneJson(finalPricing)

  const originalProcessingTiers = originalProviderOverride?.processing_tiers
  const preservedProcessingTiers = hasOwn(originalProviderOverride || {}, 'processing_tiers')
    ? originalProcessingTiers === undefined
      ? undefined
      : cloneJson(originalProcessingTiers)
    : undefined
  let result = cloneJson(originalProviderOverride || {})

  if (!jsonValuesEqual(pricingRoot(finalPricing), pricingRoot(originalEditorPricing))) {
    result = cloneJson(pricingRoot(finalPricing)) as ProviderTieredPricingConfig
    if (preservedProcessingTiers !== undefined || originalProcessingTiers === null) {
      result.processing_tiers = preservedProcessingTiers ?? null
    }
  }

  const finalProcessingTiers = new Map(processingTierEntries(finalPricing))
  const baselineProcessingTiers = new Map(processingTierEntries(originalEditorPricing))
  const nextProcessingTiers = new Map(processingTierEntries(result))
  let processingTiersChanged = false
  const keys = new Set([
    ...finalProcessingTiers.keys(),
    ...baselineProcessingTiers.keys(),
  ])
  for (const key of keys) {
    const finalOverlay = finalProcessingTiers.get(key)
    const baselineOverlay = baselineProcessingTiers.get(key)
    if (jsonValuesEqual(finalOverlay, baselineOverlay)) continue
    processingTiersChanged = true
    if (finalOverlay === undefined) nextProcessingTiers.delete(key)
    else nextProcessingTiers.set(key, cloneJson(finalOverlay))
  }

  if (processingTiersChanged) {
    if (nextProcessingTiers.size > 0) {
      result.processing_tiers = Object.fromEntries(nextProcessingTiers)
    } else {
      delete result.processing_tiers
    }
  }

  return Object.keys(result).length > 0 ? result : null
}

export function buildProviderModelCreatePayload(input: ProviderModelCreatePayloadInput): ModelCreate {
  return {
    global_model_id: input.globalModelId,
    provider_model_name: input.providerModelName,
    tiered_pricing: input.tieredPricingModified && input.finalTieredPricing ? input.finalTieredPricing : undefined,
    price_per_request: input.pricePerRequestModified ? input.pricePerRequest : undefined,
    config: input.configTouched ? input.cleanConfig : undefined,
    supports_vision: input.supportsVision,
    supports_function_calling: input.supportsFunctionCalling,
    supports_streaming: input.supportsStreaming,
    supports_extended_thinking: input.supportsExtendedThinking,
    supports_image_generation: input.supportsImageGeneration,
    is_active: input.isActive,
  }
}

export function buildProviderModelUpdatePayload(input: ProviderModelUpdatePayloadInput): ModelUpdate {
  return {
    ...(input.tieredPricingModified ? { tiered_pricing: input.finalTieredPricing } : {}),
    ...(input.pricePerRequestModified
      ? { price_per_request: input.pricePerRequest ?? null }
      : {}),
    ...(input.configTouched ? { config: input.cleanConfig || null } : {}),
    supports_vision: input.supportsVision,
    supports_function_calling: input.supportsFunctionCalling,
    supports_streaming: input.supportsStreaming,
    supports_extended_thinking: input.supportsExtendedThinking,
    supports_image_generation: input.supportsImageGeneration,
    is_active: input.isActive,
  }
}
