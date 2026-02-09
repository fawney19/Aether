// API 格式常量
export const API_FORMATS = {
  // 新模式：endpoint signature key（family:kind，全小写）
  CLAUDE: 'claude:chat',
  CLAUDE_CLI: 'claude:cli',
  OPENAI: 'openai:chat',
  OPENAI_CLI: 'openai:cli',
  OPENAI_VIDEO: 'openai:video',
  GEMINI: 'gemini:chat',
  GEMINI_CLI: 'gemini:cli',
  GEMINI_VIDEO: 'gemini:video',
} as const

export type APIFormat = typeof API_FORMATS[keyof typeof API_FORMATS]

// API 格式显示名称映射（按品牌分组：Chat 在前，CLI/Video 在后）
export const API_FORMAT_LABELS: Record<string, string> = {
  [API_FORMATS.CLAUDE]: 'Claude Chat',
  [API_FORMATS.CLAUDE_CLI]: 'Claude CLI',
  [API_FORMATS.OPENAI]: 'OpenAI Chat',
  [API_FORMATS.OPENAI_CLI]: 'OpenAI CLI',
  [API_FORMATS.OPENAI_VIDEO]: 'OpenAI Video',
  [API_FORMATS.GEMINI]: 'Gemini Chat',
  [API_FORMATS.GEMINI_CLI]: 'Gemini CLI',
  [API_FORMATS.GEMINI_VIDEO]: 'Gemini Video',
  // legacy 兼容（仅用于展示历史数据）
  CLAUDE: 'Claude Chat',
  CLAUDE_CLI: 'Claude CLI',
  OPENAI: 'OpenAI Chat',
  OPENAI_CLI: 'OpenAI CLI',
  OPENAI_VIDEO: 'OpenAI Video',
  GEMINI: 'Gemini Chat',
  GEMINI_CLI: 'Gemini CLI',
  GEMINI_VIDEO: 'Gemini Video',
}

// API 格式缩写映射（用于空间紧凑的显示场景）
export const API_FORMAT_SHORT: Record<string, string> = {
  [API_FORMATS.OPENAI]: 'O',
  [API_FORMATS.OPENAI_CLI]: 'OC',
  [API_FORMATS.OPENAI_VIDEO]: 'OV',
  [API_FORMATS.CLAUDE]: 'C',
  [API_FORMATS.CLAUDE_CLI]: 'CC',
  [API_FORMATS.GEMINI]: 'G',
  [API_FORMATS.GEMINI_CLI]: 'GC',
  [API_FORMATS.GEMINI_VIDEO]: 'GV',
  // legacy 兼容（仅用于展示历史数据）
  OPENAI: 'O',
  OPENAI_CLI: 'OC',
  OPENAI_VIDEO: 'OV',
  CLAUDE: 'C',
  CLAUDE_CLI: 'CC',
  GEMINI: 'G',
  GEMINI_CLI: 'GC',
  GEMINI_VIDEO: 'GV',
}

// API 格式排序顺序（统一的显示顺序）
export const API_FORMAT_ORDER: string[] = [
  API_FORMATS.OPENAI,
  API_FORMATS.OPENAI_CLI,
  API_FORMATS.OPENAI_VIDEO,
  API_FORMATS.CLAUDE,
  API_FORMATS.CLAUDE_CLI,
  API_FORMATS.GEMINI,
  API_FORMATS.GEMINI_CLI,
  API_FORMATS.GEMINI_VIDEO,
]

// 工具函数：按标准顺序排序 API 格式数组
export function sortApiFormats(formats: string[]): string[] {
  return [...formats].sort((a, b) => {
    const aIdx = API_FORMAT_ORDER.indexOf(a)
    const bIdx = API_FORMAT_ORDER.indexOf(b)
    if (aIdx === -1 && bIdx === -1) return 0
    if (aIdx === -1) return 1
    if (bIdx === -1) return -1
    return aIdx - bIdx
  })
}

/**
 * 代理配置类型
 * 支持两种模式：
 * - 手动配置：设置 url/username/password
 * - 代理节点：设置 node_id（与 url 互斥）
 */
