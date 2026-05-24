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
  turnstile_enabled: boolean
  turnstile_site_key: string | null
  turnstile_secret_key: string
  turnstile_secret_key_is_set: boolean
  turnstile_allowed_hostnames: string[]
  referral_enabled: boolean
  referral_reward_mode: string
  referral_recharge_percent: number
  referral_headcount_amount_usd: number
  referral_headcount_trigger: string
  registration_privacy_policy_enabled: boolean
  registration_privacy_policy_format: string
  registration_privacy_policy_content: string
  registration_privacy_policy_version: string
  // 独立余额 Key 过期管理
  auto_delete_expired_keys: boolean
  // 格式转换
  enable_format_conversion: boolean
  // 同步生图心跳
  enable_openai_image_sync_heartbeat: boolean
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
  proxy_node_metrics_1m_retention_days: number
  proxy_node_metrics_1h_retention_days: number
  proxy_node_metrics_cleanup_batch_size: number
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
  'turnstile_enabled',
  'turnstile_site_key',
  'turnstile_secret_key',
  'turnstile_allowed_hostnames',
  'referral_enabled',
  'referral_reward_mode',
  'referral_recharge_percent',
  'referral_headcount_amount_usd',
  'referral_headcount_trigger',
  'registration_privacy_policy_enabled',
  'registration_privacy_policy_format',
  'registration_privacy_policy_content',
  'registration_privacy_policy_version',
  // 独立余额 Key 过期管理
  'auto_delete_expired_keys',
  // 格式转换
  'enable_format_conversion',
  // 同步生图心跳
  'enable_openai_image_sync_heartbeat',
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
  'proxy_node_metrics_1m_retention_days',
  'proxy_node_metrics_1h_retention_days',
  'proxy_node_metrics_cleanup_batch_size',
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
    turnstile_enabled: false,
    turnstile_site_key: null,
    turnstile_secret_key: '',
    turnstile_secret_key_is_set: false,
    turnstile_allowed_hostnames: [],
    referral_enabled: false,
    referral_reward_mode: 'percent',
    referral_recharge_percent: 5,
    referral_headcount_amount_usd: 0,
    referral_headcount_trigger: 'registration',
    registration_privacy_policy_enabled: false,
    registration_privacy_policy_format: 'markdown',
    registration_privacy_policy_content: '',
    registration_privacy_policy_version: '1',
    // 独立余额 Key 过期管理
    auto_delete_expired_keys: false,
    // 格式转换
    enable_format_conversion: false,
    // 同步生图心跳
    enable_openai_image_sync_heartbeat: false,
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
    proxy_node_metrics_1m_retention_days: 30,
    proxy_node_metrics_1h_retention_days: 180,
    proxy_node_metrics_cleanup_batch_size: 5000,
    // 定时任务
    enable_provider_checkin: true,
    provider_checkin_time: '01:05',
    enable_oauth_token_refresh: true,
  }
}

