import { describe, expect, it } from 'vitest'

import {
  buildDefaultModelTestRequestBody,
  buildExactModelMappingTestRequest,
  extractModelTestImagePreviews,
  extractModelTestResponsePreview,
  formatModelTestDiagnostic,
  getOpenAiImageModelTestMaxGenerationCount,
  isModelTestableEndpoint,
  isModelTestableApiFormat,
  listModelTestMappedModelOptions,
  normalizeModelTestMappedModelSelection,
  selectPreferredModelTestEndpoint,
  setModelTestRequestBodyModel,
  syncModelTestRequestBodyDraft,
} from '../model-test-request'

describe('buildDefaultModelTestRequestBody', () => {
  it.each([
    'openai:embedding',
    'gemini:embedding',
    'jina:embedding',
    'doubao:embedding',
    '  OPENAI:EMBEDDING  ',
  ])('uses embedding input payloads for %s api formats', (apiFormat) => {
    const body = JSON.parse(buildDefaultModelTestRequestBody('text-embedding-3-small', apiFormat))

    expect(body).toEqual({
      model: 'text-embedding-3-small',
      input: 'This is a test embedding input.',
    })
    expect(body.messages).toBeUndefined()
    expect(body.stream).toBeUndefined()
  })

  it.each([
    'openai:rerank',
    'jina:rerank',
    '  JINA:RERANK  ',
  ])('uses rerank query/documents payloads for %s api formats', (apiFormat) => {
    const body = JSON.parse(buildDefaultModelTestRequestBody('bge-reranker-base', apiFormat))

    expect(body.model).toBe('bge-reranker-base')
    expect(body.query).toBe('Apple')
    expect(body.documents).toEqual(['apple', 'banana', 'fruit', 'vegetable'])
    expect(body.return_documents).toBe(true)
    expect(body.top_n).toBe(4)
    expect(body.messages).toBeUndefined()
    expect(body.stream).toBeUndefined()
  })

  it('keeps chat payloads for chat api formats', () => {
    const body = JSON.parse(buildDefaultModelTestRequestBody('gpt-5.1', 'openai:chat'))

    expect(body.messages).toEqual([{ role: 'user', content: 'Hello! This is a test message.' }])
    expect(body.stream).toBe(true)
    expect(body.input).toBeUndefined()
  })

  it('uses image prompt payloads for OpenAI image test requests', () => {
    const body = JSON.parse(buildDefaultModelTestRequestBody('gpt-image-2', 'openai:image'))

    expect(body).toEqual({
      model: 'gpt-image-2',
      prompt: 'Hello! This is a test message.',
      n: 1,
      size: '1024x1024',
      stream: true,
    })
    expect(body.messages).toBeUndefined()
  })

  it('uses image generation tools for image models on OpenAI Responses endpoints', () => {
    const body = JSON.parse(buildDefaultModelTestRequestBody(
      'gpt-image-2',
      'openai:responses',
      {
        effective_supports_image_generation: true,
      },
    ))

    expect(body.model).toBe('gpt-image-2')
    expect(body.input).toBe('Hello! This is a test message.')
    expect(body.tools).toEqual([
      {
        type: 'image_generation',
        size: '1024x1024',
        output_format: 'png',
      },
    ])
    expect(body.tool_choice).toEqual({ type: 'image_generation' })
    expect(body.messages).toBeUndefined()
  })

  it('lists endpoint-scoped provider model mappings in test selection order', () => {
    const options = listModelTestMappedModelOptions({
      provider_model_name: 'claude-opus-4-6',
      provider_model_mappings: [
        {
          name: 'MiniMax-M2.7-balanced',
          priority: 3,
          api_formats: ['openai:chat'],
          endpoint_ids: ['endpoint-minimax-chat'],
        },
        {
          name: 'MiniMax-M2.7-highspeed',
          priority: 2,
          api_formats: ['OPENAI'],
          endpoint_ids: ['endpoint-minimax-chat'],
        },
        {
          name: 'ignored-anthropic-model',
          priority: 1,
          api_formats: ['anthropic:messages'],
        },
        {
          name: 'MiniMax-M2.7-highspeed',
          priority: 4,
          api_formats: ['openai:chat'],
          endpoint_ids: ['endpoint-minimax-chat'],
        },
      ],
    }, {
      id: 'endpoint-minimax-chat',
      api_format: 'openai:chat',
    })

    expect(options).toEqual([
      { name: 'MiniMax-M2.7-highspeed', priority: 2 },
      { name: 'MiniMax-M2.7-balanced', priority: 3 },
    ])
  })

  it('does not select a provider model mapping outside endpoint scope', () => {
    const options = listModelTestMappedModelOptions({
      provider_model_name: 'claude-opus-4-6',
      provider_model_mappings: [
        {
          name: 'MiniMax-M2.7-highspeed',
          priority: 1,
          api_formats: ['openai:chat'],
          endpoint_ids: ['another-endpoint'],
        },
      ],
    }, {
      id: 'endpoint-minimax-chat',
      api_format: 'openai:chat',
    })

    expect(options).toEqual([])
  })

  it('keeps the current model selected by default until a mapped model is chosen', () => {
    const options = [
      { name: 'MiniMax-M2.7-highspeed', priority: 1 },
      { name: 'MiniMax-M2.7-balanced', priority: 2 },
    ]

    expect(normalizeModelTestMappedModelSelection(options, null)).toBeNull()
    expect(normalizeModelTestMappedModelSelection(options, '')).toBeNull()
    expect(normalizeModelTestMappedModelSelection(options, 'MiniMax-M2.7-balanced')).toBe('MiniMax-M2.7-balanced')
    expect(normalizeModelTestMappedModelSelection(options, 'another-model')).toBeNull()
  })

  it('updates only the request body model when model mapping is toggled', () => {
    const draft = buildDefaultModelTestRequestBody('claude-opus-4-6', 'openai:chat')
    const body = JSON.parse(setModelTestRequestBodyModel(draft, 'MiniMax-M2.7-highspeed'))

    expect(body.model).toBe('MiniMax-M2.7-highspeed')
    expect(body.messages).toEqual([{ role: 'user', content: 'Hello! This is a test message.' }])
    expect(body.stream).toBe(true)
  })

  it('updates the draft to the next endpoint default when the user has not edited it', () => {
    const previous = buildDefaultModelTestRequestBody('chat-model', 'openai:chat')
    const nextDefault = buildDefaultModelTestRequestBody('chat-model', 'openai:embedding')
    const synced = syncModelTestRequestBodyDraft(previous, previous, nextDefault, 'embedding-model')

    expect(JSON.parse(synced.draft)).toEqual({
      model: 'embedding-model',
      input: 'This is a test embedding input.',
    })
    expect(synced.resetValue).toBe(synced.draft)
  })

  it('preserves edited request bodies when switching endpoints', () => {
    const previous = buildDefaultModelTestRequestBody('chat-model', 'openai:chat')
    const edited = JSON.stringify({
      model: 'chat-model',
      messages: [{ role: 'user', content: 'custom prompt' }],
      max_tokens: 128,
      temperature: 0.2,
      stream: true,
    }, null, 2)
    const nextDefault = buildDefaultModelTestRequestBody('chat-model', 'openai:embedding')
    const synced = syncModelTestRequestBodyDraft(edited, previous, nextDefault, 'embedding-model')
    const body = JSON.parse(synced.draft)

    expect(body).toEqual({
      model: 'embedding-model',
      messages: [{ role: 'user', content: 'custom prompt' }],
      max_tokens: 128,
      temperature: 0.2,
      stream: true,
    })
    expect(JSON.parse(synced.resetValue)).toEqual({
      model: 'embedding-model',
      input: 'This is a test embedding input.',
    })
  })
})

