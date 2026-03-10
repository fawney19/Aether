"""统计数据聚合服务

实现预聚合统计，避免每次请求都全表扫描。
"""

from __future__ import annotations

import os
import uuid
from dataclasses import dataclass
from datetime import date, datetime, time, timedelta, timezone
from decimal import Decimal
from typing import Any

from sqlalchemy import Float, and_, case, cast, func
from sqlalchemy.exc import IntegrityError
from sqlalchemy.orm import Session

from src.core.logger import logger
from src.models.database import (
    ApiKey,
    RequestCandidate,
    StatsDaily,
    StatsDailyApiKey,
    StatsDailyError,
    StatsDailyModel,
    StatsDailyProvider,
    StatsHourly,
    StatsHourlyModel,
    StatsHourlyProvider,
    StatsHourlyUser,
    StatsSummary,
    StatsUserDaily,
    Usage,
)
from src.models.database import User as DBUser
from src.services.system.time_range import TimeRangeParams, split_time_range_for_hourly

# App timezone (legacy defaults for dashboard)
APP_TIMEZONE = os.getenv("APP_TIMEZONE", "Asia/Shanghai")
MIN_PERCENTILE_SAMPLES = 10


def _get_utc_day_range(value: datetime) -> tuple[datetime, datetime]:
    """Convert a date (UTC) to [start, end) UTC range."""
    day_start = datetime(value.year, value.month, value.day, 0, 0, 0, tzinfo=timezone.utc)
    return day_start, day_start + timedelta(days=1)


