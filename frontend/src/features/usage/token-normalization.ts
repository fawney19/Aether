type UsageTokenLike = {
  effective_input_tokens?: number | null
  input_tokens?: number | null
  cache_creation_input_tokens?: number | null
  cache_creation_ephemeral_5m_input_tokens?: number | null
  cache_creation_ephemeral_1h_input_tokens?: number | null
  cache_read_input_tokens?: number | null
  api_format?: string | null
  endpoint_api_format?: string | null
}

function toNonNegativeNumber(value: number | null | undefined): number {
  return typeof value === 'number' && Number.isFinite(value) ? Math.max(value, 0) : 0
}

function apiFamily(apiFormat: string | null | undefined): string {
  return String(apiFormat || '')
    .split(':', 1)[0]
    .trim()
    .toLowerCase()
}

export function getCacheCreationTokens(usage: UsageTokenLike): number {
  const explicit = toNonNegativeNumber(usage.cache_creation_input_tokens)
  const classified = toNonNegativeNumber(usage.cache_creation_ephemeral_5m_input_tokens)
    + toNonNegativeNumber(usage.cache_creation_ephemeral_1h_input_tokens)
  if (explicit === 0 && classified > 0) {
    return classified
  }
  return explicit
}

export function getCacheReadTokens(usage: UsageTokenLike): number {
  return toNonNegativeNumber(usage.cache_read_input_tokens)
}

export function getEffectiveInputTokens(usage: UsageTokenLike): number {
  const explicit = toNonNegativeNumber(usage.effective_input_tokens)
  if (explicit > 0) {
    return explicit
  }

  const inputTokens = toNonNegativeNumber(usage.input_tokens)
  const cacheReadTokens = toNonNegativeNumber(usage.cache_read_input_tokens)
  if (inputTokens === 0 || cacheReadTokens === 0) {
    return inputTokens
  }

  switch (apiFamily(usage.endpoint_api_format || usage.api_format)) {
    case 'openai':
    case 'gemini':
    case 'google':
      return Math.max(inputTokens - cacheReadTokens, 0)
    default:
      return inputTokens
  }
}
