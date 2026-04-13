import { describe, expect, it } from 'vitest'

import { normalizePoolAdvancedConfig } from '@/api/endpoints/types'

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
