import type { RequestStatus, UsageRecord } from '../types'

export type TimelineFinalStatus = 'success' | 'failed' | 'streaming' | 'pending' | 'cancelled'

type RequestStatusLike = RequestStatus | string | null | undefined

type UsageFailureSignal = {
  status_code?: number | null
  error_message?: string | null
  image_progress?: {
    phase?: string | null
  } | null
}

type UsageDisplayStatusRecord = UsageFailureSignal & {
  status?: RequestStatusLike
  first_byte_time_ms?: number | null
}

function hasLegacyFailureSignal(
  record: UsageFailureSignal
): boolean {
  return (typeof record.status_code === 'number' && record.status_code >= 400) ||
    (typeof record.error_message === 'string' && record.error_message.trim().length > 0)
}

function hasImageProgressFailureSignal(
  record: UsageFailureSignal
): boolean {
  return typeof record.image_progress?.phase === 'string' &&
    record.image_progress.phase.trim().toLowerCase() === 'failed'
}

function hasAnyFailureSignal(
  record: UsageFailureSignal
): boolean {
  return hasLegacyFailureSignal(record) || hasImageProgressFailureSignal(record)
}

export function hasUsageFallback(
  record: Pick<UsageRecord, 'has_fallback'>
): boolean {
  return record.has_fallback === true
}

export function hasUsageRetry(
  record: Pick<UsageRecord, 'has_retry'>
): boolean {
  return record.has_retry === true
}

export function resolveUsageStreamModes(
  record: Pick<
    UsageRecord,
    | 'is_stream'
    | 'upstream_is_stream'
    | 'client_requested_stream'
    | 'client_is_stream'
    | 'api_format'
    | 'endpoint_api_format'
  >
): { clientRequestedStream: boolean, upstreamStream: boolean } {
  const upstreamStream = typeof record.upstream_is_stream === 'boolean'
    ? record.upstream_is_stream
    : record.is_stream

  const streamFormat = normalizeUsageStreamApiFormat(
    record.api_format ?? record.endpoint_api_format
  )

  return {
    clientRequestedStream: typeof record.client_requested_stream === 'boolean'
      ? record.client_requested_stream
      : typeof record.client_is_stream === 'boolean'
        ? record.client_is_stream
        : usageApiFormatDefaultsToNonStream(streamFormat)
          ? false
        : upstreamStream,
    upstreamStream
  }
}

export function isUsageUpstreamStream(
  record: Pick<
    UsageRecord,
    | 'is_stream'
    | 'upstream_is_stream'
    | 'client_requested_stream'
    | 'client_is_stream'
    | 'api_format'
    | 'endpoint_api_format'
  >
): boolean {
  return resolveUsageStreamModes(record).upstreamStream
}

export function formatUsageStreamLabel(
  record: Pick<
    UsageRecord,
    | 'is_stream'
    | 'upstream_is_stream'
    | 'client_requested_stream'
    | 'client_is_stream'
    | 'api_format'
    | 'endpoint_api_format'
  >
): string {
  const { clientRequestedStream, upstreamStream } = resolveUsageStreamModes(record)
  const clientLabel = clientRequestedStream ? '流式' : '标准'
  const upstreamLabel = upstreamStream ? '流式' : '标准'

  if (clientRequestedStream === upstreamStream) {
    return clientLabel
  }

  return `${clientLabel}->${upstreamLabel}`
}

export interface UsageStreamLabelSegments {
  client: '流式' | '标准'
  upstream: '流式' | '标准'
  hasConversion: boolean
}

export function resolveUsageStreamLabelSegments(
  record: Pick<
    UsageRecord,
    | 'is_stream'
    | 'upstream_is_stream'
    | 'client_requested_stream'
    | 'client_is_stream'
    | 'api_format'
    | 'endpoint_api_format'
  >
): UsageStreamLabelSegments {
  const { clientRequestedStream, upstreamStream } = resolveUsageStreamModes(record)
  return {
    client: clientRequestedStream ? '流式' : '标准',
    upstream: upstreamStream ? '流式' : '标准',
    hasConversion: clientRequestedStream !== upstreamStream,
  }
}

function normalizeUsageStreamApiFormat(value: string | null | undefined): string {
  const normalized = value?.trim().toLowerCase().replaceAll('_', ':') ?? ''
  switch (normalized) {
    case 'openai':
      return 'openai:chat'
    case 'claude':
      return 'claude:messages'
    case 'gemini':
      return 'gemini:generate_content'
    default:
      return normalized
  }
}

function usageApiFormatDefaultsToNonStream(apiFormat: string): boolean {
  switch (apiFormat) {
    case 'openai:chat':
    case 'openai:responses':
    case 'openai:responses:compact':
    case 'openai:image':
    case 'claude:messages':
      return true
    default:
      return false
  }
}

