"""Admin leaderboard stats routes."""

from __future__ import annotations

from datetime import date
from typing import Any, Literal

from fastapi import APIRouter, Depends, Query, Request
from sqlalchemy import func
from sqlalchemy.orm import Session

from src.api.base.admin_adapter import AdminApiAdapter
from src.api.base.context import ApiRequestContext
from src.clients.redis_client import get_redis_client_sync
from src.config.constants import CacheTTL
from src.core.enums import UserRole
from src.database import get_db
from src.models.database import (
    ApiKey,
    StatsDailyApiKey,
    StatsDailyModel,
    StatsUserDaily,
    Usage,
    User,
)
from src.services.system.time_range import TimeRangeParams
from src.utils import json_helper as json

from .common import (
    _apply_admin_default_range,
    _apply_usage_time_segments,
    _build_cache_key,
    _build_time_range_params,
    _is_today_range,
    _metric_order,
    _split_daily_and_usage_segments,
    _union_queries,
    pipeline,
)

router = APIRouter()


class AdminUserLeaderboardAdapter(AdminApiAdapter):
    def __init__(
        self,
        time_range: TimeRangeParams | None,
        metric: Literal["requests", "tokens", "cost"],
        order: Literal["asc", "desc"],
        limit: int,
        offset: int,
        provider_name: str | None,
        model: str | None,
        include_inactive: bool,
        exclude_admin: bool,
    ) -> None:
        self.time_range = _apply_admin_default_range(time_range)
        self.metric = metric
        self.order = order
        self.limit = limit
        self.offset = offset
        self.provider_name = provider_name
        self.model = model
        self.include_inactive = include_inactive
        self.exclude_admin = exclude_admin

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        cacheable = not _is_today_range(self.time_range)
        redis_client = get_redis_client_sync()
        cache_key = None
        if cacheable and redis_client:
            cache_key = _build_cache_key(
                "users",
                self.metric,
                self.time_range,
                {
                    "order": self.order,
                    "limit": self.limit,
                    "offset": self.offset,
                    "provider_name": self.provider_name,
                    "model": self.model,
                    "include_inactive": self.include_inactive,
                    "exclude_admin": self.exclude_admin,
                },
            )
            cached = await redis_client.get(cache_key)
            if cached:
                try:
                    return json.loads(cached)
                except Exception:
                    pass

        use_daily = self.time_range is not None and not self.provider_name and not self.model
        daily_range, usage_segments = _split_daily_and_usage_segments(self.time_range, use_daily)

        daily_query = None
        if daily_range:
            daily_query = (
                db.query(
                    StatsUserDaily.user_id.label("entity_id"),
                    func.sum(StatsUserDaily.total_requests).label("requests"),
                    func.sum(
                        StatsUserDaily.input_tokens
                        + StatsUserDaily.output_tokens
                        + StatsUserDaily.cache_creation_tokens
                        + StatsUserDaily.cache_read_tokens
                    ).label("tokens"),
                    func.sum(StatsUserDaily.total_cost).label("cost"),
                )
                .filter(StatsUserDaily.date >= daily_range[0], StatsUserDaily.date < daily_range[1])
                .group_by(StatsUserDaily.user_id)
            )

        usage_query = db.query(
            Usage.user_id.label("entity_id"),
            func.count(Usage.id).label("requests"),
            func.sum(
                Usage.input_tokens
                + Usage.output_tokens
                + Usage.cache_creation_input_tokens
                + Usage.cache_read_input_tokens
            ).label("tokens"),
            func.sum(Usage.total_cost_usd).label("cost"),
        ).filter(
            Usage.user_id.isnot(None),
            Usage.status.notin_(["pending", "streaming"]),
            Usage.provider_name.notin_(["unknown", "pending"]),
        )
        if self.provider_name:
            usage_query = usage_query.filter(Usage.provider_name == self.provider_name)
        if self.model:
            usage_query = usage_query.filter(Usage.model == self.model)
        usage_query = _apply_usage_time_segments(usage_query, usage_segments)
        if usage_query is not None:
            usage_query = usage_query.group_by(Usage.user_id)

        union_query = _union_queries([daily_query, usage_query])
        if union_query is None:
            return {
                "items": [],
                "total": 0,
                "metric": self.metric,
                "start_date": self.time_range.start_date.isoformat() if self.time_range else None,
                "end_date": self.time_range.end_date.isoformat() if self.time_range else None,
            }

        union_subq = union_query.subquery()
        agg_subq = (
            db.query(
                union_subq.c.entity_id.label("entity_id"),
                func.sum(union_subq.c.requests).label("requests"),
                func.sum(union_subq.c.tokens).label("tokens"),
                func.sum(union_subq.c.cost).label("cost"),
            )
            .group_by(union_subq.c.entity_id)
            .subquery()
        )

        base_query = (
            db.query(
                User.id.label("id"),
                User.username,
                User.email,
                agg_subq.c.requests,
                agg_subq.c.tokens,
                agg_subq.c.cost,
            )
            .join(agg_subq, agg_subq.c.entity_id == User.id)
            .filter(User.is_deleted.is_(False))
        )
        if not self.include_inactive:
            base_query = base_query.filter(User.is_active.is_(True))
        if self.exclude_admin:
            base_query = base_query.filter(User.role != UserRole.ADMIN)

        metric_expr = {
            "requests": agg_subq.c.requests,
            "tokens": agg_subq.c.tokens,
            "cost": agg_subq.c.cost,
        }[self.metric]
        order_expr = _metric_order(self.metric, self.order, metric_expr)
        rank_expr = func.dense_rank().over(order_by=order_expr).label("rank")

        total = db.query(func.count()).select_from(base_query.subquery()).scalar() or 0
        rows = (
            base_query.add_columns(rank_expr, metric_expr.label("metric_value"))
            .order_by(order_expr)
            .offset(self.offset)
            .limit(self.limit)
            .all()
        )

        items = []
        for row in rows:
            name = row.username or row.email or str(row.id)
            value = row.metric_value or 0
            if self.metric in {"requests", "tokens"}:
                value = int(value)
            else:
                value = float(value)
            items.append(
                {
                    "rank": int(row.rank),
                    "id": row.id,
                    "name": name,
                    "value": value,
                    "requests": int(row.requests or 0),
                    "tokens": int(row.tokens or 0),
                    "cost": float(row.cost or 0.0),
                }
            )

        context.add_audit_metadata(
            action="leaderboard_users",
            start_date=self.time_range.start_date.isoformat() if self.time_range else None,
            end_date=self.time_range.end_date.isoformat() if self.time_range else None,
            preset=self.time_range.preset if self.time_range else None,
            timezone=self.time_range.timezone if self.time_range else None,
            metric=self.metric,
            order=self.order,
            limit=self.limit,
            offset=self.offset,
            provider_name=self.provider_name,
            model=self.model,
            include_inactive=self.include_inactive,
            exclude_admin=self.exclude_admin,
            result_count=len(items),
            total=total,
        )

        result = {
            "items": items,
            "total": total,
            "metric": self.metric,
            "start_date": self.time_range.start_date.isoformat() if self.time_range else None,
            "end_date": self.time_range.end_date.isoformat() if self.time_range else None,
        }

        if cacheable and redis_client and cache_key:
            try:
                await redis_client.setex(
                    cache_key, CacheTTL.ADMIN_LEADERBOARD, json.dumps(result, ensure_ascii=False)
                )
            except Exception:
                pass

        return result


