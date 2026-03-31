"""
Pydantic 数据模型（阶段一统一模型管理）
"""

from __future__ import annotations

from datetime import datetime
from typing import Any

from pydantic import BaseModel, ConfigDict, Field, field_validator, model_validator

# ========== 阶梯计费相关模型 ==========


class CacheTTLPricing(BaseModel):
    """缓存时长定价配置"""

    ttl_minutes: int = Field(..., ge=1, description="缓存时长（分钟）")
    cache_creation_price_per_1m: float = Field(
        ..., ge=0, description="该时长的缓存创建价格/M tokens"
    )
    cache_read_price_per_1m: float | None = Field(
        None, ge=0, description="该时长的缓存读取价格/M tokens"
    )


class PricingTier(BaseModel):
    """单个价格阶梯配置"""

    up_to: int | None = Field(
        None, ge=1, description="阶梯上限（tokens），null 表示无上限（最后一个阶梯）"
    )
    input_price_per_1m: float = Field(..., ge=0, description="输入价格/M tokens")
    output_price_per_1m: float = Field(..., ge=0, description="输出价格/M tokens")
    cache_creation_price_per_1m: float | None = Field(
        None, ge=0, description="缓存创建价格/M tokens"
    )
    cache_read_price_per_1m: float | None = Field(None, ge=0, description="缓存读取价格/M tokens")
    cache_ttl_pricing: list[CacheTTLPricing] | None = Field(
        None, description="按缓存时长分价格（可选）"
    )


class TieredPricingConfig(BaseModel):
    """阶梯计费配置"""

    tiers: list[PricingTier] = Field(
        ..., min_length=1, description="价格阶梯列表，按 up_to 升序排列"
    )

    @model_validator(mode="after")
    def validate_tiers(self) -> TieredPricingConfig:
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
                        raise ValueError("cache_ttl_pricing 必须按 ttl_minutes 升序排列")
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

    min_input: float | None = None
    max_input: float | None = None
    min_output: float | None = None
    max_output: float | None = None


class ModelCatalogProviderDetail(BaseModel):
    """统一模型目录中的关联提供商信息"""

    provider_id: str
    provider_name: str
    model_id: str | None
    target_model: str
    input_price_per_1m: float | None
    output_price_per_1m: float | None
    cache_creation_price_per_1m: float | None
    cache_read_price_per_1m: float | None
    cache_1h_creation_price_per_1m: float | None = None  # 1h 缓存创建价格
    price_per_request: float | None = None  # 按次计费价格
    effective_tiered_pricing: dict[str, Any] | None = None  # 有效阶梯计费配置（含继承）
    tier_count: int = 1  # 阶梯数量
    supports_vision: bool | None = None
    supports_function_calling: bool | None = None
    supports_streaming: bool | None = None
    is_active: bool


class ModelCatalogItem(BaseModel):
    """统一模型目录条目（基于 GlobalModel）"""

    global_model_name: str  # GlobalModel.name
    display_name: str  # GlobalModel.display_name
    description: str | None  # GlobalModel.description
    providers: list[ModelCatalogProviderDetail]  # 支持该模型的 Provider 列表
    price_range: ModelPriceRange  # 价格区间（从所有 Provider 的 Model 中聚合）
    total_providers: int
    capabilities: ModelCapabilities  # 能力聚合（从所有 Provider 的 Model 中聚合）


class ModelCatalogResponse(BaseModel):
    """统一模型目录响应"""

    models: list[ModelCatalogItem]
    total: int


class ProviderModelPriceInfo(BaseModel):
    """Provider 维度的模型价格信息"""

    input_price_per_1m: float | None
    output_price_per_1m: float | None
    cache_creation_price_per_1m: float | None
    cache_read_price_per_1m: float | None
    price_per_request: float | None = None  # 按次计费价格


class ProviderAvailableSourceModel(BaseModel):
    """Provider 支持的统一模型条目"""

    global_model_name: str  # GlobalModel.name
    display_name: str  # GlobalModel.display_name
    provider_model_name: str  # Model.provider_model_name (Provider 侧的模型名)
    model_id: str | None  # Model.id
    price: ProviderModelPriceInfo
    capabilities: ModelCapabilities
    is_active: bool


