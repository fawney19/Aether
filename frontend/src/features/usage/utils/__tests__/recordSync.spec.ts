import { describe, expect, it } from 'vitest'

import type { UsageRecord } from '../../types'
import {
  mergeUsageRecordErrorMessage,
  mergeUsageRecordFirstByteTimeMs,
  mergeUsageRecordLifecycleSnapshot,
  mergeUsageRecordResponseTiming,
  syncUsageRecordStreamResolution,
} from '../recordSync'

function buildUsageRecord(overrides: Partial<UsageRecord> = {}): UsageRecord {
  return {
    id: 'usage-1',
    model: 'gpt-5.4',
    input_tokens: 10,
    output_tokens: 5,
    total_tokens: 15,
    cost: 0.001,
    is_stream: true,
    upstream_is_stream: true,
    client_requested_stream: true,
    client_is_stream: true,
    created_at: '2026-04-23T00:00:00Z',
    ...overrides,
  }
}

describe('syncUsageRecordStreamResolution', () => {
  it('updates the matching row with resolved client and upstream stream modes', () => {
    const records = [
      buildUsageRecord(),
      buildUsageRecord({ id: 'usage-2', model: 'gpt-4.1' }),
    ]

    const nextRecords = syncUsageRecordStreamResolution(records, {
      id: 'usage-1',
      is_stream: true,
      upstream_is_stream: true,
      client_requested_stream: false,
      client_is_stream: false,
    })

    expect(nextRecords[0]).toMatchObject({
      id: 'usage-1',
      is_stream: true,
      upstream_is_stream: true,
      client_requested_stream: false,
      client_is_stream: false,
    })
    expect(nextRecords[1]).toBe(records[1])
  })

  it('returns the original array when no matching row exists', () => {
    const records = [buildUsageRecord()]

    const nextRecords = syncUsageRecordStreamResolution(records, {
      id: 'missing',
      is_stream: false,
      upstream_is_stream: false,
      client_requested_stream: false,
      client_is_stream: false,
    })

    expect(nextRecords).toBe(records)
  })
})

describe('mergeUsageRecordFirstByteTimeMs', () => {
  it('does not let a stale active update clear or reduce a resolved first-byte time', () => {
    expect(mergeUsageRecordFirstByteTimeMs(500, null)).toBe(500)
    expect(mergeUsageRecordFirstByteTimeMs(500, 320)).toBe(500)
    expect(mergeUsageRecordFirstByteTimeMs(500, 640)).toBe(640)
    expect(mergeUsageRecordFirstByteTimeMs(undefined, 320)).toBe(320)
    expect(mergeUsageRecordFirstByteTimeMs(undefined, 0)).toBe(0)
    expect(mergeUsageRecordFirstByteTimeMs(0, null)).toBe(0)
    expect(mergeUsageRecordFirstByteTimeMs(null, -1)).toBeNull()
    expect(mergeUsageRecordFirstByteTimeMs(-1, null)).toBeUndefined()
  })
})

describe('mergeUsageRecordResponseTiming', () => {
  it('keeps duration and update timestamp as one monotonic active snapshot', () => {
    const existing = {
      response_time_ms: 5500,
      response_time_updated_at: '2026-07-17T12:00:06Z',
    }
    const stale = {
      response_time_ms: 5000,
      response_time_updated_at: '2026-07-17T12:00:07Z',
    }

    expect(mergeUsageRecordResponseTiming(existing, stale)).toBe(existing)
  })

  it('accepts a live snapshot whose projected elapsed time has advanced', () => {
    const existing = {
      response_time_ms: 5500,
      response_time_updated_at: '2026-07-17T12:00:06Z',
    }
    const advanced = {
      response_time_ms: 7000,
      response_time_updated_at: '2026-07-17T12:00:07Z',
    }

    expect(mergeUsageRecordResponseTiming(existing, advanced)).toBe(advanced)
  })

  it('does not combine an unanchored detail estimate with an existing anchor', () => {
    const existing = {
      response_time_ms: 5500,
      response_time_updated_at: '2026-07-17T12:00:06Z',
    }
    const detailEstimate = {
      response_time_ms: 6000,
      response_time_updated_at: null,
    }

    expect(mergeUsageRecordResponseTiming(existing, detailEstimate)).toBe(existing)
  })

  it('lets a terminal snapshot replace the active estimate', () => {
    const terminal = {
      response_time_ms: 5200,
      response_time_updated_at: null,
    }

    expect(mergeUsageRecordResponseTiming({
      response_time_ms: 5500,
      response_time_updated_at: '2026-07-17T12:00:06Z',
    }, terminal, { preferNext: true })).toBe(terminal)
  })
})

