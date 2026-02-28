import { describe, it, expect } from 'vitest'
import { parseResponse, renderResponse } from '../registry'

describe('Gemini conversation parser', () => {
  const requestBody = { model: 'gemini-3-pro-preview' }
  const normalizedResponse = {
    status: 'completed',
    output: [
      {
        type: 'message',
        role: 'assistant',
        content: [
          {
            type: 'output_text',
            text: 'Hello!',
          },
        ],
      },
    ],
  }

  it('parses normalized output payload when hint is gemini', () => {
    const parsed = parseResponse(normalizedResponse, requestBody, 'gemini:chat')
    expect(parsed.apiFormat).toBe('gemini')
    expect(parsed.messages).toHaveLength(1)
    expect(parsed.messages[0]?.role).toBe('assistant')
    expect(parsed.messages[0]?.content[0]).toMatchObject({
      type: 'text',
      text: 'Hello!',
    })
  })

  it('renders normalized output payload when hint is gemini', () => {
    const rendered = renderResponse(normalizedResponse, requestBody, 'gemini:chat')
    expect(rendered.error).toBeUndefined()
    expect(rendered.blocks).toHaveLength(1)
    expect(rendered.blocks[0]).toMatchObject({
      type: 'message',
      role: 'assistant',
    })

    const firstBlock = rendered.blocks[0]
    if (!firstBlock || firstBlock.type !== 'message') {
      throw new Error('expected first render block to be message')
    }

    expect(firstBlock.content[0]).toMatchObject({
      type: 'text',
      content: 'Hello!',
    })
  })
})
