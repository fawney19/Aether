import type { RequestStatus, UsageRecord } from '../types'
import { isCyberPolicyError } from './cyberError'

export type UsageRecordStreamResolution = Pick<
  UsageRecord,
  'id' | 'is_stream' | 'upstream_is_stream' | 'client_requested_stream' | 'client_is_stream'
>

export type UsageRecordResponseTiming = Pick<
  UsageRecord,
  'response_time_ms' | 'response_time_updated_at'
>

function finiteNonNegativeDurationMs(value: number | null | undefined): number | null {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0 ? value : null
}

export function parseUsageTimestampMs(value: string | null | undefined): number | null {
  if (!value) return null
  const normalized = /(?:Z|[+-]\d{2}:\d{2})$/i.test(value) ? value : `${value}Z`
  const timestampMs = new Date(normalized).getTime()
  return Number.isFinite(timestampMs) ? timestampMs : null
}

/**
 * Merge a live response duration together with the timestamp that anchors it.
 *
 * These fields form one clock snapshot: active elapsed time is projected as
 * `response_time_ms + (now - response_time_updated_at)`. Merging the larger
 * duration with a newer timestamp can therefore manufacture a shorter clock
 * that never existed. Keep the pair atomic and, while active, retain whichever
 * snapshot projects the larger elapsed value.
 */
export function mergeUsageRecordResponseTiming(
  existing: UsageRecordResponseTiming,
  next: UsageRecordResponseTiming,
  options: { preferNext?: boolean } = {},
): UsageRecordResponseTiming {
  const existingDurationMs = finiteNonNegativeDurationMs(existing.response_time_ms)
  const nextDurationMs = finiteNonNegativeDurationMs(next.response_time_ms)

  if (nextDurationMs == null) return existing
  if (options.preferNext || existingDurationMs == null) return next

  const existingUpdatedAtMs = parseUsageTimestampMs(existing.response_time_updated_at)
  const nextUpdatedAtMs = parseUsageTimestampMs(next.response_time_updated_at)

  if (existingUpdatedAtMs != null && nextUpdatedAtMs != null) {
    const existingStartedAtMs = existingUpdatedAtMs - existingDurationMs
    const nextStartedAtMs = nextUpdatedAtMs - nextDurationMs
    return nextStartedAtMs <= existingStartedAtMs ? next : existing
  }

  // An anchored snapshot is safer than an unanchored duration for a live clock.
  if (existingUpdatedAtMs != null) return existing
  if (nextUpdatedAtMs != null) return next

  return nextDurationMs >= existingDurationMs ? next : existing
}

export function mergeUsageRecordFirstByteTimeMs(
  existingValue: number | null | undefined,
  nextValue: number | null | undefined
): number | null | undefined {
  // Millisecond timing is floored, so 0 still means the first byte was observed.
  const existingIsResolved = typeof existingValue === 'number' &&
    Number.isFinite(existingValue) &&
    existingValue >= 0
  const nextIsResolved = typeof nextValue === 'number' &&
    Number.isFinite(nextValue) &&
    nextValue >= 0

  if (existingIsResolved && nextIsResolved) {
    return Math.max(existingValue, nextValue)
  }
  if (existingIsResolved) {
    return existingValue
  }
  if (nextIsResolved) {
    return nextValue
  }
  return existingValue == null ? existingValue : undefined
}

export function mergeUsageRecordErrorMessage(
  existingValue: string | null | undefined,
  nextValue: string | null | undefined,
  options: { authoritative?: boolean } = {},
): string | undefined {
  const existing = typeof existingValue === 'string' && existingValue.trim()
    ? existingValue
    : undefined
  const next = typeof nextValue === 'string' && nextValue.trim()
    ? nextValue
    : undefined

  // Complete list/active snapshots describe the current final candidate. They
  // must be able to replace *and clear* an error left by an earlier candidate.
  if (options.authoritative) return next

  if (!next) return existing

  // Detail/trace snapshots may carry a generic runtime message. Do not let that
  // downgrade a provider Cyber Policy refusal already resolved by the usage list.
  if (isCyberPolicyError(existing) && !isCyberPolicyError(next)) return existing

  return next
}

