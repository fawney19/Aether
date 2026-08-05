import { describe, expect, it } from 'vitest'

import {
  getVideoBillingMode,
  getVideoDefaultPricePerSecond,
  getVideoPricePerSecondTable,
  getVideoTokenPriceEntries,
  hasVideoPerSecondPricing,
  hasVideoPerTokenPricing,
  hasVideoPricing,
} from '@/utils/form'

/** 一个先配了按秒、之后改用按 token 的模型 —— 秒价表仍留在配置里。 */
const switchedToPerToken = {
  billing: {
    video: {
      mode: 'per_token',
      price_per_second_by_resolution: { '720p': 2 },
      price_per_second_default: 0.5,
      token_prices_by_resolution: {
        '720p': { output_price_per_1m: 15 },
      },
    },
  },
}

const perSecondConfig = {
  billing: {
    video: {
      price_per_second_by_resolution: { '720p': 2, '1080p': 3 },
      price_per_second_default: 0.5,
    },
  },
}

describe('video billing config helpers', () => {
  it('缺省计费方式为按秒', () => {
    expect(getVideoBillingMode(perSecondConfig)).toBe('per_second')
    expect(getVideoBillingMode({})).toBe('per_second')
    expect(getVideoBillingMode(undefined)).toBe('per_second')
  })

  it('显式 per_token 才算按 token', () => {
    expect(getVideoBillingMode(switchedToPerToken)).toBe('per_token')
  })

  it('按 token 时不再暴露遗留的秒价', () => {
    // 两种计费方式是覆盖二选一，后设置的应完全盖住先设置的。
    expect(getVideoPricePerSecondTable(switchedToPerToken)).toEqual({})
    expect(getVideoDefaultPricePerSecond(switchedToPerToken)).toBeNull()
    expect(hasVideoPerSecondPricing(switchedToPerToken)).toBe(false)
  })

  it('按秒时正常返回价格表与默认价', () => {
    expect(getVideoPricePerSecondTable(perSecondConfig)).toEqual({ '720p': 2, '1080p': 3 })
    expect(getVideoDefaultPricePerSecond(perSecondConfig)).toBe(0.5)
    expect(hasVideoPerSecondPricing(perSecondConfig)).toBe(true)
  })

  it('只配了默认价也算配置了按秒计费', () => {
    const defaultOnly = { billing: { video: { price_per_second_default: 0.5 } } }
    expect(hasVideoPerSecondPricing(defaultOnly)).toBe(true)
    expect(getVideoPricePerSecondTable(defaultOnly)).toEqual({})
  })

  it('按 token 的配置判定只在 per_token 下成立', () => {
    expect(hasVideoPerTokenPricing(switchedToPerToken)).toBe(true)
    // 同样的 token 表，模式回到按秒后就不该算数。
    const backToPerSecond = {
      billing: {
        video: {
          token_prices_by_resolution: { '720p': { output_price_per_1m: 15 } },
        },
      },
    }
    expect(hasVideoPerTokenPricing(backToPerSecond)).toBe(false)
  })

  it('token 默认价与含视频输入表也能单独启用按 token 计费', () => {
    expect(hasVideoPerTokenPricing({
      billing: { video: { mode: 'per_token', token_price_default: { output_price_per_1m: 8 } } },
    })).toBe(true)
    expect(hasVideoPerTokenPricing({
      billing: {
        video: {
          mode: 'per_token',
          with_video_input: { token_prices_by_resolution: { '720p': { output_price_per_1m: 9 } } },
        },
      },
    })).toBe(true)
  })

  it('能力标签两种计费方式都算', () => {
    expect(hasVideoPricing(perSecondConfig)).toBe(true)
    expect(hasVideoPricing(switchedToPerToken)).toBe(true)
    expect(hasVideoPricing({ billing: { video: { mode: 'per_token' } } })).toBe(false)
    expect(hasVideoPricing({})).toBe(false)
  })

  it('无效输入不会抛错', () => {
    for (const input of [null, undefined, 'x', 42, [], { billing: 'x' }, { billing: { video: [] } }]) {
      expect(hasVideoPricing(input)).toBe(false)
      expect(getVideoPricePerSecondTable(input)).toEqual({})
      expect(getVideoDefaultPricePerSecond(input)).toBeNull()
      expect(getVideoTokenPriceEntries(input)).toEqual([])
    }
  })

  it('按 token 时能取到分辨率单价，供列表展示', () => {
    // 这些价格不在 default_tiered_pricing 里，展示端必须从 billing.video 取，
    // 否则整个价格列会退化成「-」。
    expect(getVideoTokenPriceEntries(switchedToPerToken)).toEqual([
      { resolution: '720p', inputPricePer1m: null, outputPricePer1m: 15 },
    ])
  })

  it('token 默认价作为最后一行返回', () => {
    const config = {
      billing: {
        video: {
          mode: 'per_token',
          token_prices_by_resolution: {
            '1080p': { input_price_per_1m: 2, output_price_per_1m: 30 },
            '720p': { output_price_per_1m: 15 },
          },
          token_price_default: { output_price_per_1m: 8 },
        },
      },
    }
    // 分辨率从低到高，默认价垫底。
    expect(getVideoTokenPriceEntries(config).map(e => e.resolution))
      .toEqual(['720p', '1080p', '默认'])
    expect(getVideoTokenPriceEntries(config)[1]).toEqual({
      resolution: '1080p',
      inputPricePer1m: 2,
      outputPricePer1m: 30,
    })
  })

  it('按秒模式下不返回 token 单价', () => {
    const perSecondWithStaleTokens = {
      billing: {
        video: {
          price_per_second_by_resolution: { '720p': 2 },
          token_prices_by_resolution: { '720p': { output_price_per_1m: 15 } },
        },
      },
    }
    expect(getVideoTokenPriceEntries(perSecondWithStaleTokens)).toEqual([])
  })

  it('忽略 token 表里没有任何有效价格的条目', () => {
    const messy = {
      billing: {
        video: {
          mode: 'per_token',
          token_prices_by_resolution: {
            '720p': { output_price_per_1m: 15 },
            '1080p': { output_price_per_1m: 'free' },
            '4k': {},
            '8k': 'nope',
          },
        },
      },
    }
    expect(getVideoTokenPriceEntries(messy).map(e => e.resolution)).toEqual(['720p'])
  })

  it('忽略价格表里的非数值项', () => {
    const messy = {
      billing: {
        video: {
          price_per_second_by_resolution: { '720p': 2, '1080p': 'free', '4k': null },
          price_per_second_default: 'oops',
        },
      },
    }
    expect(getVideoPricePerSecondTable(messy)).toEqual({ '720p': 2 })
    expect(getVideoDefaultPricePerSecond(messy)).toBeNull()
  })
})
