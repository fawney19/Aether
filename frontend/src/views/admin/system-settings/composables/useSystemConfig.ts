import { ref, computed } from 'vue'
import { useToast } from '@/composables/useToast'
import { adminApi } from '@/api/admin'
import { log } from '@/utils/logger'
import { useSiteInfo } from '@/composables/useSiteInfo'

export interface SystemConfig {
  // 站点信息
  site_name: string
  site_subtitle: string
  // 网络代理
  system_proxy_node_id: string | null
  // 基础配置
  default_user_initial_gift_usd: number
  rate_limit_per_minute: number
  enable_registration: boolean
  password_policy_level: string
  // 独立余额 Key 过期管理
  auto_delete_expired_keys: boolean
  // 格式转换
  enable_format_conversion: boolean
  // 监控容量
  redis_memory_total_bytes: number
  postgres_storage_total_bytes: number
  // 请求记录
  request_record_level: string
  max_request_body_size: number
  max_response_body_size: number
  sensitive_headers: string[]
  // 请求记录清理
  enable_auto_cleanup: boolean
  detail_log_retention_days: number
  compressed_log_retention_days: number
  header_retention_days: number
  log_retention_days: number
  cleanup_batch_size: number
  audit_log_retention_days: number
  request_candidates_retention_days: number
  request_candidates_cleanup_batch_size: number
  // 定时任务
  enable_provider_checkin: boolean
  provider_checkin_time: string
  enable_oauth_token_refresh: boolean
}

const CONFIG_KEYS = [
  // 站点信息
  'site_name',
  'site_subtitle',
  // 网络代理
  'system_proxy_node_id',
  // 基础配置
  'default_user_initial_gift_usd',
  'rate_limit_per_minute',
  'enable_registration',
  'password_policy_level',
  // 独立余额 Key 过期管理
  'auto_delete_expired_keys',
  // 格式转换
  'enable_format_conversion',
  // 监控容量
  'redis_memory_total_bytes',
  'postgres_storage_total_bytes',
  // 请求记录
  'request_record_level',
  'max_request_body_size',
  'max_response_body_size',
  'sensitive_headers',
  // 请求记录清理
  'enable_auto_cleanup',
  'detail_log_retention_days',
  'compressed_log_retention_days',
  'header_retention_days',
  'log_retention_days',
  'cleanup_batch_size',
  'audit_log_retention_days',
  'request_candidates_retention_days',
  'request_candidates_cleanup_batch_size',
  // 定时任务
  'enable_provider_checkin',
  'provider_checkin_time',
  'enable_oauth_token_refresh',
]

function createDefaultConfig(): SystemConfig {
  return {
    // 站点信息
    site_name: 'Aether',
    site_subtitle: 'AI Gateway',
    // 网络代理
    system_proxy_node_id: null,
    // 基础配置
    default_user_initial_gift_usd: 10.0,
    rate_limit_per_minute: 0,
    enable_registration: false,
    password_policy_level: 'weak',
    // 独立余额 Key 过期管理
    auto_delete_expired_keys: false,
    // 格式转换
    enable_format_conversion: false,
    // 监控容量
    redis_memory_total_bytes: 0,
    postgres_storage_total_bytes: 0,
    // 请求记录
    request_record_level: 'basic',
    max_request_body_size: 1048576,
    max_response_body_size: 1048576,
    sensitive_headers: ['authorization', 'x-api-key', 'api-key', 'cookie', 'set-cookie'],
    // 请求记录清理
    enable_auto_cleanup: true,
    detail_log_retention_days: 7,
    compressed_log_retention_days: 30,
    header_retention_days: 90,
    log_retention_days: 365,
    cleanup_batch_size: 1000,
    audit_log_retention_days: 30,
    request_candidates_retention_days: 30,
    request_candidates_cleanup_batch_size: 5000,
    // 定时任务
    enable_provider_checkin: true,
    provider_checkin_time: '01:05',
    enable_oauth_token_refresh: true,
  }
}

const BYTES_PER_GB = 1024 * 1024 * 1024

