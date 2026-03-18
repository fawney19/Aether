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

export interface OAuthOrganizationInfo {
  id?: string | null
  title?: string | null
  is_default?: boolean | null
  role?: string | null
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
  value: unknown
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
  value: unknown
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
  value: unknown
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

export type BodyRuleConditionOp =
  | 'eq' | 'neq'
  | 'gt' | 'lt' | 'gte' | 'lte'
  | 'starts_with' | 'ends_with' | 'contains' | 'matches'
  | 'exists' | 'not_exists'
  | 'in' | 'type_is'

export interface BodyRuleConditionLeaf {
  path: string
  op: BodyRuleConditionOp
  value?: unknown  // exists / not_exists 不需要 value
  source?: 'original' | 'current'
}

export interface BodyRuleConditionAll {
  all: BodyRuleCondition[]
}

export interface BodyRuleConditionAny {
  any: BodyRuleCondition[]
}

export type BodyRuleCondition =
  | BodyRuleConditionLeaf
  | BodyRuleConditionAll
  | BodyRuleConditionAny

export type HeaderRule = (HeaderRuleSet | HeaderRuleDrop | HeaderRuleRename) & {
  condition?: BodyRuleCondition
}

/**
 * 请求体规则 - 转换命名风格
 *
 * - path 指向目标字符串字段，支持通配符如 "tools[*].name"
 * - style 为目标命名风格
 */
export interface BodyRuleNameStyle {
  action: 'name_style'
  path: string
  style: 'snake_case' | 'camelCase' | 'PascalCase' | 'kebab-case' | 'capitalize'
}

export type BodyRule = (BodyRuleSet | BodyRuleDrop | BodyRuleRename | BodyRuleAppend | BodyRuleInsert | BodyRuleRegexReplace | BodyRuleNameStyle) & {
  condition?: BodyRuleCondition
}

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
  config?: Record<string, unknown>
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
  auth_type: 'api_key' | 'service_account' | 'oauth'  // 认证类型（必返回）
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
  oauth_account_user_id?: string | null  // Codex ChatGPT account-user 联合 ID
  oauth_account_name?: string | null
  oauth_organizations?: OAuthOrganizationInfo[] | null  // OAuth 关联组织/工作区摘要
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
  auth_type?: 'api_key' | 'service_account' | 'oauth'  // 认证类型
  auth_config?: Record<string, unknown>  // 认证配置（Vertex AI Service Account JSON）
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

export type ProviderType = 'custom' | 'claude_code' | 'codex' | 'gemini_cli' | 'antigravity' | 'kiro' | 'vertex_ai'

export interface ClaudeCodeAdvancedConfig {
  // 会话数量控制：null/undefined 表示不限制
  max_sessions?: number | null
  session_idle_timeout_minutes?: number | null
  // 会话 ID 伪装（固定 metadata.user_id 中 session 片段）
  session_id_masking_enabled?: boolean
  // Cache TTL 统一（强制所有 cache_control 使用相同 TTL 类型）
  cache_ttl_override_enabled?: boolean
  cache_ttl_override_target?: string
  // 仅允许 CLI 客户端
  cli_only_enabled?: boolean
}

export interface SchedulingPresetItem {
  preset: string
  enabled: boolean
  mode?: string | null
}

export interface PoolAdvancedConfig {
  global_priority?: number | null
  sticky_session_ttl_seconds?: number | null
  load_threshold_percent?: number | null
  // 旧字段（兼容读取）
  lru_enabled?: boolean
  scheduling_mode?: 'lru' | 'multi_score' | null
  // 新格式：对象列表；旧格式：字符串列表
  scheduling_presets?: SchedulingPresetItem[] | string[] | null
  scoring_weights?: {
    lru?: number
    latency?: number
    health?: number
    cost_remaining?: number
  } | null
  latency_window_seconds?: number | null
  latency_sample_limit?: number | null
  cost_window_seconds?: number | null
  cost_limit_per_key_tokens?: number | null
  cost_soft_threshold_percent?: number | null
  rate_limit_cooldown_seconds?: number | null
  overload_cooldown_seconds?: number | null
  proactive_refresh_seconds?: number | null
  health_policy_enabled?: boolean
  unschedulable_rules?: Array<Record<string, unknown>> | null
  batch_concurrency?: number | null
  probing_enabled?: boolean
  probing_interval_minutes?: number | null
  auto_remove_banned_keys?: boolean
}

export interface FailoverRuleItem {
  pattern: string
  description?: string
  status_codes?: number[]
}

export interface FailoverRulesConfig {
  success_failover_patterns: FailoverRuleItem[]
  error_stop_patterns: FailoverRuleItem[]
}

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
  claude_code_advanced?: ClaudeCodeAdvancedConfig | null
  pool_advanced?: PoolAdvancedConfig | null
  failover_rules?: FailoverRulesConfig | null
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
  key_statistics?: Record<string, unknown>
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
    [key: string]: unknown
  }>
}