class AdminApiKeyLeaderboardAdapter(AdminApiAdapter):
    def __init__(
        self,
        time_range: TimeRangeParams | None,
        metric: Literal["requests", "tokens", "cost"],
        order: Literal["asc", "desc"],
        limit: int,
        offset: int,
        provider_name: str | None,
        model: str | None,
        include_inactive: bool,
        exclude_admin: bool,
    ) -> None:
        self.time_range = _apply_admin_default_range(time_range)
        self.metric = metric
        self.order = order
        self.limit = limit
        self.offset = offset
        self.provider_name = provider_name
        self.model = model
        self.include_inactive = include_inactive
        self.exclude_admin = exclude_admin

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        cacheable = not _is_today_range(self.time_range)
        redis_client = get_redis_client_sync()
        cache_key = None
        if cacheable and redis_client:
            cache_key = _build_cache_key(
                "api_keys",
                self.metric,
                self.time_range,
                {
                    "order": self.order,
                    "limit": self.limit,
                    "offset": self.offset,
                    "provider_name": self.provider_name,
                    "model": self.model,
                    "include_inactive": self.include_inactive,
                    "exclude_admin": self.exclude_admin,
                },
            )
            cached = await redis_client.get(cache_key)
            if cached:
                try:
                    return json.loads(cached)
                except Exception:
                    pass

        use_daily = self.time_range is not None and not self.provider_name and not self.model
        daily_range, usage_segments = _split_daily_and_usage_segments(self.time_range, use_daily)

        daily_query = None
        if daily_range:
            daily_query = (
                db.query(
                    StatsDailyApiKey.api_key_id.label("entity_id"),
                    func.sum(StatsDailyApiKey.total_requests).label("requests"),
                    func.sum(
                        StatsDailyApiKey.input_tokens
                        + StatsDailyApiKey.output_tokens
                        + StatsDailyApiKey.cache_creation_tokens
                        + StatsDailyApiKey.cache_read_tokens
                    ).label("tokens"),
                    func.sum(StatsDailyApiKey.total_cost).label("cost"),
                )
                .filter(
                    StatsDailyApiKey.date >= daily_range[0], StatsDailyApiKey.date < daily_range[1]
                )
                .group_by(StatsDailyApiKey.api_key_id)
            )

        usage_query = db.query(
            Usage.api_key_id.label("entity_id"),
            func.count(Usage.id).label("requests"),
            func.sum(
                Usage.input_tokens
                + Usage.output_tokens
                + Usage.cache_creation_input_tokens
                + Usage.cache_read_input_tokens
            ).label("tokens"),
            func.sum(Usage.total_cost_usd).label("cost"),
        ).filter(
            Usage.api_key_id.isnot(None),
            Usage.status.notin_(["pending", "streaming"]),
            Usage.provider_name.notin_(["unknown", "pending"]),
        )
        if self.provider_name:
            usage_query = usage_query.filter(Usage.provider_name == self.provider_name)
        if self.model:
            usage_query = usage_query.filter(Usage.model == self.model)

        usage_query = _apply_usage_time_segments(usage_query, usage_segments)
        if usage_query is None:
            return {
                "items": [],
                "total": 0,
                "metric": self.metric,
                "start_date": self.time_range.start_date.isoformat() if self.time_range else None,
                "end_date": self.time_range.end_date.isoformat() if self.time_range else None,
            }
        usage_query = usage_query.group_by(Usage.api_key_id)

        union_query = _union_queries([daily_query, usage_query])
        if union_query is None:
            return {
                "items": [],
                "total": 0,
                "metric": self.metric,
                "start_date": self.time_range.start_date.isoformat() if self.time_range else None,
                "end_date": self.time_range.end_date.isoformat() if self.time_range else None,
            }

        union_subq = union_query.subquery()
        agg_subq = (
            db.query(
                union_subq.c.entity_id.label("entity_id"),
                func.sum(union_subq.c.requests).label("requests"),
                func.sum(union_subq.c.tokens).label("tokens"),
                func.sum(union_subq.c.cost).label("cost"),
            )
            .group_by(union_subq.c.entity_id)
            .subquery()
        )

        base_query = (
            db.query(
                ApiKey,
                User,
                agg_subq.c.requests,
                agg_subq.c.tokens,
                agg_subq.c.cost,
            )
            .join(agg_subq, agg_subq.c.entity_id == ApiKey.id)
            .join(User, User.id == ApiKey.user_id)
            .filter(User.is_deleted.is_(False))
        )
        if not self.include_inactive:
            base_query = base_query.filter(ApiKey.is_active.is_(True))
        if self.exclude_admin:
            base_query = base_query.filter(User.role != UserRole.ADMIN)

        metric_expr = {
            "requests": agg_subq.c.requests,
            "tokens": agg_subq.c.tokens,
            "cost": agg_subq.c.cost,
        }[self.metric]
        order_expr = _metric_order(self.metric, self.order, metric_expr)
        rank_expr = func.dense_rank().over(order_by=order_expr).label("rank")

        total = db.query(func.count()).select_from(base_query.subquery()).scalar() or 0
        rows = (
            base_query.add_columns(rank_expr, metric_expr.label("metric_value"))
            .order_by(order_expr)
            .offset(self.offset)
            .limit(self.limit)
            .all()
        )

        items = []
        for row in rows:
            api_key = row.ApiKey
            name = api_key.name or api_key.get_display_key()
            value = row.metric_value or 0
            if self.metric in {"requests", "tokens"}:
                value = int(value)
            else:
                value = float(value)
            items.append(
                {
                    "rank": int(row.rank),
                    "id": api_key.id,
                    "name": name,
                    "value": value,
                    "requests": int(row.requests or 0),
                    "tokens": int(row.tokens or 0),
                    "cost": float(row.cost or 0.0),
                }
            )

        context.add_audit_metadata(
            action="leaderboard_api_keys",
            start_date=self.time_range.start_date.isoformat() if self.time_range else None,
            end_date=self.time_range.end_date.isoformat() if self.time_range else None,
            preset=self.time_range.preset if self.time_range else None,
            timezone=self.time_range.timezone if self.time_range else None,
            metric=self.metric,
            order=self.order,
            limit=self.limit,
            offset=self.offset,
            provider_name=self.provider_name,
            model=self.model,
            include_inactive=self.include_inactive,
            exclude_admin=self.exclude_admin,
            result_count=len(items),
            total=total,
        )

        result = {
            "items": items,
            "total": total,
            "metric": self.metric,
            "start_date": self.time_range.start_date.isoformat() if self.time_range else None,
            "end_date": self.time_range.end_date.isoformat() if self.time_range else None,
        }

        if cacheable and redis_client and cache_key:
            try:
                await redis_client.setex(
                    cache_key, CacheTTL.ADMIN_LEADERBOARD, json.dumps(result, ensure_ascii=False)
                )
            except Exception:
                pass

        return result


