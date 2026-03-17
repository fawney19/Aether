"""管理员使用情况统计路由。"""

from __future__ import annotations

from collections import defaultdict
from dataclasses import dataclass
from datetime import date, datetime, timedelta, timezone
from typing import Any

from fastapi import APIRouter, Depends, HTTPException, Query, Request
from sqlalchemy import case, func
from sqlalchemy.orm import Session, defer

from src.api.base.admin_adapter import AdminApiAdapter
from src.api.base.context import ApiRequestContext
from src.api.base.pipeline import get_pipeline
from src.config.constants import CacheTTL
from src.config.settings import config
from src.core.logger import logger
from src.database import get_db
from src.models.database import (
    ApiKey,
    Provider,
    ProviderAPIKey,
    ProviderEndpoint,
    RequestCandidate,
    Usage,
    User,
)
from src.services.system.stats_aggregator import AggregatedStats, StatsFilter, query_stats_hybrid
from src.services.system.time_range import TimeRangeParams
from src.services.usage.query import input_context_expr
from src.services.usage.service import UsageService
from src.utils.cache_decorator import cache_result

router = APIRouter(prefix="/api/admin/usage", tags=["Admin - Usage"])
pipeline = get_pipeline()


def _apply_admin_default_range(
    params: TimeRangeParams | None,
) -> TimeRangeParams | None:
    """Apply a default range to avoid unbounded scans."""
    if params is not None:
        return params

    days = int(getattr(config, "admin_usage_default_days", 0) or 0)
    if days <= 0:
        return None

    today = datetime.now(timezone.utc).date()
    start_date = today - timedelta(days=days - 1)
    return TimeRangeParams(
        start_date=start_date,
        end_date=today,
        timezone="UTC",
        tz_offset_minutes=0,
    ).validate_and_resolve()


def _build_time_range_params(
    start_date: date | None,
    end_date: date | None,
    preset: str | None,
    timezone_name: str | None,
    tz_offset_minutes: int | None,
) -> TimeRangeParams | None:
    if not preset and start_date is None and end_date is None:
        return None
    try:
        return TimeRangeParams(
            start_date=start_date,
            end_date=end_date,
            preset=preset,
            timezone=timezone_name,
            tz_offset_minutes=tz_offset_minutes or 0,
        ).validate_and_resolve()
    except Exception as exc:
        raise HTTPException(status_code=400, detail=str(exc)) from exc


def _calculate_token_cache_hit_rate(
    total_input_context: int | None,
    cache_read_tokens: int | None,
) -> float:
    """计算缓存命中率。

    Args:
        total_input_context: 已归一化的总输入上下文 token 数。
            Claude 格式: input_tokens + cache_read_input_tokens
            OpenAI/Gemini 格式: input_tokens（已包含 cache_read）
        cache_read_tokens: 缓存读取 token 数
    """
    context = int(total_input_context or 0)
    cached = int(cache_read_tokens or 0)
    if context <= 0:
        return 0.0
    return round(cached / context * 100, 2)


# ==================== RESTful Routes ====================


@router.get("/aggregation/stats")
async def get_usage_aggregation(
    request: Request,
    group_by: str = Query(
        ..., description="Aggregation dimension: model, user, provider, or api_format"
    ),
    start_date: date | None = None,
    end_date: date | None = None,
    preset: str | None = None,
    timezone_name: str | None = Query(None, alias="timezone"),
    tz_offset_minutes: int | None = None,
    limit: int = Query(20, ge=1, le=100),
    db: Session = Depends(get_db),
) -> Any:
    """
    获取使用情况聚合统计

    按指定维度聚合使用情况统计数据。

    **查询参数**:
    - `group_by`: 必需，聚合维度，可选值：model（按模型）、user（按用户）、provider（按提供商）、api_format（按 API 格式）
    - `start_date`: 可选，开始日期（ISO 格式）
    - `end_date`: 可选，结束日期（ISO 格式）
    - `limit`: 返回数量限制，默认 20，最大 100

    **返回字段**:
    - 按模型聚合时：model, request_count, total_tokens, total_cost, actual_cost, cache_read_tokens, cache_hit_rate
    - 按用户聚合时：user_id, email, username, request_count, total_tokens, total_cost
    - 按提供商聚合时：provider_id, provider, request_count, total_tokens, total_cost, actual_cost, avg_response_time_ms, success_rate, error_count, cache_read_tokens, cache_hit_rate
    - 按 API 格式聚合时：api_format, request_count, total_tokens, total_cost, actual_cost, avg_response_time_ms, cache_read_tokens, cache_hit_rate
    """
    time_range = _apply_admin_default_range(
        _build_time_range_params(start_date, end_date, preset, timezone_name, tz_offset_minutes)
    )

    if group_by == "model":
        adapter = AdminUsageByModelAdapter(time_range=time_range, limit=limit)
    elif group_by == "user":
        adapter = AdminUsageByUserAdapter(time_range=time_range, limit=limit)
    elif group_by == "provider":
        adapter = AdminUsageByProviderAdapter(time_range=time_range, limit=limit)
    elif group_by == "api_format":
        adapter = AdminUsageByApiFormatAdapter(time_range=time_range, limit=limit)
    else:
        raise HTTPException(
            status_code=400,
            detail=f"Invalid group_by value: {group_by}. Must be one of: model, user, provider, api_format",
        )
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/stats")
async def get_usage_stats(
    request: Request,
    start_date: date | None = None,
    end_date: date | None = None,
    preset: str | None = None,
    timezone_name: str | None = Query(None, alias="timezone"),
    tz_offset_minutes: int | None = None,
    db: Session = Depends(get_db),
) -> Any:
    """
    获取使用情况总体统计

    获取指定时间范围内的使用情况总体统计数据。

    **查询参数**:
    - `start_date`: 可选，开始日期（ISO 格式）
    - `end_date`: 可选，结束日期（ISO 格式）

    **返回字段**:
    - `total_requests`: 总请求数
    - `total_tokens`: 总 token 数
    - `total_cost`: 总成本（美元）
    - `total_actual_cost`: 实际总成本（美元）
    - `avg_response_time`: 平均响应时间（秒）
    - `error_count`: 错误请求数
    - `error_rate`: 错误率（百分比）
    - `cache_stats`: 缓存统计信息（cache_creation_tokens, cache_read_tokens, cache_creation_cost, cache_read_cost）
    """
    time_range = _apply_admin_default_range(
        _build_time_range_params(start_date, end_date, preset, timezone_name, tz_offset_minutes)
    )
    adapter = AdminUsageStatsAdapter(time_range=time_range)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/heatmap")
