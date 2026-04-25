import type { CandidateRecord } from '@/api/requestTrace'

export const TIMELINE_STATUS: CandidateRecord['status'][] = [
  'success',
  'failed',
  'skipped',
  'cancelled',
  'pending',
  'streaming',
  'available',
  'unused',
  'stream_interrupted',
]

const POOL_HIDDEN_STATUS = new Set<CandidateRecord['status']>([
  'available',
  'unused',
])

const PROVIDER_TYPE_LIKE_NAMES = new Set<string>([
  'codex',
  'kiro',
  'antigravity',
  'claude_code',
  'claude code',
  'gemini_cli',
  'gemini cli',
  'oauth',
  'api_key',
  'api key',
])

const toInt = (value: unknown, defaultValue = 0): number => {
  const num = Number(value)
  return Number.isFinite(num) ? Math.trunc(num) : defaultValue
}

export const makeAttemptKey = (candidateIndex: number, retryIndex: number): string => {
  return `${candidateIndex}:${retryIndex}`
}

export const isPoolParticipatedCandidate = (candidate: CandidateRecord): boolean => {
  if (POOL_HIDDEN_STATUS.has(candidate.status)) return false
  if (candidate.status === 'pending' && !candidate.started_at) return false
  return true
}

export const isAttemptedCandidate = (
  candidate: Pick<CandidateRecord, 'status' | 'started_at'>,
): boolean => {
  switch (candidate.status) {
    case 'streaming':
    case 'success':
    case 'failed':
    case 'cancelled':
    case 'stream_interrupted':
      return true
    case 'pending':
      return Boolean(candidate.started_at)
    case 'available':
    case 'unused':
    case 'skipped':
    default:
      return false
  }
}

export function buildPoolGroupVisibleAttempts(
  attempts: CandidateRecord[],
): CandidateRecord[] {
  if (attempts.length === 0) return []

  const attempted = attempts.filter(isAttemptedCandidate)
  if (attempted.length > 0) {
    return attempted
  }

  return [attempts[attempts.length - 1]]
}

export const parseTimelineStatus = (value: unknown): CandidateRecord['status'] | null => {
  if (typeof value !== 'string') return null
  const normalized = value.trim().toLowerCase()
  if ((TIMELINE_STATUS as string[]).includes(normalized)) {
    return normalized as CandidateRecord['status']
  }
  return null
}

export const extractPoolGroupId = (
  candidate: Pick<CandidateRecord, 'extra_data'>,
): string | null => {
  const extra = candidate.extra_data
  if (!extra || typeof extra !== 'object' || Array.isArray(extra)) return null
  const value = (extra as Record<string, unknown>).pool_group_id
  if (typeof value !== 'string') return null
  const text = value.trim()
  return text || null
}

export function buildPoolParticipatedCandidates(
  rawTimeline: CandidateRecord[],
  attempts: unknown,
  requestId?: string | null,
): CandidateRecord[] {
  const fromTrace = rawTimeline.filter(
    candidate => extractPoolGroupId(candidate) !== null && isPoolParticipatedCandidate(candidate),
  )
  const fromAudit = buildPoolAttemptCandidatesFromAudit(rawTimeline, attempts, requestId)

  if (fromTrace.length === 0) return fromAudit
  if (fromAudit.length === 0) return fromTrace

  const traceKeys = new Set(
    fromTrace.map(candidate => makeAttemptKey(candidate.candidate_index, candidate.retry_index)),
  )
  const merged = [...fromTrace]
  for (const candidate of fromAudit) {
    const key = makeAttemptKey(candidate.candidate_index, candidate.retry_index)
    if (!traceKeys.has(key)) {
      merged.push(candidate)
    }
  }

  return merged.sort((a, b) => {
    if (a.candidate_index !== b.candidate_index) {
      return a.candidate_index - b.candidate_index
    }
    return a.retry_index - b.retry_index
  })
}

export function buildPoolAttemptCandidatesFromAudit(
  rawTimeline: CandidateRecord[],
  attempts: unknown,
  requestId?: string | null,
): CandidateRecord[] {
  if (!Array.isArray(attempts) || attempts.length === 0) return []

  const providerNameById = new Map<string, string>()
  for (const candidate of rawTimeline) {
    const providerId = String(candidate.provider_id || '').trim()
    const providerName = String(candidate.provider_name || '').trim()
    if (!providerId || !providerName) continue
    if (!providerNameById.has(providerId)) {
      providerNameById.set(providerId, providerName)
    }
  }

  const traceMap = new Map<string, CandidateRecord>()
  for (const candidate of rawTimeline) {
    traceMap.set(makeAttemptKey(candidate.candidate_index, candidate.retry_index), candidate)
  }

  return attempts
    .map((item, index) => {
      if (!item || typeof item !== 'object' || Array.isArray(item)) return null
      const raw = item as Record<string, unknown>
      const candidateIndex = toInt(raw.candidate_index, index)
      const retryIndex = toInt(raw.retry_index, 0)
      const key = makeAttemptKey(candidateIndex, retryIndex)
      const fromTrace = traceMap.get(key)
      const parsedStatus = parseTimelineStatus(raw.status)

      if (!fromTrace && parsedStatus === null) {
        return null
      }

      const merged: CandidateRecord = fromTrace
        ? { ...fromTrace }
        : {
            id: `pool-${requestId || 'unknown'}-${candidateIndex}-${retryIndex}-${index}`,
            request_id: requestId || '',
            candidate_index: candidateIndex,
            retry_index: retryIndex,
            provider_id: undefined,
            provider_name: undefined,
            endpoint_id: undefined,
            key_id: undefined,
            key_name: undefined,
            status: 'failed',
            is_cached: false,
            created_at: new Date(0).toISOString(),
          }

      if (parsedStatus !== null) {
        merged.status = parsedStatus
      }
      if (typeof raw.provider_id === 'string') merged.provider_id = raw.provider_id
      if (typeof raw.provider_name === 'string') merged.provider_name = raw.provider_name
      if (typeof raw.endpoint_id === 'string') merged.endpoint_id = raw.endpoint_id
      if (typeof raw.key_id === 'string') merged.key_id = raw.key_id
      if (typeof raw.key_name === 'string') merged.key_name = raw.key_name
      if (typeof raw.status_code === 'number') merged.status_code = raw.status_code
      if (typeof raw.error_type === 'string') merged.error_type = raw.error_type
      const rawPoolGroupId = typeof raw.pool_group_id === 'string' ? raw.pool_group_id.trim() : ''
      const fallbackPoolGroupId = typeof raw.provider_id === 'string' ? raw.provider_id.trim() : ''
      const finalPoolGroupId = rawPoolGroupId || fallbackPoolGroupId
      if (finalPoolGroupId) {
        merged.extra_data = {
          ...(merged.extra_data || {}),
          pool_group_id: finalPoolGroupId,
        }
      }

      const mergedProviderId = String(merged.provider_id || '').trim()
      if (mergedProviderId) {
        const inferredProviderName = providerNameById.get(mergedProviderId)
        const currentProviderName = String(merged.provider_name || '').trim()
        if (
          inferredProviderName
          && (
            !currentProviderName
            || PROVIDER_TYPE_LIKE_NAMES.has(currentProviderName.toLowerCase())
          )
        ) {
          merged.provider_name = inferredProviderName
        }
      }

      return isPoolParticipatedCandidate(merged) ? merged : null
    })
    .filter((item): item is CandidateRecord => item !== null)
}
