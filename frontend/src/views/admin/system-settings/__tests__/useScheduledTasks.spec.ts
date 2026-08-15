import { beforeEach, describe, expect, it, vi } from 'vitest'

const { errorMock, successMock, updateSystemConfigMock } = vi.hoisted(() => ({
  errorMock: vi.fn(),
  successMock: vi.fn(),
  updateSystemConfigMock: vi.fn(),
}))

vi.mock('@/api/admin', () => ({
  adminApi: {
    getAllSystemConfigs: vi.fn(),
    updateSystemConfig: updateSystemConfigMock,
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
    error: vi.fn(),
  },
}))

import { useScheduledTasks } from '../composables/useScheduledTasks'
import { useSystemConfig } from '../composables/useSystemConfig'

function remoteQuotaTask(tasks: ReturnType<typeof useScheduledTasks>) {
  const task = tasks.scheduledTasks.value.find(
    (candidate) => candidate.id === 'provider-remote-quota-sync'
  )
  if (!task || task.schedule?.kind !== 'interval') {
    throw new Error('remote quota interval task is missing')
  }
  return { task, schedule: task.schedule }
}

describe('useScheduledTasks remote quota sync', () => {
  beforeEach(() => {
    errorMock.mockReset()
    successMock.mockReset()
    updateSystemConfigMock.mockReset()
    updateSystemConfigMock.mockResolvedValue({})
  })

  it('uses the backward-compatible five-minute default', () => {
    const config = useSystemConfig().systemConfig
    const tasks = useScheduledTasks(config)
    tasks.initPreviousValues()

    const { task, schedule } = remoteQuotaTask(tasks)
    expect(task.enabled).toBe(true)
    expect(schedule.minutes).toBe(5)
    expect(schedule.minMinutes).toBe(1)
    expect(schedule.maxMinutes).toBe(60)
    expect(schedule.hasChanges).toBe(false)
  })

  it('stores an edited minute interval as bounded seconds', async () => {
    const config = useSystemConfig().systemConfig
    const tasks = useScheduledTasks(config)
    tasks.initPreviousValues()

    remoteQuotaTask(tasks).schedule.updateMinutes(10)
    expect(config.value.provider_remote_quota_sync_interval_seconds).toBe(600)
    expect(remoteQuotaTask(tasks).schedule.hasChanges).toBe(true)

    await remoteQuotaTask(tasks).schedule.onSave()

    expect(updateSystemConfigMock).toHaveBeenCalledWith(
      'provider_remote_quota_sync_interval_seconds',
      600,
      'Provider 远程额度自动同步间隔（秒）'
    )
    expect(remoteQuotaTask(tasks).schedule.hasChanges).toBe(false)
    expect(successMock).toHaveBeenCalledWith('远程额度同步间隔已设置为 10 分钟')
  })

  it('rejects non-integer and out-of-range minute intervals before the API call', async () => {
    const config = useSystemConfig().systemConfig
    const tasks = useScheduledTasks(config)
    tasks.initPreviousValues()

    for (const minutes of [0, 1.5, 61]) {
      updateSystemConfigMock.mockClear()
      errorMock.mockClear()
      remoteQuotaTask(tasks).schedule.updateMinutes(minutes)

      await remoteQuotaTask(tasks).schedule.onSave()

      expect(updateSystemConfigMock).not.toHaveBeenCalled()
      expect(errorMock).toHaveBeenCalledWith(
        '同步间隔必须是 1 到 60 之间的整数分钟'
      )
    }
  })

  it('persists the existing global kill switch and rolls back failed updates', async () => {
    const config = useSystemConfig().systemConfig
    const tasks = useScheduledTasks(config)
    tasks.initPreviousValues()

    await remoteQuotaTask(tasks).task.onToggle(false)
    expect(updateSystemConfigMock).toHaveBeenCalledWith(
      'enable_provider_remote_quota_sync',
      false,
      '是否允许 Provider 远程额度自动与手动同步应用'
    )
    expect(config.value.enable_provider_remote_quota_sync).toBe(false)

    updateSystemConfigMock.mockRejectedValueOnce(new Error('write failed'))
    await remoteQuotaTask(tasks).task.onToggle(true)
    expect(config.value.enable_provider_remote_quota_sync).toBe(false)
    expect(errorMock).toHaveBeenCalledWith('保存配置失败')
  })
})
