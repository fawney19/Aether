import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, nextTick, type App } from 'vue'

import type { GlobalModelResponse } from '@/api/global-models'
import type { ModelsDevModelItem } from '@/api/models-dev'
import { setI18nLocale } from '@/i18n'
import ModelManagement from '@/views/admin/ModelManagement.vue'

const apiMocks = vi.hoisted(() => ({
  listGlobalModels: vi.fn(),
  getGlobalModel: vi.fn(),
  updateGlobalModel: vi.fn(),
  deleteGlobalModel: vi.fn(),
  batchDeleteGlobalModels: vi.fn(),
  batchAssignToProviders: vi.fn(),
  getGlobalModelProviders: vi.fn(),
  getModelsDevList: vi.fn(),
  getProvidersSummary: vi.fn(),
}))

const interactionMocks = vi.hoisted(() => ({
  confirm: vi.fn(),
  confirmDanger: vi.fn(),
  success: vi.fn(),
  error: vi.fn(),
}))

vi.mock('@/api/global-models', () => ({
  listGlobalModels: apiMocks.listGlobalModels,
  getGlobalModel: apiMocks.getGlobalModel,
  updateGlobalModel: apiMocks.updateGlobalModel,
  deleteGlobalModel: apiMocks.deleteGlobalModel,
  batchDeleteGlobalModels: apiMocks.batchDeleteGlobalModels,
  batchAssignToProviders: apiMocks.batchAssignToProviders,
  getGlobalModelProviders: apiMocks.getGlobalModelProviders,
}))

vi.mock('@/api/models-dev', () => ({
  getModelsDevList: apiMocks.getModelsDevList,
}))

vi.mock('@/api/endpoints/providers', () => ({
  getProvidersSummary: apiMocks.getProvidersSummary,
}))

vi.mock('@/composables/useConfirm', () => ({
  useConfirm: () => ({
    confirm: interactionMocks.confirm,
    confirmDanger: interactionMocks.confirmDanger,
  }),
}))

vi.mock('@/composables/useToast', () => ({
  useToast: () => ({
    success: interactionMocks.success,
    error: interactionMocks.error,
  }),
}))

vi.mock('@/composables/useClipboard', () => ({
  useClipboard: () => ({ copyToClipboard: vi.fn() }),
}))

vi.mock('@/features/models/components/GlobalModelFormDialog.vue', async () => {
  const { defineComponent } = await import('vue')
  return {
    default: defineComponent({
      name: 'ChildStub',
      setup: () => () => null,
    }),
  }
})

vi.mock('@/features/models/components/ModelDetailDrawer.vue', async () => {
  const { defineComponent } = await import('vue')
  return {
    default: defineComponent({
      name: 'ChildStub',
      setup: () => () => null,
    }),
  }
})

vi.mock('@/features/models/components/ExternalModelsAccessControl.vue', async () => {
  const { defineComponent } = await import('vue')
  return {
    default: defineComponent({
      name: 'ChildStub',
      setup: () => () => null,
    }),
  }
})

vi.mock('@/features/providers/components/ProviderModelFormDialog.vue', async () => {
  const { defineComponent } = await import('vue')
  return {
    default: defineComponent({
      name: 'ChildStub',
      setup: () => () => null,
    }),
  }
})

const pricing = {
  tiers: [{
    up_to: null,
    input_price_per_1m: 1,
    output_price_per_1m: 2,
  }],
}

const onlineModel: ModelsDevModelItem = {
  providerId: 'openai',
  providerName: 'OpenAI',
  modelId: 'test-model',
  modelName: 'Test Model',
  official: true,
  inputPrice: 1,
  outputPrice: 2,
  tieredPricing: pricing,
}

let mountedApp: App | null = null
let mountedRoot: HTMLElement | null = null
let persistedModel: GlobalModelResponse

function cloneModel(model: GlobalModelResponse): GlobalModelResponse {
  return JSON.parse(JSON.stringify(model)) as GlobalModelResponse
}

async function settle() {
  for (let index = 0; index < 8; index += 1) {
    await Promise.resolve()
    await nextTick()
  }
}

function findButton(text: string): HTMLButtonElement {
  const button = [...document.body.querySelectorAll('button')]
    .find(candidate => candidate.textContent?.trim().includes(text))
  if (!(button instanceof HTMLButtonElement)) {
    throw new Error(`Missing button containing: ${text}`)
  }
  return button
}

function mountView() {
  mountedRoot = document.createElement('div')
  document.body.appendChild(mountedRoot)
  mountedApp = createApp(ModelManagement)
  mountedApp.mount(mountedRoot)
}