export interface ProxyConfig {
  url?: string
  username?: string
  password?: string
  node_id?: string    // 代理节点 ID（aether-proxy 注册的节点，与 url 互斥）
  enabled?: boolean   // 是否启用代理（false 时保留配置但不使用）
}

/**
 * 请求头规则类型
 * - set: 设置/覆盖请求头
 * - drop: 删除请求头
 * - rename: 重命名请求头（保留原值）
 */
export interface HeaderRuleSet {
  action: 'set'
  key: string
  value: string
}

export interface HeaderRuleDrop {
  action: 'drop'
  key: string
}

export interface HeaderRuleRename {
  action: 'rename'
  from: string
  to: string
}

export type HeaderRule = HeaderRuleSet | HeaderRuleDrop | HeaderRuleRename

/**
 * 请求体规则类型
 * - set: 设置/覆盖字段
 * - drop: 删除字段
 * - rename: 重命名字段（保留原值）
 */
/**
 * 请求体规则 - 覆写字段
 *
 * - path 支持嵌套路径，如 "metadata.user.name"
 * - 使用 "\." 转义字面量点号，如 "config\.v1.enabled"
 */
export interface BodyRuleSet {
  action: 'set'
  path: string
  value: any
}

/**
 * 请求体规则 - 删除字段
 *
 * - path 支持嵌套路径，如 "metadata.internal_flag"
 * - 使用 "\." 转义字面量点号，如 "config\.v1.enabled"
 */
export interface BodyRuleDrop {
  action: 'drop'
  path: string
}

/**
 * 请求体规则 - 重命名/移动字段
 *
 * - from/to 支持嵌套路径，如 "extra.old_config" -> "settings.new_config"
 * - 使用 "\." 转义字面量点号，如 "config\.v1.enabled"
 */
export interface BodyRuleRename {
  action: 'rename'
  from: string
  to: string
}

/**
 * 请求体规则 - 向数组追加元素
 *
 * - path 指向目标数组，如 "messages"
 * - value 为要追加的元素
 */
export interface BodyRuleAppend {
  action: 'append'
  path: string
  value: any
}

/**
 * 请求体规则 - 在数组指定位置插入元素
 *
 * - path 指向目标数组，如 "messages"
 * - index 为插入位置（支持负数）
 * - value 为要插入的元素
 */
export interface BodyRuleInsert {
  action: 'insert'
  path: string
  index: number
  value: any
}

/**
 * 请求体规则 - 正则替换字符串值
 *
 * - path 指向目标字符串字段，如 "messages[0].content"
 * - pattern 为正则表达式
 * - replacement 为替换字符串
 * - flags 可选，支持 i(忽略大小写)/m(多行)/s(dotall)
 * - count 替换次数，0=全部替换（默认）
 */
export interface BodyRuleRegexReplace {
  action: 'regex_replace'
  path: string
  pattern: string
  replacement: string
  flags?: string
  count?: number
}

export type BodyRule = BodyRuleSet | BodyRuleDrop | BodyRuleRename | BodyRuleAppend | BodyRuleInsert | BodyRuleRegexReplace

/**
 * 格式接受策略配置
 * 用于控制端点是否接受来自不同 API 格式的请求，并自动进行格式转换
 */
export interface FormatAcceptanceConfig {
  enabled: boolean                // 是否启用格式转换
  accept_formats?: string[]       // 白名单：接受哪些格式的请求
  reject_formats?: string[]       // 黑名单：拒绝哪些格式（优先级高于白名单）
}

export interface ProviderEndpoint {
  id: string
  provider_id: string
  provider_name: string
  api_format: string
  base_url: string
  custom_path?: string  // 自定义请求路径（可选，为空则使用 API 格式默认路径）
  // 请求头配置
  header_rules?: HeaderRule[]  // 请求头规则列表，支持 set/drop/rename 操作
  // 请求体配置
  body_rules?: BodyRule[]  // 请求体规则列表，支持 set/drop/rename 操作
  max_retries: number
  is_active: boolean
  config?: Record<string, any>
  proxy?: ProxyConfig | null
  // 格式转换配置
  format_acceptance_config?: FormatAcceptanceConfig | null
  total_keys: number
  active_keys: number
  created_at: string
  updated_at: string
}