export type UsageRecordLifecycleSnapshot = Pick<
  UsageRecord,
  'status' | 'status_code' | 'error_message' | 'updated_at'
>

export type UsageRecordLifecycleUpdate = {
  status?: RequestStatus
  statusCode?: number | null
  errorMessage?: string | null
  updatedAt?: string | null
}

/**
 * Merge the sparse lifecycle state emitted by the detail drawer.
 *
 * Status, status code and error belong to one snapshot. If a detail response
 * is older (or its status would regress), none of those fields may leak into
 * the newer row. A completed/cancelled or explicitly newer terminal snapshot
 * is authoritative for errors; a same-snapshot generic failure still keeps a
 * provider Cyber refusal already known by the list.
 */
export function mergeUsageRecordLifecycleSnapshot(
  existing: UsageRecordLifecycleSnapshot,
  update: UsageRecordLifecycleUpdate,
): UsageRecordLifecycleSnapshot & { accepted: boolean } {
  const statusPriority: Record<RequestStatus, number> = {
    pending: 0,
    streaming: 1,
    completed: 2,
    failed: 2,
    cancelled: 2,
  }
  const existingUpdatedAtMs = parseUsageTimestampMs(existing.updated_at)
  const nextUpdatedAtMs = parseUsageTimestampMs(update.updatedAt)
  const nextSnapshotIsOlder = existingUpdatedAtMs != null &&
    nextUpdatedAtMs != null &&
    nextUpdatedAtMs < existingUpdatedAtMs
  const currentRank = existing.status ? statusPriority[existing.status] : -1
  const nextRank = update.status ? statusPriority[update.status] : -1
  const statusAccepted = update.status != null &&
    !nextSnapshotIsOlder &&
    nextRank >= currentRank
  const accepted = !nextSnapshotIsOlder && (update.status == null || statusAccepted)

  if (!accepted) {
    return { ...existing, accepted: false }
  }

  const terminalSnapshotIsStrictlyNewer = statusAccepted &&
    (update.status === 'completed' || update.status === 'failed' || update.status === 'cancelled') &&
    existingUpdatedAtMs != null &&
    nextUpdatedAtMs != null &&
    nextUpdatedAtMs > existingUpdatedAtMs
  const errorIsAuthoritative = statusAccepted && (
    update.status === 'completed' ||
    update.status === 'cancelled' ||
    terminalSnapshotIsStrictlyNewer
  )
  const hasStatusCode = Object.prototype.hasOwnProperty.call(update, 'statusCode')
  const hasErrorMessage = Object.prototype.hasOwnProperty.call(update, 'errorMessage')

  return {
    status: statusAccepted ? update.status : existing.status,
    status_code: hasStatusCode ? (update.statusCode ?? undefined) : existing.status_code,
    error_message: hasErrorMessage
      ? mergeUsageRecordErrorMessage(
        existing.error_message,
        update.errorMessage,
        { authoritative: errorIsAuthoritative },
      )
      : existing.error_message,
    updated_at: typeof update.updatedAt === 'string'
      ? update.updatedAt
      : existing.updated_at,
    accepted: true,
  }
}

export function syncUsageRecordStreamResolution(
  records: UsageRecord[],
  resolved: UsageRecordStreamResolution
): UsageRecord[] {
  let changed = false

  const nextRecords = records.map((record) => {
    if (record.id !== resolved.id) {
      return record
    }

    changed = true
    return {
      ...record,
      is_stream: resolved.is_stream,
      upstream_is_stream: typeof resolved.upstream_is_stream === 'boolean'
        ? resolved.upstream_is_stream
        : resolved.is_stream,
      client_requested_stream: typeof resolved.client_requested_stream === 'boolean'
        ? resolved.client_requested_stream
        : resolved.client_is_stream,
      client_is_stream: typeof resolved.client_is_stream === 'boolean'
        ? resolved.client_is_stream
        : resolved.client_requested_stream,
    }
  })

  return changed ? nextRecords : records
}