beforeEach(() => {
  persistedModel = {
    id: 'model-1',
    name: 'test-model',
    display_name: 'Test Model',
    is_active: true,
    default_tiered_pricing: pricing,
    config: {
      streaming: true,
      models_dev_pricing_source: {
        provider_id: 'openai',
        provider_name: 'Old OpenAI label',
      },
    },
    provider_count: 1,
    active_provider_count: 1,
    usage_count: 0,
    created_at: '2026-09-03T00:00:00Z',
  }

  for (const mock of Object.values(apiMocks)) mock.mockReset()
  for (const mock of Object.values(interactionMocks)) mock.mockReset()

  apiMocks.listGlobalModels.mockImplementation(async () => ({
    models: [cloneModel(persistedModel)],
    total: 1,
  }))
  apiMocks.updateGlobalModel.mockImplementation(async (
    _modelId: string,
    payload: Partial<GlobalModelResponse>,
  ) => {
    persistedModel = { ...persistedModel, ...payload }
    return cloneModel(persistedModel)
  })
  apiMocks.getModelsDevList.mockResolvedValue([onlineModel])
  apiMocks.getGlobalModelProviders.mockResolvedValue({ providers: [] })
  apiMocks.getProvidersSummary.mockResolvedValue({ items: [] })
  interactionMocks.confirm.mockResolvedValue(true)
  interactionMocks.confirmDanger.mockResolvedValue(true)
})

afterEach(() => {
  mountedApp?.unmount()
  mountedRoot?.remove()
  mountedApp = null
  mountedRoot = null
  document.body.innerHTML = ''
})

describe('ModelManagement pricing-source workflow', () => {
  it('keeps list selection in batch management and refreshes stale source metadata when prices match', async () => {
    mountView()
    await settle()

    const desktopCheckbox = document.body.querySelector<HTMLInputElement>(
      '[data-testid="model-select-desktop-model-1"]',
    )
    const mobileCheckbox = document.body.querySelector<HTMLInputElement>(
      '[data-testid="model-select-mobile-model-1"]',
    )
    expect(desktopCheckbox).not.toBeNull()
    expect(mobileCheckbox).not.toBeNull()

    desktopCheckbox!.checked = true
    desktopCheckbox!.dispatchEvent(new Event('change', { bubbles: true }))
    await settle()

    expect(mobileCheckbox!.checked).toBe(true)
    expect(document.body.textContent).toContain('已选 1 个')
    findButton('批量操作 (1)').click()
    await settle()

    expect(document.body.textContent).toContain('已选择 1 个')
    expect(document.body.textContent).toContain('来源待保存')
    findButton('同步价格与来源 (1)').click()
    await settle()

    expect(interactionMocks.confirm).toHaveBeenCalledOnce()
    expect(apiMocks.updateGlobalModel).toHaveBeenCalledWith('model-1', {
      default_tiered_pricing: pricing,
      config: {
        streaming: true,
        models_dev_pricing_source: {
          provider_id: 'openai',
          provider_name: 'OpenAI',
        },
      },
    })
    expect(persistedModel.config).toEqual({
      streaming: true,
      models_dev_pricing_source: {
        provider_id: 'openai',
        provider_name: 'OpenAI',
      },
    })
  })

  it('does not render the pricing source in the model list', async () => {
    mountView()
    await settle()
    setI18nLocale('en-US')
    await settle()

    expect(document.body.textContent).not.toContain('Online pricing source')
    expect(document.body.textContent).toContain('Batch manage')
    expect(document.body.querySelector(
      '[aria-label="Select model Test Model"]',
    )).not.toBeNull()
    expect(document.body.querySelector(
      '[data-testid="model-pricing-source-model-1"]',
    )).toBeNull()
  })

  it('migrates a legacy browser-only source into the model database config', async () => {
    persistedModel.config = { streaming: true }
    localStorage.setItem('aether:models-dev-pricing-sources:v1', JSON.stringify({
      version: 1,
      models: {
        'model-1': {
          provider_id: 'openai',
          provider_name: 'OpenAI',
        },
      },
    }))

    mountView()
    await settle()

    expect(apiMocks.updateGlobalModel).toHaveBeenCalledWith('model-1', {
      config: {
        streaming: true,
        models_dev_pricing_source: {
          provider_id: 'openai',
          provider_name: 'OpenAI',
        },
      },
    })
    expect(persistedModel.config).toHaveProperty(
      'models_dev_pricing_source.provider_id',
      'openai',
    )
  })
})
