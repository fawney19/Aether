import {
  apiFormatPermissionCovers,
  normalizeApiFormatAlias,
} from '@/api/endpoints/types/api-format'
import type { ModelTestCapabilities, OpenAiImageModelTestCapability } from '@/api/endpoints/types'

export type ModelTestEndpointSource = {
  api_format: string
  is_active?: boolean | null
}

export type ModelTestImageSource = {
  effective_supports_image_generation?: boolean | null
  supports_image_generation?: boolean | null
  model_test_capabilities?: ModelTestCapabilities | null
}

export type ModelTestKeySource = {
  api_formats?: string[] | null
  is_active?: boolean | null
  auth_type?: string | null
  credential_kind?: string | null
  oauth_managed?: boolean | null
}

const MODEL_TEST_UNSUPPORTED_API_FORMATS = new Set([
  'openai:video',
  'gemini:video',
  'gemini:files',
])

const MODEL_TEST_OAUTH_INHERITS_PROVIDER_FORMATS = new Set([
  'claude_code',
  'codex',
  'chatgpt_web',
  'gemini_cli',
  'vertex_ai',
  'antigravity',
  'kiro',
])

const MODEL_TEST_BEARER_INHERITS_PROVIDER_FORMATS = new Set([
  'chatgpt_web',
])

const MODEL_TEST_DIAGNOSTIC_LABELS: Record<string, string> = {
  key_model_not_allowed: 'Key 未允许当前模型，已跳过',
  pool_account_blocked: '账号已失效，需重新授权',
}

export function normalizeModelTestStringList(values: string[] | null | undefined): string[] {
  return (values ?? [])
    .map(value => value.trim())
    .filter(Boolean)
}

export function isModelTestableApiFormat(apiFormat: string | null | undefined): boolean {
  const normalized = normalizeApiFormatAlias(apiFormat ?? '')
  return Boolean(normalized) && !MODEL_TEST_UNSUPPORTED_API_FORMATS.has(normalized)
}

export function modelTestKeySupportsEndpoint(
  key: ModelTestKeySource,
  endpoint: ModelTestEndpointSource,
  providerType?: string | null,
): boolean {
  if (key.is_active === false) return false

  const endpointFormat = normalizeApiFormatAlias(endpoint.api_format)
  if (!isModelTestableApiFormat(endpointFormat)) return false

  if (modelTestKeyInheritsProviderFormats(key, providerType)) return true

  const keyFormats = normalizeModelTestStringList(key.api_formats)
  if (keyFormats.length === 0) return true

  return keyFormats.some(format => apiFormatPermissionCovers(format, endpointFormat))
}

export function isModelTestableEndpoint(
  endpoint: ModelTestEndpointSource,
  keys: ModelTestKeySource[],
  providerType?: string | null,
): boolean {
  return endpoint.is_active !== false
    && isModelTestableApiFormat(endpoint.api_format)
    && keys.some(key => modelTestKeySupportsEndpoint(key, endpoint, providerType))
}

function modelTestKeyInheritsProviderFormats(
  key: ModelTestKeySource,
  providerType: string | null | undefined,
): boolean {
  const normalizedProviderType = providerType?.trim().toLowerCase()
  if (!normalizedProviderType) return false

  const authType = key.auth_type?.trim().toLowerCase()
  const credentialKind = key.credential_kind?.trim().toLowerCase()
  const oauthManaged = key.oauth_managed === true
    || credentialKind === 'oauth_session'
    || authType === 'oauth'

  if (oauthManaged && MODEL_TEST_OAUTH_INHERITS_PROVIDER_FORMATS.has(normalizedProviderType)) {
    return true
  }

  return authType === 'bearer'
    && MODEL_TEST_BEARER_INHERITS_PROVIDER_FORMATS.has(normalizedProviderType)
}

export function selectPreferredModelTestEndpoint<T extends ModelTestEndpointSource>(
  model: ModelTestImageSource | null | undefined,
  endpoints: T[],
): T | null {
  if (modelSupportsImageGeneration(model)) {
    const imageEndpoint = endpoints.find(
      endpoint => normalizeApiFormatAlias(endpoint.api_format) === 'openai:image',
    )
    if (imageEndpoint) return imageEndpoint
  }

  return endpoints[0] ?? null
}

export function getOpenAiImageModelTestCapability(
  model: ModelTestImageSource | null | undefined,
): OpenAiImageModelTestCapability | null {
  const capability = model?.model_test_capabilities?.['openai:image']
  return capability && typeof capability === 'object'
    ? capability as OpenAiImageModelTestCapability
    : null
}

export function getOpenAiImageModelTestMaxGenerationCount(
  model: ModelTestImageSource | null | undefined,
): number | null {
  const maxGenerationCount = getOpenAiImageModelTestCapability(model)?.max_generation_count
  return typeof maxGenerationCount === 'number' && Number.isFinite(maxGenerationCount)
    ? Math.max(1, Math.floor(maxGenerationCount))
    : null
}

export function formatModelTestDiagnostic(value: string | null | undefined): string {
  const normalized = value?.trim()
  if (!normalized) return ''
  return MODEL_TEST_DIAGNOSTIC_LABELS[normalized] ?? normalized
}

export function modelSupportsImageGeneration(model: ModelTestImageSource | null | undefined): boolean {
  const imageCapability = getOpenAiImageModelTestCapability(model)
  if (imageCapability) {
    return imageCapability.supports_generation !== false
  }
  return Boolean(
    model?.effective_supports_image_generation ?? model?.supports_image_generation,
  )
}