class AdminModelLeaderboardAdapter(AdminApiAdapter):
    def __init__(
        self,
        time_range: TimeRangeParams | None,
        metric: Literal["requests", "tokens", "cost"],
        order: Literal["asc", "desc"],
        limit: int,
        offset: int,
        provider_name: str | None,
        model: str | None,
    ) -> None:
        self.time_range = _apply_admin_default_range(time_range)
        self.metric = metric
        self.order = order
        self.limit = limit
        self.offset = offset
        self.provider_name = provider_name
        self.model = model

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        cacheable = not _is_today_range(self.time_range)
        redis_client = get_redis_client_sync()
        cache_key = None
        if cacheable and redis_client:
            cache_key = _build_cache_key(
                "models",
                self.metric,
                self.time_range,
                {
                    "order": self.order,
                    "limit": self.limit,
                    "offset": self.offset,
                    "provider_name": self.provider_name,
                    "model": self.model,
                },
            )
            cached = await redis_client.get(cache_key)
            if cached:
                try:
                    return json.loads(cached)
                except Exception:
                    pass

        use_daily = self.time_range is not None and not self.provider_name
        daily_range, usage_segments = _split_daily_and_usage_segments(self.time_range, use_daily)

        daily_query = None
        if daily_range:
            daily_query = (
                db.query(
                    StatsDailyModel.model.label("entity_id"),
                    func.sum(StatsDailyModel.total_requests).label("requests"),
                    func.sum(
                        StatsDailyModel.input_tokens
                        + StatsDailyModel.output_tokens
                        + StatsDailyModel.cache_creation_tokens
                        + StatsDailyModel.cache_read_tokens
                    ).label("tokens"),
                    func.sum(StatsDailyModel.total_cost).label("cost"),
                )
                .filter(
                    StatsDailyModel.date >= daily_range[0], StatsDailyModel.date < daily_range[1]
                )
                .group_by(StatsDailyModel.model)
            )
            if self.model:
                daily_query = daily_query.filter(StatsDailyModel.model == self.model)

        usage_query = db.query(
            Usage.model.label("entity_id"),
            func.count(Usage.id).label("requests"),
            func.sum(
                Usage.input_tokens
                + Usage.output_tokens
                + Usage.cache_creation_input_tokens
                + Usage.cache_read_input_tokens
            ).label("tokens"),
            func.sum(Usage.total_cost_usd).label("cost"),
        ).filter(
            Usage.status.notin_(["pending", "streaming"]),
            Usage.provider_name.notin_(["unknown", "pending"]),
        )
        if self.provider_name:
            usage_query = usage_query.filter(Usage.provider_name == self.provider_name)
        if self.model:
            usage_query = usage_query.filter(Usage.model == self.model)
        usage_query = _apply_usage_time_segments(usage_query, usage_segments)
        if usage_query is not None:
            usage_query = usage_query.group_by(Usage.model)

        union_query = _union_queries([daily_query, usage_query])
        if union_query is None:
            return {
                "items": [],
                "total": 0,
                "metric": self.metric,
                "start_date": self.time_range.start_date.isoformat() if self.time_range else None,
                "end_date": self.time_range.end_date.isoformat() if self.time_range else None,
            }

        union_subq = union_query.subquery()
        agg_subq = (
            db.query(
                union_subq.c.entity_id.label("entity_id"),
                func.sum(union_subq.c.requests).label("requests"),
                func.sum(union_subq.c.tokens).label("tokens"),
                func.sum(union_subq.c.cost).label("cost"),
            )
            .group_by(union_subq.c.entity_id)
            .subquery()
        )

        base_query = db.query(
            agg_subq.c.entity_id.label("id"),
            agg_subq.c.entity_id.label("name"),
            agg_subq.c.requests,
            agg_subq.c.tokens,
            agg_subq.c.cost,
        )

        metric_expr = {
            "requests": agg_subq.c.requests,
            "tokens": agg_subq.c.tokens,
            "cost": agg_subq.c.cost,
        }[self.metric]
        order_expr = _metric_order(self.metric, self.order, metric_expr)
        rank_expr = func.dense_rank().over(order_by=order_expr).label("rank")

        total = db.query(func.count()).select_from(base_query.subquery()).scalar() or 0
        rows = (
            base_query.add_columns(rank_expr, metric_expr.label("metric_value"))
            .order_by(order_expr)
            .offset(self.offset)
            .limit(self.limit)
            .all()
        )

        items = []
        for row in rows:
            value = row.metric_value or 0
            if self.metric in {"requests", "tokens"}:
                value = int(value)
            else:
                value = float(value)
            items.append(
                {
                    "rank": int(row.rank),
                    "id": row.id,
                    "name": row.name,
                    "value": value,
                    "requests": int(row.requests or 0),
                    "tokens": int(row.tokens or 0),
                    "cost": float(row.cost or 0.0),
                }
            )

        context.add_audit_metadata(
            action="leaderboard_models",
            start_date=self.time_range.start_date.isoformat() if self.time_range else None,
            end_date=self.time_range.end_date.isoformat() if self.time_range else None,
            preset=self.time_range.preset if self.time_range else None,
            timezone=self.time_range.timezone if self.time_range else None,
            metric=self.metric,
            order=self.order,
            limit=self.limit,
            offset=self.offset,
            provider_name=self.provider_name,
            model=self.model,
            result_count=len(items),
            total=total,
        )

        result = {
            "items": items,
            "total": total,
            "metric": self.metric,
            "start_date": self.time_range.start_date.isoformat() if self.time_range else None,
            "end_date": self.time_range.end_date.isoformat() if self.time_range else None,
        }

        if cacheable and redis_client and cache_key:
            try:
                await redis_client.setex(
                    cache_key, CacheTTL.ADMIN_LEADERBOARD, json.dumps(result, ensure_ascii=False)
                )
            except Exception:
                pass

        return result


