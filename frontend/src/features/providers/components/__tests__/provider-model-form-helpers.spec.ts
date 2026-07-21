import { describe, expect, it } from 'vitest'

import {
  buildProviderModelCreatePayload,
  buildProviderModelUpdatePayload,
  buildProviderTieredPricingOverride,
  mergeProviderTieredPricingForEditing,
  modelSupportsEmbedding,
} from '../provider-model-form-helpers'

const pricing = {
  tiers: [{ up_to: null, input_price_per_1m: 0.02, output_price_per_1m: 0 }],
}

describe('provider model form embedding helpers', () => {
  it.each([
    { supported_capabilities: ['embedding'], config: {} },
    { supported_capabilities: null, config: { embedding: true } },
    { supported_capabilities: null, config: { model_type: 'embedding' } },
    { supported_capabilities: null, config: { api_formats: ['doubao:embedding'] } },
    { supported_capabilities: null, config: { api_formats: ['aliyun:multimodal_embedding'] } },
    { supports_embedding: true, effective_supports_embedding: null, config: {} },
    { supports_embedding: null, effective_supports_embedding: true, config: {} },
  ])('detects embedding metadata from %o', (model) => {
    expect(modelSupportsEmbedding(model)).toBe(true)
  })

  it('keeps provider create payload inherited from the selected embedding global model', () => {
    const payload = buildProviderModelCreatePayload({
      globalModelId: 'gm-embedding',
      providerModelName: 'text-embedding-3-small',
      finalTieredPricing: pricing,
      tieredPricingModified: false,
      pricePerRequest: 0.25,
      pricePerRequestModified: false,
      cleanConfig: {
        embedding: true,
        model_type: 'embedding',
        api_formats: ['openai:embedding'],
      },
      configTouched: false,
      supportsStreaming: false,
      isActive: true,
    })

    expect(payload).toMatchObject({
      global_model_id: 'gm-embedding',
      provider_model_name: 'text-embedding-3-small',
      tiered_pricing: undefined,
      price_per_request: undefined,
      config: undefined,
      supports_streaming: false,
    })
    expect('supports_embedding' in payload).toBe(false)
  })

  it('uses manually supplied provider model name in create payload', () => {
    const payload = buildProviderModelCreatePayload({
      globalModelId: 'gm-local-manual',
      providerModelName: 'intranet-chat-model-v1',
      finalTieredPricing: pricing,
      tieredPricingModified: false,
      pricePerRequest: undefined,
      pricePerRequestModified: false,
      cleanConfig: undefined,
      configTouched: false,
      isActive: true,
    })

    expect(payload).toMatchObject({
      global_model_id: 'gm-local-manual',
      provider_model_name: 'intranet-chat-model-v1',
      tiered_pricing: undefined,
      price_per_request: undefined,
      config: undefined,
      is_active: true,
    })
  })

  it('preserves edited provider embedding config without posting unsupported embedding controls', () => {
    const payload = buildProviderModelUpdatePayload({
      finalTieredPricing: pricing,
      tieredPricingModified: true,
      pricePerRequest: undefined,
      pricePerRequestModified: false,
      cleanConfig: {
        streaming: false,
        embedding: true,
        model_type: 'embedding',
        api_formats: ['gemini:embedding'],
      },
      configTouched: true,
      supportsStreaming: false,
      isActive: true,
    })

    expect(payload.config).toEqual({
      streaming: false,
      embedding: true,
      model_type: 'embedding',
      api_formats: ['gemini:embedding'],
    })
    expect(payload.tiered_pricing).toEqual(pricing)
    expect(payload.supports_streaming).toBe(false)
    expect('supports_embedding' in payload).toBe(false)
  })

  it('keeps inherited pricing and config out of an unchanged provider update', () => {
    const payload = buildProviderModelUpdatePayload({
      finalTieredPricing: pricing,
      tieredPricingModified: false,
      pricePerRequest: 0.25,
      pricePerRequestModified: false,
      cleanConfig: { billing: { video: { price_per_second_by_resolution: { '720p': 0.1 } } } },
      configTouched: false,
      isActive: true,
    })

    expect(payload).not.toHaveProperty('tiered_pricing')
    expect(payload).not.toHaveProperty('price_per_request')
    expect(payload).not.toHaveProperty('config')
  })

  it('writes an explicitly edited per-request price and supports clearing it', () => {
    const edited = buildProviderModelUpdatePayload({
      finalTieredPricing: pricing,
      tieredPricingModified: false,
      pricePerRequest: 0.5,
      pricePerRequestModified: true,
      cleanConfig: undefined,
      configTouched: false,
      isActive: true,
    })
    const cleared = buildProviderModelUpdatePayload({
      finalTieredPricing: pricing,
      tieredPricingModified: false,
      pricePerRequest: undefined,
      pricePerRequestModified: true,
      cleanConfig: undefined,
      configTouched: false,
      isActive: true,
    })

    expect(edited.price_per_request).toBe(0.5)
    expect(cleared.price_per_request).toBeNull()
  })
})

