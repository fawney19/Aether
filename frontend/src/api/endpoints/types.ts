// API 格式常量
export const API_FORMATS = {
  CLAUDE: 'CLAUDE',
  CLAUDE_CLI: 'CLAUDE_CLI',
  OPENAI: 'OPENAI',
  OPENAI_CLI: 'OPENAI_CLI',
  GEMINI: 'GEMINI',
  GEMINI_CLI: 'GEMINI_CLI',
} as const

export type APIFormat = typeof API_FORMATS[keyof typeof API_FORMATS]

// API 格式显示名称映射（按品牌分组：API 在前，CLI 在后）
export const API_FORMAT_LABELS: Record<string, string> = {
  [API_FORMATS.CLAUDE]: 'Claude',
  [API_FORMATS.CLAUDE_CLI]: 'Claude CLI',
  [API_FORMATS.OPENAI]: 'OpenAI',
  [API_FORMATS.OPENAI_CLI]: 'OpenAI CLI',
  [API_FORMATS.GEMINI]: 'Gemini',
  [API_FORMATS.GEMINI_CLI]: 'Gemini CLI',
}

// API 格式缩写映射（用于空间紧凑的显示场景）
export const API_FORMAT_SHORT: Record<string, string> = {
  [API_FORMATS.OPENAI]: 'O',
  [API_FORMATS.OPENAI_CLI]: 'OC',
  [API_FORMATS.CLAUDE]: 'C',
  [API_FORMATS.CLAUDE_CLI]: 'CC',
  [API_FORMATS.GEMINI]: 'G',
  [API_FORMATS.GEMINI_CLI]: 'GC',
}

// API 格式排序顺序（统一的显示顺序）
export const API_FORMAT_ORDER: string[] = [
  API_FORMATS.OPENAI,
  API_FORMATS.OPENAI_CLI,
  API_FORMATS.CLAUDE,
  API_FORMATS.CLAUDE_CLI,
  API_FORMATS.GEMINI,
  API_FORMATS.GEMINI_CLI,
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
 */
export interface ProxyConfig {
  url: string
  username?: string
  password?: string
  enabled?: boolean  // 是否启用代理（false 时保留配置但不使用）
}

export interface ProviderEndpoint {
  id: string
  provider_id: string
  provider_name: string
  api_format: string
  base_url: string
  custom_path?: string  // 自定义请求路径（可选，为空则使用 API 格式默认路径）
  headers?: Record<string, string>
  timeout: number
  max_retries: number
  is_active: boolean
  config?: Record<string, any>
  proxy?: ProxyConfig | null
  total_keys: number
  active_keys: number
  created_at: string
  updated_at: string
}

/**
 * 模型权限配置类型（支持简单列表和按格式字典两种模式）
 *
 * 使用示例：
 * 1. 不限制（允许所有模型）: null
 * 2. 简单列表模式（所有 API 格式共享同一个白名单）: ["gpt-4", "claude-3-opus"]
 * 3. 按格式字典模式（不同 API 格式使用不同的白名单）:
 *    { "OPENAI": ["gpt-4"], "CLAUDE": ["claude-3-opus"] }
 */
export type AllowedModels = string[] | Record<string, string[]> | null

// AllowedModels 类型守卫函数
export function isAllowedModelsList(value: AllowedModels): value is string[] {
  return Array.isArray(value)
}

export function isAllowedModelsDict(value: AllowedModels): value is Record<string, string[]> {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    return false
  }
  // 验证所有值都是字符串数组
  return Object.values(value).every(
    (v) => Array.isArray(v) && v.every((item) => typeof item === 'string')
  )
}

export interface EndpointAPIKey {
  id: string
  provider_id: string
  api_formats: string[]  // 支持的 API 格式列表
  api_key_masked: string
  api_key_plain?: string | null
  name: string  // 密钥名称（必填，用于识别）
  rate_multiplier: number  // 默认成本倍率（真实成本 = 表面成本 × 倍率）
  rate_multipliers?: Record<string, number> | null  // 按 API 格式的成本倍率，如 {"CLAUDE": 1.0, "OPENAI": 0.8}
  internal_priority: number  // Key 内部优先级
  global_priority?: number | null  // 全局 Key 优先级
  rpm_limit?: number | null  // RPM 速率限制 (1-10000)，null 表示自适应模式
  allowed_models?: AllowedModels  // 允许使用的模型列表（null=不限制，列表=简单白名单，字典=按格式区分）
  capabilities?: Record<string, boolean> | null  // 能力标签配置（如 cache_1h, context_1m）
  // 缓存与熔断配置
  cache_ttl_minutes: number  // 缓存 TTL（分钟），0=禁用
  max_probe_interval_minutes: number  // 熔断探测间隔（分钟）
  // 按格式的健康度数据
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
  effective_limit?: number  // 当前有效 RPM 限制（自适应使用学习值，固定使用配置值）
  learned_rpm_limit?: number  // 学习到的 RPM 限制
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
  rate_multiplier?: number  // 默认成本倍率
  rate_multipliers?: Record<string, number> | null  // 按 API 格式的成本倍率
  internal_priority?: number
  global_priority?: number | null
  rpm_limit?: number | null  // RPM 速率限制 (1-10000)，null 表示切换为自适应模式
  allowed_models?: AllowedModels
  capabilities?: Record<string, boolean> | null
  cache_ttl_minutes?: number
  max_probe_interval_minutes?: number
  note?: string
  is_active?: boolean
}

export interface EndpointHealthDetail {
  api_format: string
  health_score: number
  is_active: boolean
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

export interface ProviderWithEndpointsSummary {
  id: string
  name: string
  description?: string
  website?: string
  provider_priority: number
  billing_type?: 'monthly_quota' | 'pay_as_you_go' | 'free_tier'
  monthly_quota_usd?: number
  monthly_used_usd?: number
  quota_reset_day?: number
  quota_last_reset_at?: string  // 当前周期开始时间
  quota_expires_at?: string
  // 请求配置（从 Endpoint 迁移）
  timeout?: number  // 请求超时（秒）
  max_retries?: number  // 最大重试次数
  proxy?: ProxyConfig | null  // 代理配置
  is_active: boolean
  total_endpoints: number
  active_endpoints: number
  total_keys: number
  active_keys: number
  total_models: number
  active_models: number
  avg_health_score: number
  unhealthy_endpoints: number
  api_formats: string[]
  endpoint_health_details: EndpointHealthDetail[]
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
 */
export interface UpstreamModel {
  id: string
  owned_by?: string
  display_name?: string
  api_format?: string
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
  health_score: number
  is_active: boolean
  api_formats: string[]
  circuit_breaker_open: boolean
  circuit_breaker_formats: string[]
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
 * 模型请求链路预览响应
 */
export interface ModelRoutingPreviewResponse {
  global_model_id: string
  global_model_name: string
  display_name: string
  is_active: boolean
  providers: RoutingProviderInfo[]
  total_providers: number
  active_providers: number
  scheduling_mode: string
  priority_mode: string
}