@router.get("/leaderboard/users")
async def get_user_leaderboard(
    request: Request,
    db: Session = Depends(get_db),
    start_date: date | None = Query(None),
    end_date: date | None = Query(None),
    preset: str | None = Query(None),
    timezone_name: str | None = Query(None, alias="timezone"),
    tz_offset_minutes: int | None = Query(0),
    metric: Literal["requests", "tokens", "cost"] = Query("requests"),
    order: Literal["desc", "asc"] = Query("desc"),
    limit: int = Query(10, ge=1, le=100),
    offset: int = Query(0, ge=0),
    provider_name: str | None = Query(None),
    model: str | None = Query(None),
    include_inactive: bool = Query(False),
    exclude_admin: bool = Query(False),
) -> Any:
    time_range = _build_time_range_params(
        start_date, end_date, preset, timezone_name, tz_offset_minutes
    )
    adapter = AdminUserLeaderboardAdapter(
        time_range=time_range,
        metric=metric,
        order=order,
        limit=limit,
        offset=offset,
        provider_name=provider_name,
        model=model,
        include_inactive=include_inactive,
        exclude_admin=exclude_admin,
    )
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/leaderboard/api-keys")
async def get_api_key_leaderboard(
    request: Request,
    db: Session = Depends(get_db),
    start_date: date | None = Query(None),
    end_date: date | None = Query(None),
    preset: str | None = Query(None),
    timezone_name: str | None = Query(None, alias="timezone"),
    tz_offset_minutes: int | None = Query(0),
    metric: Literal["requests", "tokens", "cost"] = Query("requests"),
    order: Literal["desc", "asc"] = Query("desc"),
    limit: int = Query(10, ge=1, le=100),
    offset: int = Query(0, ge=0),
    provider_name: str | None = Query(None),
    model: str | None = Query(None),
    include_inactive: bool = Query(False),
    exclude_admin: bool = Query(False),
) -> Any:
    time_range = _build_time_range_params(
        start_date, end_date, preset, timezone_name, tz_offset_minutes
    )
    adapter = AdminApiKeyLeaderboardAdapter(
        time_range=time_range,
        metric=metric,
        order=order,
        limit=limit,
        offset=offset,
        provider_name=provider_name,
        model=model,
        include_inactive=include_inactive,
        exclude_admin=exclude_admin,
    )
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/leaderboard/models")
async def get_model_leaderboard(
    request: Request,
    db: Session = Depends(get_db),
    start_date: date | None = Query(None),
    end_date: date | None = Query(None),
    preset: str | None = Query(None),
    timezone_name: str | None = Query(None, alias="timezone"),
    tz_offset_minutes: int | None = Query(0),
    metric: Literal["requests", "tokens", "cost"] = Query("requests"),
    order: Literal["desc", "asc"] = Query("desc"),
    limit: int = Query(10, ge=1, le=100),
    offset: int = Query(0, ge=0),
    provider_name: str | None = Query(None),
    model: str | None = Query(None),
) -> Any:
    time_range = _build_time_range_params(
        start_date, end_date, preset, timezone_name, tz_offset_minutes
    )
    adapter = AdminModelLeaderboardAdapter(
        time_range=time_range,
        metric=metric,
        order=order,
        limit=limit,
        offset=offset,
        provider_name=provider_name,
        model=model,
    )
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)