class StatsAggregatorService:
    """统计数据聚合服务"""

    @staticmethod
    def compute_daily_stats(db: Session, date: datetime) -> dict:
        """计算指定 UTC 日期的统计数据（不写入数据库）"""
        day_start, day_end = _get_utc_day_range(date)

        error_cond = (Usage.status_code >= 400) | (Usage.error_message.isnot(None))
        aggregated = (
            db.query(
                func.count(Usage.id).label("total_requests"),
                func.sum(case((error_cond, 1), else_=0)).label("error_requests"),
                func.sum(Usage.input_tokens).label("input_tokens"),
                func.sum(Usage.output_tokens).label("output_tokens"),
                func.sum(Usage.cache_creation_input_tokens).label("cache_creation_tokens"),
                func.sum(Usage.cache_read_input_tokens).label("cache_read_tokens"),
                func.sum(Usage.total_cost_usd).label("total_cost"),
                func.sum(Usage.actual_total_cost_usd).label("actual_total_cost"),
                func.sum(Usage.input_cost_usd).label("input_cost"),
                func.sum(Usage.output_cost_usd).label("output_cost"),
                func.sum(Usage.cache_creation_cost_usd).label("cache_creation_cost"),
                func.sum(Usage.cache_read_cost_usd).label("cache_read_cost"),
                func.avg(Usage.response_time_ms).label("avg_response_time"),
                func.count(func.distinct(Usage.model)).label("unique_models"),
                func.count(func.distinct(Usage.provider_name)).label("unique_providers"),
            )
            .filter(and_(Usage.created_at >= day_start, Usage.created_at < day_end))
            .first()
        )

        total_requests = int(getattr(aggregated, "total_requests", 0) or 0)
        if total_requests == 0:
            return {
                "day_start": day_start,
                "total_requests": 0,
                "success_requests": 0,
                "error_requests": 0,
                "input_tokens": 0,
                "output_tokens": 0,
                "cache_creation_tokens": 0,
                "cache_read_tokens": 0,
                "total_cost": 0.0,
                "actual_total_cost": 0.0,
                "input_cost": 0.0,
                "output_cost": 0.0,
                "cache_creation_cost": 0.0,
                "cache_read_cost": 0.0,
                "avg_response_time_ms": 0.0,
                "fallback_count": 0,
                "unique_models": 0,
                "unique_providers": 0,
            }

        error_requests = int(getattr(aggregated, "error_requests", 0) or 0)

        # Fallback 统计 (执行候选数 > 1 的请求数)
        fallback_subquery = (
            db.query(
                RequestCandidate.request_id,
                func.count(RequestCandidate.id).label("executed_count"),
            )
            .filter(
                and_(
                    RequestCandidate.created_at >= day_start,
                    RequestCandidate.created_at < day_end,
                    RequestCandidate.status.in_(["success", "failed"]),
                )
            )
            .group_by(RequestCandidate.request_id)
            .subquery()
        )
        fallback_count = (
            db.query(func.count())
            .select_from(fallback_subquery)
            .filter(fallback_subquery.c.executed_count > 1)
            .scalar()
            or 0
        )

        return {
            "day_start": day_start,
            "total_requests": total_requests,
            "success_requests": total_requests - error_requests,
            "error_requests": error_requests,
            "input_tokens": int(getattr(aggregated, "input_tokens", 0) or 0),
            "output_tokens": int(getattr(aggregated, "output_tokens", 0) or 0),
            "cache_creation_tokens": (int(getattr(aggregated, "cache_creation_tokens", 0) or 0)),
            "cache_read_tokens": int(getattr(aggregated, "cache_read_tokens", 0) or 0),
            "total_cost": float(getattr(aggregated, "total_cost", 0) or 0.0),
            "actual_total_cost": float(getattr(aggregated, "actual_total_cost", 0) or 0.0),
            "input_cost": float(getattr(aggregated, "input_cost", 0) or 0.0),
            "output_cost": float(getattr(aggregated, "output_cost", 0) or 0.0),
            "cache_creation_cost": (float(getattr(aggregated, "cache_creation_cost", 0) or 0.0)),
            "cache_read_cost": float(getattr(aggregated, "cache_read_cost", 0) or 0.0),
            "avg_response_time_ms": float(getattr(aggregated, "avg_response_time", 0) or 0.0),
            "fallback_count": fallback_count,
            "unique_models": int(getattr(aggregated, "unique_models", 0) or 0),
            "unique_providers": int(getattr(aggregated, "unique_providers", 0) or 0),
        }

    @staticmethod
    def compute_daily_percentiles(
        db: Session, day_start: datetime, day_end: datetime
    ) -> dict[str, int | None]:
        """计算指定 UTC 日期的性能百分位（P50/P90/P99）"""
        bind = db.bind
        dialect = bind.dialect.name if bind is not None else "sqlite"
        if dialect != "postgresql":
            return {
                "p50_response_time_ms": None,
                "p90_response_time_ms": None,
                "p99_response_time_ms": None,
                "p50_first_byte_time_ms": None,
                "p90_first_byte_time_ms": None,
                "p99_first_byte_time_ms": None,
            }
        rt_row = (
            db.query(
                func.percentile_cont(0.5).within_group(Usage.response_time_ms).label("p50"),
                func.percentile_cont(0.9).within_group(Usage.response_time_ms).label("p90"),
                func.percentile_cont(0.99).within_group(Usage.response_time_ms).label("p99"),
                func.count().label("count"),
            )
            .filter(
                Usage.created_at >= day_start,
                Usage.created_at < day_end,
                Usage.status == "completed",
                Usage.response_time_ms.isnot(None),
            )
            .first()
        )

        ttfb_row = (
            db.query(
                func.percentile_cont(0.5).within_group(Usage.first_byte_time_ms).label("p50"),
                func.percentile_cont(0.9).within_group(Usage.first_byte_time_ms).label("p90"),
                func.percentile_cont(0.99).within_group(Usage.first_byte_time_ms).label("p99"),
                func.count().label("count"),
            )
            .filter(
                Usage.created_at >= day_start,
                Usage.created_at < day_end,
                Usage.status == "completed",
                Usage.first_byte_time_ms.isnot(None),
            )
            .first()
        )

        def _resolve(row: Any | None) -> tuple[int | None, int | None, int | None]:
            if not row:
                return None, None, None
            count = int(getattr(row, "count", 0) or 0)
            if count < MIN_PERCENTILE_SAMPLES:
                return None, None, None
            p50 = getattr(row, "p50", None)
            p90 = getattr(row, "p90", None)
            p99 = getattr(row, "p99", None)
            return (
                int(p50) if p50 is not None else None,
                int(p90) if p90 is not None else None,
                int(p99) if p99 is not None else None,
            )

        p50_rt, p90_rt, p99_rt = _resolve(rt_row)
        p50_ttfb, p90_ttfb, p99_ttfb = _resolve(ttfb_row)

        return {
            "p50_response_time_ms": p50_rt,
            "p90_response_time_ms": p90_rt,
            "p99_response_time_ms": p99_rt,
            "p50_first_byte_time_ms": p50_ttfb,
            "p90_first_byte_time_ms": p90_ttfb,
            "p99_first_byte_time_ms": p99_ttfb,
        }

    @staticmethod
    def aggregate_daily_stats(db: Session, date: datetime, commit: bool = True) -> StatsDaily:
        """聚合指定 UTC 日期的统计数据

        Args:
            db: 数据库会话
            date: 要聚合的 UTC 日期
            commit: 是否立即提交事务

        Returns:
            StatsDaily 记录
        """
        computed = StatsAggregatorService.compute_daily_stats(db, date)
        day_start = computed["day_start"]

        # stats_daily.date 存储的是 UTC 日期对应的开始时间
        # 检查是否已存在该日期的记录
        existing = db.query(StatsDaily).filter(StatsDaily.date == day_start).first()
        if existing:
            stats = existing
        else:
            stats = StatsDaily(id=str(uuid.uuid4()), date=day_start)

        # 更新统计记录
        stats.total_requests = computed["total_requests"]
        stats.success_requests = computed["success_requests"]
        stats.error_requests = computed["error_requests"]
        stats.input_tokens = computed["input_tokens"]
        stats.output_tokens = computed["output_tokens"]
        stats.cache_creation_tokens = computed["cache_creation_tokens"]
        stats.cache_read_tokens = computed["cache_read_tokens"]
        stats.total_cost = computed["total_cost"]
        stats.actual_total_cost = computed["actual_total_cost"]
        stats.input_cost = computed["input_cost"]
        stats.output_cost = computed["output_cost"]
        stats.cache_creation_cost = computed["cache_creation_cost"]
        stats.cache_read_cost = computed["cache_read_cost"]
        stats.avg_response_time_ms = computed["avg_response_time_ms"]
        stats.fallback_count = computed["fallback_count"]
        stats.unique_models = computed["unique_models"]
        stats.unique_providers = computed["unique_providers"]
        percentiles = StatsAggregatorService.compute_daily_percentiles(
            db, day_start, day_start + timedelta(days=1)
        )
        stats.p50_response_time_ms = percentiles["p50_response_time_ms"]
        stats.p90_response_time_ms = percentiles["p90_response_time_ms"]
        stats.p99_response_time_ms = percentiles["p99_response_time_ms"]
        stats.p50_first_byte_time_ms = percentiles["p50_first_byte_time_ms"]
        stats.p90_first_byte_time_ms = percentiles["p90_first_byte_time_ms"]
        stats.p99_first_byte_time_ms = percentiles["p99_first_byte_time_ms"]

        if not existing:
            db.add(stats)
        if commit:
            db.commit()

        return stats

    @staticmethod
    def aggregate_daily_model_stats(
        db: Session, date: datetime, commit: bool = True
    ) -> list[StatsDailyModel]:
        """聚合指定日期的模型维度统计数据

        Args:
            db: 数据库会话
            date: 要聚合的 UTC 日期
            commit: 是否立即提交事务

        Returns:
            StatsDailyModel 记录列表
        """
        day_start, day_end = _get_utc_day_range(date)

        # 按模型分组统计
        model_stats = (
            db.query(
                Usage.model,
                func.count(Usage.id).label("total_requests"),
                func.sum(Usage.input_tokens).label("input_tokens"),
                func.sum(Usage.output_tokens).label("output_tokens"),
                func.sum(Usage.cache_creation_input_tokens).label("cache_creation_tokens"),
                func.sum(Usage.cache_read_input_tokens).label("cache_read_tokens"),
                func.sum(Usage.total_cost_usd).label("total_cost"),
                func.avg(Usage.response_time_ms).label("avg_response_time"),
            )
            .filter(and_(Usage.created_at >= day_start, Usage.created_at < day_end))
            .group_by(Usage.model)
            .all()
        )

        results = []
        for stat in model_stats:
            if not stat.model:
                continue

            existing = (
                db.query(StatsDailyModel)
                .filter(
                    and_(StatsDailyModel.date == day_start, StatsDailyModel.model == stat.model)
                )
                .first()
            )

            if existing:
                record = existing
            else:
                record = StatsDailyModel(id=str(uuid.uuid4()), date=day_start, model=stat.model)

            record.total_requests = stat.total_requests or 0
            record.input_tokens = int(stat.input_tokens or 0)
            record.output_tokens = int(stat.output_tokens or 0)
            record.cache_creation_tokens = int(stat.cache_creation_tokens or 0)
            record.cache_read_tokens = int(stat.cache_read_tokens or 0)
            record.total_cost = float(stat.total_cost or 0)
            record.avg_response_time_ms = float(stat.avg_response_time or 0)

            if not existing:
                db.add(record)
            results.append(record)

        if commit:
            db.commit()
        return results

    @staticmethod
    def aggregate_daily_provider_stats(
        db: Session, date: datetime, commit: bool = True
    ) -> list[StatsDailyProvider]:
        """聚合指定日期的供应商维度统计数据

        Args:
            db: 数据库会话
            date: 要聚合的 UTC 日期
            commit: 是否立即提交事务

        Returns:
            StatsDailyProvider 记录列表
        """
        day_start, day_end = _get_utc_day_range(date)

        # 按供应商分组统计
        provider_name_expr = func.coalesce(Usage.provider_name, "Unknown")
        provider_stats = (
            db.query(
                provider_name_expr.label("provider_name"),
                func.count(Usage.id).label("total_requests"),
                func.sum(Usage.input_tokens).label("input_tokens"),
                func.sum(Usage.output_tokens).label("output_tokens"),
                func.sum(Usage.cache_creation_input_tokens).label("cache_creation_tokens"),
                func.sum(Usage.cache_read_input_tokens).label("cache_read_tokens"),
                func.sum(Usage.total_cost_usd).label("total_cost"),
            )
            .filter(and_(Usage.created_at >= day_start, Usage.created_at < day_end))
            .group_by(provider_name_expr)
            .all()
        )

        results = []
        for stat in provider_stats:
            existing = (
                db.query(StatsDailyProvider)
                .filter(
                    and_(
                        StatsDailyProvider.date == day_start,
                        StatsDailyProvider.provider_name == stat.provider_name,
                    )
                )
                .first()
            )

            if existing:
                record = existing
            else:
                record = StatsDailyProvider(
                    id=str(uuid.uuid4()), date=day_start, provider_name=stat.provider_name
                )

            record.total_requests = stat.total_requests or 0
            record.input_tokens = int(stat.input_tokens or 0)
            record.output_tokens = int(stat.output_tokens or 0)
            record.cache_creation_tokens = int(stat.cache_creation_tokens or 0)
            record.cache_read_tokens = int(stat.cache_read_tokens or 0)
            record.total_cost = float(stat.total_cost or 0)

            if not existing:
                db.add(record)
            results.append(record)

        if commit:
            db.commit()
        return results

    @staticmethod
    def aggregate_daily_api_key_stats(
        db: Session, date: datetime, commit: bool = True
    ) -> list[StatsDailyApiKey]:
        """聚合指定日期的 API Key 维度统计数据"""
        day_start, day_end = _get_utc_day_range(date)
        error_cond = (Usage.status_code >= 400) | (Usage.error_message.isnot(None))

        stats = (
            db.query(
                Usage.api_key_id,
                func.max(Usage.api_key_name).label("api_key_name"),
                func.count(Usage.id).label("total_requests"),
                func.sum(case((error_cond, 1), else_=0)).label("error_requests"),
                func.sum(Usage.input_tokens).label("input_tokens"),
                func.sum(Usage.output_tokens).label("output_tokens"),
                func.sum(Usage.cache_creation_input_tokens).label("cache_creation_tokens"),
                func.sum(Usage.cache_read_input_tokens).label("cache_read_tokens"),
                func.sum(Usage.total_cost_usd).label("total_cost"),
            )
            .filter(and_(Usage.created_at >= day_start, Usage.created_at < day_end))
            .filter(Usage.api_key_id.isnot(None))
            .group_by(Usage.api_key_id)
            .all()
        )

        results = []
        for stat in stats:
            existing = (
                db.query(StatsDailyApiKey)
                .filter(
                    and_(
                        StatsDailyApiKey.date == day_start,
                        StatsDailyApiKey.api_key_id == stat.api_key_id,
                    )
                )
                .first()
            )

            if existing:
                record = existing
            else:
                record = StatsDailyApiKey(
                    id=str(uuid.uuid4()), date=day_start, api_key_id=stat.api_key_id
                )

            # 填充 api_key_name 快照（优先用已有值，新数据从 usage 聚合获取）
            if not record.api_key_name and stat.api_key_name:
                record.api_key_name = stat.api_key_name

            error_requests = int(stat.error_requests or 0)
            total_requests = int(stat.total_requests or 0)
            record.total_requests = total_requests
            record.success_requests = total_requests - error_requests
            record.error_requests = error_requests
            record.input_tokens = int(stat.input_tokens or 0)
            record.output_tokens = int(stat.output_tokens or 0)
            record.cache_creation_tokens = int(stat.cache_creation_tokens or 0)
            record.cache_read_tokens = int(stat.cache_read_tokens or 0)
            record.total_cost = float(stat.total_cost or 0)

            if not existing:
                db.add(record)
            results.append(record)

        if commit:
            db.commit()
        return results

    @staticmethod
    def aggregate_daily_error_stats(
        db: Session, date: datetime, commit: bool = True
    ) -> list[StatsDailyError]:
        """聚合指定日期的错误分类统计数据"""
        day_start, day_end = _get_utc_day_range(date)

        db.query(StatsDailyError).filter(StatsDailyError.date == day_start).delete(
            synchronize_session=False
        )

        rows = (
            db.query(
                Usage.error_category,
                Usage.provider_name,
                Usage.model,
                func.count(Usage.id).label("cnt"),
            )
            .filter(and_(Usage.created_at >= day_start, Usage.created_at < day_end))
            .filter(Usage.error_category.isnot(None))
            .group_by(Usage.error_category, Usage.provider_name, Usage.model)
            .all()
        )

        results = []
        for row in rows:
            record = StatsDailyError(
                id=str(uuid.uuid4()),
                date=day_start,
                error_category=row.error_category,
                provider_name=row.provider_name,
                model=row.model,
                count=int(row.cnt or 0),
            )
            db.add(record)
            results.append(record)

        if commit:
            db.commit()
        return results

    @staticmethod
    def get_daily_model_stats(db: Session, start_date: datetime, end_date: datetime) -> list[dict]:
        """获取日期范围内的模型统计数据（优先使用预聚合）

        Args:
            db: 数据库会话
            start_date: 开始日期 (UTC)
            end_date: 结束日期 (UTC)

        Returns:
            模型统计数据列表
        """
        # 从预聚合表获取历史数据
        stats = (
            db.query(StatsDailyModel)
            .filter(and_(StatsDailyModel.date >= start_date, StatsDailyModel.date < end_date))
            .order_by(StatsDailyModel.date.asc(), StatsDailyModel.total_cost.desc())
            .all()
        )

        # 转换为字典格式，按 UTC 日期分组
        result = []
        for stat in stats:
            if stat.date.tzinfo is None:
                date_utc = stat.date.replace(tzinfo=timezone.utc)
            else:
                date_utc = stat.date.astimezone(timezone.utc)
            date_str = date_utc.date().isoformat()

            result.append(
                {
                    "date": date_str,
                    "model": stat.model,
                    "requests": stat.total_requests,
                    "tokens": (
                        stat.input_tokens
                        + stat.output_tokens
                        + stat.cache_creation_tokens
                        + stat.cache_read_tokens
                    ),
                    "cost": stat.total_cost,
                    "avg_response_time": (
                        stat.avg_response_time_ms / 1000.0 if stat.avg_response_time_ms else 0
                    ),
                }
            )

        return result

    @staticmethod
    def aggregate_user_daily_stats(
        db: Session, user_id: str, date: datetime, commit: bool = True
    ) -> StatsUserDaily:
        """聚合指定用户指定 UTC 日期的统计数据"""
        # 将 UTC 日期转换为 UTC 时间范围
        day_start, day_end = _get_utc_day_range(date)

        existing = (
            db.query(StatsUserDaily)
            .filter(and_(StatsUserDaily.user_id == user_id, StatsUserDaily.date == day_start))
            .first()
        )

        if existing:
            stats = existing
        else:
            stats = StatsUserDaily(id=str(uuid.uuid4()), user_id=user_id, date=day_start)

        error_cond = (Usage.status_code >= 400) | (Usage.error_message.isnot(None))
        aggregated = (
            db.query(
                func.count(Usage.id).label("total_requests"),
                func.sum(case((error_cond, 1), else_=0)).label("error_requests"),
                func.sum(Usage.input_tokens).label("input_tokens"),
                func.sum(Usage.output_tokens).label("output_tokens"),
                func.sum(Usage.cache_creation_input_tokens).label("cache_creation_tokens"),
                func.sum(Usage.cache_read_input_tokens).label("cache_read_tokens"),
                func.sum(Usage.total_cost_usd).label("total_cost"),
                func.max(Usage.username).label("username"),
            )
            .filter(
                and_(
                    Usage.user_id == user_id,
                    Usage.created_at >= day_start,
                    Usage.created_at < day_end,
                )
            )
            .first()
        )

        # 填充 username 快照：从 Usage 聚合获取，用户删除后仍可追溯
        if not stats.username:
            username = getattr(aggregated, "username", None)
            if username:
                stats.username = username

        total_requests = int(getattr(aggregated, "total_requests", 0) or 0)
        if total_requests == 0:
            stats.total_requests = 0
            stats.success_requests = 0
            stats.error_requests = 0
            stats.input_tokens = 0
            stats.output_tokens = 0
            stats.cache_creation_tokens = 0
            stats.cache_read_tokens = 0
            stats.total_cost = 0.0

            if not existing:
                db.add(stats)
            if commit:
                db.commit()
            return stats

        error_requests = int(getattr(aggregated, "error_requests", 0) or 0)

        stats.total_requests = total_requests
        stats.success_requests = total_requests - error_requests
        stats.error_requests = error_requests
        stats.input_tokens = int(getattr(aggregated, "input_tokens", 0) or 0)
        stats.output_tokens = int(getattr(aggregated, "output_tokens", 0) or 0)
        stats.cache_creation_tokens = int(getattr(aggregated, "cache_creation_tokens", 0) or 0)
        stats.cache_read_tokens = int(getattr(aggregated, "cache_read_tokens", 0) or 0)
        stats.total_cost = float(getattr(aggregated, "total_cost", 0) or 0.0)

        if not existing:
            db.add(stats)
        if commit:
            db.commit()
        return stats

    @staticmethod
    def aggregate_daily_stats_bundle(
        db: Session, date: datetime, user_ids: list[str] | None = None
    ) -> StatsDaily:
        """聚合单日所有统计（原子提交）"""
        stats = StatsAggregatorService.aggregate_daily_stats(db, date, commit=False)
        StatsAggregatorService.aggregate_daily_model_stats(db, date, commit=False)
        StatsAggregatorService.aggregate_daily_provider_stats(db, date, commit=False)
        StatsAggregatorService.aggregate_daily_api_key_stats(db, date, commit=False)
        StatsAggregatorService.aggregate_daily_error_stats(db, date, commit=False)

        if user_ids:
            for user_id in user_ids:
                StatsAggregatorService.aggregate_user_daily_stats(db, user_id, date, commit=False)

        stats.is_complete = True
        stats.aggregated_at = datetime.now(timezone.utc)
        db.commit()
        return stats

    @staticmethod
    def aggregate_hourly_stats(db: Session, hour_utc: datetime, commit: bool = True) -> StatsHourly:
        """聚合指定 UTC 小时的全局统计"""
        if hour_utc.tzinfo is None:
            hour_utc = hour_utc.replace(tzinfo=timezone.utc)
        hour_start = hour_utc.astimezone(timezone.utc).replace(minute=0, second=0, microsecond=0)
        hour_end = hour_start + timedelta(hours=1)

        error_cond = (Usage.status_code >= 400) | (Usage.error_message.isnot(None))
        aggregated = (
            db.query(
                func.count(Usage.id).label("total_requests"),
                func.sum(case((error_cond, 1), else_=0)).label("error_requests"),
                func.sum(Usage.input_tokens).label("input_tokens"),
                func.sum(Usage.output_tokens).label("output_tokens"),
                func.sum(Usage.cache_creation_input_tokens).label("cache_creation_tokens"),
                func.sum(Usage.cache_read_input_tokens).label("cache_read_tokens"),
                func.sum(Usage.total_cost_usd).label("total_cost"),
                func.sum(Usage.actual_total_cost_usd).label("actual_total_cost"),
                func.avg(Usage.response_time_ms).label("avg_response_time"),
            )
            .filter(and_(Usage.created_at >= hour_start, Usage.created_at < hour_end))
            .first()
        )

        existing = db.query(StatsHourly).filter(StatsHourly.hour_utc == hour_start).first()
        stats = existing or StatsHourly(id=str(uuid.uuid4()), hour_utc=hour_start)

        total_requests = int(getattr(aggregated, "total_requests", 0) or 0)
        error_requests = int(getattr(aggregated, "error_requests", 0) or 0)
        stats.total_requests = total_requests
        stats.success_requests = total_requests - error_requests
        stats.error_requests = error_requests
        stats.input_tokens = int(getattr(aggregated, "input_tokens", 0) or 0)
        stats.output_tokens = int(getattr(aggregated, "output_tokens", 0) or 0)
        stats.cache_creation_tokens = int(getattr(aggregated, "cache_creation_tokens", 0) or 0)
        stats.cache_read_tokens = int(getattr(aggregated, "cache_read_tokens", 0) or 0)
        stats.total_cost = float(getattr(aggregated, "total_cost", 0) or 0.0)
        stats.actual_total_cost = float(getattr(aggregated, "actual_total_cost", 0) or 0.0)
        stats.avg_response_time_ms = float(getattr(aggregated, "avg_response_time", 0) or 0.0)

        if not existing:
            db.add(stats)
        if commit:
            db.commit()
        return stats

    @staticmethod
    def aggregate_hourly_user_stats(
        db: Session, hour_utc: datetime, commit: bool = True
    ) -> list[StatsHourlyUser]:
        """聚合指定 UTC 小时的用户维度统计"""
        if hour_utc.tzinfo is None:
            hour_utc = hour_utc.replace(tzinfo=timezone.utc)
        hour_start = hour_utc.astimezone(timezone.utc).replace(minute=0, second=0, microsecond=0)
        hour_end = hour_start + timedelta(hours=1)

        error_cond = (Usage.status_code >= 400) | (Usage.error_message.isnot(None))
        rows = (
            db.query(
                Usage.user_id,
                func.count(Usage.id).label("total_requests"),
                func.sum(case((error_cond, 1), else_=0)).label("error_requests"),
                func.sum(Usage.input_tokens).label("input_tokens"),
                func.sum(Usage.output_tokens).label("output_tokens"),
                func.sum(Usage.total_cost_usd).label("total_cost"),
            )
            .filter(and_(Usage.created_at >= hour_start, Usage.created_at < hour_end))
            .filter(Usage.user_id.isnot(None))
            .group_by(Usage.user_id)
            .all()
        )

        results = []
        for row in rows:
            existing = (
                db.query(StatsHourlyUser)
                .filter(
                    and_(
                        StatsHourlyUser.hour_utc == hour_start,
                        StatsHourlyUser.user_id == row.user_id,
                    )
                )
                .first()
            )
            record = existing or StatsHourlyUser(
                id=str(uuid.uuid4()), hour_utc=hour_start, user_id=row.user_id
            )
            total_requests = int(row.total_requests or 0)
            error_requests = int(row.error_requests or 0)
            record.total_requests = total_requests
            record.success_requests = total_requests - error_requests
            record.error_requests = error_requests
            record.input_tokens = int(row.input_tokens or 0)
            record.output_tokens = int(row.output_tokens or 0)
            record.total_cost = float(row.total_cost or 0)

            if not existing:
                db.add(record)
            results.append(record)

        if commit:
            db.commit()
        return results

    @staticmethod
    def aggregate_hourly_model_stats(
        db: Session, hour_utc: datetime, commit: bool = True
    ) -> list[StatsHourlyModel]:
        """聚合指定 UTC 小时的模型维度统计"""
        if hour_utc.tzinfo is None:
            hour_utc = hour_utc.replace(tzinfo=timezone.utc)
        hour_start = hour_utc.astimezone(timezone.utc).replace(minute=0, second=0, microsecond=0)
        hour_end = hour_start + timedelta(hours=1)

        rows = (
            db.query(
                Usage.model,
                func.count(Usage.id).label("total_requests"),
                func.sum(Usage.input_tokens).label("input_tokens"),
                func.sum(Usage.output_tokens).label("output_tokens"),
                func.sum(Usage.total_cost_usd).label("total_cost"),
                func.avg(Usage.response_time_ms).label("avg_response_time"),
            )
            .filter(and_(Usage.created_at >= hour_start, Usage.created_at < hour_end))
            .group_by(Usage.model)
            .all()
        )

        results = []
        for row in rows:
            if not row.model:
                continue
            existing = (
                db.query(StatsHourlyModel)
                .filter(
                    and_(
                        StatsHourlyModel.hour_utc == hour_start, StatsHourlyModel.model == row.model
                    )
                )
                .first()
            )
            record = existing or StatsHourlyModel(
                id=str(uuid.uuid4()), hour_utc=hour_start, model=row.model
            )
            record.total_requests = int(row.total_requests or 0)
            record.input_tokens = int(row.input_tokens or 0)
            record.output_tokens = int(row.output_tokens or 0)
            record.total_cost = float(row.total_cost or 0)
            record.avg_response_time_ms = float(row.avg_response_time or 0)

            if not existing:
                db.add(record)
            results.append(record)

        if commit:
            db.commit()
        return results

    @staticmethod
    def aggregate_hourly_provider_stats(
        db: Session, hour_utc: datetime, commit: bool = True
    ) -> list[StatsHourlyProvider]:
        """聚合指定 UTC 小时的提供商维度统计"""
        if hour_utc.tzinfo is None:
            hour_utc = hour_utc.replace(tzinfo=timezone.utc)
        hour_start = hour_utc.astimezone(timezone.utc).replace(minute=0, second=0, microsecond=0)
        hour_end = hour_start + timedelta(hours=1)

        rows = (
            db.query(
                Usage.provider_name,
                func.count(Usage.id).label("total_requests"),
                func.sum(Usage.input_tokens).label("input_tokens"),
                func.sum(Usage.output_tokens).label("output_tokens"),
                func.sum(Usage.total_cost_usd).label("total_cost"),
            )
            .filter(and_(Usage.created_at >= hour_start, Usage.created_at < hour_end))
            .group_by(Usage.provider_name)
            .all()
        )

        results = []
        for row in rows:
            if not row.provider_name:
                continue
            existing = (
                db.query(StatsHourlyProvider)
                .filter(
                    and_(
                        StatsHourlyProvider.hour_utc == hour_start,
                        StatsHourlyProvider.provider_name == row.provider_name,
                    )
                )
                .first()
            )
            record = existing or StatsHourlyProvider(
                id=str(uuid.uuid4()),
                hour_utc=hour_start,
                provider_name=row.provider_name,
            )
            record.total_requests = int(row.total_requests or 0)
            record.input_tokens = int(row.input_tokens or 0)
            record.output_tokens = int(row.output_tokens or 0)
            record.total_cost = float(row.total_cost or 0)

            if not existing:
                db.add(record)
            results.append(record)

        if commit:
            db.commit()
        return results

    @staticmethod
    def aggregate_hourly_stats_bundle(db: Session, hour_utc: datetime) -> StatsHourly:
        """聚合单小时所有统计（原子提交）"""

        def _do_aggregate() -> StatsHourly:
            stats = StatsAggregatorService.aggregate_hourly_stats(db, hour_utc, commit=False)
            StatsAggregatorService.aggregate_hourly_user_stats(db, hour_utc, commit=False)
            StatsAggregatorService.aggregate_hourly_model_stats(db, hour_utc, commit=False)
            StatsAggregatorService.aggregate_hourly_provider_stats(db, hour_utc, commit=False)
            stats.is_complete = True
            stats.aggregated_at = datetime.now(timezone.utc)
            db.commit()
            return stats

        try:
            return _do_aggregate()
        except IntegrityError:
            db.rollback()
            logger.warning("小时统计聚合冲突，重试更新: {}", hour_utc)
            return _do_aggregate()

    @staticmethod
    def update_summary(db: Session) -> StatsSummary:
        """更新全局统计汇总

        汇总截止到昨天的所有数据。
        """
        # 以 UTC 日为边界
        now_utc = datetime.now(timezone.utc)
        cutoff_date = now_utc.replace(hour=0, minute=0, second=0, microsecond=0)

        # 获取或创建 summary 记录
        summary = db.query(StatsSummary).first()
        if not summary:
            summary = StatsSummary(id=str(uuid.uuid4()), cutoff_date=cutoff_date)

        # 从 stats_daily 聚合历史数据
        daily_aggregated = (
            db.query(
                func.sum(StatsDaily.total_requests).label("total_requests"),
                func.sum(StatsDaily.success_requests).label("success_requests"),
                func.sum(StatsDaily.error_requests).label("error_requests"),
                func.sum(StatsDaily.input_tokens).label("input_tokens"),
                func.sum(StatsDaily.output_tokens).label("output_tokens"),
                func.sum(StatsDaily.cache_creation_tokens).label("cache_creation_tokens"),
                func.sum(StatsDaily.cache_read_tokens).label("cache_read_tokens"),
                func.sum(StatsDaily.total_cost).label("total_cost"),
                func.sum(StatsDaily.actual_total_cost).label("actual_total_cost"),
            )
            .filter(StatsDaily.date < cutoff_date)
            .first()
        )

        # 用户/API Key 统计
        total_users = db.query(func.count(DBUser.id)).scalar() or 0
        active_users = (
            db.query(func.count(DBUser.id)).filter(DBUser.is_active.is_(True)).scalar() or 0
        )
        total_api_keys = db.query(func.count(ApiKey.id)).scalar() or 0
        active_api_keys = (
            db.query(func.count(ApiKey.id)).filter(ApiKey.is_active.is_(True)).scalar() or 0
        )

        # 更新 summary
        summary.cutoff_date = cutoff_date
        summary.all_time_requests = int(daily_aggregated.total_requests or 0)
        summary.all_time_success_requests = int(daily_aggregated.success_requests or 0)
        summary.all_time_error_requests = int(daily_aggregated.error_requests or 0)
        summary.all_time_input_tokens = int(daily_aggregated.input_tokens or 0)
        summary.all_time_output_tokens = int(daily_aggregated.output_tokens or 0)
        summary.all_time_cache_creation_tokens = int(daily_aggregated.cache_creation_tokens or 0)
        summary.all_time_cache_read_tokens = int(daily_aggregated.cache_read_tokens or 0)
        summary.all_time_cost = Decimal(str(daily_aggregated.total_cost or 0))
        summary.all_time_actual_cost = Decimal(str(daily_aggregated.actual_total_cost or 0))
        summary.total_users = total_users
        summary.active_users = active_users
        summary.total_api_keys = total_api_keys
        summary.active_api_keys = active_api_keys

        db.add(summary)
        db.commit()

        logger.info(f"[StatsAggregator] 更新全局汇总完成，截止日期: {cutoff_date.date()}")
        return summary

    @staticmethod
    def get_today_realtime_stats(db: Session) -> dict:
        """获取今日实时统计（用于与预聚合数据合并）"""
        # 使用 UTC 今天的开始时间
        now_utc = datetime.now(timezone.utc)
        today_utc = now_utc.replace(hour=0, minute=0, second=0, microsecond=0)

        error_cond = (Usage.status_code >= 400) | (Usage.error_message.isnot(None))
        aggregated = (
            db.query(
                func.count(Usage.id).label("total_requests"),
                func.sum(case((error_cond, 1), else_=0)).label("error_requests"),
                func.sum(Usage.input_tokens).label("input_tokens"),
                func.sum(Usage.output_tokens).label("output_tokens"),
                func.sum(Usage.cache_creation_input_tokens).label("cache_creation_tokens"),
                func.sum(Usage.cache_read_input_tokens).label("cache_read_tokens"),
                func.sum(Usage.total_cost_usd).label("total_cost"),
                func.sum(Usage.actual_total_cost_usd).label("actual_total_cost"),
                func.avg(Usage.response_time_ms).label("avg_response_time"),
                func.count(func.distinct(Usage.model)).label("unique_models"),
                func.count(func.distinct(Usage.provider_name)).label("unique_providers"),
            )
            .filter(Usage.created_at >= today_utc)
            .first()
        )

        total_requests = int(getattr(aggregated, "total_requests", 0) or 0)
        if total_requests == 0:
            return {
                "total_requests": 0,
                "success_requests": 0,
                "error_requests": 0,
                "input_tokens": 0,
                "output_tokens": 0,
                "cache_creation_tokens": 0,
                "cache_read_tokens": 0,
                "total_cost": Decimal(0),
                "actual_total_cost": Decimal(0),
                "avg_response_time_ms": 0.0,
                "unique_models": 0,
                "unique_providers": 0,
            }

        error_requests = int(getattr(aggregated, "error_requests", 0) or 0)

        return {
            "total_requests": total_requests,
            "success_requests": total_requests - error_requests,
            "error_requests": error_requests,
            "input_tokens": int(getattr(aggregated, "input_tokens", 0) or 0),
            "output_tokens": int(getattr(aggregated, "output_tokens", 0) or 0),
            "cache_creation_tokens": int(getattr(aggregated, "cache_creation_tokens", 0) or 0),
            "cache_read_tokens": int(getattr(aggregated, "cache_read_tokens", 0) or 0),
            "total_cost": Decimal(str(getattr(aggregated, "total_cost", 0) or 0)),
            "actual_total_cost": Decimal(str(getattr(aggregated, "actual_total_cost", 0) or 0)),
            "avg_response_time_ms": float(getattr(aggregated, "avg_response_time", 0) or 0.0),
            "unique_models": int(getattr(aggregated, "unique_models", 0) or 0),
            "unique_providers": int(getattr(aggregated, "unique_providers", 0) or 0),
        }

    @staticmethod
    def get_combined_stats(db: Session, today_stats: dict | None = None) -> dict:
        """获取合并后的统计数据（预聚合 + 今日实时）"""
        summary = db.query(StatsSummary).first()
        today_stats = today_stats or StatsAggregatorService.get_today_realtime_stats(db)

        if not summary:
            # 如果没有预聚合数据，返回今日数据
            return today_stats

        return {
            "total_requests": summary.all_time_requests + today_stats["total_requests"],
            "success_requests": summary.all_time_success_requests + today_stats["success_requests"],
            "error_requests": summary.all_time_error_requests + today_stats["error_requests"],
            "input_tokens": summary.all_time_input_tokens + today_stats["input_tokens"],
            "output_tokens": summary.all_time_output_tokens + today_stats["output_tokens"],
            "cache_creation_tokens": summary.all_time_cache_creation_tokens
            + today_stats["cache_creation_tokens"],
            "cache_read_tokens": summary.all_time_cache_read_tokens
            + today_stats["cache_read_tokens"],
            "total_cost": summary.all_time_cost + today_stats["total_cost"],
            "actual_total_cost": summary.all_time_actual_cost + today_stats["actual_total_cost"],
            "total_users": summary.total_users,
            "active_users": summary.active_users,
            "total_api_keys": summary.total_api_keys,
            "active_api_keys": summary.active_api_keys,
        }

    @staticmethod
    def backfill_historical_data(db: Session, days: int = 365) -> int:
        """回填历史数据（首次部署时使用）

        Args:
            db: 数据库会话
            days: 要回填的天数

        Returns:
            回填的天数
        """
        now_utc = datetime.now(timezone.utc)
        today_utc = now_utc.replace(hour=0, minute=0, second=0, microsecond=0)

        # 找到最早的 Usage 记录
        earliest = db.query(func.min(Usage.created_at)).scalar()
        if not earliest:
            logger.info("[StatsAggregator] 没有历史数据需要回填")
            return 0

        # 将最早记录时间转换为 UTC 日期
        earliest_utc = earliest.astimezone(timezone.utc).replace(
            hour=0, minute=0, second=0, microsecond=0
        )
        start_date = max(earliest_utc, today_utc - timedelta(days=days))

        user_ids = [
            user_id for (user_id,) in db.query(DBUser.id).filter(DBUser.is_active.is_(True)).all()
        ]
        count = 0
        current_date = start_date
        while current_date < today_utc:
            StatsAggregatorService.aggregate_daily_stats_bundle(db, current_date, user_ids=user_ids)
            db.expunge_all()  # 释放 Session identity map，防止 ORM 对象累积导致内存暴涨
            count += 1
            current_date += timedelta(days=1)

        # 更新汇总
        if count > 0:
            StatsAggregatorService.update_summary(db)

        logger.info(f"[StatsAggregator] 回填历史数据完成，共 {count} 天")
        return count

    @staticmethod
    def backfill_api_key_stats(db: Session, start_date: date, end_date: date) -> int:
        """回填 API Key 每日统计"""
        current = datetime.combine(start_date, time.min, tzinfo=timezone.utc)
        end_dt = datetime.combine(end_date, time.min, tzinfo=timezone.utc)
        count = 0
        while current <= end_dt:
            StatsAggregatorService.aggregate_daily_api_key_stats(db, current, commit=True)
            db.expunge_all()
            count += 1
            current += timedelta(days=1)
        return count

    @staticmethod
    def backfill_percentiles(
        db: Session, start_date: date, end_date: date, skip_existing: bool = True
    ) -> int:
        """回填历史百分位数据"""
        current = start_date
        processed = 0
        while current <= end_date:
            day_start = datetime.combine(current, time.min, tzinfo=timezone.utc)
            if skip_existing:
                existing = (
                    db.query(StatsDaily.p50_response_time_ms)
                    .filter(StatsDaily.date == day_start)
                    .scalar()
                )
                if existing is not None:
                    current += timedelta(days=1)
                    continue

            percentiles = StatsAggregatorService.compute_daily_percentiles(
                db, day_start, day_start + timedelta(days=1)
            )
            if any(value is not None for value in percentiles.values()):
                db.query(StatsDaily).filter(StatsDaily.date == day_start).update(percentiles)
                processed += 1
            if processed % 30 == 0:
                db.commit()
                db.expunge_all()
            current += timedelta(days=1)

        db.commit()
        db.expunge_all()
        return processed

    @staticmethod
    def backfill_error_categories(
        db: Session, batch_size: int = 1000, resume_from_id: str | None = None
    ) -> int:
        """回填 Usage.error_category 字段"""
        from src.services.usage.error_classifier import classify_error

        last_id = resume_from_id
        total_processed = 0

        while True:
            query = (
                db.query(Usage)
                .filter(Usage.status == "failed")
                .filter(Usage.error_category.is_(None))
                .order_by(Usage.id.asc())
                .limit(batch_size)
            )
            if last_id is not None:
                query = query.filter(Usage.id > last_id)

            records = query.all()
            if not records:
                break

            for record in records:
                record.error_category = classify_error(
                    record.status_code, record.error_message, record.status
                ).value

            db.commit()
            last_id = records[-1].id
            total_processed += len(records)
            db.expunge_all()

        return total_processed

    @staticmethod
    def backfill_error_stats(db: Session, start_date: date, end_date: date) -> int:
        """回填每日错误统计"""
        current = datetime.combine(start_date, time.min, tzinfo=timezone.utc)
        end_dt = datetime.combine(end_date, time.min, tzinfo=timezone.utc)
        count = 0
        while current <= end_dt:
            StatsAggregatorService.aggregate_daily_error_stats(db, current, commit=True)
            db.expunge_all()
            count += 1
            current += timedelta(days=1)
        return count