describe('provider model pricing override helpers', () => {
  const inheritedPricing = {
    tiers: [{ up_to: null, input_price_per_1m: 5, output_price_per_1m: 30 }],
    future_global_option: 'inherit-only',
    processing_tiers: {
      priority: { price_multiplier: 2.5 },
      fast: { price_multiplier: 2 },
    },
  }

  it('projects a processing-tier edit without freezing inherited Standard or other tiers', () => {
    const finalPricing = structuredClone(inheritedPricing)
    finalPricing.processing_tiers.priority.price_multiplier = 3

    const override = buildProviderTieredPricingOverride(
      finalPricing,
      inheritedPricing,
      null,
    )

    expect(override).toEqual({
      processing_tiers: {
        priority: { price_multiplier: 3 },
      },
    })
    expect(override).not.toHaveProperty('tiers')
    expect(override?.processing_tiers).not.toHaveProperty('fast')
    expect(override).not.toHaveProperty('future_global_option')

    const createPayload = buildProviderModelCreatePayload({
      globalModelId: 'global-model-1',
      providerModelName: 'gpt-test',
      finalTieredPricing: override,
      tieredPricingModified: true,
      pricePerRequestModified: false,
      configTouched: false,
      isActive: true,
    })
    expect(createPayload.tiered_pricing).toEqual(override)
    expect(createPayload.tiered_pricing).not.toHaveProperty('tiers')
  })

  it('keeps an existing Provider Standard override while adding only the edited tier', () => {
    const providerStandard = {
      tiers: [{ up_to: null, input_price_per_1m: 7, output_price_per_1m: 42 }],
      provider_contract: 'keep-provider-standard',
    }
    const editorPricing = {
      ...structuredClone(providerStandard),
      processing_tiers: structuredClone(inheritedPricing.processing_tiers),
    }
    const finalPricing = structuredClone(editorPricing)
    finalPricing.processing_tiers.priority.price_multiplier = 3

    const override = buildProviderTieredPricingOverride(
      finalPricing,
      editorPricing,
      providerStandard,
    )

    expect(override).toEqual({
      ...providerStandard,
      processing_tiers: {
        priority: { price_multiplier: 3 },
      },
    })
    expect(override?.processing_tiers).not.toHaveProperty('fast')
  })

  it('merges a saved processing-only override for editing and stays partial on the next save', () => {
    const savedOverride = {
      processing_tiers: {
        priority: { price_multiplier: 3 },
      },
    }
    const reopenedEditorPricing = mergeProviderTieredPricingForEditing(
      inheritedPricing,
      savedOverride,
    )

    expect(reopenedEditorPricing).toEqual({
      ...inheritedPricing,
      processing_tiers: {
        priority: { price_multiplier: 3 },
        fast: { price_multiplier: 2 },
      },
    })

    const finalPricing = structuredClone(reopenedEditorPricing!)
    finalPricing.processing_tiers!.priority.price_multiplier = 4
    expect(buildProviderTieredPricingOverride(
      finalPricing,
      reopenedEditorPricing,
      savedOverride,
    )).toEqual({
      processing_tiers: {
        priority: { price_multiplier: 4 },
      },
    })
  })
})
