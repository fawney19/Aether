"""管理员使用情况统计路由。"""

from dataclasses import dataclass
from datetime import datetime
from typing import Optional

from fastapi import APIRouter, Depends, HTTPException, Query, Request
from sqlalchemy import func
from sqlalchemy.orm import Session

from src.api.base.admin_adapter import AdminApiAdapter
from src.api.base.pipeline import ApiRequestPipeline
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
from src.services.usage.service import UsageService

router = APIRouter(prefix="/api/admin/usage", tags=["Admin - Usage"])
pipeline = ApiRequestPipeline()


# ==================== RESTful Routes ====================


@router.get("/aggregation/stats")
async def get_usage_aggregation(
    request: Request,
    group_by: str = Query(..., description="Aggregation dimension: model, user, provider, or api_format"),
    start_date: Optional[datetime] = None,
    end_date: Optional[datetime] = None,
    limit: int = Query(20, ge=1, le=100),
    db: Session = Depends(get_db),
):
    """
    获取使用情况聚合统计

    按指定维度聚合使用情况统计数据。

    **查询参数**:
    - `group_by`: 必需，聚合维度，可选值：model（按模型）、user（按用户）、provider（按提供商）、api_format（按 API 格式）
    - `start_date`: 可选，开始日期（ISO 格式）
    - `end_date`: 可选，结束日期（ISO 格式）
    - `limit`: 返回数量限制，默认 20，最大 100

    **返回字段**:
    - 按模型聚合时：model, request_count, total_tokens, total_cost, actual_cost
    - 按用户聚合时：user_id, email, username, request_count, total_tokens, total_cost
    - 按提供商聚合时：provider_id, provider, request_count, total_tokens, total_cost, actual_cost, avg_response_time_ms, success_rate, error_count
    - 按 API 格式聚合时：api_format, request_count, total_tokens, total_cost, actual_cost, avg_response_time_ms
    """
    if group_by == "model":
        adapter = AdminUsageByModelAdapter(start_date=start_date, end_date=end_date, limit=limit)
    elif group_by == "user":
        adapter = AdminUsageByUserAdapter(start_date=start_date, end_date=end_date, limit=limit)
    elif group_by == "provider":
        adapter = AdminUsageByProviderAdapter(start_date=start_date, end_date=end_date, limit=limit)
    elif group_by == "api_format":
        adapter = AdminUsageByApiFormatAdapter(start_date=start_date, end_date=end_date, limit=limit)
    else:
        raise HTTPException(
            status_code=400,
            detail=f"Invalid group_by value: {group_by}. Must be one of: model, user, provider, api_format"
        )
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/stats")
async def get_usage_stats(
    request: Request,
    start_date: Optional[datetime] = None,
    end_date: Optional[datetime] = None,
    db: Session = Depends(get_db),
):
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
    adapter = AdminUsageStatsAdapter(start_date=start_date, end_date=end_date)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/heatmap")
