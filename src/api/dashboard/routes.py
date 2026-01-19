"""仪表盘统计 API 端点。"""

from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from typing import List

from fastapi import APIRouter, Depends, HTTPException, Query, Request
from sqlalchemy import and_, func
from sqlalchemy.orm import Session

from src.api.base.adapter import ApiAdapter, ApiMode
from src.api.base.admin_adapter import AdminApiAdapter
from src.api.base.pipeline import ApiRequestPipeline
from src.config.constants import CacheTTL
from src.core.enums import UserRole
from src.database import get_db
from src.models.database import ApiKey, Provider, RequestCandidate, StatsDaily, StatsDailyModel, StatsDailyProvider, Usage
from src.models.database import User as DBUser
from src.services.system.stats_aggregator import StatsAggregatorService
from src.utils.cache_decorator import cache_result

router = APIRouter(prefix="/api/dashboard", tags=["Dashboard"])
pipeline = ApiRequestPipeline()


def format_tokens(num: int) -> str:
    """格式化 Token 数量，自动转换 K/M 单位"""
    if num < 1000:
        return str(num)
    if num < 1000000:
        thousands = num / 1000
        if thousands >= 100:
            return f"{round(thousands)}K"
        elif thousands >= 10:
            return f"{thousands:.1f}K"
        else:
            return f"{thousands:.2f}K"
    millions = num / 1000000
    if millions >= 100:
        return f"{round(millions)}M"
    elif millions >= 10:
        return f"{millions:.1f}M"
    else:
        return f"{millions:.2f}M"


@router.get("/stats")
async def get_dashboard_stats(request: Request, db: Session = Depends(get_db)):
    """
    获取仪表盘统计数据

    根据用户角色返回不同的统计数据。管理员可以看到全局数据，普通用户只能看到自己的数据。

    **返回字段（管理员）**:
    - `stats`: 统计卡片数组，包含总请求、总费用、总Token、总缓存等信息
    - `today`: 今日统计（requests, cost, actual_cost, tokens, cache_creation_tokens, cache_read_tokens）
    - `api_keys`: API Key 统计（total, active）
    - `tokens`: 本月 Token 统计
    - `token_breakdown`: Token 详细分类（input, output, cache_creation, cache_read）
    - `system_health`: 系统健康指标（avg_response_time, error_rate, error_requests, fallback_count, total_requests）
    - `cost_stats`: 成本统计（total_cost, total_actual_cost, cost_savings）
    - `cache_stats`: 缓存统计信息
    - `users`: 用户统计（total, active）

    **返回字段（普通用户）**:
    - `stats`: 统计卡片数组，包含 API 密钥、本月请求、配额使用、总Token 等信息
    - `today`: 今日统计
    - `token_breakdown`: Token 详细分类
    - `cache_stats`: 缓存统计信息
    - `monthly_cost`: 本月费用
    """
    adapter = DashboardStatsAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/recent-requests")
async def get_recent_requests(
    request: Request,
    limit: int = Query(10, ge=1, le=100),
    db: Session = Depends(get_db),
):
    """
    获取最近请求列表

    获取最近的 API 请求记录。管理员可以看到所有用户的请求，普通用户只能看到自己的请求。

    **查询参数**:
    - `limit`: 返回记录数，默认 10，最大 100

    **返回字段**:
    - `requests`: 请求列表，每条记录包含：
      - `id`: 请求 ID
      - `user`: 用户名
      - `model`: 使用的模型
      - `tokens`: Token 数量
      - `time`: 请求时间（HH:MM 格式）
      - `is_stream`: 是否为流式请求
    """
    adapter = DashboardRecentRequestsAdapter(limit=limit)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


# NOTE: /request-detail/{request_id} has been moved to /api/admin/usage/{id}
# The old route is removed. Use dashboardApi.getRequestDetail() which now calls the new API.


@router.get("/provider-status")
async def get_provider_status(request: Request, db: Session = Depends(get_db)):
    """
    获取提供商状态

    获取所有活跃提供商的状态和最近 24 小时的请求统计。

    **返回字段**:
    - `providers`: 提供商列表，每个提供商包含：
      - `name`: 提供商名称
      - `status`: 状态（active/inactive）
      - `requests`: 最近 24 小时的请求数
    """
    adapter = DashboardProviderStatusAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/daily-stats")
async def get_daily_stats(
    request: Request,
    days: int = Query(7, ge=1, le=30),
    db: Session = Depends(get_db),
):
    """
    获取每日统计数据

    获取指定天数的每日使用统计数据，用于生成图表。

    **查询参数**:
    - `days`: 统计天数，默认 7 天，最大 30 天

    **返回字段**:
    - `daily_stats`: 每日统计数组，每天包含：
      - `date`: 日期（ISO 格式）
      - `requests`: 请求数
      - `tokens`: Token 数量
      - `cost`: 费用（USD）
      - `avg_response_time`: 平均响应时间（秒）
      - `unique_models`: 使用的模型数量（仅管理员）
      - `unique_providers`: 使用的提供商数量（仅管理员）
      - `fallback_count`: 故障转移次数（仅管理员）
      - `model_breakdown`: 按模型分解的统计（仅管理员）
    - `model_summary`: 模型使用汇总，按费用排序
    - `period`: 统计周期信息（start_date, end_date, days）
    """
    adapter = DashboardDailyStatsAdapter(days=days)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


class DashboardAdapter(ApiAdapter):
    """需要登录的仪表盘适配器基类。"""

    mode = ApiMode.USER  # 普通用户也可访问仪表盘

    def authorize(self, context):  # type: ignore[override]
        if not context.user:
            raise HTTPException(status_code=401, detail="未登录")


class DashboardStatsAdapter(DashboardAdapter):
    async def handle(self, context):  # type: ignore[override]
        user = context.user
        if not user:
            raise HTTPException(status_code=401, detail="未登录")

        adapter = (
            AdminDashboardStatsAdapter()
            if user.role == UserRole.ADMIN
            else UserDashboardStatsAdapter()
        )
        return await adapter.handle(context)