describe('buildExactModelMappingTestRequest', () => {
  it('tests the clicked mapping name without applying another provider mapping', () => {
    expect(buildExactModelMappingTestRequest(
      'provider-1',
      'MiniMax-M2.7-balanced',
      'openai:chat',
    )).toEqual({
      provider_id: 'provider-1',
      model_name: 'MiniMax-M2.7-balanced',
      mode: 'direct',
      apply_model_mapping: false,
      api_format: 'openai:chat',
    })
  })
})

describe('isModelTestableApiFormat', () => {
  it.each([
    'openai:video',
    'gemini:video',
    'gemini:files',
    '  OPENAI:VIDEO  ',
  ])('excludes task and file endpoint formats from model tests: %s', (apiFormat) => {
    expect(isModelTestableApiFormat(apiFormat)).toBe(false)
  })

  it.each([
    'openai:chat',
    'openai:responses',
    'openai:responses:compact',
    'openai:image',
    'claude:messages',
    'gemini:generate_content',
    'openai:image',
    'openai:embedding',
    'jina:rerank',
  ])('allows synchronous model-test endpoint formats: %s', (apiFormat) => {
    expect(isModelTestableApiFormat(apiFormat)).toBe(true)
  })
})

describe('selectPreferredModelTestEndpoint', () => {
  it('prefers openai image endpoints for image generation models', () => {
    const chatEndpoint = { id: 'chat', api_format: 'openai:chat', is_active: true }
    const imageEndpoint = { id: 'image', api_format: 'openai:image', is_active: true }

    expect(selectPreferredModelTestEndpoint({
      effective_supports_image_generation: true,
    }, [chatEndpoint, imageEndpoint])).toBe(imageEndpoint)
  })

  it('prefers openai image endpoints from model-test capability metadata', () => {
    const chatEndpoint = { id: 'chat', api_format: 'openai:chat', is_active: true }
    const imageEndpoint = { id: 'image', api_format: 'openai:image', is_active: true }

    expect(selectPreferredModelTestEndpoint({
      effective_supports_image_generation: false,
      model_test_capabilities: {
        'openai:image': {
          supports_generation: true,
          max_generation_count: 4,
        },
      },
    }, [chatEndpoint, imageEndpoint])).toBe(imageEndpoint)
  })

  it('does not treat edit-only image capability as generation support', () => {
    const chatEndpoint = { id: 'chat', api_format: 'openai:chat', is_active: true }
    const imageEndpoint = { id: 'image', api_format: 'openai:image', is_active: true }

    expect(selectPreferredModelTestEndpoint({
      effective_supports_image_generation: true,
      model_test_capabilities: {
        'openai:image': {
          supports_generation: false,
          supports_edit: true,
          max_generation_count: 4,
        },
      },
    }, [chatEndpoint, imageEndpoint])).toBe(chatEndpoint)
  })

  it('keeps the existing endpoint order for non-image models', () => {
    const chatEndpoint = { id: 'chat', api_format: 'openai:chat', is_active: true }
    const imageEndpoint = { id: 'image', api_format: 'openai:image', is_active: true }

    expect(selectPreferredModelTestEndpoint({
      effective_supports_image_generation: false,
    }, [chatEndpoint, imageEndpoint])).toBe(chatEndpoint)
  })
})

