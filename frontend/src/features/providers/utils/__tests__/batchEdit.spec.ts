import { describe, expect, it } from 'vitest'
import {
  buildProviderBasicPatch,
  buildUpstreamStreamConfig,
  collectFormatUnion,
  computeEndpointTargets,
  createDefaultProviderBasicState,
  getProviderBasicFieldLabels,
  hasProviderBasicPatch,
  mergeRules,
  toProviderOptions,
  validateProviderBasicState,
} from '../batchEdit'

describe('toProviderOptions', () => {
  it('formats providers for MultiSelect labels', () => {
    expect(toProviderOptions([
      { id: 'p1', name: 'OpenAI', provider_type: 'openai', is_active: true },
      { id: 'p2', name: 'Claude', provider_type: '', is_active: false },
    ])).toEqual([
      { value: 'p1', label: 'OpenAI · openai · 活跃' },
      { value: 'p2', label: 'Claude · custom · 停用' },
    ])
  })
})

describe('collectFormatUnion', () => {
  it('returns sorted unique api formats', () => {
    expect(collectFormatUnion([
      { id: 'p1', name: 'P1', endpoints: [{ id: 'e1', api_format: 'openai' }, { id: 'e2', api_format: 'anthropic' }] },
      { id: 'p2', name: 'P2', endpoints: [{ id: 'e3', api_format: 'openai' }] },
    ])).toEqual(['anthropic', 'openai'])
  })
})

describe('provider basic batch helpers', () => {
  it('keeps the default state out of update payloads', () => {
    const state = createDefaultProviderBasicState()

    expect(hasProviderBasicPatch(state)).toBe(false)
    expect(getProviderBasicFieldLabels(state)).toEqual([])
    expect(buildProviderBasicPatch(state)).toEqual({})
    expect(validateProviderBasicState(state)).toBeNull()
  })

  it('builds only selected provider fields and clears empty timeout overrides', () => {
    const state = createDefaultProviderBasicState()
    state.maxRetries = { include: true, value: 3 }
    state.streamFirstByteTimeout = { include: true, value: undefined }
    state.requestTimeout = { include: true, value: 120 }
    state.keepPriorityOnConversion = { include: true, value: true }

    expect(getProviderBasicFieldLabels(state)).toEqual([
      '最大重试次数',
      '流式首字节超时',
      '非流式请求超时',
      '格式转换保持优先级',
    ])
    expect(buildProviderBasicPatch(state)).toEqual({
      max_retries: 3,
      stream_first_byte_timeout: null,
      request_timeout: 120,
      keep_priority_on_conversion: true,
    })
  })

  it('requires max retries when that field is selected', () => {
    const state = createDefaultProviderBasicState()
    state.maxRetries.include = true

    expect(validateProviderBasicState(state)).toBe('最大重试次数不能为空')
    expect(buildProviderBasicPatch(state)).toEqual({})
  })
})

describe('computeEndpointTargets', () => {
  const providers = [
    { id: 'p1', name: 'P1', endpoints: [{ id: 'e1', api_format: 'openai' }, { id: 'e2', api_format: 'anthropic' }] },
    { id: 'p2', name: 'P2', endpoints: [{ id: 'e3', api_format: 'gemini' }] },
    { id: 'p3', name: 'P3', endpoints: [] },
  ]

  it('selects endpoints matching selected formats', () => {
    const result = computeEndpointTargets(providers, ['openai', 'anthropic'])
    expect(result.targets.map(target => target.endpointId).sort()).toEqual(['e1', 'e2'])
    expect(result.matchedProviderCount).toBe(1)
    expect(result.skippedProviders.map(provider => provider.id).sort()).toEqual(['p2', 'p3'])
  })
})

describe('buildUpstreamStreamConfig', () => {
  it('removes legacy keys and returns null when auto leaves empty config', () => {
    expect(buildUpstreamStreamConfig({ upstreamStreamPolicy: 'force_stream' }, 'auto')).toBeNull()
  })

  it('sets upstream_stream_policy for non-auto policies', () => {
    expect(buildUpstreamStreamConfig({ keep: true, upstream_stream: false }, 'force_non_stream')).toEqual({
      keep: true,
      upstream_stream_policy: 'force_non_stream',
    })
  })
})

describe('mergeRules', () => {
  it('appends incoming rules after existing rules', () => {
    expect(mergeRules([{ action: 'set', key: 'A' }], [{ action: 'drop', key: 'B' }], 'append')).toEqual([
      { action: 'set', key: 'A' },
      { action: 'drop', key: 'B' },
    ])
  })

  it('overwrites existing rules', () => {
    expect(mergeRules([{ action: 'set', key: 'A' }], [], 'overwrite')).toEqual([])
  })
})