class AdminDashboardStatsAdapter(AdminApiAdapter):
    @cache_result(key_prefix="dashboard:admin:stats", ttl=CacheTTL.DASHBOARD_STATS, user_specific=False)
    async def handle(self, context):  # type: ignore[override]
        """管理员仪表盘统计 - 使用预聚合数据优化性能"""
        from zoneinfo import ZoneInfo
        from src.services.system.stats_aggregator import APP_TIMEZONE

        db = context.db
        # 使用业务时区计算日期，与 stats_daily 表保持一致
        app_tz = ZoneInfo(APP_TIMEZONE)
        now_local = datetime.now(app_tz)
        today_local = now_local.replace(hour=0, minute=0, second=0, microsecond=0)
        # 转换为 UTC 用于与 stats_daily.date 比较（存储的是业务日期对应的 UTC 开始时间）
        today = today_local.astimezone(timezone.utc)
        yesterday = (today_local - timedelta(days=1)).astimezone(timezone.utc)
        # 本月第一天（自然月）
        month_start_local = today_local.replace(day=1)
        month_start = month_start_local.astimezone(timezone.utc)

        # ==================== 使用预聚合数据 ====================
        # 从 stats_summary + 今日实时数据获取全局统计
        combined_stats = StatsAggregatorService.get_combined_stats(db)

        all_time_requests = combined_stats["total_requests"]
        all_time_success_requests = combined_stats["success_requests"]
        all_time_error_requests = combined_stats["error_requests"]
        all_time_input_tokens = combined_stats["input_tokens"]
        all_time_output_tokens = combined_stats["output_tokens"]
        all_time_cache_creation = combined_stats["cache_creation_tokens"]
        all_time_cache_read = combined_stats["cache_read_tokens"]
        all_time_cost = combined_stats["total_cost"]
        all_time_actual_cost = combined_stats["actual_total_cost"]

        # 用户/API Key 统计
        total_users = combined_stats.get("total_users") or db.query(func.count(DBUser.id)).scalar()
        active_users = combined_stats.get("active_users") or (
            db.query(func.count(DBUser.id)).filter(DBUser.is_active.is_(True)).scalar()
        )
        total_api_keys = combined_stats.get("total_api_keys") or db.query(func.count(ApiKey.id)).scalar()
        active_api_keys = combined_stats.get("active_api_keys") or (
            db.query(func.count(ApiKey.id)).filter(ApiKey.is_active.is_(True)).scalar()
        )

        # ==================== 今日实时统计 ====================
        today_stats = StatsAggregatorService.get_today_realtime_stats(db)
        requests_today = today_stats["total_requests"]
        cost_today = today_stats["total_cost"]
        actual_cost_today = today_stats["actual_total_cost"]
        input_tokens_today = today_stats["input_tokens"]
        output_tokens_today = today_stats["output_tokens"]
        cache_creation_today = today_stats["cache_creation_tokens"]
        cache_read_today = today_stats["cache_read_tokens"]
        tokens_today = (
            input_tokens_today + output_tokens_today + cache_creation_today + cache_read_today
        )

        # ==================== 昨日统计（从预聚合表获取）====================
        yesterday_stats = db.query(StatsDaily).filter(StatsDaily.date == yesterday).first()
        if yesterday_stats:
            requests_yesterday = yesterday_stats.total_requests
            cost_yesterday = yesterday_stats.total_cost
            input_tokens_yesterday = yesterday_stats.input_tokens
            output_tokens_yesterday = yesterday_stats.output_tokens
            cache_creation_yesterday = yesterday_stats.cache_creation_tokens
            cache_read_yesterday = yesterday_stats.cache_read_tokens
        else:
            # 如果没有预聚合数据，回退到实时查询
            requests_yesterday = (
                db.query(func.count(Usage.id))
                .filter(Usage.created_at >= yesterday, Usage.created_at < today)
                .scalar() or 0
            )
            cost_yesterday = (
                db.query(func.sum(Usage.total_cost_usd))
                .filter(Usage.created_at >= yesterday, Usage.created_at < today)
                .scalar() or 0
            )
            yesterday_token_stats = (
                db.query(
                    func.sum(Usage.input_tokens).label("input_tokens"),
                    func.sum(Usage.output_tokens).label("output_tokens"),
                    func.sum(Usage.cache_creation_input_tokens).label("cache_creation_tokens"),
                    func.sum(Usage.cache_read_input_tokens).label("cache_read_tokens"),
                )
                .filter(Usage.created_at >= yesterday, Usage.created_at < today)
                .first()
            )
            input_tokens_yesterday = int(yesterday_token_stats.input_tokens or 0) if yesterday_token_stats else 0
            output_tokens_yesterday = int(yesterday_token_stats.output_tokens or 0) if yesterday_token_stats else 0
            cache_creation_yesterday = int(yesterday_token_stats.cache_creation_tokens or 0) if yesterday_token_stats else 0
            cache_read_yesterday = int(yesterday_token_stats.cache_read_tokens or 0) if yesterday_token_stats else 0

        # ==================== 本月统计（从预聚合表聚合）====================
        monthly_stats = (
            db.query(
                func.sum(StatsDaily.total_requests).label("total_requests"),
                func.sum(StatsDaily.error_requests).label("error_requests"),
                func.sum(StatsDaily.total_cost).label("total_cost"),
                func.sum(StatsDaily.actual_total_cost).label("actual_total_cost"),
                func.sum(StatsDaily.input_tokens + StatsDaily.output_tokens +
                         StatsDaily.cache_creation_tokens + StatsDaily.cache_read_tokens).label("total_tokens"),
                func.sum(StatsDaily.cache_creation_tokens).label("cache_creation_tokens"),
                func.sum(StatsDaily.cache_read_tokens).label("cache_read_tokens"),
                func.sum(StatsDaily.cache_creation_cost).label("cache_creation_cost"),
                func.sum(StatsDaily.cache_read_cost).label("cache_read_cost"),
                func.sum(StatsDaily.fallback_count).label("fallback_count"),
            )
            .filter(StatsDaily.date >= month_start, StatsDaily.date < today)
            .first()
        )

        # 本月数据 = 预聚合月数据 + 今日实时数据
        if monthly_stats and monthly_stats.total_requests:
            total_requests = int(monthly_stats.total_requests or 0) + requests_today
            error_requests = int(monthly_stats.error_requests or 0) + today_stats["error_requests"]
            total_cost = float(monthly_stats.total_cost or 0) + cost_today
            total_actual_cost = float(monthly_stats.actual_total_cost or 0) + actual_cost_today
            total_tokens = int(monthly_stats.total_tokens or 0) + tokens_today
            cache_creation_tokens = int(monthly_stats.cache_creation_tokens or 0) + cache_creation_today
            cache_read_tokens = int(monthly_stats.cache_read_tokens or 0) + cache_read_today
            cache_creation_cost = float(monthly_stats.cache_creation_cost or 0)
            cache_read_cost = float(monthly_stats.cache_read_cost or 0)
            fallback_count = int(monthly_stats.fallback_count or 0)
        else:
            # 回退到实时查询（没有预聚合数据时）
            total_requests = (
                db.query(func.count(Usage.id)).filter(Usage.created_at >= month_start).scalar() or 0
            )
            total_cost = (
                db.query(func.sum(Usage.total_cost_usd)).filter(Usage.created_at >= month_start).scalar() or 0
            )
            total_actual_cost = (
                db.query(func.sum(Usage.actual_total_cost_usd))
                .filter(Usage.created_at >= month_start).scalar() or 0
            )
            error_requests = (
                db.query(func.count(Usage.id))
                .filter(
                    Usage.created_at >= month_start,
                    (Usage.status_code >= 400) | (Usage.error_message.isnot(None)),
                ).scalar() or 0
            )
            total_tokens = (
                db.query(func.sum(Usage.total_tokens)).filter(Usage.created_at >= month_start).scalar() or 0
            )
            cache_stats = (
                db.query(
                    func.sum(Usage.cache_creation_input_tokens).label("cache_creation_tokens"),
                    func.sum(Usage.cache_read_input_tokens).label("cache_read_tokens"),
                    func.sum(Usage.cache_creation_cost_usd).label("cache_creation_cost"),
                    func.sum(Usage.cache_read_cost_usd).label("cache_read_cost"),
                )
                .filter(Usage.created_at >= month_start)
                .first()
            )
            cache_creation_tokens = int(cache_stats.cache_creation_tokens or 0) if cache_stats else 0
            cache_read_tokens = int(cache_stats.cache_read_tokens or 0) if cache_stats else 0
            cache_creation_cost = float(cache_stats.cache_creation_cost or 0) if cache_stats else 0
            cache_read_cost = float(cache_stats.cache_read_cost or 0) if cache_stats else 0

            # Fallback 统计
            fallback_subquery = (
                db.query(
                    RequestCandidate.request_id, func.count(RequestCandidate.id).label("executed_count")
                )
                .filter(
                    RequestCandidate.created_at >= month_start,
                    RequestCandidate.status.in_(["success", "failed"]),
                )
                .group_by(RequestCandidate.request_id)
                .subquery()
            )
            fallback_count = (
                db.query(func.count())
                .select_from(fallback_subquery)
                .filter(fallback_subquery.c.executed_count > 1)
                .scalar() or 0
            )

        # ==================== 系统健康指标 ====================
        error_rate = round((error_requests / total_requests) * 100, 2) if total_requests > 0 else 0

        # 平均响应时间（仅查询今日数据，降低查询成本）
        avg_response_time = (
            db.query(func.avg(Usage.response_time_ms))
            .filter(
                Usage.created_at >= today,
                Usage.status_code == 200,
                Usage.response_time_ms.isnot(None),
            )
            .scalar() or 0
        )
        avg_response_time_seconds = float(avg_response_time) / 1000.0

        # 缓存命中率
        total_input_with_cache = all_time_input_tokens + all_time_cache_read
        cache_hit_rate = (
            round((all_time_cache_read / total_input_with_cache) * 100, 1)
            if total_input_with_cache > 0
            else 0
        )

        return {
            "stats": [
                {
                    "name": "总请求",
                    "value": f"{all_time_requests:,}",
                    "subValue": f"有效 {all_time_success_requests:,} / 异常 {all_time_error_requests:,}",
                    "change": (
                        f"+{requests_today}"
                        if requests_today > requests_yesterday
                        else str(requests_today)
                    ),
                    "changeType": (
                        "increase"
                        if requests_today > requests_yesterday
                        else ("decrease" if requests_today < requests_yesterday else "neutral")
                    ),
                    "icon": "Activity",
                },
                {
                    "name": "总费用",
                    "value": f"${all_time_cost:.2f}",
                    "subValue": f"倍率后 ${all_time_actual_cost:.2f}",
                    "change": (
                        f"+${cost_today:.2f}"
                        if cost_today > cost_yesterday
                        else f"${cost_today:.2f}"
                    ),
                    "changeType": (
                        "increase"
                        if cost_today > cost_yesterday
                        else ("decrease" if cost_today < cost_yesterday else "neutral")
                    ),
                    "icon": "DollarSign",
                },
                {
                    "name": "总Token",
                    "value": format_tokens(
                        all_time_input_tokens
                        + all_time_output_tokens
                        + all_time_cache_creation
                        + all_time_cache_read
                    ),
                    "subValue": f"输入 {format_tokens(all_time_input_tokens)} / 输出 {format_tokens(all_time_output_tokens)}",
                    "change": (
                        f"+{format_tokens(input_tokens_today + output_tokens_today + cache_creation_today + cache_read_today)}"
                        if (input_tokens_today + output_tokens_today + cache_creation_today + cache_read_today)
                        > (input_tokens_yesterday + output_tokens_yesterday + cache_creation_yesterday + cache_read_yesterday)
                        else format_tokens(input_tokens_today + output_tokens_today + cache_creation_today + cache_read_today)
                    ),
                    "changeType": (
                        "increase"
                        if (input_tokens_today + output_tokens_today + cache_creation_today + cache_read_today)
                        > (input_tokens_yesterday + output_tokens_yesterday + cache_creation_yesterday + cache_read_yesterday)
                        else (
                            "decrease"
                            if (input_tokens_today + output_tokens_today + cache_creation_today + cache_read_today)
                            < (input_tokens_yesterday + output_tokens_yesterday + cache_creation_yesterday + cache_read_yesterday)
                            else "neutral"
                        )
                    ),
                    "icon": "Hash",
                },
                {
                    "name": "总缓存",
                    "value": format_tokens(all_time_cache_creation + all_time_cache_read),
                    "subValue": f"创建 {format_tokens(all_time_cache_creation)} / 读取 {format_tokens(all_time_cache_read)}",
                    "change": (
                        f"+{format_tokens(cache_creation_today + cache_read_today)}"
                        if (cache_creation_today + cache_read_today)
                        > (cache_creation_yesterday + cache_read_yesterday)
                        else format_tokens(cache_creation_today + cache_read_today)
                    ),
                    "changeType": (
                        "increase"
                        if (cache_creation_today + cache_read_today)
                        > (cache_creation_yesterday + cache_read_yesterday)
                        else (
                            "decrease"
                            if (cache_creation_today + cache_read_today)
                            < (cache_creation_yesterday + cache_read_yesterday)
                            else "neutral"
                        )
                    ),
                    "extraBadge": f"命中率 {cache_hit_rate}%",
                    "icon": "Database",
                },
            ],
            "today": {
                "requests": requests_today,
                "cost": cost_today,
                "actual_cost": actual_cost_today,
                "tokens": tokens_today,
                "cache_creation_tokens": cache_creation_today,
                "cache_read_tokens": cache_read_today,
            },
            "api_keys": {"total": total_api_keys, "active": active_api_keys},
            "tokens": {"month": total_tokens},
            "token_breakdown": {
                "input": all_time_input_tokens,
                "output": all_time_output_tokens,
                "cache_creation": all_time_cache_creation,
                "cache_read": all_time_cache_read,
            },
            "system_health": {
                "avg_response_time": round(avg_response_time_seconds, 2),
                "error_rate": error_rate,
                "error_requests": error_requests,
                "fallback_count": fallback_count,
                "total_requests": total_requests,
            },
            "cost_stats": {
                "total_cost": round(total_cost, 4),
                "total_actual_cost": round(total_actual_cost, 4),
                "cost_savings": round(total_cost - total_actual_cost, 4),
            },
            "cache_stats": {
                "cache_creation_tokens": cache_creation_tokens,
                "cache_read_tokens": cache_read_tokens,
                "cache_creation_cost": round(cache_creation_cost, 4),
                "cache_read_cost": round(cache_read_cost, 4),
                "total_cache_tokens": cache_creation_tokens + cache_read_tokens,
            },
            "users": {
                "total": total_users,
                "active": active_users,
            },
        }


