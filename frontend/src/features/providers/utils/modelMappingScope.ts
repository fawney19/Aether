import {
  API_FORMATS,
  formatApiFormat,
  normalizeApiFormatAlias,
} from '@/api/endpoints/types/api-format'

export const MODEL_MAPPING_OPERATION_COMPACT = 'compact'

export const ALL_REQUESTS_SCOPE_VALUE = '[]'
export const COMPACT_REQUEST_SCOPE_VALUE = JSON.stringify([
  MODEL_MAPPING_OPERATION_COMPACT,
])

export interface ModelMappingRequestScopeOption {
  value: string
  label: string
}

export interface ModelMappingRequestScopeCapabilities {
  sessionCompaction: boolean
}

export interface ModelMappingRequestScopeLabels {
  allRequests: string
  sessionCompactionOnly: string
  customOperations: (operations: string[]) => string
}

export interface ModelMappingEndpoint {
  id: string
  api_format: string
  base_url: string
  custom_path?: string
  is_active: boolean
}

const DEFAULT_REQUEST_SCOPE_LABELS: ModelMappingRequestScopeLabels = {
  allRequests: '所有请求',
  sessionCompactionOnly: '仅会话压缩',
  customOperations: operations => `仅匹配：${operations.join(', ')}`,
}

export function normalizeModelMappingOperations(
  operations: string[] | undefined,
): string[] {
  const seen = new Set<string>()
  const normalized: string[] = []
  for (const operation of operations ?? []) {
    const value = operation.trim().toLowerCase()
    if (!value || seen.has(value)) continue
    seen.add(value)
    normalized.push(value)
  }
  return normalized
}

export function modelMappingRequestScopeValue(
  operations: string[] | undefined,
): string {
  return JSON.stringify(normalizeModelMappingOperations(operations))
}

export function modelMappingOperationsKey(
  operations: string[] | undefined,
): string {
  return normalizeModelMappingOperations(operations).sort().join(',')
}

export function modelMappingOperationsFromScopeValue(
  value: string,
): string[] | undefined {
  try {
    const parsed = JSON.parse(value)
    if (!Array.isArray(parsed) || parsed.some(item => typeof item !== 'string')) {
      return undefined
    }
    const operations = normalizeModelMappingOperations(parsed)
    return operations.length > 0 ? operations : undefined
  } catch {
    return undefined
  }
}

export function formatModelMappingRequestScope(
  operations: string[] | undefined,
  labels: ModelMappingRequestScopeLabels = DEFAULT_REQUEST_SCOPE_LABELS,
): string {
  const normalized = normalizeModelMappingOperations(operations)
  if (normalized.length === 0) return labels.allRequests
  if (
    normalized.length === 1
    && normalized[0] === MODEL_MAPPING_OPERATION_COMPACT
  ) {
    return labels.sessionCompactionOnly
  }
  return labels.customOperations(normalized)
}

export function modelMappingRequestScopeOptions(
  operations: string[] | undefined,
  capabilities: ModelMappingRequestScopeCapabilities,
  labels: ModelMappingRequestScopeLabels = DEFAULT_REQUEST_SCOPE_LABELS,
): ModelMappingRequestScopeOption[] {
  const options: ModelMappingRequestScopeOption[] = [
    { value: ALL_REQUESTS_SCOPE_VALUE, label: labels.allRequests },
  ]
  if (capabilities.sessionCompaction) {
    options.push({
      value: COMPACT_REQUEST_SCOPE_VALUE,
      label: labels.sessionCompactionOnly,
    })
  }
  const currentValue = modelMappingRequestScopeValue(operations)
  const compactScopeUnavailable = currentValue === COMPACT_REQUEST_SCOPE_VALUE
    && !capabilities.sessionCompaction
  if (!compactScopeUnavailable && !options.some(option => option.value === currentValue)) {
    options.push({
      value: currentValue,
      label: formatModelMappingRequestScope(operations, labels),
    })
  }
  return options
}

export function modelMappingEndpointScopeSupportsSessionCompaction(
  endpointIds: string[] | undefined,
  endpoints: ModelMappingEndpoint[],
): boolean {
  const selectedIds = [...new Set(
    (endpointIds ?? []).map(id => id.trim()).filter(Boolean),
  )]
  if (selectedIds.length === 0) return false

  const endpointsById = new Map(endpoints.map(endpoint => [endpoint.id, endpoint]))
  return selectedIds.every((endpointId) => {
    const endpoint = endpointsById.get(endpointId)
    return endpoint !== undefined
      && normalizeApiFormatAlias(endpoint.api_format) === API_FORMATS.OPENAI_RESPONSES
  })
}

export function formatModelMappingEndpointLabel(
  endpoint: ModelMappingEndpoint,
  endpoints: ModelMappingEndpoint[],
): string {
  const sameFormatCount = endpoints.filter(item => item.api_format === endpoint.api_format).length
  const format = formatApiFormat(endpoint.api_format)
  const discriminator = sameFormatCount > 1
    ? formatModelMappingEndpointDiscriminator(endpoint)
    : ''
  const status = endpoint.is_active ? '' : '（停用）'
  return `${format}${discriminator ? ` · ${discriminator}` : ''}${status}`
}

function formatModelMappingEndpointDiscriminator(endpoint: ModelMappingEndpoint): string {
  const baseUrl = endpoint.base_url.trim()
  try {
    const parsed = new URL(baseUrl)
    const customPath = endpoint.custom_path?.trim()
    const path = (customPath || parsed.pathname).replace(/\/$/, '')
    return `${parsed.host}${path && path !== '/' ? path : ''}`
  } catch {
    return baseUrl || endpoint.id.slice(0, 8)
  }
}
