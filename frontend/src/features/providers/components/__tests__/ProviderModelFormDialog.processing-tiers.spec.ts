import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h, nextTick, ref, type App } from 'vue'

import type { Model } from '@/api/endpoints'
import ProviderModelFormDialog from '../ProviderModelFormDialog.vue'

const modelMocks = vi.hoisted(() => ({
  createModel: vi.fn(),
  updateModel: vi.fn(),
  getProviderModels: vi.fn(),
}))

const globalModelMocks = vi.hoisted(() => ({
  createGlobalModel: vi.fn(),
  getGlobalModel: vi.fn(),
  listGlobalModels: vi.fn(),
}))

vi.mock('@/api/endpoints/models', () => modelMocks)
vi.mock('@/api/global-models', () => globalModelMocks)
vi.mock('@/composables/useToast', () => ({
  useToast: () => ({
    error: vi.fn(),
    success: vi.fn(),
  }),
}))

const mountedApps: Array<{ app: App, root: HTMLElement }> = []

const editingModel = {
  id: 'provider-model-1',
  provider_id: 'provider-1',
  global_model_id: 'global-model-1',
  provider_model_name: 'gpt-test',
  tiered_pricing: null,
  price_per_request: null,
  effective_price_per_request: 0.25,
  config: null,
  effective_config: {
    billing: {
      video: {
        price_per_second_by_resolution: { '720p': 0.1 },
      },
    },
  },
  effective_tiered_pricing: {
    tiers: [{ up_to: null, input_price_per_1m: 5, output_price_per_1m: 30 }],
    processing_tiers: {
      priority: { price_multiplier: 2.5 },
      fast: { price_multiplier: 2 },
      hyperlane: {
        tiers: [{ up_to: null, input_price_per_1m: 8, output_price_per_1m: 48 }],
      },
    },
  },
  is_active: true,
  is_available: true,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
} as Model

function mountDialog(model: Model | null = editingModel) {
  const root = document.createElement('div')
  document.body.appendChild(root)
  const open = ref(false)
  const app = createApp(defineComponent({
    setup() {
      return () => h(ProviderModelFormDialog, {
        open: open.value,
        providerId: 'provider-1',
        editingModel: model,
      })
    },
  }))
  app.mount(root)
  mountedApps.push({ app, root })
  open.value = true
}

function findButton(text: string): HTMLButtonElement {
  const button = [...document.body.querySelectorAll('button')]
    .find(candidate => candidate.textContent?.trim() === text)
  if (!(button instanceof HTMLButtonElement)) throw new Error(`Missing button: ${text}`)
  return button
}

async function settle() {
  for (let index = 0; index < 5; index += 1) {
    await Promise.resolve()
    await nextTick()
  }
}

beforeEach(() => {
  modelMocks.createModel.mockReset()
  modelMocks.updateModel.mockReset()
  modelMocks.updateModel.mockResolvedValue(editingModel)
  modelMocks.getProviderModels.mockReset()
  globalModelMocks.createGlobalModel.mockReset()
  globalModelMocks.getGlobalModel.mockReset()
  globalModelMocks.listGlobalModels.mockReset()
})

afterEach(() => {
  for (const { app, root } of mountedApps.splice(0)) {
    app.unmount()
    root.remove()
  }
  document.body.innerHTML = ''
})

