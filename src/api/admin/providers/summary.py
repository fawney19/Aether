"""
Provider 摘要与健康监控 API
"""

from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from typing import Any, Protocol

from fastapi import APIRouter, Depends, Query, Request
from sqlalchemy import case, func
from sqlalchemy.orm import Session, load_only

from src.api.base.admin_adapter import AdminApiAdapter
from src.api.base.context import ApiRequestContext
from src.api.base.models_service import invalidate_models_list_cache
from src.api.base.pipeline import get_pipeline
from src.config.constants import CacheTTL
from src.core.enums import ProviderBillingType
from src.core.exceptions import InvalidRequestException, NotFoundException
from src.core.logger import logger
from src.database import get_db
from src.models.admin_requests import (
    ClaudeCodeAdvancedConfig,
    ErrorPassthroughRulesConfig,
    FailoverRulesConfig,
    PoolAdvancedConfig,
)
from src.models.database import (
    Model,
    Provider,
    ProviderAPIKey,
    ProviderEndpoint,
    RequestCandidate,
)
from src.models.endpoint_models import (
    EndpointHealthEvent,
    EndpointHealthMonitor,
    ProviderEndpointHealthMonitorResponse,
    ProviderSummaryPageResponse,
    ProviderUpdateRequest,
    ProviderWithEndpointsSummary,
)
from src.services.cache.model_cache import ModelCacheService
from src.services.cache.provider_cache import ProviderCacheService
from src.utils.cache_decorator import cache_result


class _HasProviderSortFields(Protocol):
    is_active: Any
    provider_priority: Any
    created_at: Any


router = APIRouter(tags=["Provider Summary"])
pipeline = get_pipeline()


def _provider_summary_ordering(provider_model: _HasProviderSortFields) -> tuple[Any, Any, Any]:
    """Provider 摘要列表排序：启用在前，其次按优先级与创建时间。"""
    return (
        case((provider_model.is_active == True, 0), else_=1).asc(),
        provider_model.provider_priority.asc(),
        provider_model.created_at.asc(),
    )