describe('mergeUsageRecordErrorMessage', () => {
  const cyberMessage = 'This content was flagged for possible cybersecurity risk. Join the Trusted Access for Cyber program: https://chatgpt.com/cyber'

  it('keeps an authoritative Cyber Policy message when trace reports a generic error', () => {
    expect(mergeUsageRecordErrorMessage(
      cyberMessage,
      'execution runtime stream ended with a terminal error',
    )).toBe(cyberMessage)
  })

  it('keeps an existing error when trace omits its error message', () => {
    expect(mergeUsageRecordErrorMessage(cyberMessage, undefined)).toBe(cyberMessage)
    expect(mergeUsageRecordErrorMessage(cyberMessage, null)).toBe(cyberMessage)
    expect(mergeUsageRecordErrorMessage(cyberMessage, '   ')).toBe(cyberMessage)
  })

  it('accepts a Cyber Policy message discovered by trace', () => {
    expect(mergeUsageRecordErrorMessage('Request failed', cyberMessage)).toBe(cyberMessage)
  })

  it('updates ordinary errors when the next snapshot has a more specific message', () => {
    expect(mergeUsageRecordErrorMessage('Request failed', 'rate limit exceeded'))
      .toBe('rate limit exceeded')
    expect(mergeUsageRecordErrorMessage(undefined, 'Request failed')).toBe('Request failed')
  })

  it('lets an authoritative final-candidate snapshot replace or clear Cyber', () => {
    expect(mergeUsageRecordErrorMessage(
      cyberMessage,
      'rate limit exceeded',
      { authoritative: true },
    )).toBe('rate limit exceeded')
    expect(mergeUsageRecordErrorMessage(
      cyberMessage,
      null,
      { authoritative: true },
    )).toBeUndefined()
  })
})

describe('mergeUsageRecordLifecycleSnapshot', () => {
  const cyberMessage = 'This content was flagged for possible cybersecurity risk. https://chatgpt.com/cyber'

  it('rejects an older failed detail without changing status, code, or error', () => {
    expect(mergeUsageRecordLifecycleSnapshot({
      status: 'completed',
      status_code: 200,
      error_message: undefined,
      updated_at: '2026-07-17T00:00:02Z',
    }, {
      status: 'failed',
      statusCode: 400,
      errorMessage: cyberMessage,
      updatedAt: '2026-07-17T00:00:01Z',
    })).toEqual({
      status: 'completed',
      status_code: 200,
      error_message: undefined,
      updated_at: '2026-07-17T00:00:02Z',
      accepted: false,
    })
  })

  it('accepts a newer completed detail and clears an earlier Cyber failure', () => {
    expect(mergeUsageRecordLifecycleSnapshot({
      status: 'failed',
      status_code: 400,
      error_message: cyberMessage,
      updated_at: '2026-07-17T00:00:01Z',
    }, {
      status: 'completed',
      statusCode: 200,
      errorMessage: null,
      updatedAt: '2026-07-17T00:00:02Z',
    })).toEqual({
      status: 'completed',
      status_code: 200,
      error_message: undefined,
      updated_at: '2026-07-17T00:00:02Z',
      accepted: true,
    })
  })
})
