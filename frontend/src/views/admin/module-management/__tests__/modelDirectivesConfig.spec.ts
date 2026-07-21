import { describe, expect, it } from 'vitest'

import {
  MODEL_DIRECTIVE_API_FORMATS,
  MODEL_DIRECTIVE_SUFFIX_METADATA,
  MODEL_DIRECTIVE_SUFFIXES,
  REASONING_EFFORTS,
  createDefaultModelDirectivesConfig,
  defaultModelDirectiveSuffixesForApiFormat,
  normalizeModelDirectivesConfig,
  updateModelDirectiveMappingOverride,
  updateModelDirectiveSuffixEnabled,
} from '../modelDirectivesConfig'

describe('modelDirectivesConfig', () => {
  it('defines the complete reasoning effort ladder in protocol order', () => {
    expect(REASONING_EFFORTS).toEqual([
      'none',
      'minimal',
      'low',
      'medium',
      'high',
      'xhigh',
      'max',
    ])
    expect(MODEL_DIRECTIVE_SUFFIXES).toEqual([
      ...REASONING_EFFORTS,
      'ultra',
      'fast',
    ])
    expect(defaultModelDirectiveSuffixesForApiFormat('openai:responses')).toContain('ultra')
    expect(defaultModelDirectiveSuffixesForApiFormat('openai:search')).toEqual(MODEL_DIRECTIVE_SUFFIXES)
    expect(defaultModelDirectiveSuffixesForApiFormat('claude:messages')).not.toContain('ultra')
    expect(defaultModelDirectiveSuffixesForApiFormat('gemini:generate_content')).not.toContain('ultra')
    expect(MODEL_DIRECTIVE_SUFFIX_METADATA.fast.description).toBe('Fast 服务层级')
  })

  it('creates a config whose mappings contain overrides only', () => {
    const config = createDefaultModelDirectivesConfig()

    expect(Object.keys(config.reasoning_effort.api_formats)).toEqual(
      MODEL_DIRECTIVE_API_FORMATS.map(format => format.key),
    )
    for (const format of MODEL_DIRECTIVE_API_FORMATS) {
      expect(config.reasoning_effort.api_formats[format.key]).toEqual({
        enabled: true,
        suffixes: [...defaultModelDirectiveSuffixesForApiFormat(format.key)],
        mappings: {},
      })
    }
  })

  it('normalizes configured suffixes without persisting built-in mappings', () => {
    const config = normalizeModelDirectivesConfig({
      reasoning_effort: {
        api_formats: {
          'openai:responses': {
            enabled: true,
            suffixes: ['low', 'MAX', 'ULTRA'],
          },
        },
      },
    })

    expect(config.reasoning_effort.api_formats['openai:responses'].mappings).toEqual({})
    expect(config.reasoning_effort.api_formats['openai:responses'].suffixes).toEqual([
      'low',
      'max',
      'ultra',
    ])
  })

  it('preserves every explicit mapping as an authoritative override', () => {
    const customHigh = { reasoning: { effort: 'high' }, trace: { sample: true } }
    const futureMapping = { vendor_option: 'keep-me' }
    const config = normalizeModelDirectivesConfig({
      reasoning_effort: {
        api_formats: {
          'openai:responses': {
            enabled: true,
            mappings: {
              low: { reasoning: { effort: 'low' } },
              max: { reasoning: { effort: 'xhigh' } },
              high: customHigh,
              future: futureMapping,
            },
          },
          'claude:messages': {
            enabled: true,
            mappings: {
              medium: { thinking: { type: 'enabled', budget_tokens: 4096 } },
              max: { thinking: { type: 'enabled', budget_tokens: 65536 } },
            },
          },
        },
      },
    })

    expect(config.reasoning_effort.api_formats['openai:responses'].mappings).toEqual({
      low: { reasoning: { effort: 'low' } },
      max: { reasoning: { effort: 'xhigh' } },
      high: customHigh,
      future: futureMapping,
    })
    expect(config.reasoning_effort.api_formats['openai:responses'].suffixes).toEqual([
      'low',
      'high',
      'max',
      'future',
    ])
    expect(config.reasoning_effort.api_formats['claude:messages'].mappings).toEqual({
      medium: { thinking: { type: 'enabled', budget_tokens: 4096 } },
      max: { thinking: { type: 'enabled', budget_tokens: 65536 } },
    })
    expect(config.reasoning_effort.api_formats['claude:messages'].suffixes).toEqual([
      'medium',
      'max',
    ])
  })

  it('preserves custom overrides and unknown fields exactly', () => {
    const customMax = { reasoning: { effort: 'max' }, trace: { sample: true } }
    const config = normalizeModelDirectivesConfig({
      future_option: { keep: true },
      reasoning_effort: {
        enabled: false,
        future_reasoning_option: 'keep-reasoning',
        api_formats: {
          'openai:responses': {
            enabled: true,
            future_format_option: 'keep-format',
            mappings: {
              max: customMax,
              future: { vendor_option: 'keep-future' },
            },
          },
          'vendor:future': {
            enabled: false,
            mappings: {
              custom: { vendor_option: 'keep-vendor' },
            },
          },
        },
      },
    })

    expect(config.future_option).toEqual({ keep: true })
    expect(config.reasoning_effort.future_reasoning_option).toBe('keep-reasoning')
    expect(config.reasoning_effort.api_formats['openai:responses']).toEqual({
      enabled: true,
      future_format_option: 'keep-format',
      suffixes: ['max', 'future'],
      mappings: {
        max: customMax,
        future: { vendor_option: 'keep-future' },
      },
    })
    expect(config.reasoning_effort.api_formats['vendor:future']).toEqual({
      enabled: false,
      suffixes: ['custom'],
      mappings: {
        custom: { vendor_option: 'keep-vendor' },
      },
    })
  })

  it('removes empty overrides without mutating unrelated custom mappings', () => {
    const mappings = {
      low: { reasoning: { effort: 'custom-low' } },
      future: { vendor_option: 'keep-future' },
    }

    expect(updateModelDirectiveMappingOverride(mappings, 'low', {})).toEqual({
      future: { vendor_option: 'keep-future' },
    })
    expect(updateModelDirectiveMappingOverride(mappings, 'max', {
      reasoning: { effort: 'custom-max' },
    })).toEqual({
      low: { reasoning: { effort: 'custom-low' } },
      max: { reasoning: { effort: 'custom-max' } },
      future: { vendor_option: 'keep-future' },
    })
    expect(mappings).toEqual({
      low: { reasoning: { effort: 'custom-low' } },
      future: { vendor_option: 'keep-future' },
    })
  })

  it('derives suffixes from configured mapping keys when suffixes are omitted', () => {
    const configuredMappings = Object.fromEntries([
      ['low', { reasoning_effort: 'low' }],
      ['medium', { reasoning_effort: 'medium' }],
      ['high', { reasoning_effort: 'high' }],
      ['xhigh', { reasoning_effort: 'xhigh' }],
      ['max', { reasoning_effort: 'xhigh' }],
      ['fast', { service_tier: 'priority' }],
    ])

    const config = normalizeModelDirectivesConfig({
      reasoning_effort: {
        api_formats: {
          'openai:chat': { enabled: true, mappings: configuredMappings },
        },
      },
    })

    expect(config.reasoning_effort.api_formats['openai:chat'].suffixes).toEqual([
      'low',
      'medium',
      'high',
      'xhigh',
      'max',
      'fast',
    ])
    expect(config.reasoning_effort.api_formats['openai:chat'].mappings).toEqual(configuredMappings)
  })

  it.each([
    ['none', ['minimal', 'low', 'medium', 'high', 'xhigh', 'max', 'fast']],
    ['minimal', ['none', 'low', 'medium', 'high', 'xhigh', 'max', 'fast']],
    ['none and minimal', ['low', 'medium', 'high', 'xhigh', 'max', 'fast']],
  ])('keeps explicitly disabled %s efforts disabled across normalization round-trips', (_, suffixes) => {
    const persisted = {
      reasoning_effort: {
        api_formats: {
          'openai:responses': {
            enabled: true,
            suffixes,
            mappings: {},
          },
        },
      },
    }

    const firstRead = normalizeModelDirectivesConfig(persisted)
    const secondRead = normalizeModelDirectivesConfig(firstRead)

    expect(firstRead.reasoning_effort.api_formats['openai:responses'].suffixes).toEqual(suffixes)
    expect(secondRead.reasoning_effort.api_formats['openai:responses'].suffixes).toEqual(suffixes)
  })

  it('canonicalizes known suffix casing while retaining unknown extensions', () => {
    const config = normalizeModelDirectivesConfig({
      reasoning_effort: {
        api_formats: {
          'openai:responses': {
            suffixes: [' MAX ', 'VendorFuture'],
            mappings: {
              MAX: { reasoning: { effort: 'xhigh' } },
              VendorFuture: { keep: true },
            },
          },
        },
      },
    })

    expect(config.reasoning_effort.api_formats['openai:responses']).toMatchObject({
      suffixes: ['max', 'VendorFuture'],
      mappings: {
        max: { reasoning: { effort: 'xhigh' } },
        VendorFuture: { keep: true },
      },
    })
  })

  it('ignores malformed suffix entries exactly as the backend parser does', () => {
    const config = normalizeModelDirectivesConfig({
      reasoning_effort: {
        api_formats: {
          'openai:responses': {
            suffixes: ['low', null, 42, { future: true }, 'VendorFuture'],
            mappings: {},
          },
        },
      },
    })

    expect(config.reasoning_effort.api_formats['openai:responses'].suffixes).toEqual([
      'low',
      'VendorFuture',
    ])
  })

  it('toggles one suffix without changing the rest of the allowlist', () => {
    expect(updateModelDirectiveSuffixEnabled(['low', 'max', 'fast'], 'max', false)).toEqual([
      'low',
      'fast',
    ])
    expect(updateModelDirectiveSuffixEnabled(['low', 'fast'], 'max', true)).toEqual([
      'low',
      'max',
      'fast',
    ])
  })

  it('preserves unknown extensions when a known suffix is toggled', () => {
    expect(updateModelDirectiveSuffixEnabled(
      ['VendorFuture', ' MAX ', 'VendorFuture', 'another-extension'],
      'low',
      true,
    )).toEqual([
      'low',
      'max',
      'VendorFuture',
      'another-extension',
    ])

    expect(updateModelDirectiveSuffixEnabled(
      ['low', 'VendorFuture', 'another-extension'],
      'low',
      false,
    )).toEqual([
      'VendorFuture',
      'another-extension',
    ])
  })
})