class UserDashboardStatsAdapter(DashboardAdapter):
    @cache_result(key_prefix="dashboard:user:stats", ttl=30, user_specific=True)
    async def handle(self, context):  # type: ignore[override]
        from zoneinfo import ZoneInfo
        from src.services.system.stats_aggregator import APP_TIMEZONE

        db = context.db
        user = context.user
        # 使用业务时区计算日期，确保与用户感知的"今天"一致
        app_tz = ZoneInfo(APP_TIMEZONE)
        now_local = datetime.now(app_tz)
        today_local = now_local.replace(hour=0, minute=0, second=0, microsecond=0)
        # 转换为 UTC 用于数据库查询
        today = today_local.astimezone(timezone.utc)
        yesterday = (today_local - timedelta(days=1)).astimezone(timezone.utc)
        # 本月第一天（自然月）
        month_start_local = today_local.replace(day=1)
        month_start = month_start_local.astimezone(timezone.utc)

        user_api_keys = db.query(func.count(ApiKey.id)).filter(ApiKey.user_id == user.id).scalar()
        active_keys = (
            db.query(func.count(ApiKey.id))
            .filter(and_(ApiKey.user_id == user.id, ApiKey.is_active.is_(True)))
            .scalar()
        )

        # 全局 Token 统计
        all_time_token_stats = (
            db.query(
                func.sum(Usage.input_tokens).label("input_tokens"),
                func.sum(Usage.output_tokens).label("output_tokens"),
                func.sum(Usage.cache_creation_input_tokens).label("cache_creation_tokens"),
                func.sum(Usage.cache_read_input_tokens).label("cache_read_tokens"),
            )
            .filter(Usage.user_id == user.id)
            .first()
        )
        all_time_input_tokens = (
            int(all_time_token_stats.input_tokens or 0) if all_time_token_stats else 0
        )
        all_time_output_tokens = (
            int(all_time_token_stats.output_tokens or 0) if all_time_token_stats else 0
        )
        all_time_cache_creation = (
            int(all_time_token_stats.cache_creation_tokens or 0) if all_time_token_stats else 0
        )
        all_time_cache_read = (
            int(all_time_token_stats.cache_read_tokens or 0) if all_time_token_stats else 0
        )

        # 本月请求统计
        user_requests = (
            db.query(func.count(Usage.id))
            .filter(and_(Usage.user_id == user.id, Usage.created_at >= month_start))
            .scalar()
        )
        user_cost = (
            db.query(func.sum(Usage.total_cost_usd))
            .filter(and_(Usage.user_id == user.id, Usage.created_at >= month_start))
            .scalar()
            or 0
        )

        # 今日统计
        requests_today = (
            db.query(func.count(Usage.id))
            .filter(and_(Usage.user_id == user.id, Usage.created_at >= today))
            .scalar()
        )
        cost_today = (
            db.query(func.sum(Usage.total_cost_usd))
            .filter(and_(Usage.user_id == user.id, Usage.created_at >= today))
            .scalar()
            or 0
        )
        tokens_today = (
            db.query(func.sum(Usage.total_tokens))
            .filter(and_(Usage.user_id == user.id, Usage.created_at >= today))
            .scalar()
            or 0
        )

        # 昨日统计（用于计算变化）
        requests_yesterday = (
            db.query(func.count(Usage.id))
            .filter(
                and_(
                    Usage.user_id == user.id,
                    Usage.created_at >= yesterday,
                    Usage.created_at < today,
                )
            )
            .scalar()
        )

        # 缓存统计（本月）
        cache_stats = (
            db.query(
                func.sum(Usage.cache_creation_input_tokens).label("cache_creation_tokens"),
                func.sum(Usage.cache_read_input_tokens).label("cache_read_tokens"),
                func.sum(Usage.input_tokens).label("total_input_tokens"),
            )
            .filter(and_(Usage.user_id == user.id, Usage.created_at >= month_start))
            .first()
        )
        cache_creation_tokens = int(cache_stats.cache_creation_tokens or 0) if cache_stats else 0
        cache_read_tokens = int(cache_stats.cache_read_tokens or 0) if cache_stats else 0
        monthly_input_tokens = int(cache_stats.total_input_tokens or 0) if cache_stats else 0

        # 计算本月缓存命中率：cache_read / (input_tokens + cache_read)
        # input_tokens 是实际发送给模型的输入（不含缓存读取），cache_read 是从缓存读取的
        # 总输入 = input_tokens + cache_read，缓存命中率 = cache_read / 总输入
        total_input_with_cache = monthly_input_tokens + cache_read_tokens
        cache_hit_rate = (
            round((cache_read_tokens / total_input_with_cache) * 100, 1)
            if total_input_with_cache > 0
            else 0
        )

        # 今日缓存统计
        cache_stats_today = (
            db.query(
                func.sum(Usage.cache_creation_input_tokens).label("cache_creation_tokens"),
                func.sum(Usage.cache_read_input_tokens).label("cache_read_tokens"),
            )
            .filter(and_(Usage.user_id == user.id, Usage.created_at >= today))
            .first()
        )
        cache_creation_tokens_today = (
            int(cache_stats_today.cache_creation_tokens or 0) if cache_stats_today else 0
        )
        cache_read_tokens_today = (
            int(cache_stats_today.cache_read_tokens or 0) if cache_stats_today else 0
        )

        # 配额状态
        if user.quota_usd is None:
            quota_value = "无限制"
            quota_change = f"已用 ${user.used_usd:.2f}"
            quota_high = False
        elif user.quota_usd > 0:
            percent = min(100, int((user.used_usd / user.quota_usd) * 100))
            quota_value = f"${user.quota_usd:.0f}"
            quota_change = f"已用 ${user.used_usd:.2f}"
            quota_high = percent > 80
        else:
            quota_value = "$0"
            quota_change = f"已用 ${user.used_usd:.2f}"
            quota_high = True

        return {
            "stats": [
                {
                    "name": "API 密钥",
                    "value": f"{active_keys}/{user_api_keys}",
                    "icon": "Key",
                },
                {
                    "name": "本月请求",
                    "value": f"{user_requests:,}",
                    "change": f"今日 {requests_today}",
                    "changeType": (
                        "increase"
                        if requests_today > requests_yesterday
                        else ("decrease" if requests_today < requests_yesterday else "neutral")
                    ),
                    "icon": "Activity",
                },
                {
                    "name": "配额使用",
                    "value": quota_value,
                    "change": quota_change,
                    "changeType": "increase" if quota_high else "neutral",
                    "icon": "TrendingUp",
                },
                {
                    "name": "总Token",
                    "value": format_tokens(
                        all_time_input_tokens
                        + all_time_output_tokens
                        + all_time_cache_creation
                        + all_time_cache_read
                    ),
                    "subValue": f"输入 {format_tokens(all_time_input_tokens)} / 输出 {format_tokens(all_time_output_tokens)}",
                    "icon": "Hash",
                },
            ],
            "today": {
                "requests": requests_today,
                "cost": cost_today,
                "tokens": tokens_today,
                "cache_creation_tokens": cache_creation_tokens_today,
                "cache_read_tokens": cache_read_tokens_today,
            },
            # 全局 Token 详细分类（与管理员端对齐）
            "token_breakdown": {
                "input": all_time_input_tokens,
                "output": all_time_output_tokens,
                "cache_creation": all_time_cache_creation,
                "cache_read": all_time_cache_read,
            },
            # 用户视角：缓存使用情况
            "cache_stats": {
                "cache_creation_tokens": cache_creation_tokens,
                "cache_read_tokens": cache_read_tokens,
                "cache_hit_rate": cache_hit_rate,
                "total_cache_tokens": cache_creation_tokens + cache_read_tokens,
            },
            # 本月费用（用于下方缓存区域显示）
            "monthly_cost": float(user_cost),
        }


