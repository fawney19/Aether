import { describe, expect, it } from 'vitest'
import { formatClientFamily } from '../clientFamily'

describe('formatClientFamily', () => {
  it('labels supported clients and SDKs', () => {
    expect(formatClientFamily('qwen_code')).toBe('Qwen Code')
    expect(formatClientFamily('roo_code')).toBe('Roo Code')
    expect(formatClientFamily('kilocode')).toBe('KiloCode')
    expect(formatClientFamily('cherrystudio')).toBe('Cherry Studio')
    expect(formatClientFamily('openui')).toBe('OpenUI')
    expect(formatClientFamily('openai_python_sdk')).toBe('OpenAI Python SDK')
    expect(formatClientFamily('anthropic_js_sdk')).toBe('Anthropic JS SDK')
  })

  it('renders unrecognized families as unknown', () => {
    expect(formatClientFamily(null)).toBe('unknown')
    expect(formatClientFamily('')).toBe('unknown')
    expect(formatClientFamily('custom-client')).toBe('unknown')
  })
})
