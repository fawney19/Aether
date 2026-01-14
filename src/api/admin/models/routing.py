"""
GlobalModel 请求链路预览 API

提供模型的请求链路信息，包括：
- 请求会流向哪些提供商
- 每个提供商的优先级和负载均衡配置
- 模型名称映射关系
- Key 的并发配置和健康状态
"""

from dataclasses import dataclass
from typing import Dict, List, Optional

from fastapi import APIRouter, Depends, Request
from pydantic import BaseModel, ConfigDict, Field
from sqlalchemy.orm import Session, selectinload

from src.api.base.admin_adapter import AdminApiAdapter
from src.api.base.pipeline import ApiRequestPipeline
from src.core.model_permissions import parse_allowed_models_to_list
from src.database import get_db
from src.models.database import (
    GlobalModel,
    Model,
    ProviderAPIKey,
    ProviderEndpoint,
)
from src.services.cache.aware_scheduler import CacheAwareScheduler
from src.services.system.config import SystemConfigService

router = APIRouter(prefix="/global", tags=["Admin - Global Models"])
pipeline = ApiRequestPipeline()


# ========== Response Models ==========


class RoutingKeyInfo(BaseModel):
    """Key 路由信息"""

    id: str
    name: str
    masked_key: str = Field("", description="脱敏的 API Key")
    internal_priority: int = Field(..., description="Key 内部优先级")
    global_priority: Optional[int] = Field(None, description="全局 Key 优先级")
    rpm_limit: Optional[int] = Field(None, description="RPM 限制，null 表示自适应")
    is_adaptive: bool = Field(False, description="是否为自适应 RPM 模式")
    effective_rpm: Optional[int] = Field(None, description="有效 RPM 限制")
    cache_ttl_minutes: int = Field(0, description="缓存 TTL（分钟）")
    health_score: float = Field(1.0, description="健康度分数（0-1 小数格式）")
    is_active: bool
    api_formats: List[str] = Field(default_factory=list, description="支持的 API 格式")
    # 模型白名单
    allowed_models: Optional[List[str]] = Field(None, description="允许的模型列表，null 表示不限制")
    # 熔断状态
    circuit_breaker_open: bool = Field(False, description="熔断器是否打开")
    circuit_breaker_formats: List[str] = Field(default_factory=list, description="熔断的 API 格式列表")
    next_probe_at: Optional[str] = Field(None, description="下次探测时间（ISO格式）")

    model_config = ConfigDict(from_attributes=True)


class RoutingEndpointInfo(BaseModel):
    """Endpoint 路由信息"""

    id: str
    api_format: str
    base_url: str
    custom_path: Optional[str] = None
    is_active: bool
    keys: List[RoutingKeyInfo] = Field(default_factory=list)
    total_keys: int = 0
    active_keys: int = 0

    model_config = ConfigDict(from_attributes=True)


class RoutingModelMapping(BaseModel):
    """模型名称映射信息"""

    name: str = Field(..., description="映射名称")
    priority: int = Field(..., description="优先级（数字越小优先级越高）")
    api_formats: Optional[List[str]] = Field(None, description="作用域（适用的 API 格式）")


class RoutingProviderInfo(BaseModel):
    """Provider 路由信息"""

    id: str
    name: str
    model_id: str = Field(..., description="Model ID（GlobalModel 与 Provider 的关联记录 ID）")
    provider_priority: int = Field(..., description="提供商优先级（数字越小优先级越高）")
    billing_type: Optional[str] = Field(None, description="计费类型")
    monthly_quota_usd: Optional[float] = Field(None, description="月额度（美元）")
    monthly_used_usd: Optional[float] = Field(None, description="已用额度（美元）")
    is_active: bool
    # 模型映射信息
    provider_model_name: str = Field(..., description="提供商侧的模型名称")
    model_mappings: List[RoutingModelMapping] = Field(
        default_factory=list, description="模型名称映射列表"
    )
    model_is_active: bool = Field(True, description="Model 是否活跃")
    # Endpoint 和 Key 信息
    endpoints: List[RoutingEndpointInfo] = Field(default_factory=list)
    total_endpoints: int = 0
    active_endpoints: int = 0

    model_config = ConfigDict(from_attributes=True)