class ProviderAvailableSourceModelsResponse(BaseModel):
    """Provider 可用统一模型响应"""

    models: list[ProviderAvailableSourceModel]
    total: int


# ========== GlobalModel 相关模型 ==========


class GlobalModelCreate(BaseModel):
    """创建 GlobalModel 请求"""

    name: str = Field(..., min_length=1, max_length=100, description="统一模型名（唯一）")
    display_name: str = Field(..., min_length=1, max_length=100, description="显示名称")
    # 按次计费配置（可选，与阶梯计费叠加）
    default_price_per_request: float | None = Field(None, ge=0, description="每次请求固定费用")
    # 统一阶梯计费配置（必填）
    # 固定价格也用单阶梯表示: {"tiers": [{"up_to": null, "input_price_per_1m": X, ...}]}
    default_tiered_pricing: TieredPricingConfig = Field(
        ..., description="阶梯计费配置（固定价格用单阶梯表示）"
    )
    # Key 能力配置 - 模型支持的能力列表（如 ["cache_1h", "context_1m"]）
    supported_capabilities: list[str] | None = Field(None, description="支持的 Key 能力列表")
    # 模型配置（JSON格式）- 包含能力、规格、元信息等
    config: dict[str, Any] | None = Field(
        None, description="模型配置（streaming, vision, context_limit, description 等）"
    )
    is_active: bool | None = Field(True, description="是否激活")
    model_group_ids: list[str] | None = Field(
        default=None,
        description="所属模型分组 ID 列表；一个模型可属于多个模型分组",
    )

    @field_validator("model_group_ids")
    @classmethod
    def validate_model_group_ids(cls, v: list[str] | None) -> list[str] | None:
        if v is None:
            return None
        normalized: list[str] = []
        seen: set[str] = set()
        for model_group_id in v:
            item = str(model_group_id or "").strip()
            if not item or item in seen:
                continue
            seen.add(item)
            normalized.append(item)
        return normalized


class GlobalModelUpdate(BaseModel):
    """更新 GlobalModel 请求"""

    display_name: str | None = Field(None, min_length=1, max_length=100)
    is_active: bool | None = None
    # 按次计费配置
    default_price_per_request: float | None = Field(None, ge=0, description="每次请求固定费用")
    # 阶梯计费配置
    default_tiered_pricing: TieredPricingConfig | None = Field(None, description="阶梯计费配置")
    # Key 能力配置 - 模型支持的能力列表（如 ["cache_1h", "context_1m"]）
    supported_capabilities: list[str] | None = Field(None, description="支持的 Key 能力列表")
    # 模型配置（JSON格式）- 包含能力、规格、元信息等
    config: dict[str, Any] | None = Field(
        None, description="模型配置（streaming, vision, context_limit, description 等）"
    )
    model_group_ids: list[str] | None = Field(
        default=None,
        description="所属模型分组 ID 列表；传 null 或空列表表示移除全部绑定",
    )

    @field_validator("model_group_ids")
    @classmethod
    def validate_model_group_ids(cls, v: list[str] | None) -> list[str] | None:
        if v is None:
            return None
        normalized: list[str] = []
        seen: set[str] = set()
        for model_group_id in v:
            item = str(model_group_id or "").strip()
            if not item or item in seen:
                continue
            seen.add(item)
            normalized.append(item)
        return normalized


class GlobalModelGroupRef(BaseModel):
    """GlobalModel 关联的模型分组摘要。"""

    id: str
    name: str
    display_name: str
    is_default: bool = False