/**
 * 模型权限配置类型
 *
 * 使用示例：
 * 1. 不限制（允许所有模型）: null
 * 2. 白名单模式: ["gpt-4", "claude-3-opus"]
 */
export type AllowedModels = string[] | null

// AllowedModels 类型守卫函数
export function isAllowedModelsList(value: AllowedModels): value is string[] {
  return Array.isArray(value)
}

export interface EndpointAPIKey {
  id: string
  provider_id: string
  api_formats: string[]  // 支持的 endpoint signature 列表（如 "openai:chat"）
  api_key_masked: string
  api_key_plain?: string | null
  auth_type: 'api_key' | 'vertex_ai' | 'oauth'  // 认证类型（必返回）
  name: string  // 密钥名称（必填，用于识别）
  rate_multipliers?: Record<string, number> | null  // 按 endpoint signature 的成本倍率
  internal_priority: number  // Key 内部优先级
  global_priority_by_format?: Record<string, number> | null  // 按 endpoint signature 的全局优先级
  rpm_limit?: number | null  // RPM 速率限制 (1-10000)，null 表示自适应模式
  allowed_models?: AllowedModels  // 允许使用的模型列表（null=不限制）
  capabilities?: Record<string, boolean> | null  // 能力标签配置（如 cache_1h, context_1m）
  // 缓存与熔断配置
  cache_ttl_minutes: number  // 缓存 TTL（分钟），0=禁用
  max_probe_interval_minutes: number  // 熔断探测间隔（分钟）
  // 按 endpoint signature 的健康度数据
  health_by_format?: Record<string, FormatHealthData>
  circuit_breaker_by_format?: Record<string, FormatCircuitBreakerData>
  // 聚合字段（从 health_by_format 计算，用于列表显示）
  health_score: number
  circuit_breaker_open?: boolean
  consecutive_failures: number
  last_failure_at?: string
  request_count: number
  success_count: number
  error_count: number
  success_rate: number
  avg_response_time_ms: number
  is_active: boolean
  note?: string  // 备注说明（可选）
  last_used_at?: string
  created_at: string
  updated_at: string
  // 自适应 RPM 字段
  is_adaptive?: boolean  // 是否为自适应模式（rpm_limit=NULL）
  effective_limit?: number | null  // 当前有效 RPM 限制（自适应使用学习值，固定使用配置值，未学习时为 null）
  learned_rpm_limit?: number | null  // 学习到的 RPM 限制
  // 滑动窗口利用率采样
  utilization_samples?: Array<{ ts: number; util: number }>  // 利用率采样窗口
  last_probe_increase_at?: string  // 上次探测性扩容时间
  concurrent_429_count?: number
  rpm_429_count?: number
  last_429_at?: string
  last_429_type?: string
  // 单格式场景的熔断器字段
  circuit_breaker_open_at?: string
  next_probe_at?: string
  half_open_until?: string
  half_open_successes?: number
  half_open_failures?: number
  request_results_window?: Array<{ ts: number; ok: boolean }>  // 请求结果滑动窗口
  // 自动获取模型
  auto_fetch_models?: boolean  // 是否启用自动获取模型
  last_models_fetch_at?: string  // 最后获取模型时间
  last_models_fetch_error?: string  // 最后获取模型错误信息
  locked_models?: string[]  // 被锁定的模型列表
  // 模型过滤规则（仅当 auto_fetch_models=true 时生效）
  model_include_patterns?: string[]  // 模型包含规则（支持 * 和 ? 通配符）
  model_exclude_patterns?: string[]  // 模型排除规则（支持 * 和 ? 通配符）
  // OAuth 相关
  oauth_expires_at?: number | null  // OAuth Token 过期时间（Unix 时间戳）
  oauth_email?: string | null  // OAuth 授权的邮箱
  oauth_plan_type?: string | null  // Codex 订阅类型: plus/free/team/enterprise
  oauth_account_id?: string | null  // Codex ChatGPT 账号 ID
  oauth_invalid_at?: number | null  // OAuth Token 失效时间（Unix 时间戳）
  oauth_invalid_reason?: string | null  // OAuth Token 失效原因
  // 上游元数据（由上游响应采集，如 Codex 额度信息 / Antigravity 配额信息）
  upstream_metadata?: UpstreamMetadata | null
  // Key 级别代理配置（覆盖 Provider 级别代理）
  proxy?: ProxyConfig | null
}

