import { computed, ref } from 'vue'
import { adminApi, type S3BackupScope } from '@/api/admin'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'
import { log } from '@/utils/logger'

export type S3BackupScheduleUnit = 'hours' | 'days' | 'weeks' | 'months'

export interface S3BackupConfig {
  enabled: boolean
  scope: S3BackupScope
  endpoint: string
  region: string
  userAgent: string
  bucket: string
  prefix: string
  accessKeyId: string
  secretAccessKey: string
  secretAccessKeyIsSet: boolean
  pathStyle: boolean
  compression: string
  scheduleUnit: S3BackupScheduleUnit
  scheduleInterval: number
  scheduleMinute: number
  scheduleHour: number
  scheduleWeekday: number
  scheduleMonthDay: number
  retentionCount: number
}

type ConfigField = Exclude<keyof S3BackupConfig, 'secretAccessKeyIsSet'>

const CONFIG_KEY_BY_FIELD: Record<ConfigField, string> = {
  enabled: 'backup_s3_enabled',
  scope: 'backup_s3_scope',
  endpoint: 'backup_s3_endpoint',
  region: 'backup_s3_region',
  userAgent: 'backup_s3_user_agent',
  bucket: 'backup_s3_bucket',
  prefix: 'backup_s3_prefix',
  accessKeyId: 'backup_s3_access_key_id',
  secretAccessKey: 'backup_s3_secret_access_key',
  pathStyle: 'backup_s3_path_style',
  compression: 'backup_s3_compression',
  scheduleUnit: 'backup_s3_schedule_unit',
  scheduleInterval: 'backup_s3_schedule_interval',
  scheduleMinute: 'backup_s3_schedule_minute',
  scheduleHour: 'backup_s3_schedule_hour',
  scheduleWeekday: 'backup_s3_schedule_weekday',
  scheduleMonthDay: 'backup_s3_schedule_month_day',
  retentionCount: 'backup_s3_retention_count',
}

const FIELD_BY_CONFIG_KEY = Object.fromEntries(
  Object.entries(CONFIG_KEY_BY_FIELD).map(([field, key]) => [key, field])
) as Record<string, ConfigField>

const CONFIG_KEYS = Object.values(CONFIG_KEY_BY_FIELD)
const VALID_SCOPES: S3BackupScope[] = ['config', 'users', 'data']
const VALID_UNITS: S3BackupScheduleUnit[] = ['hours', 'days', 'weeks', 'months']

function defaultS3BackupConfig(): S3BackupConfig {
  return {
    enabled: false,
    scope: 'data',
    endpoint: '',
    region: 'auto',
    userAgent: 'rclone/v1.68.0',
    bucket: '',
    prefix: 'aether/backups/',
    accessKeyId: '',
    secretAccessKey: '',
    secretAccessKeyIsSet: false,
    pathStyle: true,
    compression: 'zstd',
    scheduleUnit: 'days',
    scheduleInterval: 1,
    scheduleMinute: 0,
    scheduleHour: 3,
    scheduleWeekday: 1,
    scheduleMonthDay: 1,
    retentionCount: 7,
  }
}

function stringValue(value: unknown, fallback = ''): string {
  return typeof value === 'string' ? value : fallback
}

function numberValue(value: unknown, fallback: number): number {
  if (typeof value === 'number' && Number.isFinite(value)) return value
  if (typeof value === 'string' && value.trim() !== '') {
    const parsed = Number(value)
    if (Number.isFinite(parsed)) return parsed
  }
  return fallback
}

function booleanValue(value: unknown, fallback: boolean): boolean {
  if (typeof value === 'boolean') return value
  if (value === 'true') return true
  if (value === 'false') return false
  return fallback
}

function scopeValue(value: unknown, fallback: S3BackupScope): S3BackupScope {
  return typeof value === 'string' && VALID_SCOPES.includes(value as S3BackupScope)
    ? value as S3BackupScope
    : fallback
}

function scheduleUnitValue(
  value: unknown,
  fallback: S3BackupScheduleUnit,
): S3BackupScheduleUnit {
  return typeof value === 'string' && VALID_UNITS.includes(value as S3BackupScheduleUnit)
    ? value as S3BackupScheduleUnit
    : fallback
}

function cloneConfig(value: S3BackupConfig): S3BackupConfig {
  return JSON.parse(JSON.stringify(value)) as S3BackupConfig
}