class GlobalModelResponse(BaseModel):
    """GlobalModel 响应"""

    id: str
    name: str
    display_name: str
    is_active: bool
    # 按次计费配置
    default_price_per_request: float | None = Field(None, description="每次请求固定费用")
    # 阶梯计费配置
    default_tiered_pricing: TieredPricingConfig | None = Field(
        default=None, description="阶梯计费配置"
    )
    # Key 能力配置 - 模型支持的能力列表
    supported_capabilities: list[str] | None = Field(
        default=None, description="支持的 Key 能力列表"
    )
    # 模型配置（JSON格式）
    config: dict[str, Any] | None = Field(
        default=None, description="模型配置（streaming, vision, context_limit, description 等）"
    )
    # 统计数据（可选）
    provider_count: int | None = Field(default=0, description="支持的 Provider 总数量")
    active_provider_count: int | None = Field(default=0, description="可用的 Provider 数量")
    usage_count: int | None = Field(default=0, description="调用次数")
    model_group_ids: list[str] = Field(default_factory=list, description="所属模型分组 ID 列表")
    model_groups: list[GlobalModelGroupRef] = Field(
        default_factory=list,
        description="所属模型分组摘要",
    )
    created_at: datetime
    updated_at: datetime | None

    model_config = ConfigDict(from_attributes=True)


class GlobalModelWithStats(GlobalModelResponse):
    """带统计信息的 GlobalModel"""

    total_models: int = Field(..., description="关联的 Model 数量")
    total_providers: int = Field(..., description="支持的 Provider 数量")
    price_range: ModelPriceRange


class GlobalModelListResponse(BaseModel):
    """GlobalModel 列表响应"""

    models: list[GlobalModelResponse]
    total: int


class GlobalModelProvidersResponse(BaseModel):
    """GlobalModel 关联提供商列表响应"""

    providers: list[ModelCatalogProviderDetail]
    total: int


class BatchAssignToProvidersRequest(BaseModel):
    """批量为 Provider 添加 GlobalModel 实现"""

    provider_ids: list[str] = Field(..., min_length=1, description="Provider ID 列表")
    create_models: bool = Field(default=False, description="是否自动创建 Model 记录")


class ModelGroupRouteRequest(BaseModel):
    """模型分组路由配置。"""

    provider_id: str = Field(..., min_length=1, description="Provider ID")
    provider_api_key_id: str | None = Field(
        default=None,
        description="Provider API Key ID；null 表示该 Provider 下全部 Key",
    )
    priority: int = Field(default=50, ge=0, le=999999, description="分组内优先级")
    user_billing_multiplier_override: float | None = Field(
        default=None,
        ge=0,
        description="用户计费倍率覆盖；null 表示使用模型分组默认倍率",
    )
    is_active: bool = Field(default=True, description="是否启用")
    notes: str | None = Field(default=None, max_length=500, description="备注")


class ModelGroupRouteResponse(ModelGroupRouteRequest):
    """模型分组路由配置响应。"""

    id: str
    provider_name: str | None = None
    provider_api_key_name: str | None = None


class ModelGroupModelRef(BaseModel):
    """模型分组中的模型摘要。"""

    id: str
    global_model_id: str
    model_name: str
    model_display_name: str
    is_active: bool


class ModelGroupUserGroupRef(BaseModel):
    """模型分组被哪些用户分组引用。"""

    user_group_id: str
    user_group_name: str
    priority: int
    is_active: bool


class ModelGroupCreate(BaseModel):
    """创建模型分组请求。"""

    name: str = Field(..., min_length=1, max_length=100)
    display_name: str = Field(..., min_length=1, max_length=100)
    description: str | None = Field(None, max_length=500)
    default_user_billing_multiplier: float = Field(default=1.0, ge=0)
    routing_mode: str = Field(default="inherit", pattern="^(inherit|custom)$")
    is_active: bool = Field(default=True)
    sort_order: int = Field(default=100, ge=0, le=999999)
    model_ids: list[str] = Field(default_factory=list, description="绑定的 GlobalModel ID 列表")
    routes: list[ModelGroupRouteRequest] = Field(default_factory=list, description="路由策略列表")


