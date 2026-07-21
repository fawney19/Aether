import type { GlobalModelCreate, GlobalModelUpdate } from '@/api/global-models'
import type { TieredPricingConfig } from '@/api/endpoints/types'

export const EMBEDDING_API_FORMATS = [
  'openai:embedding',
  'gemini:embedding',
  'jina:embedding',
  'doubao:embedding',
  'aliyun:multimodal_embedding',
] as const

export const RERANK_API_FORMATS = [
  'openai:rerank',
  'jina:rerank',
] as const

export interface GlobalModelFormPayloadState {
  name: string
  display_name: string
  default_price_per_request?: number
  supported_capabilities?: string[]
  config?: Record<string, unknown>
  is_active?: boolean
}

export function cloneTieredPricingConfig(
  pricing: TieredPricingConfig,
): TieredPricingConfig {
  // Presets live inside a Vue ref and can therefore be reactive proxies, which
  // structuredClone cannot clone directly. Pricing configs are JSON payloads.
  return JSON.parse(JSON.stringify(pricing)) as TieredPricingConfig
}

function cleanGlobalModelConfig(form: GlobalModelFormPayloadState): Record<string, unknown> | undefined {
  return form.config && Object.keys(form.config).length > 0 ? form.config : undefined
}

export function buildGlobalModelCreatePayload(
  form: GlobalModelFormPayloadState,
  defaultTieredPricing: TieredPricingConfig,
): GlobalModelCreate {
  return {
    name: form.name ?? '',
    display_name: form.display_name ?? '',
    config: cleanGlobalModelConfig(form),
    default_price_per_request: form.default_price_per_request ?? undefined,
    default_tiered_pricing: defaultTieredPricing,
    supported_capabilities: form.supported_capabilities?.length ? form.supported_capabilities : undefined,
    is_active: form.is_active,
  }
}

export function buildGlobalModelUpdatePayload(
  form: GlobalModelFormPayloadState,
  defaultTieredPricing: TieredPricingConfig,
): GlobalModelUpdate {
  return {
    display_name: form.display_name,
    config: cleanGlobalModelConfig(form) || null,
    default_price_per_request: form.default_price_per_request ?? null,
    default_tiered_pricing: defaultTieredPricing,
    supported_capabilities: form.supported_capabilities?.length ? form.supported_capabilities : null,
    is_active: form.is_active,
  }
}