@dataclass
class DashboardRecentRequestsAdapter(DashboardAdapter):
    limit: int

    async def handle(self, context):  # type: ignore[override]
        db = context.db
        user = context.user
        query = db.query(Usage)
        if user.role != UserRole.ADMIN:
            query = query.filter(Usage.user_id == user.id)

        recent_requests = query.order_by(Usage.created_at.desc()).limit(self.limit).all()

        results = []
        for req in recent_requests:
            owner = db.query(DBUser).filter(DBUser.id == req.user_id).first()
            results.append(
                {
                    "id": req.id,
                    "user": owner.username if owner else "Unknown",
                    "model": req.model or "N/A",
                    "tokens": req.total_tokens,
                    "time": req.created_at.strftime("%H:%M") if req.created_at else None,
                    "is_stream": req.is_stream,
                }
            )

        return {"requests": results}


# NOTE: DashboardRequestDetailAdapter has been moved to AdminUsageDetailAdapter
# in src/api/admin/usage/routes.py


class DashboardProviderStatusAdapter(DashboardAdapter):
    @cache_result(key_prefix="dashboard:provider:status", ttl=60, user_specific=False)
    async def handle(self, context):  # type: ignore[override]
        db = context.db
        user = context.user
        providers = db.query(Provider).filter(Provider.is_active.is_(True)).all()
        since = datetime.now(timezone.utc) - timedelta(days=1)

        entries = []
        for provider in providers:
            count = (
                db.query(func.count(Usage.id))
                .filter(and_(Usage.provider_name == provider.name, Usage.created_at >= since))
                .scalar()
            )
            entries.append(
                {
                    "name": provider.name,
                    "status": "active" if provider.is_active else "inactive",
                    "requests": count,
                }
            )

        entries.sort(key=lambda x: x["requests"], reverse=True)
        limit = 10 if user.role == UserRole.ADMIN else 5
        return {"providers": entries[:limit]}


