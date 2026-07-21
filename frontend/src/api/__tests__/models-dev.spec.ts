import { beforeEach, describe, expect, it, vi } from 'vitest'

const apiMocks = vi.hoisted(() => ({
  get: vi.fn(),
}))

vi.mock('@/api/client', () => ({
  default: { get: apiMocks.get },
}))

import { clearModelsDevCache, getModelsDevList } from '@/api/models-dev'

beforeEach(() => {
  clearModelsDevCache()
  localStorage.clear()
  apiMocks.get.mockReset()
})

describe('getModelsDevList', () => {
  it('uses current modalities and experimental mode pricing while keeping legacy fallbacks', async () => {
    apiMocks.get.mockResolvedValue({
      data: {
        openai: {
          id: 'openai',
          name: 'OpenAI',
          official: true,
          models: {
            'gpt-test': {
              id: 'gpt-test',
              name: 'GPT Test',
              input: ['text'],
              output: ['text'],
              modalities: {
                input: ['text', 'image'],
                output: ['text', 'image'],
              },
              cost: { input: 2, output: 4 },
              experimental: {
                modes: {
                  fast: {
                    cost: { input: 4, output: 8 },
                    provider: { body: { service_tier: 'priority' } },
                  },
                },
              },
            },
            legacy: {
              id: 'legacy',
              name: 'Legacy',
              input: ['text', 'image'],
              output: ['text'],
              cost: { input: 1, output: 2 },
            },
          },
        },
      },
    })

    const models = await getModelsDevList()
    const current = models.find(model => model.modelId === 'gpt-test')
    const legacy = models.find(model => model.modelId === 'legacy')

    expect(current).toMatchObject({
      supportsVision: true,
      inputModalities: ['text', 'image'],
      outputModalities: ['text', 'image'],
      tieredPricing: {
        processing_tiers: { priority: { price_multiplier: 2 } },
      },
    })
    expect(legacy).toMatchObject({
      supportsVision: true,
      inputModalities: ['text', 'image'],
      outputModalities: ['text'],
    })
  })
})
