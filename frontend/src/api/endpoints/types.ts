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
  auth_type: string
  auth_header?: string
  headers?: Record<string, string>
  timeout: number
  max_retries: number
  priority: number
  weight: number
  max_concurrent?: number
  rate_limit?: number
  health_score: number
  consecutive_failures: number
  last_failure_at?: string
  is_active: boolean
  config?: Record<string, any>
  proxy?: ProxyConfig | null
  total_keys: number
  active_keys: number
  created_at: string
  updated_at: string
}

export interface EndpointAPIKey {
  id: string
  endpoint_id: string
  api_key_masked: string
  api_key_plain?: string | null
  name: string  // 密钥名称（必填，用于识别）
  rate_multiplier: number  // 成本倍率（真实成本 = 表面成本 × 倍率）
  internal_priority: number  // Endpoint 内部优先级
  global_priority?: number | null  // 全局 Key 优先级
  max_concurrent?: number
  rate_limit?: number
  daily_limit?: number
  monthly_limit?: number
  allowed_models?: string[] | null  // 允许使用的模型列表（null = 支持所有模型）
  capabilities?: Record<string, boolean> | null  // 能力标签配置（如 cache_1h, context_1m）
  // 缓存与熔断配置
  cache_ttl_minutes: number  // 缓存 TTL（分钟），0=禁用
  max_probe_interval_minutes: number  // 熔断探测间隔（分钟）
  health_score: number
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
  // 自适应并发字段
  is_adaptive?: boolean  // 是否为自适应模式（max_concurrent=NULL）
  effective_limit?: number  // 当前有效限制（自适应使用学习值，固定使用配置值）
  learned_max_concurrent?: number
  // 滑动窗口利用率采样
  utilization_samples?: Array<{ ts: number; util: number }>  // 利用率采样窗口
  last_probe_increase_at?: string  // 上次探测性扩容时间
  concurrent_429_count?: number
  rpm_429_count?: number
  last_429_at?: string
  last_429_type?: string
  // 熔断器字段（滑动窗口 + 半开模式）
  circuit_breaker_open?: boolean
  circuit_breaker_open_at?: string
  next_probe_at?: string
  half_open_until?: string
  half_open_successes?: number
  half_open_failures?: number
  request_results_window?: Array<{ ts: number; ok: boolean }>  // 请求结果滑动窗口
}

export interface EndpointAPIKeyUpdate {
  name?: string
  api_key?: string  // 仅在需要更新时提供
  rate_multiplier?: number
  internal_priority?: number
  global_priority?: number | null
  max_concurrent?: number | null  // null 表示切换为自适应模式
  rate_limit?: number
  daily_limit?: number
  monthly_limit?: number
  allowed_models?: string[] | null
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
  display_name: string
  description?: string
  website?: string
  provider_priority: number
  billing_type?: 'monthly_quota' | 'pay_as_you_go' | 'free_tier'
  monthly_quota_usd?: number
  monthly_used_usd?: number
  quota_reset_day?: number
  quota_last_reset_at?: string  // 当前周期开始时间
  quota_expires_at?: string
  rpm_limit?: number | null
  rpm_used?: number
  rpm_reset_at?: string
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

export interface ConcurrencyStatus {
  endpoint_id?: string
  endpoint_current_concurrency: number
  endpoint_max_concurrent?: number
  key_id?: string
  key_current_concurrency: number
  key_max_concurrent?: number
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
  provider_display_name?: string | null
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
  mapped?: boolean
}

/**
 * 导入成功的模型信息
 */
export interface ImportFromUpstreamSuccessItem {
  model_id: string
  global_model_id: string
  global_model_name: string
  provider_model_id: string
  created_global_model: boolean
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
