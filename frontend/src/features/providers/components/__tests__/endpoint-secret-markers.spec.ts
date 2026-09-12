import { describe, expect, it } from 'vitest'

import {
  endpointSecretMarkerPayload,
  retainsEndpointSecret,
} from '../endpoint-secret-markers'

describe('endpoint secret marker contract', () => {
  it('retains only a marked redacted placeholder', () => {
    expect(retainsEndpointSecret('***', true)).toBe(true)
    expect(retainsEndpointSecret('***', false)).toBe(false)
    expect(retainsEndpointSecret('new-secret', true)).toBe(false)
  })

  it('emits a marker only while the redacted value remains untouched', () => {
    expect(endpointSecretMarkerPayload('has_value', true, '***')).toEqual({ has_value: true })
    expect(endpointSecretMarkerPayload('has_pattern', true, '***')).toEqual({ has_pattern: true })
    expect(endpointSecretMarkerPayload('has_replacement', false, '***')).toEqual({})
    expect(endpointSecretMarkerPayload('has_value', true, 'new-secret')).toEqual({})
  })
})
