import { describe, expect, it } from 'vitest'
import {
  SERVER_NOW_UNIX_MS_HEADER,
  buildServerTimingMetadata,
  readServerNowUnixMsFromHeaders,
  withServerTiming,
} from '../serverTiming'

describe('serverTiming', () => {
  it('reads server time from response headers', () => {
    expect(readServerNowUnixMsFromHeaders({
      [SERVER_NOW_UNIX_MS_HEADER]: '1779999000123',
    })).toBe(1_779_999_000_123)
    expect(readServerNowUnixMsFromHeaders({
      'X-Aether-Server-Now-Unix-Ms': '1779999000456',
    })).toBe(1_779_999_000_456)
  })

  it('does not fall back to body fields', () => {
    const timing = buildServerTimingMetadata({
      headers: {},
      data: {
        server_now_unix_ms: 1_779_999_000_123,
      },
    }, 1_000, 1_100)

    expect(timing).toBeUndefined()
  })

  it('builds metadata with round trip duration', () => {
    const timing = buildServerTimingMetadata({
      headers: {
        [SERVER_NOW_UNIX_MS_HEADER]: '1050',
      },
    }, 1_000, 1_125)

    expect(timing).toEqual({
      server_now_unix_ms: 1_050,
      client_send_unix_ms: 1_000,
      client_receive_unix_ms: 1_125,
      round_trip_ms: 125,
    })
  })

  it('returns the original payload when the header is missing or invalid', () => {
    const payload = { records: [] }

    expect(withServerTiming({ data: payload, headers: {} }, 1_000)).toBe(payload)
    expect(withServerTiming({
      data: payload,
      headers: { [SERVER_NOW_UNIX_MS_HEADER]: 'not-a-number' },
    }, 1_000)).toBe(payload)
    expect(withServerTiming({
      data: payload,
      headers: { [SERVER_NOW_UNIX_MS_HEADER]: '0' },
    }, 1_000)).toBe(payload)
    expect(withServerTiming({
      data: payload,
      headers: { [SERVER_NOW_UNIX_MS_HEADER]: '1050.5' },
    }, 1_000)).toBe(payload)
  })
})
