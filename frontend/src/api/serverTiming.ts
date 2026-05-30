import type { AxiosResponse } from 'axios'

export const SERVER_NOW_UNIX_MS_HEADER = 'x-aether-server-now-unix-ms'

export interface ServerTimingMetadata {
  server_now_unix_ms: number
  client_send_unix_ms: number
  client_receive_unix_ms: number
  round_trip_ms: number
}

export interface ServerTimedPayload {
  server_timing?: ServerTimingMetadata
}

export function beginServerTimingSample(): number {
  return Date.now()
}

function readHeaderValue(headers: unknown, name: string): unknown {
  if (!headers || typeof headers !== 'object') return undefined

  const get = (headers as { get?: unknown }).get
  if (typeof get === 'function') {
    return get.call(headers, name)
  }

  const lowerName = name.toLowerCase()
  for (const [key, value] of Object.entries(headers as Record<string, unknown>)) {
    if (key.toLowerCase() === lowerName) return value
  }

  return undefined
}

export function readServerNowUnixMsFromHeaders(headers: unknown): number | null {
  const value = readHeaderValue(headers, SERVER_NOW_UNIX_MS_HEADER)
  const raw = Array.isArray(value) ? value[0] : value
  const parsed = typeof raw === 'number'
    ? raw
    : typeof raw === 'string'
      ? Number(raw.trim())
      : Number.NaN

  return Number.isSafeInteger(parsed) && parsed > 0 ? parsed : null
}

export function buildServerTimingMetadata(
  response: Pick<AxiosResponse, 'headers'> | { headers?: unknown } | null | undefined,
  clientSendUnixMs: number,
  clientReceiveUnixMs = Date.now()
): ServerTimingMetadata | undefined {
  const serverNowUnixMs = readServerNowUnixMsFromHeaders(response?.headers)
  if (serverNowUnixMs == null) return undefined
  if (!Number.isFinite(clientSendUnixMs) || !Number.isFinite(clientReceiveUnixMs)) return undefined
  if (clientReceiveUnixMs < clientSendUnixMs) return undefined

  const roundTripMs = clientReceiveUnixMs - clientSendUnixMs

  return {
    server_now_unix_ms: serverNowUnixMs,
    client_send_unix_ms: clientSendUnixMs,
    client_receive_unix_ms: clientReceiveUnixMs,
    round_trip_ms: roundTripMs,
  }
}

export function withServerTiming<T extends object>(
  response: Pick<AxiosResponse<T>, 'data' | 'headers'>,
  clientSendUnixMs: number
): T & ServerTimedPayload {
  const serverTiming = buildServerTimingMetadata(response, clientSendUnixMs)
  if (!serverTiming) return response.data
  return {
    ...response.data,
    server_timing: serverTiming,
  }
}