function toBytesFromGB(value: number): number {
  if (!Number.isFinite(value) || value <= 0) return 0
  return Math.round(value * BYTES_PER_GB)
}

function toGBFromBytes(value: number): number {
  if (!Number.isFinite(value) || value <= 0) return 0
  return Math.round((value / BYTES_PER_GB) * 100) / 100
}

export function useSystemConfig() {
  const { success, error } = useToast()
  const { refreshSiteInfo } = useSiteInfo()

  const systemConfig = ref<SystemConfig>(createDefaultConfig())
  const originalConfig = ref<SystemConfig | null>(null)
  const systemVersion = ref<string>('')

  // 各模块 loading 状态
  const siteInfoLoading = ref(false)
  const proxyConfigLoading = ref(false)
  const basicConfigLoading = ref(false)
  const monitoringConfigLoading = ref(false)
  const logConfigLoading = ref(false)
  const cleanupConfigLoading = ref(false)

  // 变动检测
  const hasSiteInfoChanges = computed(() => {
    if (!originalConfig.value) return false
    return (
      systemConfig.value.site_name !== originalConfig.value.site_name ||
      systemConfig.value.site_subtitle !== originalConfig.value.site_subtitle
    )
  })

  const hasProxyConfigChanges = computed(() => {
    if (!originalConfig.value) return false
    return systemConfig.value.system_proxy_node_id !== originalConfig.value.system_proxy_node_id
  })

  const hasBasicConfigChanges = computed(() => {
    if (!originalConfig.value) return false
    return (
      systemConfig.value.default_user_initial_gift_usd !== originalConfig.value.default_user_initial_gift_usd ||
      systemConfig.value.rate_limit_per_minute !== originalConfig.value.rate_limit_per_minute ||
      systemConfig.value.enable_registration !== originalConfig.value.enable_registration ||
      systemConfig.value.password_policy_level !== originalConfig.value.password_policy_level ||
      systemConfig.value.auto_delete_expired_keys !== originalConfig.value.auto_delete_expired_keys ||
      systemConfig.value.enable_format_conversion !== originalConfig.value.enable_format_conversion
    )
  })

  const hasLogConfigChanges = computed(() => {
    if (!originalConfig.value) return false
    return (
      systemConfig.value.request_record_level !== originalConfig.value.request_record_level ||
      systemConfig.value.max_request_body_size !== originalConfig.value.max_request_body_size ||
      systemConfig.value.max_response_body_size !== originalConfig.value.max_response_body_size ||
      JSON.stringify(systemConfig.value.sensitive_headers) !==
      JSON.stringify(originalConfig.value.sensitive_headers)
    )
  })

  const hasMonitoringConfigChanges = computed(() => {
    if (!originalConfig.value) return false
    return (
      systemConfig.value.redis_memory_total_bytes !== originalConfig.value.redis_memory_total_bytes ||
      systemConfig.value.postgres_storage_total_bytes !== originalConfig.value.postgres_storage_total_bytes
    )
  })

  const hasCleanupConfigChanges = computed(() => {
    if (!originalConfig.value) return false
    return (
      systemConfig.value.detail_log_retention_days !==
      originalConfig.value.detail_log_retention_days ||
      systemConfig.value.compressed_log_retention_days !==
      originalConfig.value.compressed_log_retention_days ||
      systemConfig.value.header_retention_days !== originalConfig.value.header_retention_days ||
      systemConfig.value.log_retention_days !== originalConfig.value.log_retention_days ||
      systemConfig.value.cleanup_batch_size !== originalConfig.value.cleanup_batch_size ||
      systemConfig.value.audit_log_retention_days !==
      originalConfig.value.audit_log_retention_days ||
      systemConfig.value.request_candidates_retention_days !==
      originalConfig.value.request_candidates_retention_days ||
      systemConfig.value.request_candidates_cleanup_batch_size !==
      originalConfig.value.request_candidates_cleanup_batch_size
    )
  })

  // KB 和字节之间的转换
  const maxRequestBodySizeKB = computed({
    get: () => Math.round(systemConfig.value.max_request_body_size / 1024),
    set: (val: number) => {
      systemConfig.value.max_request_body_size = val * 1024
    },
  })

  const maxResponseBodySizeKB = computed({
    get: () => Math.round(systemConfig.value.max_response_body_size / 1024),
    set: (val: number) => {
      systemConfig.value.max_response_body_size = val * 1024
    },
  })

  const redisMemoryTotalGB = computed({
    get: () => toGBFromBytes(systemConfig.value.redis_memory_total_bytes),
    set: (val: number) => {
      systemConfig.value.redis_memory_total_bytes = toBytesFromGB(val)
    },
  })

  const postgresStorageTotalGB = computed({
    get: () => toGBFromBytes(systemConfig.value.postgres_storage_total_bytes),
    set: (val: number) => {
      systemConfig.value.postgres_storage_total_bytes = toBytesFromGB(val)
    },
  })

  // 敏感请求头数组和字符串之间的转换
  const sensitiveHeadersStr = computed({
    get: () => systemConfig.value.sensitive_headers.join(', '),
    set: (val: string) => {
      systemConfig.value.sensitive_headers = val
        .split(',')
        .map((s) => s.trim().toLowerCase())
        .filter((s) => s.length > 0)
    },
  })

  // 加载配置
  async function loadSystemConfig() {
    try {
      for (const key of CONFIG_KEYS) {
        try {
          const response = await adminApi.getSystemConfig(key)
          if (response.value !== null && response.value !== undefined) {
            ; (systemConfig.value as Record<string, unknown>)[key] = response.value
          }
        } catch {
          // 单个配置项加载失败时忽略，使用默认值
        }
      }
      originalConfig.value = JSON.parse(JSON.stringify(systemConfig.value))
    } catch (err) {
      error('加载系统配置失败')
      log.error('加载系统配置失败:', err)
    }
  }

  async function loadSystemVersion() {
    try {
      const data = await adminApi.getSystemVersion()
      systemVersion.value = data.version
    } catch (err) {
      log.error('加载系统版本失败:', err)
    }
  }

  // 保存函数
  async function saveSiteInfo() {
    siteInfoLoading.value = true
    try {
      const configItems = [
        { key: 'site_name', value: systemConfig.value.site_name, description: '站点名称' },
        {
          key: 'site_subtitle',
          value: systemConfig.value.site_subtitle,
          description: '站点副标题',
        },
      ]
      await Promise.all(
        configItems.map((item) =>
          adminApi.updateSystemConfig(item.key, item.value, item.description)
        )
      )
      if (originalConfig.value) {
        originalConfig.value.site_name = systemConfig.value.site_name
        originalConfig.value.site_subtitle = systemConfig.value.site_subtitle
      }
      await refreshSiteInfo()
      success('站点信息已保存')
    } catch (err) {
      error('保存站点信息失败')
      log.error('保存站点信息失败:', err)
    } finally {
      siteInfoLoading.value = false
    }
  }

  async function saveProxyConfig() {
    proxyConfigLoading.value = true
    try {
      await adminApi.updateSystemConfig(
        'system_proxy_node_id',
        systemConfig.value.system_proxy_node_id || null,
        '系统默认代理节点 ID'
      )
      if (originalConfig.value) {
        originalConfig.value.system_proxy_node_id = systemConfig.value.system_proxy_node_id
      }
      success('网络代理配置已保存')
    } catch (err) {
      error('保存代理配置失败')
      log.error('保存代理配置失败:', err)
    } finally {
      proxyConfigLoading.value = false
    }
  }

  async function saveBasicConfig() {
    basicConfigLoading.value = true
    try {
      const configItems = [
        {
          key: 'default_user_initial_gift_usd',
          value: systemConfig.value.default_user_initial_gift_usd,
          description: '默认用户初始赠款（美元）',
        },
        {
          key: 'rate_limit_per_minute',
          value: systemConfig.value.rate_limit_per_minute,
          description: '每分钟请求限制',
        },
        {
          key: 'enable_registration',
          value: systemConfig.value.enable_registration,
          description: '是否开放用户注册',
        },
        {
          key: 'password_policy_level',
          value: systemConfig.value.password_policy_level,
          description: '密码策略等级',
        },
        {
          key: 'auto_delete_expired_keys',
          value: systemConfig.value.auto_delete_expired_keys,
          description: '是否自动删除过期的API Key',
        },
        {
          key: 'enable_format_conversion',
          value: systemConfig.value.enable_format_conversion,
          description: '全局格式转换开关：开启时强制允许所有提供商的格式转换',
        },
      ]

      await Promise.all(
        configItems.map((item) =>
          adminApi.updateSystemConfig(item.key, item.value, item.description)
        )
      )
      if (originalConfig.value) {
        originalConfig.value.default_user_initial_gift_usd = systemConfig.value.default_user_initial_gift_usd
        originalConfig.value.rate_limit_per_minute = systemConfig.value.rate_limit_per_minute
        originalConfig.value.enable_registration = systemConfig.value.enable_registration
        originalConfig.value.password_policy_level = systemConfig.value.password_policy_level
        originalConfig.value.auto_delete_expired_keys =
          systemConfig.value.auto_delete_expired_keys
        originalConfig.value.enable_format_conversion =
          systemConfig.value.enable_format_conversion
      }
      success('基础配置已保存')
    } catch (err) {
      error('保存配置失败')
      log.error('保存基础配置失败:', err)
    } finally {
      basicConfigLoading.value = false
    }
  }

  async function saveLogConfig() {
    logConfigLoading.value = true
    try {
      const configItems = [
        {
          key: 'request_record_level',
          value: systemConfig.value.request_record_level,
          description: '请求记录级别',
        },
        {
          key: 'max_request_body_size',
          value: systemConfig.value.max_request_body_size,
          description: '最大请求体记录大小（字节）',
        },
        {
          key: 'max_response_body_size',
          value: systemConfig.value.max_response_body_size,
          description: '最大响应体记录大小（字节）',
        },
        {
          key: 'sensitive_headers',
          value: systemConfig.value.sensitive_headers,
          description: '敏感请求头列表',
        },
      ]

      await Promise.all(
        configItems.map((item) =>
          adminApi.updateSystemConfig(item.key, item.value, item.description)
        )
      )
      if (originalConfig.value) {
        originalConfig.value.request_record_level = systemConfig.value.request_record_level
        originalConfig.value.max_request_body_size = systemConfig.value.max_request_body_size
        originalConfig.value.max_response_body_size = systemConfig.value.max_response_body_size
        originalConfig.value.sensitive_headers = [...systemConfig.value.sensitive_headers]
      }
      success('请求记录配置已保存')
    } catch (err) {
      error('保存配置失败')
      log.error('保存请求记录配置失败:', err)
    } finally {
      logConfigLoading.value = false
    }
  }

  async function saveMonitoringConfig() {
    monitoringConfigLoading.value = true
    try {
      const configItems = [
        {
          key: 'redis_memory_total_bytes',
          value: systemConfig.value.redis_memory_total_bytes,
          description: 'Redis 总内存容量（字节）',
        },
        {
          key: 'postgres_storage_total_bytes',
          value: systemConfig.value.postgres_storage_total_bytes,
          description: 'PostgreSQL 总存储空间（字节）',
        },
      ]

      await Promise.all(
        configItems.map((item) =>
          adminApi.updateSystemConfig(item.key, item.value, item.description)
        )
      )
      if (originalConfig.value) {
        originalConfig.value.redis_memory_total_bytes = systemConfig.value.redis_memory_total_bytes
        originalConfig.value.postgres_storage_total_bytes = systemConfig.value.postgres_storage_total_bytes
      }
      success('监控容量配置已保存')
    } catch (err) {
      error('保存监控容量配置失败')
      log.error('保存监控容量配置失败:', err)
    } finally {
      monitoringConfigLoading.value = false
    }
  }

  async function saveCleanupConfig() {
    cleanupConfigLoading.value = true
    try {
      const configItems = [
        {
          key: 'detail_log_retention_days',
          value: systemConfig.value.detail_log_retention_days,
          description: '详细记录保留天数',
        },
        {
          key: 'compressed_log_retention_days',
          value: systemConfig.value.compressed_log_retention_days,
          description: '压缩记录保留天数',
        },
        {
          key: 'header_retention_days',
          value: systemConfig.value.header_retention_days,
          description: '请求头保留天数',
        },
        {
          key: 'log_retention_days',
          value: systemConfig.value.log_retention_days,
          description: '完整记录保留天数',
        },
        {
          key: 'cleanup_batch_size',
          value: systemConfig.value.cleanup_batch_size,
          description: '每批次清理的记录数',
        },
        {
          key: 'audit_log_retention_days',
          value: systemConfig.value.audit_log_retention_days,
          description: '审计日志保留天数',
        },
        {
          key: 'request_candidates_retention_days',
          value: systemConfig.value.request_candidates_retention_days,
          description: '请求候选记录保留天数',
        },
        {
          key: 'request_candidates_cleanup_batch_size',
          value: systemConfig.value.request_candidates_cleanup_batch_size,
          description: '请求候选记录每批次清理条数',
        },
      ]

      await Promise.all(
        configItems.map((item) =>
          adminApi.updateSystemConfig(item.key, item.value, item.description)
        )
      )
      if (originalConfig.value) {
        originalConfig.value.detail_log_retention_days =
          systemConfig.value.detail_log_retention_days
        originalConfig.value.compressed_log_retention_days =
          systemConfig.value.compressed_log_retention_days
        originalConfig.value.header_retention_days = systemConfig.value.header_retention_days
        originalConfig.value.log_retention_days = systemConfig.value.log_retention_days
        originalConfig.value.cleanup_batch_size = systemConfig.value.cleanup_batch_size
        originalConfig.value.audit_log_retention_days =
          systemConfig.value.audit_log_retention_days
        originalConfig.value.request_candidates_retention_days =
          systemConfig.value.request_candidates_retention_days
        originalConfig.value.request_candidates_cleanup_batch_size =
          systemConfig.value.request_candidates_cleanup_batch_size
      }
      success('请求记录清理配置已保存')
    } catch (err) {
      error('保存配置失败')
      log.error('保存请求记录清理配置失败:', err)
    } finally {
      cleanupConfigLoading.value = false
    }
  }

  async function handleAutoCleanupToggle(enabled: boolean) {
    const previousValue = systemConfig.value.enable_auto_cleanup
    systemConfig.value.enable_auto_cleanup = enabled
    try {
      await adminApi.updateSystemConfig(
        'enable_auto_cleanup',
        enabled,
        '是否启用自动清理任务'
      )
      success(enabled ? '已启用自动清理' : '已禁用自动清理')
    } catch (err) {
      error('保存配置失败')
      log.error('保存自动清理配置失败:', err)
      systemConfig.value.enable_auto_cleanup = previousValue
    }
  }

  return {
    systemConfig,
    originalConfig,
    systemVersion,
    // loading 状态
    siteInfoLoading,
    proxyConfigLoading,
    basicConfigLoading,
    monitoringConfigLoading,
    logConfigLoading,
    cleanupConfigLoading,
    // 变动检测
    hasSiteInfoChanges,
    hasProxyConfigChanges,
    hasBasicConfigChanges,
    hasMonitoringConfigChanges,
    hasLogConfigChanges,
    hasCleanupConfigChanges,
    // 计算属性
    maxRequestBodySizeKB,
    maxResponseBodySizeKB,
    redisMemoryTotalGB,
    postgresStorageTotalGB,
    sensitiveHeadersStr,
    // 加载函数
    loadSystemConfig,
    loadSystemVersion,
    // 保存函数
    saveSiteInfo,
    saveProxyConfig,
    saveBasicConfig,
    saveMonitoringConfig,
    saveLogConfig,
    saveCleanupConfig,
    handleAutoCleanupToggle,
  }
}
