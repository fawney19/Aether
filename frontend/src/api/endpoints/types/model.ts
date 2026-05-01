import type { ProviderModelMapping } from './provider'

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

export interface Model {
  id: string
  provider_id: string
  global_model_id: string  // 关联的 GlobalModel ID
  provider_model_name: string  // Provider 侧的主模型名称
  provider_model_mappings?: ProviderModelMapping[] | null  // 模型名称映射列表（带优先级）
  config?: Record<string, unknown> | null  // 额外配置（如 billing/video 等）
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
  effective_config?: Record<string, unknown> | null
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
  config?: Record<string, unknown>
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
  config?: Record<string, unknown> | null
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
  config?: Record<string, unknown>
  is_active?: boolean
  model_group_ids?: string[] | null
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
  config?: Record<string, unknown> | null
  model_group_ids?: string[] | null
}

export interface GlobalModelGroupRef {
  id: string
  name: string
  display_name: string
  is_default: boolean
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
  config?: Record<string, unknown> | null
  // 统计数据
  provider_count?: number
  active_provider_count?: number
  usage_count?: number
  model_group_ids: string[]
  model_groups: GlobalModelGroupRef[]
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
  global_model_id: string
  global_model_name: string
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
