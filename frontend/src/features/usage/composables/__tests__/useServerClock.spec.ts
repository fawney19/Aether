import { describe, expect, it } from 'vitest'
import {
  calculateServerClockOffsetMs,
  shouldUseServerClockSample,
  useServerClock
} from '../useServerClock'

describe('useServerClock', () => {
  it('calculates offset from the response receive time', () => {
    const offset = calculateServerClockOffsetMs({
      server_now_unix_ms: 10_500,
      client_send_unix_ms: 20_000,
      client_receive_unix_ms: 20_200,
      round_trip_ms: 200,
    })

    expect(offset).toBe(-9_700)
  })

  it('ignores missing or invalid timing samples', () => {
    expect(calculateServerClockOffsetMs(undefined)).toBeNull()
    expect(calculateServerClockOffsetMs({
      server_now_unix_ms: Number.NaN,
      client_send_unix_ms: 20_000,
      client_receive_unix_ms: 20_200,
      round_trip_ms: 200,
    })).toBeNull()
    expect(calculateServerClockOffsetMs({
      server_now_unix_ms: 10_500,
      client_send_unix_ms: 20_200,
      client_receive_unix_ms: 20_000,
      round_trip_ms: 200,
    })).toBeNull()
    expect(calculateServerClockOffsetMs({
      server_now_unix_ms: 10_500,
      client_send_unix_ms: 20_000,
      client_receive_unix_ms: 20_200,
      round_trip_ms: Number.NaN,
    })).toBeNull()
  })

  it('keeps the previous offset when a response has no server timing', () => {
    const clock = useServerClock()

    clock.updateServerClockOffset({
      server_now_unix_ms: 10_500,
      client_send_unix_ms: 20_000,
      client_receive_unix_ms: 20_200,
      round_trip_ms: 200,
    })
    clock.updateServerClockOffset(undefined)

    expect(clock.hasServerClockOffset.value).toBe(true)
    expect(clock.serverClockOffsetMs.value).toBe(-9_700)
    expect(clock.serverClockSampleRoundTripMs.value).toBe(200)
  })

  it('does not let a much slower sample overwrite a better clock offset', () => {
    const clock = useServerClock()

    clock.updateServerClockOffset({
      server_now_unix_ms: 10_500,
      client_send_unix_ms: 20_000,
      client_receive_unix_ms: 20_050,
      round_trip_ms: 50,
    })
    clock.updateServerClockOffset({
      server_now_unix_ms: 20_500,
      client_send_unix_ms: 30_000,
      client_receive_unix_ms: 30_500,
      round_trip_ms: 500,
    })

    expect(clock.serverClockOffsetMs.value).toBe(-9_550)
    expect(clock.serverClockSampleRoundTripMs.value).toBe(50)
  })

  it('accepts a faster sample after an initial slow sample', () => {
    const clock = useServerClock()

    clock.updateServerClockOffset({
      server_now_unix_ms: 10_500,
      client_send_unix_ms: 20_000,
      client_receive_unix_ms: 20_500,
      round_trip_ms: 500,
    })
    clock.updateServerClockOffset({
      server_now_unix_ms: 20_500,
      client_send_unix_ms: 30_000,
      client_receive_unix_ms: 30_050,
      round_trip_ms: 50,
    })

    expect(clock.serverClockOffsetMs.value).toBe(-9_550)
    expect(clock.serverClockSampleRoundTripMs.value).toBe(50)
  })

  it('allows small RTT regressions so the offset can stay fresh', () => {
    expect(shouldUseServerClockSample(140, 50)).toBe(true)
    expect(shouldUseServerClockSample(151, 50)).toBe(false)
  })
})
