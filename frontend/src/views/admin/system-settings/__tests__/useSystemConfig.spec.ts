import { beforeEach, describe, expect, it, vi } from 'vitest'

const { getAllSystemConfigsMock, updateSystemConfigMock } = vi.hoisted(() => ({
  getAllSystemConfigsMock: vi.fn(),
  updateSystemConfigMock: vi.fn(),
}))

vi.mock('@/api/admin', () => ({
  adminApi: {
    getAllSystemConfigs: getAllSystemConfigsMock,
    updateSystemConfig: updateSystemConfigMock,
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

describe('useSystemConfig', () => {
  beforeEach(() => {
    getAllSystemConfigsMock.mockReset()
    updateSystemConfigMock.mockReset()
  })

  it('loads config keys in one request and keeps change detection disabled until the baseline is ready', async () => {
    let resolveConfigs: ((value: Array<{ key: string, value: unknown, is_set?: boolean }>) => void) | null = null
    getAllSystemConfigsMock.mockImplementation(() => new Promise((resolve) => {
      resolveConfigs = resolve
    }))

    const state = useSystemConfig()
    const loadPromise = state.loadSystemConfig()

    expect(getAllSystemConfigsMock).toHaveBeenCalledTimes(1)
    expect(getAllSystemConfigsMock).toHaveBeenCalledWith({ cacheTtlMs: 30_000 })

    state.systemConfig.value.request_record_level = 'headers'
    expect(state.systemConfigLoading.value).toBe(true)
    expect(state.hasLogConfigChanges.value).toBe(false)

    resolveConfigs?.([
      { key: 'request_record_level', value: 'basic' },
      { key: 'proxy_node_metrics_cleanup_batch_size', value: 5000 },
      { key: 'enable_standard_text_sync_heartbeat', value: false },
    ])
    await loadPromise

    expect(state.systemConfigLoading.value).toBe(false)
    expect(state.systemConfig.value.request_record_level).toBe('basic')
    expect(state.hasLogConfigChanges.value).toBe(false)

    state.systemConfig.value.request_record_level = 'full'
    expect(state.hasLogConfigChanges.value).toBe(true)
  })

  it('loads and saves the standard text sync heartbeat flag as a basic config item', async () => {
    getAllSystemConfigsMock.mockResolvedValue([
      { key: 'enable_standard_text_sync_heartbeat', value: false },
    ])
    updateSystemConfigMock.mockResolvedValue({})

    const state = useSystemConfig()
    await state.loadSystemConfig()

    expect(state.systemConfig.value.enable_standard_text_sync_heartbeat).toBe(false)
    state.systemConfig.value.enable_standard_text_sync_heartbeat = true
    expect(state.hasBasicConfigChanges.value).toBe(true)

    await state.saveBasicConfig()

    expect(updateSystemConfigMock).toHaveBeenCalledWith(
      'enable_standard_text_sync_heartbeat',
      true,
      '标准文本非流式心跳开关：开启后外层 HTTP 状态固定为 200，上游失败写入响应体'
    )
    expect(state.hasBasicConfigChanges.value).toBe(false)
  })

  it('keeps Cyber failover disabled by default and saves the enabled state', async () => {
    getAllSystemConfigsMock.mockResolvedValue([])
    updateSystemConfigMock.mockResolvedValue({})

    const state = useSystemConfig()
    await state.loadSystemConfig()

    expect(state.systemConfig.value.cyber_continue_failover).toBe(false)
    state.systemConfig.value.cyber_continue_failover = true
    expect(state.hasBasicConfigChanges.value).toBe(true)

    await state.saveBasicConfig()

    expect(updateSystemConfigMock).toHaveBeenCalledWith(
      'cyber_continue_failover',
      true,
      'Cyber继续转移开关：开启后在响应内容开始前将Cyber Policy错误按普通错误继续故障转移，可能增加首字等待时间'
    )
    expect(state.hasBasicConfigChanges.value).toBe(false)
  })

  it('uses backend-compatible defaults when config rows have not been persisted yet', async () => {
    getAllSystemConfigsMock.mockResolvedValue([])

    const state = useSystemConfig()
    await state.loadSystemConfig()

    expect(state.systemConfig.value.request_record_level).toBe('full')
    expect(state.systemConfig.value.request_record_compact_base64_images).toBe(false)
    expect(state.systemConfig.value.request_record_max_body_size_kb).toBe(0)
    expect(state.systemConfig.value).not.toHaveProperty('max_request_body_size')
    expect(state.systemConfig.value).not.toHaveProperty('max_response_body_size')
  })

  it('loads and saves request-record body storage controls', async () => {
    getAllSystemConfigsMock.mockResolvedValue([
      { key: 'request_record_compact_base64_images', value: true },
      { key: 'request_record_max_body_size_kb', value: 512 },
    ])
    updateSystemConfigMock.mockResolvedValue({})

    const state = useSystemConfig()
    await state.loadSystemConfig()

    expect(state.systemConfig.value.request_record_compact_base64_images).toBe(true)
    expect(state.systemConfig.value.request_record_max_body_size_kb).toBe(512)
    state.systemConfig.value.request_record_max_body_size_kb = 256
    expect(state.hasLogConfigChanges.value).toBe(true)

    await state.saveLogConfig()

    expect(updateSystemConfigMock).toHaveBeenCalledWith(
      'request_record_compact_base64_images',
      true,
      '请求记录 Base64 图片精简开关'
    )
    expect(updateSystemConfigMock).toHaveBeenCalledWith(
      'request_record_max_body_size_kb',
      256,
      '请求记录单个 Body 大小上限（KB），0 表示不限制'
    )
    expect(state.hasLogConfigChanges.value).toBe(false)
  })
})