// Codex 上游元数据类型
export interface CodexUpstreamMetadata {
  updated_at?: number  // 更新时间（Unix 时间戳）
  plan_type?: string  // 套餐类型
  primary_used_percent?: number  // 周限额窗口使用百分比
  primary_reset_seconds?: number  // 周限额重置剩余秒数
  primary_reset_at?: number  // 周限额重置时间（Unix 时间戳）
  primary_window_minutes?: number  // 周限额窗口大小（分钟）
  secondary_used_percent?: number  // 5H限额窗口使用百分比
  secondary_reset_seconds?: number  // 5H限额重置剩余秒数
  secondary_reset_at?: number  // 5H限额重置时间（Unix 时间戳）
  secondary_window_minutes?: number  // 5H限额窗口大小（分钟）
  code_review_used_percent?: number  // 代码审查限额使用百分比
  code_review_reset_seconds?: number  // 代码审查限额重置剩余秒数
  code_review_reset_at?: number  // 代码审查限额重置时间（Unix 时间戳）
  code_review_window_minutes?: number  // 代码审查限额窗口大小（分钟）
  has_credits?: boolean  // 是否有积分
  credits_balance?: number  // 积分余额
}

export interface AntigravityModelQuota {
  remaining_fraction: number  // 剩余比例 (0.0-1.0)
  used_percent: number  // 已用百分比 (0.0-100.0)
  reset_time?: string  // RFC3339
}

export interface AntigravityUpstreamMetadata {
  updated_at?: number  // Unix 时间戳（秒）
  quota_by_model?: Record<string, AntigravityModelQuota>
  is_forbidden?: boolean  // 账户是否被禁止访问
  forbidden_reason?: string  // 禁止访问原因
  forbidden_at?: number  // 禁止时间（Unix 时间戳，秒）
}

// Kiro 上游配额信息
export interface KiroUpstreamMetadata {
  subscription_title?: string  // 订阅类型 (如 "KIRO PRO+")
  current_usage?: number  // 当前使用量
  usage_limit?: number  // 使用限额
  remaining?: number  // 剩余额度
  usage_percentage?: number  // 使用百分比 (0-100)
  next_reset_at?: number  // 下次重置时间（Unix 时间戳，毫秒）
  email?: string  // 用户邮箱
  updated_at?: number  // Unix 时间戳（秒）
  is_banned?: boolean  // 账户是否被封禁
  ban_reason?: string  // 封禁原因
  banned_at?: number  // 封禁时间（Unix 时间戳，秒）
}

export interface UpstreamMetadata {
  codex?: CodexUpstreamMetadata
  antigravity?: AntigravityUpstreamMetadata
  kiro?: KiroUpstreamMetadata
}

// 按格式的健康度数据
export interface FormatHealthData {
  health_score: number
  error_rate: number
  window_size: number
  consecutive_failures: number
  last_failure_at?: string | null
  circuit_breaker: FormatCircuitBreakerData
}

// 按格式的熔断器数据
export interface FormatCircuitBreakerData {
  open: boolean
  open_at?: string | null
  next_probe_at?: string | null
  half_open_until?: string | null
  half_open_successes: number
  half_open_failures: number
}

export interface EndpointAPIKeyUpdate {
  api_formats?: string[]  // 支持的 API 格式列表
  name?: string
  api_key?: string  // 仅在需要更新时提供
  auth_type?: 'api_key' | 'vertex_ai' | 'oauth'  // 认证类型
  auth_config?: Record<string, any>  // 认证配置（Vertex AI Service Account JSON）
  rate_multipliers?: Record<string, number> | null  // 按 API 格式的成本倍率
  internal_priority?: number
  global_priority_by_format?: Record<string, number> | null  // 按 API 格式的全局优先级
  rpm_limit?: number | null  // RPM 速率限制 (1-10000)，null 表示切换为自适应模式
  allowed_models?: AllowedModels
  capabilities?: Record<string, boolean> | null
  cache_ttl_minutes?: number
  max_probe_interval_minutes?: number
  note?: string
  is_active?: boolean
  auto_fetch_models?: boolean  // 是否启用自动获取模型
  locked_models?: string[]  // 被锁定的模型列表
  // 模型过滤规则（仅当 auto_fetch_models=true 时生效）
  model_include_patterns?: string[]  // 模型包含规则（支持 * 和 ? 通配符）
  model_exclude_patterns?: string[]  // 模型排除规则（支持 * 和 ? 通配符）
  // Key 级别代理配置（覆盖 Provider 级别代理），null=清除
  proxy?: ProxyConfig | null
}