export function useSystemConfig() {
  const { success, error } = useToast()
  const { refreshSiteInfo } = useSiteInfo()

  const systemConfig = ref<SystemConfig>(createDefaultConfig())
  const originalConfig = ref<SystemConfig | null>(null)
  const systemVersion = ref<string>('')
  const systemConfigLoading = ref(true)

  // 各模块 loading 状态
  const siteInfoLoading = ref(false)
  const proxyConfigLoading = ref(false)
  const basicConfigLoading = ref(false)
  const logConfigLoading = ref(false)
  const cleanupConfigLoading = ref(false)

  // 变动检测
  const hasSiteInfoChanges = computed(() => {
    if (systemConfigLoading.value) return false
    if (!originalConfig.value) return false
    return (
      systemConfig.value.site_name !== originalConfig.value.site_name ||
      systemConfig.value.site_subtitle !== originalConfig.value.site_subtitle
    )
  })

  const hasProxyConfigChanges = computed(() => {
    if (systemConfigLoading.value) return false
    if (!originalConfig.value) return false
    return systemConfig.value.system_proxy_node_id !== originalConfig.value.system_proxy_node_id
  })

  const hasBasicConfigChanges = computed(() => {
    if (systemConfigLoading.value) return false
    if (!originalConfig.value) return false
    return (
      systemConfig.value.default_user_initial_gift_usd !== originalConfig.value.default_user_initial_gift_usd ||
      systemConfig.value.rate_limit_per_minute !== originalConfig.value.rate_limit_per_minute ||
      systemConfig.value.enable_registration !== originalConfig.value.enable_registration ||
      systemConfig.value.password_policy_level !== originalConfig.value.password_policy_level ||
      systemConfig.value.turnstile_enabled !== originalConfig.value.turnstile_enabled ||
      systemConfig.value.turnstile_site_key !== originalConfig.value.turnstile_site_key ||
      systemConfig.value.turnstile_secret_key.trim() !== '' ||
      JSON.stringify(systemConfig.value.turnstile_allowed_hostnames) !==
      JSON.stringify(originalConfig.value.turnstile_allowed_hostnames) ||
      systemConfig.value.referral_enabled !== originalConfig.value.referral_enabled ||
      systemConfig.value.referral_reward_mode !== originalConfig.value.referral_reward_mode ||
      systemConfig.value.referral_recharge_percent !== originalConfig.value.referral_recharge_percent ||
      systemConfig.value.referral_headcount_amount_usd !== originalConfig.value.referral_headcount_amount_usd ||
      systemConfig.value.referral_headcount_trigger !== originalConfig.value.referral_headcount_trigger ||
      systemConfig.value.registration_privacy_policy_enabled !==
      originalConfig.value.registration_privacy_policy_enabled ||
      systemConfig.value.registration_privacy_policy_format !==
      originalConfig.value.registration_privacy_policy_format ||
      systemConfig.value.registration_privacy_policy_content !==
      originalConfig.value.registration_privacy_policy_content ||
      systemConfig.value.registration_privacy_policy_version !==
      originalConfig.value.registration_privacy_policy_version ||
      systemConfig.value.auto_delete_expired_keys !== originalConfig.value.auto_delete_expired_keys ||
      systemConfig.value.enable_format_conversion !== originalConfig.value.enable_format_conversion ||
      systemConfig.value.enable_openai_image_sync_heartbeat !== originalConfig.value.enable_openai_image_sync_heartbeat
    )
  })

  const hasLogConfigChanges = computed(() => {
    if (systemConfigLoading.value) return false
    if (!originalConfig.value) return false
    return (
      systemConfig.value.request_record_level !== originalConfig.value.request_record_level ||
      systemConfig.value.max_request_body_size !== originalConfig.value.max_request_body_size ||
      systemConfig.value.max_response_body_size !== originalConfig.value.max_response_body_size ||
      JSON.stringify(systemConfig.value.sensitive_headers) !==
      JSON.stringify(originalConfig.value.sensitive_headers)
    )
  })

  const hasCleanupConfigChanges = computed(() => {
    if (systemConfigLoading.value) return false
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
      originalConfig.value.request_candidates_cleanup_batch_size ||
      systemConfig.value.proxy_node_metrics_1m_retention_days !==
      originalConfig.value.proxy_node_metrics_1m_retention_days ||
      systemConfig.value.proxy_node_metrics_1h_retention_days !==
      originalConfig.value.proxy_node_metrics_1h_retention_days ||
      systemConfig.value.proxy_node_metrics_cleanup_batch_size !==
      originalConfig.value.proxy_node_metrics_cleanup_batch_size
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

  const turnstileAllowedHostnamesStr = computed({
    get: () => systemConfig.value.turnstile_allowed_hostnames.join(', '),
    set: (val: string) => {
      systemConfig.value.turnstile_allowed_hostnames = val
        .split(',')
        .map((s) => s.trim().toLowerCase())
        .filter((s) => s.length > 0)
    },
  })

  // 加载配置
  async function loadSystemConfig() {
    systemConfigLoading.value = true
    try {
      const results = await Promise.all(
        CONFIG_KEYS.map(async (key) => {
          try {
            return {
              key,
              response: await adminApi.getSystemConfig(key),
            }
          } catch {
            return null
          }
        })
      )

      const nextConfig = createDefaultConfig()
      for (const result of results) {
        if (!result) {
          continue
        }
        const { key, response } = result
        try {
          if (key === 'turnstile_secret_key') {
            nextConfig.turnstile_secret_key = ''
            nextConfig.turnstile_secret_key_is_set = !!response.is_set
            continue
          }
          if (response.value !== null && response.value !== undefined) {
            ; (nextConfig as Record<string, unknown>)[key] = response.value
          }
        } catch {
          // 单个配置项加载失败时忽略，使用默认值
        }
      }
      systemConfig.value = nextConfig
      originalConfig.value = JSON.parse(JSON.stringify(nextConfig))
    } catch (err) {
      error('加载系统配置失败')
      log.error('加载系统配置失败:', err)
    } finally {
      systemConfigLoading.value = false
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
          key: 'turnstile_enabled',
          value: systemConfig.value.turnstile_enabled,
          description: 'Cloudflare Turnstile 注册人机验证开关',
        },
        {
          key: 'turnstile_site_key',
          value: systemConfig.value.turnstile_site_key?.trim() || null,
          description: 'Cloudflare Turnstile 站点 Key',
        },
        {
          key: 'turnstile_allowed_hostnames',
          value: systemConfig.value.turnstile_allowed_hostnames,
          description: 'Cloudflare Turnstile 允许的 hostname 列表',
        },
        {
          key: 'referral_enabled',
          value: systemConfig.value.referral_enabled,
          description: '邀请返利开关',
        },
        {
          key: 'referral_reward_mode',
          value: systemConfig.value.referral_reward_mode,
          description: '邀请返利方式',
        },
        {
          key: 'referral_recharge_percent',
          value: systemConfig.value.referral_recharge_percent,
          description: '邀请充值比例返利百分比',
        },
        {
          key: 'referral_headcount_amount_usd',
          value: systemConfig.value.referral_headcount_amount_usd,
          description: '邀请人头返利金额（美元）',
        },
        {
          key: 'referral_headcount_trigger',
          value: systemConfig.value.referral_headcount_trigger,
          description: '邀请人头返利触发时机',
        },
        {
          key: 'registration_privacy_policy_enabled',
          value: systemConfig.value.registration_privacy_policy_enabled,
          description: '注册隐私政策确认开关',
        },
        {
          key: 'registration_privacy_policy_format',
          value: systemConfig.value.registration_privacy_policy_format,
          description: '注册隐私政策内容格式',
        },
        {
          key: 'registration_privacy_policy_content',
          value: systemConfig.value.registration_privacy_policy_content,
          description: '注册隐私政策内容',
        },
        {
          key: 'registration_privacy_policy_version',
          value: systemConfig.value.registration_privacy_policy_version,
          description: '注册隐私政策版本',
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
        {
          key: 'enable_openai_image_sync_heartbeat',
          value: systemConfig.value.enable_openai_image_sync_heartbeat,
          description: '同步生图心跳开关：开启后外层 HTTP 状态固定为 200，上游失败写入响应体',
        },
      ]
      const turnstileSecret = systemConfig.value.turnstile_secret_key.trim()
      if (turnstileSecret) {
        configItems.push({
          key: 'turnstile_secret_key',
          value: turnstileSecret,
          description: 'Cloudflare Turnstile Secret Key',
        })
      }

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
        originalConfig.value.turnstile_enabled = systemConfig.value.turnstile_enabled
        originalConfig.value.turnstile_site_key = systemConfig.value.turnstile_site_key?.trim() || null
        originalConfig.value.turnstile_allowed_hostnames = [
          ...systemConfig.value.turnstile_allowed_hostnames,
        ]
        originalConfig.value.referral_enabled = systemConfig.value.referral_enabled
        originalConfig.value.referral_reward_mode = systemConfig.value.referral_reward_mode
        originalConfig.value.referral_recharge_percent = systemConfig.value.referral_recharge_percent
        originalConfig.value.referral_headcount_amount_usd =
          systemConfig.value.referral_headcount_amount_usd
        originalConfig.value.referral_headcount_trigger =
          systemConfig.value.referral_headcount_trigger
        originalConfig.value.registration_privacy_policy_enabled =
          systemConfig.value.registration_privacy_policy_enabled
        originalConfig.value.registration_privacy_policy_format =
          systemConfig.value.registration_privacy_policy_format
        originalConfig.value.registration_privacy_policy_content =
          systemConfig.value.registration_privacy_policy_content
        originalConfig.value.registration_privacy_policy_version =
          systemConfig.value.registration_privacy_policy_version
        if (turnstileSecret) {
          systemConfig.value.turnstile_secret_key = ''
          systemConfig.value.turnstile_secret_key_is_set = true
          originalConfig.value.turnstile_secret_key = ''
          originalConfig.value.turnstile_secret_key_is_set = true
        }
        originalConfig.value.auto_delete_expired_keys =
          systemConfig.value.auto_delete_expired_keys
        originalConfig.value.enable_format_conversion =
          systemConfig.value.enable_format_conversion
        originalConfig.value.enable_openai_image_sync_heartbeat =
          systemConfig.value.enable_openai_image_sync_heartbeat
      }
      success('基础配置已保存')
    } catch (err) {
      error('保存配置失败')
      log.error('保存基础配置失败:', err)
    } finally {
      basicConfigLoading.value = false
    }
  }

  async function clearTurnstileSecret() {
    basicConfigLoading.value = true
    try {
      await adminApi.updateSystemConfig(
        'turnstile_secret_key',
        '',
        'Cloudflare Turnstile Secret Key'
      )
      systemConfig.value.turnstile_secret_key = ''
      systemConfig.value.turnstile_secret_key_is_set = false
      if (originalConfig.value) {
        originalConfig.value.turnstile_secret_key = ''
        originalConfig.value.turnstile_secret_key_is_set = false
      }
      success('Turnstile 密钥已清空')
    } catch (err) {
      error('清空 Turnstile 密钥失败')
      log.error('清空 Turnstile 密钥失败:', err)
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
        {
          key: 'proxy_node_metrics_1m_retention_days',
          value: systemConfig.value.proxy_node_metrics_1m_retention_days,
          description: '代理节点 1m 指标保留天数',
        },
        {
          key: 'proxy_node_metrics_1h_retention_days',
          value: systemConfig.value.proxy_node_metrics_1h_retention_days,
          description: '代理节点 1h 指标保留天数',
        },
        {
          key: 'proxy_node_metrics_cleanup_batch_size',
          value: systemConfig.value.proxy_node_metrics_cleanup_batch_size,
          description: '代理节点指标每批次清理条数',
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
        originalConfig.value.proxy_node_metrics_1m_retention_days =
          systemConfig.value.proxy_node_metrics_1m_retention_days
        originalConfig.value.proxy_node_metrics_1h_retention_days =
          systemConfig.value.proxy_node_metrics_1h_retention_days
        originalConfig.value.proxy_node_metrics_cleanup_batch_size =
          systemConfig.value.proxy_node_metrics_cleanup_batch_size
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
    systemConfigLoading,
    // loading 状态
    siteInfoLoading,
    proxyConfigLoading,
    basicConfigLoading,
    logConfigLoading,
    cleanupConfigLoading,
    // 变动检测
    hasSiteInfoChanges,
    hasProxyConfigChanges,
    hasBasicConfigChanges,
    hasLogConfigChanges,
    hasCleanupConfigChanges,
    // 计算属性
    maxRequestBodySizeKB,
    maxResponseBodySizeKB,
    sensitiveHeadersStr,
    turnstileAllowedHostnamesStr,
    // 加载函数
    loadSystemConfig,
    loadSystemVersion,
    // 保存函数
    saveSiteInfo,
    saveProxyConfig,
    saveBasicConfig,
    clearTurnstileSecret,
    saveLogConfig,
    saveCleanupConfig,
    handleAutoCleanupToggle,
  }
}