export function useS3BackupConfig() {
  const { success, error } = useToast()
  const config = ref<S3BackupConfig>(defaultS3BackupConfig())
  const originalConfig = ref<S3BackupConfig | null>(null)
  const loading = ref(false)
  const saving = ref(false)
  const running = ref(false)
  const advancedOpen = ref(false)

  const hasChanges = computed(() => {
    if (!originalConfig.value) return false
    return JSON.stringify({
      ...config.value,
      secretAccessKey: config.value.secretAccessKey.trim(),
    }) !== JSON.stringify({
      ...originalConfig.value,
      secretAccessKey: '',
    })
  })

  async function loadS3BackupConfig() {
    loading.value = true
    try {
      const next = defaultS3BackupConfig()
      const configs = await adminApi.getAllSystemConfigs({ cacheTtlMs: 30_000 })
      const configsByKey = new Map(configs.map((item) => [item.key, item]))
      for (const key of CONFIG_KEYS) {
        const response = configsByKey.get(key)
        if (!response) continue
        const field = FIELD_BY_CONFIG_KEY[key]
        if (field === 'secretAccessKey') {
          next.secretAccessKey = ''
          next.secretAccessKeyIsSet = !!response.is_set
          continue
        }
        if (response.value === null || response.value === undefined) continue
        if (field === 'enabled' || field === 'pathStyle') {
          next[field] = booleanValue(response.value, next[field])
        } else if (
          field === 'scheduleInterval' ||
          field === 'scheduleMinute' ||
          field === 'scheduleHour' ||
          field === 'scheduleWeekday' ||
          field === 'scheduleMonthDay' ||
          field === 'retentionCount'
        ) {
          next[field] = numberValue(response.value, next[field])
        } else if (field === 'scope') {
          next.scope = scopeValue(response.value, next.scope)
        } else if (field === 'scheduleUnit') {
          next.scheduleUnit = scheduleUnitValue(response.value, next.scheduleUnit)
        } else {
          next[field] = stringValue(response.value, next[field] as string)
        }
      }
      config.value = next
      originalConfig.value = cloneConfig(next)
    } catch (err) {
      error('加载 S3 备份配置失败')
      log.error('加载 S3 备份配置失败:', err)
    } finally {
      loading.value = false
    }
  }

  async function saveS3BackupConfig() {
    saving.value = true
    try {
      const entries: Array<{ key: string; value: unknown; description: string }> = [
        { key: 'backup_s3_enabled', value: config.value.enabled, description: 'S3 自动备份开关' },
        { key: 'backup_s3_scope', value: config.value.scope, description: 'S3 备份范围' },
        { key: 'backup_s3_endpoint', value: config.value.endpoint.trim() || null, description: 'S3 Endpoint' },
        { key: 'backup_s3_region', value: config.value.region.trim() || 'auto', description: 'S3 Region' },
        { key: 'backup_s3_user_agent', value: config.value.userAgent.trim() || 'rclone/v1.68.0', description: 'S3 User-Agent' },
        { key: 'backup_s3_bucket', value: config.value.bucket.trim() || null, description: 'S3 Bucket' },
        { key: 'backup_s3_prefix', value: config.value.prefix.trim() || 'aether/backups/', description: 'S3 备份前缀' },
        { key: 'backup_s3_access_key_id', value: config.value.accessKeyId.trim() || null, description: 'S3 Access Key ID' },
        { key: 'backup_s3_path_style', value: config.value.pathStyle, description: 'S3 Path Style' },
        { key: 'backup_s3_compression', value: config.value.compression || 'zstd', description: 'S3 备份压缩格式' },
        { key: 'backup_s3_schedule_unit', value: config.value.scheduleUnit, description: 'S3 备份周期单位' },
        { key: 'backup_s3_schedule_interval', value: Math.max(1, Math.floor(config.value.scheduleInterval || 1)), description: 'S3 备份周期间隔' },
        { key: 'backup_s3_schedule_minute', value: Math.min(59, Math.max(0, Math.floor(config.value.scheduleMinute || 0))), description: 'S3 备份分钟' },
        { key: 'backup_s3_schedule_hour', value: Math.min(23, Math.max(0, Math.floor(config.value.scheduleHour || 0))), description: 'S3 备份小时' },
        { key: 'backup_s3_schedule_weekday', value: Math.min(7, Math.max(1, Math.floor(config.value.scheduleWeekday || 1))), description: 'S3 备份星期' },
        { key: 'backup_s3_schedule_month_day', value: Math.min(31, Math.max(1, Math.floor(config.value.scheduleMonthDay || 1))), description: 'S3 备份月日' },
        { key: 'backup_s3_retention_count', value: Math.max(1, Math.floor(config.value.retentionCount || 1)), description: 'S3 备份保留份数' },
      ]
      const secret = config.value.secretAccessKey.trim()
      if (secret) {
        entries.push({
          key: 'backup_s3_secret_access_key',
          value: secret,
          description: 'S3 Secret Access Key',
        })
      }
      for (const item of entries) {
        await adminApi.updateSystemConfig(item.key, item.value, item.description)
      }
      if (secret) {
        config.value.secretAccessKey = ''
        config.value.secretAccessKeyIsSet = true
      }
      originalConfig.value = cloneConfig(config.value)
      success('S3 备份配置已保存')
    } catch (err) {
      error(parseApiError(err, '保存 S3 备份配置失败'))
      log.error('保存 S3 备份配置失败:', err)
      await loadS3BackupConfig()
    } finally {
      saving.value = false
    }
  }

  async function clearS3SecretAccessKey() {
    saving.value = true
    try {
      await adminApi.updateSystemConfig(
        'backup_s3_secret_access_key',
        '',
        'S3 Secret Access Key',
      )
      config.value.secretAccessKey = ''
      config.value.secretAccessKeyIsSet = false
      if (originalConfig.value) {
        originalConfig.value.secretAccessKey = ''
        originalConfig.value.secretAccessKeyIsSet = false
      }
      success('S3 访问密钥已清除')
    } catch (err) {
      error(parseApiError(err, '清除 S3 访问密钥失败'))
      log.error('清除 S3 访问密钥失败:', err)
    } finally {
      saving.value = false
    }
  }

  async function runS3BackupNow() {
    running.value = true
    try {
      const response = await adminApi.runS3Backup()
      success(response.message || 'S3 备份任务已提交')
    } catch (err) {
      error(parseApiError(err, '提交 S3 备份任务失败'))
      log.error('提交 S3 备份任务失败:', err)
    } finally {
      running.value = false
    }
  }

  return {
    config,
    loading,
    saving,
    running,
    advancedOpen,
    hasChanges,
    loadS3BackupConfig,
    saveS3BackupConfig,
    clearS3SecretAccessKey,
    runS3BackupNow,
  }
}