describe('getOpenAiImageModelTestMaxGenerationCount', () => {
  it('reads image generation count from backend capability metadata', () => {
    expect(getOpenAiImageModelTestMaxGenerationCount({
      model_test_capabilities: {
        'openai:image': {
          max_generation_count: 4,
        },
      },
    })).toBe(4)
  })

  it('returns null when backend capability metadata is absent', () => {
    expect(getOpenAiImageModelTestMaxGenerationCount({
      effective_supports_image_generation: true,
    })).toBeNull()
  })
})

describe('isModelTestableEndpoint', () => {
  it('requires at least one active key compatible with the endpoint format', () => {
    const keys = [
      {
        api_formats: ['openai:chat'],
        is_active: true,
      },
      {
        api_formats: ['claude:messages'],
        is_active: false,
      },
    ]

    expect(isModelTestableEndpoint({
      api_format: 'openai:chat',
      is_active: true,
    }, keys)).toBe(true)
    expect(isModelTestableEndpoint({
      api_format: 'claude:messages',
      is_active: true,
    }, keys)).toBe(false)
  })

  it('treats an active key without explicit api formats as compatible with all testable endpoints', () => {
    const keys = [{ api_formats: [], is_active: true }]

    expect(isModelTestableEndpoint({
      api_format: 'openai:responses',
      is_active: true,
    }, keys)).toBe(true)
  })

  it('lets fixed provider OAuth keys inherit testable endpoint formats', () => {
    const keys = [{
      api_formats: ['legacy:mismatch'],
      auth_type: 'oauth',
      is_active: true,
    }]

    expect(isModelTestableEndpoint({
      api_format: 'openai:image',
      is_active: true,
    }, keys, 'chatgpt_web')).toBe(true)
    expect(isModelTestableEndpoint({
      api_format: 'openai:image',
      is_active: true,
    }, keys, 'custom')).toBe(false)
  })
})

