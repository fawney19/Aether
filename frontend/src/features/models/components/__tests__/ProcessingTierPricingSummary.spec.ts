import { afterEach, describe, expect, it } from 'vitest'
import { createApp, defineComponent, h, nextTick, type App } from 'vue'

import type { TieredPricingConfig } from '@/api/endpoints/types'
import ProcessingTierPricingSummary from '../ProcessingTierPricingSummary.vue'

const mountedApps: Array<{ app: App, root: HTMLElement }> = []

function mountSummary(pricing: TieredPricingConfig) {
  const root = document.createElement('div')
  document.body.appendChild(root)
  const app = createApp(defineComponent({
    setup: () => () => h(ProcessingTierPricingSummary, { pricing }),
  }))
  app.mount(root)
  mountedApps.push({ app, root })
  return root
}

function clickTier(root: HTMLElement, tier: string) {
  const button = root.querySelector(`[data-processing-tier="${tier}"]`)
  if (!(button instanceof HTMLButtonElement)) throw new Error(`Missing ${tier} tier button`)
  button.click()
}

afterEach(() => {
  for (const { app, root } of mountedApps.splice(0)) {
    app.unmount()
    root.remove()
  }
})

describe('ProcessingTierPricingSummary', () => {
  it('shows multiplier-only processing tiers', async () => {
    const root = mountSummary({
      tiers: [{ up_to: null, input_price_per_1m: 5, output_price_per_1m: 30 }],
      processing_tiers: {
        priority: { price_multiplier: 2.5 },
        fast: { price_multiplier: 2 },
        flex: {
          price_multiplier: 99,
          tiers: [{ up_to: null, input_price_per_1m: 2.5, output_price_per_1m: 15 }],
        },
      },
    })

    expect(root.querySelector('[data-processing-tier="priority"]')?.textContent)
      .toContain('Fast（OpenAI）')
    expect(root.querySelector('[data-processing-tier="fast"]')?.textContent)
      .toContain('Fast（Claude）')
    expect(root.textContent).not.toContain('Priority')
    expect(root.querySelector('[data-testid="processing-tier-price-multiplier"]')?.textContent)
      .toContain('2.5×')
    clickTier(root, 'fast')
    await nextTick()
    expect(root.querySelector('[data-testid="processing-tier-price-multiplier"]')?.textContent)
      .toContain('2×')

    clickTier(root, 'flex')
    await nextTick()
    expect(root.querySelector('[data-testid="processing-tier-price-multiplier"]')).toBeNull()
    expect(root.querySelectorAll('[data-testid="processing-token-tier-row"]')).toHaveLength(1)
  })

  it('shows finite and unbounded token tiers in stable processing-tier order', () => {
    const root = mountSummary({
      tiers: [{ up_to: null, input_price_per_1m: 5, output_price_per_1m: 30 }],
      processing_tiers: {
        hyperlane: {
          tiers: [{ up_to: 128_000, input_price_per_1m: 7, output_price_per_1m: 35 }],
        },
        priority: {
          tiers: [
            {
              up_to: 272_000,
              input_price_per_1m: 10,
              output_price_per_1m: 60,
              cache_creation_price_per_1m: 12.5,
              cache_read_price_per_1m: 1,
              cache_ttl_pricing: [{ ttl_minutes: 60, cache_creation_price_per_1m: 20 }],
            },
            { up_to: null, input_price_per_1m: 20, output_price_per_1m: 120 },
          ],
        },
        empty: {},
      },
    })

    expect([...root.querySelectorAll('[data-processing-tier]')].map(element => (
      element.getAttribute('data-processing-tier')
    ))).toEqual(['priority', 'hyperlane'])
    expect(root.querySelectorAll('[data-testid="processing-token-tier-row"]')).toHaveLength(2)
    expect(root.textContent).toContain('0 - 272K')
    expect(root.textContent).toContain('> 272K')
    expect(root.textContent).toContain('$20.00')
  })

  it('renders image-only overlays, zero prices and future qualities', async () => {
    const root = mountSummary({
      tiers: [{ up_to: null, input_price_per_1m: 5, output_price_per_1m: 30 }],
      processing_tiers: {
        flex: {
          image_output_price_default: 0,
          image_output_prices: {
            '1024x1024': { low: 0, high: 0.04 },
          },
        },
        hyperlane: {
          image_output_price_ranges: [
            { up_to_pixels: null, prices: { ultra: 0.05 } },
            { up_to_pixels: 1_048_576, prices: { ultra: 0.03 } },
          ],
        },
      },
    })

    expect(root.querySelectorAll('[data-testid="processing-token-tier-row"]')).toHaveLength(0)
    expect(root.textContent).toContain('默认 $0.00/张')
    expect(root.textContent).toContain('1024 x 1024')
    expect(root.textContent).toContain('$0.04')

    clickTier(root, 'hyperlane')
    await nextTick()

    expect(root.textContent).toContain('ultra')
    expect(root.textContent).toContain('0 - 1.05M px')
    expect(root.textContent).toContain('> 1.05M px')
    expect(root.textContent).toContain('$0.03')
    expect(root.textContent!.indexOf('0 - 1.05M px')).toBeLessThan(
      root.textContent!.indexOf('> 1.05M px'),
    )
  })
})