@dataclass
class AggregatedStats:
    total_requests: int = 0
    success_requests: int = 0
    error_requests: int = 0
    input_tokens: int = 0
    output_tokens: int = 0
    cache_creation_tokens: int = 0
    cache_read_tokens: int = 0
    cache_creation_cost: float = 0.0
    cache_read_cost: float = 0.0
    total_cost: float = 0.0
    actual_total_cost: float = 0.0
    total_response_time_ms: float = 0.0

    def add(self, other: "AggregatedStats") -> "AggregatedStats":
        self.total_requests += other.total_requests
        self.success_requests += other.success_requests
        self.error_requests += other.error_requests
        self.input_tokens += other.input_tokens
        self.output_tokens += other.output_tokens
        self.cache_creation_tokens += other.cache_creation_tokens
        self.cache_read_tokens += other.cache_read_tokens
        self.cache_creation_cost += other.cache_creation_cost
        self.cache_read_cost += other.cache_read_cost
        self.total_cost += other.total_cost
        self.actual_total_cost += other.actual_total_cost
        self.total_response_time_ms += other.total_response_time_ms
        return self

    @property
    def avg_response_time_ms(self) -> float:
        if self.total_requests <= 0:
            return 0.0
        return self.total_response_time_ms / self.total_requests


@dataclass
class StatsFilter:
    user_id: str | None = None
    model: str | None = None
    provider_name: str | None = None