@router.get("/summary", response_model=ProviderSummaryPageResponse)
async def get_providers_summary(
    request: Request,
    page: int = Query(1, ge=1),
    page_size: int = Query(20, ge=1, le=10000),
    search: str = Query("", description="按名称搜索"),
    status: str = Query("all", description="all/active/inactive"),
    api_format: str = Query("all", description="API 格式筛选"),
    model_id: str = Query("all", description="全局模型 ID 筛选"),
    db: Session = Depends(get_db),
) -> ProviderSummaryPageResponse:
    """获取提供商摘要信息（分页）"""
    adapter = AdminProviderSummaryAdapter(
        page=page,
        page_size=page_size,
        search=search,
        status=status,
        api_format=api_format,
        model_id=model_id,
    )
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/{provider_id}/summary", response_model=ProviderWithEndpointsSummary)
async def get_provider_summary(
    provider_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> ProviderWithEndpointsSummary:
    """
    获取单个提供商摘要信息

    获取指定提供商的详细摘要信息，包含端点、密钥、模型统计和健康状态。

    **路径参数**:
    - `provider_id`: 提供商 ID

    **返回字段**:
    - `id`: 提供商 ID
    - `name`: 提供商名称
    - `description`: 描述信息
    - `website`: 官网地址
    - `provider_priority`: 优先级
    - `is_active`: 是否启用
    - `billing_type`: 计费类型
    - `monthly_quota_usd`: 月度配额（美元）
    - `monthly_used_usd`: 本月已使用金额（美元）
    - `quota_reset_day`: 配额重置日期
    - `quota_last_reset_at`: 上次配额重置时间
    - `quota_expires_at`: 配额过期时间
    - `timeout`: 默认请求超时（秒）
    - `max_retries`: 默认最大重试次数
    - `proxy`: 默认代理配置
    - `total_endpoints`: 端点总数
    - `active_endpoints`: 活跃端点数
    - `total_keys`: 密钥总数
    - `active_keys`: 活跃密钥数
    - `total_models`: 模型总数
    - `active_models`: 活跃模型数
    - `avg_health_score`: 平均健康分数（0-1）
    - `unhealthy_endpoints`: 不健康端点数（健康分数 < 0.5）
    - `api_formats`: 支持的 API 格式列表
    - `endpoint_health_details`: 端点健康详情（包含 api_format, health_score, is_active, active_keys）
    - `created_at`: 创建时间
    - `updated_at`: 更新时间
    """
    adapter = AdminProviderDetailAdapter(provider_id=provider_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/{provider_id}/health-monitor", response_model=ProviderEndpointHealthMonitorResponse)
async def get_provider_health_monitor(
    provider_id: str,
    request: Request,
    lookback_hours: int = Query(6, ge=1, le=72, description="回溯的小时数"),
    per_endpoint_limit: int = Query(48, ge=10, le=200, description="每个端点的事件数量"),
    db: Session = Depends(get_db),
) -> ProviderEndpointHealthMonitorResponse:
    """
    获取提供商健康监控数据

    获取指定提供商下所有端点的健康监控时间线，包含请求成功率、延迟、错误信息等。

    **路径参数**:
    - `provider_id`: 提供商 ID

    **查询参数**:
    - `lookback_hours`: 回溯的小时数，范围 1-72，默认为 6
    - `per_endpoint_limit`: 每个端点返回的事件数量，范围 10-200，默认为 48

    **返回字段**:
    - `provider_id`: 提供商 ID
    - `provider_name`: 提供商名称
    - `generated_at`: 生成时间
    - `endpoints`: 端点健康监控数据数组，每项包含：
      - `endpoint_id`: 端点 ID
      - `api_format`: API 格式
      - `is_active`: 是否活跃
      - `total_attempts`: 总请求次数
      - `success_count`: 成功次数
      - `failed_count`: 失败次数
      - `skipped_count`: 跳过次数
      - `success_rate`: 成功率（0-1）
      - `last_event_at`: 最后事件时间
      - `events`: 事件详情数组（包含 timestamp, status, status_code, latency_ms, error_type, error_message）
    """

    adapter = AdminProviderHealthMonitorAdapter(
        provider_id=provider_id,
        lookback_hours=lookback_hours,
        per_endpoint_limit=per_endpoint_limit,
    )
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.patch("/{provider_id}", response_model=ProviderWithEndpointsSummary)
async def update_provider_settings(
    provider_id: str,
    update_data: ProviderUpdateRequest,
    request: Request,
    db: Session = Depends(get_db),
) -> ProviderWithEndpointsSummary:
    """
    更新提供商基础配置

    更新提供商的基础配置信息，如名称、描述、优先级等。只需传入需要更新的字段。

    **路径参数**:
    - `provider_id`: 提供商 ID

    **请求体字段**（所有字段可选）:
    - `name`: 提供商名称
    - `description`: 描述信息
    - `website`: 官网地址
    - `provider_priority`: 优先级
    - `is_active`: 是否启用
    - `billing_type`: 计费类型
    - `monthly_quota_usd`: 月度配额（美元）
    - `quota_reset_day`: 配额重置日期
    - `quota_expires_at`: 配额过期时间
    - `timeout`: 默认请求超时（秒）
    - `max_retries`: 默认最大重试次数
    - `proxy`: 默认代理配置

    **返回字段**: 返回更新后的提供商摘要信息（与 GET /summary 接口返回格式相同）
    """

    adapter = AdminUpdateProviderSettingsAdapter(provider_id=provider_id, update_data=update_data)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


def _extract_pool_advanced_from_config(
    provider_config: dict[str, Any] | None,
    *,
    provider_id: str,
) -> PoolAdvancedConfig | None:
    """从 Provider.config 中安全提取通用号池配置。

    优先查找 ``pool_advanced``，回退查找 ``claude_code_advanced`` 中的号池字段。
    """
    cfg = provider_config or {}
    raw = cfg.get("pool_advanced")
    if raw is None:
        return None

    if isinstance(raw, PoolAdvancedConfig):
        return raw

    if not isinstance(raw, dict):
        logger.warning(
            "Provider {} 的 pool_advanced 类型无效: {}，已忽略",
            provider_id,
            type(raw).__name__,
        )
        return None

    try:
        return PoolAdvancedConfig.model_validate(raw)
    except Exception as exc:
        logger.warning(
            "Provider {} 的 pool_advanced 配置无效，已忽略: {}",
            provider_id,
            str(exc),
        )
        return None


def _extract_claude_code_advanced_from_config(
    provider_config: dict[str, Any] | None,
    *,
    provider_id: str,
) -> ClaudeCodeAdvancedConfig | None:
    """从 Provider.config 中安全提取 Claude Code 高级配置。"""
    raw_config = (provider_config or {}).get("claude_code_advanced")
    if raw_config is None:
        return None

    if isinstance(raw_config, ClaudeCodeAdvancedConfig):
        return raw_config

    if not isinstance(raw_config, dict):
        logger.warning(
            "Provider {} 的 claude_code_advanced 类型无效: {}，已忽略",
            provider_id,
            type(raw_config).__name__,
        )
        return None

    try:
        return ClaudeCodeAdvancedConfig.model_validate(raw_config)
    except Exception as exc:
        logger.warning(
            "Provider {} 的 claude_code_advanced 配置无效，已忽略: {}",
            provider_id,
            str(exc),
        )
        return None


def _extract_failover_rules_from_config(
    provider_config: dict[str, Any] | None,
    *,
    provider_id: str,
) -> FailoverRulesConfig | None:
    """从 Provider.config 中安全提取故障转移规则配置。"""
    raw = (provider_config or {}).get("failover_rules")
    if raw is None:
        return None

    if isinstance(raw, FailoverRulesConfig):
        return raw

    if not isinstance(raw, dict):
        logger.warning(
            "Provider {} 的 failover_rules 类型无效: {}，已忽略",
            provider_id,
            type(raw).__name__,
        )
        return None

    try:
        return FailoverRulesConfig.model_validate(raw)
    except Exception as exc:
        logger.warning(
            "Provider {} 的 failover_rules 配置无效，已忽略: {}",
            provider_id,
            str(exc),
        )
        return None


def _extract_error_passthrough_rules_from_config(
    provider_config: dict[str, Any] | None,
    *,
    provider_id: str,
) -> ErrorPassthroughRulesConfig | None:
    """从 Provider.config 中安全提取错误透传规则配置。"""
    raw = (provider_config or {}).get("error_passthrough_rules")
    if raw is None:
        return None

    if isinstance(raw, ErrorPassthroughRulesConfig):
        return raw

    if not isinstance(raw, dict):
        logger.warning(
            "Provider {} 的 error_passthrough_rules 类型无效: {}，已忽略",
            provider_id,
            type(raw).__name__,
        )
        return None

    try:
        return ErrorPassthroughRulesConfig.model_validate(raw)
    except Exception as exc:
        logger.warning(
            "Provider {} 的 error_passthrough_rules 配置无效，已忽略: {}",
            provider_id,
            str(exc),
        )
        return None


def _build_provider_summary(db: Session, provider: Provider) -> ProviderWithEndpointsSummary:
    endpoints = (
        db.query(ProviderEndpoint)
        .options(
            load_only(
                ProviderEndpoint.id,
                ProviderEndpoint.provider_id,
                ProviderEndpoint.api_format,
                ProviderEndpoint.is_active,
            )
        )
        .filter(ProviderEndpoint.provider_id == provider.id)
        .all()
    )

    key_stats = (
        db.query(
            func.count(ProviderAPIKey.id).label("total"),
            func.sum(case((ProviderAPIKey.is_active == True, 1), else_=0)).label("active"),
        )
        .filter(ProviderAPIKey.provider_id == provider.id)
        .first()
    )
    total_keys = int(key_stats.total or 0)
    active_keys = int(key_stats.active or 0)

    model_stats = (
        db.query(
            func.count(Model.id).label("total"),
            func.sum(case((Model.is_active == True, 1), else_=0)).label("active"),
        )
        .filter(Model.provider_id == provider.id)
        .first()
    )
    total_models = int(model_stats.total or 0)
    active_models = int(model_stats.active or 0)

    global_model_ids = [
        row[0]
        for row in db.query(Model.global_model_id)
        .filter(
            Model.provider_id == provider.id,
            Model.is_active == True,
            Model.global_model_id.isnot(None),
        )
        .distinct()
        .all()
    ]

    all_keys = (
        db.query(ProviderAPIKey)
        .options(
            load_only(
                ProviderAPIKey.id,
                ProviderAPIKey.provider_id,
                ProviderAPIKey.is_active,
                ProviderAPIKey.api_formats,
                ProviderAPIKey.health_by_format,
            )
        )
        .filter(ProviderAPIKey.provider_id == provider.id)
        .all()
    )

    return _compose_provider_summary(
        provider=provider,
        endpoints=endpoints,
        all_keys=all_keys,
        total_keys=total_keys,
        active_keys=active_keys,
        total_models=total_models,
        active_models=active_models,
        global_model_ids=global_model_ids,
    )


def _compose_provider_summary(
    *,
    provider: Provider,
    endpoints: list[ProviderEndpoint],
    all_keys: list[ProviderAPIKey],
    total_keys: int,
    active_keys: int,
    total_models: int,
    active_models: int,
    global_model_ids: list[Any],
) -> ProviderWithEndpointsSummary:
    total_endpoints = len(endpoints)
    active_endpoints = sum(1 for e in endpoints if e.is_active)
    api_formats = [e.api_format for e in endpoints]

    # 按 api_formats 分组 keys（通过 api_formats 关联）
    format_to_endpoint_id: dict[str, str] = {e.api_format: e.id for e in endpoints}
    keys_by_endpoint: dict[str, list[ProviderAPIKey]] = {e.id: [] for e in endpoints}
    for key in all_keys:
        formats = key.api_formats or []
        for fmt in formats:
            endpoint_id = format_to_endpoint_id.get(fmt)
            if endpoint_id:
                keys_by_endpoint[endpoint_id].append(key)

    endpoint_health_map: dict[str, float] = {}
    for endpoint in endpoints:
        keys = keys_by_endpoint.get(endpoint.id, [])
        if keys:
            api_fmt = endpoint.api_format
            health_scores: list[float] = []
            for k in keys:
                health_by_format = k.health_by_format or {}
                if api_fmt in health_by_format:
                    score = health_by_format[api_fmt].get("health_score")
                    if score is not None:
                        health_scores.append(float(score))
                else:
                    health_scores.append(1.0)
            avg_health = sum(health_scores) / len(health_scores) if health_scores else 1.0
            endpoint_health_map[endpoint.id] = avg_health
        else:
            endpoint_health_map[endpoint.id] = 1.0

    all_health_scores = list(endpoint_health_map.values())
    avg_health_score = sum(all_health_scores) / len(all_health_scores) if all_health_scores else 1.0
    unhealthy_endpoints = sum(1 for score in all_health_scores if score < 0.5)

    active_keys_by_endpoint: dict[str, int] = {}
    for endpoint_id, keys in keys_by_endpoint.items():
        active_keys_by_endpoint[endpoint_id] = sum(1 for k in keys if k.is_active)

    endpoint_health_details = [
        {
            "api_format": e.api_format,
            "health_score": endpoint_health_map.get(e.id, 1.0),
            "is_active": e.is_active,
            "total_keys": len(keys_by_endpoint.get(e.id, [])),
            "active_keys": active_keys_by_endpoint.get(e.id, 0),
        }
        for e in endpoints
    ]

    provider_config_raw = provider.config
    provider_config = provider_config_raw if isinstance(provider_config_raw, dict) else {}
    if provider_config_raw is not None and not isinstance(provider_config_raw, dict):
        logger.warning(
            "Provider {} 的 config 类型无效: {}，按空配置处理",
            provider.id,
            type(provider_config_raw).__name__,
        )

    # 检查是否配置了 Provider Ops（余额监控等）
    provider_ops_config = provider_config.get("provider_ops")
    ops_configured = bool(provider_ops_config)
    ops_architecture_id = (
        provider_ops_config.get("architecture_id") if provider_ops_config else None
    )
    claude_code_advanced = _extract_claude_code_advanced_from_config(
        provider_config,
        provider_id=str(provider.id),
    )
    pool_advanced = _extract_pool_advanced_from_config(
        provider_config,
        provider_id=str(provider.id),
    )
    failover_rules = _extract_failover_rules_from_config(
        provider_config,
        provider_id=str(provider.id),
    )
    error_passthrough_rules = _extract_error_passthrough_rules_from_config(
        provider_config,
        provider_id=str(provider.id),
    )

    return ProviderWithEndpointsSummary(
        id=provider.id,
        name=provider.name,
        provider_type=getattr(provider, "provider_type", None),
        description=provider.description,
        website=provider.website,
        provider_priority=provider.provider_priority,
        keep_priority_on_conversion=provider.keep_priority_on_conversion,
        enable_format_conversion=provider.enable_format_conversion,
        is_active=provider.is_active,
        billing_type=provider.billing_type.value if provider.billing_type else None,
        monthly_quota_usd=provider.monthly_quota_usd,
        monthly_used_usd=provider.monthly_used_usd,
        quota_reset_day=provider.quota_reset_day,
        quota_last_reset_at=provider.quota_last_reset_at,
        quota_expires_at=provider.quota_expires_at,
        max_retries=provider.max_retries,
        proxy=provider.proxy,
        stream_first_byte_timeout=provider.stream_first_byte_timeout,
        request_timeout=provider.request_timeout,
        claude_code_advanced=claude_code_advanced,
        pool_advanced=pool_advanced,
        failover_rules=failover_rules,
        error_passthrough_rules=error_passthrough_rules,
        total_endpoints=total_endpoints,
        active_endpoints=active_endpoints,
        total_keys=total_keys,
        active_keys=active_keys,
        total_models=total_models,
        active_models=active_models,
        global_model_ids=global_model_ids,
        avg_health_score=avg_health_score,
        unhealthy_endpoints=unhealthy_endpoints,
        api_formats=api_formats,
        endpoint_health_details=endpoint_health_details,
        ops_configured=ops_configured,
        ops_architecture_id=ops_architecture_id,
        created_at=provider.created_at,
        updated_at=provider.updated_at,
    )


def _build_provider_summaries_batch(
    db: Session, providers: list[Provider]
) -> list[ProviderWithEndpointsSummary]:
    if not providers:
        return []

    provider_ids = [provider.id for provider in providers]

    endpoint_rows = (
        db.query(ProviderEndpoint)
        .options(
            load_only(
                ProviderEndpoint.id,
                ProviderEndpoint.provider_id,
                ProviderEndpoint.api_format,
                ProviderEndpoint.is_active,
            )
        )
        .filter(ProviderEndpoint.provider_id.in_(provider_ids))
        .all()
    )
    endpoints_by_provider: dict[str, list[ProviderEndpoint]] = {}
    for endpoint in endpoint_rows:
        endpoints_by_provider.setdefault(str(endpoint.provider_id), []).append(endpoint)

    key_rows = (
        db.query(ProviderAPIKey)
        .options(
            load_only(
                ProviderAPIKey.id,
                ProviderAPIKey.provider_id,
                ProviderAPIKey.is_active,
                ProviderAPIKey.api_formats,
                ProviderAPIKey.health_by_format,
            )
        )
        .filter(ProviderAPIKey.provider_id.in_(provider_ids))
        .all()
    )
    keys_by_provider: dict[str, list[ProviderAPIKey]] = {}
    for key in key_rows:
        keys_by_provider.setdefault(str(key.provider_id), []).append(key)

    key_stats_rows = (
        db.query(
            ProviderAPIKey.provider_id.label("provider_id"),
            func.count(ProviderAPIKey.id).label("total"),
            func.sum(case((ProviderAPIKey.is_active == True, 1), else_=0)).label("active"),
        )
        .filter(ProviderAPIKey.provider_id.in_(provider_ids))
        .group_by(ProviderAPIKey.provider_id)
        .all()
    )
    key_stats_by_provider: dict[str, dict[str, int]] = {
        str(row.provider_id): {
            "total": int(row.total or 0),
            "active": int(row.active or 0),
        }
        for row in key_stats_rows
    }

    model_stats_rows = (
        db.query(
            Model.provider_id.label("provider_id"),
            func.count(Model.id).label("total"),
            func.sum(case((Model.is_active == True, 1), else_=0)).label("active"),
        )
        .filter(Model.provider_id.in_(provider_ids))
        .group_by(Model.provider_id)
        .all()
    )
    model_stats_by_provider: dict[str, dict[str, int]] = {
        str(row.provider_id): {
            "total": int(row.total or 0),
            "active": int(row.active or 0),
        }
        for row in model_stats_rows
    }

    global_model_rows = (
        db.query(Model.provider_id, Model.global_model_id)
        .filter(
            Model.provider_id.in_(provider_ids),
            Model.is_active == True,
            Model.global_model_id.isnot(None),
        )
        .distinct()
        .all()
    )
    global_model_ids_by_provider: dict[str, list[Any]] = {}
    for provider_id, global_model_id in global_model_rows:
        global_model_ids_by_provider.setdefault(str(provider_id), []).append(global_model_id)

    summaries: list[ProviderWithEndpointsSummary] = []
    for provider in providers:
        pid = str(provider.id)
        key_stats = key_stats_by_provider.get(pid, {"total": 0, "active": 0})
        model_stats = model_stats_by_provider.get(pid, {"total": 0, "active": 0})
        summaries.append(
            _compose_provider_summary(
                provider=provider,
                endpoints=endpoints_by_provider.get(pid, []),
                all_keys=keys_by_provider.get(pid, []),
                total_keys=key_stats["total"],
                active_keys=key_stats["active"],
                total_models=model_stats["total"],
                active_models=model_stats["active"],
                global_model_ids=global_model_ids_by_provider.get(pid, []),
            )
        )
    return summaries


# -------- Adapters --------


@dataclass
class AdminProviderHealthMonitorAdapter(AdminApiAdapter):
    provider_id: str
    lookback_hours: int
    per_endpoint_limit: int

    @cache_result(
        key_prefix="admin:providers:health-monitor",
        ttl=CacheTTL.ADMIN_USAGE_RECORDS,
        user_specific=False,
        vary_by=["provider_id", "lookback_hours", "per_endpoint_limit"],
    )
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        provider = db.query(Provider).filter(Provider.id == self.provider_id).first()
        if not provider:
            raise NotFoundException(f"Provider {self.provider_id} 不存在")

        endpoints = (
            db.query(ProviderEndpoint)
            .options(
                load_only(
                    ProviderEndpoint.id,
                    ProviderEndpoint.provider_id,
                    ProviderEndpoint.api_format,
                    ProviderEndpoint.is_active,
                )
            )
            .filter(ProviderEndpoint.provider_id == self.provider_id)
            .all()
        )

        now = datetime.now(timezone.utc)
        since = now - timedelta(hours=self.lookback_hours)

        endpoint_ids = [str(endpoint.id) for endpoint in endpoints]
        if not endpoint_ids:
            response = ProviderEndpointHealthMonitorResponse(
                provider_id=provider.id,
                provider_name=provider.name,
                generated_at=now,
                endpoints=[],
            )
            context.add_audit_metadata(
                action="provider_health_monitor",
                provider_id=self.provider_id,
                endpoint_count=0,
                lookback_hours=self.lookback_hours,
            )
            return response.model_dump()

        ranked_attempts_subq = (
            db.query(RequestCandidate)
            .with_entities(
                RequestCandidate.endpoint_id.label("endpoint_id"),
                RequestCandidate.status.label("status"),
                RequestCandidate.status_code.label("status_code"),
                RequestCandidate.latency_ms.label("latency_ms"),
                RequestCandidate.error_type.label("error_type"),
                RequestCandidate.error_message.label("error_message"),
                func.coalesce(
                    RequestCandidate.finished_at,
                    RequestCandidate.started_at,
                    RequestCandidate.created_at,
                ).label("event_timestamp"),
                func.row_number()
                .over(
                    partition_by=RequestCandidate.endpoint_id,
                    order_by=RequestCandidate.created_at.desc(),
                )
                .label("rn"),
            )
            .filter(
                RequestCandidate.endpoint_id.in_(endpoint_ids),
                RequestCandidate.created_at >= since,
            )
            .subquery()
        )
        attempt_rows = (
            db.query(ranked_attempts_subq)
            .filter(ranked_attempts_subq.c.rn <= self.per_endpoint_limit)
            .order_by(
                ranked_attempts_subq.c.endpoint_id.asc(),
                ranked_attempts_subq.c.event_timestamp.asc(),
            )
            .all()
        )

        events_by_endpoint: dict[str, list[EndpointHealthEvent]] = {eid: [] for eid in endpoint_ids}
        for row in attempt_rows:
            endpoint_id = str(row.endpoint_id) if row.endpoint_id is not None else ""
            if not endpoint_id or endpoint_id not in events_by_endpoint:
                continue
            events_by_endpoint[endpoint_id].append(
                EndpointHealthEvent(
                    timestamp=row.event_timestamp,
                    status=row.status,
                    status_code=row.status_code,
                    latency_ms=row.latency_ms,
                    error_type=row.error_type,
                    error_message=row.error_message,
                )
            )

        endpoint_monitors: list[EndpointHealthMonitor] = []
        for endpoint in endpoints:
            endpoint_id = str(endpoint.id)
            events = events_by_endpoint.get(endpoint_id, [])

            success_count = sum(1 for event in events if event.status == "success")
            failed_count = sum(1 for event in events if event.status == "failed")
            skipped_count = sum(1 for event in events if event.status == "skipped")
            total_attempts = len(events)
            success_rate = success_count / total_attempts if total_attempts else 1.0
            last_event_at = events[-1].timestamp if events else None

            endpoint_monitors.append(
                EndpointHealthMonitor(
                    endpoint_id=endpoint.id,
                    api_format=endpoint.api_format,
                    is_active=endpoint.is_active,
                    total_attempts=total_attempts,
                    success_count=success_count,
                    failed_count=failed_count,
                    skipped_count=skipped_count,
                    success_rate=success_rate,
                    last_event_at=last_event_at,
                    events=events,
                )
            )

        response = ProviderEndpointHealthMonitorResponse(
            provider_id=provider.id,
            provider_name=provider.name,
            generated_at=now,
            endpoints=endpoint_monitors,
        )
        context.add_audit_metadata(
            action="provider_health_monitor",
            provider_id=self.provider_id,
            endpoint_count=len(endpoint_monitors),
            lookback_hours=self.lookback_hours,
            per_endpoint_limit=self.per_endpoint_limit,
        )
        return response.model_dump()


@dataclass
class AdminProviderSummaryAdapter(AdminApiAdapter):
    page: int = 1
    page_size: int = 20
    search: str = ""
    status: str = "all"
    api_format: str = "all"
    model_id: str = "all"

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db

        query = db.query(Provider)

        # 搜索筛选
        if self.search.strip():
            keywords = self.search.strip().lower().split()
            for kw in keywords:
                query = query.filter(func.lower(Provider.name).contains(kw))

        # 状态筛选
        if self.status == "active":
            query = query.filter(Provider.is_active == True)
        elif self.status == "inactive":
            query = query.filter(Provider.is_active == False)

        # API 格式筛选
        if self.api_format != "all":
            query = query.filter(
                Provider.id.in_(
                    db.query(ProviderEndpoint.provider_id)
                    .filter(ProviderEndpoint.api_format == self.api_format)
                    .distinct()
                )
            )

        # 全局模型 ID 筛选
        if self.model_id != "all":
            query = query.filter(
                Provider.id.in_(
                    db.query(Model.provider_id)
                    .filter(
                        Model.global_model_id == self.model_id,
                        Model.is_active == True,
                    )
                    .distinct()
                )
            )

        total = query.count()

        providers = (
            query.order_by(*_provider_summary_ordering(Provider))
            .offset((self.page - 1) * self.page_size)
            .limit(self.page_size)
            .all()
        )

        items = _build_provider_summaries_batch(db, providers)
        return ProviderSummaryPageResponse(
            total=total,
            page=self.page,
            page_size=self.page_size,
            items=items,
        ).model_dump()


@dataclass
class AdminProviderDetailAdapter(AdminApiAdapter):
    provider_id: str

    @cache_result(
        key_prefix="admin:providers:summary:detail",
        ttl=CacheTTL.ADMIN_USAGE_RECORDS,
        user_specific=False,
        vary_by=["provider_id"],
    )
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        provider = db.query(Provider).filter(Provider.id == self.provider_id).first()
        if not provider:
            raise NotFoundException(f"Provider {self.provider_id} not found")
        return _build_provider_summary(db, provider).model_dump()


@dataclass
class AdminUpdateProviderSettingsAdapter(AdminApiAdapter):
    provider_id: str
    update_data: ProviderUpdateRequest

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        provider = db.query(Provider).filter(Provider.id == self.provider_id).first()
        if not provider:
            raise NotFoundException("Provider not found", "provider")

        update_dict = self.update_data.model_dump(exclude_unset=True)
        if "claude_code_advanced" in update_dict:
            claude_advanced = update_dict.pop("claude_code_advanced")
            provider_type = str(getattr(provider, "provider_type", "") or "").strip().lower()
            if claude_advanced is not None and provider_type != "claude_code":
                raise InvalidRequestException(
                    "claude_code_advanced 仅适用于 provider_type=claude_code"
                )

            provider_config = dict(provider.config or {})
            if claude_advanced is None:
                provider_config.pop("claude_code_advanced", None)
            else:
                provider_config["claude_code_advanced"] = dict(claude_advanced)
            update_dict["config"] = provider_config or None

        if "pool_advanced" in update_dict:
            pool_advanced = update_dict.pop("pool_advanced")
            provider_config = dict(update_dict.get("config") or provider.config or {})
            if pool_advanced is None:
                provider_config.pop("pool_advanced", None)
            else:
                provider_config["pool_advanced"] = dict(pool_advanced)
            update_dict["config"] = provider_config or None

        if "billing_type" in update_dict and update_dict["billing_type"] is not None:
            update_dict["billing_type"] = ProviderBillingType(update_dict["billing_type"])

        for key, value in update_dict.items():
            setattr(provider, key, value)

        provider.updated_at = datetime.now(timezone.utc)
        db.commit()
        db.refresh(provider)

        admin_name = context.user.username if context.user else "admin"
        logger.info(f"Provider {provider.name} updated by {admin_name}: {update_dict}")

        # 缓存失效
        affects_model_visibility = {"is_active", "enable_format_conversion"} & update_dict.keys()
        if affects_model_visibility:
            await invalidate_models_list_cache()
            if "is_active" in update_dict:
                await ModelCacheService.invalidate_all_resolve_cache()

        if "billing_type" in update_dict:
            await ProviderCacheService.invalidate_provider_cache(provider.id)

        return _build_provider_summary(db, provider)
