import { describe, expect, it } from 'vitest'

import {
  hasUsageFallback,
  isUsageRecordFailed,
  isUsageRecordSuccessful,
  mapRequestStatusToTimelineStatus,
  normalizeRequestStatus,
  resolveDisplayRequestStatus,
  resolveTimelineFinalStatus,
} from '../status'
import type { UsageRecord } from '../../types'

function buildUsageRecord(overrides: Partial<UsageRecord> = {}): UsageRecord {
  return {
    id: 'usage-1',
    model: 'gpt-5',
    input_tokens: 10,
    output_tokens: 20,
    total_tokens: 30,
    cost: 0,
    is_stream: false,
    created_at: '2026-04-10T00:00:00Z',
    status: 'completed',
    ...overrides
  }
}

describe('usage status helpers', () => {
  it('treats explicit completed status as authoritative over stale legacy failure fields', () => {
    const record = buildUsageRecord({
      status: 'completed',
      status_code: 429,
      error_message: 'rate limited on first attempt'
    })

    expect(isUsageRecordFailed(record)).toBe(false)
    expect(isUsageRecordSuccessful(record)).toBe(true)
  })

  it('falls back to legacy failure signals when status is missing', () => {
    const record = buildUsageRecord({
      status: undefined,
      status_code: 429,
      error_message: 'rate limited'
    })

    expect(isUsageRecordFailed(record)).toBe(true)
    expect(isUsageRecordSuccessful(record)).toBe(false)
  })

  it('treats explicit failed status with a 2xx status code as successful for display', () => {
    const record = buildUsageRecord({
      status: 'failed',
      status_code: 200,
      error_message: 'stale failure flag'
    })

    expect(isUsageRecordFailed(record)).toBe(false)
    expect(isUsageRecordSuccessful(record)).toBe(true)
  })

  it('normalizes request status strings before mapping timeline status', () => {
    expect(normalizeRequestStatus(' Completed ')).toBe('completed')
    expect(mapRequestStatusToTimelineStatus('completed')).toBe('success')
    expect(mapRequestStatusToTimelineStatus('failed')).toBe('failed')
  })

  it('shows streaming as pending until first byte is recorded', () => {
    expect(resolveDisplayRequestStatus(buildUsageRecord({
      status: 'streaming',
      first_byte_time_ms: undefined,
    }))).toBe('pending')

    expect(resolveDisplayRequestStatus(buildUsageRecord({
      status: 'streaming',
      first_byte_time_ms: 320,
    }))).toBe('streaming')
  })

  it('treats explicit success status code as authoritative for the timeline', () => {
    expect(resolveTimelineFinalStatus({
      traceFinalStatus: 'success',
      requestStatus: 'failed',
      statusCode: 200,
    })).toBe('success')
  })

  it('falls back to request lifecycle status when status code and trace are missing', () => {
    expect(resolveTimelineFinalStatus({
      requestStatus: 'failed',
    })).toBe('failed')
  })

  it('uses explicit has_fallback flag for transfer filtering', () => {
    expect(hasUsageFallback(buildUsageRecord({ has_fallback: true }))).toBe(true)
    expect(hasUsageFallback(buildUsageRecord({ has_fallback: false }))).toBe(false)
    expect(hasUsageFallback(buildUsageRecord({ has_fallback: undefined }))).toBe(false)
  })

  it('uses status code only as a last fallback for timeline status', () => {
    expect(resolveTimelineFinalStatus({
      statusCode: 200,
    })).toBe('success')
    expect(resolveTimelineFinalStatus({
      statusCode: 503,
    })).toBe('failed')
  })
})