def aggregate_usage_range(
    db: Session,
    start_utc: datetime,
    end_utc: datetime,
    filters: StatsFilter | None = None,
) -> AggregatedStats:
    """Aggregate usage within [start_utc, end_utc)."""
    filters = filters or StatsFilter()
    error_cond = (Usage.status_code >= 400) | (Usage.error_message.isnot(None))

    query = (
        db.query(
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
        )
        .filter(Usage.created_at >= start_utc)
        .filter(Usage.created_at < end_utc)
    )

    if filters.user_id:
        query = query.filter(Usage.user_id == filters.user_id)
    if filters.model:
        query = query.filter(Usage.model == filters.model)
    if filters.provider_name:
        query = query.filter(Usage.provider_name == filters.provider_name)

    row = query.first()
    total_requests = int(getattr(row, "total_requests", 0) or 0)
    error_requests = int(getattr(row, "error_requests", 0) or 0)
    return AggregatedStats(
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


def query_stats_hybrid(
    db: Session, params: TimeRangeParams, filters: StatsFilter | None = None
) -> AggregatedStats:
    """Hybrid stats query: StatsDaily + Usage (boundaries)."""
    if filters and (filters.user_id or filters.model or filters.provider_name):
        start_utc, end_utc = params.to_utc_datetime_range()
        return aggregate_usage_range(db, start_utc, end_utc, filters=filters)

    complete_dates, head_boundary, tail_boundary = params.get_complete_utc_dates()
    today_utc = datetime.now(timezone.utc).date()
    filters = filters or StatsFilter()

    result = AggregatedStats()

    preaggregate_dates: list[datetime] = []
    realtime_dates: list[datetime] = []
    for day in complete_dates:
        if day >= today_utc:
            realtime_dates.append(datetime.combine(day, time.min, tzinfo=timezone.utc))
            continue
        day_dt = datetime.combine(day, time.min, tzinfo=timezone.utc)
        stats = (
            db.query(StatsDaily)
            .filter(StatsDaily.date == day_dt, StatsDaily.is_complete.is_(True))
            .first()
        )
        if stats:
            preaggregate_dates.append(day_dt)
        else:
            realtime_dates.append(day_dt)

    # Pre-aggregated totals
    for day_dt in preaggregate_dates:
        stats = db.query(StatsDaily).filter(StatsDaily.date == day_dt).first()
        if not stats:
            continue
        result.total_requests += stats.total_requests
        result.success_requests += stats.success_requests
        result.error_requests += stats.error_requests
        result.input_tokens += stats.input_tokens
        result.output_tokens += stats.output_tokens
        result.cache_creation_tokens += stats.cache_creation_tokens
        result.cache_read_tokens += stats.cache_read_tokens
        result.cache_creation_cost += float(stats.cache_creation_cost or 0)
        result.cache_read_cost += float(stats.cache_read_cost or 0)
        result.total_cost += float(stats.total_cost or 0)
        result.actual_total_cost += float(stats.actual_total_cost or 0)
        result.total_response_time_ms += (stats.avg_response_time_ms or 0.0) * stats.total_requests

    # Realtime per day
    for day_dt in realtime_dates:
        result.add(aggregate_usage_range(db, day_dt, day_dt + timedelta(days=1), filters=filters))

    if head_boundary:
        result.add(aggregate_usage_range(db, head_boundary[0], head_boundary[1], filters=filters))
    if tail_boundary:
        result.add(aggregate_usage_range(db, tail_boundary[0], tail_boundary[1], filters=filters))

    return result


def get_completed_hours(
    db: Session, hours: list[datetime], current_hour_utc: datetime
) -> set[datetime]:
    """Return completed UTC hours from StatsHourly."""
    candidate_hours = [h for h in hours if h < current_hour_utc]
    if not candidate_hours:
        return set()

    completed = (
        db.query(StatsHourly.hour_utc)
        .filter(StatsHourly.hour_utc.in_(candidate_hours))
        .filter(StatsHourly.is_complete.is_(True))
        .all()
    )
    return {row[0] for row in completed}


@dataclass
class TimeSeriesFilter:
    user_id: str | None = None
    model: str | None = None
    provider_name: str | None = None


def query_time_series(
    db: Session, params: TimeRangeParams, filters: TimeSeriesFilter | None = None
) -> list[dict]:
    """Query time series data using StatsHourly + Usage fragments."""
    params.validate_for_time_series()
    filters = filters or TimeSeriesFilter()

    if params.granularity == "hour":
        return query_time_series_hourly(db, params, filters)

    def build_hourly_query(complete_hours: list[datetime]) -> Any | None:
        if not complete_hours:
            return None
        if filters.user_id:
            return (
                db.query(
                    func.sum(StatsHourlyUser.total_requests).label("total_requests"),
                    func.sum(StatsHourlyUser.input_tokens).label("input_tokens"),
                    func.sum(StatsHourlyUser.output_tokens).label("output_tokens"),
                    func.sum(StatsHourlyUser.total_cost).label("total_cost"),
                    cast(0.0, Float).label("total_response_time_ms"),
                    cast(0, Float).label("cache_creation_tokens"),
                    cast(0, Float).label("cache_read_tokens"),
                )
                .select_from(StatsHourlyUser)
                .join(StatsHourly, StatsHourlyUser.hour_utc == StatsHourly.hour_utc)
                .filter(StatsHourly.is_complete.is_(True))
                .filter(StatsHourlyUser.hour_utc.in_(complete_hours))
                .filter(StatsHourlyUser.user_id == filters.user_id)
            )
        if filters.model:
            return (
                db.query(
                    func.sum(StatsHourlyModel.total_requests).label("total_requests"),
                    func.sum(StatsHourlyModel.input_tokens).label("input_tokens"),
                    func.sum(StatsHourlyModel.output_tokens).label("output_tokens"),
                    func.sum(StatsHourlyModel.total_cost).label("total_cost"),
                    func.sum(
                        StatsHourlyModel.avg_response_time_ms * StatsHourlyModel.total_requests
                    ).label("total_response_time_ms"),
                    cast(0, Float).label("cache_creation_tokens"),
                    cast(0, Float).label("cache_read_tokens"),
                )
                .select_from(StatsHourlyModel)
                .join(StatsHourly, StatsHourlyModel.hour_utc == StatsHourly.hour_utc)
                .filter(StatsHourly.is_complete.is_(True))
                .filter(StatsHourlyModel.hour_utc.in_(complete_hours))
                .filter(StatsHourlyModel.model == filters.model)
            )
        if filters.provider_name:
            return (
                db.query(
                    func.sum(StatsHourlyProvider.total_requests).label("total_requests"),
                    func.sum(StatsHourlyProvider.input_tokens).label("input_tokens"),
                    func.sum(StatsHourlyProvider.output_tokens).label("output_tokens"),
                    func.sum(StatsHourlyProvider.total_cost).label("total_cost"),
                    cast(0.0, Float).label("total_response_time_ms"),
                    cast(0, Float).label("cache_creation_tokens"),
                    cast(0, Float).label("cache_read_tokens"),
                )
                .select_from(StatsHourlyProvider)
                .join(StatsHourly, StatsHourlyProvider.hour_utc == StatsHourly.hour_utc)
                .filter(StatsHourly.is_complete.is_(True))
                .filter(StatsHourlyProvider.hour_utc.in_(complete_hours))
                .filter(StatsHourlyProvider.provider_name == filters.provider_name)
            )
        return (
            db.query(
                func.sum(StatsHourly.total_requests).label("total_requests"),
                func.sum(StatsHourly.input_tokens).label("input_tokens"),
                func.sum(StatsHourly.output_tokens).label("output_tokens"),
                func.sum(StatsHourly.total_cost).label("total_cost"),
                func.sum(StatsHourly.avg_response_time_ms * StatsHourly.total_requests).label(
                    "total_response_time_ms"
                ),
                func.sum(StatsHourly.cache_creation_tokens).label("cache_creation_tokens"),
                func.sum(StatsHourly.cache_read_tokens).label("cache_read_tokens"),
            )
            .filter(StatsHourly.hour_utc.in_(complete_hours))
            .filter(StatsHourly.is_complete.is_(True))
        )

    local_days = params.get_local_day_hours()
    result: list[dict] = []
    current_hour_utc = datetime.now(timezone.utc).replace(minute=0, second=0, microsecond=0)

    for local_date, day_start_utc, day_end_utc in local_days:
        head_fragment, complete_hours, tail_fragment = split_time_range_for_hourly(
            day_start_utc, day_end_utc
        )

        total_requests = 0
        input_tokens = 0
        output_tokens = 0
        total_cost = 0.0
        total_response_time_ms = 0.0
        cache_creation_tokens = 0
        cache_read_tokens = 0

        if head_fragment:
            head = aggregate_usage_range(
                db, head_fragment[0], head_fragment[1], StatsFilter(**filters.__dict__)
            )
            total_requests += head.total_requests
            input_tokens += head.input_tokens
            output_tokens += head.output_tokens
            total_cost += head.total_cost
            total_response_time_ms += head.total_response_time_ms
            cache_creation_tokens += head.cache_creation_tokens
            cache_read_tokens += head.cache_read_tokens

        if complete_hours:
            completed_hours = get_completed_hours(db, complete_hours, current_hour_utc)
            if completed_hours:
                hourly_query = build_hourly_query(list(completed_hours))
                if hourly_query is not None:
                    row = hourly_query.first()
                    total_requests += int(getattr(row, "total_requests", 0) or 0)
                    input_tokens += int(getattr(row, "input_tokens", 0) or 0)
                    output_tokens += int(getattr(row, "output_tokens", 0) or 0)
                    total_cost += float(getattr(row, "total_cost", 0) or 0.0)
                    total_response_time_ms += float(
                        getattr(row, "total_response_time_ms", 0) or 0.0
                    )
                    cache_creation_tokens += int(getattr(row, "cache_creation_tokens", 0) or 0)
                    cache_read_tokens += int(getattr(row, "cache_read_tokens", 0) or 0)

            incomplete_hours = [
                h for h in complete_hours if h not in completed_hours or h >= current_hour_utc
            ]
            for hour in incomplete_hours:
                hour_stats = aggregate_usage_range(
                    db, hour, hour + timedelta(hours=1), StatsFilter(**filters.__dict__)
                )
                total_requests += hour_stats.total_requests
                input_tokens += hour_stats.input_tokens
                output_tokens += hour_stats.output_tokens
                total_cost += hour_stats.total_cost
                total_response_time_ms += hour_stats.total_response_time_ms
                cache_creation_tokens += hour_stats.cache_creation_tokens
                cache_read_tokens += hour_stats.cache_read_tokens

        if tail_fragment:
            tail = aggregate_usage_range(
                db, tail_fragment[0], tail_fragment[1], StatsFilter(**filters.__dict__)
            )
            total_requests += tail.total_requests
            input_tokens += tail.input_tokens
            output_tokens += tail.output_tokens
            total_cost += tail.total_cost
            total_response_time_ms += tail.total_response_time_ms
            cache_creation_tokens += tail.cache_creation_tokens
            cache_read_tokens += tail.cache_read_tokens

        result.append(
            {
                "date": local_date.isoformat(),
                "total_requests": total_requests,
                "input_tokens": input_tokens,
                "output_tokens": output_tokens,
                "cache_creation_tokens": cache_creation_tokens,
                "cache_read_tokens": cache_read_tokens,
                "total_cost": float(total_cost),
                "avg_response_time_ms": (
                    total_response_time_ms / total_requests if total_requests > 0 else 0.0
                ),
            }
        )

    if params.granularity == "week":
        result = aggregate_by_week(result)
    elif params.granularity == "month":
        result = aggregate_by_month(result)

    return result


def query_time_series_hourly(
    db: Session, params: TimeRangeParams, filters: TimeSeriesFilter
) -> list[dict]:
    """Hourly time series (single local day)."""
    params.validate_for_time_series()
    local_day = params.get_local_day_hours()[0]
    _, day_start_utc, day_end_utc = local_day
    current_hour_utc = datetime.now(timezone.utc).replace(minute=0, second=0, microsecond=0)

    def hour_label(utc_dt: datetime) -> str:
        if params.timezone:
            try:
                from zoneinfo import ZoneInfo

                return (
                    utc_dt.replace(tzinfo=timezone.utc)
                    .astimezone(ZoneInfo(params.timezone))
                    .isoformat()
                )
            except Exception:
                pass
        return (utc_dt + timedelta(minutes=params.tz_offset_minutes)).isoformat()

    result: list[dict] = []
    current = day_start_utc
    while current < day_end_utc:
        next_hour = min(current + timedelta(hours=1), day_end_utc)
        is_full_hour = (
            current.minute == 0
            and current.second == 0
            and current.microsecond == 0
            and next_hour == current + timedelta(hours=1)
        )

        total_requests = 0
        input_tokens = 0
        output_tokens = 0
        total_cost = 0.0
        cache_creation_tokens = 0
        cache_read_tokens = 0

        if is_full_hour and current < current_hour_utc:
            completed = get_completed_hours(db, [current], current_hour_utc)
            if completed:
                row = None
                if filters.user_id:
                    row = (
                        db.query(
                            StatsHourlyUser.total_requests,
                            StatsHourlyUser.input_tokens,
                            StatsHourlyUser.output_tokens,
                            StatsHourlyUser.total_cost,
                        )
                        .join(StatsHourly, StatsHourlyUser.hour_utc == StatsHourly.hour_utc)
                        .filter(StatsHourly.is_complete.is_(True))
                        .filter(StatsHourlyUser.hour_utc == current)
                        .filter(StatsHourlyUser.user_id == filters.user_id)
                        .first()
                    )
                elif filters.model:
                    row = (
                        db.query(
                            StatsHourlyModel.total_requests,
                            StatsHourlyModel.input_tokens,
                            StatsHourlyModel.output_tokens,
                            StatsHourlyModel.total_cost,
                        )
                        .join(StatsHourly, StatsHourlyModel.hour_utc == StatsHourly.hour_utc)
                        .filter(StatsHourly.is_complete.is_(True))
                        .filter(StatsHourlyModel.hour_utc == current)
                        .filter(StatsHourlyModel.model == filters.model)
                        .first()
                    )
                elif filters.provider_name:
                    row = (
                        db.query(
                            StatsHourlyProvider.total_requests,
                            StatsHourlyProvider.input_tokens,
                            StatsHourlyProvider.output_tokens,
                            StatsHourlyProvider.total_cost,
                        )
                        .join(StatsHourly, StatsHourlyProvider.hour_utc == StatsHourly.hour_utc)
                        .filter(StatsHourly.is_complete.is_(True))
                        .filter(StatsHourlyProvider.hour_utc == current)
                        .filter(StatsHourlyProvider.provider_name == filters.provider_name)
                        .first()
                    )
                else:
                    row = (
                        db.query(
                            StatsHourly.total_requests,
                            StatsHourly.input_tokens,
                            StatsHourly.output_tokens,
                            StatsHourly.total_cost,
                            StatsHourly.cache_creation_tokens,
                            StatsHourly.cache_read_tokens,
                        )
                        .filter(StatsHourly.hour_utc == current)
                        .filter(StatsHourly.is_complete.is_(True))
                        .first()
                    )

                if row:
                    total_requests = int(row[0] or 0)
                    input_tokens = int(row[1] or 0)
                    output_tokens = int(row[2] or 0)
                    total_cost = float(row[3] or 0)
                    if not (filters.user_id or filters.model or filters.provider_name):
                        cache_creation_tokens = int(row[4] or 0)
                        cache_read_tokens = int(row[5] or 0)

        if total_requests == 0 and input_tokens == 0 and output_tokens == 0 and total_cost == 0.0:
            usage_stats = aggregate_usage_range(
                db, current, next_hour, StatsFilter(**filters.__dict__)
            )
            total_requests = usage_stats.total_requests
            input_tokens = usage_stats.input_tokens
            output_tokens = usage_stats.output_tokens
            total_cost = usage_stats.total_cost
            cache_creation_tokens = usage_stats.cache_creation_tokens
            cache_read_tokens = usage_stats.cache_read_tokens

        result.append(
            {
                "date": hour_label(current),
                "total_requests": total_requests,
                "input_tokens": input_tokens,
                "output_tokens": output_tokens,
                "cache_creation_tokens": cache_creation_tokens,
                "cache_read_tokens": cache_read_tokens,
                "total_cost": float(total_cost),
            }
        )
        current = next_hour

    return result


def aggregate_by_week(items: list[dict]) -> list[dict]:
    """Aggregate daily series into week buckets (ISO week)."""
    buckets: dict[tuple[int, int], dict] = {}
    for item in items:
        day = datetime.fromisoformat(item["date"]).date()
        key = (day.isocalendar().year, day.isocalendar().week)
        bucket = buckets.setdefault(
            key,
            {
                "date": day.isoformat(),
                "total_requests": 0,
                "input_tokens": 0,
                "output_tokens": 0,
                "cache_creation_tokens": 0,
                "cache_read_tokens": 0,
                "total_cost": 0.0,
                "total_response_time_ms": 0.0,
            },
        )
        bucket["total_requests"] += item["total_requests"]
        bucket["input_tokens"] += item["input_tokens"]
        bucket["output_tokens"] += item["output_tokens"]
        bucket["cache_creation_tokens"] += item.get("cache_creation_tokens", 0)
        bucket["cache_read_tokens"] += item.get("cache_read_tokens", 0)
        bucket["total_cost"] += item["total_cost"]
        bucket["total_response_time_ms"] += (
            item.get("avg_response_time_ms", 0.0) * item["total_requests"]
        )
    result = []
    for bucket in buckets.values():
        avg_ms = (
            bucket["total_response_time_ms"] / bucket["total_requests"]
            if bucket["total_requests"] > 0
            else 0.0
        )
        bucket["avg_response_time_ms"] = avg_ms
        bucket.pop("total_response_time_ms", None)
        result.append(bucket)
    return result


def aggregate_by_month(items: list[dict]) -> list[dict]:
    """Aggregate daily series into month buckets."""
    buckets: dict[tuple[int, int], dict] = {}
    for item in items:
        day = datetime.fromisoformat(item["date"]).date()
        key = (day.year, day.month)
        bucket = buckets.setdefault(
            key,
            {
                "date": f"{day.year:04d}-{day.month:02d}-01",
                "total_requests": 0,
                "input_tokens": 0,
                "output_tokens": 0,
                "cache_creation_tokens": 0,
                "cache_read_tokens": 0,
                "total_cost": 0.0,
                "total_response_time_ms": 0.0,
            },
        )
        bucket["total_requests"] += item["total_requests"]
        bucket["input_tokens"] += item["input_tokens"]
        bucket["output_tokens"] += item["output_tokens"]
        bucket["cache_creation_tokens"] += item.get("cache_creation_tokens", 0)
        bucket["cache_read_tokens"] += item.get("cache_read_tokens", 0)
        bucket["total_cost"] += item["total_cost"]
        bucket["total_response_time_ms"] += (
            item.get("avg_response_time_ms", 0.0) * item["total_requests"]
        )
    result = []
    for bucket in buckets.values():
        avg_ms = (
            bucket["total_response_time_ms"] / bucket["total_requests"]
            if bucket["total_requests"] > 0
            else 0.0
        )
        bucket["avg_response_time_ms"] = avg_ms
        bucket.pop("total_response_time_ms", None)
        result.append(bucket)
    return result
