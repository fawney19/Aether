import { describe, expect, it } from 'vitest'

import {
  ALL_REQUESTS_SCOPE_VALUE,
  COMPACT_REQUEST_SCOPE_VALUE,
  formatModelMappingEndpointLabel,
  formatModelMappingRequestScope,
  modelMappingEndpointScopeSupportsSessionCompaction,
  modelMappingOperationsKey,
  modelMappingOperationsFromScopeValue,
  modelMappingRequestScopeOptions,
  modelMappingRequestScopeValue,
  normalizeModelMappingOperations,
} from '../modelMappingScope'

describe('model mapping request scope', () => {
  it('represents an omitted operation filter as all requests', () => {
    expect(modelMappingRequestScopeValue(undefined)).toBe(ALL_REQUESTS_SCOPE_VALUE)
    expect(modelMappingOperationsFromScopeValue(ALL_REQUESTS_SCOPE_VALUE)).toBeUndefined()
    expect(formatModelMappingRequestScope(undefined)).toBe('所有请求')
  })

  it('round-trips the compact operation as a dedicated request scope', () => {
    expect(modelMappingRequestScopeValue(['compact'])).toBe(COMPACT_REQUEST_SCOPE_VALUE)
    expect(modelMappingOperationsFromScopeValue(COMPACT_REQUEST_SCOPE_VALUE)).toEqual(['compact'])
    expect(formatModelMappingRequestScope(['compact'])).toBe('仅会话压缩')
  })

  it('normalizes operation values using the backend matching semantics', () => {
    expect(normalizeModelMappingOperations([' Compact ', 'compact', '', 'SEARCH'])).toEqual([
      'compact',
      'search',
    ])
    expect(modelMappingOperationsKey(['SEARCH', ' compact ', 'Compact'])).toBe('compact,search')
  })

  it('preserves an unknown operation scope while editing', () => {
    const operations = ['future_operation', 'compact']
    const value = modelMappingRequestScopeValue(operations)
    const options = modelMappingRequestScopeOptions(
      operations,
      { sessionCompaction: true },
    )

    expect(modelMappingOperationsFromScopeValue(value)).toEqual(operations)
    expect(options).toContainEqual({ value, label: '仅匹配：future_operation, compact' })
  })

  it('offers compact scope only when the selected endpoint scope supports it', () => {
    expect(modelMappingRequestScopeOptions(undefined, { sessionCompaction: false }))
      .toEqual([{ value: ALL_REQUESTS_SCOPE_VALUE, label: '所有请求' }])
    expect(modelMappingRequestScopeOptions(undefined, { sessionCompaction: true }))
      .toContainEqual({ value: COMPACT_REQUEST_SCOPE_VALUE, label: '仅会话压缩' })
  })

  it('requires every explicitly selected endpoint to use OpenAI Responses', () => {
    const responsesEndpoint = {
      id: 'responses',
      api_format: 'OPENAI_RESPONSES',
      base_url: 'https://api.example.com/v1',
      is_active: true,
    }
    const chatEndpoint = {
      id: 'chat',
      api_format: 'openai:chat',
      base_url: 'https://api.example.com/v1',
      is_active: true,
    }

    expect(modelMappingEndpointScopeSupportsSessionCompaction(
      undefined,
      [responsesEndpoint, chatEndpoint],
    )).toBe(false)
    expect(modelMappingEndpointScopeSupportsSessionCompaction(
      [responsesEndpoint.id],
      [responsesEndpoint, chatEndpoint],
    )).toBe(true)
    expect(modelMappingEndpointScopeSupportsSessionCompaction(
      [responsesEndpoint.id, chatEndpoint.id],
      [responsesEndpoint, chatEndpoint],
    )).toBe(false)
  })

  it('rejects malformed scope values without constructing operations', () => {
    expect(modelMappingOperationsFromScopeValue('compact')).toBeUndefined()
    expect(modelMappingOperationsFromScopeValue('{"compact":true}')).toBeUndefined()
  })

  it('disambiguates endpoints that share an API format', () => {
    const endpoints = [
      {
        id: 'endpoint-1',
        api_format: 'openai:responses',
        base_url: 'https://api.example.com/v1',
        is_active: true,
      },
      {
        id: 'endpoint-2',
        api_format: 'openai:responses',
        base_url: 'https://backup.example.com/v1',
        custom_path: '/backend-api/codex/responses',
        is_active: false,
      },
    ]

    expect(formatModelMappingEndpointLabel(endpoints[0], endpoints)).toBe(
      'OpenAI Responses · api.example.com/v1',
    )
    expect(formatModelMappingEndpointLabel(endpoints[1], endpoints)).toBe(
      'OpenAI Responses · backup.example.com/backend-api/codex/responses（停用）',
    )
  })
})
