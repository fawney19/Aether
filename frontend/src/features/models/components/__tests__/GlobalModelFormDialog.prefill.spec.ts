import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  createApp,
  defineComponent,
  h,
  nextTick,
  ref,
  type App,
} from 'vue'

import type { ModelsDevModelItem } from '@/api/models-dev'
import type { GlobalModelResponse } from '@/api/global-models'
import GlobalModelFormDialog from '../GlobalModelFormDialog.vue'

const modelsDevMocks = vi.hoisted(() => ({
  getModelsDevList: vi.fn(),
  refreshModelsDevList: vi.fn(),
}))

const globalModelMocks = vi.hoisted(() => ({
  createGlobalModel: vi.fn(),
  listGlobalModels: vi.fn(),
  updateGlobalModel: vi.fn(),
}))

vi.mock('@/api/models-dev', () => ({
  getModelsDevList: modelsDevMocks.getModelsDevList,
  refreshModelsDevList: modelsDevMocks.refreshModelsDevList,
  getProviderLogoUrl: (providerId: string) => `/logos/${providerId}.svg`,
}))

vi.mock('@/api/global-models', () => ({
  createGlobalModel: globalModelMocks.createGlobalModel,
  listGlobalModels: globalModelMocks.listGlobalModels,
  updateGlobalModel: globalModelMocks.updateGlobalModel,
}))

const mountedApps: Array<{ app: App, root: HTMLElement }> = []

const stalePreset: ModelsDevModelItem = {
  providerId: 'openai',
  providerName: 'OpenAI',
  modelId: 'stale-model',
  modelName: 'Stale Model',
  official: true,
  supportsReasoning: true,
  inputPrice: 1,
  outputPrice: 2,
  tieredPricing: {
    tiers: [{
      up_to: null,
      input_price_per_1m: 1,
      output_price_per_1m: 2,
    }],
    processing_tiers: {
      priority: {
        tiers: [{
          up_to: null,
          input_price_per_1m: 2,
          output_price_per_1m: 4,
        }],
      },
    },
  },
}

const alternateStalePreset: ModelsDevModelItem = {
  ...stalePreset,
  providerId: 'azure-openai',
  providerName: 'Azure OpenAI',
  inputPrice: 7,
  outputPrice: 8,
  tieredPricing: {
    tiers: [{
      up_to: null,
      input_price_per_1m: 7,
      output_price_per_1m: 8,
    }],
  },
}

const unavailableStalePreset: ModelsDevModelItem = {
  ...stalePreset,
  pricingUnsupportedFields: ['reasoning'],
  tieredPricing: undefined,
}

const freshPreset: ModelsDevModelItem = {
  providerId: 'openai',
  providerName: 'OpenAI',
  modelId: 'fresh-model',
  modelName: 'Fresh Model',
  family: 'fresh-family',
  official: true,
  supportsTemperature: false,
  contextLimit: 128_000,
  outputLimit: 4_096,
  inputModalities: ['text'],
  outputModalities: ['text'],
  inputPrice: 3,
  outputPrice: 4,
  tieredPricing: {
    tiers: [
      {
        up_to: 99_999,
        input_price_per_1m: 3,
        output_price_per_1m: 4,
      },
      {
        up_to: null,
        input_price_per_1m: 5,
        output_price_per_1m: 6,
      },
    ],
  },
}

const unsupportedPreset: ModelsDevModelItem = {
  providerId: 'openai',
  providerName: 'OpenAI',
  modelId: 'reasoning-priced-model',
  modelName: 'Reasoning Priced Model',
  official: true,
  inputPrice: 1,
  outputPrice: 2,
  pricingUnsupportedFields: ['reasoning'],
}

function buildExistingStaleModel(): GlobalModelResponse {
  return {
    id: 'global-stale-model',
    name: 'stale-model',
    display_name: 'Configured Stale Model',
    is_active: true,
    default_tiered_pricing: {
      tiers: [{
        up_to: null,
        input_price_per_1m: 9,
        output_price_per_1m: 18,
      }],
    },
    config: { streaming: true },
    created_at: '2026-07-23T00:00:00Z',
  }
}