describe('ProviderModelFormDialog processing-tier pricing', () => {
  it('uses the same compact Fast grouping for inherited global-model pricing', async () => {
    mountDialog()
    await settle()

    expect(document.body.textContent).toContain('选择计费模式')
    for (const tab of ['Token', '按次', '图片', '视频']) {
      expect(findButton(tab)).toBeDefined()
    }
    findButton('Token').click()
    await nextTick()

    expect(document.body.querySelector('[data-processing-tier="standard"]')).not.toBeNull()
    expect(document.body.querySelector('[data-processing-tier="hyperlane"]')).not.toBeNull()
    const fastGroup = document.body.querySelector('[data-processing-tier-group="fast"]')
    expect(fastGroup?.textContent).toContain('Fast')
    expect(fastGroup?.textContent).toContain('OpenAI')
    expect(fastGroup?.textContent).toContain('Chat / Responses')
    expect(fastGroup?.textContent).toContain('Claude')
    expect(fastGroup?.textContent).toContain('Messages')
    expect(document.body.querySelector<HTMLInputElement>(
      '[data-testid="processing-tier-multiplier-priority"]',
    )?.value).toBe('2.5')
    expect(document.body.querySelector<HTMLInputElement>(
      '[data-testid="processing-tier-multiplier-fast"]',
    )?.value).toBe('2')

    findButton('保存').click()
    await settle()

    const payload = modelMocks.updateModel.mock.calls[0][2]
    expect(payload).not.toHaveProperty('tiered_pricing')
    expect(payload).not.toHaveProperty('price_per_request')
    expect(payload).not.toHaveProperty('config')
  })

  it('creates a Provider price override only after the inherited price is edited', async () => {
    mountDialog()
    await settle()
    findButton('Token').click()
    await nextTick()
    const multiplier = document.body.querySelector<HTMLInputElement>(
      '[data-testid="processing-tier-multiplier-priority"]',
    )
    if (!multiplier) throw new Error('Missing OpenAI Fast multiplier')

    multiplier.value = '3'
    multiplier.dispatchEvent(new Event('input', { bubbles: true }))
    await nextTick()
    findButton('保存').click()
    await settle()

    const payload = modelMocks.updateModel.mock.calls[0][2]
    expect(payload.tiered_pricing.processing_tiers.priority).toEqual({
      price_multiplier: 3,
    })
    expect(payload.tiered_pricing).not.toHaveProperty('tiers')
    expect(payload.tiered_pricing.processing_tiers).not.toHaveProperty('fast')
    expect(payload.tiered_pricing.processing_tiers).not.toHaveProperty('hyperlane')
    expect(payload).not.toHaveProperty('price_per_request')
    expect(payload).not.toHaveProperty('config')
  })

  it('reopens a processing-only override with inherited Standard and keeps the next save partial', async () => {
    const partialOverride = {
      processing_tiers: {
        priority: { price_multiplier: 3 },
      },
    }
    const reopenedModel = {
      ...editingModel,
      tiered_pricing: partialOverride,
      // The current backend returns raw-or-global here, so a partial raw value has no tiers.
      effective_tiered_pricing: partialOverride,
    } as Model
    globalModelMocks.getGlobalModel.mockResolvedValue({
      id: 'global-model-1',
      name: 'gpt-test',
      display_name: 'GPT Test',
      is_active: true,
      default_tiered_pricing: editingModel.effective_tiered_pricing,
      created_at: '2026-01-01T00:00:00Z',
      total_models: 1,
      total_providers: 1,
      price_range: {},
    })

    mountDialog(reopenedModel)
    await settle()
    findButton('Token').click()
    await nextTick()

    expect(globalModelMocks.getGlobalModel).toHaveBeenCalledWith('global-model-1')
    expect(document.body.querySelector<HTMLInputElement>(
      'input[aria-label="Standard 阶梯 1 输入价格（美元/百万 Token）"]',
    )?.value).toBe('5')
    expect(document.body.querySelector<HTMLInputElement>(
      '[data-testid="processing-tier-multiplier-priority"]',
    )?.value).toBe('3')
    expect(document.body.querySelector<HTMLInputElement>(
      '[data-testid="processing-tier-multiplier-fast"]',
    )?.value).toBe('2')
    expect(document.body.querySelector('[data-processing-tier="hyperlane"]')).not.toBeNull()

    const multiplier = document.body.querySelector<HTMLInputElement>(
      '[data-testid="processing-tier-multiplier-priority"]',
    )
    if (!multiplier) throw new Error('Missing OpenAI Fast multiplier')
    multiplier.value = '4'
    multiplier.dispatchEvent(new Event('input', { bubbles: true }))
    await nextTick()
    findButton('保存').click()
    await settle()

    expect(modelMocks.updateModel.mock.calls[0][2].tiered_pricing).toEqual({
      processing_tiers: {
        priority: { price_multiplier: 4 },
      },
    })
  })

  it('reopens and edits an explicit unknown Provider processing tier', async () => {
    const providerHyperlane = {
      tiers: [{ up_to: null, input_price_per_1m: 9, output_price_per_1m: 54 }],
      future_overlay_option: 'keep-provider-hyperlane',
    }
    const partialOverride = {
      processing_tiers: {
        hyperlane: providerHyperlane,
      },
    }
    const reopenedModel = {
      ...editingModel,
      tiered_pricing: partialOverride,
      effective_tiered_pricing: partialOverride,
    } as Model
    globalModelMocks.getGlobalModel.mockResolvedValue({
      id: 'global-model-1',
      name: 'gpt-test',
      display_name: 'GPT Test',
      is_active: true,
      default_tiered_pricing: editingModel.effective_tiered_pricing,
      created_at: '2026-01-01T00:00:00Z',
      total_models: 1,
      total_providers: 1,
      price_range: {},
    })

    mountDialog(reopenedModel)
    await settle()
    findButton('Token').click()
    await nextTick()
    const hyperlane = document.body.querySelector<HTMLButtonElement>(
      '[data-processing-tier="hyperlane"]',
    )
    if (!hyperlane) throw new Error('Missing hyperlane pricing entry')
    hyperlane.click()
    await nextTick()

    const input = document.body.querySelector<HTMLInputElement>(
      'input[aria-label="hyperlane 阶梯 1 输入价格（美元/百万 Token）"]',
    )
    if (!input) throw new Error('Missing hyperlane input-price editor')
    expect(input.value).toBe('9')
    input.value = '10'
    input.dispatchEvent(new Event('input', { bubbles: true }))
    await nextTick()
    findButton('保存').click()
    await settle()

    expect(modelMocks.updateModel.mock.calls[0][2].tiered_pricing).toEqual({
      processing_tiers: {
        hyperlane: {
          ...providerHyperlane,
          tiers: [{ up_to: null, input_price_per_1m: 10, output_price_per_1m: 54 }],
        },
      },
    })
  })

  it('edits the per-request override through the same billing-mode tabs', async () => {
    mountDialog()
    await settle()
    findButton('按次').click()
    await nextTick()
    const input = document.body.querySelector<HTMLInputElement>(
      'input[placeholder="留空使用全局模型默认值"]',
    )
    if (!input) throw new Error('Missing per-request price input')

    input.value = '0.5'
    input.dispatchEvent(new Event('input', { bubbles: true }))
    await nextTick()
    findButton('保存').click()
    await settle()

    const payload = modelMocks.updateModel.mock.calls[0][2]
    expect(payload.price_per_request).toBe(0.5)
    expect(payload).not.toHaveProperty('tiered_pricing')
    expect(payload).not.toHaveProperty('config')
  })

  it('enables the Provider image capability when the Image tab is explicitly selected', async () => {
    mountDialog()
    await settle()

    findButton('图片').click()
    await nextTick()
    findButton('保存').click()
    await settle()

    const payload = modelMocks.updateModel.mock.calls[0][2]
    expect(payload.supports_image_generation).toBe(true)
    expect(payload).not.toHaveProperty('tiered_pricing')
    expect(payload).not.toHaveProperty('price_per_request')
    expect(payload).not.toHaveProperty('config')
  })
})