describe('formatModelTestDiagnostic', () => {
  it('maps pool account blocked scheduler code to an actionable label', () => {
    expect(formatModelTestDiagnostic('pool_account_blocked')).toBe('账号已失效，需重新授权')
  })

  it('keeps unknown diagnostics unchanged', () => {
    expect(formatModelTestDiagnostic('provider auth is unavailable')).toBe('provider auth is unavailable')
  })
})

describe('extractModelTestResponsePreview', () => {
  it('extracts assistant text from Claude Messages response content', () => {
    expect(extractModelTestResponsePreview({
      id: 'msg_1',
      content: [
        {
          type: 'text',
          text: 'Hello! This is a test message.\n\nTest message received.',
        },
      ],
    })).toBe('Hello! This is a test message. Test message received.')
  })

  it('extracts assistant text from OpenAI Responses output content', () => {
    expect(extractModelTestResponsePreview({
      output: [
        {
          type: 'message',
          content: [
            {
              type: 'output_text',
              text: 'Response API test passed.',
            },
          ],
        },
      ],
    })).toBe('Response API test passed.')
  })

  it('extracts assistant text from Gemini candidates', () => {
    expect(extractModelTestResponsePreview({
      candidates: [
        {
          content: {
            parts: [
              { text: 'Gemini test passed.' },
            ],
          },
        },
      ],
    })).toBe('Gemini test passed.')
  })

  it('uses OpenAI reasoning content when answer text is empty', () => {
    expect(extractModelTestResponsePreview({
      model: 'deepseek-v4-pro',
      choices: [
        {
          message: {
            role: 'assistant',
            content: '',
            reasoning_content: 'We are asked: "Hello! This is a test message." This is just a greeting.',
          },
          finish_reason: 'length',
        },
      ],
    })).toBe('推理：We are asked: "Hello! This is a test message." This is just a greeting.')
  })

  it('uses Claude thinking content when no text content is present', () => {
    expect(extractModelTestResponsePreview({
      model: 'deepseek-v4-pro',
      content: [
        {
          type: 'thinking',
          thinking: 'We are given a simple test message: "Hello! This is a test message."',
        },
      ],
      stop_reason: 'max_tokens',
    })).toBe('推理：We are given a simple test message: "Hello! This is a test message."')
  })

  it('summarizes embedding and rerank responses without assistant text', () => {
    expect(extractModelTestResponsePreview({
      data: [
        { embedding: [0.1, 0.2, 0.3] },
      ],
    })).toBe('Embedding 维度：3')

    expect(extractModelTestResponsePreview({
      results: [
        { index: 0, relevance_score: 0.9 },
        { index: 1, relevance_score: 0.5 },
      ],
    })).toBe('Rerank 结果：2 条')
  })

  it('extracts image URLs from OpenAI image responses', () => {
    expect(extractModelTestResponsePreview({
      data: [
        {
          url: 'https://example.com/generated.png',
          revised_prompt: 'A generated image',
        },
      ],
    })).toBe('图片：https://example.com/generated.png')
  })

  it('extracts renderable URL image previews from OpenAI image responses', () => {
    expect(extractModelTestImagePreviews({
      data: [
        {
          url: 'https://example.com/generated.png',
          revised_prompt: 'A generated image',
        },
      ],
    })).toEqual([
      {
        src: 'https://example.com/generated.png',
        label: '图片 1',
        source: 'url',
      },
    ])
  })

  it('summarizes base64 image responses without dumping the image payload', () => {
    expect(extractModelTestResponsePreview({
      data: [
        {
          b64_json: 'aGVsbG8=',
        },
      ],
    })).toBe('图片：base64')
  })

  it('extracts renderable base64 image previews from OpenAI image responses', () => {
    expect(extractModelTestImagePreviews({
      data: [
        {
          b64_json: 'aGVsbG8=',
          mime_type: 'image/jpeg',
        },
      ],
    })).toEqual([
      {
        src: 'data:image/jpeg;base64,aGVsbG8=',
        label: '图片 1',
        source: 'base64',
      },
    ])
  })

  it('accepts root-relative URLs in OpenAI image responses', () => {
    expect(extractModelTestImagePreviews({
      data: [
        {
          url: '/v1/files/image?id=2af14311-a0cb-4bbf-ae20-d1fcf44e0479',
        },
      ],
    })).toEqual([
      {
        src: '/v1/files/image?id=2af14311-a0cb-4bbf-ae20-d1fcf44e0479',
        label: '图片 1',
        source: 'url',
      },
    ])
  })

  it('parses stringified JSON image responses', () => {
    expect(extractModelTestImagePreviews(JSON.stringify({
      data: [
        {
          url: 'https://example.com/generated.png',
        },
      ],
    }))).toEqual([
      {
        src: 'https://example.com/generated.png',
        label: '图片 1',
        source: 'url',
      },
    ])
  })

  it('parses stringified JSON base64 image responses', () => {
    expect(extractModelTestImagePreviews(JSON.stringify({
      data: [
        {
          b64_json: 'aGVsbG8=',
          mime_type: 'image/jpeg',
        },
      ],
    }))).toEqual([
      {
        src: 'data:image/jpeg;base64,aGVsbG8=',
        label: '图片 1',
        source: 'base64',
      },
    ])
  })

  it('extracts image previews from nested response image urls', () => {
    expect(extractModelTestImagePreviews({
      output: [
        {
          content: [
            {
              type: 'output_image',
              image_url: {
                url: 'https://example.com/generated.png',
              },
            },
          ],
        },
      ],
    })).toEqual([
      {
        src: 'https://example.com/generated.png',
        label: '图片 1',
        source: 'url',
      },
    ])
  })

  it('extracts image previews from OpenAI Responses image_generation_call results', () => {
    expect(extractModelTestResponsePreview({
      output: [
        {
          type: 'image_generation_call',
          output_format: 'png',
          result: 'aGVsbG8=',
        },
      ],
    })).toBe('图片：base64')

    expect(extractModelTestImagePreviews({
      output: [
        {
          type: 'image_generation_call',
          output_format: 'png',
          result: 'aGVsbG8=',
        },
      ],
    })).toEqual([
      {
        src: 'data:image/png;base64,aGVsbG8=',
        label: '图片 1',
        source: 'base64',
      },
    ])
  })

  it('falls back to response model when no text payload exists', () => {
    expect(extractModelTestResponsePreview({
      model: 'glm-4.5-air',
    })).toBe('返回模型：glm-4.5-air')
  })
})
