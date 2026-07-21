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
import GlobalModelFormDialog from '../GlobalModelFormDialog.vue'

const modelsDevMocks = vi.hoisted(() => ({
  getModelsDevList: vi.fn(),
}))

const globalModelMocks = vi.hoisted(() => ({
  createGlobalModel: vi.fn(),
  updateGlobalModel: vi.fn(),
}))

vi.mock('@/api/models-dev', () => ({
  getModelsDevList: modelsDevMocks.getModelsDevList,
  getProviderLogoUrl: (providerId: string) => `/logos/${providerId}.svg`,
}))

vi.mock('@/api/global-models', () => ({
  createGlobalModel: globalModelMocks.createGlobalModel,
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

function mountDialog() {
  const root = document.createElement('div')
  document.body.appendChild(root)
  const open = ref(false)
  const app = createApp(defineComponent({
    setup() {
      return () => h(GlobalModelFormDialog, {
        open: open.value,
        model: null,
      })
    },
  }))
  app.mount(root)
  mountedApps.push({ app, root })
  open.value = true
  return { root, open }
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

async function setInput(input: HTMLInputElement | null, value: string) {
  if (!input) throw new Error('Missing input')
  input.value = value
  input.dispatchEvent(new Event('input', { bubbles: true }))
  await nextTick()
}

beforeEach(() => {
  modelsDevMocks.getModelsDevList.mockReset()
  modelsDevMocks.getModelsDevList.mockResolvedValue([stalePreset, freshPreset])
  globalModelMocks.createGlobalModel.mockReset()
  globalModelMocks.createGlobalModel.mockResolvedValue({})
  globalModelMocks.updateGlobalModel.mockReset()
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
  it('drops the previous draft and submits only the newly selected model preset', async () => {
    mountDialog()
    await settle()

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
})