@dataclass
class DashboardDailyStatsAdapter(DashboardAdapter):
    days: int

    @cache_result(key_prefix="dashboard:daily:stats", ttl=CacheTTL.DASHBOARD_DAILY, user_specific=True)
    async def handle(self, context):  # type: ignore[override]
        from zoneinfo import ZoneInfo
        from src.services.system.stats_aggregator import APP_TIMEZONE

        db = context.db
        user = context.user
        is_admin = user.role == UserRole.ADMIN

        # 使用业务时区计算日期，确保每日统计与业务日期一致
        app_tz = ZoneInfo(APP_TIMEZONE)
        now_local = datetime.now(app_tz)
        today_local = now_local.replace(hour=0, minute=0, second=0, microsecond=0)
        # 转换为 UTC 用于数据库查询
        today = today_local.astimezone(timezone.utc)
        end_date_local = now_local.replace(hour=23, minute=59, second=59, microsecond=999999)
        end_date = end_date_local.astimezone(timezone.utc)
        start_date_local = (today_local - timedelta(days=self.days - 1))
        start_date = start_date_local.astimezone(timezone.utc)

        # ==================== 使用预聚合数据优化 ====================
        if is_admin:
            # 管理员：从 stats_daily 获取历史数据
            daily_stats = (
                db.query(StatsDaily)
                .filter(and_(StatsDaily.date >= start_date, StatsDaily.date < today))
                .order_by(StatsDaily.date.asc())
                .all()
            )
            # stats_daily.date 存储的是业务日期对应的 UTC 开始时间
            # 需要转回业务时区再取日期，才能与日期序列匹配
            def _to_business_date_str(value: datetime) -> str:
                if value.tzinfo is None:
                    value_utc = value.replace(tzinfo=timezone.utc)
                else:
                    value_utc = value.astimezone(timezone.utc)
                return value_utc.astimezone(app_tz).date().isoformat()

            stats_map = {
                _to_business_date_str(stat.date): {
                    "requests": stat.total_requests,
                    "tokens": stat.input_tokens + stat.output_tokens + stat.cache_creation_tokens + stat.cache_read_tokens,
                    "cost": stat.total_cost,
                    "avg_response_time": stat.avg_response_time_ms / 1000.0 if stat.avg_response_time_ms else 0,
                    "unique_models": getattr(stat, 'unique_models', 0) or 0,
                    "unique_providers": getattr(stat, 'unique_providers', 0) or 0,
                    "fallback_count": stat.fallback_count or 0,
                }
                for stat in daily_stats
            }

            # 今日实时数据
            today_stats = StatsAggregatorService.get_today_realtime_stats(db)
            today_str = today_local.date().isoformat()
            if today_stats["total_requests"] > 0:
                # 今日平均响应时间需要单独查询
                today_avg_rt = (
                    db.query(func.avg(Usage.response_time_ms))
                    .filter(Usage.created_at >= today, Usage.response_time_ms.isnot(None))
                    .scalar() or 0
                )
                # 今日 unique_models 和 unique_providers
                today_unique_models = (
                    db.query(func.count(func.distinct(Usage.model)))
                    .filter(Usage.created_at >= today)
                    .scalar() or 0
                )
                today_unique_providers = (
                    db.query(func.count(func.distinct(Usage.provider_name)))
                    .filter(Usage.created_at >= today)
                    .scalar() or 0
                )
                # 今日 fallback_count
                today_fallback_count = (
                    db.query(func.count())
                    .select_from(
                        db.query(RequestCandidate.request_id)
                        .filter(
                            RequestCandidate.created_at >= today,
                            RequestCandidate.status.in_(["success", "failed"]),
                        )
                        .group_by(RequestCandidate.request_id)
                        .having(func.count(RequestCandidate.id) > 1)
                        .subquery()
                    )
                    .scalar() or 0
                )
                stats_map[today_str] = {
                    "requests": today_stats["total_requests"],
                    "tokens": (today_stats["input_tokens"] + today_stats["output_tokens"] +
                              today_stats["cache_creation_tokens"] + today_stats["cache_read_tokens"]),
                    "cost": today_stats["total_cost"],
                    "avg_response_time": float(today_avg_rt) / 1000.0 if today_avg_rt else 0,
                    "unique_models": today_unique_models,
                    "unique_providers": today_unique_providers,
                    "fallback_count": today_fallback_count,
                }

            # 历史预聚合缺失时兜底：按业务日范围实时计算（仅补最近少量缺失，避免全表扫描）
            yesterday_date = today_local.date() - timedelta(days=1)
            historical_end = min(end_date_local.date(), yesterday_date)
            missing_dates: list[str] = []
            cursor = start_date_local.date()
            while cursor <= historical_end:
                date_str = cursor.isoformat()
                if date_str not in stats_map:
                    missing_dates.append(date_str)
                cursor += timedelta(days=1)

            if missing_dates:
                for date_str in missing_dates[-7:]:
                    target_local = datetime.fromisoformat(date_str).replace(tzinfo=app_tz)
                    computed = StatsAggregatorService.compute_daily_stats(db, target_local)
                    stats_map[date_str] = {
                        "requests": computed["total_requests"],
                        "tokens": (
                            computed["input_tokens"]
                            + computed["output_tokens"]
                            + computed["cache_creation_tokens"]
                            + computed["cache_read_tokens"]
                        ),
                        "cost": computed["total_cost"],
                        "avg_response_time": computed["avg_response_time_ms"] / 1000.0
                        if computed["avg_response_time_ms"]
                        else 0,
                        "unique_models": computed["unique_models"],
                        "unique_providers": computed["unique_providers"],
                        "fallback_count": computed["fallback_count"],
                    }
        else:
            # 普通用户：仍需实时查询（用户级预聚合可选）
            query = db.query(Usage).filter(
                and_(
                    Usage.user_id == user.id,
                    Usage.created_at >= start_date,
                    Usage.created_at <= end_date,
                )
            )

            user_daily_stats = (
                query.with_entities(
                    func.date(Usage.created_at).label("date"),
                    func.count(Usage.id).label("requests"),
                    func.sum(Usage.total_tokens).label("tokens"),
                    func.sum(Usage.total_cost_usd).label("cost"),
                    func.avg(Usage.response_time_ms).label("avg_response_time"),
                )
                .group_by(func.date(Usage.created_at))
                .order_by(func.date(Usage.created_at).asc())
                .all()
            )

            stats_map = {
                stat.date.isoformat(): {
                    "requests": stat.requests or 0,
                    "tokens": int(stat.tokens or 0),
                    "cost": float(stat.cost or 0),
                    "avg_response_time": float(stat.avg_response_time or 0) / 1000.0 if stat.avg_response_time else 0,
                }
                for stat in user_daily_stats
            }

        # 构建完整日期序列（使用业务时区日期）
        current_date = start_date_local.date()
        end_date_date = end_date_local.date()
        formatted: List[dict] = []
        while current_date <= end_date_date:
            date_str = current_date.isoformat()
            stat = stats_map.get(date_str)
            if stat:
                formatted.append({
                    "date": date_str,
                    "requests": stat["requests"],
                    "tokens": stat["tokens"],
                    "cost": stat["cost"],
                    "avg_response_time": stat["avg_response_time"],
                    "unique_models": stat.get("unique_models", 0),
                    "unique_providers": stat.get("unique_providers", 0),
                    "fallback_count": stat.get("fallback_count", 0),
                })
            else:
                formatted.append({
                    "date": date_str,
                    "requests": 0,
                    "tokens": 0,
                    "cost": 0.0,
                    "avg_response_time": 0.0,
                    "unique_models": 0,
                    "unique_providers": 0,
                    "fallback_count": 0,
                })
            current_date += timedelta(days=1)

        # ==================== 模型统计 ====================
        if is_admin:
            # 管理员：使用预聚合数据 + 今日实时数据
            # 历史数据从 stats_daily_model 获取
            historical_model_stats = (
                db.query(StatsDailyModel)
                .filter(and_(StatsDailyModel.date >= start_date, StatsDailyModel.date < today))
                .all()
            )

            # 按模型汇总历史数据
            model_agg: dict = {}
            daily_breakdown: dict = {}

            for stat in historical_model_stats:
                model = stat.model
                if model not in model_agg:
                    model_agg[model] = {
                        "requests": 0, "tokens": 0, "cost": 0.0,
                        "total_response_time": 0.0, "response_count": 0
                    }
                model_agg[model]["requests"] += stat.total_requests
                tokens = (stat.input_tokens + stat.output_tokens +
                          stat.cache_creation_tokens + stat.cache_read_tokens)
                model_agg[model]["tokens"] += tokens
                model_agg[model]["cost"] += stat.total_cost
                if stat.avg_response_time_ms is not None:
                    model_agg[model]["total_response_time"] += stat.avg_response_time_ms * stat.total_requests
                    model_agg[model]["response_count"] += stat.total_requests

                # 按日期分组
                if stat.date.tzinfo is None:
                    date_utc = stat.date.replace(tzinfo=timezone.utc)
                else:
                    date_utc = stat.date.astimezone(timezone.utc)
                date_str = date_utc.astimezone(app_tz).date().isoformat()

                daily_breakdown.setdefault(date_str, []).append({
                    "model": model,
                    "requests": stat.total_requests,
                    "tokens": tokens,
                    "cost": stat.total_cost,
                })

            # 今日实时模型统计
            today_model_stats = (
                db.query(
                    Usage.model,
                    func.count(Usage.id).label("requests"),
                    func.sum(Usage.total_tokens).label("tokens"),
                    func.sum(Usage.total_cost_usd).label("cost"),
                    func.avg(Usage.response_time_ms).label("avg_response_time"),
                )
                .filter(Usage.created_at >= today)
                .group_by(Usage.model)
                .all()
            )

            today_str = today_local.date().isoformat()
            for stat in today_model_stats:
                model = stat.model
                if model not in model_agg:
                    model_agg[model] = {
                        "requests": 0, "tokens": 0, "cost": 0.0,
                        "total_response_time": 0.0, "response_count": 0
                    }
                model_agg[model]["requests"] += stat.requests or 0
                model_agg[model]["tokens"] += int(stat.tokens or 0)
                model_agg[model]["cost"] += float(stat.cost or 0)
                if stat.avg_response_time is not None:
                    model_agg[model]["total_response_time"] += float(stat.avg_response_time) * (stat.requests or 0)
                    model_agg[model]["response_count"] += stat.requests or 0

                # 今日 breakdown
                daily_breakdown.setdefault(today_str, []).append({
                    "model": model,
                    "requests": stat.requests or 0,
                    "tokens": int(stat.tokens or 0),
                    "cost": float(stat.cost or 0),
                })

            # 构建 model_summary
            model_summary = []
            for model, agg in model_agg.items():
                avg_rt = (agg["total_response_time"] / agg["response_count"] / 1000.0
                          if agg["response_count"] > 0 else 0)
                model_summary.append({
                    "model": model,
                    "requests": agg["requests"],
                    "tokens": agg["tokens"],
                    "cost": agg["cost"],
                    "avg_response_time": avg_rt,
                    "cost_per_request": agg["cost"] / max(agg["requests"], 1),
                    "tokens_per_request": agg["tokens"] / max(agg["requests"], 1),
                })
            model_summary.sort(key=lambda x: x["cost"], reverse=True)

            # 填充 model_breakdown
            for item in formatted:
                item["model_breakdown"] = daily_breakdown.get(item["date"], [])

        else:
            # 普通用户：实时查询（数据量较小）
            model_query = db.query(Usage).filter(
                and_(
                    Usage.user_id == user.id,
                    Usage.created_at >= start_date,
                    Usage.created_at <= end_date
                )
            )

            model_stats = (
                model_query.with_entities(
                    Usage.model,
                    func.count(Usage.id).label("requests"),
                    func.sum(Usage.total_tokens).label("tokens"),
                    func.sum(Usage.total_cost_usd).label("cost"),
                    func.avg(Usage.response_time_ms).label("avg_response_time"),
                )
                .group_by(Usage.model)
                .order_by(func.sum(Usage.total_cost_usd).desc())
                .all()
            )

            model_summary = [
                {
                    "model": stat.model,
                    "requests": stat.requests or 0,
                    "tokens": int(stat.tokens or 0),
                    "cost": float(stat.cost or 0),
                    "avg_response_time": (
                        float(stat.avg_response_time or 0) / 1000.0 if stat.avg_response_time else 0
                    ),
                    "cost_per_request": float(stat.cost or 0) / max(stat.requests or 1, 1),
                    "tokens_per_request": int(stat.tokens or 0) / max(stat.requests or 1, 1),
                }
                for stat in model_stats
            ]

            daily_model_stats = (
                model_query.with_entities(
                    func.date(Usage.created_at).label("date"),
                    Usage.model,
                    func.count(Usage.id).label("requests"),
                    func.sum(Usage.total_tokens).label("tokens"),
                    func.sum(Usage.total_cost_usd).label("cost"),
                )
                .group_by(func.date(Usage.created_at), Usage.model)
                .order_by(func.date(Usage.created_at).desc(), func.sum(Usage.total_cost_usd).desc())
                .all()
            )

            breakdown = {}
            for stat in daily_model_stats:
                date_str = stat.date.isoformat()
                breakdown.setdefault(date_str, []).append(
                    {
                        "model": stat.model,
                        "requests": stat.requests or 0,
                        "tokens": int(stat.tokens or 0),
                        "cost": float(stat.cost or 0),
                    }
                )

            for item in formatted:
                item["model_breakdown"] = breakdown.get(item["date"], [])

        # ==================== 供应商统计 ====================
        if is_admin:
            # 管理员：使用预聚合数据 + 今日实时数据

            # 历史数据从 stats_daily_provider 获取
            historical_provider_stats = (
                db.query(StatsDailyProvider)
                .filter(and_(StatsDailyProvider.date >= start_date, StatsDailyProvider.date < today))
                .all()
            )

            # 按供应商汇总历史数据
            provider_agg: dict[str, dict[str, int | float]] = {}
            for stat in historical_provider_stats:
                provider = stat.provider_name or "Unknown"
                if provider not in provider_agg:
                    provider_agg[provider] = {"requests": 0, "tokens": 0, "cost": 0.0}
                provider_agg[provider]["requests"] += stat.total_requests
                tokens = (stat.input_tokens + stat.output_tokens +
                          stat.cache_creation_tokens + stat.cache_read_tokens)
                provider_agg[provider]["tokens"] += tokens
                provider_agg[provider]["cost"] += stat.total_cost

            # 今日实时供应商统计
            today_provider_stats = (
                db.query(
                    Usage.provider_name,
                    func.count(Usage.id).label("requests"),
                    func.sum(Usage.total_tokens).label("tokens"),
                    func.sum(Usage.total_cost_usd).label("cost"),
                )
                .filter(Usage.created_at >= today)
                .group_by(Usage.provider_name)
                .all()
            )

            for stat in today_provider_stats:
                provider = stat.provider_name or "Unknown"
                if provider not in provider_agg:
                    provider_agg[provider] = {"requests": 0, "tokens": 0, "cost": 0.0}
                provider_agg[provider]["requests"] += stat.requests or 0
                provider_agg[provider]["tokens"] += int(stat.tokens or 0)
                provider_agg[provider]["cost"] += float(stat.cost or 0)

            # 构建 provider_summary
            provider_summary = [
                {
                    "provider": provider,
                    "requests": agg["requests"],
                    "tokens": agg["tokens"],
                    "cost": agg["cost"],
                }
                for provider, agg in provider_agg.items()
            ]
            provider_summary.sort(key=lambda x: x["cost"], reverse=True)

        else:
            # 普通用户：实时查询
            provider_stats = (
                db.query(
                    Usage.provider_name,
                    func.count(Usage.id).label("requests"),
                    func.sum(Usage.total_tokens).label("tokens"),
                    func.sum(Usage.total_cost_usd).label("cost"),
                )
                .filter(
                    and_(
                        Usage.user_id == user.id,
                        Usage.created_at >= start_date,
                        Usage.created_at <= end_date
                    )
                )
                .group_by(Usage.provider_name)
                .order_by(func.sum(Usage.total_cost_usd).desc())
                .all()
            )

            provider_summary = [
                {
                    "provider": stat.provider_name or "Unknown",
                    "requests": stat.requests or 0,
                    "tokens": int(stat.tokens or 0),
                    "cost": float(stat.cost or 0),
                }
                for stat in provider_stats
            ]

        return {
            "daily_stats": formatted,
            "model_summary": model_summary,
            "provider_summary": provider_summary,
            "period": {
                "start_date": start_date.date().isoformat(),
                "end_date": end_date.date().isoformat(),
                "days": self.days,
            },
        }
