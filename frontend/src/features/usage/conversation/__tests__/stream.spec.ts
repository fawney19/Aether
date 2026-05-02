import { describe, expect, it } from 'vitest'
import { parseResponse, renderResponse } from '../registry'

describe('Conversation stream compatibility', () => {
  it('parses raw OpenAI chat SSE text from stored usage records', () => {
    const requestBody = {
      model: 'gpt-5.4',
      stream: true,
      messages: [
        { role: 'user', content: 'Hello' },
      ],
    }
    const rawSse = [
      'data: {"id":"chatcmpl_123","object":"chat.completion.chunk","model":"gpt-5.4","choices":[{"index":0,"delta":{"role":"assistant"},"finish_reason":null}]}',
      '',
      'data: {"id":"chatcmpl_123","object":"chat.completion.chunk","model":"gpt-5.4","choices":[{"index":0,"delta":{"content":"Hello from stream"},"finish_reason":null}]}',
      '',
      'data: {"id":"chatcmpl_123","object":"chat.completion.chunk","model":"gpt-5.4","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}',
      '',
      'data: [DONE]',
      '',
    ].join('\n')

    const parsed = parseResponse(rawSse, requestBody, 'openai:chat')
    expect(parsed.apiFormat).toBe('openai')
    expect(parsed.isStream).toBe(true)
    expect(parsed.messages).toHaveLength(1)
    expect(parsed.messages[0]?.content[0]).toMatchObject({
      type: 'text',
      text: 'Hello from stream',
    })
  })

  it('renders raw OpenAI CLI SSE text from stored usage records', () => {
    const requestBody = {
      model: 'gpt-5.4',
      stream: true,
      input: 'Hello',
    }
    const rawSse = [
      'event: response.created',
      'data: {"type":"response.created","response":{"id":"resp_123","object":"response","model":"gpt-5.4","status":"in_progress"}}',
      '',
      'event: response.output_text.delta',
      'data: {"type":"response.output_text.delta","delta":"Hello from CLI stream"}',
      '',
      'event: response.completed',
      'data: {"type":"response.completed","response":{"id":"resp_123","object":"response","model":"gpt-5.4","status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Hello from CLI stream"}]}]}}',
      '',
      'data: [DONE]',
      '',
    ].join('\n')

    const rendered = renderResponse(rawSse, requestBody, 'openai:responses')
    expect(rendered.error).toBeUndefined()
    expect(rendered.isStream).toBe(true)
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
      content: 'Hello from CLI stream',
    })
  })

  it('renders legacy OpenAI CLI outtext delta alias from stored usage records', () => {
    const requestBody = {
      model: 'gpt-5.4',
      stream: true,
      input: 'Hello',
    }
    const rawSse = [
      'event: response.created',
      'data: {"type":"response.created","response":{"id":"resp_legacy_123","object":"response","model":"gpt-5.4","status":"in_progress"}}',
      '',
      'event: response.outtext.delta',
      'data: {"type":"response.outtext.delta","delta":"Hello from legacy alias"}',
      '',
      'event: response.completed',
      'data: {"type":"response.completed","response":{"id":"resp_legacy_123","object":"response","model":"gpt-5.4","status":"completed","output":[]}}',
      '',
      'data: [DONE]',
      '',
    ].join('\n')

    const rendered = renderResponse(rawSse, requestBody, 'openai:responses')
    expect(rendered.error).toBeUndefined()
    expect(rendered.isStream).toBe(true)
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
      content: 'Hello from legacy alias',
    })
  })

  it('renders HTML-entity encoded OpenAI tool arguments as formatted JSON', () => {
    const requestBody = {
      model: 'gpt-5.4',
      messages: [
        { role: 'user', content: 'Call a tool' },
      ],
    }
    const responseBody = {
      id: 'chatcmpl_tool_123',
      object: 'chat.completion',
      model: 'gpt-5.4',
      choices: [
        {
          index: 0,
          message: {
            role: 'assistant',
            content: null,
            tool_calls: [
              {
                id: 'call_123',
                type: 'function',
                function: {
                  name: 'skill',
                  arguments: '{&quot;name&quot;:&quot;hai-ai&quot;,&quot;user_message&quot;:&quot;A &amp; B &lt; C &gt; D &#39;ok&#39;&quot;}',
                },
              },
            ],
          },
          finish_reason: 'tool_calls',
        },
      ],
    }

    const rendered = renderResponse(responseBody, requestBody, 'openai:chat')
    expect(rendered.error).toBeUndefined()

    const firstBlock = rendered.blocks[0]
    if (!firstBlock || firstBlock.type !== 'message') {
      throw new Error('expected first render block to be message')
    }

    expect(firstBlock.content[0]).toMatchObject({
      type: 'tool_use',
      toolName: 'skill',
      input: JSON.stringify({
        name: 'hai-ai',
        user_message: "A & B < C > D 'ok'",
      }, null, 2),
    })
  })
})