export interface EndpointHealthDetail {
  api_format: string
  health_score: number
  is_active: boolean
  total_keys?: number
  active_keys?: number
}

export interface EndpointHealthEvent {
  timestamp: string
  status: 'success' | 'failed' | 'skipped' | 'started'
  status_code?: number | null
  latency_ms?: number | null
  error_type?: string | null
  error_message?: string | null
}

export interface EndpointStatusMonitor {
  api_format: string
  total_attempts: number
  success_count: number
  failed_count: number
  skipped_count: number
  success_rate: number
  provider_count: number
  key_count: number
  last_event_at?: string | null
  events: EndpointHealthEvent[]
  timeline?: string[]
  time_range_start?: string | null
  time_range_end?: string | null
}

export interface EndpointStatusMonitorResponse {
  generated_at: string
  formats: EndpointStatusMonitor[]
}

// 公开版事件（不含敏感信息如 provider_id, key_id）
export interface PublicHealthEvent {
  timestamp: string
  status: string
  status_code?: number | null
  latency_ms?: number | null
  error_type?: string | null
}

// 公开版端点状态监控类型（返回 events，前端复用 EndpointHealthTimeline 组件）
export interface PublicEndpointStatusMonitor {
  api_format: string
  api_path: string  // 本站入口路径
  total_attempts: number
  success_count: number
  failed_count: number
  skipped_count: number
  success_rate: number
  last_event_at?: string | null
  events: PublicHealthEvent[]
  timeline?: string[]
  time_range_start?: string | null
  time_range_end?: string | null
}

export interface PublicEndpointStatusMonitorResponse {
  generated_at: string
  formats: PublicEndpointStatusMonitor[]
}

export type ProviderType = 'custom' | 'claude_code' | 'codex' | 'gemini_cli' | 'antigravity' | 'kiro'

export interface ProviderWithEndpointsSummary {
  id: string
  name: string
  provider_type?: ProviderType
  description?: string
  website?: string
  provider_priority: number
  keep_priority_on_conversion: boolean  // 格式转换时是否保持优先级
  enable_format_conversion: boolean  // 是否允许格式转换（提供商级别开关）
  billing_type?: 'monthly_quota' | 'pay_as_you_go' | 'free_tier'
  monthly_quota_usd?: number
  monthly_used_usd?: number
  quota_reset_day?: number
  quota_last_reset_at?: string  // 当前周期开始时间
  quota_expires_at?: string
  // 请求配置（从 Endpoint 迁移）
  max_retries?: number  // 最大重试次数
  proxy?: ProxyConfig | null  // 代理配置
  // 超时配置（秒），为空时使用全局配置
  stream_first_byte_timeout?: number  // 流式请求首字节超时
  request_timeout?: number  // 非流式请求整体超时
  is_active: boolean
  total_endpoints: number
  active_endpoints: number
  total_keys: number
  active_keys: number
  total_models: number
  active_models: number
  global_model_ids: string[]
  avg_health_score: number
  unhealthy_endpoints: number
  api_formats: string[]
  endpoint_health_details: EndpointHealthDetail[]
  ops_configured: boolean  // 是否配置了扩展操作（余额监控等）
  ops_architecture_id?: string  // 扩展操作使用的架构 ID（如 cubence, anyrouter）
  created_at: string
  updated_at: string
}

export interface HealthStatus {
  endpoint_id?: string
  endpoint_health_score?: number
  endpoint_consecutive_failures?: number
  endpoint_last_failure_at?: string
  endpoint_is_active?: boolean
  key_id?: string
  key_health_score?: number
  key_consecutive_failures?: number
  key_last_failure_at?: string
  key_is_active?: boolean
  key_statistics?: Record<string, any>
}

