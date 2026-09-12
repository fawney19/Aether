import apiClient from '@/api/client'

const BASE_PATH = '/api/users/me/vscodex'

export type VscodexDeviceStatus = 'online' | 'offline' | 'connecting' | 'unknown'

export interface VscodexDevice {
  id: string
  name: string
  status: VscodexDeviceStatus
  last_seen_at: string | null
  created_at: string | null
}

export interface VscodexPairing {
  code: string
  expires_at: string | null
  expires_in_seconds: number | null
}

export interface VscodexWsTicket {
  ticket: string
  ws_url: string
  expires_at: string | null
}

type DevicePayload = Partial<VscodexDevice> & {
  device_id?: string
  display_name?: string
  connected?: boolean
}

type DevicesPayload = DevicePayload[] | {
  devices?: DevicePayload[]
  items?: DevicePayload[]
}

type PairingPayload = Partial<VscodexPairing> & {
  pairing_code?: string
}

type WsTicketPayload = Partial<VscodexWsTicket> & {
  wsUrl?: string
}

function normalizeStatus(device: DevicePayload): VscodexDeviceStatus {
  if (device.connected === true) return 'online'
  if (device.connected === false) return 'offline'

  switch (device.status) {
    case 'online':
    case 'offline':
    case 'connecting':
      return device.status
    default:
      return 'unknown'
  }
}

function normalizeDevice(device: DevicePayload): VscodexDevice | null {
  const id = device.id || device.device_id
  if (!id) return null

  return {
    id,
    name: device.name || device.display_name || id,
    status: normalizeStatus(device),
    last_seen_at: device.last_seen_at ?? null,
    created_at: device.created_at ?? null,
  }
}

export const vscodexApi = {
  async listDevices(): Promise<VscodexDevice[]> {
    const response = await apiClient.get<DevicesPayload>(`${BASE_PATH}/devices`)
    const payload = response.data
    const devices = Array.isArray(payload) ? payload : payload.devices ?? payload.items ?? []
    return devices.map(normalizeDevice).filter((device): device is VscodexDevice => device !== null)
  },

  async createPairing(): Promise<VscodexPairing> {
    const response = await apiClient.post<PairingPayload>(`${BASE_PATH}/pairings`, {})
    const code = response.data.code || response.data.pairing_code
    if (!code) throw new Error('Pairing response did not include a code')

    return {
      code,
      expires_at: response.data.expires_at ?? null,
      expires_in_seconds: response.data.expires_in_seconds ?? null,
    }
  },

  async createWsTicket(deviceId: string): Promise<VscodexWsTicket> {
    const response = await apiClient.post<WsTicketPayload>(`${BASE_PATH}/ws-tickets`, {
      device_id: deviceId,
    })
    const { ticket } = response.data
    const wsUrl = response.data.ws_url || response.data.wsUrl
    if (!ticket || !wsUrl) throw new Error('WebSocket ticket response was incomplete')

    return {
      ticket,
      ws_url: wsUrl,
      expires_at: response.data.expires_at ?? null,
    }
  },

  async deleteDevice(deviceId: string): Promise<void> {
    await apiClient.delete(`${BASE_PATH}/devices/${encodeURIComponent(deviceId)}`)
  },
}