function hasTerminalSuccessStatusCode(
  record: UsageFailureSignal
): boolean {
  return typeof record.status_code === 'number' &&
    record.status_code >= 200 &&
    record.status_code < 300
}

export function isUsageRecordFailed(record: UsageFailureSignal & Pick<UsageRecord, 'status'>): boolean {
  const status = typeof record.status === 'string' ? record.status.trim().toLowerCase() : ''
  if (status) {
    if (status === 'pending' || status === 'streaming') {
      return !hasTerminalSuccessStatusCode(record) && hasAnyFailureSignal(record)
    }
    if (status === 'cancelled') {
      return false
    }
    if (status === 'completed') {
      return false
    }
    if (status === 'failed') {
      return true
    }
  }
  if (hasTerminalSuccessStatusCode(record)) {
    return false
  }
  if (status) {
    return status === 'failed'
  }
  return hasAnyFailureSignal(record)
}

export function isUsageRecordSuccessful(record: UsageFailureSignal & Pick<UsageRecord, 'status'>): boolean {
  const status = typeof record.status === 'string' ? record.status.trim().toLowerCase() : ''
  if (status) {
    if (status === 'completed') {
      return true
    }
    if (status === 'failed') {
      return false
    }
    return false
  }
  if (hasTerminalSuccessStatusCode(record)) {
    return true
  }
  return !hasAnyFailureSignal(record)
}

export function normalizeRequestStatus(status: RequestStatusLike): RequestStatus | undefined {
  const normalized = typeof status === 'string' ? status.trim().toLowerCase() : ''
  switch (normalized) {
    case 'pending':
    case 'streaming':
    case 'completed':
    case 'failed':
    case 'cancelled':
      return normalized
    default:
      return undefined
  }
}

export function resolveDisplayRequestStatus(record: UsageDisplayStatusRecord): RequestStatus | undefined {
  const status = normalizeRequestStatus(record.status)
  if ((status === 'pending' || status === 'streaming') &&
    !hasTerminalSuccessStatusCode(record) &&
    hasAnyFailureSignal(record)) {
    return 'failed'
  }
  if (status === 'streaming' && record.first_byte_time_ms == null) {
    return 'pending'
  }
  return status ?? (hasAnyFailureSignal(record) ? 'failed' : undefined)
}

export function mapRequestStatusToTimelineStatus(
  status: RequestStatusLike
): TimelineFinalStatus | undefined {
  switch (normalizeRequestStatus(status)) {
    case 'completed':
      return 'success'
    case 'failed':
      return 'failed'
    case 'streaming':
      return 'streaming'
    case 'pending':
      return 'pending'
    case 'cancelled':
      return 'cancelled'
    default:
      return undefined
  }
}

function normalizeTimelineFinalStatus(status: string | null | undefined): TimelineFinalStatus | undefined {
  const normalized = typeof status === 'string' ? status.trim().toLowerCase() : ''
  switch (normalized) {
    case 'success':
    case 'failed':
    case 'streaming':
    case 'pending':
    case 'cancelled':
      return normalized
    default:
      return undefined
  }
}

export function resolveTimelineFinalStatus(params: {
  hasPendingCandidates?: boolean
  traceFinalStatus?: string | null
  requestStatus?: RequestStatusLike
  statusCode?: number
}): TimelineFinalStatus {
  const hasTerminalSuccessStatusCode = typeof params.statusCode === 'number'
    ? params.statusCode >= 200 && params.statusCode < 300
    : undefined

  const requestStatus = mapRequestStatusToTimelineStatus(params.requestStatus)
  if (requestStatus === 'success' || requestStatus === 'failed' || requestStatus === 'cancelled') {
    if (requestStatus === 'success' && hasTerminalSuccessStatusCode === false) {
      return 'failed'
    }
    return requestStatus
  }
  if (requestStatus === 'pending' || requestStatus === 'streaming') {
    return requestStatus
  }

  const traceStatus = normalizeTimelineFinalStatus(params.traceFinalStatus)
  if (traceStatus === 'success' || traceStatus === 'failed' || traceStatus === 'cancelled') {
    if (traceStatus === 'success' && hasTerminalSuccessStatusCode === false) {
      return 'failed'
    }
    return traceStatus
  }

  if (params.hasPendingCandidates) {
    return 'pending'
  }

  if (hasTerminalSuccessStatusCode !== undefined) {
    return hasTerminalSuccessStatusCode ? 'success' : 'failed'
  }

  if (traceStatus) {
    return traceStatus
  }

  if (requestStatus) {
    return requestStatus
  }

  return 'pending'
}
