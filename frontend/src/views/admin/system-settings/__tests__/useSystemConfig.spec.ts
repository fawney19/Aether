import { beforeEach, describe, expect, it, vi } from 'vitest'

const { getSystemConfigMock } = vi.hoisted(() => ({
  getSystemConfigMock: vi.fn(),
}))

vi.mock('@/api/admin', () => ({
  adminApi: {
    getSystemConfig: getSystemConfigMock,
    updateSystemConfig: vi.fn(),
    getSystemVersion: vi.fn(),
  },
}))

vi.mock('@/composables/useToast', () => ({
  useToast: () => ({
    success: vi.fn(),
    error: vi.fn(),
  }),
}))

vi.mock('@/composables/useSiteInfo', () => ({
  useSiteInfo: () => ({
    refreshSiteInfo: vi.fn(),
  }),
}))

vi.mock('@/utils/logger', () => ({
  log: {
    error: vi.fn(),
  },
}))

import { useSystemConfig } from '../composables/useSystemConfig'

interface DeferredConfigResponse {
  resolve: (value: { key: string, value: unknown, is_set?: boolean }) => void
}

describe('useSystemConfig', () => {
  beforeEach(() => {
    getSystemConfigMock.mockReset()
  })

  it('loads config keys in parallel and keeps change detection disabled until the baseline is ready', async () => {
    const pending = new Map<string, DeferredConfigResponse>()
    getSystemConfigMock.mockImplementation((key: string) => new Promise((resolve) => {
      pending.set(key, { resolve })
    }))

    const state = useSystemConfig()
    const loadPromise = state.loadSystemConfig()

    expect(getSystemConfigMock.mock.calls.map(([key]) => key)).toContain('request_record_level')
    expect(getSystemConfigMock.mock.calls.map(([key]) => key)).toContain('proxy_node_metrics_cleanup_batch_size')

    state.systemConfig.value.request_record_level = 'headers'
    expect(state.systemConfigLoading.value).toBe(true)
    expect(state.hasLogConfigChanges.value).toBe(false)

    for (const [key, deferred] of pending) {
      deferred.resolve({
        key,
        value: key === 'request_record_level' ? 'basic' : undefined,
        is_set: false,
      })
    }
    await loadPromise

    expect(state.systemConfigLoading.value).toBe(false)
    expect(state.systemConfig.value.request_record_level).toBe('basic')
    expect(state.hasLogConfigChanges.value).toBe(false)

    state.systemConfig.value.request_record_level = 'full'
    expect(state.hasLogConfigChanges.value).toBe(true)
  })
})
