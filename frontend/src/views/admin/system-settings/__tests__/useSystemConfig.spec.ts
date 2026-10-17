import { beforeEach, describe, expect, it, vi } from 'vitest'

const { getAllSystemConfigsMock, updateSystemConfigMock, updateReferralSettingsMock } = vi.hoisted(() => ({
  getAllSystemConfigsMock: vi.fn(),
  updateSystemConfigMock: vi.fn(),
  updateReferralSettingsMock: vi.fn(),
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
    updateReferralSettingsMock.mockReset()
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
    expect(state.hasBasicConfigChanges.value).toBe(true)
  })

})
