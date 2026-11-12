import { describe, expect, it } from 'vitest'
import { API_FORMATS, formatApiFormat, formatApiFormatShort, normalizeApiFormatAlias } from '@/api/endpoints/types/api-format'
import { getDefaultEndpointBaseUrl, getDefaultEndpointPath } from '../../endpoint-default-paths'
import { buildDefaultModelTestRequestBody, extractModelTestResponsePreview } from '../model-test-request'

describe('Gemini Decisions', () => {
  it('统一显示名称、别名和端点默认值', () => {
    expect(normalizeApiFormatAlias('GEMINI_DECISIONS')).toBe('gemini:decisions')
    expect(normalizeApiFormatAlias('GEMINI_INTERACTIONS')).toBe('gemini:interactions')
    expect(formatApiFormat(API_FORMATS.GEMINI_DECISIONS)).toBe('Gemini Decisions')
    expect(formatApiFormatShort(API_FORMATS.GEMINI_DECISIONS)).toBe('GD')
    expect(getDefaultEndpointBaseUrl({ apiFormat: 'gemini:decisions', baseUrl: 'https://generativelanguage.googleapis.com' })).toBe('https://generativelanguage.googleapis.com/v1beta')
    expect(getDefaultEndpointPath({ apiFormat: 'gemini:decisions', apiFormats: [{ value: 'gemini:decisions', default_path: '/v1beta/decisions' }] })).toBe('/decisions')
  })

  it('模型测试使用原生问题结构且展示判断结果', () => {
    const body = JSON.parse(buildDefaultModelTestRequestBody('gemini-decision-1', 'gemini:decisions'))
    expect(body).toEqual({ model: 'gemini-decision-1', input: 'Hello! This is a test message.', questions: { greeting: { type: 'noul', instructions: 'Does the input contain a greeting?' } } })
    const answers = { greeting: { type: 'noul', noul: 0.99, confidence: 0.99 } }
    expect(extractModelTestResponsePreview({ answers, usage: { total_input_tokens: 80, total_output_tokens: 0 } })).toBe(JSON.stringify(answers))
  })
})
