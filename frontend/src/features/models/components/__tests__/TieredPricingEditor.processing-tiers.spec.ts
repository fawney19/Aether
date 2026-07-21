import { afterEach, describe, expect, it, vi } from 'vitest'
import {
  createApp,
  defineComponent,
  h,
  nextTick,
  shallowRef,
  type App,
  type ComponentPublicInstance,
} from 'vue'

import type { TieredPricingConfig } from '@/api/endpoints/types'
import TieredPricingEditor from '../TieredPricingEditor.vue'

interface TieredPricingEditorExposed extends ComponentPublicInstance {
  getFinalPricing: () => TieredPricingConfig
  getValidationError: () => string | null
}

const mountedApps: Array<{ app: App, root: HTMLElement }> = []

function mountEditor(
  modelValue: TieredPricingConfig,
  options: {
    autoFillMissingCachePrices?: boolean
    showCache1h?: boolean
    showImagePricing?: boolean
    showTokenPricing?: boolean
    showImageEditor?: boolean
    showProcessingTierControls?: boolean
    showProcessingTierMultiplierControls?: boolean
  } = {},
) {
  const root = document.createElement('div')
  document.body.appendChild(root)
  const onUpdate = vi.fn()
  const currentModelValue = shallowRef(modelValue)
  const showProcessingTierControls = shallowRef(options.showProcessingTierControls)
  let editor: TieredPricingEditorExposed | null = null

  const app = createApp(defineComponent({
    setup() {
      return () => h(TieredPricingEditor, {
        ref: (instance: unknown) => {
          editor = instance as TieredPricingEditorExposed | null
        },
        modelValue: currentModelValue.value,
        autoFillMissingCachePrices: options.autoFillMissingCachePrices,
        showCache1h: options.showCache1h,
        showImagePricing: options.showImagePricing,
        showTokenPricing: options.showTokenPricing,
        showImageEditor: options.showImageEditor,
        showProcessingTierControls: showProcessingTierControls.value,
        showProcessingTierMultiplierControls: options.showProcessingTierMultiplierControls,
        'onUpdate:modelValue': onUpdate,
      })
    },
  }))

  app.mount(root)
  mountedApps.push({ app, root })

  return {
    root,
    onUpdate,
    setModelValue: (value: TieredPricingConfig) => {
      currentModelValue.value = value
    },
    setShowProcessingTierControls: (value: boolean) => {
      showProcessingTierControls.value = value
    },
    getFinalPricing: () => {
      if (!editor) throw new Error('TieredPricingEditor ref was not mounted')
      return editor.getFinalPricing()
    },
    getValidationError: () => {
      if (!editor) throw new Error('TieredPricingEditor ref was not mounted')
      return editor.getValidationError()
    },
  }
}

function click(element: Element | null) {
  if (!(element instanceof HTMLButtonElement)) {
    throw new Error('Expected a button')
  }
  element.click()
}

afterEach(() => {
  for (const { app, root } of mountedApps.splice(0)) {
    app.unmount()
    root.remove()
  }
})