async def get_activity_heatmap(
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    获取活动热力图数据

    获取过去 365 天的活动热力图数据。此接口缓存 5 分钟以减少数据库负载。

    **返回字段**:
    - 按日期聚合的请求数、token 数、成本等统计数据
    """
    adapter = AdminActivityHeatmapAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/records")
async def get_usage_records(
    request: Request,
    start_date: date | None = None,
    end_date: date | None = None,
    preset: str | None = None,
    timezone_name: str | None = Query(None, alias="timezone"),
    tz_offset_minutes: int | None = None,
    search: str | None = None,  # 通用搜索：用户名、密钥名、模型名、提供商名
    user_id: str | None = None,
    username: str | None = None,
    model: str | None = None,
    provider: str | None = None,
    api_format: str | None = None,  # API 格式筛选（如 openai:chat, claude:chat）
    status: str | None = None,  # stream, standard, error
    limit: int = Query(100, ge=1, le=500),
    offset: int = Query(0, ge=0),
    db: Session = Depends(get_db),
) -> Any:
    """
    获取使用记录列表

    获取详细的使用记录列表，支持多种筛选条件。

    **查询参数**:
    - `start_date`: 可选，开始日期（ISO 格式）
    - `end_date`: 可选，结束日期（ISO 格式）
    - `search`: 可选，通用搜索关键词（支持用户名、密钥名、模型名、提供商名模糊搜索，多个关键词用空格分隔）
    - `user_id`: 可选，用户 ID 筛选
    - `username`: 可选，用户名模糊搜索
    - `model`: 可选，模型名模糊搜索
    - `provider`: 可选，提供商名称搜索
    - `api_format`: 可选，API 格式筛选（如 openai:chat, claude:chat）
    - `status`: 可选，状态筛选（stream: 流式请求，standard: 标准请求，error: 错误请求，pending: 等待中，streaming: 流式中，completed: 已完成，failed: 失败，active: 活跃请求）
    - `limit`: 返回数量限制，默认 100，最大 500
    - `offset`: 分页偏移量，默认 0

    **返回字段**:
    - `records`: 使用记录列表，包含 id, user_id, user_email, username, api_key, provider, model, target_model,
      model_version, input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens,
      total_tokens, cost, actual_cost, rate_multiplier, response_time_ms, first_byte_time_ms, created_at,
      is_stream, input_price_per_1m, output_price_per_1m, cache_creation_price_per_1m,
      cache_read_price_per_1m, status_code, error_message, status, has_fallback, has_retry,
      has_rectified, api_format, api_key_name
    - `total`: 符合条件的总记录数
    - `limit`: 当前分页限制
    - `offset`: 当前分页偏移量
    """
    time_range = _apply_admin_default_range(
        _build_time_range_params(start_date, end_date, preset, timezone_name, tz_offset_minutes)
    )
    adapter = AdminUsageRecordsAdapter(
        time_range=time_range,
        search=search,
        user_id=user_id,
        username=username,
        model=model,
        provider=provider,
        api_format=api_format,
        status=status,
        limit=limit,
        offset=offset,
    )
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/active")
async def get_active_requests(
    request: Request,
    ids: str | None = Query(None, description="逗号分隔的请求 ID 列表，用于查询特定请求的状态"),
    db: Session = Depends(get_db),
) -> Any:
    """
    获取活跃请求的状态

    获取当前活跃（pending/streaming 状态）请求的状态信息。这是一个轻量级接口，适合前端轮询。

    **查询参数**:
    - `ids`: 可选，逗号分隔的请求 ID 列表，用于查询特定请求的状态

    **行为说明**:
    - 如果提供 ids 参数，只返回这些 ID 对应请求的最新状态
    - 如果不提供 ids，返回所有 pending/streaming 状态的请求

    **返回字段**:
    - `requests`: 活跃请求列表，包含请求状态信息
    """
    adapter = AdminActiveRequestsAdapter(ids=ids)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/{usage_id}/curl")
async def get_usage_curl_data(
    usage_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    获取使用记录的 cURL 命令数据

    返回重建 cURL 命令所需的 URL、请求头（含明文 API Key）和请求体。

    **路径参数**:
    - `usage_id`: 使用记录 ID

    **返回字段**:
    - `url`: 提供商请求 URL
    - `method`: HTTP 方法
    - `headers`: 提供商请求头（含明文 API Key）
    - `body`: 请求体
    - `curl`: 生成的 cURL 命令字符串
    """
    adapter = AdminUsageCurlAdapter(usage_id=usage_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/{usage_id}/replay")
async def replay_usage_request(
    usage_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    回放使用记录请求

    将原始请求重新发送到原始或指定的提供商，并返回响应结果。

    **路径参数**:
    - `usage_id`: 使用记录 ID

    **请求体**:
    - `provider_id`: 可选，目标提供商 ID（不指定则使用原始提供商）
    - `endpoint_id`: 可选，目标端点 ID（不指定则使用原始端点）
    - `body_override`: 可选，覆盖原始请求体

    **返回字段**:
    - `url`: 请求 URL
    - `status_code`: HTTP 状态码
    - `response_headers`: 响应头
    - `response_body`: 响应体
    - `response_time_ms`: 响应时间（毫秒）
    """
    # 从 JSON body 中解析参数
    try:
        json_body = await request.json()
    except Exception:
        json_body = {}

    adapter = AdminUsageReplayAdapter(
        usage_id=usage_id,
        target_provider_id=json_body.get("provider_id"),
        target_endpoint_id=json_body.get("endpoint_id"),
        target_api_key_id=json_body.get("api_key_id"),
        body_override=json_body.get("body_override"),
    )
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


# NOTE: This route must be defined AFTER all other routes to avoid matching
# routes like /stats, /records, /active, /curl, /replay, etc.
@router.get("/{usage_id}")
async def get_usage_detail(
    usage_id: str,
    request: Request,
    include_bodies: bool = Query(True, description="是否返回请求/响应 body 内容"),
    db: Session = Depends(get_db),
) -> Any:
    """
    获取使用记录详情

    获取指定使用记录的详细信息，包括请求/响应的头部和正文。

    **路径参数**:
    - `usage_id`: 使用记录 ID

    **返回字段**:
    - `id`: 记录 ID
    - `request_id`: 请求 ID
    - `user`: 用户信息（id, username, email）
    - `api_key`: API Key 信息（id, name, display）
    - `provider`: 提供商名称
    - `api_format`: API 格式
    - `model`: 请求的模型名称
    - `target_model`: 映射后的目标模型名称
    - `tokens`: Token 统计（input, output, total）
    - `cost`: 成本统计（input, output, total）
    - `cache_creation_input_tokens`: 缓存创建输入 token 数
    - `cache_read_input_tokens`: 缓存读取输入 token 数
    - `cache_creation_cost`: 缓存创建成本
    - `cache_read_cost`: 缓存读取成本
    - `request_cost`: 请求成本
    - `input_price_per_1m`: 输入价格（每百万 token）
    - `output_price_per_1m`: 输出价格（每百万 token）
    - `cache_creation_price_per_1m`: 缓存创建价格（每百万 token）
    - `cache_read_price_per_1m`: 缓存读取价格（每百万 token）
    - `price_per_request`: 每请求价格
    - `request_type`: 请求类型
    - `is_stream`: 是否为流式请求
    - `status_code`: HTTP 状态码
    - `error_message`: 错误信息
    - `response_time_ms`: 响应时间（毫秒）
    - `first_byte_time_ms`: 首字节时间（TTFB，毫秒）
    - `created_at`: 创建时间
    - `request_headers`: 请求头
    - `request_body`: 请求体
    - `provider_request_headers`: 提供商请求头
    - `response_headers`: 提供商响应头
    - `client_response_headers`: 返回给客户端的响应头
    - `response_body`: 响应体
    - `metadata`: 提供商响应元数据
    - `tiered_pricing`: 阶梯计费信息（如适用）
    """
    adapter = AdminUsageDetailAdapter(usage_id=usage_id, include_bodies=include_bodies)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


class AdminUsageStatsAdapter(AdminApiAdapter):
    def __init__(self, time_range: TimeRangeParams | None):
        self.time_range = _apply_admin_default_range(time_range)
        self.start_date = self.time_range.start_date if self.time_range else None
        self.end_date = self.time_range.end_date if self.time_range else None
        self.preset = self.time_range.preset if self.time_range else None
        self.timezone = self.time_range.timezone if self.time_range else None
        self.tz_offset_minutes = self.time_range.tz_offset_minutes if self.time_range else None

    @cache_result(
        key_prefix="admin:usage:stats",
        ttl=CacheTTL.ADMIN_USAGE_AGGREGATION,
        user_specific=False,
        vary_by=["start_date", "end_date", "preset", "timezone", "tz_offset_minutes"],
    )
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        if self.time_range:
            stats = query_stats_hybrid(db, self.time_range, filters=StatsFilter())
        else:
            error_cond = (Usage.status_code >= 400) | (Usage.error_message.isnot(None))
            row = db.query(
                func.count(Usage.id).label("total_requests"),
                func.sum(case((error_cond, 1), else_=0)).label("error_requests"),
                func.sum(Usage.input_tokens).label("input_tokens"),
                func.sum(Usage.output_tokens).label("output_tokens"),
                func.sum(Usage.cache_creation_input_tokens).label("cache_creation_tokens"),
                func.sum(Usage.cache_read_input_tokens).label("cache_read_tokens"),
                func.sum(Usage.cache_creation_cost_usd).label("cache_creation_cost"),
                func.sum(Usage.cache_read_cost_usd).label("cache_read_cost"),
                func.sum(Usage.total_cost_usd).label("total_cost"),
                func.sum(Usage.actual_total_cost_usd).label("actual_total_cost"),
                func.sum(Usage.response_time_ms).label("total_response_time_ms"),
            ).first()
            total_requests = int(getattr(row, "total_requests", 0) or 0)
            error_requests = int(getattr(row, "error_requests", 0) or 0)
            stats = AggregatedStats(
                total_requests=total_requests,
                success_requests=total_requests - error_requests,
                error_requests=error_requests,
                input_tokens=int(getattr(row, "input_tokens", 0) or 0),
                output_tokens=int(getattr(row, "output_tokens", 0) or 0),
                cache_creation_tokens=int(getattr(row, "cache_creation_tokens", 0) or 0),
                cache_read_tokens=int(getattr(row, "cache_read_tokens", 0) or 0),
                cache_creation_cost=float(getattr(row, "cache_creation_cost", 0) or 0.0),
                cache_read_cost=float(getattr(row, "cache_read_cost", 0) or 0.0),
                total_cost=float(getattr(row, "total_cost", 0) or 0.0),
                actual_total_cost=float(getattr(row, "actual_total_cost", 0) or 0.0),
                total_response_time_ms=float(getattr(row, "total_response_time_ms", 0) or 0.0),
            )

        context.add_audit_metadata(
            action="usage_stats",
            start_date=self.start_date.isoformat() if self.start_date else None,
            end_date=self.end_date.isoformat() if self.end_date else None,
            preset=self.preset,
            timezone=self.timezone,
        )
        total_requests = stats.total_requests
        avg_response_time = stats.avg_response_time_ms / 1000.0
        error_count = stats.error_requests

        return {
            "total_requests": total_requests,
            "total_tokens": int(
                stats.input_tokens
                + stats.output_tokens
                + stats.cache_creation_tokens
                + stats.cache_read_tokens
            ),
            "total_cost": float(stats.total_cost),
            "total_actual_cost": float(stats.actual_total_cost),
            "avg_response_time": round(avg_response_time, 2),
            "error_count": error_count,
            "error_rate": (
                round((error_count / total_requests) * 100, 2) if total_requests > 0 else 0
            ),
            "cache_stats": {
                "cache_creation_tokens": int(stats.cache_creation_tokens),
                "cache_read_tokens": int(stats.cache_read_tokens),
                "cache_creation_cost": float(stats.cache_creation_cost),
                "cache_read_cost": float(stats.cache_read_cost),
            },
        }


class AdminActivityHeatmapAdapter(AdminApiAdapter):
    """Activity heatmap adapter with Redis caching."""

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        result = await UsageService.get_cached_heatmap(
            db=context.db,
            user_id=None,
            include_actual_cost=True,
        )
        context.add_audit_metadata(action="activity_heatmap")
        return result


class AdminUsageByModelAdapter(AdminApiAdapter):
    def __init__(self, time_range: TimeRangeParams | None, limit: int):
        self.time_range = _apply_admin_default_range(time_range)
        self.start_date = self.time_range.start_date if self.time_range else None
        self.end_date = self.time_range.end_date if self.time_range else None
        self.preset = self.time_range.preset if self.time_range else None
        self.timezone = self.time_range.timezone if self.time_range else None
        self.tz_offset_minutes = self.time_range.tz_offset_minutes if self.time_range else None
        self.limit = limit

    @cache_result(
        key_prefix="admin:usage:agg:model",
        ttl=CacheTTL.ADMIN_USAGE_AGGREGATION,
        user_specific=False,
        vary_by=["start_date", "end_date", "preset", "timezone", "tz_offset_minutes", "limit"],
    )
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        query = db.query(
            Usage.model,
            func.count(Usage.id).label("request_count"),
            func.sum(Usage.total_tokens).label("total_tokens"),
            func.sum(Usage.total_cost_usd).label("total_cost"),
            func.sum(Usage.actual_total_cost_usd).label("actual_cost"),
            func.sum(Usage.cache_read_input_tokens).label("cache_read_tokens"),
            func.sum(input_context_expr()).label("total_input_context"),
            func.sum(Usage.output_tokens).label("output_tokens"),
            func.sum(Usage.cache_creation_input_tokens).label("cache_creation_tokens"),
        )
        # 过滤掉 pending/streaming 状态的请求（尚未完成的请求不应计入统计）
        query = query.filter(Usage.status.notin_(["pending", "streaming"]))
        # 过滤掉 unknown/pending provider_name（请求未到达任何提供商）
        query = query.filter(Usage.provider_name.notin_(["unknown", "pending"]))

        if self.time_range:
            start_utc, end_utc = self.time_range.to_utc_datetime_range()
            query = query.filter(Usage.created_at >= start_utc, Usage.created_at < end_utc)

        query = query.group_by(Usage.model).order_by(func.count(Usage.id).desc()).limit(self.limit)
        stats = query.all()
        context.add_audit_metadata(
            action="usage_by_model",
            start_date=self.start_date.isoformat() if self.start_date else None,
            end_date=self.end_date.isoformat() if self.end_date else None,
            preset=self.preset,
            timezone=self.timezone,
            limit=self.limit,
            result_count=len(stats),
        )

        return [
            {
                "model": model,
                "request_count": count,
                "total_tokens": int(tokens or 0),
                "total_input_context": int(total_input_context or 0),
                "output_tokens": int(output_tokens or 0),
                "total_cost": float(cost or 0),
                "actual_cost": float(actual_cost or 0),
                "cache_read_tokens": int(cache_read_tokens or 0),
                "cache_creation_tokens": int(cache_creation_tokens or 0),
                "cache_hit_rate": _calculate_token_cache_hit_rate(
                    total_input_context=total_input_context,
                    cache_read_tokens=cache_read_tokens,
                ),
            }
            for (
                model,
                count,
                tokens,
                cost,
                actual_cost,
                cache_read_tokens,
                total_input_context,
                output_tokens,
                cache_creation_tokens,
            ) in stats
        ]


class AdminUsageByUserAdapter(AdminApiAdapter):
    def __init__(self, time_range: TimeRangeParams | None, limit: int):
        self.time_range = _apply_admin_default_range(time_range)
        self.start_date = self.time_range.start_date if self.time_range else None
        self.end_date = self.time_range.end_date if self.time_range else None
        self.preset = self.time_range.preset if self.time_range else None
        self.timezone = self.time_range.timezone if self.time_range else None
        self.tz_offset_minutes = self.time_range.tz_offset_minutes if self.time_range else None
        self.limit = limit

    @cache_result(
        key_prefix="admin:usage:agg:user",
        ttl=CacheTTL.ADMIN_USAGE_AGGREGATION,
        user_specific=False,
        vary_by=["start_date", "end_date", "preset", "timezone", "tz_offset_minutes", "limit"],
    )
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        query = (
            db.query(
                User.id,
                User.email,
                User.username,
                func.count(Usage.id).label("request_count"),
                func.sum(Usage.total_tokens).label("total_tokens"),
                func.sum(Usage.total_cost_usd).label("total_cost"),
            )
            .join(Usage, Usage.user_id == User.id)
            .group_by(User.id, User.email, User.username)
        )

        if self.time_range:
            start_utc, end_utc = self.time_range.to_utc_datetime_range()
            query = query.filter(Usage.created_at >= start_utc, Usage.created_at < end_utc)

        query = query.order_by(func.count(Usage.id).desc()).limit(self.limit)
        stats = query.all()

        context.add_audit_metadata(
            action="usage_by_user",
            start_date=self.start_date.isoformat() if self.start_date else None,
            end_date=self.end_date.isoformat() if self.end_date else None,
            preset=self.preset,
            timezone=self.timezone,
            limit=self.limit,
            result_count=len(stats),
        )

        return [
            {
                "user_id": user_id,
                "email": email,
                "username": username,
                "request_count": count,
                "total_tokens": int(tokens or 0),
                "total_cost": float(cost or 0),
            }
            for user_id, email, username, count, tokens, cost in stats
        ]


class AdminUsageByProviderAdapter(AdminApiAdapter):
    def __init__(self, time_range: TimeRangeParams | None, limit: int):
        self.time_range = _apply_admin_default_range(time_range)
        self.start_date = self.time_range.start_date if self.time_range else None
        self.end_date = self.time_range.end_date if self.time_range else None
        self.preset = self.time_range.preset if self.time_range else None
        self.timezone = self.time_range.timezone if self.time_range else None
        self.tz_offset_minutes = self.time_range.tz_offset_minutes if self.time_range else None
        self.limit = limit

    @cache_result(
        key_prefix="admin:usage:agg:provider",
        ttl=CacheTTL.ADMIN_USAGE_AGGREGATION,
        user_specific=False,
        vary_by=["start_date", "end_date", "preset", "timezone", "tz_offset_minutes", "limit"],
    )
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db

        # 从 request_candidates 表统计每个 Provider 的尝试次数和成功率
        # 这样可以正确统计 Fallback 场景（一个请求可能尝试多个 Provider）
        from sqlalchemy import case

        attempt_query = db.query(
            RequestCandidate.provider_id,
            func.count(RequestCandidate.id).label("attempt_count"),
            func.sum(case((RequestCandidate.status == "success", 1), else_=0)).label(
                "success_count"
            ),
            func.sum(case((RequestCandidate.status == "failed", 1), else_=0)).label("failed_count"),
            func.avg(RequestCandidate.latency_ms).label("avg_latency_ms"),
        ).filter(
            RequestCandidate.provider_id.isnot(None),
            # 只统计实际执行的尝试（排除 available/skipped 状态）
            RequestCandidate.status.in_(["success", "failed"]),
        )

        if self.time_range:
            start_utc, end_utc = self.time_range.to_utc_datetime_range()
            attempt_query = attempt_query.filter(
                RequestCandidate.created_at >= start_utc,
                RequestCandidate.created_at < end_utc,
            )

        attempt_stats = (
            attempt_query.group_by(RequestCandidate.provider_id)
            .order_by(func.count(RequestCandidate.id).desc())
            .limit(self.limit)
            .all()
        )

        # 从 Usage 表获取 token 和费用统计（基于成功的请求）
        usage_query = db.query(
            Usage.provider_id,
            func.count(Usage.id).label("request_count"),
            func.sum(Usage.total_tokens).label("total_tokens"),
            func.sum(Usage.total_cost_usd).label("total_cost"),
            func.sum(Usage.actual_total_cost_usd).label("actual_cost"),
            func.avg(Usage.response_time_ms).label("avg_response_time_ms"),
            func.sum(Usage.cache_read_input_tokens).label("cache_read_tokens"),
            func.sum(input_context_expr()).label("total_input_context"),
            func.sum(Usage.output_tokens).label("output_tokens"),
            func.sum(Usage.cache_creation_input_tokens).label("cache_creation_tokens"),
        ).filter(
            Usage.provider_id.isnot(None),
            # 过滤掉 pending/streaming 状态的请求
            Usage.status.notin_(["pending", "streaming"]),
        )

        if self.time_range:
            start_utc, end_utc = self.time_range.to_utc_datetime_range()
            usage_query = usage_query.filter(
                Usage.created_at >= start_utc, Usage.created_at < end_utc
            )

        usage_stats = usage_query.group_by(Usage.provider_id).all()
        usage_map = {str(u.provider_id): u for u in usage_stats}

        # 获取所有相关的 Provider ID
        provider_ids = set()
        for stat in attempt_stats:
            if stat.provider_id:
                provider_ids.add(stat.provider_id)
        for stat in usage_stats:
            if stat.provider_id:
                provider_ids.add(stat.provider_id)

        # 获取 Provider 名称映射
        provider_map = {}
        if provider_ids:
            providers_data = (
                db.query(Provider.id, Provider.name).filter(Provider.id.in_(provider_ids)).all()
            )
            provider_map = {str(p.id): p.name for p in providers_data}

        context.add_audit_metadata(
            action="usage_by_provider",
            start_date=self.start_date.isoformat() if self.start_date else None,
            end_date=self.end_date.isoformat() if self.end_date else None,
            preset=self.preset,
            timezone=self.timezone,
            limit=self.limit,
            result_count=len(attempt_stats),
        )

        result = []
        for stat in attempt_stats:
            provider_id_str = str(stat.provider_id) if stat.provider_id else None
            attempt_count = stat.attempt_count or 0
            success_count = int(stat.success_count or 0)
            failed_count = int(stat.failed_count or 0)
            success_rate = (success_count / attempt_count * 100) if attempt_count > 0 else 0

            # 从 usage_map 获取 token 和费用信息
            usage_stat = usage_map.get(provider_id_str)

            result.append(
                {
                    "provider_id": provider_id_str,
                    "provider": provider_map.get(provider_id_str, "Unknown"),
                    "request_count": attempt_count,  # 尝试次数
                    "total_tokens": int(usage_stat.total_tokens or 0) if usage_stat else 0,
                    "total_input_context": (
                        int(usage_stat.total_input_context or 0) if usage_stat else 0
                    ),
                    "output_tokens": int(usage_stat.output_tokens or 0) if usage_stat else 0,
                    "total_cost": float(usage_stat.total_cost or 0) if usage_stat else 0,
                    "actual_cost": float(usage_stat.actual_cost or 0) if usage_stat else 0,
                    "avg_response_time_ms": float(stat.avg_latency_ms or 0),
                    "success_rate": round(success_rate, 2),
                    "error_count": failed_count,
                    "cache_read_tokens": (
                        int(usage_stat.cache_read_tokens or 0) if usage_stat else 0
                    ),
                    "cache_creation_tokens": (
                        int(usage_stat.cache_creation_tokens or 0) if usage_stat else 0
                    ),
                    "cache_hit_rate": _calculate_token_cache_hit_rate(
                        total_input_context=(usage_stat.total_input_context if usage_stat else 0),
                        cache_read_tokens=(usage_stat.cache_read_tokens if usage_stat else 0),
                    ),
                }
            )

        return result


class AdminUsageByApiFormatAdapter(AdminApiAdapter):
    def __init__(self, time_range: TimeRangeParams | None, limit: int):
        self.time_range = _apply_admin_default_range(time_range)
        self.start_date = self.time_range.start_date if self.time_range else None
        self.end_date = self.time_range.end_date if self.time_range else None
        self.preset = self.time_range.preset if self.time_range else None
        self.timezone = self.time_range.timezone if self.time_range else None
        self.tz_offset_minutes = self.time_range.tz_offset_minutes if self.time_range else None
        self.limit = limit

    @cache_result(
        key_prefix="admin:usage:agg:api_format",
        ttl=CacheTTL.ADMIN_USAGE_AGGREGATION,
        user_specific=False,
        vary_by=["start_date", "end_date", "preset", "timezone", "tz_offset_minutes", "limit"],
    )
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        query = db.query(
            Usage.api_format,
            func.count(Usage.id).label("request_count"),
            func.sum(Usage.total_tokens).label("total_tokens"),
            func.sum(Usage.total_cost_usd).label("total_cost"),
            func.sum(Usage.actual_total_cost_usd).label("actual_cost"),
            func.avg(Usage.response_time_ms).label("avg_response_time_ms"),
            func.sum(Usage.cache_read_input_tokens).label("cache_read_tokens"),
            func.sum(input_context_expr()).label("total_input_context"),
            func.sum(Usage.output_tokens).label("output_tokens"),
            func.sum(Usage.cache_creation_input_tokens).label("cache_creation_tokens"),
        )
        # 过滤掉 pending/streaming 状态的请求
        query = query.filter(Usage.status.notin_(["pending", "streaming"]))
        # 过滤掉 unknown/pending provider_name
        query = query.filter(Usage.provider_name.notin_(["unknown", "pending"]))
        # 只统计有 api_format 的记录
        query = query.filter(Usage.api_format.isnot(None))

        if self.time_range:
            start_utc, end_utc = self.time_range.to_utc_datetime_range()
            query = query.filter(Usage.created_at >= start_utc, Usage.created_at < end_utc)

        query = (
            query.group_by(Usage.api_format).order_by(func.count(Usage.id).desc()).limit(self.limit)
        )
        stats = query.all()

        context.add_audit_metadata(
            action="usage_by_api_format",
            start_date=self.start_date.isoformat() if self.start_date else None,
            end_date=self.end_date.isoformat() if self.end_date else None,
            preset=self.preset,
            timezone=self.timezone,
            limit=self.limit,
            result_count=len(stats),
        )

        return [
            {
                "api_format": api_format or "unknown",
                "request_count": count,
                "total_tokens": int(tokens or 0),
                "total_input_context": int(total_input_context or 0),
                "output_tokens": int(output_tokens or 0),
                "total_cost": float(cost or 0),
                "actual_cost": float(actual_cost or 0),
                "avg_response_time_ms": float(avg_response_time or 0),
                "cache_read_tokens": int(cache_read_tokens or 0),
                "cache_creation_tokens": int(cache_creation_tokens or 0),
                "cache_hit_rate": _calculate_token_cache_hit_rate(
                    total_input_context=total_input_context,
                    cache_read_tokens=cache_read_tokens,
                ),
            }
            for (
                api_format,
                count,
                tokens,
                cost,
                actual_cost,
                avg_response_time,
                cache_read_tokens,
                total_input_context,
                output_tokens,
                cache_creation_tokens,
            ) in stats
        ]


class AdminUsageRecordsAdapter(AdminApiAdapter):
    def __init__(
        self,
        time_range: TimeRangeParams | None,
        search: str | None,
        user_id: str | None,
        username: str | None,
        model: str | None,
        provider: str | None,
        api_format: str | None,
        status: str | None,
        limit: int,
        offset: int,
    ):
        self.time_range = _apply_admin_default_range(time_range)
        self.start_date = self.time_range.start_date if self.time_range else None
        self.end_date = self.time_range.end_date if self.time_range else None
        self.preset = self.time_range.preset if self.time_range else None
        self.timezone = self.time_range.timezone if self.time_range else None
        self.tz_offset_minutes = self.time_range.tz_offset_minutes if self.time_range else None
        self.search = search
        self.user_id = user_id
        self.username = username
        self.model = model
        self.provider = provider
        self.api_format = api_format
        self.status = status
        self.limit = limit
        self.offset = offset

    @cache_result(
        key_prefix="admin:usage:records",
        ttl=3,  # 使用记录页强调实时性，避免 15s 缓存导致列表滞后
        user_specific=False,
        vary_by=[
            "start_date",
            "end_date",
            "preset",
            "timezone",
            "tz_offset_minutes",
            "search",
            "user_id",
            "username",
            "model",
            "provider",
            "api_format",
            "status",
            "limit",
            "offset",
        ],
    )
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        from sqlalchemy import or_
        from sqlalchemy.orm import load_only

        from src.utils.database_helpers import escape_like_pattern, safe_truncate_escaped

        db = context.db

        # -- 构建轻量 count 查询（仅按需 JOIN） --
        needs_user_join = bool(self.search or self.username)
        needs_provider_join = bool(self.search or self.provider)
        needs_apikey_join = bool(self.search)

        count_query = db.query(func.count(Usage.id))
        if needs_user_join:
            count_query = count_query.outerjoin(User, Usage.user_id == User.id)
        if needs_provider_join:
            count_query = count_query.outerjoin(Provider, Usage.provider_id == Provider.id)
        if needs_apikey_join:
            count_query = count_query.outerjoin(ApiKey, Usage.api_key_id == ApiKey.id)

        # -- 构建数据查询（完整 JOIN） --
        usage_model_version = (
            Usage.request_metadata["model_version"].as_string().label("model_version")
        )

        query = (
            db.query(Usage, User, ProviderEndpoint, ProviderAPIKey, ApiKey, usage_model_version)
            .outerjoin(User, Usage.user_id == User.id)
            .outerjoin(ProviderEndpoint, Usage.provider_endpoint_id == ProviderEndpoint.id)
            .outerjoin(ProviderAPIKey, Usage.provider_api_key_id == ProviderAPIKey.id)
            .outerjoin(ApiKey, Usage.api_key_id == ApiKey.id)
        )

        # 如果需要按 Provider 名称搜索/筛选，统一在这里 JOIN
        if self.search or self.provider:
            query = query.join(Provider, Usage.provider_id == Provider.id, isouter=True)

        # 通用搜索：用户名、密钥名、模型名、提供商名
        # 支持空格分隔的组合搜索，多个关键词之间是 AND 关系
        # 限制：最多 10 个关键词，转义后每个关键词最长 100 字符
        if self.search:
            keywords = [kw for kw in self.search.strip().split() if kw][:10]
            for keyword in keywords:
                escaped = safe_truncate_escaped(escape_like_pattern(keyword), 100)
                search_pattern = f"%{escaped}%"
                search_filter = or_(
                    User.username.ilike(search_pattern, escape="\\"),
                    ApiKey.name.ilike(search_pattern, escape="\\"),
                    Usage.model.ilike(search_pattern, escape="\\"),
                    Provider.name.ilike(search_pattern, escape="\\"),
                )
                query = query.filter(search_filter)
                count_query = count_query.filter(search_filter)

        if self.user_id:
            query = query.filter(Usage.user_id == self.user_id)
            count_query = count_query.filter(Usage.user_id == self.user_id)
        if self.username:
            # 支持用户名模糊搜索
            escaped = escape_like_pattern(self.username)
            username_filter = User.username.ilike(f"%{escaped}%", escape="\\")
            query = query.filter(username_filter)
            count_query = count_query.filter(username_filter)
        if self.model:
            # 模型筛选：前端为下拉框精确值，使用精确匹配以启用索引
            # 如需模糊搜索，请使用 search 参数。
            query = query.filter(Usage.model == self.model)
            count_query = count_query.filter(Usage.model == self.model)
        if self.provider:
            # 提供商筛选：前端为下拉框精确值，使用精确匹配以启用索引
            # 如需模糊搜索，请使用 search 参数。
            query = query.filter(Provider.name == self.provider)
            count_query = count_query.filter(Provider.name == self.provider)
        if self.api_format:
            # API 格式筛选：精确匹配（大小写不敏感）
            api_format_filter = func.lower(Usage.api_format) == self.api_format.lower()
            query = query.filter(api_format_filter)
            count_query = count_query.filter(api_format_filter)
        if self.status:
            # 状态筛选
            # 旧的筛选值（基于 is_stream 和 status_code）：stream, standard, error
            # 新的筛选值（基于 status 字段）：pending, streaming, completed, failed, active
            status_filter = None
            if self.status == "stream":
                status_filter = Usage.is_stream == True  # noqa: E712
            elif self.status == "standard":
                status_filter = Usage.is_stream == False  # noqa: E712
            elif self.status == "error":
                status_filter = (Usage.status_code >= 400) | (Usage.error_message.isnot(None))
            elif self.status in ("pending", "streaming", "completed", "cancelled"):
                # 新的状态筛选：直接按 status 字段过滤
                status_filter = Usage.status == self.status
            elif self.status == "failed":
                # 失败请求需要同时考虑新旧两种判断方式：
                # 1. 新方式：status = "failed"
                # 2. 旧方式：status_code >= 400 或 error_message 不为空
                status_filter = (
                    (Usage.status == "failed")
                    | (Usage.status_code >= 400)
                    | (Usage.error_message.isnot(None))
                )
            elif self.status == "active":
                # 活跃请求：pending 或 streaming 状态
                status_filter = Usage.status.in_(["pending", "streaming"])
            elif self.status == "has_retry":
                # 发生重试：存在 retry_index > 0 的已执行候选
                retry_subq = (
                    db.query(RequestCandidate.request_id)
                    .filter(
                        RequestCandidate.status.in_(["success", "failed"]),
                        RequestCandidate.retry_index > 0,
                    )
                    .distinct()
                    .subquery()
                )
                status_filter = Usage.request_id.in_(retry_subq)
            elif self.status == "has_fallback":
                # 发生转移：同一请求有多个不同 candidate_index 的已执行候选
                fallback_subq = (
                    db.query(RequestCandidate.request_id)
                    .filter(RequestCandidate.status.in_(["success", "failed"]))
                    .group_by(RequestCandidate.request_id)
                    .having(func.count(func.distinct(RequestCandidate.candidate_index)) > 1)
                    .subquery()
                )
                status_filter = Usage.request_id.in_(fallback_subq)

            if status_filter is not None:
                query = query.filter(status_filter)
                count_query = count_query.filter(status_filter)

        if self.time_range:
            start_utc, end_utc = self.time_range.to_utc_datetime_range()
            time_filter_start = Usage.created_at >= start_utc
            time_filter_end = Usage.created_at < end_utc
            query = query.filter(time_filter_start, time_filter_end)
            count_query = count_query.filter(time_filter_start, time_filter_end)

        # Perf: count query uses fewer JOINs than the data query
        total = int(count_query.scalar() or 0)

        # Perf: do not load large request/response columns or full request_metadata for list view
        query = query.options(
            load_only(
                Usage.id,
                Usage.request_id,
                Usage.user_id,
                Usage.api_key_id,
                Usage.provider_name,
                Usage.provider_id,
                Usage.provider_endpoint_id,
                Usage.provider_api_key_id,
                Usage.model,
                Usage.target_model,
                Usage.input_tokens,
                Usage.output_tokens,
                Usage.cache_creation_input_tokens,
                Usage.cache_read_input_tokens,
                Usage.total_tokens,
                Usage.total_cost_usd,
                Usage.actual_total_cost_usd,
                Usage.rate_multiplier,
                Usage.response_time_ms,
                Usage.first_byte_time_ms,
                Usage.created_at,
                Usage.is_stream,
                Usage.status_code,
                Usage.error_message,
                Usage.status,
                Usage.api_format,
                Usage.endpoint_api_format,
                Usage.has_format_conversion,
                Usage.input_price_per_1m,
                Usage.output_price_per_1m,
                Usage.cache_creation_price_per_1m,
                Usage.cache_read_price_per_1m,
            ),
            load_only(User.id, User.email, User.username),
            load_only(ProviderEndpoint.id, ProviderEndpoint.api_format),
            load_only(ProviderAPIKey.id, ProviderAPIKey.name),
            load_only(ApiKey.id, ApiKey.name, ApiKey.key_encrypted),
        )
        records = (
            query.order_by(Usage.created_at.desc()).offset(self.offset).limit(self.limit).all()
        )

        request_ids = [usage.request_id for usage, _, _, _, _, _ in records if usage.request_id]
        fallback_map = {}
        retry_map = {}
        rectified_map = {}
        if request_ids:
            # 查询每个请求的候选执行情况
            # 只统计实际执行的候选（success 或 failed），不包括 skipped/pending/available
            executed_candidates = (
                db.query(
                    RequestCandidate.request_id,
                    RequestCandidate.candidate_index,
                    RequestCandidate.retry_index,
                    RequestCandidate.extra_data,
                )
                .filter(
                    RequestCandidate.request_id.in_(request_ids),
                    RequestCandidate.status.in_(["success", "failed"]),
                )
                .all()
            )

            # 按 request_id 分组分析
            request_candidates: dict[str, list[tuple[int, int, dict]]] = defaultdict(list)
            for req_id, candidate_idx, retry_idx, extra_data in executed_candidates:
                request_candidates[req_id].append((candidate_idx, retry_idx, extra_data or {}))

            for req_id, candidates in request_candidates.items():
                # 提取所有不同的 candidate_index
                unique_candidates = {c[0] for c in candidates}
                # 如果有多个不同的 candidate_index，说明发生了 Fallback（Provider 切换）
                fallback_map[req_id] = len(unique_candidates) > 1

                # 检查是否有重试：同一个 candidate_index 有多个 retry_index
                has_retry = False
                for candidate_idx in unique_candidates:
                    retry_indices = [c[1] for c in candidates if c[0] == candidate_idx]
                    if len(retry_indices) > 1 or (retry_indices and max(retry_indices) > 0):
                        has_retry = True
                        break
                retry_map[req_id] = has_retry

                # 检查是否有整流：任意候选的 extra_data 中有 rectified=True
                rectified_map[req_id] = any(c[2].get("rectified", False) for c in candidates)

        context.add_audit_metadata(
            action="usage_records",
            start_date=self.start_date.isoformat() if self.start_date else None,
            end_date=self.end_date.isoformat() if self.end_date else None,
            preset=self.preset,
            timezone=self.timezone,
            search=self.search,
            user_id=self.user_id,
            username=self.username,
            model=self.model,
            provider=self.provider,
            status=self.status,
            limit=self.limit,
            offset=self.offset,
            total=total,
        )

        # 构建 provider_id -> Provider 名称的映射，避免 N+1 查询
        provider_ids = list(
            {usage.provider_id for usage, _, _, _, _, _ in records if usage.provider_id}
        )
        provider_map = {}
        if provider_ids:
            providers_data = (
                db.query(Provider.id, Provider.name).filter(Provider.id.in_(provider_ids)).all()
            )
            provider_map = {str(p.id): p.name for p in providers_data}

        data = []
        api_key_display_cache: dict[str, str] = {}
        for usage, user, endpoint, provider_api_key, user_api_key, model_version in records:
            actual_cost = (
                float(usage.actual_total_cost_usd)
                if usage.actual_total_cost_usd is not None
                else 0.0
            )
            rate_multiplier = (
                float(usage.rate_multiplier) if usage.rate_multiplier is not None else 1.0
            )

            # 提供商名称优先级：关联的 Provider 表 > usage.provider_name 字段
            provider_name = usage.provider_name
            if usage.provider_id and str(usage.provider_id) in provider_map:
                provider_name = provider_map[str(usage.provider_id)]

            # 格式转换追踪（兼容历史数据：尽量回填可展示信息）
            api_format = usage.api_format or (
                endpoint.api_format if endpoint and endpoint.api_format else None
            )
            endpoint_api_format = usage.endpoint_api_format or (
                endpoint.api_format if endpoint else None
            )

            has_format_conversion = usage.has_format_conversion
            if has_format_conversion is None:
                client_fmt = str(api_format or "").upper()
                endpoint_fmt = str(endpoint_api_format or "").upper()
                has_format_conversion = bool(
                    client_fmt and endpoint_fmt and client_fmt != endpoint_fmt
                )

            data.append(
                {
                    "id": usage.id,
                    "user_id": user.id if user else None,
                    "user_email": user.email if user else "已删除用户",
                    "username": user.username if user else "已删除用户",
                    "api_key": (
                        {
                            "id": user_api_key.id,
                            "name": user_api_key.name,
                            "display": api_key_display_cache.setdefault(
                                user_api_key.id, user_api_key.get_display_key()
                            ),
                        }
                        if user_api_key
                        else None
                    ),
                    "provider": provider_name,
                    "model": usage.model,
                    "target_model": usage.target_model,  # 映射后的目标模型名
                    "input_tokens": usage.input_tokens,
                    "output_tokens": usage.output_tokens,
                    "cache_creation_input_tokens": usage.cache_creation_input_tokens,
                    "cache_read_input_tokens": usage.cache_read_input_tokens,
                    "total_tokens": usage.total_tokens,
                    "cost": float(usage.total_cost_usd),
                    "actual_cost": actual_cost,
                    "rate_multiplier": rate_multiplier,
                    "response_time_ms": usage.response_time_ms,
                    "first_byte_time_ms": usage.first_byte_time_ms,  # 首字时间 (TTFB)
                    "created_at": usage.created_at.isoformat(),
                    "is_stream": usage.is_stream,
                    "input_price_per_1m": usage.input_price_per_1m,
                    "output_price_per_1m": usage.output_price_per_1m,
                    "cache_creation_price_per_1m": usage.cache_creation_price_per_1m,
                    "cache_read_price_per_1m": usage.cache_read_price_per_1m,
                    "status_code": usage.status_code,
                    "error_message": usage.error_message,
                    "status": usage.status,  # 请求状态: pending, streaming, completed, failed
                    "has_fallback": fallback_map.get(usage.request_id, False),
                    "has_retry": retry_map.get(usage.request_id, False),
                    "has_rectified": rectified_map.get(usage.request_id, False),
                    "api_format": api_format,
                    "endpoint_api_format": endpoint_api_format,
                    "has_format_conversion": bool(has_format_conversion),
                    "api_key_name": provider_api_key.name if provider_api_key else None,
                    "model_version": model_version,  # Provider 返回的实际模型版本（轻量字段）
                }
            )

        return {
            "records": data,
            "total": total,
            "limit": self.limit,
            "offset": self.offset,
        }


class AdminActiveRequestsAdapter(AdminApiAdapter):
    """轻量级活跃请求状态查询适配器"""

    def __init__(self, ids: str | None):
        self.ids = ids

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        from src.services.usage import UsageService

        db = context.db
        id_list = None
        if self.ids:
            id_list = [id.strip() for id in self.ids.split(",") if id.strip()]
            if not id_list:
                return {"requests": []}

        requests = UsageService.get_active_requests_status(
            db=db,
            ids=id_list,
            include_admin_fields=True,
            maintain_status=True,
        )
        return {"requests": requests}


@dataclass
class AdminUsageDetailAdapter(AdminApiAdapter):
    """Get detailed usage record with request/response body"""

    usage_id: str
    include_bodies: bool = True

    def _build_usage_detail_query(self, db: Session) -> Any:
        query = db.query(
            Usage,
            case(
                (
                    (Usage.request_body.isnot(None)) | (Usage.request_body_compressed.isnot(None)),
                    True,
                ),
                else_=False,
            ).label("has_request_body"),
            case(
                (
                    (Usage.provider_request_body.isnot(None))
                    | (Usage.provider_request_body_compressed.isnot(None)),
                    True,
                ),
                else_=False,
            ).label("has_provider_request_body"),
            case(
                (
                    (Usage.response_body.isnot(None))
                    | (Usage.response_body_compressed.isnot(None)),
                    True,
                ),
                else_=False,
            ).label("has_response_body"),
            case(
                (
                    (Usage.client_response_body.isnot(None))
                    | (Usage.client_response_body_compressed.isnot(None)),
                    True,
                ),
                else_=False,
            ).label("has_client_response_body"),
        )

        if not self.include_bodies:
            query = query.options(
                defer(Usage.request_body),
                defer(Usage.provider_request_body),
                defer(Usage.response_body),
                defer(Usage.client_response_body),
                defer(Usage.request_body_compressed),
                defer(Usage.provider_request_body_compressed),
                defer(Usage.response_body_compressed),
                defer(Usage.client_response_body_compressed),
            )

        return query

    def _load_usage_detail_row(self, db: Session) -> Any:
        usage_row = self._build_usage_detail_query(db).filter(Usage.id == self.usage_id).first()
        if usage_row:
            return usage_row
        return self._build_usage_detail_query(db).filter(Usage.request_id == self.usage_id).first()

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        # 先通过主键 id 查找，如果找不到再尝试通过 request_id 查找
        usage_row = self._load_usage_detail_row(db)
        if not usage_row:
            raise HTTPException(status_code=404, detail="Usage record not found")

        (
            usage_record,
            has_request_body,
            has_provider_request_body,
            has_response_body,
            has_client_response_body,
        ) = usage_row

        user = db.query(User).filter(User.id == usage_record.user_id).first()
        api_key = db.query(ApiKey).filter(ApiKey.id == usage_record.api_key_id).first()

        # 获取阶梯计费信息
        tiered_pricing_info = await self._get_tiered_pricing_info(db, usage_record)

        context.add_audit_metadata(
            action="usage_detail",
            usage_id=self.usage_id,
        )

        # 提取视频/图像/音频计费信息
        video_billing_info = self._extract_video_billing_info(usage_record)

        request_body = usage_record.get_request_body() if self.include_bodies else None
        provider_request_body = (
            usage_record.get_provider_request_body() if self.include_bodies else None
        )
        response_body = usage_record.get_response_body() if self.include_bodies else None
        client_response_body = (
            usage_record.get_client_response_body() if self.include_bodies else None
        )

        return {
            "id": usage_record.id,
            "request_id": usage_record.request_id,
            "user": {
                "id": user.id if user else None,
                "username": user.username if user else "Unknown",
                "email": user.email if user else None,
            },
            "api_key": {
                "id": api_key.id if api_key else None,
                "name": api_key.name if api_key else None,
                "display": api_key.get_display_key() if api_key else None,
            },
            "provider": usage_record.provider_name,
            "api_format": usage_record.api_format,
            "model": usage_record.model,
            "target_model": usage_record.target_model,
            "tokens": {
                "input": usage_record.input_tokens,
                "output": usage_record.output_tokens,
                "total": usage_record.total_tokens,
            },
            "cost": {
                "input": float(usage_record.input_cost_usd or 0),
                "output": float(usage_record.output_cost_usd or 0),
                "total": float(usage_record.total_cost_usd or 0),
            },
            "cache_creation_input_tokens": usage_record.cache_creation_input_tokens,
            "cache_read_input_tokens": usage_record.cache_read_input_tokens,
            "cache_creation_input_tokens_5m": usage_record.cache_creation_input_tokens_5m or 0,
            "cache_creation_input_tokens_1h": usage_record.cache_creation_input_tokens_1h or 0,
            "cache_creation_cost": float(getattr(usage_record, "cache_creation_cost_usd", 0) or 0),
            "cache_read_cost": float(getattr(usage_record, "cache_read_cost_usd", 0) or 0),
            "request_cost": float(getattr(usage_record, "request_cost_usd", 0) or 0),
            "input_price_per_1m": (
                float(usage_record.input_price_per_1m)
                if usage_record.input_price_per_1m is not None
                else None
            ),
            "output_price_per_1m": (
                float(usage_record.output_price_per_1m)
                if usage_record.output_price_per_1m is not None
                else None
            ),
            "cache_creation_price_per_1m": (
                float(usage_record.cache_creation_price_per_1m)
                if usage_record.cache_creation_price_per_1m is not None
                else None
            ),
            "cache_read_price_per_1m": (
                float(usage_record.cache_read_price_per_1m)
                if usage_record.cache_read_price_per_1m is not None
                else None
            ),
            "price_per_request": (
                float(usage_record.price_per_request)
                if usage_record.price_per_request is not None
                else None
            ),
            "request_type": usage_record.request_type,
            "is_stream": usage_record.is_stream,
            "status_code": usage_record.status_code,
            "error_message": usage_record.error_message,
            "status": usage_record.status,
            "response_time_ms": usage_record.response_time_ms,
            "first_byte_time_ms": usage_record.first_byte_time_ms,  # 首字时间 (TTFB)
            "created_at": usage_record.created_at.isoformat() if usage_record.created_at else None,
            "has_request_body": has_request_body,
            "has_provider_request_body": has_provider_request_body,
            "has_response_body": has_response_body,
            "has_client_response_body": has_client_response_body,
            "request_headers": usage_record.request_headers,
            "request_body": request_body,
            "provider_request_headers": usage_record.provider_request_headers,
            "provider_request_body": provider_request_body,
            "response_headers": usage_record.response_headers,
            "client_response_headers": usage_record.client_response_headers,
            "response_body": response_body,
            "client_response_body": client_response_body,
            "metadata": usage_record.request_metadata,
            "tiered_pricing": tiered_pricing_info,
            "video_billing": video_billing_info,
        }

    async def _get_tiered_pricing_info(self, db: Session, usage_record: Any) -> dict | None:
        """获取阶梯计费信息"""
        from src.services.model.cost import ModelCostService

        # 计算总输入上下文（用于阶梯判定）：输入 + 缓存创建 + 缓存读取
        input_tokens = usage_record.input_tokens or 0
        cache_creation_tokens = usage_record.cache_creation_input_tokens or 0
        cache_read_tokens = usage_record.cache_read_input_tokens or 0
        total_input_context = input_tokens + cache_creation_tokens + cache_read_tokens

        # 尝试获取模型的阶梯配置（带来源信息）
        cost_service = ModelCostService(db)
        pricing_result = await cost_service.get_tiered_pricing_with_source_async(
            usage_record.provider_name, usage_record.model
        )

        if not pricing_result:
            return None

        tiered_pricing = pricing_result.get("pricing")
        pricing_source = pricing_result.get("source")  # 'provider' 或 'global'

        if not tiered_pricing or not tiered_pricing.get("tiers"):
            return None

        tiers = tiered_pricing.get("tiers", [])
        if not tiers:
            return None

        # 找到命中的阶梯
        tier_index = None
        matched_tier = None
        for i, tier in enumerate(tiers):
            up_to = tier.get("up_to")
            if up_to is None or total_input_context <= up_to:
                tier_index = i
                matched_tier = tier
                break

        # 如果都没匹配，使用最后一个阶梯
        if tier_index is None and tiers:
            tier_index = len(tiers) - 1
            matched_tier = tiers[-1]

        return {
            "total_input_context": total_input_context,
            "tier_index": tier_index,
            "tier_count": len(tiers),
            "current_tier": matched_tier,
            "tiers": tiers,
            "source": pricing_source,  # 定价来源: 'provider' 或 'global'
        }

    def _extract_video_billing_info(self, usage_record: Any) -> dict | None:
        """
        从 request_metadata.billing_snapshot 和 dimensions 中提取视频/图像/音频计费信息。

        返回结构:
        {
            "task_type": "video" | "image" | "audio",
            "duration_seconds": 10.5,  # 视频时长（秒）
            "resolution": "1080p",     # 分辨率
            "video_price_per_second": 0.1,  # 每秒单价
            "video_cost": 1.05,        # 视频费用
            "rule_name": "...",        # 计费规则名称
            "expression": "...",       # 计费公式
            "status": "complete",      # 计费状态
        }
        """
        request_type = getattr(usage_record, "request_type", None)
        if request_type not in {"video", "image", "audio"}:
            return None

        metadata = getattr(usage_record, "request_metadata", None)
        if not metadata:
            return None

        billing_snapshot = metadata.get("billing_snapshot") if isinstance(metadata, dict) else None
        dimensions = metadata.get("dimensions") if isinstance(metadata, dict) else None

        result: dict = {
            "task_type": request_type,
        }

        # 从 billing_snapshot 中提取计费规则信息
        if billing_snapshot and isinstance(billing_snapshot, dict):
            result["rule_name"] = billing_snapshot.get("rule_name")
            result["expression"] = billing_snapshot.get("expression")
            result["status"] = billing_snapshot.get("status")
            result["cost"] = billing_snapshot.get("cost")

            # 从 dimensions_used 中提取维度
            dims_used = billing_snapshot.get("dimensions_used")
            if dims_used and isinstance(dims_used, dict):
                if "duration_seconds" in dims_used:
                    result["duration_seconds"] = dims_used["duration_seconds"]
                if "video_resolution_key" in dims_used:
                    result["resolution"] = dims_used["video_resolution_key"]
                if "video_price_per_second" in dims_used:
                    result["video_price_per_second"] = dims_used["video_price_per_second"]
                if "video_cost" in dims_used:
                    result["video_cost"] = dims_used["video_cost"]

        # 补充从 dimensions 中提取（备用）
        if dimensions and isinstance(dimensions, dict):
            if "duration_seconds" not in result and "duration_seconds" in dimensions:
                result["duration_seconds"] = dimensions["duration_seconds"]
            if "resolution" not in result and "video_resolution_key" in dimensions:
                result["resolution"] = dimensions["video_resolution_key"]

        # 如果没有有意义的视频计费信息，返回 None
        has_video_info = (
            result.get("duration_seconds")
            or result.get("resolution")
            or result.get("video_cost")
            or result.get("cost")
        )
        if not has_video_info:
            return None

        return result


# ==================== cURL 导出 & 请求回放 ====================


def _find_usage_record(db: Session, usage_id: str) -> Usage:
    """按 id 或 request_id 查找 Usage 记录，找不到则抛 404。"""
    record = db.query(Usage).filter(Usage.id == usage_id).first()
    if not record:
        record = db.query(Usage).filter(Usage.request_id == usage_id).first()
    if not record:
        raise HTTPException(status_code=404, detail="Usage record not found")
    return record


def _build_provider_url_safe(
    endpoint: ProviderEndpoint,
    model_name: str | None,
    is_stream: bool,
    provider_key: ProviderAPIKey | None,
    decrypted_auth_config: dict[str, Any] | None = None,
) -> str:
    """构建 Provider URL，build_provider_url 失败时回退到 base_url + custom_path/默认路径。"""
    from src.services.provider.transport import build_provider_url

    try:
        return build_provider_url(
            endpoint,
            path_params={"model": model_name} if model_name else None,
            is_stream=is_stream,
            key=provider_key,
            decrypted_auth_config=decrypted_auth_config,
        )
    except Exception:
        base = (endpoint.base_url or "").rstrip("/")
        if endpoint.custom_path:
            return f"{base}{endpoint.custom_path}"
        # 尝试使用 API 格式的默认路径
        try:
            from src.core.api_format.metadata import get_default_path_for_endpoint

            ep_sig = (getattr(endpoint, "api_format", "") or "").strip().lower()
            if ep_sig:
                path = get_default_path_for_endpoint(ep_sig)
                if model_name:
                    # Gemini 路径含 {action}，回退时固定用非流式操作
                    action = "streamGenerateContent" if is_stream else "generateContent"
                    try:
                        path = path.format(model=model_name, action=action)
                    except KeyError:
                        pass
                elif "{model}" in path:
                    # model 为空且路径含模板变量，无法构造有效路径，回退到纯 base URL
                    return base
                return f"{base}{path}"
        except Exception:
            pass
        return base


def _build_fresh_headers(
    auth_headers: dict[str, str],
    endpoint: ProviderEndpoint,
) -> dict[str, str]:
    """从零构建请求头：Content-Type + 认证头 + endpoint header_rules 额外头。"""
    from src.core.api_format.headers import get_extra_headers_from_endpoint

    headers: dict[str, str] = {"Content-Type": "application/json"}
    headers.update(auth_headers)
    extra = get_extra_headers_from_endpoint(endpoint)
    if extra:
        headers.update(extra)
    return headers


async def _resolve_provider_auth(
    provider_key: ProviderAPIKey,
    endpoint: ProviderEndpoint,
    db: Session,
) -> tuple[dict[str, str], dict[str, Any] | None]:
    """解析 Provider Key 的认证信息，返回 (认证头字典, 解密后的 auth_config)。

    支持: api_key / oauth / vertex_ai 三种 auth_type。
    """
    from src.core.api_format.metadata import get_auth_config_for_endpoint
    from src.core.crypto import crypto_service

    auth_type = str(getattr(provider_key, "auth_type", "api_key") or "api_key").lower()
    auth_headers: dict[str, str] = {}
    decrypted_auth_config: dict[str, Any] | None = None

    if auth_type == "oauth":
        from src.services.provider.oauth_token import resolve_oauth_access_token
        from src.services.proxy_node.resolver import resolve_effective_proxy

        # 获取 Provider 对象以读取 proxy 和 provider_type
        provider_obj = db.query(Provider).filter(Provider.id == provider_key.provider_id).first()
        provider_type = (
            str(getattr(provider_obj, "provider_type", "") or "").lower() if provider_obj else ""
        )

        # Antigravity 使用 gemini:chat 端点格式
        ep_format = str(getattr(endpoint, "api_format", "") or "")
        if provider_type == "antigravity" and not ep_format:
            ep_format = "gemini:chat"

        resolved = await resolve_oauth_access_token(
            key_id=str(provider_key.id),
            encrypted_api_key=str(provider_key.api_key or ""),
            encrypted_auth_config=(
                str(provider_key.auth_config)
                if getattr(provider_key, "auth_config", None) is not None
                else None
            ),
            provider_proxy_config=(
                resolve_effective_proxy(
                    getattr(provider_obj, "proxy", None),
                    getattr(provider_key, "proxy", None),
                )
                if provider_obj
                else None
            ),
            endpoint_api_format=ep_format,
        )
        access_token = resolved.access_token or ""
        auth_headers["Authorization"] = f"Bearer {access_token}"
        decrypted_auth_config = resolved.decrypted_auth_config

        # Codex 等需要 account_id
        if decrypted_auth_config:
            account_id = decrypted_auth_config.get("account_id")
            if account_id:
                auth_headers["chatgpt-account-id"] = str(account_id)

    elif auth_type in ("service_account", "vertex_ai"):
        from src.api.handlers.base.request_builder import get_provider_auth

        auth_info = await get_provider_auth(endpoint, provider_key)
        if auth_info:
            auth_headers[auth_info.auth_header] = auth_info.auth_value
        else:
            # 回退
            decrypted_key = crypto_service.decrypt(provider_key.api_key)
            auth_headers["Authorization"] = f"Bearer {decrypted_key}"

    else:
        # 标准 API Key
        decrypted_key = crypto_service.decrypt(provider_key.api_key)

        # 根据 endpoint signature 确定认证头名称和类型
        api_family = str(getattr(endpoint, "api_family", "") or "").lower()
        api_kind = str(getattr(endpoint, "endpoint_kind", "") or "").lower()
        if api_family and api_kind:
            endpoint_sig = f"{api_family}:{api_kind}"
        else:
            endpoint_sig = str(getattr(endpoint, "api_format", "") or "") or "openai:chat"

        auth_header, auth_type_cfg = get_auth_config_for_endpoint(endpoint_sig)
        auth_value = f"Bearer {decrypted_key}" if auth_type_cfg == "bearer" else decrypted_key
        auth_headers[auth_header] = auth_value

    return auth_headers, decrypted_auth_config


@dataclass
class AdminUsageCurlAdapter(AdminApiAdapter):
    """Generate cURL command data from a usage record."""

    usage_id: str

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        import json as _json
        import shlex

        db = context.db
        usage_record = _find_usage_record(db, self.usage_id)

        # 获取端点和密钥
        endpoint_id = usage_record.provider_endpoint_id
        key_id = usage_record.provider_api_key_id

        # 兜底：Usage 记录缺少 provider 信息时，从 RequestCandidate 表查找
        if not endpoint_id or not key_id:
            from src.models.database import RequestCandidate as RC

            candidate = (
                db.query(RC)
                .filter(
                    RC.request_id == usage_record.request_id,
                    RC.status.in_(["success", "failed", "streaming"]),
                )
                .order_by(RC.candidate_index.desc(), RC.retry_index.desc())
                .first()
            )
            if candidate:
                endpoint_id = endpoint_id or candidate.endpoint_id
                key_id = key_id or candidate.key_id

        endpoint = None
        if endpoint_id:
            endpoint = db.query(ProviderEndpoint).filter(ProviderEndpoint.id == endpoint_id).first()
        provider_key = None
        if key_id:
            provider_key = db.query(ProviderAPIKey).filter(ProviderAPIKey.id == key_id).first()

        # 解析认证信息
        stored_headers = usage_record.provider_request_headers or {}
        headers: dict[str, str] = {}
        auth_headers: dict[str, str] = {}
        decrypted_auth_config: dict[str, Any] | None = None

        if provider_key and endpoint:
            try:
                auth_headers, decrypted_auth_config = await _resolve_provider_auth(
                    provider_key, endpoint, db
                )

                if stored_headers:
                    # 有存储的请求头：替换被脱敏的认证头为真实值
                    headers = dict(stored_headers)
                    auth_lower_keys = {k.lower() for k in auth_headers}
                    for key_name in list(headers.keys()):
                        if key_name.lower() in auth_lower_keys:
                            del headers[key_name]
                    headers.update(auth_headers)
                else:
                    headers = _build_fresh_headers(auth_headers, endpoint)
            except Exception:
                headers = dict(stored_headers)
        else:
            headers = dict(stored_headers)

        # 重建请求 URL（在认证解析之后，以便传递 decrypted_auth_config 给 Vertex AI 等场景）
        url: str | None = None
        if endpoint:
            model_name = usage_record.target_model or usage_record.model
            url = _build_provider_url_safe(
                endpoint,
                model_name,
                usage_record.is_stream or False,
                provider_key,
                decrypted_auth_config,
            )

        # 确保始终有 Content-Type
        if not any(k.lower() == "content-type" for k in headers):
            headers["Content-Type"] = "application/json"

        # 获取请求体
        body = usage_record.get_request_body()

        # 生成 cURL 命令
        curl_parts = ["curl"]
        if url:
            curl_parts.append(shlex.quote(url))
        curl_parts.append("-X POST")

        for h_key, h_value in headers.items():
            curl_parts.append(f"-H {shlex.quote(f'{h_key}: {h_value}')}")

        if body:
            body_str = _json.dumps(body, ensure_ascii=False)
            curl_parts.append(f"-d {shlex.quote(body_str)}")

        curl_command = " \\\n  ".join(curl_parts)

        context.add_audit_metadata(
            action="usage_curl",
            usage_id=self.usage_id,
        )

        return {
            "url": url,
            "method": "POST",
            "headers": headers,
            "body": body,
            "curl": curl_command,
        }


def _resolve_replay_mode(same_provider: bool, same_endpoint: bool) -> str:
    if same_provider and same_endpoint:
        return "same_endpoint_reuse"
    if same_provider:
        return "same_provider_remap"
    return "cross_provider_remap"


async def _resolve_replay_model_name(
    db: Session,
    *,
    source_model: str,
    original_target_model: str | None,
    target_provider: Provider,
    target_endpoint: ProviderEndpoint,
    target_api_key: ProviderAPIKey | None,
    same_provider: bool,
    same_endpoint: bool,
    force_remap: bool,
) -> tuple[str, str]:
    """解析 replay 的最终模型名，并返回 mapping_source。"""
    from src.services.model.mapper import ModelMapperMiddleware

    target_api_format = (getattr(target_endpoint, "api_format", "") or "").strip().lower()

    if same_provider and same_endpoint and original_target_model and not force_remap:
        return original_target_model, "original_target_model"

    mapper = ModelMapperMiddleware(db)
    mapping = await mapper.get_mapping(source_model, str(target_provider.id))

    if mapping and mapping.model:
        affinity_key = target_api_key.id if target_api_key else None
        mapped_name = mapping.model.select_provider_model_name(
            affinity_key, api_format=target_api_format
        )
        return mapped_name, "model_mapping"

    if not (same_provider and same_endpoint):
        logger.debug(
            "[replay] No explicit model mapping for '{}' on provider '{}' (endpoint={}, api_format={}); "
            "forwarding original source model",
            source_model,
            target_provider.name or str(target_provider.id),
            str(getattr(target_endpoint, "id", "") or "unknown"),
            target_api_format or "unknown",
        )

    # Keep replay aligned with the normal request path: if no global-model mapping exists,
    # forward the original source model name and let the target provider validate it.
    return source_model, "none"


def _apply_replay_model_to_body(
    body: dict[str, Any],
    resolved_model: str,
    target_api_format: str | None,
) -> None:
    """根据目标格式决定是否写入 body.model。"""
    from src.core.api_format.metadata import resolve_endpoint_definition

    target_meta = (
        resolve_endpoint_definition(target_api_format) if target_api_format else None
    )
    if target_meta is not None and not target_meta.model_in_body:
        body.pop("model", None)
        return

    body["model"] = resolved_model


@dataclass
class AdminUsageReplayAdapter(AdminApiAdapter):
    """Replay a usage record request to the same or a different provider."""

    usage_id: str
    target_provider_id: str | None = None
    target_endpoint_id: str | None = None
    target_api_key_id: str | None = None
    body_override: dict | None = None

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        import time

        import httpx

        db = context.db
        usage_record = _find_usage_record(db, self.usage_id)

        # 确定原始请求的 API 格式（用于端点/Key 匹配）
        original_api_format = (
            (usage_record.endpoint_api_format or usage_record.api_format or "").strip().lower()
        )
        original_api_family = (
            original_api_format.split(":")[0] if ":" in original_api_format else ""
        )
        original_request_body = usage_record.get_request_body()
        body_override_payload = (
            self.body_override if isinstance(self.body_override, dict) else None
        )
        override_model: str | None = None
        if isinstance(body_override_payload, dict):
            override_val = body_override_payload.get("model")
            if isinstance(override_val, str) and override_val.strip():
                override_model = override_val.strip()

        # 解析源模型名：优先 request_body.model，缺失时回退 usage_record.model
        source_model = usage_record.model
        if override_model:
            source_model = override_model
        elif isinstance(original_request_body, dict):
            raw_model = original_request_body.get("model")
            if isinstance(raw_model, str) and raw_model.strip():
                source_model = raw_model.strip()

        original_target_model = usage_record.target_model

        # 确定目标端点
        target_pid = self.target_provider_id
        target_provider_obj: Provider | None = None
        if self.target_endpoint_id:
            endpoint = (
                db.query(ProviderEndpoint)
                .filter(ProviderEndpoint.id == self.target_endpoint_id)
                .first()
            )
            if not endpoint:
                raise HTTPException(status_code=404, detail="Target endpoint not found")
            target_pid = str(endpoint.provider_id)
        elif target_pid:
            target_provider_obj = db.query(Provider).filter(Provider.id == target_pid).first()
            if not target_provider_obj:
                raise HTTPException(status_code=404, detail="Target provider not found")
            # 优先匹配相同 api_format 的端点，其次匹配同 family，最后取任意 active 端点
            active_endpoints = (
                db.query(ProviderEndpoint)
                .filter(
                    ProviderEndpoint.provider_id == target_pid,
                    ProviderEndpoint.is_active == True,  # noqa: E712
                )
                .all()
            )
            endpoint = None
            if active_endpoints and original_api_format:
                # 精确匹配 api_format
                for ep in active_endpoints:
                    ep_fmt = (getattr(ep, "api_format", "") or "").strip().lower()
                    if ep_fmt == original_api_format:
                        endpoint = ep
                        break
                # 同 family 匹配
                if not endpoint and original_api_family:
                    for ep in active_endpoints:
                        ep_family = (getattr(ep, "api_family", "") or "").strip().lower()
                        if ep_family == original_api_family:
                            endpoint = ep
                            break
            if not endpoint and active_endpoints:
                endpoint = active_endpoints[0]
            if not endpoint:
                raise HTTPException(
                    status_code=404, detail="No active endpoint found for target provider"
                )
        else:
            endpoint = None
            if usage_record.provider_endpoint_id:
                endpoint = (
                    db.query(ProviderEndpoint)
                    .filter(ProviderEndpoint.id == usage_record.provider_endpoint_id)
                    .first()
                )
            if not endpoint:
                raise HTTPException(
                    status_code=404,
                    detail="Original endpoint not found, specify target_endpoint_id",
                )

        if not target_provider_obj:
            target_provider_obj = (
                db.query(Provider).filter(Provider.id == endpoint.provider_id).first()
            )
            if not target_provider_obj:
                raise HTTPException(status_code=404, detail="Target provider not found")

        if not target_pid:
            target_pid = str(target_provider_obj.id)

        # 确定 API Key
        target_ep_format = (getattr(endpoint, "api_format", "") or "").strip().lower()
        provider_key = None
        if self.target_api_key_id:
            provider_key = (
                db.query(ProviderAPIKey).filter(ProviderAPIKey.id == self.target_api_key_id).first()
            )
        elif target_pid:
            # 优先选择 api_formats 包含目标端点格式的 Key
            active_keys = (
                db.query(ProviderAPIKey)
                .filter(
                    ProviderAPIKey.provider_id == target_pid,
                    ProviderAPIKey.is_active == True,  # noqa: E712
                )
                .all()
            )
            if active_keys and target_ep_format:
                for k in active_keys:
                    k_formats = getattr(k, "api_formats", None)
                    if k_formats is None:
                        # None 表示支持所有格式，直接选中
                        provider_key = k
                        break
                    if isinstance(k_formats, list) and target_ep_format in [
                        f.strip().lower() for f in k_formats
                    ]:
                        provider_key = k
                        break
            if not provider_key and active_keys:
                provider_key = active_keys[0]
        else:
            if usage_record.provider_api_key_id:
                provider_key = (
                    db.query(ProviderAPIKey)
                    .filter(ProviderAPIKey.id == usage_record.provider_api_key_id)
                    .first()
                )

        if not provider_key:
            raise HTTPException(status_code=404, detail="No API key available for replay")

        # 判定 replay 模式
        original_provider_id = usage_record.provider_id
        if not original_provider_id and usage_record.provider_endpoint_id:
            original_endpoint = (
                db.query(ProviderEndpoint)
                .filter(ProviderEndpoint.id == usage_record.provider_endpoint_id)
                .first()
            )
            if original_endpoint:
                original_provider_id = str(original_endpoint.provider_id)

        same_provider = bool(
            original_provider_id and target_pid and str(original_provider_id) == str(target_pid)
        )
        same_endpoint = bool(
            usage_record.provider_endpoint_id
            and str(usage_record.provider_endpoint_id) == str(endpoint.id)
        )
        replay_mode = _resolve_replay_mode(same_provider, same_endpoint)

        # 解析目标模型
        resolved_model_name, mapping_source = await _resolve_replay_model_name(
            db,
            source_model=source_model,
            original_target_model=original_target_model,
            target_provider=target_provider_obj,
            target_endpoint=endpoint,
            target_api_key=provider_key,
            same_provider=same_provider,
            same_endpoint=same_endpoint,
            force_remap=bool(override_model),
        )
        mapping_applied = mapping_source != "none"

        mapping_info = {
            "source_model": source_model,
            "original_target_model": original_target_model,
            "resolved_model": resolved_model_name,
            "target_provider_id": str(target_provider_obj.id),
            "target_provider": target_provider_obj.name,
            "target_endpoint_id": str(endpoint.id),
            "target_api_format": target_ep_format,
            "replay_mode": replay_mode,
            "mapping_applied": mapping_applied,
            "mapping_source": mapping_source,
        }

        # 根据 auth_type 正确解析认证（支持 OAuth / Vertex AI / API Key）
        try:
            auth_headers, decrypted_auth_config = await _resolve_provider_auth(
                provider_key, endpoint, db
            )
        except Exception as e:
            logger.error("[replay] Failed to resolve auth for key {}: {}", provider_key.id, e)
            raise HTTPException(status_code=500, detail="Failed to resolve provider authentication")

        # 构建 URL
        url = _build_provider_url_safe(
            endpoint, resolved_model_name, False, provider_key, decrypted_auth_config
        )

        # 构建请求头（Content-Type + 认证头 + 端点额外头）
        headers = _build_fresh_headers(auth_headers, endpoint)

        # 使用覆盖体或原始请求体
        body = body_override_payload or original_request_body or {}

        # 格式转换：如果存储的请求体格式与目标端点格式不同，需要转换
        if isinstance(body, dict):
            target_format = str(getattr(endpoint, "api_format", "") or "").strip().lower()

            if original_api_format and target_format and original_api_format != target_format:
                try:
                    from src.core.api_format.conversion import format_conversion_registry

                    body = format_conversion_registry.convert_request(
                        body,
                        source_format=original_api_format,
                        target_format=target_format,
                    )
                except Exception as conv_err:
                    logger.warning(
                        "[replay] Format conversion {} -> {} failed: {}",
                        original_api_format,
                        target_format,
                        conv_err,
                    )
                    # 转换失败仍发送原始体，让用户看到上游的实际报错

            _apply_replay_model_to_body(body, resolved_model_name, target_format)

        # 强制非流式以获取完整响应
        # Gemini 格式通过 URL 控制流式（streamGenerateContent vs generateContent），
        # 不支持 body 中的 stream 字段，设置会导致 400 错误
        if isinstance(body, dict):
            target_family = str(getattr(endpoint, "api_family", "") or "").lower()
            if target_family == "gemini":
                body.pop("stream", None)
            else:
                body["stream"] = False

        # 反代提供商 envelope 包装：kiro/codex/antigravity 等需要特殊的请求体格式和额外请求头
        if not target_provider_obj:
            target_provider_obj = (
                db.query(Provider).filter(Provider.id == endpoint.provider_id).first()
            )
        if isinstance(body, dict):
            try:
                from src.services.provider.envelope import get_provider_envelope

                target_provider_type = (
                    str(getattr(target_provider_obj, "provider_type", "") or "").lower()
                    if target_provider_obj
                    else None
                )
                target_ep_sig = (getattr(endpoint, "api_format", "") or "").strip().lower()

                envelope = get_provider_envelope(
                    provider_type=target_provider_type,
                    endpoint_sig=target_ep_sig,
                )
                if envelope:
                    body, _ = envelope.wrap_request(
                        body,
                        model=resolved_model_name or "",
                        url_model=resolved_model_name,
                        decrypted_auth_config=decrypted_auth_config,
                    )
                    # envelope 可能注入额外请求头（如 Kiro 的 AWS 签名头、Codex 的 OAuth 头）
                    extra_envelope_headers = envelope.extra_headers()
                    if extra_envelope_headers:
                        headers.update(extra_envelope_headers)
            except Exception as env_err:
                logger.warning("[replay] Envelope wrap failed: {}", env_err)
                # envelope 失败仍发送原始体

        # 获取提供商名称
        provider_name = target_provider_obj.name if target_provider_obj else usage_record.provider_name

        # 发送请求
        try:
            from src.services.proxy_node.resolver import (
                build_proxy_client_kwargs,
                resolve_effective_proxy,
            )

            # 解析代理（key > provider > 系统默认）
            eff_proxy = resolve_effective_proxy(
                getattr(target_provider_obj, "proxy", None) if target_provider_obj else None,
                getattr(provider_key, "proxy", None) if provider_key else None,
            )

            start_time = time.monotonic()
            async with httpx.AsyncClient(
                **build_proxy_client_kwargs(eff_proxy, timeout=60.0)
            ) as client:
                response = await client.post(
                    url,
                    headers=headers,
                    json=body,
                )
            elapsed_ms = int((time.monotonic() - start_time) * 1000)

            # 解析响应体
            try:
                response_body = response.json()
            except Exception:
                response_body = {"raw": response.text[:10000]}

            response_headers = dict(response.headers)

        except httpx.TimeoutException:
            raise HTTPException(status_code=504, detail="Request to provider timed out")
        except Exception as e:
            logger.error("[replay] Failed to connect to provider at {}: {}", url, e)
            raise HTTPException(status_code=502, detail="Failed to connect to provider")

        context.add_audit_metadata(
            action="usage_replay",
            usage_id=self.usage_id,
            target_provider=provider_name,
            target_url=url,
            source_model=source_model,
            original_target_model=original_target_model,
            resolved_model=resolved_model_name,
            target_provider_id=str(target_provider_obj.id),
            target_endpoint_id=str(endpoint.id),
            target_api_format=target_ep_format,
            replay_mode=replay_mode,
            mapping_applied=mapping_applied,
            mapping_source=mapping_source,
        )

        return {
            "url": url,
            "provider": provider_name,
            "status_code": response.status_code,
            "response_headers": response_headers,
            "response_body": response_body,
            "response_time_ms": elapsed_ms,
            "mapping": mapping_info,
        }


# ==================== 缓存亲和性分析 ====================


@router.get("/cache-affinity/ttl-analysis")
async def analyze_cache_affinity_ttl(
    request: Request,
    user_id: str | None = Query(None, description="指定用户 ID"),
    api_key_id: str | None = Query(None, description="指定 API Key ID"),
    hours: int = Query(168, ge=1, le=720, description="分析最近多少小时的数据"),
    db: Session = Depends(get_db),
) -> Any:
    """
    分析用户请求间隔分布，推荐合适的缓存亲和性 TTL。

    通过分析同一用户连续请求之间的时间间隔，判断用户的使用模式：
    - 高频用户（间隔短）：5 分钟 TTL 足够
    - 中频用户：15-30 分钟 TTL
    - 低频用户（间隔长）：需要 60 分钟 TTL
    """
    adapter = CacheAffinityTTLAnalysisAdapter(
        user_id=user_id,
        api_key_id=api_key_id,
        hours=hours,
    )
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/cache-affinity/hit-analysis")
async def analyze_cache_hit(
    request: Request,
    user_id: str | None = Query(None, description="指定用户 ID"),
    api_key_id: str | None = Query(None, description="指定 API Key ID"),
    hours: int = Query(168, ge=1, le=720, description="分析最近多少小时的数据"),
    db: Session = Depends(get_db),
) -> Any:
    """
    分析缓存命中情况。

    返回缓存命中率、节省的费用等统计信息。
    """
    adapter = CacheHitAnalysisAdapter(
        user_id=user_id,
        api_key_id=api_key_id,
        hours=hours,
    )
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


class CacheAffinityTTLAnalysisAdapter(AdminApiAdapter):
    """缓存亲和性 TTL 分析适配器"""

    def __init__(
        self,
        user_id: str | None,
        api_key_id: str | None,
        hours: int,
    ):
        self.user_id = user_id
        self.api_key_id = api_key_id
        self.hours = hours

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db

        result = UsageService.analyze_cache_affinity_ttl(
            db=db,
            user_id=self.user_id,
            api_key_id=self.api_key_id,
            hours=self.hours,
        )

        context.add_audit_metadata(
            action="cache_affinity_ttl_analysis",
            user_id=self.user_id,
            api_key_id=self.api_key_id,
            hours=self.hours,
            total_users_analyzed=result.get("total_users_analyzed", 0),
        )

        return result


class CacheHitAnalysisAdapter(AdminApiAdapter):
    """缓存命中分析适配器"""

    def __init__(
        self,
        user_id: str | None,
        api_key_id: str | None,
        hours: int,
    ):
        self.user_id = user_id
        self.api_key_id = api_key_id
        self.hours = hours

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db

        result = UsageService.get_cache_hit_analysis(
            db=db,
            user_id=self.user_id,
            api_key_id=self.api_key_id,
            hours=self.hours,
        )

        context.add_audit_metadata(
            action="cache_hit_analysis",
            user_id=self.user_id,
            api_key_id=self.api_key_id,
            hours=self.hours,
        )

        return result


@router.get("/cache-affinity/interval-timeline")
async def get_interval_timeline(
    request: Request,
    hours: int = Query(24, ge=1, le=720, description="分析最近多少小时的数据"),
    limit: int = Query(3000, ge=100, le=50000, description="最大返回数据点数量"),
    user_id: str | None = Query(None, description="指定用户 ID"),
    include_user_info: bool = Query(False, description="是否包含用户信息（用于管理员多用户视图）"),
    db: Session = Depends(get_db),
) -> Any:
    """
    获取请求间隔时间线数据，用于散点图展示。

    返回每个请求的时间点和与上一个请求的间隔（分钟），
    可用于可视化用户请求模式。

    当 include_user_info=true 且未指定 user_id 时，返回数据会包含:
    - points 中每个点包含 user_id 字段
    - users 字段包含 user_id -> username 的映射
    """
    adapter = IntervalTimelineAdapter(
        hours=hours,
        limit=limit,
        user_id=user_id,
        include_user_info=include_user_info,
    )
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


class IntervalTimelineAdapter(AdminApiAdapter):
    """请求间隔时间线适配器"""

    def __init__(
        self,
        hours: int,
        limit: int,
        user_id: str | None = None,
        include_user_info: bool = False,
    ):
        self.hours = hours
        self.limit = limit
        self.user_id = user_id
        self.include_user_info = include_user_info

    @cache_result(
        key_prefix="admin:usage:interval_timeline",
        ttl=CacheTTL.ADMIN_USAGE_AGGREGATION,
        user_specific=False,
        vary_by=["hours", "limit", "user_id", "include_user_info"],
    )
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db

        result = UsageService.get_interval_timeline(
            db=db,
            hours=self.hours,
            limit=self.limit,
            user_id=self.user_id,
            include_user_info=self.include_user_info,
        )

        context.add_audit_metadata(
            action="interval_timeline",
            hours=self.hours,
            limit=self.limit,
            user_id=self.user_id,
            include_user_info=self.include_user_info,
            total_points=result.get("total_points", 0),
        )

        return result