export interface HealthSummary {
  endpoints: {
    total: number
    active: number
    unhealthy: number
  }
  keys: {
    total: number
    active: number
    unhealthy: number
  }
}

export interface KeyRpmStatus {
  key_id: string
  current_rpm: number
  rpm_limit?: number
}

export interface ProviderModelMapping {
  name: string
  priority: number  // 优先级（数字越小优先级越高）
  api_formats?: string[]  // 作用域（适用的 API 格式），为空表示对所有格式生效
}

// 保留别名以保持向后兼容
export type ProviderModelAlias = ProviderModelMapping

export interface Model {
  id: string
  provider_id: string
  global_model_id?: string  // 关联的 GlobalModel ID
  provider_model_name: string  // Provider 侧的主模型名称
  provider_model_mappings?: ProviderModelMapping[] | null  // 模型名称映射列表（带优先级）
  config?: Record<string, any> | null  // 额外配置（如 billing/video 等）
  // 原始配置值（可能为空，为空时使用 GlobalModel 默认值）
  price_per_request?: number | null  // 按次计费价格
  tiered_pricing?: TieredPricingConfig | null  // 阶梯计费配置
  supports_vision?: boolean | null
  supports_function_calling?: boolean | null
  supports_streaming?: boolean | null
  supports_extended_thinking?: boolean | null
  supports_image_generation?: boolean | null
  // 有效值（合并 Model 和 GlobalModel 默认值后的结果）
  effective_tiered_pricing?: TieredPricingConfig | null  // 有效阶梯计费配置
  effective_input_price?: number | null
  effective_output_price?: number | null
  effective_price_per_request?: number | null  // 有效按次计费价格
  effective_supports_vision?: boolean | null
  effective_supports_function_calling?: boolean | null
  effective_supports_streaming?: boolean | null
  effective_supports_extended_thinking?: boolean | null
  effective_supports_image_generation?: boolean | null
  is_active: boolean
  is_available: boolean
  created_at: string
  updated_at: string
  // GlobalModel 信息（从后端 join 获取）
  global_model_name?: string
  global_model_display_name?: string
  // 有效配置（合并 Model 和 GlobalModel 的 config）
  effective_config?: Record<string, any> | null
}

export interface ModelCreate {
  provider_model_name: string  // Provider 侧的主模型名称
  provider_model_mappings?: ProviderModelMapping[]  // 模型名称映射列表（带优先级）
  global_model_id: string  // 关联的 GlobalModel ID（必填）
  // 计费配置（可选，为空时使用 GlobalModel 默认值）
  price_per_request?: number  // 按次计费价格
  tiered_pricing?: TieredPricingConfig  // 阶梯计费配置
  // 能力配置（可选，为空时使用 GlobalModel 默认值）
  supports_vision?: boolean
  supports_function_calling?: boolean
  supports_streaming?: boolean
  supports_extended_thinking?: boolean
  supports_image_generation?: boolean
  is_active?: boolean
  config?: Record<string, any>
}

export interface ModelUpdate {
  provider_model_name?: string
  provider_model_mappings?: ProviderModelMapping[] | null  // 模型名称映射列表（带优先级）
  global_model_id?: string
  price_per_request?: number | null  // 按次计费价格（null 表示清空/使用默认值）
  tiered_pricing?: TieredPricingConfig | null  // 阶梯计费配置
  supports_vision?: boolean
  supports_function_calling?: boolean
  supports_streaming?: boolean
  supports_extended_thinking?: boolean
  supports_image_generation?: boolean
  is_active?: boolean
  is_available?: boolean
  config?: Record<string, any> | null
}

export interface ModelCapabilities {
  supports_vision: boolean
  supports_function_calling: boolean
  supports_streaming: boolean
  [key: string]: boolean
}

export interface ProviderModelPriceInfo {
  input_price_per_1m?: number | null
  output_price_per_1m?: number | null
  cache_creation_price_per_1m?: number | null
  cache_read_price_per_1m?: number | null
  price_per_request?: number | null  // 按次计费价格
}

