import { beforeEach, describe, expect, it } from 'vitest'

import {
  getModelsDevPricingSourceFromConfig,
  modelsDevPricingSourcesEqual,
  useModelsDevPricingSources,
  withModelsDevPricingSource,
} from '../useModelsDevPricingSources'

const STORAGE_KEY = 'aether:models-dev-pricing-sources:v1'
const LEGACY_STORAGE_KEY = 'aether:models-dev-pricing-preferences:v1'

describe('useModelsDevPricingSources', () => {
  beforeEach(() => {
    localStorage.clear()
  })

  it('stores the provider used by a manual pricing action', () => {
    const { getSource, setSource } = useModelsDevPricingSources()

    setSource('model-1', {
      provider_id: 'openai',
      provider_name: 'OpenAI',
    })

    expect(getSource('model-1')).toEqual({
      provider_id: 'openai',
      provider_name: 'OpenAI',
    })
    expect(JSON.parse(localStorage.getItem(STORAGE_KEY) || 'null')).toEqual({
      version: 1,
      models: {
        'model-1': {
          provider_id: 'openai',
          provider_name: 'OpenAI',
        },
      },
    })
  })

  it('prefers the database-backed model config over the local migration fallback', () => {
    const { getSource, setSource } = useModelsDevPricingSources()
    setSource('model-1', {
      provider_id: 'openai',
      provider_name: 'OpenAI',
    })

    expect(getSource('model-1', {
      models_dev_pricing_source: {
        provider_id: 'anthropic',
        provider_name: 'Anthropic',
      },
    })).toEqual({
      provider_id: 'anthropic',
      provider_name: 'Anthropic',
    })
  })

  it('migrates the previous provider record without retaining its automatic preference key', () => {
    localStorage.setItem(LEGACY_STORAGE_KEY, JSON.stringify({
      version: 1,
      models: {
        'model-1': {
          provider_id: 'anthropic',
          provider_name: 'Anthropic',
        },
      },
    }))

    const { getSource } = useModelsDevPricingSources()

    expect(getSource('model-1')).toEqual({
      provider_id: 'anthropic',
      provider_name: 'Anthropic',
    })
    expect(localStorage.getItem(LEGACY_STORAGE_KEY)).toBeNull()
    expect(localStorage.getItem(STORAGE_KEY)).not.toBeNull()
  })

  it.each([
    '{broken',
    JSON.stringify({ version: 2, models: {} }),
    JSON.stringify({ version: 1, models: [] }),
  ])('ignores incompatible or malformed source documents', (stored) => {
    localStorage.setItem(STORAGE_KEY, stored)

    const { getSource } = useModelsDevPricingSources()

    expect(getSource('model-1')).toBeNull()
  })
})

describe('database-backed models.dev pricing sources', () => {
  it('merges the source into model config without dropping unrelated settings', () => {
    const config = withModelsDevPricingSource({
      streaming: true,
      billing: { video: { price_per_second: 0.1 } },
    }, {
      provider_id: ' google ',
      provider_name: ' Google ',
    })

    expect(config).toEqual({
      streaming: true,
      billing: { video: { price_per_second: 0.1 } },
      models_dev_pricing_source: {
        provider_id: 'google',
        provider_name: 'Google',
      },
    })
    expect(getModelsDevPricingSourceFromConfig(config)).toEqual({
      provider_id: 'google',
      provider_name: 'Google',
    })
  })

  it('rejects malformed config records and compares provider ids case-insensitively', () => {
    expect(getModelsDevPricingSourceFromConfig({
      models_dev_pricing_source: { provider_id: '', provider_name: 'Missing id' },
    })).toBeNull()
    expect(modelsDevPricingSourcesEqual(
      { provider_id: 'OpenAI', provider_name: 'OpenAI' },
      { provider_id: 'openai', provider_name: 'OpenAI' },
    )).toBe(true)
  })
})