async def get_activity_heatmap(
    request: Request,
    db: Session = Depends(get_db),
):
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
    start_date: Optional[datetime] = None,
    end_date: Optional[datetime] = None,
    search: Optional[str] = None,  # 通用搜索：用户名、密钥名、模型名、提供商名
    user_id: Optional[str] = None,
    username: Optional[str] = None,
    model: Optional[str] = None,
    provider: Optional[str] = None,
    status: Optional[str] = None,  # stream, standard, error
    limit: int = Query(100, ge=1, le=500),
    offset: int = Query(0, ge=0),
    db: Session = Depends(get_db),
):
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
    - `status`: 可选，状态筛选（stream: 流式请求，standard: 标准请求，error: 错误请求，pending: 等待中，streaming: 流式中，completed: 已完成，failed: 失败，active: 活跃请求）
    - `limit`: 返回数量限制，默认 100，最大 500
    - `offset`: 分页偏移量，默认 0

    **返回字段**:
    - `records`: 使用记录列表，包含 id, user_id, user_email, username, api_key, provider, model, target_model,
      input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens, total_tokens,
      cost, actual_cost, rate_multiplier, response_time_ms, first_byte_time_ms, created_at, is_stream,
      input_price_per_1m, output_price_per_1m, cache_creation_price_per_1m, cache_read_price_per_1m,
      status_code, error_message, status, has_fallback, api_format, api_key_name, request_metadata
    - `total`: 符合条件的总记录数
    - `limit`: 当前分页限制
    - `offset`: 当前分页偏移量
    """
    adapter = AdminUsageRecordsAdapter(
        start_date=start_date,
        end_date=end_date,
        search=search,
        user_id=user_id,
        username=username,
        model=model,
        provider=provider,
        status=status,
        limit=limit,
        offset=offset,
    )
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/active")
async def get_active_requests(
    request: Request,
    ids: Optional[str] = Query(None, description="逗号分隔的请求 ID 列表，用于查询特定请求的状态"),
    db: Session = Depends(get_db),
):
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


# NOTE: This route must be defined AFTER all other routes to avoid matching
# routes like /stats, /records, /active, etc.
@router.get("/{usage_id}")
async def get_usage_detail(
    usage_id: str,
    request: Request,
    db: Session = Depends(get_db),
):
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
    adapter = AdminUsageDetailAdapter(usage_id=usage_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


class AdminUsageStatsAdapter(AdminApiAdapter):
    def __init__(self, start_date: Optional[datetime], end_date: Optional[datetime]):
        self.start_date = start_date
        self.end_date = end_date

    async def handle(self, context):  # type: ignore[override]
        db = context.db
        query = db.query(Usage)
        if self.start_date:
            query = query.filter(Usage.created_at >= self.start_date)
        if self.end_date:
            query = query.filter(Usage.created_at <= self.end_date)

        total_stats = query.with_entities(
            func.count(Usage.id).label("total_requests"),
            func.sum(Usage.total_tokens).label("total_tokens"),
            func.sum(Usage.total_cost_usd).label("total_cost"),
            func.sum(Usage.actual_total_cost_usd).label("total_actual_cost"),
            func.avg(Usage.response_time_ms).label("avg_response_time_ms"),
        ).first()

        # 缓存统计
        cache_stats = query.with_entities(
            func.sum(Usage.cache_creation_input_tokens).label("cache_creation_tokens"),
            func.sum(Usage.cache_read_input_tokens).label("cache_read_tokens"),
            func.sum(Usage.cache_creation_cost_usd).label("cache_creation_cost"),
            func.sum(Usage.cache_read_cost_usd).label("cache_read_cost"),
        ).first()

        # 错误统计
        error_count = query.filter(
            (Usage.status_code >= 400) | (Usage.error_message.isnot(None))
        ).count()

        context.add_audit_metadata(
            action="usage_stats",
            start_date=self.start_date.isoformat() if self.start_date else None,
            end_date=self.end_date.isoformat() if self.end_date else None,
        )

        total_requests = total_stats.total_requests if total_stats else 0
        avg_response_time_ms = float(total_stats.avg_response_time_ms or 0) if total_stats else 0
        avg_response_time = avg_response_time_ms / 1000.0

        return {
            "total_requests": total_requests,
            "total_tokens": int(total_stats.total_tokens or 0),
            "total_cost": float(total_stats.total_cost or 0),
            "total_actual_cost": float(total_stats.total_actual_cost or 0),
            "avg_response_time": round(avg_response_time, 2),
            "error_count": error_count,
            "error_rate": (
                round((error_count / total_requests) * 100, 2) if total_requests > 0 else 0
            ),
            "cache_stats": {
                "cache_creation_tokens": (
                    int(cache_stats.cache_creation_tokens or 0) if cache_stats else 0
                ),
                "cache_read_tokens": int(cache_stats.cache_read_tokens or 0) if cache_stats else 0,
                "cache_creation_cost": (
                    float(cache_stats.cache_creation_cost or 0) if cache_stats else 0
                ),
                "cache_read_cost": float(cache_stats.cache_read_cost or 0) if cache_stats else 0,
            },
        }


class AdminActivityHeatmapAdapter(AdminApiAdapter):
    """Activity heatmap adapter with Redis caching."""

    async def handle(self, context):  # type: ignore[override]
        result = await UsageService.get_cached_heatmap(
            db=context.db,
            user_id=None,
            include_actual_cost=True,
        )
        context.add_audit_metadata(action="activity_heatmap")
        return result


class AdminUsageByModelAdapter(AdminApiAdapter):
    def __init__(self, start_date: Optional[datetime], end_date: Optional[datetime], limit: int):
        self.start_date = start_date
        self.end_date = end_date
        self.limit = limit

    async def handle(self, context):  # type: ignore[override]
        db = context.db
        query = db.query(
            Usage.model,
            func.count(Usage.id).label("request_count"),
            func.sum(Usage.total_tokens).label("total_tokens"),
            func.sum(Usage.total_cost_usd).label("total_cost"),
            func.sum(Usage.actual_total_cost_usd).label("actual_cost"),
        )
        # 过滤掉 pending/streaming 状态的请求（尚未完成的请求不应计入统计）
        query = query.filter(Usage.status.notin_(["pending", "streaming"]))
        # 过滤掉 unknown/pending provider_name（请求未到达任何提供商）
        query = query.filter(Usage.provider_name.notin_(["unknown", "pending"]))

        if self.start_date:
            query = query.filter(Usage.created_at >= self.start_date)
        if self.end_date:
            query = query.filter(Usage.created_at <= self.end_date)

        query = query.group_by(Usage.model).order_by(func.count(Usage.id).desc()).limit(self.limit)
        stats = query.all()
        context.add_audit_metadata(
            action="usage_by_model",
            start_date=self.start_date.isoformat() if self.start_date else None,
            end_date=self.end_date.isoformat() if self.end_date else None,
            limit=self.limit,
            result_count=len(stats),
        )

        return [
            {
                "model": model,
                "request_count": count,
                "total_tokens": int(tokens or 0),
                "total_cost": float(cost or 0),
                "actual_cost": float(actual_cost or 0),
            }
            for model, count, tokens, cost, actual_cost in stats
        ]


class AdminUsageByUserAdapter(AdminApiAdapter):
    def __init__(self, start_date: Optional[datetime], end_date: Optional[datetime], limit: int):
        self.start_date = start_date
        self.end_date = end_date
        self.limit = limit

    async def handle(self, context):  # type: ignore[override]
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

        if self.start_date:
            query = query.filter(Usage.created_at >= self.start_date)
        if self.end_date:
            query = query.filter(Usage.created_at <= self.end_date)

        query = query.order_by(func.count(Usage.id).desc()).limit(self.limit)
        stats = query.all()

        context.add_audit_metadata(
            action="usage_by_user",
            start_date=self.start_date.isoformat() if self.start_date else None,
            end_date=self.end_date.isoformat() if self.end_date else None,
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
    def __init__(self, start_date: Optional[datetime], end_date: Optional[datetime], limit: int):
        self.start_date = start_date
        self.end_date = end_date
        self.limit = limit

    async def handle(self, context):  # type: ignore[override]
        db = context.db

        # 从 request_candidates 表统计每个 Provider 的尝试次数和成功率
        # 这样可以正确统计 Fallback 场景（一个请求可能尝试多个 Provider）
        from sqlalchemy import case, Integer

        attempt_query = db.query(
            RequestCandidate.provider_id,
            func.count(RequestCandidate.id).label("attempt_count"),
            func.sum(
                case((RequestCandidate.status == "success", 1), else_=0)
            ).label("success_count"),
            func.sum(
                case((RequestCandidate.status == "failed", 1), else_=0)
            ).label("failed_count"),
            func.avg(RequestCandidate.latency_ms).label("avg_latency_ms"),
        ).filter(
            RequestCandidate.provider_id.isnot(None),
            # 只统计实际执行的尝试（排除 available/skipped 状态）
            RequestCandidate.status.in_(["success", "failed"]),
        )

        if self.start_date:
            attempt_query = attempt_query.filter(RequestCandidate.created_at >= self.start_date)
        if self.end_date:
            attempt_query = attempt_query.filter(RequestCandidate.created_at <= self.end_date)

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
        ).filter(
            Usage.provider_id.isnot(None),
            # 过滤掉 pending/streaming 状态的请求
            Usage.status.notin_(["pending", "streaming"]),
        )

        if self.start_date:
            usage_query = usage_query.filter(Usage.created_at >= self.start_date)
        if self.end_date:
            usage_query = usage_query.filter(Usage.created_at <= self.end_date)

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

            result.append({
                "provider_id": provider_id_str,
                "provider": provider_map.get(provider_id_str, "Unknown"),
                "request_count": attempt_count,  # 尝试次数
                "total_tokens": int(usage_stat.total_tokens or 0) if usage_stat else 0,
                "total_cost": float(usage_stat.total_cost or 0) if usage_stat else 0,
                "actual_cost": float(usage_stat.actual_cost or 0) if usage_stat else 0,
                "avg_response_time_ms": float(stat.avg_latency_ms or 0),
                "success_rate": round(success_rate, 2),
                "error_count": failed_count,
            })

        return result


class AdminUsageByApiFormatAdapter(AdminApiAdapter):
    def __init__(self, start_date: Optional[datetime], end_date: Optional[datetime], limit: int):
        self.start_date = start_date
        self.end_date = end_date
        self.limit = limit

    async def handle(self, context):  # type: ignore[override]
        db = context.db
        query = db.query(
            Usage.api_format,
            func.count(Usage.id).label("request_count"),
            func.sum(Usage.total_tokens).label("total_tokens"),
            func.sum(Usage.total_cost_usd).label("total_cost"),
            func.sum(Usage.actual_total_cost_usd).label("actual_cost"),
            func.avg(Usage.response_time_ms).label("avg_response_time_ms"),
        )
        # 过滤掉 pending/streaming 状态的请求
        query = query.filter(Usage.status.notin_(["pending", "streaming"]))
        # 过滤掉 unknown/pending provider_name
        query = query.filter(Usage.provider_name.notin_(["unknown", "pending"]))
        # 只统计有 api_format 的记录
        query = query.filter(Usage.api_format.isnot(None))

        if self.start_date:
            query = query.filter(Usage.created_at >= self.start_date)
        if self.end_date:
            query = query.filter(Usage.created_at <= self.end_date)

        query = (
            query.group_by(Usage.api_format)
            .order_by(func.count(Usage.id).desc())
            .limit(self.limit)
        )
        stats = query.all()

        context.add_audit_metadata(
            action="usage_by_api_format",
            start_date=self.start_date.isoformat() if self.start_date else None,
            end_date=self.end_date.isoformat() if self.end_date else None,
            limit=self.limit,
            result_count=len(stats),
        )

        return [
            {
                "api_format": api_format or "unknown",
                "request_count": count,
                "total_tokens": int(tokens or 0),
                "total_cost": float(cost or 0),
                "actual_cost": float(actual_cost or 0),
                "avg_response_time_ms": float(avg_response_time or 0),
            }
            for api_format, count, tokens, cost, actual_cost, avg_response_time in stats
        ]


class AdminUsageRecordsAdapter(AdminApiAdapter):
    def __init__(
        self,
        start_date: Optional[datetime],
        end_date: Optional[datetime],
        search: Optional[str],
        user_id: Optional[str],
        username: Optional[str],
        model: Optional[str],
        provider: Optional[str],
        status: Optional[str],
        limit: int,
        offset: int,
    ):
        self.start_date = start_date
        self.end_date = end_date
        self.search = search
        self.user_id = user_id
        self.username = username
        self.model = model
        self.provider = provider
        self.status = status
        self.limit = limit
        self.offset = offset

    async def handle(self, context):  # type: ignore[override]
        from sqlalchemy import or_

        from src.utils.database_helpers import escape_like_pattern, safe_truncate_escaped

        db = context.db
        query = (
            db.query(Usage, User, ProviderEndpoint, ProviderAPIKey, ApiKey)
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
                query = query.filter(
                    or_(
                        User.username.ilike(search_pattern, escape="\\"),
                        ApiKey.name.ilike(search_pattern, escape="\\"),
                        Usage.model.ilike(search_pattern, escape="\\"),
                        Provider.name.ilike(search_pattern, escape="\\"),
                    )
                )

        if self.user_id:
            query = query.filter(Usage.user_id == self.user_id)
        if self.username:
            # 支持用户名模糊搜索
            escaped = escape_like_pattern(self.username)
            query = query.filter(User.username.ilike(f"%{escaped}%", escape="\\"))
        if self.model:
            # 支持模型名模糊搜索
            escaped = escape_like_pattern(self.model)
            query = query.filter(Usage.model.ilike(f"%{escaped}%", escape="\\"))
        if self.provider:
            # 支持提供商名称搜索
            escaped = escape_like_pattern(self.provider)
            query = query.filter(Provider.name.ilike(f"%{escaped}%", escape="\\"))
        if self.status:
            # 状态筛选
            # 旧的筛选值（基于 is_stream 和 status_code）：stream, standard, error
            # 新的筛选值（基于 status 字段）：pending, streaming, completed, failed, active
            if self.status == "stream":
                query = query.filter(Usage.is_stream == True)  # noqa: E712
            elif self.status == "standard":
                query = query.filter(Usage.is_stream == False)  # noqa: E712
            elif self.status == "error":
                query = query.filter(
                    (Usage.status_code >= 400) | (Usage.error_message.isnot(None))
                )
            elif self.status in ("pending", "streaming", "completed"):
                # 新的状态筛选：直接按 status 字段过滤
                query = query.filter(Usage.status == self.status)
            elif self.status == "failed":
                # 失败请求需要同时考虑新旧两种判断方式：
                # 1. 新方式：status = "failed"
                # 2. 旧方式：status_code >= 400 或 error_message 不为空
                query = query.filter(
                    (Usage.status == "failed") |
                    (Usage.status_code >= 400) |
                    (Usage.error_message.isnot(None))
                )
            elif self.status == "active":
                # 活跃请求：pending 或 streaming 状态
                query = query.filter(Usage.status.in_(["pending", "streaming"]))
        if self.start_date:
            query = query.filter(Usage.created_at >= self.start_date)
        if self.end_date:
            query = query.filter(Usage.created_at <= self.end_date)

        total = query.count()
        records = (
            query.order_by(Usage.created_at.desc()).offset(self.offset).limit(self.limit).all()
        )

        request_ids = [usage.request_id for usage, _, _, _, _ in records if usage.request_id]
        fallback_map = {}
        if request_ids:
            # 只统计实际执行的候选（success 或 failed），不包括 skipped/pending/available
            executed_counts = (
                db.query(RequestCandidate.request_id, func.count(RequestCandidate.id))
                .filter(
                    RequestCandidate.request_id.in_(request_ids),
                    RequestCandidate.status.in_(["success", "failed"]),
                )
                .group_by(RequestCandidate.request_id)
                .all()
            )
            # 如果实际执行的候选数 > 1，说明发生了 Provider 切换
            fallback_map = {req_id: count > 1 for req_id, count in executed_counts}

        context.add_audit_metadata(
            action="usage_records",
            start_date=self.start_date.isoformat() if self.start_date else None,
            end_date=self.end_date.isoformat() if self.end_date else None,
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
        provider_ids = [usage.provider_id for usage, _, _, _, _ in records if usage.provider_id]
        provider_map = {}
        if provider_ids:
            providers_data = (
                db.query(Provider.id, Provider.name).filter(Provider.id.in_(provider_ids)).all()
            )
            provider_map = {str(p.id): p.name for p in providers_data}

        data = []
        for usage, user, endpoint, provider_api_key, user_api_key in records:
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
                            "display": user_api_key.get_display_key(),
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
                    "api_format": usage.api_format
                    or (endpoint.api_format if endpoint and endpoint.api_format else None),
                    "api_key_name": provider_api_key.name if provider_api_key else None,
                    "request_metadata": usage.request_metadata,  # Provider 响应元数据
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

    def __init__(self, ids: Optional[str]):
        self.ids = ids

    async def handle(self, context):  # type: ignore[override]
        from src.services.usage import UsageService

        db = context.db
        id_list = None
        if self.ids:
            id_list = [id.strip() for id in self.ids.split(",") if id.strip()]
            if not id_list:
                return {"requests": []}

        requests = UsageService.get_active_requests_status(
            db=db, ids=id_list, include_admin_fields=True
        )
        return {"requests": requests}


@dataclass
class AdminUsageDetailAdapter(AdminApiAdapter):
    """Get detailed usage record with request/response body"""

    usage_id: str

    async def handle(self, context):  # type: ignore[override]
        db = context.db
        usage_record = db.query(Usage).filter(Usage.id == self.usage_id).first()
        if not usage_record:
            raise HTTPException(status_code=404, detail="Usage record not found")

        user = db.query(User).filter(User.id == usage_record.user_id).first()
        api_key = db.query(ApiKey).filter(ApiKey.id == usage_record.api_key_id).first()

        # 获取阶梯计费信息
        tiered_pricing_info = await self._get_tiered_pricing_info(db, usage_record)

        context.add_audit_metadata(
            action="usage_detail",
            usage_id=self.usage_id,
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
                "input": usage_record.input_cost_usd,
                "output": usage_record.output_cost_usd,
                "total": usage_record.total_cost_usd,
            },
            "cache_creation_input_tokens": usage_record.cache_creation_input_tokens,
            "cache_read_input_tokens": usage_record.cache_read_input_tokens,
            "cache_creation_cost": getattr(usage_record, "cache_creation_cost_usd", 0.0),
            "cache_read_cost": getattr(usage_record, "cache_read_cost_usd", 0.0),
            "request_cost": getattr(usage_record, "request_cost_usd", 0.0),
            "input_price_per_1m": usage_record.input_price_per_1m,
            "output_price_per_1m": usage_record.output_price_per_1m,
            "cache_creation_price_per_1m": usage_record.cache_creation_price_per_1m,
            "cache_read_price_per_1m": usage_record.cache_read_price_per_1m,
            "price_per_request": usage_record.price_per_request,
            "request_type": usage_record.request_type,
            "is_stream": usage_record.is_stream,
            "status_code": usage_record.status_code,
            "error_message": usage_record.error_message,
            "response_time_ms": usage_record.response_time_ms,
            "first_byte_time_ms": usage_record.first_byte_time_ms,  # 首字时间 (TTFB)
            "created_at": usage_record.created_at.isoformat() if usage_record.created_at else None,
            "request_headers": usage_record.request_headers,
            "request_body": usage_record.get_request_body(),
            "provider_request_headers": usage_record.provider_request_headers,
            "response_headers": usage_record.response_headers,
            "client_response_headers": usage_record.client_response_headers,
            "response_body": usage_record.get_response_body(),
            "metadata": usage_record.request_metadata,
            "tiered_pricing": tiered_pricing_info,
        }

    async def _get_tiered_pricing_info(self, db, usage_record) -> dict | None:
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


# ==================== 缓存亲和性分析 ====================


@router.get("/cache-affinity/ttl-analysis")
async def analyze_cache_affinity_ttl(
    request: Request,
    user_id: Optional[str] = Query(None, description="指定用户 ID"),
    api_key_id: Optional[str] = Query(None, description="指定 API Key ID"),
    hours: int = Query(168, ge=1, le=720, description="分析最近多少小时的数据"),
    db: Session = Depends(get_db),
):
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
    user_id: Optional[str] = Query(None, description="指定用户 ID"),
    api_key_id: Optional[str] = Query(None, description="指定 API Key ID"),
    hours: int = Query(168, ge=1, le=720, description="分析最近多少小时的数据"),
    db: Session = Depends(get_db),
):
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
        user_id: Optional[str],
        api_key_id: Optional[str],
        hours: int,
    ):
        self.user_id = user_id
        self.api_key_id = api_key_id
        self.hours = hours

    async def handle(self, context):  # type: ignore[override]
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
        user_id: Optional[str],
        api_key_id: Optional[str],
        hours: int,
    ):
        self.user_id = user_id
        self.api_key_id = api_key_id
        self.hours = hours

    async def handle(self, context):  # type: ignore[override]
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
    limit: int = Query(10000, ge=100, le=50000, description="最大返回数据点数量"),
    user_id: Optional[str] = Query(None, description="指定用户 ID"),
    include_user_info: bool = Query(False, description="是否包含用户信息（用于管理员多用户视图）"),
    db: Session = Depends(get_db),
):
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
        user_id: Optional[str] = None,
        include_user_info: bool = False,
    ):
        self.hours = hours
        self.limit = limit
        self.user_id = user_id
        self.include_user_info = include_user_info

    async def handle(self, context):  # type: ignore[override]
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