export interface ModelPriceRange {
  min_input: number | null
  max_input: number | null
  min_output: number | null
  max_output: number | null
}

export interface ModelCatalogProviderDetail {
  provider_id: string
  provider_name: string
  model_id?: string | null
  target_model: string
  input_price_per_1m?: number | null
  output_price_per_1m?: number | null
  cache_creation_price_per_1m?: number | null
  cache_read_price_per_1m?: number | null
  cache_1h_creation_price_per_1m?: number | null  // 1h 缓存创建价格
  price_per_request?: number | null  // 按次计费价格
  effective_tiered_pricing?: TieredPricingConfig | null  // 有效阶梯计费配置（含继承）
  tier_count?: number  // 阶梯数量
  supports_vision?: boolean | null
  supports_function_calling?: boolean | null
  supports_streaming?: boolean | null
  is_active: boolean
  mapping_id?: string | null
}

export interface ModelCatalogItem {
  global_model_name: string  // GlobalModel.name（原 source_model）
  display_name: string  // GlobalModel.display_name
  description?: string | null  // GlobalModel.description
  providers: ModelCatalogProviderDetail[]  // 支持该模型的 Provider 列表
  price_range: ModelPriceRange  // 价格区间
  total_providers: number
  capabilities: ModelCapabilities  // 能力聚合
}

export interface ModelCatalogResponse {
  models: ModelCatalogItem[]
  total: number
}

export interface ProviderAvailableSourceModel {
  global_model_name: string  // GlobalModel.name（原 source_model）
  display_name: string  // GlobalModel.display_name
  provider_model_name: string  // Model.provider_model_name（Provider 侧的模型名）
  model_id?: string | null  // Model.id
  price: ProviderModelPriceInfo
  capabilities: ModelCapabilities
  is_active: boolean
}

export interface ProviderAvailableSourceModelsResponse {
  models: ProviderAvailableSourceModel[]
  total: number
}

export interface BatchAssignProviderConfig {
  provider_id: string
  create_model?: boolean
  model_config?: ModelCreate
  model_id?: string
}

export interface AdaptiveStatsResponse {
  adaptive_mode: boolean
  current_limit: number | null
  learned_limit: number | null
  concurrent_429_count: number
  rpm_429_count: number
  last_429_at: string | null
  last_429_type: string | null
  adjustment_count: number
  recent_adjustments: Array<{
    timestamp: string
    old_limit: number
    new_limit: number
    reason: string
    [key: string]: any
  }>
}

// ========== 阶梯计费类型 ==========

/** 缓存时长定价配置 */
export interface CacheTTLPricing {
  ttl_minutes: number
  cache_creation_price_per_1m: number
}

/** 单个价格阶梯配置 */
export interface PricingTier {
  up_to: number | null  // null 表示无上限（最后一个阶梯）
  input_price_per_1m: number
  output_price_per_1m: number
  cache_creation_price_per_1m?: number
  cache_read_price_per_1m?: number
  cache_ttl_pricing?: CacheTTLPricing[]
}

/** 阶梯计费配置 */
export interface TieredPricingConfig {
  tiers: PricingTier[]
}

// ========== GlobalModel 类型 ==========

export interface GlobalModelCreate {
  name: string
  display_name: string
  // 按次计费配置（可选，与阶梯计费叠加）
  default_price_per_request?: number
  // 阶梯计费配置（必填，固定价格用单阶梯表示）
  default_tiered_pricing: TieredPricingConfig
  // Key 能力配置 - 模型支持的能力列表
  supported_capabilities?: string[]
  // 模型配置（JSON格式）- 包含能力、规格、元信息等
  config?: Record<string, any>
  is_active?: boolean
}

export interface GlobalModelUpdate {
  display_name?: string
  is_active?: boolean
  // 按次计费配置
  default_price_per_request?: number | null  // null 表示清空
  // 阶梯计费配置
  default_tiered_pricing?: TieredPricingConfig
  // Key 能力配置 - 模型支持的能力列表
  supported_capabilities?: string[] | null
  // 模型配置（JSON格式）- 包含能力、规格、元信息等
  config?: Record<string, any> | null
}

