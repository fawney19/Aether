import { describe, expect, it } from 'vitest'
import {
  normalizeProviderKeyScope,
  providerKeyScopeForSubmit,
  providerKeyScopeFromApi,
  providerKeyScopeSelectedCount,
  restrictProviderKeyScopeToProviders,
} from '../providerKeyScope'

describe('providerKeyScope helpers', () => {
  it('normalizes trims, dedups, sorts and drops empty entries', () => {
    expect(
      normalizeProviderKeyScope({
        'provider-1': ['key-b', ' key-a ', 'key-b', ''],
        'provider-2': [],
        'provider-3': ['key-c'],
      }),
    ).toEqual({
      'provider-1': ['key-a', 'key-b'],
      'provider-3': ['key-c'],
    })
  })

  it('returns null for absent, empty or all-empty scopes', () => {
    expect(normalizeProviderKeyScope(null)).toBeNull()
    expect(normalizeProviderKeyScope(undefined)).toBeNull()
    expect(normalizeProviderKeyScope({})).toBeNull()
    expect(normalizeProviderKeyScope({ 'provider-1': [] })).toBeNull()
  })

  it('round-trips backend values into editor state', () => {
    expect(providerKeyScopeFromApi({ 'provider-1': ['a'] })).toEqual({ 'provider-1': ['a'] })
    expect(providerKeyScopeFromApi(null)).toEqual({})
    expect(providerKeyScopeFromApi(undefined)).toEqual({})
  })

  it('builds null payload when providers are unrestricted', () => {
    expect(providerKeyScopeForSubmit({ 'provider-1': ['a'] }, true)).toBeNull()
    expect(providerKeyScopeForSubmit({}, false)).toBeNull()
    expect(providerKeyScopeForSubmit({ 'provider-1': ['a'] }, false)).toEqual({
      'provider-1': ['a'],
    })
  })

  it('restricts scope entries to the selected providers', () => {
    expect(
      restrictProviderKeyScopeToProviders(
        { 'provider-1': ['a'], 'provider-2': ['b'] },
        ['provider-1'],
      ),
    ).toEqual({ 'provider-1': ['a'] })
  })

  it('counts selected keys for display', () => {
    expect(providerKeyScopeSelectedCount({})).toBe(0)
    expect(providerKeyScopeSelectedCount({ 'provider-1': ['a', 'b'], 'provider-2': ['c'] })).toBe(3)
  })
})
