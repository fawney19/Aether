import { describe, expect, it } from 'vitest'
import { reactive } from 'vue'

import {
  EMBEDDING_API_FORMATS,
  buildGlobalModelCreatePayload,
  buildGlobalModelUpdatePayload,
  cloneTieredPricingConfig,
} from '../global-model-form-helpers'
import type { TieredPricingConfig } from '@/api/endpoints/types'

const embeddingPricing = {
  tiers: [{ up_to: null, input_price_per_1m: 0.02, output_price_per_1m: 0 }],
}

describe('global model form embedding payload helpers', () => {
  it('preserves embedding metadata in create payloads', () => {
    const payload = buildGlobalModelCreatePayload({
      name: 'text-embedding-3-small',
      display_name: 'text-embedding-3-small',
      supported_capabilities: ['embedding'],
      config: {
        streaming: false,
        embedding: true,
        model_type: 'embedding',
        api_formats: [...EMBEDDING_API_FORMATS],
      },
      is_active: true,
    }, embeddingPricing)

    expect(payload).toMatchObject({
      name: 'text-embedding-3-small',
      supported_capabilities: ['embedding'],
      config: {
        streaming: false,
        embedding: true,
        model_type: 'embedding',
        api_formats: [
          'openai:embedding',
          'gemini:embedding',
          'jina:embedding',
          'doubao:embedding',
          'aliyun:multimodal_embedding',
        ],
      },
    })
  })

  it('preserves embedding metadata in update payloads', () => {
    const payload = buildGlobalModelUpdatePayload({
      name: 'unused-on-update',
      display_name: 'Jina Embeddings v3',
      supported_capabilities: ['embedding'],
      config: {
        streaming: false,
        embedding: true,
        model_type: 'embedding',
        api_formats: ['jina:embedding'],
      },
      is_active: true,
    }, embeddingPricing)

    expect(payload.supported_capabilities).toEqual(['embedding'])
    expect(payload.config).toEqual({
      streaming: false,
      embedding: true,
      model_type: 'embedding',
      api_formats: ['jina:embedding'],
    })
  })
})

describe('global model form pricing presets', () => {
  it('clones reactive pricing before opening the preset editor', () => {
    const pricing = reactive({
      tiers: [{
        up_to: null,
        input_price_per_1m: 3,
        output_price_per_1m: 15,
      }],
    }) as TieredPricingConfig

    const cloned = cloneTieredPricingConfig(pricing)

    expect(cloned).toEqual(pricing)
    expect(cloned).not.toBe(pricing)
    expect(cloned.tiers[0]).not.toBe(pricing.tiers[0])

    cloned.tiers[0].input_price_per_1m = 9
    expect(pricing.tiers[0].input_price_per_1m).toBe(3)
  })
})