export interface GlobalModelResponse {
  id: string
  name: string
  display_name: string
  is_active: boolean
  // 按次计费配置
  default_price_per_request?: number
  // 阶梯计费配置（必填）
  default_tiered_pricing: TieredPricingConfig
  // Key 能力配置 - 模型支持的能力列表
  supported_capabilities?: string[] | null
  // 模型配置（JSON格式）
  config?: Record<string, any> | null
  // 统计数据
  provider_count?: number
  usage_count?: number
  created_at: string
  updated_at?: string
}

export interface GlobalModelWithStats extends GlobalModelResponse {
  total_models: number
  total_providers: number
  price_range: ModelPriceRange
}

export interface GlobalModelListResponse {
  models: GlobalModelResponse[]
  total: number
}

// ==================== 上游模型导入相关 ====================

/**
 * 上游模型（从提供商 API 获取的原始模型）
 * 后端已按 model id 聚合，api_formats 包含该模型支持的所有 API 格式
 */
export interface UpstreamModel {
  id: string
  owned_by?: string
  display_name?: string
  api_formats: string[]  // 该模型支持的所有 API 格式（后端保证返回数组）
}

/**
 * 导入成功的模型信息
 */
export interface ImportFromUpstreamSuccessItem {
  model_id: string
  provider_model_id: string
  global_model_id?: string  // 可选，未关联时为空字符串
  global_model_name?: string  // 可选，未关联时为空字符串
  created_global_model: boolean  // 始终为 false（不再自动创建 GlobalModel）
}

/**
 * 导入失败的模型信息
 */
export interface ImportFromUpstreamErrorItem {
  model_id: string
  error: string
}

/**
 * 从上游提供商导入模型响应
 */
export interface ImportFromUpstreamResponse {
  success: ImportFromUpstreamSuccessItem[]
  errors: ImportFromUpstreamErrorItem[]
}

// ========== 路由预览相关类型 ==========

/**
 * Key 路由信息
 */
export interface RoutingKeyInfo {
  id: string
  name: string
  masked_key: string
  internal_priority: number
  global_priority?: number | null
  rpm_limit?: number | null
  is_adaptive: boolean
  effective_rpm?: number | null
  cache_ttl_minutes: number
  health_score: number  // 0-1 小数格式
  is_active: boolean
  api_formats: string[]
  allowed_models?: string[] | null  // 允许的模型列表，null 表示不限制
  circuit_breaker_open: boolean
  circuit_breaker_formats: string[]
  next_probe_at?: string | null  // 下次探测时间（ISO格式）
}

/**
 * Endpoint 路由信息
 */
export interface RoutingEndpointInfo {
  id: string
  api_format: string
  base_url: string
  custom_path?: string | null
  is_active: boolean
  keys: RoutingKeyInfo[]
  total_keys: number
  active_keys: number
}

/**
 * 模型名称映射信息
 */
export interface RoutingModelMapping {
  name: string
  priority: number
  api_formats?: string[] | null
}

/**
 * Provider 路由信息
 */
export interface RoutingProviderInfo {
  id: string
  name: string
  model_id: string
  provider_priority: number
  billing_type?: string | null
  monthly_quota_usd?: number | null
  monthly_used_usd?: number | null
  is_active: boolean
  provider_model_name: string
  model_mappings: RoutingModelMapping[]
  model_is_active: boolean
  endpoints: RoutingEndpointInfo[]
  total_endpoints: number
  active_endpoints: number
}

/**
 * 全局 Key 白名单项（用于前端实时匹配）
 */
export interface GlobalKeyWhitelistItem {
  key_id: string
  key_name: string
  masked_key: string
  provider_id: string
  provider_name: string
  allowed_models: string[]
}

/**
 * 模型请求链路预览响应
 */
export interface ModelRoutingPreviewResponse {
  global_model_id: string
  global_model_name: string
  display_name: string
  is_active: boolean
  global_model_mappings: string[]  // GlobalModel 的模型映射规则（正则模式）
  providers: RoutingProviderInfo[]
  total_providers: number
  active_providers: number
  scheduling_mode: string
  priority_mode: string
  all_keys_whitelist: GlobalKeyWhitelistItem[]
}
