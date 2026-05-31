import { ref } from 'vue'
import type { ServerTimingMetadata } from '@/api/serverTiming'

const SERVER_CLOCK_RTT_REGRESSION_TOLERANCE_MS = 100

export function calculateServerClockOffsetMs(timing: ServerTimingMetadata | null | undefined): number | null {
  if (!timing) return null
  const {
    server_now_unix_ms: serverNowUnixMs,
    client_send_unix_ms: clientSendUnixMs,
    client_receive_unix_ms: clientReceiveUnixMs,
    round_trip_ms: roundTripMs,
  } = timing

  if (
    !Number.isFinite(serverNowUnixMs) ||
    !Number.isFinite(clientSendUnixMs) ||
    !Number.isFinite(clientReceiveUnixMs) ||
    !Number.isFinite(roundTripMs)
  ) {
    return null
  }
  if (clientReceiveUnixMs < clientSendUnixMs || roundTripMs < 0) {
    return null
  }

  return serverNowUnixMs - clientReceiveUnixMs
}

export function shouldUseServerClockSample(
  nextRoundTripMs: number,
  currentRoundTripMs: number | null | undefined
): boolean {
  if (!Number.isFinite(nextRoundTripMs) || nextRoundTripMs < 0) return false
  if (currentRoundTripMs == null || !Number.isFinite(currentRoundTripMs)) return true
  return nextRoundTripMs <= currentRoundTripMs + SERVER_CLOCK_RTT_REGRESSION_TOLERANCE_MS
}

export function useServerClock() {
  const serverClockOffsetMs = ref(0)
  const hasServerClockOffset = ref(false)
  const serverClockSampleRoundTripMs = ref<number | null>(null)

  function updateServerClockOffset(timing: ServerTimingMetadata | null | undefined): void {
    const offsetMs = calculateServerClockOffsetMs(timing)
    if (offsetMs == null) return
    if (!shouldUseServerClockSample(timing?.round_trip_ms ?? Number.NaN, serverClockSampleRoundTripMs.value)) {
      return
    }

    serverClockOffsetMs.value = offsetMs
    serverClockSampleRoundTripMs.value = timing?.round_trip_ms ?? null
    hasServerClockOffset.value = true
  }

  return {
    serverClockOffsetMs,
    hasServerClockOffset,
    serverClockSampleRoundTripMs,
    updateServerClockOffset,
  }
}
