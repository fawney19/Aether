import { beforeEach, describe, expect, it, vi } from 'vitest'

const { getAllSystemConfigsMock, updateSystemConfigMock, updateReferralSettingsMock, fetchModulesMock, successMock, errorMock, logErrorMock } = vi.hoisted(() => ({
  getAllSystemConfigsMock: vi.fn(),
  updateSystemConfigMock: vi.fn(),
  updateReferralSettingsMock: vi.fn(),
  fetchModulesMock: vi.fn(),
  successMock: vi.fn(),
  errorMock: vi.fn(),
  logErrorMock: vi.fn(),
}))

vi.mock('@/api/admin', () => ({
  adminApi: {
    getAllSystemConfigs: getAllSystemConfigsMock,
    updateSystemConfig: updateSystemConfigMock,
    updateReferralSettings: updateReferralSettingsMock,
    getSystemVersion: vi.fn(),
  },
}))

vi.mock('@/composables/useToast', () => ({
  useToast: () => ({
    success: successMock,
    error: errorMock,
  }),
}))

vi.mock('@/composables/useSiteInfo', () => ({
  useSiteInfo: () => ({
    refreshSiteInfo: vi.fn(),
  }),
}))

vi.mock('@/utils/logger', () => ({
  log: {
    error: logErrorMock,
  },
}))

vi.mock('@/stores/modules', () => ({
  useModuleStore: () => ({ fetchModules: fetchModulesMock }),
}))

import { useSystemConfig } from '../composables/useSystemConfig'

