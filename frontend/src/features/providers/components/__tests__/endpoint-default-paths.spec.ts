import { describe, expect, it } from 'vitest'

import { getDefaultEndpointBaseUrl, getDefaultEndpointPath } from '../endpoint-default-paths'

const apiFormats = [
  { value: 'openai:chat', default_path: '/v1/chat/completions' },
  { value: 'gemini:generate_content', default_path: '/v1beta/models/{model}:{action}' },
  { value: 'gemini:embedding', default_path: '/v1beta/models/{model}:embedContent' },
  { value: 'gemini:video', default_path: '/v1beta/models/{model}:predictLongRunning' },
  { value: 'openai:responses', default_path: '/v1/responses' },
  { value: 'openai:embedding', default_path: '/v1/embeddings' },
  { value: 'openai:rerank', default_path: '/v1/rerank' },
  { value: 'openai:image', default_path: '/v1/images/generations' },
  { value: 'openai:video', default_path: '/v1/videos' },
  { value: 'jina:embedding', default_path: '/v1/embeddings' },
  { value: 'jina:rerank', default_path: '/v1/rerank' },
  { value: 'claude:messages', default_path: '/v1/messages' },
]

describe('endpoint default paths', () => {
  it('uses Gemini Developer API resource paths for custom Gemini endpoints', () => {
    expect(getDefaultEndpointPath({
      apiFormat: 'gemini:generate_content',
      providerType: 'custom',
      apiFormats,
    })).toBe('/models/{model}:{action}')

    expect(getDefaultEndpointPath({
      apiFormat: 'gemini:embedding',
      providerType: 'custom',
      apiFormats,
    })).toBe('/models/{model}:embedContent')

    expect(getDefaultEndpointPath({
      apiFormat: 'gemini:video',
      providerType: 'custom',
      apiFormats,
    })).toBe('/models/{model}:predictLongRunning')
  })

  it('uses Vertex AI project/location paths for Vertex provider Gemini endpoints', () => {
    expect(getDefaultEndpointPath({
      apiFormat: 'gemini:generate_content',
      providerType: 'vertex_ai',
      apiFormats,
    })).toBe('/v1/projects/{project_id}/locations/{region}/publishers/google/models/{model}:{action}')

    expect(getDefaultEndpointPath({
      apiFormat: 'gemini:embedding',
      providerType: 'vertex_ai',
      apiFormats,
    })).toBe('/v1/projects/{project_id}/locations/{region}/publishers/google/models/{model}:predict')
  })

  it('uses Gemini CLI v1internal paths for fixed Gemini CLI endpoints', () => {
    expect(getDefaultEndpointPath({
      apiFormat: 'gemini:generate_content',
      providerType: 'gemini_cli',
      apiFormats,
    })).toBe('/v1internal:{action}')
  })

  it('keeps Codex Responses root path without duplicating /v1', () => {
    expect(getDefaultEndpointPath({
      apiFormat: 'openai:responses',
      providerType: 'codex',
      apiFormats,
    })).toBe('/responses')
  })

  it('drops /v1 from API-root defaults because base URL is the API root', () => {
    expect(getDefaultEndpointPath({
      apiFormat: 'openai:chat',
      providerType: 'custom',
      baseUrl: 'https://proxy.example.com/api',
      apiFormats,
    })).toBe('/chat/completions')

    expect(getDefaultEndpointPath({
      apiFormat: 'openai:embedding',
      providerType: 'custom',
      baseUrl: 'https://proxy.example.com/api?tenant=demo',
      apiFormats,
    })).toBe('/embeddings')

    expect(getDefaultEndpointPath({
      apiFormat: 'openai:rerank',
      providerType: 'custom',
      baseUrl: 'https://proxy.example.com/api?tenant=demo',
      apiFormats,
    })).toBe('/rerank')

    expect(getDefaultEndpointPath({
      apiFormat: 'openai:image',
      providerType: 'custom',
      baseUrl: 'https://proxy.example.com/api',
      apiFormats,
    })).toBe('/images/generations')

    expect(getDefaultEndpointPath({
      apiFormat: 'openai:video',
      providerType: 'custom',
      baseUrl: 'https://proxy.example.com/api',
      apiFormats,
    })).toBe('/videos')

    expect(getDefaultEndpointPath({
      apiFormat: 'jina:embedding',
      providerType: 'custom',
      baseUrl: 'https://api.jina.ai/v1',
      apiFormats,
    })).toBe('/embeddings')

    expect(getDefaultEndpointPath({
      apiFormat: 'jina:rerank',
      providerType: 'custom',
      baseUrl: 'https://api.jina.ai/v1',
      apiFormats,
    })).toBe('/rerank')

    expect(getDefaultEndpointPath({
      apiFormat: 'openai:chat',
      providerType: 'custom',
      baseUrl: 'https://proxy.example.com/openai',
      apiFormats,
    })).toBe('/chat/completions')

    expect(getDefaultEndpointPath({
      apiFormat: 'openai:chat',
      providerType: 'custom',
      baseUrl: 'https://proxy.example.com',
      apiFormats,
    })).toBe('/chat/completions')
  })

  it('drops /v1 from OpenAI-compatible defaults when base URL already includes a known API root', () => {
    expect(getDefaultEndpointPath({
      apiFormat: 'openai:chat',
      providerType: 'custom',
      baseUrl: 'https://open.bigmodel.cn/api/coding/paas/v4',
      apiFormats,
    })).toBe('/chat/completions')

    expect(getDefaultEndpointPath({
      apiFormat: 'openai:responses',
      providerType: 'custom',
      baseUrl: 'https://api.openai.example/v1',
      apiFormats,
    })).toBe('/responses')
  })

  it('drops /v1 from Claude Messages defaults because base URL is the API root', () => {
    expect(getDefaultEndpointPath({
      apiFormat: 'claude:messages',
      providerType: 'custom',
      baseUrl: 'https://api.anthropic.example/v1',
      apiFormats,
    })).toBe('/messages')

    expect(getDefaultEndpointPath({
      apiFormat: 'claude:messages',
      providerType: 'custom',
      baseUrl: 'https://proxy.example.com/api',
      apiFormats,
    })).toBe('/messages')

    expect(getDefaultEndpointPath({
      apiFormat: 'claude:messages',
      providerType: 'custom',
      baseUrl: 'https://proxy.example.com/anthropic',
      apiFormats,
    })).toBe('/messages')
  })

  it('defaults API-root base URLs to the format version when using a provider website', () => {
    expect(getDefaultEndpointBaseUrl({
      apiFormat: 'openai:chat',
      baseUrl: 'https://api.openai.com',
    })).toBe('https://api.openai.com/v1')

    expect(getDefaultEndpointBaseUrl({
      apiFormat: 'openai:responses',
      baseUrl: 'https://dashscope.aliyuncs.com/compatible-mode',
    })).toBe('https://dashscope.aliyuncs.com/compatible-mode/v1')

    expect(getDefaultEndpointBaseUrl({
      apiFormat: 'claude:messages',
      baseUrl: 'https://api.anthropic.com',
    })).toBe('https://api.anthropic.com/v1')

    expect(getDefaultEndpointBaseUrl({
      apiFormat: 'openai:embedding',
      baseUrl: 'https://api.openai.com',
    })).toBe('https://api.openai.com/v1')

    expect(getDefaultEndpointBaseUrl({
      apiFormat: 'openai:image',
      baseUrl: 'https://api.openai.com',
    })).toBe('https://api.openai.com/v1')

    expect(getDefaultEndpointBaseUrl({
      apiFormat: 'openai:video',
      baseUrl: 'https://api.openai.com',
    })).toBe('https://api.openai.com/v1')

    expect(getDefaultEndpointBaseUrl({
      apiFormat: 'jina:embedding',
      baseUrl: 'https://api.jina.ai',
    })).toBe('https://api.jina.ai/v1')

    expect(getDefaultEndpointBaseUrl({
      apiFormat: 'gemini:generate_content',
      baseUrl: 'https://generativelanguage.googleapis.com',
    })).toBe('https://generativelanguage.googleapis.com/v1beta')

    expect(getDefaultEndpointBaseUrl({
      apiFormat: 'gemini:embedding',
      baseUrl: 'https://generativelanguage.googleapis.com',
    })).toBe('https://generativelanguage.googleapis.com/v1beta')

    expect(getDefaultEndpointBaseUrl({
      apiFormat: 'gemini:video',
      baseUrl: 'https://generativelanguage.googleapis.com',
    })).toBe('https://generativelanguage.googleapis.com/v1beta')

    expect(getDefaultEndpointBaseUrl({
      apiFormat: 'openai:chat',
      baseUrl: 'https://open.bigmodel.cn/api/coding/paas/v4',
    })).toBe('https://open.bigmodel.cn/api/coding/paas/v4')

    expect(getDefaultEndpointBaseUrl({
      apiFormat: 'openai:chat',
      baseUrl: 'https://generativelanguage.googleapis.com/v1beta/openai',
    })).toBe('https://generativelanguage.googleapis.com/v1beta/openai')

    expect(getDefaultEndpointBaseUrl({
      apiFormat: 'openai:chat',
      baseUrl: 'https://api.deepseek.com',
    })).toBe('https://api.deepseek.com')
  })
})
