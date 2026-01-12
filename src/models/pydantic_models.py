"""
Pydantic 数据模型（阶段一统一模型管理）
"""

from datetime import datetime
from typing import Any, Dict, List, Optional

from pydantic import BaseModel, ConfigDict, Field, model_validator


# ========== 阶梯计费相关模型 ==========


class CacheTTLPricing(BaseModel):
    """缓存时长定价配置"""

    ttl_minutes: int = Field(..., ge=1, description="缓存时长（分钟）")
    cache_creation_price_per_1m: float = Field(..., ge=0, description="该时长的缓存创建价格/M tokens")


class PricingTier(BaseModel):
    """单个价格阶梯配置"""

    up_to: Optional[int] = Field(
        None,
        ge=1,
        description="阶梯上限（tokens），null 表示无上限（最后一个阶梯）"
    )
    input_price_per_1m: float = Field(..., ge=0, description="输入价格/M tokens")
    output_price_per_1m: float = Field(..., ge=0, description="输出价格/M tokens")
    cache_creation_price_per_1m: Optional[float] = Field(
        None, ge=0, description="缓存创建价格/M tokens"
    )
    cache_read_price_per_1m: Optional[float] = Field(
        None, ge=0, description="缓存读取价格/M tokens"
    )
    cache_ttl_pricing: Optional[List[CacheTTLPricing]] = Field(
        None, description="按缓存时长分价格（可选）"
    )


class TieredPricingConfig(BaseModel):
    """阶梯计费配置"""

    tiers: List[PricingTier] = Field(
        ...,
        min_length=1,
        description="价格阶梯列表，按 up_to 升序排列"
    )

    @model_validator(mode="after")
    def validate_tiers(self) -> "TieredPricingConfig":
        """验证阶梯配置的合法性"""
        tiers = self.tiers
        if not tiers:
            raise ValueError("至少需要一个价格阶梯")

        # 检查阶梯顺序和唯一性
        prev_up_to = 0
        has_unlimited = False

        for i, tier in enumerate(tiers):
            if has_unlimited:
                raise ValueError("无上限阶梯（up_to=null）必须是最后一个")

            if tier.up_to is None:
                has_unlimited = True
            else:
                if tier.up_to <= prev_up_to:
                    raise ValueError(
                        f"阶梯 {i+1} 的 up_to ({tier.up_to}) 必须大于前一个阶梯 ({prev_up_to})"
                    )
                prev_up_to = tier.up_to

            # 验证缓存时长定价顺序
            if tier.cache_ttl_pricing:
                prev_ttl = 0
                for ttl_pricing in tier.cache_ttl_pricing:
                    if ttl_pricing.ttl_minutes <= prev_ttl:
                        raise ValueError(
                            f"cache_ttl_pricing 必须按 ttl_minutes 升序排列"
                        )
                    prev_ttl = ttl_pricing.ttl_minutes

        # 最后一个阶梯必须是无上限的
        if not has_unlimited:
            raise ValueError("最后一个阶梯必须设置 up_to=null（无上限）")

        return self


# ========== 其他模型 ==========


class ModelCapabilities(BaseModel):
    """模型能力聚合"""

    supports_vision: bool = False
    supports_function_calling: bool = False
    supports_streaming: bool = False


class ModelPriceRange(BaseModel):
    """统一模型价格区间"""

    min_input: Optional[float] = None
    max_input: Optional[float] = None
    min_output: Optional[float] = None
    max_output: Optional[float] = None


class ModelCatalogProviderDetail(BaseModel):
    """统一模型目录中的关联提供商信息"""

    provider_id: str
    provider_name: str
    model_id: Optional[str]
    target_model: str
    input_price_per_1m: Optional[float]
    output_price_per_1m: Optional[float]
    cache_creation_price_per_1m: Optional[float]
    cache_read_price_per_1m: Optional[float]
    cache_1h_creation_price_per_1m: Optional[float] = None  # 1h 缓存创建价格
    price_per_request: Optional[float] = None  # 按次计费价格
    effective_tiered_pricing: Optional[Dict[str, Any]] = None  # 有效阶梯计费配置（含继承）
    tier_count: int = 1  # 阶梯数量
    supports_vision: Optional[bool] = None
    supports_function_calling: Optional[bool] = None
    supports_streaming: Optional[bool] = None
    is_active: bool


