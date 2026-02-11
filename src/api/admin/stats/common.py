"""Shared helpers for admin stats routes."""

from __future__ import annotations

import hashlib
from datetime import date, datetime, time, timedelta, timezone
from typing import Any, Literal

from fastapi import HTTPException
from sqlalchemy import and_, or_

from src.api.base.pipeline import ApiRequestPipeline
from src.config.settings import config
from src.models.database import Usage
from src.services.system.time_range import TimeRangeParams
from src.utils import json_helper as json

pipeline = ApiRequestPipeline()


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


def _hash_filters(filters: dict[str, Any]) -> str:
    raw = json.dumps(filters, sort_keys=True, ensure_ascii=False, default=str)
    return hashlib.sha1(raw.encode("utf-8")).hexdigest()[:16]


def _build_time_range_from_days(
    days: int, timezone_name: str | None, tz_offset_minutes: int | None
) -> TimeRangeParams:
    base = TimeRangeParams(
        preset="today",
        timezone=timezone_name,
        tz_offset_minutes=tz_offset_minutes or 0,
    ).validate_and_resolve()
    user_today = base.start_date
    start_date = user_today - timedelta(days=days - 1)
    return TimeRangeParams(
        start_date=start_date,
        end_date=user_today,
        timezone=timezone_name,
        tz_offset_minutes=tz_offset_minutes or 0,
    ).validate_and_resolve()


def _linear_regression(values: list[float]) -> tuple[float, float]:
    n = len(values)
    if n <= 1:
        return 0.0, values[0] if values else 0.0
    xs = list(range(n))
    sum_x = sum(xs)
    sum_y = sum(values)
    sum_x2 = sum(x * x for x in xs)
    sum_xy = sum(x * y for x, y in zip(xs, values))
    denom = n * sum_x2 - sum_x * sum_x
    if denom == 0:
        return 0.0, values[-1]
    slope = (n * sum_xy - sum_x * sum_y) / denom
    intercept = (sum_y - slope * sum_x) / n
    return slope, intercept


def _build_cache_key(
    leaderboard_type: str,
    metric: str,
    time_range: TimeRangeParams | None,
    filters: dict[str, Any],
) -> str:
    start_value = time_range.start_date.isoformat() if time_range else "all"
    end_value = time_range.end_date.isoformat() if time_range else "all"
    tz_value = time_range.timezone if time_range else "utc"
    offset_value = time_range.tz_offset_minutes if time_range else 0
    return (
        f"leaderboard:{leaderboard_type}:{metric}:{start_value}:{end_value}:"
        f"{tz_value}:{offset_value}:{_hash_filters(filters)}"
    )


def _is_today_range(time_range: TimeRangeParams | None) -> bool:
    if not time_range:
        return False
    try:
        user_today = time_range._get_user_today()
    except Exception:
        return False
    return time_range.end_date == user_today


def _split_daily_and_usage_segments(
    time_range: TimeRangeParams | None,
    use_daily: bool,
) -> tuple[tuple[datetime, datetime] | None, list[tuple[datetime, datetime]] | None]:
    if not time_range:
        return None, None

    start_utc, end_utc = time_range.to_utc_datetime_range()
    if not use_daily:
        return None, [(start_utc, end_utc)]

    complete_dates, head_boundary, tail_boundary = time_range.get_complete_utc_dates()
    daily_range = None
    if complete_dates:
        daily_start = datetime.combine(complete_dates[0], time.min, tzinfo=timezone.utc)
        daily_end = datetime.combine(
            complete_dates[-1] + timedelta(days=1), time.min, tzinfo=timezone.utc
        )
        daily_range = (daily_start, daily_end)

    usage_segments: list[tuple[datetime, datetime]] = []
    if head_boundary:
        usage_segments.append(head_boundary)
    if tail_boundary:
        usage_segments.append(tail_boundary)

    if not daily_range and not usage_segments:
        usage_segments = [(start_utc, end_utc)]

    return daily_range, usage_segments


def _apply_usage_time_segments(
    query: Any, segments: list[tuple[datetime, datetime]] | None
) -> Any | None:
    if segments is None:
        return query
    if not segments:
        return None

    conditions = []
    for start_utc, end_utc in segments:
        if start_utc >= end_utc:
            continue
        conditions.append(and_(Usage.created_at >= start_utc, Usage.created_at < end_utc))

    if not conditions:
        return None

    return query.filter(or_(*conditions))


def _union_queries(queries: list[Any]) -> Any | None:
    base = None
    for query in queries:
        if query is None:
            continue
        if base is None:
            base = query
        else:
            base = base.union_all(query)
    return base


def _metric_order(
    metric: Literal["requests", "tokens", "cost"], order: Literal["asc", "desc"], expr: Any
) -> Any:
    return expr.asc() if order == "asc" else expr.desc()
