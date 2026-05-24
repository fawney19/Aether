import { beforeEach, describe, expect, it, vi } from 'vitest'

const { getSystemConfigMock, updateSystemConfigMock, runS3BackupMock, errorMock, successMock } =
  vi.hoisted(() => ({
    getSystemConfigMock: vi.fn(),
    updateSystemConfigMock: vi.fn(),
    runS3BackupMock: vi.fn(),
    errorMock: vi.fn(),
    successMock: vi.fn(),
  }))

vi.mock('@/api/admin', () => ({
  adminApi: {
    getSystemConfig: getSystemConfigMock,
    updateSystemConfig: updateSystemConfigMock,
    runS3Backup: runS3BackupMock,
  },
}))

vi.mock('@/composables/useToast', () => ({
  useToast: () => ({
    error: errorMock,
    success: successMock,
  }),
}))

import { useS3BackupConfig } from '../composables/useS3BackupConfig'

describe('useS3BackupConfig', () => {
  beforeEach(() => {
    getSystemConfigMock.mockReset()
    updateSystemConfigMock.mockReset()
    runS3BackupMock.mockReset()
    errorMock.mockReset()
    successMock.mockReset()
  })

  it('loads write-only secret as configured without exposing the value', async () => {
    getSystemConfigMock.mockImplementation(async (key: string) => {
      if (key === 'backup_s3_secret_access_key') {
        return { key, value: null, is_set: true }
      }
      if (key === 'backup_s3_scope') {
        return { key, value: 'data' }
      }
      return { key, value: null }
    })

    const backup = useS3BackupConfig()
    await backup.loadS3BackupConfig()

    expect(backup.config.value.scope).toBe('data')
    expect(backup.config.value.secretAccessKey).toBe('')
    expect(backup.config.value.secretAccessKeyIsSet).toBe(true)
  })

  it('keeps an existing secret when saving with an empty secret field', async () => {
    getSystemConfigMock.mockImplementation(async (key: string) => {
      if (key === 'backup_s3_secret_access_key') {
        return { key, value: null, is_set: true }
      }
      return { key, value: null }
    })
    updateSystemConfigMock.mockResolvedValue({})

    const backup = useS3BackupConfig()
    await backup.loadS3BackupConfig()
    backup.config.value.bucket = 'aether-backups'
    await backup.saveS3BackupConfig()

    const savedKeys = updateSystemConfigMock.mock.calls.map(([key]) => key)
    expect(savedKeys).toContain('backup_s3_bucket')
    expect(savedKeys).not.toContain('backup_s3_secret_access_key')
    expect(backup.config.value.secretAccessKeyIsSet).toBe(true)
  })

  it('reloads server state when saving fails before writing a new secret', async () => {
    let loadRound = 0
    getSystemConfigMock.mockImplementation(async (key: string) => {
      if (key === 'backup_s3_secret_access_key') {
        return { key, value: null, is_set: false }
      }
      if (key === 'backup_s3_bucket') {
        return { key, value: loadRound === 0 ? 'old-bucket' : 'server-bucket' }
      }
      return { key, value: null }
    })
    updateSystemConfigMock.mockImplementation(async (key: string) => {
      if (key === 'backup_s3_bucket') {
        loadRound += 1
        throw new Error('save failed')
      }
      return {}
    })

    const backup = useS3BackupConfig()
    await backup.loadS3BackupConfig()
    backup.config.value.bucket = 'new-bucket'
    backup.config.value.secretAccessKey = 'new-secret'
    await backup.saveS3BackupConfig()

    const savedKeys = updateSystemConfigMock.mock.calls.map(([key]) => key)
    expect(savedKeys).toContain('backup_s3_bucket')
    expect(savedKeys).not.toContain('backup_s3_secret_access_key')
    expect(backup.config.value.bucket).toBe('server-bucket')
    expect(backup.config.value.secretAccessKey).toBe('')
  })
})