class ModelCatalogItem(BaseModel):
    """统一模型目录条目（基于 GlobalModel）"""

    global_model_name: str  # GlobalModel.name
    display_name: str  # GlobalModel.display_name
    description: Optional[str]  # GlobalModel.description
    providers: List[ModelCatalogProviderDetail]  # 支持该模型的 Provider 列表
    price_range: ModelPriceRange  # 价格区间（从所有 Provider 的 Model 中聚合）
    total_providers: int
    capabilities: ModelCapabilities  # 能力聚合（从所有 Provider 的 Model 中聚合）


class ModelCatalogResponse(BaseModel):
    """统一模型目录响应"""

    models: List[ModelCatalogItem]
    total: int


class ProviderModelPriceInfo(BaseModel):
    """Provider 维度的模型价格信息"""

    input_price_per_1m: Optional[float]
    output_price_per_1m: Optional[float]
    cache_creation_price_per_1m: Optional[float]
    cache_read_price_per_1m: Optional[float]
    price_per_request: Optional[float] = None  # 按次计费价格


class ProviderAvailableSourceModel(BaseModel):
    """Provider 支持的统一模型条目"""

    global_model_name: str  # GlobalModel.name
    display_name: str  # GlobalModel.display_name
    provider_model_name: str  # Model.provider_model_name (Provider 侧的模型名)
    model_id: Optional[str]  # Model.id
    price: ProviderModelPriceInfo
    capabilities: ModelCapabilities
    is_active: bool


class ProviderAvailableSourceModelsResponse(BaseModel):
    """Provider 可用统一模型响应"""

    models: List[ProviderAvailableSourceModel]
    total: int


# ========== GlobalModel 相关模型 ==========


class GlobalModelCreate(BaseModel):
    """创建 GlobalModel 请求"""

    name: str = Field(..., min_length=1, max_length=100, description="统一模型名（唯一）")
    display_name: str = Field(..., min_length=1, max_length=100, description="显示名称")
    # 按次计费配置（可选，与阶梯计费叠加）
    default_price_per_request: Optional[float] = Field(None, ge=0, description="每次请求固定费用")
    # 统一阶梯计费配置（必填）
    # 固定价格也用单阶梯表示: {"tiers": [{"up_to": null, "input_price_per_1m": X, ...}]}
    default_tiered_pricing: TieredPricingConfig = Field(
        ..., description="阶梯计费配置（固定价格用单阶梯表示）"
    )
    # Key 能力配置 - 模型支持的能力列表（如 ["cache_1h", "context_1m"]）
    supported_capabilities: Optional[List[str]] = Field(
        None, description="支持的 Key 能力列表"
    )
    # 模型配置（JSON格式）- 包含能力、规格、元信息等
    config: Optional[Dict[str, Any]] = Field(
        None,
        description="模型配置（streaming, vision, context_limit, description 等）"
    )
    is_active: Optional[bool] = Field(True, description="是否激活")


class GlobalModelUpdate(BaseModel):
    """更新 GlobalModel 请求"""

    display_name: Optional[str] = Field(None, min_length=1, max_length=100)
    is_active: Optional[bool] = None
    # 按次计费配置
    default_price_per_request: Optional[float] = Field(None, ge=0, description="每次请求固定费用")
    # 阶梯计费配置
    default_tiered_pricing: Optional[TieredPricingConfig] = Field(
        None, description="阶梯计费配置"
    )
    # Key 能力配置 - 模型支持的能力列表（如 ["cache_1h", "context_1m"]）
    supported_capabilities: Optional[List[str]] = Field(
        None, description="支持的 Key 能力列表"
    )
    # 模型配置（JSON格式）- 包含能力、规格、元信息等
    config: Optional[Dict[str, Any]] = Field(
        None,
        description="模型配置（streaming, vision, context_limit, description 等）"
    )