class ModelRoutingPreviewResponse(BaseModel):
    """模型请求链路预览响应"""

    global_model_id: str
    global_model_name: str
    display_name: str
    is_active: bool
    # 链路信息
    providers: List[RoutingProviderInfo] = Field(
        default_factory=list, description="按优先级排序的提供商列表"
    )
    total_providers: int = 0
    active_providers: int = 0
    # 调度配置
    scheduling_mode: str = Field("cache_affinity", description="调度模式")
    priority_mode: str = Field("provider", description="优先级模式")

    model_config = ConfigDict(from_attributes=True)


# ========== API Endpoints ==========


@router.get("/{global_model_id}/routing", response_model=ModelRoutingPreviewResponse)
async def get_model_routing_preview(
    request: Request,
    global_model_id: str,
    db: Session = Depends(get_db),
) -> ModelRoutingPreviewResponse:
    """
    获取模型请求链路预览

    查看指定 GlobalModel 的完整请求链路信息，包括：
    - 关联的所有提供商及其优先级
    - 每个提供商的模型名称映射配置
    - Endpoint 和 Key 的详细配置
    - 负载均衡和调度策略

    **路径参数**:
    - `global_model_id`: GlobalModel ID

    **返回字段**:
    - `global_model_id`: GlobalModel ID
    - `global_model_name`: 模型名称
    - `display_name`: 显示名称
    - `is_active`: 是否活跃
    - `providers`: 按优先级排序的提供商列表，每个包含：
      - `id`: Provider ID
      - `name`: Provider 名称
      - `provider_priority`: 提供商优先级
      - `provider_model_name`: 提供商侧的模型名称
      - `model_mappings`: 模型名称映射列表
      - `endpoints`: Endpoint 列表，每个包含 Key 信息
    - `scheduling_mode`: 调度模式（cache_affinity, fixed_order, load_balance）
    - `priority_mode`: 优先级模式（provider, global_key）
    """
    adapter = AdminGetModelRoutingPreviewAdapter(global_model_id=global_model_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


# ========== Adapters ==========


@dataclass
class AdminGetModelRoutingPreviewAdapter(AdminApiAdapter):
    """获取模型请求链路预览"""

    global_model_id: str

    async def handle(self, context) -> ModelRoutingPreviewResponse:  # type: ignore[override]
        db = context.db

        # 获取 GlobalModel
        global_model = (
            db.query(GlobalModel).filter(GlobalModel.id == self.global_model_id).first()
        )
        if not global_model:
            from fastapi import HTTPException

            raise HTTPException(status_code=404, detail="GlobalModel not found")

        # 获取所有关联的 Model（包含 Provider 信息）
        models = (
            db.query(Model)
            .options(selectinload(Model.provider))
            .filter(Model.global_model_id == global_model.id)
            .all()
        )

        # 获取所有相关的 Provider ID
        provider_ids = [m.provider_id for m in models if m.provider_id]

        # 批量获取 Provider 的 Endpoints
        endpoints_by_provider: Dict[str, List[ProviderEndpoint]] = {}
        if provider_ids:
            endpoints = (
                db.query(ProviderEndpoint)
                .filter(ProviderEndpoint.provider_id.in_(provider_ids))
                .all()
            )
            for ep in endpoints:
                if ep.provider_id not in endpoints_by_provider:
                    endpoints_by_provider[ep.provider_id] = []
                endpoints_by_provider[ep.provider_id].append(ep)

        # 批量获取 Provider 的 Keys
        keys_by_provider: Dict[str, List[ProviderAPIKey]] = {}
        if provider_ids:
            keys = (
                db.query(ProviderAPIKey)
                .filter(ProviderAPIKey.provider_id.in_(provider_ids))
                .all()
            )
            for key in keys:
                if key.provider_id not in keys_by_provider:
                    keys_by_provider[key.provider_id] = []
                keys_by_provider[key.provider_id].append(key)

        # 构建 Provider 路由信息
        provider_infos: List[RoutingProviderInfo] = []
        for model in models:
            provider = model.provider
            if not provider:
                continue

            # 获取模型映射
            model_mappings = []
            if model.provider_model_mappings:
                for mapping in model.provider_model_mappings:
                    model_mappings.append(
                        RoutingModelMapping(
                            name=mapping.get("name", ""),
                            priority=mapping.get("priority", 0),
                            api_formats=mapping.get("api_formats"),
                        )
                    )

            # 获取 Endpoints
            provider_endpoints = endpoints_by_provider.get(provider.id, [])
            provider_keys = keys_by_provider.get(provider.id, [])

            # 按 api_format 组织 Keys
            keys_by_endpoint: Dict[str, List[ProviderAPIKey]] = {}
            for key in provider_keys:
                # 每个 Key 可能支持多个 api_formats
                for fmt in key.api_formats or []:
                    if fmt not in keys_by_endpoint:
                        keys_by_endpoint[fmt] = []
                    keys_by_endpoint[fmt].append(key)

            endpoint_infos = []
            for ep in provider_endpoints:
                # 获取该 Endpoint 格式对应的 Keys
                ep_keys = keys_by_endpoint.get(ep.api_format or "", [])
                # 按优先级排序
                ep_keys.sort(key=lambda k: (k.global_priority or 999, k.internal_priority or 0))

                key_infos = []
                for key in ep_keys:
                    # 计算有效 RPM
                    effective_rpm = key.rpm_limit
                    is_adaptive = key.rpm_limit is None
                    if is_adaptive and key.learned_rpm_limit:
                        effective_rpm = key.learned_rpm_limit

                    # 从 health_by_format 获取健康度（0-1 小数格式）
                    health_score = 1.0
                    if key.health_by_format and ep.api_format:
                        format_health = key.health_by_format.get(ep.api_format, {})
                        health_score = format_health.get("health_score", 1.0)

                    # 生成脱敏 SK（先解密再脱敏）
                    masked_key = ""
                    if key.api_key:
                        from src.core.crypto import CryptoService
                        crypto = CryptoService()
                        try:
                            decrypted_key = crypto.decrypt(key.api_key, silent=True)
                        except Exception:
                            # 解密失败时使用加密后的值（可能是未加密的旧数据）
                            decrypted_key = key.api_key
                        if len(decrypted_key) > 8:
                            masked_key = f"{decrypted_key[:4]}***{decrypted_key[-4:]}"
                        else:
                            masked_key = f"{decrypted_key[:2]}***"

                    # 检查熔断状态
                    circuit_breaker_open = False
                    circuit_breaker_formats: List[str] = []
                    next_probe_at: Optional[str] = None
                    if key.circuit_breaker_by_format:
                        for fmt, cb_state in key.circuit_breaker_by_format.items():
                            if isinstance(cb_state, dict) and cb_state.get("open"):
                                circuit_breaker_open = True
                                circuit_breaker_formats.append(fmt)
                                # 取最早的探测时间
                                fmt_next_probe = cb_state.get("next_probe_at")
                                if fmt_next_probe:
                                    if next_probe_at is None or fmt_next_probe < next_probe_at:
                                        next_probe_at = fmt_next_probe

                    # 解析 allowed_models
                    # 语义说明：
                    # - None: 不限制（允许所有模型）
                    # - {}: 空字典 = 不限制（normalize_allowed_models 返回 None）
                    # - []: 空列表 = 拒绝所有模型
                    # - {"CLAUDE": []}: 指定格式空列表 = 该格式拒绝所有
                    raw_allowed_models = key.allowed_models
                    if raw_allowed_models is None:
                        allowed_models_list = None
                    elif isinstance(raw_allowed_models, dict) and not raw_allowed_models:
                        # 空 dict {} 在语义上等价于不限制
                        allowed_models_list = None
                    else:
                        allowed_models_list = parse_allowed_models_to_list(raw_allowed_models)

                    key_infos.append(
                        RoutingKeyInfo(
                            id=key.id or "",
                            name=key.name or "",
                            masked_key=masked_key,
                            internal_priority=key.internal_priority or 0,
                            global_priority=key.global_priority,
                            rpm_limit=key.rpm_limit,
                            is_adaptive=is_adaptive,
                            effective_rpm=effective_rpm,
                            cache_ttl_minutes=key.cache_ttl_minutes or 0,
                            health_score=health_score,
                            is_active=bool(key.is_active),
                            api_formats=key.api_formats or [],
                            allowed_models=allowed_models_list,
                            circuit_breaker_open=circuit_breaker_open,
                            circuit_breaker_formats=circuit_breaker_formats,
                            next_probe_at=next_probe_at,
                        )
                    )

                active_keys = sum(1 for k in key_infos if k.is_active)
                endpoint_infos.append(
                    RoutingEndpointInfo(
                        id=ep.id or "",
                        api_format=ep.api_format or "",
                        base_url=ep.base_url or "",
                        custom_path=ep.custom_path,
                        is_active=bool(ep.is_active),
                        keys=key_infos,
                        total_keys=len(key_infos),
                        active_keys=active_keys,
                    )
                )

            # 按 APIFormat 枚举定义的顺序排序 Endpoints
            from src.core.enums import APIFormat

            format_order = {fmt.value: i for i, fmt in enumerate(APIFormat)}
            endpoint_infos.sort(key=lambda e: format_order.get(e.api_format, 999))

            active_endpoints = sum(1 for e in endpoint_infos if e.is_active)
            provider_infos.append(
                RoutingProviderInfo(
                    id=provider.id,
                    name=provider.name,
                    model_id=model.id,
                    provider_priority=provider.provider_priority,
                    billing_type=provider.billing_type,
                    monthly_quota_usd=provider.monthly_quota_usd,
                    monthly_used_usd=provider.monthly_used_usd,
                    is_active=bool(provider.is_active),
                    provider_model_name=model.provider_model_name,
                    model_mappings=model_mappings,
                    model_is_active=bool(model.is_active),
                    endpoints=endpoint_infos,
                    total_endpoints=len(endpoint_infos),
                    active_endpoints=active_endpoints,
                )
            )

        # 按 provider_priority 排序
        provider_infos.sort(key=lambda p: p.provider_priority)

        active_providers = sum(1 for p in provider_infos if p.is_active and p.model_is_active)

        # 从数据库获取当前调度配置
        scheduling_mode = SystemConfigService.get_config(
            db,
            "scheduling_mode",
            CacheAwareScheduler.SCHEDULING_MODE_CACHE_AFFINITY,
        ) or CacheAwareScheduler.SCHEDULING_MODE_CACHE_AFFINITY
        priority_mode = SystemConfigService.get_config(
            db,
            "provider_priority_mode",
            CacheAwareScheduler.PRIORITY_MODE_PROVIDER,
        ) or CacheAwareScheduler.PRIORITY_MODE_PROVIDER

        return ModelRoutingPreviewResponse(
            global_model_id=global_model.id,
            global_model_name=global_model.name,
            display_name=global_model.display_name,
            is_active=bool(global_model.is_active),
            providers=provider_infos,
            total_providers=len(provider_infos),
            active_providers=active_providers,
            scheduling_mode=scheduling_mode,
            priority_mode=priority_mode,
        )
