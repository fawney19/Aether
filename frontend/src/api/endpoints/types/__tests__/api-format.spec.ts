import { describe, expect, it } from 'vitest'

import {
  API_FORMATS,
  formatApiFormat,
  formatApiFormatShort,
  groupApiFormats,
  normalizeApiFormatAlias,
  sortApiFormats,
} from '@/api/endpoints/types'

describe('api format display helpers', () => {
  it('normalizes current enum-style names to canonical api format ids', () => {
    expect(normalizeApiFormatAlias('CLAUDE_MESSAGES')).toBe(API_FORMATS.CLAUDE_MESSAGES)
    expect(normalizeApiFormatAlias('OPENAI_RESPONSES')).toBe(API_FORMATS.OPENAI_RESPONSES)
    expect(normalizeApiFormatAlias('OPENAI_RESPONSES_COMPACT')).toBe(API_FORMATS.OPENAI_RESPONSES_COMPACT)
    expect(normalizeApiFormatAlias('GEMINI_GENERATE_CONTENT')).toBe(API_FORMATS.GEMINI_GENERATE_CONTENT)
  })

  it('does not remap retired api format ids', () => {
    expect(normalizeApiFormatAlias('openai:cli')).toBe('openai:cli')
    expect(formatApiFormat('openai:cli')).toBe('openai:cli')
    expect(formatApiFormatShort('openai:cli')).toBe('op')

    expect(normalizeApiFormatAlias('openai:compact')).toBe('openai:compact')
    expect(formatApiFormat('openai:compact')).toBe('openai:compact')
    expect(formatApiFormatShort('openai:compact')).toBe('op')
  })

  it('does not remap retired enum-style aliases', () => {
    expect(normalizeApiFormatAlias('OPENAI_CLI')).toBe('openai_cli')
    expect(formatApiFormat('OPENAI_CLI')).toBe('openai_cli')
    expect(formatApiFormatShort('OPENAI_CLI')).toBe('op')

    expect(normalizeApiFormatAlias('OPENAI_COMPACT')).toBe('openai_compact')
    expect(formatApiFormat('OPENAI_COMPACT')).toBe('openai_compact')
    expect(formatApiFormatShort('OPENAI_COMPACT')).toBe('op')
  })

  it('sorts only current canonical formats into known slots', () => {
    expect(sortApiFormats([
      'openai:compact',
      API_FORMATS.OPENAI,
      API_FORMATS.OPENAI_RESPONSES,
    ])).toEqual([
      API_FORMATS.OPENAI,
      API_FORMATS.OPENAI_RESPONSES,
      'openai:compact',
    ])
  })

  it('groups retired enum-style aliases as unknown raw families', () => {
    expect(groupApiFormats(['OPENAI_CLI'])).toEqual([{
      family: 'openai_cli',
      label: 'openai_cli',
      formats: ['OPENAI_CLI'],
    }])
  })
})
