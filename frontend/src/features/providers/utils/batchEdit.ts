import type { BodyRule, HeaderRule, ProviderUpdatePayload } from '@/api/endpoints'

export type BatchMergeMode = 'append' | 'overwrite'
export type UpstreamStreamPolicy = 'auto' | 'force_stream' | 'force_non_stream'

export interface EndpointBatchState {
  proxy: { include: boolean; nodeId: string }
  formatConversion: { include: boolean; enabled: boolean }
  upstreamPolicy: { include: boolean; value: UpstreamStreamPolicy }
  active: { include: boolean; value: boolean }
  rules: { include: boolean; mode: BatchMergeMode; json: string; error: string | null }
}

export interface ProviderBasicBatchState {
  maxRetries: { include: boolean; value: number | undefined }
  streamFirstByteTimeout: { include: boolean; value: number | undefined }
  requestTimeout: { include: boolean; value: number | undefined }
  keepPriorityOnConversion: { include: boolean; value: boolean }
}

export interface BatchProviderOptionSource {
  id: string
  name: string
  provider_type?: string | null
  is_active?: boolean
}

export interface BatchEndpointLite {
  id: string
  api_format: string
  config?: Record<string, unknown> | null
  header_rules?: HeaderRule[] | null
  body_rules?: BodyRule[] | null
}

export interface BatchProviderEndpoints {
  id: string
  name: string
  endpoints: BatchEndpointLite[]
}

export interface EndpointBatchTarget {
  providerId: string
  providerName: string
  endpointId: string
  apiFormat: string
}

export interface BatchSkippedProvider {
  id: string
  name: string
  reason: 'no_matching_endpoint'
}

export interface EndpointTargetResult {
  targets: EndpointBatchTarget[]
  matchedProviderCount: number
  skippedProviders: BatchSkippedProvider[]
}

export function toProviderOptions(providers: BatchProviderOptionSource[]): { value: string; label: string }[] {
  return providers.map(provider => ({
    value: provider.id,
    label: `${provider.name} · ${provider.provider_type || 'custom'} · ${provider.is_active ? '活跃' : '停用'}`,
  }))
}

export function createDefaultProviderBasicState(): ProviderBasicBatchState {
  return {
    maxRetries: { include: false, value: undefined },
    streamFirstByteTimeout: { include: false, value: undefined },
    requestTimeout: { include: false, value: undefined },
    keepPriorityOnConversion: { include: false, value: false },
  }
}

export function hasProviderBasicPatch(state: ProviderBasicBatchState): boolean {
  return state.maxRetries.include
    || state.streamFirstByteTimeout.include
    || state.requestTimeout.include
    || state.keepPriorityOnConversion.include
}

export function validateProviderBasicState(state: ProviderBasicBatchState): string | null {
  if (state.maxRetries.include && state.maxRetries.value === undefined) {
    return '最大重试次数不能为空'
  }
  return null
}

export function getProviderBasicFieldLabels(state: ProviderBasicBatchState): string[] {
  const labels: string[] = []
  if (state.maxRetries.include) labels.push('最大重试次数')
  if (state.streamFirstByteTimeout.include) labels.push('流式首字节超时')
  if (state.requestTimeout.include) labels.push('非流式请求超时')
  if (state.keepPriorityOnConversion.include) labels.push('格式转换保持优先级')
  return labels
}

export function buildProviderBasicPatch(state: ProviderBasicBatchState): ProviderUpdatePayload {
  const patch: ProviderUpdatePayload = {}
  if (state.maxRetries.include && state.maxRetries.value !== undefined) {
    patch.max_retries = state.maxRetries.value
  }
  if (state.streamFirstByteTimeout.include) {
    patch.stream_first_byte_timeout = state.streamFirstByteTimeout.value ?? null
  }
  if (state.requestTimeout.include) {
    patch.request_timeout = state.requestTimeout.value ?? null
  }
  if (state.keepPriorityOnConversion.include) {
    patch.keep_priority_on_conversion = state.keepPriorityOnConversion.value
  }
  return patch
}

export function collectFormatUnion(providers: BatchProviderEndpoints[]): string[] {
  const formats = new Set<string>()
  for (const provider of providers) {
    for (const endpoint of provider.endpoints) {
      if (endpoint.api_format) formats.add(endpoint.api_format)
    }
  }
  return [...formats].sort()
}

export function computeEndpointTargets(
  providers: BatchProviderEndpoints[],
  selectedFormats: string[],
): EndpointTargetResult {
  const formatSet = new Set(selectedFormats)
  const targets: EndpointBatchTarget[] = []
  const skippedProviders: BatchSkippedProvider[] = []

  for (const provider of providers) {
    const matched = provider.endpoints.filter(endpoint => formatSet.has(endpoint.api_format))
    if (matched.length === 0) {
      skippedProviders.push({ id: provider.id, name: provider.name, reason: 'no_matching_endpoint' })
      continue
    }

    for (const endpoint of matched) {
      targets.push({
        providerId: provider.id,
        providerName: provider.name,
        endpointId: endpoint.id,
        apiFormat: endpoint.api_format,
      })
    }
  }

  return {
    targets,
    matchedProviderCount: new Set(targets.map(target => target.providerId)).size,
    skippedProviders,
  }
}

export function buildUpstreamStreamConfig(
  existing: Record<string, unknown> | null | undefined,
  policy: UpstreamStreamPolicy,
): Record<string, unknown> | null {
  const next: Record<string, unknown> = { ...(existing || {}) }
  delete next.upstream_stream_policy
  delete next.upstreamStreamPolicy
  delete next.upstream_stream

  if (policy !== 'auto') {
    next.upstream_stream_policy = policy
  }

  return Object.keys(next).length > 0 ? next : null
}

export function mergeRules<T extends HeaderRule | BodyRule>(
  existing: T[] | null | undefined,
  incoming: T[],
  mode: BatchMergeMode,
): T[] {
  if (mode === 'overwrite') return [...incoming]
  return [...(existing || []), ...incoming]
}
