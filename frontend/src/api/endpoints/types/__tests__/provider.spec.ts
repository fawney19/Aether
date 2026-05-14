import { describe, expect, it } from 'vitest'

import { normalizeChatPiiRedactionProviderConfig, normalizePoolAdvancedConfig } from '@/api/endpoints/types'

describe('normalizePoolAdvancedConfig', () => {
  it('keeps object payloads, including empty objects', () => {
    expect(normalizePoolAdvancedConfig({})).toEqual({})
    expect(normalizePoolAdvancedConfig({ global_priority: 5 })).toEqual({ global_priority: 5 })
  })

  it('maps legacy boolean payloads to the current object semantics', () => {
    expect(normalizePoolAdvancedConfig(true)).toEqual({})
    expect(normalizePoolAdvancedConfig(false)).toBeNull()
  })

  it('drops unsupported payload shapes', () => {
    expect(normalizePoolAdvancedConfig(null)).toBeNull()
    expect(normalizePoolAdvancedConfig('enabled')).toBeNull()
    expect(normalizePoolAdvancedConfig(['lru'])).toBeNull()
  })
})


describe('normalizeChatPiiRedactionProviderConfig', () => {
  it('defaults unsupported payloads to disabled', () => {
    expect(normalizeChatPiiRedactionProviderConfig(null)).toEqual({ enabled: false })
    expect(normalizeChatPiiRedactionProviderConfig({})).toEqual({ enabled: false })
    expect(normalizeChatPiiRedactionProviderConfig({ enabled: 'yes' })).toEqual({ enabled: false })
  })

  it('passes through enabled state only', () => {
    expect(normalizeChatPiiRedactionProviderConfig({ enabled: true })).toEqual({ enabled: true })
    expect(normalizeChatPiiRedactionProviderConfig({ enabled: false, entities: ['email'] })).toEqual({ enabled: false })
  })
})