class GlobalModelResponse(BaseModel):
    """GlobalModel 响应"""

    id: str
    name: str
    display_name: str
    is_active: bool
    # 按次计费配置
    default_price_per_request: Optional[float] = Field(None, description="每次请求固定费用")
    # 阶梯计费配置
    default_tiered_pricing: Optional[TieredPricingConfig] = Field(
        default=None, description="阶梯计费配置"
    )
    # Key 能力配置 - 模型支持的能力列表
    supported_capabilities: Optional[List[str]] = Field(
        default=None, description="支持的 Key 能力列表"
    )
    # 模型配置（JSON格式）
    config: Optional[Dict[str, Any]] = Field(
        default=None,
        description="模型配置（streaming, vision, context_limit, description 等）"
    )
    # 统计数据（可选）
    provider_count: Optional[int] = Field(default=0, description="支持的 Provider 数量")
    usage_count: Optional[int] = Field(default=0, description="调用次数")
    created_at: datetime
    updated_at: Optional[datetime]

    model_config = ConfigDict(from_attributes=True)


class GlobalModelWithStats(GlobalModelResponse):
    """带统计信息的 GlobalModel"""

    total_models: int = Field(..., description="关联的 Model 数量")
    total_providers: int = Field(..., description="支持的 Provider 数量")
    price_range: ModelPriceRange


class GlobalModelListResponse(BaseModel):
    """GlobalModel 列表响应"""

    models: List[GlobalModelResponse]
    total: int


class GlobalModelProvidersResponse(BaseModel):
    """GlobalModel 关联提供商列表响应"""

    providers: List[ModelCatalogProviderDetail]
    total: int


class BatchAssignToProvidersRequest(BaseModel):
    """批量为 Provider 添加 GlobalModel 实现"""

    provider_ids: List[str] = Field(..., min_length=1, description="Provider ID 列表")
    create_models: bool = Field(default=False, description="是否自动创建 Model 记录")


class BatchAssignToProvidersResponse(BaseModel):
    """批量分配响应"""

    success: List[dict]
    errors: List[dict]


class BatchAssignModelsToProviderRequest(BaseModel):
    """批量为 Provider 关联 GlobalModel"""

    global_model_ids: List[str] = Field(..., min_length=1, description="GlobalModel ID 列表")


class BatchAssignModelsToProviderResponse(BaseModel):
    """批量关联 GlobalModel 到 Provider 的响应"""

    success: List[dict]
    errors: List[dict]


class ImportFromUpstreamRequest(BaseModel):
    """从上游提供商导入模型请求"""

    model_ids: List[str] = Field(..., min_length=1, description="上游模型 ID 列表")
    # 价格覆盖配置（应用于所有导入的模型）
    tiered_pricing: Optional[Dict] = Field(
        None,
        description="阶梯计费配置（可选），格式: {tiers: [{up_to, input_price_per_1m, output_price_per_1m, ...}]}"
    )
    price_per_request: Optional[float] = Field(
        None,
        ge=0,
        description="按次计费价格（可选，单位：美元）"
    )


class ImportFromUpstreamSuccessItem(BaseModel):
    """导入成功的模型信息"""

    model_id: str = Field(..., description="上游模型 ID")
    provider_model_id: str = Field(..., description="Provider Model ID")
    global_model_id: Optional[str] = Field("", description="GlobalModel ID（如果已关联）")
    global_model_name: Optional[str] = Field("", description="GlobalModel 名称（如果已关联）")
    created_global_model: bool = Field(False, description="是否新创建了 GlobalModel（始终为 false）")


class ImportFromUpstreamErrorItem(BaseModel):
    """导入失败的模型信息"""

    model_id: str = Field(..., description="上游模型 ID")
    error: str = Field(..., description="错误信息")


class ImportFromUpstreamResponse(BaseModel):
    """从上游提供商导入模型响应"""

    success: List[ImportFromUpstreamSuccessItem]
    errors: List[ImportFromUpstreamErrorItem]


__all__ = [
    "BatchAssignModelsToProviderRequest",
    "BatchAssignModelsToProviderResponse",
    "BatchAssignToProvidersRequest",
    "BatchAssignToProvidersResponse",
    "GlobalModelCreate",
    "GlobalModelListResponse",
    "GlobalModelResponse",
    "GlobalModelUpdate",
    "GlobalModelWithStats",
    "ImportFromUpstreamErrorItem",
    "ImportFromUpstreamRequest",
    "ImportFromUpstreamResponse",
    "ImportFromUpstreamSuccessItem",
    "ModelCapabilities",
    "ModelCatalogItem",
    "ModelCatalogProviderDetail",
    "ModelCatalogResponse",
    "ModelPriceRange",
    "ProviderAvailableSourceModel",
    "ProviderAvailableSourceModelsResponse",
    "ProviderModelPriceInfo",
]
