import { describe, expect, it } from 'vitest'

import {
  API_FORMATS,
  formatApiFormat,
  formatApiFormatShort,
  groupApiFormats,
  normalizeApiFormatAlias,
  sortApiFormats,
} from '@/api/endpoints/types'

const openaiAlias = (kind: string) => ['openai', kind].join(':')
const legacyEnumAlias = (kind: string) => ['OPENAI', kind].join('_')

describe('api format display helpers', () => {
  it('maps historical OpenAI response aliases to current display names', () => {
    expect(normalizeApiFormatAlias(openaiAlias('cli'))).toBe(API_FORMATS.OPENAI_RESPONSES)
    expect(formatApiFormat(openaiAlias('cli'))).toBe('OpenAI Responses')
    expect(formatApiFormatShort(openaiAlias('cli'))).toBe('OR')

    expect(normalizeApiFormatAlias(openaiAlias('compact'))).toBe(API_FORMATS.OPENAI_RESPONSES_COMPACT)
    expect(formatApiFormat(openaiAlias('compact'))).toBe('OpenAI Responses Compact')
    expect(formatApiFormatShort(openaiAlias('compact'))).toBe('ORC')
  })

  it('maps historical uppercase enum aliases to current display names', () => {
    expect(normalizeApiFormatAlias(legacyEnumAlias('CLI'))).toBe(API_FORMATS.OPENAI_RESPONSES)
    expect(formatApiFormat(legacyEnumAlias('CLI'))).toBe('OpenAI Responses')
    expect(formatApiFormatShort(legacyEnumAlias('CLI'))).toBe('OR')

    expect(normalizeApiFormatAlias(legacyEnumAlias('COMPACT'))).toBe(API_FORMATS.OPENAI_RESPONSES_COMPACT)
    expect(formatApiFormat(legacyEnumAlias('COMPACT'))).toBe('OpenAI Responses Compact')
    expect(formatApiFormatShort(legacyEnumAlias('COMPACT'))).toBe('ORC')
  })

  it('sorts historical aliases in the same slot as their current formats', () => {
    expect(sortApiFormats([
      openaiAlias('compact'),
      API_FORMATS.OPENAI,
      openaiAlias('cli'),
    ])).toEqual([
      API_FORMATS.OPENAI,
      openaiAlias('cli'),
      openaiAlias('compact'),
    ])
  })

  it('groups uppercase historical aliases under OpenAI', () => {
    expect(groupApiFormats([legacyEnumAlias('CLI')])).toEqual([{
      family: 'openai',
      label: 'OpenAI',
      formats: [legacyEnumAlias('CLI')],
    }])
  })
})