function mountDialog() {
  const root = document.createElement('div')
  document.body.appendChild(root)
  const open = ref(false)
  const editingModel = ref<GlobalModelResponse | null>(null)
  const editModel = vi.fn((model: GlobalModelResponse) => {
    editingModel.value = model
  })
  const pricingSynced = vi.fn()
  const app = createApp(defineComponent({
    setup() {
      return () => h(GlobalModelFormDialog, {
        open: open.value,
        model: editingModel.value,
        onEditModel: editModel,
        onPricingSynced: pricingSynced,
        'onUpdate:open': (value: boolean) => { open.value = value },
      })
    },
  }))
  app.mount(root)
  mountedApps.push({ app, root })
  open.value = true
  return { root, open, editingModel, editModel, pricingSynced }
}

async function settle() {
  for (let index = 0; index < 5; index += 1) {
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

function findExactButton(text: string): HTMLButtonElement {
  const button = [...document.body.querySelectorAll('button')]
    .find(candidate => candidate.textContent?.trim() === text)
  if (!(button instanceof HTMLButtonElement)) {
    throw new Error(`Missing button: ${text}`)
  }
  return button
}

function findExistingEditButton(modelId: string): HTMLButtonElement {
  const button = document.body.querySelector(
    `[data-testid="edit-existing-model-${modelId}"]`,
  )
  if (!(button instanceof HTMLButtonElement)) {
    throw new Error(`Missing existing-model edit button: ${modelId}`)
  }
  return button
}

function findBillingTab(value: string): HTMLButtonElement {
  const button = document.body.querySelector(`button[data-value="${value}"]`)
  if (!(button instanceof HTMLButtonElement)) {
    throw new Error(`Missing billing tab: ${value}`)
  }
  return button
}

async function setInput(input: HTMLInputElement | null, value: string) {
  if (!input) throw new Error('Missing input')
  input.value = value
  input.dispatchEvent(new Event('input', { bubbles: true }))
  await nextTick()
}

beforeEach(() => {
  localStorage.clear()
  modelsDevMocks.getModelsDevList.mockReset()
  modelsDevMocks.getModelsDevList.mockResolvedValue([stalePreset, freshPreset, unsupportedPreset])
  modelsDevMocks.refreshModelsDevList.mockReset()
  modelsDevMocks.refreshModelsDevList.mockResolvedValue([stalePreset, freshPreset, unsupportedPreset])
  globalModelMocks.createGlobalModel.mockReset()
  globalModelMocks.createGlobalModel.mockResolvedValue({ id: 'created-model' })
  globalModelMocks.listGlobalModels.mockReset()
  globalModelMocks.listGlobalModels.mockResolvedValue({ models: [], total: 0 })
  globalModelMocks.updateGlobalModel.mockReset()
  globalModelMocks.updateGlobalModel.mockResolvedValue({})
  Object.defineProperty(HTMLElement.prototype, 'scrollIntoView', {
    value: vi.fn(),
    configurable: true,
  })
})

afterEach(() => {
  for (const { app, root } of mountedApps.splice(0)) {
    app.unmount()
    root.remove()
  }
  document.body.innerHTML = ''
})

describe('GlobalModelFormDialog preset replacement', () => {
  it('shows a catalog error and retries instead of presenting a failed load as an empty catalog', async () => {
    modelsDevMocks.getModelsDevList
      .mockRejectedValueOnce({
        response: {
          status: 503,
          data: { detail: 'External models catalog unavailable' },
        },
      })
      .mockResolvedValueOnce([stalePreset, freshPreset])

    mountDialog()
    await settle()

    const errorState = document.body.querySelector('[data-testid="models-catalog-load-error"]')
    expect(errorState?.textContent).toContain('外部模型目录加载失败')
    expect(errorState?.textContent).toContain('External models catalog unavailable')
    expect(document.body.textContent).not.toContain('暂无可用模型')
    expect(findExactButton('手动填写').disabled).toBe(false)

    const retryButton = document.body.querySelector<HTMLButtonElement>(
      '[data-testid="models-catalog-retry"]',
    )
    if (!retryButton) throw new Error('Missing catalog retry button')
    retryButton.click()
    await settle()

    expect(modelsDevMocks.getModelsDevList).toHaveBeenCalledTimes(2)
    expect(document.body.querySelector('[data-testid="models-catalog-load-error"]')).toBeNull()
    expect(document.body.textContent).toContain('Stale Model')
    expect(document.body.textContent).toContain('Fresh Model')
  })

  it('keeps manual model creation available when the external catalog is unavailable', async () => {
    modelsDevMocks.getModelsDevList.mockRejectedValueOnce({
      response: {
        status: 503,
        data: { detail: 'External models catalog unavailable' },
      },
    })

    mountDialog()
    await settle()

    const manualEntryButton = document.body.querySelector<HTMLButtonElement>(
      '[data-testid="models-catalog-manual-entry"]',
    )
    if (!manualEntryButton) throw new Error('Missing catalog manual-entry button')
    manualEntryButton.click()
    await settle()

    expect(document.body.querySelector('[data-testid="models-catalog-load-error"]')).toBeNull()
    expect(document.body.textContent).toContain('手动填写模式')
    expect(document.body.querySelector('#model-name')).not.toBeNull()
  })

  it('allows manual model creation while the external catalog request is still pending', async () => {
    modelsDevMocks.getModelsDevList.mockReturnValueOnce(
      new Promise<ModelsDevModelItem[]>(() => {}),
    )

    mountDialog()
    await settle()

    const manualEntryButton = document.body.querySelector<HTMLButtonElement>(
      '[data-testid="models-catalog-manual-entry-loading"]',
    )
    if (!manualEntryButton) throw new Error('Missing loading-state manual-entry button')
    manualEntryButton.click()
    await settle()

    expect(document.body.textContent).toContain('手动填写模式')
    expect(document.body.querySelector('#model-name')).not.toBeNull()
  })

  it('drops the previous draft and submits only the newly selected model preset', async () => {
    mountDialog()
    await settle()

    expect(document.body.textContent).not.toContain('fresh-family')
    findButton('Stale Model').click()
    await settle()

    expect(document.body.querySelector('[data-processing-tier]')).toBeNull()
    expect(document.body.textContent).not.toContain('处理层级')
    expect(document.body.textContent).toContain('自定义价格')

    await setInput(
      document.body.querySelector<HTMLInputElement>('input[placeholder="如 0.01"]'),
      '0.25',
    )
    await setInput(
      document.body.querySelector<HTMLInputElement>('#model-description'),
      'must not leak into the next preset',
    )
    await setInput(
      document.body.querySelector<HTMLInputElement>('[data-testid="tier-input-price"]'),
      '99',
    )
    findExactButton('视频').click()
    await nextTick()
    findExactButton('Sora').click()
    await nextTick()

    findButton('返回选择模型').click()
    await settle()
    findButton('Fresh Model').click()
    await settle()

    expect(document.body.querySelector<HTMLInputElement>('#model-name')?.value).toBe('fresh-model')
    expect(document.body.querySelector<HTMLInputElement>('#model-display-name')?.value).toBe('Fresh Model')
    expect(document.body.querySelector<HTMLInputElement>('#model-description')?.value).toBe('')
    expect(document.body.querySelector<HTMLInputElement>('input[placeholder="如 0.01"]')?.value).toBe('')
    expect(
      [...document.body.querySelectorAll<HTMLInputElement>('[data-testid="tier-input-price"]')]
        .map(input => input.value),
    ).toEqual(['3', '5'])

    findExactButton('添加').click()
    await settle()

    expect(globalModelMocks.createGlobalModel).toHaveBeenCalledOnce()
    const payload = globalModelMocks.createGlobalModel.mock.calls[0][0]
    expect(payload).toMatchObject({
      name: 'fresh-model',
      display_name: 'Fresh Model',
      default_price_per_request: undefined,
      config: {
        streaming: true,
        context_limit: 128_000,
        output_limit: 4_096,
        family: 'fresh-family',
        input_modalities: ['text'],
        output_modalities: ['text'],
        models_dev_pricing_source: {
          provider_id: 'openai',
          provider_name: 'OpenAI',
        },
      },
      default_tiered_pricing: {
        tiers: [
          {
            up_to: 99_999,
            input_price_per_1m: 3,
            output_price_per_1m: 4,
          },
          {
            up_to: null,
            input_price_per_1m: 5,
            output_price_per_1m: 6,
          },
        ],
      },
    })
    expect(JSON.parse(
      localStorage.getItem('aether:models-dev-pricing-sources:v1') || 'null',
    )).toMatchObject({
      models: {
        'created-model': {
          provider_id: 'openai',
          provider_name: 'OpenAI',
        },
      },
    })
    expect(payload.config).not.toHaveProperty('description')
    expect(payload.config).not.toHaveProperty('billing')
    expect(payload.default_tiered_pricing).not.toHaveProperty('processing_tiers')
    expect(payload.default_tiered_pricing.tiers).toEqual([
      {
        up_to: 99_999,
        input_price_per_1m: 3,
        output_price_per_1m: 4,
      },
      {
        up_to: null,
        input_price_per_1m: 5,
        output_price_per_1m: 6,
      },
    ])
  })

  it('submits a compact processing-tier multiplier without a Standard overlay', async () => {
    mountDialog()
    await settle()
    findButton('Fresh Model').click()
    await settle()

    const priorityToggle = document.body.querySelector(
      'input[aria-label="启用 Fast · OpenAI · Chat / Responses 层级倍率"]',
    ) as HTMLInputElement
    priorityToggle.click()
    await nextTick()
    await setInput(
      document.body.querySelector<HTMLInputElement>(
        '[data-testid="processing-tier-multiplier-priority"]',
      ),
      '2.5',
    )

    findExactButton('添加').click()
    await settle()

    const payload = globalModelMocks.createGlobalModel.mock.calls[0][0]
    expect(payload.default_tiered_pricing.processing_tiers).toEqual({
      priority: { price_multiplier: 2.5 },
    })
    expect(payload.default_tiered_pricing.processing_tiers).not.toHaveProperty('standard')
  })

  it('routes an already-added online model from its card action without syncing prices', async () => {
    const existingStaleModel = buildExistingStaleModel()
    globalModelMocks.listGlobalModels.mockResolvedValue({
      models: [existingStaleModel],
      total: 1,
    })
    const { editModel } = mountDialog()
    await settle()

    const editButton = findExistingEditButton(stalePreset.modelId)
    const modelCard = findButton('Stale Model')
    expect(modelCard.disabled).toBe(true)
    expect(editButton.parentElement?.textContent).toContain('已添加')
    expect(editButton.title).toBe('去编辑')
    expect(editButton.getAttribute('aria-label')).toBe('编辑 Stale Model')
    expect(document.body.textContent).not.toContain('价格可更新')
    expect(document.body.textContent).not.toContain('上次来源')
    expect(document.body.textContent).not.toContain('同步在线价格')
    expect(document.body.textContent).not.toContain('保留当前价格')
    expect(document.body.textContent).not.toContain('使用在线价格')
    expect(document.body.querySelector('[data-testid="tier-input-price"]')).toBeNull()

    modelCard.click()
    await settle()
    expect(editModel).not.toHaveBeenCalled()
    expect(document.body.textContent).toContain('创建统一模型')

    editButton.click()
    await settle()

    expect(editModel).toHaveBeenCalledOnce()
    expect(editModel).toHaveBeenCalledWith(existingStaleModel)
    expect(document.body.textContent).toContain('编辑模型')
    expect(findExactButton('保存').disabled).toBe(false)
    expect(globalModelMocks.createGlobalModel).not.toHaveBeenCalled()
    expect(globalModelMocks.updateGlobalModel).not.toHaveBeenCalled()
  })

  it('routes an existing model to edit even when its online pricing is unsupported', async () => {
    const existingModel = {
      ...buildExistingStaleModel(),
      id: 'reasoning-priced-global-model',
      name: unsupportedPreset.modelId,
      display_name: unsupportedPreset.modelName,
    }
    globalModelMocks.listGlobalModels.mockResolvedValue({
      models: [existingModel],
      total: 1,
    })
    const { editModel } = mountDialog()
    await settle()

    findExistingEditButton(unsupportedPreset.modelId).click()
    await settle()

    expect(document.body.textContent).not.toContain('计价不兼容')
    expect(document.body.textContent).not.toContain('同步在线价格')
    expect(editModel).toHaveBeenCalledWith(existingModel)
    expect(document.body.textContent).toContain('编辑模型')
    expect(globalModelMocks.createGlobalModel).not.toHaveBeenCalled()
    expect(globalModelMocks.updateGlobalModel).not.toHaveBeenCalled()
  })

  it('opens model editing on Token while preserving alternate billing data', async () => {
    const existingModel = {
      ...buildExistingStaleModel(),
      default_price_per_request: 0.25,
      supported_capabilities: ['image_generation'],
      config: {
        streaming: true,
        billing: {
          video: {
            price_per_second_by_resolution: { '720p': 0.1 },
          },
        },
      },
    }
    globalModelMocks.listGlobalModels.mockResolvedValue({
      models: [existingModel],
      total: 1,
    })
    mountDialog()
    await settle()

    findExistingEditButton(stalePreset.modelId).click()
    await settle()

    expect(findBillingTab('token').dataset.state).toBe('active')
    expect(findBillingTab('request').dataset.state).toBe('inactive')
    expect(findBillingTab('image').dataset.state).toBe('inactive')
    expect(findBillingTab('video').dataset.state).toBe('inactive')

    findBillingTab('request').click()
    await nextTick()
    expect(document.body.querySelector<HTMLInputElement>('input[placeholder="如 0.01"]')?.value)
      .toBe('0.25')
  })

  it('refreshes and applies the latest online price from the edit dialog', async () => {
    const existingStaleModel = buildExistingStaleModel()
    const nextConfig = {
      ...existingStaleModel.config,
      models_dev_pricing_source: {
        provider_id: stalePreset.providerId,
        provider_name: stalePreset.providerName,
      },
    }
    const syncedModel = {
      ...existingStaleModel,
      default_tiered_pricing: stalePreset.tieredPricing!,
      config: nextConfig,
    }
    globalModelMocks.updateGlobalModel.mockResolvedValue(syncedModel)
    globalModelMocks.listGlobalModels.mockResolvedValue({
      models: [existingStaleModel],
      total: 1,
    })
    const { editModel, pricingSynced } = mountDialog()
    await settle()

    findExistingEditButton(stalePreset.modelId).click()
    await settle()
    const syncButton = document.body.querySelector<HTMLButtonElement>(
      '[data-testid="sync-online-pricing"]',
    )
    if (!syncButton) throw new Error('Missing online pricing sync button')
    expect(syncButton.title).toBe('选择并同步在线价格来源')
    expect(syncButton.getAttribute('aria-label')).toBe('同步最新在线价格')

    syncButton.click()
    await settle()

    expect(modelsDevMocks.refreshModelsDevList).toHaveBeenCalledOnce()
    expect(modelsDevMocks.refreshModelsDevList).toHaveBeenCalledWith(false)
    expect(globalModelMocks.updateGlobalModel).toHaveBeenCalledWith(
      existingStaleModel.id,
      {
        default_tiered_pricing: stalePreset.tieredPricing,
        config: nextConfig,
      },
    )
    expect(pricingSynced).toHaveBeenCalledWith(syncedModel)
    expect(document.body.querySelector<HTMLInputElement>('[data-testid="tier-input-price"]')?.value)
      .toBe('1')
    expect(document.body.textContent).toContain('编辑模型')
    expect(editModel).toHaveBeenCalledWith(existingStaleModel)
    expect(JSON.parse(
      localStorage.getItem('aether:models-dev-pricing-sources:v1') || 'null',
    )).toMatchObject({
      models: {
        [existingStaleModel.id]: {
          provider_id: stalePreset.providerId,
          provider_name: stalePreset.providerName,
        },
      },
    })
  })

  it('persists a newly selected source even when prices already match and keeps it on save', async () => {
    const existingModel = {
      ...buildExistingStaleModel(),
      default_tiered_pricing: stalePreset.tieredPricing!,
    }
    const nextConfig = {
      ...existingModel.config,
      models_dev_pricing_source: {
        provider_id: stalePreset.providerId,
        provider_name: stalePreset.providerName,
      },
    }
    const syncedModel = { ...existingModel, config: nextConfig }
    globalModelMocks.updateGlobalModel.mockResolvedValue(syncedModel)
    const { editingModel, pricingSynced } = mountDialog()
    await settle()

    editingModel.value = existingModel
    await settle()
    const syncButton = document.body.querySelector<HTMLButtonElement>(
      '[data-testid="sync-online-pricing"]',
    )
    if (!syncButton) throw new Error('Missing online pricing sync button')
    syncButton.click()
    await settle()

    expect(globalModelMocks.updateGlobalModel).toHaveBeenNthCalledWith(
      1,
      existingModel.id,
      {
        default_tiered_pricing: stalePreset.tieredPricing,
        config: nextConfig,
      },
    )
    expect(pricingSynced).toHaveBeenCalledWith(syncedModel)

    findExactButton('保存').click()
    await settle()

    expect(globalModelMocks.updateGlobalModel).toHaveBeenCalledTimes(2)
    expect(globalModelMocks.updateGlobalModel.mock.calls[1][1].config).toEqual(nextConfig)
  })

  it('offers a provider choice when the remembered source is unavailable', async () => {
    const existingStaleModel = buildExistingStaleModel()
    const nextConfig = {
      ...existingStaleModel.config,
      models_dev_pricing_source: {
        provider_id: alternateStalePreset.providerId,
        provider_name: alternateStalePreset.providerName,
      },
    }
    const syncedModel = {
      ...existingStaleModel,
      default_tiered_pricing: alternateStalePreset.tieredPricing!,
      config: nextConfig,
    }
    modelsDevMocks.refreshModelsDevList.mockResolvedValue([
      unavailableStalePreset,
      alternateStalePreset,
    ])
    localStorage.setItem('aether:models-dev-pricing-sources:v1', JSON.stringify({
      version: 1,
      models: {
        [existingStaleModel.id]: {
          provider_id: unavailableStalePreset.providerId,
          provider_name: unavailableStalePreset.providerName,
        },
      },
    }))
    globalModelMocks.updateGlobalModel.mockResolvedValue(syncedModel)
    const { editingModel, pricingSynced } = mountDialog()
    await settle()

    // Open the editor directly so there is no transient provider preference.
    editingModel.value = existingStaleModel
    await settle()
    const syncButton = document.body.querySelector<HTMLButtonElement>(
      '[data-testid="sync-online-pricing"]',
    )
    if (!syncButton) throw new Error('Missing online pricing sync button')

    syncButton.click()
    await settle()

    expect(globalModelMocks.updateGlobalModel).not.toHaveBeenCalled()
    expect(document.body.textContent).toContain('选择在线价格来源')
    expect(document.body.textContent).toContain('OpenAI')
    expect(document.body.textContent).toContain('Azure OpenAI')

    const alternateSource = document.body.querySelector<HTMLButtonElement>(
      '[data-testid="online-pricing-source-azure-openai"]',
    )
    if (!alternateSource) throw new Error('Missing alternate pricing source')
    expect(document.body.querySelector<HTMLButtonElement>(
      '[data-testid="online-pricing-source-openai"]',
    )?.disabled).toBe(true)
    expect(document.activeElement).toBe(alternateSource)
    alternateSource.click()
    await nextTick()
    findExactButton('同步此来源').click()
    await settle()

    expect(globalModelMocks.updateGlobalModel).toHaveBeenCalledWith(
      existingStaleModel.id,
      {
        default_tiered_pricing: alternateStalePreset.tieredPricing,
        config: nextConfig,
      },
    )
    expect(pricingSynced).toHaveBeenCalledWith(syncedModel)
    expect(document.body.textContent).not.toContain('选择在线价格来源')
    expect(JSON.parse(
      localStorage.getItem('aether:models-dev-pricing-sources:v1') || 'null',
    )).toMatchObject({
      models: {
        [existingStaleModel.id]: {
          provider_id: alternateStalePreset.providerId,
          provider_name: alternateStalePreset.providerName,
        },
      },
    })
  })

  it('opens the compact source popover for multiple usable providers without a remembered source', async () => {
    const existingStaleModel = buildExistingStaleModel()
    modelsDevMocks.refreshModelsDevList.mockResolvedValue([
      stalePreset,
      alternateStalePreset,
    ])
    globalModelMocks.listGlobalModels.mockResolvedValue({
      models: [existingStaleModel],
      total: 1,
    })
    const { editingModel } = mountDialog()
    await settle()

    editingModel.value = existingStaleModel
    await settle()
    const syncButton = document.body.querySelector<HTMLButtonElement>(
      '[data-testid="sync-online-pricing"]',
    )
    if (!syncButton) throw new Error('Missing online pricing sync button')
    syncButton.click()
    await settle()

    expect(document.body.querySelector('[data-testid="online-pricing-source-openai"]')).not.toBeNull()
    expect(document.body.querySelector('[data-testid="online-pricing-source-azure-openai"]')).not.toBeNull()
    expect(document.body.querySelectorAll('.fixed.inset-0.overflow-hidden.pointer-events-none')).toHaveLength(1)
    expect(globalModelMocks.updateGlobalModel).not.toHaveBeenCalled()

    const cancelButton = document.body.querySelector<HTMLButtonElement>(
      '[data-testid="online-pricing-source-cancel"]',
    )
    if (!cancelButton) throw new Error('Missing source popover cancel button')
    cancelButton.click()
    await settle()

    expect(document.body.querySelector('[data-testid="online-pricing-source-openai"]')).toBeNull()
    expect(globalModelMocks.updateGlobalModel).not.toHaveBeenCalled()
  })
})
