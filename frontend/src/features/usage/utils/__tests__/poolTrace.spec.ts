import { describe, expect, it } from 'vitest'

import type { CandidateRecord } from '@/api/requestTrace'
import {
  buildPoolAttemptCandidatesFromAudit,
  buildPoolGroupVisibleAttempts,
  buildPoolParticipatedCandidates,
  isAttemptedCandidate,
} from '@/features/usage/utils/poolTrace'

function buildCandidate(
  overrides: Partial<CandidateRecord> = {},
): CandidateRecord {
  return {
    id: 'cand-1',
    request_id: 'req-1',
    candidate_index: 0,
    retry_index: 0,
    status: 'failed',
    is_cached: false,
    created_at: '1970-01-01T00:00:00.000Z',
    ...overrides,
  }
}

describe('poolTrace', () => {
  it('keeps only pool nodes that actually participated in scheduling audit fallback', () => {
    const attempts = buildPoolAttemptCandidatesFromAudit([], [
      {
        candidate_index: 0,
        retry_index: 0,
        provider_id: 'provider-1',
        provider_name: 'Codex反代',
        key_id: 'key-success',
        key_name: 'Success Key',
        status: 'success',
        pool_group_id: 'provider-1',
      },
      {
        candidate_index: 1,
        retry_index: 0,
        provider_id: 'provider-1',
        provider_name: 'Codex反代',
        key_id: 'key-skipped',
        key_name: 'Skipped Key',
        status: 'skipped',
        pool_group_id: 'provider-1',
      },
      {
        candidate_index: 2,
        retry_index: 0,
        provider_id: 'provider-1',
        provider_name: 'Codex反代',
        key_id: 'key-available',
        key_name: 'Available Key',
        status: 'available',
        pool_group_id: 'provider-1',
      },
      {
        candidate_index: 3,
        retry_index: 0,
        provider_id: 'provider-1',
        provider_name: 'Codex反代',
        key_id: 'key-unknown',
        key_name: 'Unknown Key',
        status: 'selected',
        pool_group_id: 'provider-1',
      },
    ], 'req-1')

    expect(attempts).toHaveLength(2)
    expect(attempts[0].key_id).toBe('key-success')
    expect(attempts[0].status).toBe('success')
    expect(attempts[1].key_id).toBe('key-skipped')
    expect(attempts[1].status).toBe('skipped')
  })

  it('preserves real trace attempts even when audit status is non-standard', () => {
    const rawTimeline = [
      buildCandidate({
        id: 'cand-trace-1',
        candidate_index: 4,
        retry_index: 0,
        provider_id: 'provider-1',
        provider_name: 'Codex反代',
        key_id: 'key-trace',
        key_name: 'Trace Key',
        status: 'failed',
        started_at: '2026-04-19T12:00:00.000Z',
      }),
    ]

    const attempts = buildPoolAttemptCandidatesFromAudit(rawTimeline, [
      {
        candidate_index: 4,
        retry_index: 0,
        provider_id: 'provider-1',
        provider_name: 'oauth',
        key_id: 'key-trace',
        key_name: 'Trace Key',
        status: 'selected',
        pool_group_id: 'provider-1',
      },
      {
        candidate_index: 5,
        retry_index: 0,
        provider_id: 'provider-1',
        provider_name: 'oauth',
        key_id: 'key-ghost',
        key_name: 'Ghost Key',
        status: 'selected',
        pool_group_id: 'provider-1',
      },
    ], 'req-1')

    expect(attempts).toHaveLength(1)
    expect(attempts[0].id).toBe('cand-trace-1')
    expect(attempts[0].status).toBe('failed')
    expect(attempts[0].provider_name).toBe('Codex反代')
  })

  it('merges audit-only skipped pool nodes when trace only carries partial pool metadata', () => {
    const rawTimeline = [
      buildCandidate({
        id: 'cand-success',
        candidate_index: 1,
        retry_index: 0,
        provider_id: 'provider-1',
        provider_name: 'Codex反代',
        key_id: 'key-success',
        key_name: 'Success Key',
        status: 'success',
        extra_data: { pool_group_id: 'provider-1' },
        started_at: '2026-04-19T12:00:00.000Z',
      }),
      buildCandidate({
        id: 'cand-skipped',
        candidate_index: 2,
        retry_index: 0,
        provider_id: 'provider-1',
        provider_name: 'Codex反代',
        key_id: 'key-skipped',
        key_name: 'Skipped Key',
        status: 'skipped',
      }),
    ]

    const attempts = buildPoolParticipatedCandidates(rawTimeline, [
      {
        candidate_index: 1,
        retry_index: 0,
        provider_id: 'provider-1',
        provider_name: 'Codex反代',
        key_id: 'key-success',
        key_name: 'Success Key',
        status: 'success',
        pool_group_id: 'provider-1',
      },
      {
        candidate_index: 2,
        retry_index: 0,
        provider_id: 'provider-1',
        provider_name: 'Codex反代',
        key_id: 'key-skipped',
        key_name: 'Skipped Key',
        status: 'skipped',
        pool_group_id: 'provider-1',
      },
    ], 'req-1')

    expect(attempts).toHaveLength(2)
    expect(attempts.map(item => item.key_id)).toEqual(['key-success', 'key-skipped'])
    expect(attempts[1].extra_data?.pool_group_id).toBe('provider-1')
  })

  it('treats only real execution statuses as attempted', () => {
    expect(isAttemptedCandidate(buildCandidate({ status: 'success' }))).toBe(true)
    expect(isAttemptedCandidate(buildCandidate({ status: 'failed' }))).toBe(true)
    expect(isAttemptedCandidate(buildCandidate({ status: 'cancelled' }))).toBe(true)
    expect(isAttemptedCandidate(buildCandidate({ status: 'streaming' }))).toBe(true)
    expect(isAttemptedCandidate(buildCandidate({ status: 'stream_interrupted' }))).toBe(true)
    expect(isAttemptedCandidate(buildCandidate({ status: 'pending' }))).toBe(false)
    expect(isAttemptedCandidate(buildCandidate({
      status: 'pending',
      started_at: '2026-04-24T12:00:00.000Z',
    }))).toBe(true)
    expect(isAttemptedCandidate(buildCandidate({ status: 'skipped' }))).toBe(false)
    expect(isAttemptedCandidate(buildCandidate({ status: 'available' }))).toBe(false)
    expect(isAttemptedCandidate(buildCandidate({ status: 'unused' }))).toBe(false)
  })

  it('shows only attempted pool children when attempted nodes exist', () => {
    const attempts = buildPoolGroupVisibleAttempts([
      buildCandidate({
        id: 'cand-skipped',
        candidate_index: 0,
        status: 'skipped',
      }),
      buildCandidate({
        id: 'cand-failed',
        candidate_index: 1,
        status: 'failed',
        started_at: '2026-04-24T12:00:00.000Z',
      }),
      buildCandidate({
        id: 'cand-success',
        candidate_index: 2,
        status: 'success',
        started_at: '2026-04-24T12:00:01.000Z',
      }),
    ])

    expect(attempts.map(item => item.id)).toEqual(['cand-failed', 'cand-success'])
  })

  it('collapses all-skipped pool nodes to a single provider node', () => {
    const attempts = buildPoolGroupVisibleAttempts([
      buildCandidate({
        id: 'cand-skipped-1',
        candidate_index: 0,
        status: 'skipped',
      }),
      buildCandidate({
        id: 'cand-skipped-2',
        candidate_index: 1,
        status: 'skipped',
      }),
    ])

    expect(attempts.map(item => item.id)).toEqual(['cand-skipped-2'])
  })
})