class ModelGroupUpdate(BaseModel):
    """更新模型分组请求。"""

    name: str | None = Field(None, min_length=1, max_length=100)
    display_name: str | None = Field(None, min_length=1, max_length=100)
    description: str | None = Field(None, max_length=500)
    default_user_billing_multiplier: float | None = Field(default=None, ge=0)
    routing_mode: str | None = Field(default=None, pattern="^(inherit|custom)$")
    is_active: bool | None = None
    sort_order: int | None = Field(default=None, ge=0, le=999999)
    model_ids: list[str] | None = Field(default=None, description="完整替换的 GlobalModel ID 列表")
    routes: list[ModelGroupRouteRequest] | None = Field(
        default=None,
        description="完整替换的路由策略列表",
    )


class ModelGroupResponse(BaseModel):
    """模型分组基础响应。"""

    id: str
    name: str
    display_name: str
    description: str | None = None
    default_user_billing_multiplier: float
    routing_mode: str
    is_default: bool = False
    is_active: bool = True
    sort_order: int
    model_count: int = 0
    user_group_count: int = 0
    created_at: datetime
    updated_at: datetime | None


class ModelGroupDetailResponse(ModelGroupResponse):
    """模型分组详情响应。"""

    models: list[ModelGroupModelRef] = Field(default_factory=list)
    routes: list[ModelGroupRouteResponse] = Field(default_factory=list)
    user_groups: list[ModelGroupUserGroupRef] = Field(default_factory=list)


class ModelGroupListResponse(BaseModel):
    """模型分组列表响应。"""

    model_groups: list[ModelGroupResponse]
    total: int


class BatchAssignToProvidersResponse(BaseModel):
    """批量分配响应"""

    success: list[dict]
    errors: list[dict]


class BatchAssignModelsToProviderRequest(BaseModel):
    """批量为 Provider 关联 GlobalModel"""

    global_model_ids: list[str] = Field(..., min_length=1, description="GlobalModel ID 列表")


class BatchAssignModelsToProviderResponse(BaseModel):
    """批量关联 GlobalModel 到 Provider 的响应"""

    success: list[dict]
    errors: list[dict]


class ImportFromUpstreamRequest(BaseModel):
    """从上游提供商导入模型请求"""

    model_ids: list[str] = Field(..., min_length=1, description="上游模型 ID 列表")
    # 价格覆盖配置（应用于所有导入的模型）
    tiered_pricing: dict | None = Field(
        None,
        description="阶梯计费配置（可选），格式: {tiers: [{up_to, input_price_per_1m, output_price_per_1m, ...}]}",
    )
    price_per_request: float | None = Field(
        None, ge=0, description="按次计费价格（可选，单位：美元）"
    )


class ImportFromUpstreamSuccessItem(BaseModel):
    """导入成功的模型信息"""

    model_id: str = Field(..., description="上游模型 ID")
    provider_model_id: str = Field(..., description="Provider Model ID")
    global_model_id: str = Field(..., description="GlobalModel ID")
    global_model_name: str = Field(..., description="GlobalModel 名称")
    created_global_model: bool = Field(
        False, description="是否新创建了 GlobalModel（始终为 false）"
    )


class ImportFromUpstreamErrorItem(BaseModel):
    """导入失败的模型信息"""

    model_id: str = Field(..., description="上游模型 ID")
    error: str = Field(..., description="错误信息")


class ImportFromUpstreamResponse(BaseModel):
    """从上游提供商导入模型响应"""

    success: list[ImportFromUpstreamSuccessItem]
    errors: list[ImportFromUpstreamErrorItem]


__all__ = [
    "BatchAssignModelsToProviderRequest",
    "BatchAssignModelsToProviderResponse",
    "BatchAssignToProvidersRequest",
    "BatchAssignToProvidersResponse",
    "GlobalModelCreate",
    "GlobalModelGroupRef",
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
    "ModelGroupCreate",
    "ModelGroupDetailResponse",
    "ModelGroupListResponse",
    "ModelGroupModelRef",
    "ModelPriceRange",
    "ModelGroupResponse",
    "ModelGroupRouteRequest",
    "ModelGroupRouteResponse",
    "ModelGroupUpdate",
    "ModelGroupUserGroupRef",
    "ProviderAvailableSourceModel",
    "ProviderAvailableSourceModelsResponse",
    "ProviderModelPriceInfo",
]