describe('useSystemConfig', () => {
  beforeEach(() => {
    getAllSystemConfigsMock.mockReset()
    updateSystemConfigMock.mockReset()
    updateReferralSettingsMock.mockReset()
    fetchModulesMock.mockReset().mockResolvedValue({})
    successMock.mockReset()
    errorMock.mockReset()
    logErrorMock.mockReset()
  })

  it('loads config keys in one request and keeps change detection disabled until the baseline is ready', async () => {
    let resolveConfigs!: (value: Array<{ key: string, value: unknown, is_set?: boolean }>) => void
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

    resolveConfigs([
      { key: 'request_record_level', value: 'basic' },
      { key: 'proxy_node_metrics_cleanup_batch_size', value: 5000 },
    ])
    await loadPromise

    expect(state.systemConfigLoading.value).toBe(false)
    expect(state.systemConfig.value.request_record_level).toBe('basic')
    expect(state.hasLogConfigChanges.value).toBe(false)

    state.systemConfig.value.request_record_level = 'full'
    expect(state.hasLogConfigChanges.value).toBe(true)
  })

  it('uses backend-compatible defaults when config rows have not been persisted yet', async () => {
    getAllSystemConfigsMock.mockResolvedValue([])

    const state = useSystemConfig()
    await state.loadSystemConfig()

    expect(state.systemConfig.value.request_record_level).toBe('basic')
    expect(state.systemConfig.value.request_record_compact_base64_images).toBe(false)
    expect(state.systemConfig.value.request_record_max_body_size_kb).toBe(0)
    expect(state.systemConfig.value).not.toHaveProperty('max_request_body_size')
    expect(state.systemConfig.value).not.toHaveProperty('max_response_body_size')
  })

  it('saves only the proxy node and ignores retired DNS allowlist settings', async () => {
    getAllSystemConfigsMock.mockResolvedValue([
      { key: 'system_proxy_node_id', value: 'node-1' },
      { key: 'execution_extra_trusted_dns_hosts', value: ['custom.example.com'] },
    ])
    updateSystemConfigMock.mockResolvedValue(undefined)
    const state = useSystemConfig()
    await state.loadSystemConfig()
    expect(state.systemConfig.value).not.toHaveProperty('execution_extra_trusted_dns_hosts')
    state.systemConfig.value.system_proxy_node_id = 'node-2'
    expect(state.hasProxyConfigChanges.value).toBe(true)
    await state.saveProxyConfig()
    expect(updateSystemConfigMock).toHaveBeenCalledTimes(1)
    expect(updateSystemConfigMock).toHaveBeenCalledWith(
      'system_proxy_node_id', 'node-2', '系统默认代理节点 ID'
    )
    expect(state.hasProxyConfigChanges.value).toBe(false)
  })

  it('saves referral rules together without overwriting email verification or issuing individual referral writes', async () => {
    getAllSystemConfigsMock.mockResolvedValue([])
    updateReferralSettingsMock.mockResolvedValue(undefined)
    updateSystemConfigMock.mockResolvedValue(undefined)
    const state = useSystemConfig()
    await state.loadSystemConfig()
    state.systemConfig.value.referral_enabled = true
    state.systemConfig.value.referral_reward_mode = 'headcount'
    state.systemConfig.value.referral_headcount_trigger = 'email_verified'
    await state.saveBasicConfig()
    expect(updateReferralSettingsMock).toHaveBeenCalledExactlyOnceWith({
      referral_enabled: true, referral_reward_mode: 'headcount', referral_headcount_trigger: 'email_verified',
      referral_recharge_percent: 5, referral_headcount_amount_usd: 0,
    })
    expect(updateSystemConfigMock.mock.calls.map(([key]) => key).some(key => key.startsWith('referral_'))).toBe(false)
    expect(fetchModulesMock).toHaveBeenCalledOnce()
    expect(updateReferralSettingsMock.mock.invocationCallOrder[0]).toBeLessThan(fetchModulesMock.mock.invocationCallOrder[0])
    expect(fetchModulesMock.mock.invocationCallOrder[0]).toBeLessThan(updateSystemConfigMock.mock.invocationCallOrder[0])
    expect(state.hasBasicConfigChanges.value).toBe(false)
  })

  it('refreshes referral navigation even when a later basic setting fails to save', async () => {
    getAllSystemConfigsMock.mockResolvedValue([])
    updateReferralSettingsMock.mockResolvedValue(undefined)
    updateSystemConfigMock.mockRejectedValue(new Error('basic setting unavailable'))
    const state = useSystemConfig()
    await state.loadSystemConfig()
    state.systemConfig.value.referral_enabled = true
    await state.saveBasicConfig()
    expect(fetchModulesMock).toHaveBeenCalledOnce()
    expect(fetchModulesMock.mock.invocationCallOrder[0]).toBeLessThan(updateSystemConfigMock.mock.invocationCallOrder[0])
    expect(errorMock).toHaveBeenCalled()
    expect(state.hasBasicConfigChanges.value).toBe(true)
  })

  it('keeps a successful save when refreshing module navigation fails', async () => {
    getAllSystemConfigsMock.mockResolvedValue([])
    updateReferralSettingsMock.mockResolvedValue(undefined)
    updateSystemConfigMock.mockResolvedValue(undefined)
    const refreshError = new Error('module status unavailable')
    fetchModulesMock.mockRejectedValue(refreshError)
    const state = useSystemConfig()
    await state.loadSystemConfig()
    state.systemConfig.value.referral_enabled = true
    await state.saveBasicConfig()
    expect(logErrorMock).toHaveBeenCalledWith('返利配置已保存，刷新模块状态失败:', refreshError)
    expect(successMock).toHaveBeenCalledWith('基础配置已保存')
    expect(errorMock).not.toHaveBeenCalled()
    expect(state.hasBasicConfigChanges.value).toBe(false)
  })

  it('retains unsaved changes and avoids other writes when combined referral validation fails', async () => {
    getAllSystemConfigsMock.mockResolvedValue([])
    updateReferralSettingsMock.mockRejectedValue(new Error('email verification required'))
    const state = useSystemConfig()
    await state.loadSystemConfig()
    state.systemConfig.value.referral_enabled = true
    await state.saveBasicConfig()
    expect(updateSystemConfigMock).not.toHaveBeenCalled()
    expect(fetchModulesMock).not.toHaveBeenCalled()
    expect(state.hasBasicConfigChanges.value).toBe(true)
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
      '请求记录图片及大型审计载荷精简开关'
    )
    expect(updateSystemConfigMock).toHaveBeenCalledWith(
      'request_record_max_body_size_kb',
      256,
      '请求记录单个 Body 大小上限（KB），0 表示不限制'
    )
    expect(state.hasLogConfigChanges.value).toBe(false)
  })
})