describe('TieredPricingEditor processing tiers', () => {
  it('hides processing-tier controls while editing Standard and preserving overlays', async () => {
    const pricing = {
      tiers: [{ up_to: null, input_price_per_1m: 5, output_price_per_1m: 30 }],
      processing_tiers: {
        priority: {
          price_multiplier: 999,
          tiers: [{ up_to: null, input_price_per_1m: 10, output_price_per_1m: 60 }],
          future_overlay_option: 'keep-hidden-overlay',
        },
      },
    } as TieredPricingConfig
    const {
      root,
      onUpdate,
      getFinalPricing,
      setShowProcessingTierControls,
    } = mountEditor(pricing)

    click(root.querySelector('[data-processing-tier="priority"]'))
    await nextTick()
    expect(root.querySelector<HTMLInputElement>('[data-testid="tier-input-price"]')?.value)
      .toBe('10')

    setShowProcessingTierControls(false)
    await nextTick()

    expect(root.querySelector('[data-processing-tier]')).toBeNull()
    expect(root.textContent).not.toContain('处理层级')

    const input = root.querySelector('[data-testid="tier-input-price"]') as HTMLInputElement | null
    if (!input) throw new Error('Expected the Standard input-price control')
    input.value = '7.5'
    input.dispatchEvent(new Event('input', { bubbles: true }))
    await nextTick()

    const emitted = onUpdate.mock.lastCall?.[0] as TieredPricingConfig
    expect(emitted.tiers[0].input_price_per_1m).toBe(7.5)
    expect(emitted.processing_tiers).toEqual(pricing.processing_tiers)
    expect(getFinalPricing().processing_tiers).toEqual(pricing.processing_tiers)
  })

  it('edits compact processing-tier multipliers without writing a Standard overlay', async () => {
    const pricing = {
      tiers: [{ up_to: null, input_price_per_1m: 5, output_price_per_1m: 30 }],
    } as TieredPricingConfig
    const { root, getFinalPricing, getValidationError } = mountEditor(pricing, {
      showProcessingTierControls: false,
      showProcessingTierMultiplierControls: true,
    })

    expect(root.querySelector('[data-testid="processing-tier-group-fast"]')?.textContent)
      .toBe('Fast')
    expect(root.querySelector('[data-processing-tier-group="fast"]')?.textContent)
      .toContain('OpenAI')
    expect(root.querySelector('[data-processing-tier-group="fast"]')?.textContent)
      .toContain('Chat / Responses')
    expect(root.querySelector('[data-processing-tier-group="fast"]')?.textContent)
      .toContain('Claude')
    expect(root.querySelector('[data-processing-tier-group="fast"]')?.textContent)
      .toContain('Messages')

    const priorityToggle = root.querySelector(
      'input[aria-label="启用 Fast · OpenAI · Chat / Responses 层级倍率"]',
    ) as HTMLInputElement
    priorityToggle.click()
    await nextTick()

    expect(getValidationError()).toContain('请输入层级倍率')
    expect(() => getFinalPricing()).toThrow('请输入层级倍率')
    const multiplier = root.querySelector(
      '[data-testid="processing-tier-multiplier-priority"]',
    ) as HTMLInputElement
    multiplier.value = '2.5'
    multiplier.dispatchEvent(new Event('input', { bubbles: true }))
    await nextTick()

    expect(getValidationError()).toBeNull()
    expect(getFinalPricing().processing_tiers).toEqual({
      priority: { price_multiplier: 2.5 },
    })
    expect(getFinalPricing().processing_tiers).not.toHaveProperty('standard')

    priorityToggle.click()
    await nextTick()
    expect(getFinalPricing()).not.toHaveProperty('processing_tiers')
  })

  it('keeps the grouped Claude Fast option mapped to the internal fast key', async () => {
    const pricing = {
      tiers: [{ up_to: null, input_price_per_1m: 5, output_price_per_1m: 30 }],
    } as TieredPricingConfig
    const { root, getFinalPricing } = mountEditor(pricing, {
      showProcessingTierControls: false,
      showProcessingTierMultiplierControls: true,
    })
    const fastToggle = root.querySelector(
      'input[aria-label="启用 Fast · Claude · Messages 层级倍率"]',
    ) as HTMLInputElement

    fastToggle.click()
    await nextTick()
    const multiplier = root.querySelector(
      '[data-testid="processing-tier-multiplier-fast"]',
    ) as HTMLInputElement
    multiplier.value = '2'
    multiplier.dispatchEvent(new Event('input', { bubbles: true }))
    await nextTick()

    expect(getFinalPricing().processing_tiers).toEqual({
      fast: { price_multiplier: 2 },
    })
  })

  it('requires an enabled processing-tier multiplier instead of clearing the saved value', async () => {
    const pricing = {
      tiers: [{ up_to: null, input_price_per_1m: 5, output_price_per_1m: 30 }],
      processing_tiers: { priority: { price_multiplier: 2.5 } },
    } as TieredPricingConfig
    const { root, onUpdate, getFinalPricing, getValidationError } = mountEditor(pricing, {
      showProcessingTierControls: false,
      showProcessingTierMultiplierControls: true,
    })
    const multiplier = root.querySelector(
      '[data-testid="processing-tier-multiplier-priority"]',
    ) as HTMLInputElement

    multiplier.value = ''
    multiplier.dispatchEvent(new Event('input', { bubbles: true }))
    await nextTick()

    expect(getValidationError()).toContain('请输入层级倍率')
    expect(() => getFinalPricing()).toThrow('请输入层级倍率')
    expect(onUpdate).not.toHaveBeenCalled()
  })

  it('preserves a custom catalog until the user explicitly replaces it with a multiplier', async () => {
    const pricing = {
      tiers: [{ up_to: null, input_price_per_1m: 5, output_price_per_1m: 30 }],
      processing_tiers: {
        priority: {
          tiers: [{ up_to: null, input_price_per_1m: 10, output_price_per_1m: 60 }],
          future_overlay_option: 'replace-with-catalog',
        },
        hyperlane: {
          tiers: [{ up_to: null, input_price_per_1m: 7, output_price_per_1m: 42 }],
          future_overlay_option: 'keep-unknown',
        },
      },
    } as TieredPricingConfig
    const { root, onUpdate, getFinalPricing, getValidationError } = mountEditor(pricing, {
      showProcessingTierControls: false,
      showProcessingTierMultiplierControls: true,
    })

    expect(root.textContent).toContain('自定义价格')
    expect(getFinalPricing().processing_tiers).toEqual(pricing.processing_tiers)

    click(root.querySelector('[data-testid="processing-tier-convert-priority"]'))
    await nextTick()
    expect(getValidationError()).toContain('请输入层级倍率')
    expect(onUpdate).not.toHaveBeenCalled()
    expect(() => getFinalPricing()).toThrow('请输入层级倍率')
    const multiplier = root.querySelector(
      '[data-testid="processing-tier-multiplier-priority"]',
    ) as HTMLInputElement
    multiplier.value = '2'
    multiplier.dispatchEvent(new Event('input', { bubbles: true }))
    await nextTick()

    expect(getFinalPricing().processing_tiers).toEqual({
      priority: { price_multiplier: 2 },
      hyperlane: pricing.processing_tiers?.hyperlane,
    })
  })

  it('validates compact multipliers as finite non-negative numbers', async () => {
    const pricing = {
      tiers: [{ up_to: null, input_price_per_1m: 5, output_price_per_1m: 30 }],
      processing_tiers: { flex: { price_multiplier: 0.5 } },
    } as TieredPricingConfig
    const { root, getValidationError } = mountEditor(pricing, {
      showProcessingTierControls: false,
      showProcessingTierMultiplierControls: true,
    })
    const multiplier = root.querySelector(
      '[data-testid="processing-tier-multiplier-flex"]',
    ) as HTMLInputElement
    expect(multiplier.value).toBe('0.5')

    multiplier.value = '-1'
    multiplier.dispatchEvent(new Event('input', { bubbles: true }))
    await nextTick()
    expect(getValidationError()).toContain('必须是非负有限数值')
  })

  it('requires a full-editor multiplier before persisting the new tier', async () => {
    const pricing = {
      tiers: [{ up_to: null, input_price_per_1m: 5, output_price_per_1m: 30 }],
    } as TieredPricingConfig
    const { root, getFinalPricing, getValidationError } = mountEditor(pricing, {
      autoFillMissingCachePrices: false,
      showProcessingTierMultiplierControls: true,
    })

    click(root.querySelector('[data-processing-tier="priority"]'))
    await nextTick()
    click(root.querySelector('[data-testid="processing-tier-add-multiplier"]'))
    await nextTick()

    const multiplier = root.querySelector(
      '[data-testid="processing-tier-multiplier-input"]',
    ) as HTMLInputElement
    expect(getValidationError()).toContain('请输入层级倍率')
    expect(() => getFinalPricing()).toThrow('请输入层级倍率')

    multiplier.value = '-1'
    multiplier.dispatchEvent(new Event('input', { bubbles: true }))
    await nextTick()
    expect(getValidationError()).toContain('必须是非负有限数值')

    multiplier.value = ''
    multiplier.dispatchEvent(new Event('input', { bubbles: true }))
    await nextTick()
    expect(getValidationError()).toContain('请输入层级倍率')
    expect(() => getFinalPricing()).toThrow('请输入层级倍率')
  })

  it('restores an explicit catalog when an incomplete multiplier conversion is cancelled', async () => {
    const pricing = {
      tiers: [{ up_to: null, input_price_per_1m: 5, output_price_per_1m: 30 }],
      processing_tiers: {
        priority: {
          tiers: [{ up_to: null, input_price_per_1m: 11, output_price_per_1m: 66 }],
          future_overlay_option: 'keep-on-cancel',
        },
      },
    } as TieredPricingConfig
    const { root, getFinalPricing, getValidationError } = mountEditor(pricing, {
      autoFillMissingCachePrices: false,
      showProcessingTierMultiplierControls: true,
    })

    click(root.querySelector('[data-testid="processing-tier-convert-priority"]'))
    click(root.querySelector('[data-processing-tier="priority"]'))
    await nextTick()
    expect(getValidationError()).toContain('请输入层级倍率')

    click(root.querySelector('[data-testid="processing-tier-use-custom"]'))
    await nextTick()

    expect(getValidationError()).toBeNull()
    expect(getFinalPricing().processing_tiers?.priority).toEqual(
      pricing.processing_tiers?.priority,
    )
  })

  it('lets the full Provider editor edit a multiplier or replace it with explicit prices', async () => {
    const pricing = {
      tiers: [{ up_to: null, input_price_per_1m: 5, output_price_per_1m: 30 }],
      processing_tiers: { priority: { price_multiplier: 2.5 } },
    } as TieredPricingConfig
    const { root, getFinalPricing } = mountEditor(pricing)

    click(root.querySelector('[data-processing-tier="priority"]'))
    await nextTick()
    const multiplier = root.querySelector(
      '[data-testid="processing-tier-multiplier-input"]',
    ) as HTMLInputElement
    expect(multiplier.value).toBe('2.5')
    multiplier.value = '3'
    multiplier.dispatchEvent(new Event('input', { bubbles: true }))
    await nextTick()
    expect(getFinalPricing().processing_tiers?.priority).toEqual({ price_multiplier: 3 })

    click(root.querySelector('[data-testid="processing-tier-use-custom"]'))
    await nextTick()
    expect(getFinalPricing().processing_tiers?.priority).toMatchObject({
      tiers: [{ up_to: null, input_price_per_1m: 5, output_price_per_1m: 30 }],
    })
    expect(getFinalPricing().processing_tiers?.priority).not.toHaveProperty('price_multiplier')
  })

  it('round-trips root, overlay and pricing-tier extension fields', () => {
    const pricing = {
      tiers: [{
        up_to: null,
        input_price_per_1m: 5,
        output_price_per_1m: 30,
        vendor_tier_note: 'keep-standard-tier',
      }],
      future_root_option: { enabled: true },
      processing_tiers: {
        priority: {
          tiers: [{
            up_to: null,
            input_price_per_1m: 10,
            output_price_per_1m: 60,
            vendor_tier_note: 'keep-priority-tier',
          }],
          contract_reference: 'priority-2026',
        },
        hyperlane: {
          tiers: [{
            up_to: null,
            input_price_per_1m: 7.5,
            output_price_per_1m: 42,
          }],
          future_overlay_option: { mode: 'reserved' },
        },
      },
    } as TieredPricingConfig

    const { getFinalPricing } = mountEditor(pricing)
    const result = getFinalPricing()

    expect(result.future_root_option).toEqual({ enabled: true })
    expect(result.tiers[0].vendor_tier_note).toBe('keep-standard-tier')
    expect(result.processing_tiers?.priority.contract_reference).toBe('priority-2026')
    expect(result.processing_tiers?.priority.tiers?.[0].vendor_tier_note).toBe('keep-priority-tier')
    expect(result.processing_tiers?.hyperlane.future_overlay_option).toEqual({ mode: 'reserved' })
  })

  it('shows known and discovered tiers and edits a discovered tier through the shared rate controls', async () => {
    const pricing = {
      tiers: [{ up_to: null, input_price_per_1m: 5, output_price_per_1m: 30 }],
      future_root_option: 'keep-root',
      processing_tiers: {
        hyperlane: {
          tiers: [{ up_to: null, input_price_per_1m: 7.5, output_price_per_1m: 42 }],
          future_overlay_option: 'keep-overlay',
        },
      },
    } as TieredPricingConfig
    const { root, onUpdate } = mountEditor(pricing)

    expect(root.querySelectorAll('[data-processing-tier]')).toHaveLength(6)
    expect(root.textContent).toContain('Standard')
    expect(root.textContent).toContain('Fast（OpenAI）')
    expect(root.textContent).toContain('Fast（Claude）')
    expect(root.textContent).not.toContain('Priority')
    expect(root.textContent).toContain('Flex')
    expect(root.textContent).toContain('Batch')
    expect(root.textContent).toContain('hyperlane')

    click(root.querySelector('[data-processing-tier="hyperlane"]'))
    await nextTick()

    const input = root.querySelector('[data-testid="tier-input-price"]') as HTMLInputElement | null
    if (!input) throw new Error('Expected the shared input-price control')
    input.value = '9.75'
    input.dispatchEvent(new Event('input', { bubbles: true }))
    await nextTick()

    const emitted = onUpdate.mock.lastCall?.[0] as TieredPricingConfig
    expect(emitted.processing_tiers?.hyperlane.tiers?.[0].input_price_per_1m).toBe(9.75)
    expect(emitted.processing_tiers?.hyperlane.future_overlay_option).toBe('keep-overlay')
    expect(emitted.future_root_option).toBe('keep-root')
  })

  it('adds and removes an explicit known-tier overlay without changing Standard', async () => {
    const pricing = {
      tiers: [{
        up_to: null,
        input_price_per_1m: 5,
        output_price_per_1m: 30,
        future_tier_option: 'keep-on-clone',
      }],
    } as TieredPricingConfig
    const { root, onUpdate } = mountEditor(pricing)

    click(root.querySelector('[data-processing-tier="priority"]'))
    await nextTick()
    expect(root.querySelector('[data-testid="processing-tier-empty"]')).not.toBeNull()

    click(root.querySelector('[data-testid="processing-tier-add"]'))
    await nextTick()

    let emitted = onUpdate.mock.lastCall?.[0] as TieredPricingConfig
    expect(emitted.tiers[0].input_price_per_1m).toBe(5)
    expect(emitted.processing_tiers?.priority.tiers?.[0]).toMatchObject(pricing.tiers[0])
    expect(emitted.processing_tiers?.priority.tiers?.[0].cache_creation_price_per_1m).toBe(6.25)
    expect(emitted.processing_tiers?.priority.tiers?.[0].cache_read_price_per_1m).toBe(0.5)

    expect(root.querySelector('[data-testid="processing-tier-remove"]'), root.innerHTML).not.toBeNull()
    click(root.querySelector('[data-testid="processing-tier-remove"]'))
    await nextTick()

    emitted = onUpdate.mock.lastCall?.[0] as TieredPricingConfig
    expect(emitted.processing_tiers).toBeUndefined()
    expect(emitted.tiers[0]).toMatchObject(pricing.tiers[0])
    expect(emitted.tiers[0].cache_creation_price_per_1m).toBe(6.25)
    expect(emitted.tiers[0].cache_read_price_per_1m).toBe(0.5)
  })

  it('keeps an unconfigured tier tab outside the persisted pricing contract', async () => {
    const pricing = {
      tiers: [{ up_to: null, input_price_per_1m: 5, output_price_per_1m: 30 }],
      image_output_price_default: 0.01,
    } as TieredPricingConfig
    const { getFinalPricing, getValidationError, root } = mountEditor(pricing, {
      showImagePricing: true,
    })

    click(root.querySelector('[data-processing-tier="priority"]'))
    await nextTick()

    expect(root.querySelector('[data-testid="processing-tier-empty"]')).not.toBeNull()
    expect(getValidationError()).toBeNull()
    const result = getFinalPricing()
    expect(result.tiers[0].input_price_per_1m).toBe(5)
    expect(result.image_output_price_default).toBe(0.01)
    expect(result.processing_tiers).toBeUndefined()
  })

  it.each([
    ['absent', undefined],
    ['null', null],
    ['empty object', {}],
  ] as const)('preserves an unedited %s processing_tiers value', (_, processingTiers) => {
    const pricing = {
      tiers: [{ up_to: null, input_price_per_1m: 5, output_price_per_1m: 30 }],
      ...(processingTiers === undefined ? {} : { processing_tiers: processingTiers }),
    } as TieredPricingConfig

    const { getFinalPricing } = mountEditor(pricing)
    const result = getFinalPricing()

    expect(Object.prototype.hasOwnProperty.call(result, 'processing_tiers'))
      .toBe(processingTiers !== undefined)
    expect(result.processing_tiers).toEqual(processingTiers)
  })

  it('edits every configured official processing tier through the same controls', async () => {
    const pricing = {
      tiers: [{ up_to: null, input_price_per_1m: 5, output_price_per_1m: 30 }],
      processing_tiers: Object.fromEntries(['priority', 'flex', 'batch'].map((key, index) => [
        key,
        {
          tiers: [{
            up_to: null,
            input_price_per_1m: index + 1,
            output_price_per_1m: (index + 1) * 6,
          }],
        },
      ])),
    } as TieredPricingConfig
    const { root, getFinalPricing } = mountEditor(pricing)

    for (const [index, key] of ['priority', 'flex', 'batch'].entries()) {
      click(root.querySelector(`[data-processing-tier="${key}"]`))
      await nextTick()

      const input = root.querySelector('[data-testid="tier-input-price"]') as HTMLInputElement | null
      if (!input) throw new Error(`Expected input-price control for ${key}`)
      input.value = String(11 + index)
      input.dispatchEvent(new Event('input', { bubbles: true }))
      await nextTick()
    }

    const result = getFinalPricing()
    expect(result.tiers[0].input_price_per_1m).toBe(5)
    expect(result.processing_tiers?.priority.tiers?.[0].input_price_per_1m).toBe(11)
    expect(result.processing_tiers?.flex.tiers?.[0].input_price_per_1m).toBe(12)
    expect(result.processing_tiers?.batch.tiers?.[0].input_price_per_1m).toBe(13)
  })

  it('keeps cache multiplier drafts isolated by processing scope', async () => {
    const pricing = {
      tiers: [{ up_to: null, input_price_per_1m: 5, output_price_per_1m: 30 }],
      processing_tiers: {
        priority: {
          tiers: [{ up_to: 272000, input_price_per_1m: 10, output_price_per_1m: 60 }],
        },
      },
    } as TieredPricingConfig
    const { root, getFinalPricing } = mountEditor(pricing)

    click(root.querySelector('[data-processing-tier="priority"]'))
    await nextTick()
    const multiplier = root.querySelector(
      'input[aria-label="Fast（OpenAI） 阶梯 1 缓存创建倍率"]',
    ) as HTMLInputElement
    multiplier.value = '2'
    multiplier.dispatchEvent(new Event('input', { bubbles: true }))
    await nextTick()

    const result = getFinalPricing()
    expect(result.tiers[0].cache_creation_price_per_1m).toBe(6.25)
    expect(result.processing_tiers?.priority.tiers?.[0].cache_creation_price_per_1m).toBe(20)
  })

  it('keeps absent cache prices empty and absent when automatic cache filling is disabled', () => {
    const pricing = {
      tiers: [{ up_to: null, input_price_per_1m: 5, output_price_per_1m: 30 }],
    } as TieredPricingConfig
    const { root, getFinalPricing } = mountEditor(pricing, {
      autoFillMissingCachePrices: false,
    })

    const creation = root.querySelector(
      'input[aria-label="Standard 阶梯 1 缓存创建倍率"]',
    ) as HTMLInputElement
    const read = root.querySelector(
      'input[aria-label="Standard 阶梯 1 缓存读取倍率"]',
    ) as HTMLInputElement

    expect(creation.value).toBe('')
    expect(read.value).toBe('')
    expect(getFinalPricing().tiers).toEqual([
      { up_to: null, input_price_per_1m: 5, output_price_per_1m: 30 },
    ])
  })

  it('preserves only the explicitly supplied side of cache pricing when automatic filling is disabled', () => {
    const pricing = {
      tiers: [
        {
          up_to: 128_000,
          input_price_per_1m: 5,
          output_price_per_1m: 30,
          cache_creation_price_per_1m: 6.25,
        },
        {
          up_to: null,
          input_price_per_1m: 7,
          output_price_per_1m: 42,
          cache_read_price_per_1m: 0.7,
        },
      ],
    } as TieredPricingConfig
    const { root, getFinalPricing } = mountEditor(pricing, {
      autoFillMissingCachePrices: false,
    })

    const creationValues = [...root.querySelectorAll<HTMLInputElement>(
      'input[aria-label*="缓存创建倍率"]',
    )].map(input => input.value)
    const readValues = [...root.querySelectorAll<HTMLInputElement>(
      'input[aria-label*="缓存读取倍率"]',
    )].map(input => input.value)

    expect(creationValues).toEqual(['1.25', ''])
    expect(readValues).toEqual(['', '0.1'])
    expect(getFinalPricing().tiers).toEqual(pricing.tiers)
  })

  it('adds and removes only the cache price edited by the user when automatic filling is disabled', async () => {
    const pricing = {
      tiers: [{ up_to: null, input_price_per_1m: 5, output_price_per_1m: 30 }],
    } as TieredPricingConfig
    const { root, getFinalPricing } = mountEditor(pricing, {
      autoFillMissingCachePrices: false,
    })
    const read = root.querySelector(
      'input[aria-label="Standard 阶梯 1 缓存读取倍率"]',
    ) as HTMLInputElement

    read.value = '0.2'
    read.dispatchEvent(new Event('input', { bubbles: true }))
    await nextTick()

    expect(getFinalPricing().tiers).toEqual([{
      up_to: null,
      input_price_per_1m: 5,
      output_price_per_1m: 30,
      cache_read_price_per_1m: 1,
    }])

    read.value = ''
    read.dispatchEvent(new Event('input', { bubbles: true }))
    await nextTick()

    expect(getFinalPricing().tiers).toEqual([{
      up_to: null,
      input_price_per_1m: 5,
      output_price_per_1m: 30,
    }])
  })

  it('does not turn absent cache prices into zero when switching editor modes', async () => {
    const pricing = {
      tiers: [{ up_to: null, input_price_per_1m: 5, output_price_per_1m: 30 }],
    } as TieredPricingConfig
    const { root, getFinalPricing } = mountEditor(pricing, {
      autoFillMissingCachePrices: false,
    })

    click(root.querySelector(
      'button[aria-label="Standard 阶梯 1 切换缓存价格输入方式"]',
    ))
    await nextTick()

    expect((root.querySelector(
      'input[aria-label="Standard 阶梯 1 缓存创建价格"]',
    ) as HTMLInputElement).value).toBe('')
    expect((root.querySelector(
      'input[aria-label="Standard 阶梯 1 缓存读取价格"]',
    ) as HTMLInputElement).value).toBe('')
    expect(getFinalPricing().tiers).toEqual(pricing.tiers)
  })

  it('rebuilds when an external model later matches an older emitted value', async () => {
    const pricing = {
      tiers: [{ up_to: null, input_price_per_1m: 5, output_price_per_1m: 30 }],
    } as TieredPricingConfig
    const { root, onUpdate, setModelValue } = mountEditor(pricing, {
      autoFillMissingCachePrices: false,
    })
    const read = root.querySelector(
      'input[aria-label="Standard 阶梯 1 缓存读取倍率"]',
    ) as HTMLInputElement

    read.value = '0.2'
    read.dispatchEvent(new Event('input', { bubbles: true }))
    await nextTick()
    const olderEmittedValue = onUpdate.mock.lastCall?.[0] as TieredPricingConfig

    setModelValue({
      tiers: [{ up_to: null, input_price_per_1m: 7, output_price_per_1m: 42 }],
    })
    await nextTick()
    expect((root.querySelector('[data-testid="tier-input-price"]') as HTMLInputElement).value)
      .toBe('7')

    setModelValue(olderEmittedValue)
    await nextTick()
    expect((root.querySelector('[data-testid="tier-input-price"]') as HTMLInputElement).value)
      .toBe('5')
    expect((root.querySelector(
      'input[aria-label="Standard 阶梯 1 缓存读取倍率"]',
    ) as HTMLInputElement).value).toBe('0.2')
  })

  it('keeps processing image catalogs editable when token controls are hidden', async () => {
    const pricing = {
      tiers: [{ up_to: null, input_price_per_1m: 5, output_price_per_1m: 30 }],
      processing_tiers: {
        priority: { image_output_price_default: 0.05 },
      },
    } as TieredPricingConfig
    const { root } = mountEditor(pricing, {
      showTokenPricing: false,
      showImagePricing: true,
      showImageEditor: true,
    })

    click(root.querySelector('[data-processing-tier="priority"]'))
    await nextTick()

    expect(root.querySelector('[data-testid="tier-input-price"]')).toBeNull()
    expect(root.querySelector('input[aria-label="Fast（OpenAI） 图像输出默认价格"]'))
      .not.toBeNull()
  })

  it('accepts a finite terminal tier for any processing overlay', () => {
    const pricing = {
      tiers: [{ up_to: null, input_price_per_1m: 5, output_price_per_1m: 30 }],
      processing_tiers: {
        priority: {
          tiers: [{ up_to: 272000, input_price_per_1m: 10, output_price_per_1m: 60 }],
        },
        hyperlane: {
          tiers: [{ up_to: 180000, input_price_per_1m: 7, output_price_per_1m: 42 }],
        },
      },
    } as TieredPricingConfig

    const { getFinalPricing, getValidationError } = mountEditor(pricing)

    expect(getValidationError()).toBeNull()
    expect(getFinalPricing().processing_tiers?.priority.tiers?.[0].up_to).toBe(272000)
    expect(getFinalPricing().processing_tiers?.hyperlane.tiers?.[0].up_to).toBe(180000)
  })

  it('keeps Standard terminal coverage unbounded', () => {
    const pricing = {
      tiers: [{ up_to: 272000, input_price_per_1m: 5, output_price_per_1m: 30 }],
    } as TieredPricingConfig

    const { getValidationError } = mountEditor(pricing)

    expect(getValidationError()).toBe('Standard: 最后一个阶梯必须是无上限的')
  })

  it('switches a processing terminal tier between finite and unbounded coverage', async () => {
    const pricing = {
      tiers: [{ up_to: null, input_price_per_1m: 5, output_price_per_1m: 30 }],
      processing_tiers: {
        priority: {
          tiers: [{ up_to: 272000, input_price_per_1m: 10, output_price_per_1m: 60 }],
        },
      },
    } as TieredPricingConfig
    const { root, getFinalPricing } = mountEditor(pricing)
    click(root.querySelector('[data-processing-tier="priority"]'))
    await nextTick()

    const terminal = root.querySelector(
      'select[aria-label="Fast（OpenAI） 阶梯 1 上限"]',
    ) as HTMLSelectElement
    expect(terminal.value).toBe('272000')

    terminal.value = '-2'
    terminal.dispatchEvent(new Event('change', { bubbles: true }))
    await nextTick()
    expect(getFinalPricing().processing_tiers?.priority.tiers?.[0].up_to).toBeNull()

    terminal.value = '272000'
    terminal.dispatchEvent(new Event('change', { bubbles: true }))
    await nextTick()
    expect(getFinalPricing().processing_tiers?.priority.tiers?.[0].up_to).toBe(272000)
  })

  it('preserves processing coverage when tiers are added and removed', async () => {
    const pricing = {
      tiers: [{ up_to: null, input_price_per_1m: 5, output_price_per_1m: 30 }],
      processing_tiers: {
        priority: {
          tiers: [{ up_to: 272000, input_price_per_1m: 10, output_price_per_1m: 60 }],
        },
      },
    } as TieredPricingConfig
    const { root, getFinalPricing } = mountEditor(pricing)
    click(root.querySelector('[data-processing-tier="priority"]'))
    await nextTick()

    const addButton = [...root.querySelectorAll('button')]
      .find(button => button.textContent?.includes('添加价格阶梯'))
    click(addButton ?? null)
    await nextTick()
    expect(getFinalPricing().processing_tiers?.priority.tiers?.map(tier => tier.up_to))
      .toEqual([272000, null])

    click(root.querySelector('button[aria-label="删除 Fast（OpenAI） 阶梯 2"]'))
    await nextTick()
    expect(getFinalPricing().processing_tiers?.priority.tiers?.map(tier => tier.up_to))
      .toEqual([272000])
  })

  it('preserves a special unknown processing tier key without prototype coercion', () => {
    const pricing = JSON.parse(`{
      "tiers": [{"up_to": null, "input_price_per_1m": 5, "output_price_per_1m": 30}],
      "processing_tiers": {
        "__proto__": {
          "tiers": [{"up_to": null, "input_price_per_1m": 7, "output_price_per_1m": 42}],
          "future_overlay_option": "keep"
        }
      }
    }`) as TieredPricingConfig

    const { root, getFinalPricing } = mountEditor(pricing)
    expect(root.textContent).toContain('__proto__')

    const result = getFinalPricing()
    expect(Object.prototype.hasOwnProperty.call(result.processing_tiers, '__proto__')).toBe(true)
    expect(result.processing_tiers?.__proto__.future_overlay_option).toBe('keep')
    expect(result.processing_tiers?.__proto__.tiers?.[0].input_price_per_1m).toBe(7)
  })

  it('preserves future image pricing fields when image pricing is enabled', () => {
    const pricing = {
      tiers: [{ up_to: null, input_price_per_1m: 5, output_price_per_1m: 30 }],
      image_output_prices: {
        '1024x1024': { low: 0.01, ultra: 0.09 },
      },
      image_output_price_ranges: [{
        up_to_pixels: 1_048_576,
        prices: { low: 0.01, ultra: 0.09 },
        future_range_option: { billing_unit: 'image' },
      }],
    } as TieredPricingConfig

    const { getFinalPricing } = mountEditor(pricing, { showImagePricing: true })
    const result = getFinalPricing()

    expect(result.image_output_prices?.['1024x1024'].ultra).toBe(0.09)
    expect(result.image_output_price_ranges?.[0].prices.ultra).toBe(0.09)
    expect(result.image_output_price_ranges?.[0].future_range_option)
      .toEqual({ billing_unit: 'image' })
  })

  it('rejects fractional image pixel limits without coercing them to integers', async () => {
    const pricing = {
      tiers: [{ up_to: null, input_price_per_1m: 5, output_price_per_1m: 30 }],
      image_output_price_ranges: [{
        up_to_pixels: 1_048_576,
        prices: { high: 0.07 },
      }],
    } as TieredPricingConfig
    const { getValidationError, root } = mountEditor(pricing, { showImagePricing: true })
    const limit = root.querySelector(
      'input[aria-label="图像像素区间 1 上限"]',
    ) as HTMLInputElement
    limit.value = '1.5'
    limit.dispatchEvent(new Event('input', { bubbles: true }))
    await nextTick()

    expect(getValidationError()).toBe('Standard: 图像像素区间 1 的上限必须是正整数')
  })

  it('treats an image-only processing overlay as a valid tier configuration', async () => {
    const pricing = {
      tiers: [{ up_to: null, input_price_per_1m: 5, output_price_per_1m: 30 }],
      processing_tiers: {
        hyperlane: {
          image_output_price_default: 0.08,
          future_overlay_option: { billing_unit: 'image' },
        },
      },
    } as TieredPricingConfig
    const { getFinalPricing, root } = mountEditor(pricing, { showImagePricing: true })

    click(root.querySelector('[data-processing-tier="hyperlane"]'))
    await nextTick()

    expect(root.textContent).not.toContain('至少需要一个价格阶梯')
    expect(getFinalPricing().processing_tiers?.hyperlane).toEqual(
      pricing.processing_tiers?.hyperlane,
    )
  })

  it('edits image pricing through the active processing-tier scope', async () => {
    const pricing = {
      tiers: [{ up_to: null, input_price_per_1m: 5, output_price_per_1m: 30 }],
      image_output_price_default: 0.01,
      processing_tiers: {
        priority: {
          image_output_price_default: 0.05,
          image_output_prices: {
            '1024x1024': { high: 0.08, ultra: 0.12 },
          },
          image_output_price_ranges: [{
            up_to_pixels: 1_048_576,
            prices: { high: 0.07, ultra: 0.11 },
            future_range_option: 'keep-priority',
          }],
        },
        flex: {
          image_output_price_default: 0.02,
        },
      },
    } as TieredPricingConfig
    const { getFinalPricing, root } = mountEditor(pricing, { showImagePricing: true })

    click(root.querySelector('[data-processing-tier="priority"]'))
    await nextTick()

    const priorityDefault = root.querySelector(
      'input[aria-label="Fast（OpenAI） 图像输出默认价格"]',
    ) as HTMLInputElement
    const priorityHigh = root.querySelector(
      'input[aria-label="1024x1024 high 图像输出价格"]',
    ) as HTMLInputElement
    expect(priorityDefault.value).toBe('0.05')
    expect(priorityHigh.value).toBe('0.08')

    priorityDefault.value = '0.06'
    priorityDefault.dispatchEvent(new Event('input', { bubbles: true }))
    priorityHigh.value = '0.09'
    priorityHigh.dispatchEvent(new Event('input', { bubbles: true }))
    await nextTick()

    click(root.querySelector('[data-processing-tier="flex"]'))
    await nextTick()
    const flexDefault = root.querySelector(
      'input[aria-label="Flex 图像输出默认价格"]',
    ) as HTMLInputElement
    expect(flexDefault.value).toBe('0.02')

    const result = getFinalPricing()
    expect(result.image_output_price_default).toBe(0.01)
    expect(result.processing_tiers?.priority.image_output_price_default).toBe(0.06)
    expect(result.processing_tiers?.priority.image_output_prices?.['1024x1024'].high).toBe(0.09)
    expect(result.processing_tiers?.priority.image_output_prices?.['1024x1024'].ultra).toBe(0.12)
    expect(result.processing_tiers?.priority.image_output_price_ranges?.[0].future_range_option)
      .toBe('keep-priority')
    expect(result.processing_tiers?.flex.image_output_price_default).toBe(0.02)
  })

  it('clears threshold editing state when removing and then adding tiers', async () => {
    const pricing = {
      tiers: [
        { up_to: 64_000, input_price_per_1m: 5, output_price_per_1m: 30 },
        { up_to: 128_000, input_price_per_1m: 7, output_price_per_1m: 42 },
        { up_to: null, input_price_per_1m: 9, output_price_per_1m: 54 },
      ],
    } as TieredPricingConfig
    const { root } = mountEditor(pricing)
    const thresholdSelects = root.querySelectorAll('select')
    const secondThreshold = thresholdSelects.item(1) as HTMLSelectElement

    secondThreshold.value = '-1'
    secondThreshold.dispatchEvent(new Event('change', { bubbles: true }))
    await nextTick()
    expect(root.querySelectorAll('input[placeholder="K"]')).toHaveLength(1)

    const tierRemoveButtons = Array.from(root.querySelectorAll('button'))
      .filter(button => button.querySelector('.lucide-x'))
    click(tierRemoveButtons[0] ?? null)
    await nextTick()

    const addTierButton = Array.from(root.querySelectorAll('button'))
      .find(button => button.textContent?.includes('添加价格阶梯'))
    click(addTierButton ?? null)
    await nextTick()

    expect(root.querySelectorAll('input[placeholder="K"]')).toHaveLength(0)
  })

  it('gives every compact pricing control an accessible name', () => {
    const pricing = {
      tiers: [
        { up_to: 64_000, input_price_per_1m: 5, output_price_per_1m: 30 },
        { up_to: null, input_price_per_1m: 7, output_price_per_1m: 42 },
      ],
    } as TieredPricingConfig
    const { root } = mountEditor(pricing, {
      showCache1h: true,
      showImagePricing: true,
    })

    for (const control of root.querySelectorAll('input, select')) {
      expect(control.getAttribute('aria-label'), control.outerHTML).toBeTruthy()
    }
    const iconOnlyButtons = Array.from(root.querySelectorAll('button'))
      .filter(button => button.textContent?.trim() === '')
    for (const button of iconOnlyButtons) {
      expect(button.getAttribute('aria-label'), button.outerHTML).toBeTruthy()
    }
  })

  it('blocks serialization when an inactive processing tier is invalid', async () => {
    const pricing = {
      tiers: [{ up_to: null, input_price_per_1m: 5, output_price_per_1m: 30 }],
      processing_tiers: {
        priority: {
          tiers: [
            { up_to: 128_000, input_price_per_1m: 10, output_price_per_1m: 60 },
            { up_to: 64_000, input_price_per_1m: 11, output_price_per_1m: 66 },
            { up_to: null, input_price_per_1m: 12, output_price_per_1m: 72 },
          ],
        },
      },
    } as TieredPricingConfig
    const { getFinalPricing, getValidationError, onUpdate, root } = mountEditor(pricing)
    const standardInput = root.querySelector('[data-testid="tier-input-price"]') as HTMLInputElement

    standardInput.value = '6'
    standardInput.dispatchEvent(new Event('input', { bubbles: true }))
    await nextTick()

    expect(onUpdate).not.toHaveBeenCalled()
    expect(getValidationError()).toContain('Fast（OpenAI）')
    expect(getValidationError()).toContain('上限必须大于前一个阶梯')
    expect(() => getFinalPricing()).toThrow('Fast（OpenAI）')
  })

  it('rejects negative known prices before they reach the billing contract', () => {
    const pricing = {
      tiers: [{ up_to: null, input_price_per_1m: -1, output_price_per_1m: 30 }],
    } as TieredPricingConfig
    const { getFinalPricing, getValidationError } = mountEditor(pricing)

    expect(getValidationError()).toBe('Standard: 阶梯 1 的输入价格必须是非负有限数值')
    expect(() => getFinalPricing()).toThrow('输入价格必须是非负有限数值')
  })
})
