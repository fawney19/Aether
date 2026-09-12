import { beforeEach, describe, expect, it, vi } from 'vitest'

const { deleteMock, getMock, postMock } = vi.hoisted(() => ({
  deleteMock: vi.fn(),
  getMock: vi.fn(),
  postMock: vi.fn(),
}))

vi.mock('@/api/client', () => ({
  default: {
    get: getMock,
    post: postMock,
    delete: deleteMock,
  },
}))

import { vscodexApi } from '@/api/vscodex'

describe('vscodexApi', () => {
  beforeEach(() => {
    getMock.mockReset()
    postMock.mockReset()
    deleteMock.mockReset()
  })

  it('lists and normalizes the current user devices', async () => {
    getMock.mockResolvedValue({
      data: {
        devices: [
          {
            device_id: 'device-online',
            display_name: 'Studio Mac',
            connected: true,
            last_seen_at: '2026-08-31T10:00:00Z',
          },
          {
            id: 'device-unknown',
            name: 'Laptop',
            status: 'unexpected-status',
          },
          { name: 'missing id' },
        ],
      },
    })

    await expect(vscodexApi.listDevices()).resolves.toEqual([
      {
        id: 'device-online',
        name: 'Studio Mac',
        status: 'online',
        last_seen_at: '2026-08-31T10:00:00Z',
        created_at: null,
      },
      {
        id: 'device-unknown',
        name: 'Laptop',
        status: 'unknown',
        last_seen_at: null,
        created_at: null,
      },
    ])
    expect(getMock).toHaveBeenCalledWith('/api/users/me/vscodex/devices')
  })

  it('creates a pairing using an explicit empty body and normalizes its code', async () => {
    postMock.mockResolvedValue({
      data: {
        pairing_code: 'PAIR-1234',
        expires_in_seconds: 300,
      },
    })

    await expect(vscodexApi.createPairing()).resolves.toEqual({
      code: 'PAIR-1234',
      expires_at: null,
      expires_in_seconds: 300,
    })
    expect(postMock).toHaveBeenCalledWith('/api/users/me/vscodex/pairings', {})
  })

  it('requests a scoped WebSocket ticket for the selected device', async () => {
    postMock.mockResolvedValue({
      data: {
        ticket: 'single-use-ticket',
        wsUrl: 'wss://aether.example/api/vscodex/ws',
      },
    })

    await expect(vscodexApi.createWsTicket('device-online')).resolves.toEqual({
      ticket: 'single-use-ticket',
      ws_url: 'wss://aether.example/api/vscodex/ws',
      expires_at: null,
    })
    expect(postMock).toHaveBeenCalledWith('/api/users/me/vscodex/ws-tickets', {
      device_id: 'device-online',
    })
  })

  it('revokes the selected device using an encoded path segment', async () => {
    deleteMock.mockResolvedValue({ status: 204 })

    await expect(vscodexApi.deleteDevice('device/one')).resolves.toBeUndefined()
    expect(deleteMock).toHaveBeenCalledWith('/api/users/me/vscodex/devices/device%2Fone')
  })

  it('rejects incomplete pairing and ticket responses', async () => {
    postMock
      .mockResolvedValueOnce({ data: {} })
      .mockResolvedValueOnce({ data: { ticket: 'missing-url' } })

    await expect(vscodexApi.createPairing()).rejects.toThrow('Pairing response did not include a code')
    await expect(vscodexApi.createWsTicket('device-online')).rejects.toThrow(
      'WebSocket ticket response was incomplete',
    )
  })
})
