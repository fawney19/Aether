import { describe, expect, it } from 'vitest'

import type { UsageRecord } from '../../types'
import { syncUsageRecordStreamResolution } from '../recordSync'

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
