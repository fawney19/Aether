import { beforeEach, describe, expect, it, vi } from 'vitest'

const { getMock } = vi.hoisted(() => ({ getMock: vi.fn() }))
vi.mock('@/api/client', () => ({ default: { get: getMock } }))

import { dashboardApi } from '@/api/dashboard'
import { requestTraceApi } from '@/api/requestTrace'
import { cache } from '@/utils/cache'

beforeEach(() => {
  cache.clear()
  getMock.mockReset()
  getMock.mockResolvedValue({ data: { id: 'usage-1' } })
})

describe('dashboard body loading', () => {
  it('reports download progress so diagnostic exports can abort oversized bodies', async () => {
    const onProgress = vi.fn()
    getMock.mockResolvedValue({ data: new ArrayBuffer(0), headers: { 'x-aether-body-encoding': 'json', 'x-aether-usage-id': 'usage-1', 'x-aether-body-field': 'response_body' } })
    await dashboardApi.getRequestBody('usage-1', 'response_body', undefined, onProgress)
    getMock.mock.calls[0][1].onDownloadProgress({ loaded: 2 * 1024 * 1024 })
    expect(onProgress).toHaveBeenCalledWith(2 * 1024 * 1024)
  })

  it('records the gateway version at export without confusing it with failure-time metadata', async () => {
    getMock.mockResolvedValue({ data: { request_id: 'request-1', candidates: [] }, headers: { 'x-aether-build-version': 'test-build' } })
    expect(await requestTraceApi.getRequestTrace('request-1')).toMatchObject({ gateway_version: 'test-build' })
    getMock.mockResolvedValue({ data: { request_id: 'request-1', candidates: [] } })
    expect(await requestTraceApi.getRequestTrace('request-1')).toMatchObject({ gateway_version: null })
  })
  it('requests opaque body bytes, outside the JSON detail cache', async () => {
    const bytes = new ArrayBuffer(20)
    const controller = new AbortController()
    getMock.mockResolvedValue({ data: bytes, headers: { 'x-aether-body-encoding': 'gzip', 'x-aether-usage-id': 'usage-1', 'x-aether-body-field': 'request_body' } })
    await expect(dashboardApi.getRequestBody('usage-1', 'request_body', controller.signal)).resolves.toEqual({ bytes, encoding: 'gzip' })
    expect(getMock).toHaveBeenCalledWith('/api/admin/usage/usage-1', { params: { include_bodies: true, body_field: 'request_body', body_format: 'raw' }, responseType: 'arraybuffer', signal: controller.signal })
  })

  it.each([
    { 'x-aether-body-encoding': 'br', 'x-aether-usage-id': 'usage-1', 'x-aether-body-field': 'request_body' },
    { 'x-aether-body-encoding': 'gzip', 'x-aether-usage-id': 'usage-other', 'x-aether-body-field': 'request_body' },
    { 'x-aether-body-encoding': 'gzip', 'x-aether-usage-id': 'usage-1', 'x-aether-body-field': 'response_body' },
    {},
  ])('rejects mismatched or invalid body response headers', async headers => {
    getMock.mockResolvedValue({ data: new ArrayBuffer(10), headers })
    await expect(dashboardApi.getRequestBody('usage-1', 'request_body')).rejects.toThrow('Invalid body response')
  })

  it('defaults to shallow details and isolates cached records', async () => {
    await dashboardApi.getRequestDetail('usage-1')
    expect(getMock).toHaveBeenLastCalledWith('/api/admin/usage/usage-1', { params: { include_bodies: false } })
    await dashboardApi.getRequestDetail('usage-1', { cacheTtlMs: 5000 })
    await dashboardApi.getRequestDetail('usage-2', { cacheTtlMs: 5000 })
    await dashboardApi.getRequestDetail('usage-1', { cacheTtlMs: 5000 })
    expect(getMock).toHaveBeenCalledTimes(3)
    expect(getMock).toHaveBeenLastCalledWith('/api/admin/usage/usage-2', { params: { include_bodies: false } })
  })

  it('does not share abortable requests with another caller or a cancelled request', async () => {
    const first = new AbortController()
    const second = new AbortController()
    const firstRequest = dashboardApi.getRequestDetail('usage-1', { signal: first.signal })
    first.abort()
    const secondRequest = dashboardApi.getRequestDetail('usage-1', { signal: second.signal })
    await Promise.all([firstRequest, secondRequest])
    expect(getMock).toHaveBeenCalledTimes(2)
    expect(getMock).toHaveBeenLastCalledWith('/api/admin/usage/usage-1', {
      params: { include_bodies: false }, signal: second.signal,
    })
  })
})
