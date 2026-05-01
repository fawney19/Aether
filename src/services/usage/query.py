from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from typing import TYPE_CHECKING, Any

from sqlalchemy import case, func
from sqlalchemy.orm import Session

if TYPE_CHECKING:
    from sqlalchemy.sql.elements import ColumnElement

from src.core.logger import logger
from src.models.database import ApiKey, Usage, User


def input_context_expr() -> ColumnElement[int]:
    """计算缓存命中率口径下的总输入上下文 token 数。

    为了与 usage 表中“输入 tokens + 缓存读取 tokens”的展示口径保持一致，
    聚合统计统一使用 `input_tokens + cache_read_input_tokens` 作为分母。
    """
    return Usage.input_tokens + Usage.cache_read_input_tokens


@dataclass(slots=True)
class RequestBalanceCheckResult:
    allowed: bool
    message: str
    remaining: float | None


class UsageQueryMixin:
    """查询/统计相关方法"""

    # 热力图缓存键前缀（依赖 TTL 自动过期，用户角色变更时主动清除）
    HEATMAP_CACHE_KEY_PREFIX = "activity_heatmap"

    @classmethod
    def _get_heatmap_cache_key(
        cls,
        user_id: str | None,
        include_actual_cost: bool,
        api_key_id: str | None = None,
        deleted_user_only: bool = False,
        deleted_api_key_only: bool = False,
    ) -> str:
        """生成热力图缓存键"""
        cost_suffix = "with_cost" if include_actual_cost else "no_cost"
        scope_tokens: list[str] = []
        if deleted_user_only:
            scope_tokens.append("deleted_user")
        elif user_id:
            scope_tokens.extend(["user", user_id])
        else:
            scope_tokens.extend(["admin", "all"])

        if deleted_api_key_only:
            scope_tokens.append("deleted_api_key")
        elif api_key_id:
            scope_tokens.extend(["api_key", api_key_id])

        scope_key = ":".join(scope_tokens)
        return f"{cls.HEATMAP_CACHE_KEY_PREFIX}:{scope_key}:{cost_suffix}"

    @classmethod
    async def clear_user_heatmap_cache(cls, user_id: str) -> None:
        """
        清除用户的热力图缓存（用户角色变更时调用）

        Args:
            user_id: 用户ID
        """
        from src.clients.redis_client import get_redis_client

        redis_client = await get_redis_client(require_redis=False)
        if not redis_client:
            return

        # 清除该用户的所有热力图缓存（with_cost 和 no_cost）
        keys_to_delete = [
            cls._get_heatmap_cache_key(user_id, include_actual_cost=True),
            cls._get_heatmap_cache_key(user_id, include_actual_cost=False),
        ]

        for key in keys_to_delete:
            try:
                await redis_client.delete(key)
                logger.debug("已清除热力图缓存: {}", key)
            except Exception as e:
                logger.warning("清除热力图缓存失败: {}, error={}", key, e)

    @classmethod
    async def get_cached_heatmap(
        cls,
        db: Session,
        user_id: str | None = None,
        api_key_id: str | None = None,
        deleted_user_only: bool = False,
        deleted_api_key_only: bool = False,
        include_actual_cost: bool = False,
    ) -> dict[str, Any]:
        """
        获取带缓存的热力图数据

        缓存策略：
        - TTL: 10分钟（CacheTTL.ACTIVITY_HEATMAP = 600）
        - 仅依赖 TTL 自动过期，新使用记录最多延迟 10 分钟出现
        - 用户角色变更时通过 clear_user_heatmap_cache() 主动清除

        Args:
            db: 数据库会话
            user_id: 用户ID，None 表示获取全局热力图（管理员）
            include_actual_cost: 是否包含实际成本

        Returns:
            热力图数据字典
        """
        import json

        from src.clients.redis_client import get_redis_client
        from src.config.constants import CacheTTL

        cache_key = cls._get_heatmap_cache_key(
            user_id,
            include_actual_cost,
            api_key_id=api_key_id,
            deleted_user_only=deleted_user_only,
            deleted_api_key_only=deleted_api_key_only,
        )

        cache_ttl = CacheTTL.ACTIVITY_HEATMAP
        redis_client = await get_redis_client(require_redis=False)

        # 尝试从缓存获取
        if redis_client:
            try:
                cached = await redis_client.get(cache_key)
                if cached:
                    try:
                        return json.loads(cached)  # type: ignore[no-any-return]
                    except json.JSONDecodeError as e:
                        logger.warning(
                            "热力图缓存解析失败，删除损坏缓存: {}, error={}", cache_key, e
                        )
                        try:
                            await redis_client.delete(cache_key)
                        except Exception:
                            pass
            except Exception as e:
                logger.error("读取热力图缓存出错: {}, error={}", cache_key, e)

        # 从数据库查询
        result = cls.get_daily_activity(
            db=db,
            user_id=user_id,
            api_key_id=api_key_id,
            deleted_user_only=deleted_user_only,
            deleted_api_key_only=deleted_api_key_only,
            window_days=365,
            include_actual_cost=include_actual_cost,
        )

        # 保存到缓存（失败不影响返回结果）
        if redis_client:
            try:
                await redis_client.setex(
                    cache_key,
                    cache_ttl,
                    json.dumps(result, ensure_ascii=False, default=str),
                )
            except Exception as e:
                logger.warning("保存热力图缓存失败: {}, error={}", cache_key, e)

        return result

    @staticmethod
    def check_request_balance_details(
        db: Session,
        user: User,
        estimated_tokens: int = 0,
        estimated_cost: float = 0,
        api_key: ApiKey | None = None,
    ) -> RequestBalanceCheckResult:
        """Return a structured balance-check result."""
        from src.services.wallet import WalletService

        wallet_access = WalletService.check_request_allowed(
            db,
            user=None if (api_key and api_key.is_standalone) else user,
            api_key=api_key,
        )
        snapshot = wallet_access.balance_snapshot
        if snapshot is None:
            snapshot = wallet_access.remaining
        remaining = float(snapshot) if snapshot is not None else None
        if wallet_access.allowed:
            return RequestBalanceCheckResult(True, "OK", remaining)

        if wallet_access.message in {"钱包欠费，请先充值", "账户欠费，请先充值"}:
            if api_key and api_key.is_standalone:
                return RequestBalanceCheckResult(False, "Key欠费，请先调账或充值", remaining)
            return RequestBalanceCheckResult(False, "账户欠费，请先充值", remaining)

        if wallet_access.message.startswith("订阅额度已用尽"):
            return RequestBalanceCheckResult(False, wallet_access.message, remaining)

        if wallet_access.message == "钱包不可用":
            if api_key and api_key.is_standalone:
                return RequestBalanceCheckResult(False, "Key钱包不可用", remaining)
            return RequestBalanceCheckResult(False, "钱包不可用", remaining)

        if api_key and api_key.is_standalone:
            if remaining is None:
                return RequestBalanceCheckResult(False, "Key余额不足", remaining)
            return RequestBalanceCheckResult(
                False, f"Key余额不足（剩余: ${remaining:.2f}）", remaining
            )

        # Admin users are already allowed in WalletService.check_request_allowed.
        if remaining is None:
            return RequestBalanceCheckResult(False, wallet_access.message or "余额不足", remaining)
        return RequestBalanceCheckResult(False, f"余额不足（剩余: ${remaining:.2f}）", remaining)

    @staticmethod
    def check_request_balance(
        db: Session,
        user: User,
        estimated_tokens: int = 0,
        estimated_cost: float = 0,
        api_key: ApiKey | None = None,
    ) -> tuple[bool, str]:
        """Check whether the request passes balance rules."""
        result = UsageQueryMixin.check_request_balance_details(
            db,
            user,
            estimated_tokens=estimated_tokens,
            estimated_cost=estimated_cost,
            api_key=api_key,
        )
        return result.allowed, result.message

    @staticmethod
    def get_usage_summary(
        db: Session,
        user_id: str | None = None,
        api_key_id: str | None = None,
        start_date: datetime | None = None,
        end_date: datetime | None = None,
        group_by: str | None = "day",  # day, week, month, None(不按时间分桶)
    ) -> list[dict[str, Any]]:
        """获取使用汇总"""

        query = db.query(Usage)
        # 过滤掉 pending/streaming 状态的请求（尚未完成的请求不应计入统计）
        query = query.filter(Usage.status.notin_(["pending", "streaming"]))

        if user_id:
            query = query.filter(Usage.user_id == user_id)
        if api_key_id:
            query = query.filter(Usage.api_key_id == api_key_id)
        if start_date:
            query = query.filter(Usage.created_at >= start_date)
        if end_date:
            query = query.filter(Usage.created_at < end_date)

        select_columns = [Usage.provider_name, Usage.model]
        group_columns = [Usage.provider_name, Usage.model]

        if group_by is not None:
            from src.utils.database_helpers import date_trunc_portable

            bind = db.bind
            dialect = bind.dialect.name if bind is not None else "sqlite"

            if group_by == "day":
                date_func = date_trunc_portable(dialect, "day", Usage.created_at)
            elif group_by == "week":
                date_func = date_trunc_portable(dialect, "week", Usage.created_at)
            elif group_by == "month":
                date_func = date_trunc_portable(dialect, "month", Usage.created_at)
            else:
                date_func = date_trunc_portable(dialect, "day", Usage.created_at)
            select_columns.insert(0, date_func.label("period"))
            group_columns.insert(0, date_func)

        summary = db.query(
            *select_columns,
            func.count(Usage.id).label("requests"),
            func.sum(Usage.input_tokens).label("input_tokens"),
            func.sum(Usage.output_tokens).label("output_tokens"),
            func.sum(Usage.total_tokens).label("total_tokens"),
            func.sum(Usage.cache_read_input_tokens).label("cache_read_tokens"),
            func.sum(Usage.cache_creation_input_tokens).label("cache_creation_tokens"),
            func.sum(input_context_expr()).label("total_input_context"),
            func.sum(Usage.total_cost_usd).label("total_cost_usd"),
            func.sum(Usage.actual_total_cost_usd).label("actual_total_cost_usd"),
            func.sum(case((Usage.status_code == 200, 1), else_=0)).label("success_count"),
            func.avg(Usage.response_time_ms).label("avg_response_time"),
            func.sum(
                case(
                    (
                        (Usage.status_code == 200) & Usage.response_time_ms.isnot(None),
                        Usage.response_time_ms,
                    ),
                    else_=0,
                )
            ).label("success_response_time_sum"),
            func.sum(
                case(
                    ((Usage.status_code == 200) & Usage.response_time_ms.isnot(None), 1),
                    else_=0,
                )
            ).label("success_response_time_count"),
        )

        # 过滤掉 pending/streaming 状态的请求（与上方明细查询一致）
        summary = summary.filter(Usage.status.notin_(["pending", "streaming"]))

        if user_id:
            summary = summary.filter(Usage.user_id == user_id)
        if api_key_id:
            summary = summary.filter(Usage.api_key_id == api_key_id)
        if start_date:
            summary = summary.filter(Usage.created_at >= start_date)
        if end_date:
            summary = summary.filter(Usage.created_at < end_date)

        summary = summary.group_by(*group_columns).all()

        return [
            {
                "period": getattr(row, "period", None),
                "provider": row.provider_name,
                "model": row.model,
                "requests": row.requests,
                "input_tokens": row.input_tokens,
                "output_tokens": row.output_tokens,
                "total_tokens": row.total_tokens,
                "cache_read_tokens": int(row.cache_read_tokens or 0),
                "cache_creation_tokens": int(row.cache_creation_tokens or 0),
                "total_input_context": int(row.total_input_context or 0),
                "total_cost_usd": float(row.total_cost_usd or 0.0),
                "actual_total_cost_usd": float(row.actual_total_cost_usd or 0.0),
                "success_count": int(row.success_count or 0),
                "avg_response_time_ms": (
                    float(row.avg_response_time) if row.avg_response_time else 0
                ),
                "success_response_time_sum_ms": float(row.success_response_time_sum or 0.0),
                "success_response_time_count": int(row.success_response_time_count or 0),
            }
            for row in summary
        ]

    @staticmethod
    def get_daily_activity(
        db: Session,
        user_id: str | None = None,
        api_key_id: str | None = None,
        start_date: datetime | None = None,
        end_date: datetime | None = None,
        window_days: int = 365,
        deleted_user_only: bool = False,
        deleted_api_key_only: bool = False,
        include_actual_cost: bool = False,
    ) -> dict[str, Any]:
        """按天统计请求活跃度，用于渲染热力图。

        优化策略：
        - 历史数据从预计算的 StatsDaily/StatsUserDaily 表读取
        - 只有"今天"的数据才实时查询 Usage 表
        """

        def ensure_timezone(value: datetime) -> datetime:
            if value.tzinfo is None:
                return value.replace(tzinfo=timezone.utc)
            return value.astimezone(timezone.utc)

        # 如果调用方未指定时间范围，则默认统计最近 window_days 天
        now = datetime.now(timezone.utc)
        end_dt = ensure_timezone(end_date) if end_date else now
        start_dt = (
            ensure_timezone(start_date) if start_date else end_dt - timedelta(days=window_days - 1)
        )

        # 对齐到自然日的开始/结束
        start_dt = datetime.combine(start_dt.date(), datetime.min.time(), tzinfo=timezone.utc)
        end_dt = datetime.combine(end_dt.date(), datetime.max.time(), tzinfo=timezone.utc)

        today = now.date()
        today_start_dt = datetime.combine(today, datetime.min.time(), tzinfo=timezone.utc)
        aggregated: dict[str, dict[str, Any]] = {}

        needs_usage_scan = api_key_id is not None or deleted_user_only or deleted_api_key_only

        # 1. 对带删除态/API Key 过滤的场景直接按 Usage 实时聚合，避免预计算表维度不完整。
        if needs_usage_scan:
            usage_query = db.query(
                Usage.created_at,
                Usage.total_tokens,
                Usage.total_cost_usd,
                Usage.actual_total_cost_usd,
            ).filter(
                Usage.created_at >= start_dt,
                Usage.created_at <= end_dt,
            )

            if deleted_user_only:
                usage_query = usage_query.filter(Usage.user_id.is_(None))
            elif user_id:
                usage_query = usage_query.filter(Usage.user_id == user_id)

            if deleted_api_key_only:
                usage_query = usage_query.filter(Usage.api_key_id.is_(None))
            elif api_key_id:
                usage_query = usage_query.filter(Usage.api_key_id == api_key_id)

            for row in usage_query.all():
                created_at = getattr(row, "created_at", None)
                if not created_at:
                    continue
                key = ensure_timezone(created_at).date().isoformat()
                bucket = aggregated.setdefault(
                    key,
                    {
                        "requests": 0,
                        "total_tokens": 0,
                        "total_cost_usd": 0.0,
                        "actual_total_cost_usd": 0.0,
                    },
                )
                bucket["requests"] += 1
                bucket["total_tokens"] += int(getattr(row, "total_tokens", 0) or 0)
                bucket["total_cost_usd"] += float(getattr(row, "total_cost_usd", 0.0) or 0.0)
                bucket["actual_total_cost_usd"] += float(
                    getattr(row, "actual_total_cost_usd", 0.0) or 0.0
                )
        else:
            # 2. 从预计算表读取历史数据（不包括今天）
            if user_id:
                from src.models.database import StatsUserDaily

                hist_query = db.query(StatsUserDaily).filter(
                    StatsUserDaily.user_id == user_id,
                    StatsUserDaily.date >= start_dt,
                    StatsUserDaily.date < today_start_dt,
                )
                for row in hist_query.all():
                    key = (
                        row.date.date().isoformat()
                        if isinstance(row.date, datetime)
                        else str(row.date)[:10]
                    )
                    aggregated[key] = {
                        "requests": row.total_requests or 0,
                        "total_tokens": (
                            (row.input_tokens or 0)
                            + (row.output_tokens or 0)
                            + (row.cache_creation_tokens or 0)
                            + (row.cache_read_tokens or 0)
                        ),
                        "total_cost_usd": float(row.total_cost or 0.0),
                    }
                    # StatsUserDaily 没有 actual_total_cost 字段，用户视图不需要倍率成本
            else:
                from src.models.database import StatsDaily

                hist_query = db.query(StatsDaily).filter(
                    StatsDaily.date >= start_dt,
                    StatsDaily.date < today_start_dt,
                )
                for row in hist_query.all():
                    key = (
                        row.date.date().isoformat()
                        if isinstance(row.date, datetime)
                        else str(row.date)[:10]
                    )
                    aggregated[key] = {
                        "requests": row.total_requests or 0,
                        "total_tokens": (
                            (row.input_tokens or 0)
                            + (row.output_tokens or 0)
                            + (row.cache_creation_tokens or 0)
                            + (row.cache_read_tokens or 0)
                        ),
                        "total_cost_usd": float(row.total_cost or 0.0),
                    }
                    if include_actual_cost:
                        aggregated[key]["actual_total_cost_usd"] = float(
                            row.actual_total_cost or 0.0  # type: ignore[attr-defined]
                        )

            # 3. 实时查询今天的数据（如果在查询范围内）
            if today >= start_dt.date() and today <= end_dt.date():
                today_start = datetime.combine(today, datetime.min.time(), tzinfo=timezone.utc)
                today_end = datetime.combine(today, datetime.max.time(), tzinfo=timezone.utc)

                if include_actual_cost:
                    today_query = db.query(
                        func.count(Usage.id).label("requests"),
                        func.sum(Usage.total_tokens).label("total_tokens"),
                        func.sum(Usage.total_cost_usd).label("total_cost_usd"),
                        func.sum(Usage.actual_total_cost_usd).label("actual_total_cost_usd"),
                    ).filter(
                        Usage.created_at >= today_start,
                        Usage.created_at <= today_end,
                    )
                else:
                    today_query = db.query(
                        func.count(Usage.id).label("requests"),
                        func.sum(Usage.total_tokens).label("total_tokens"),
                        func.sum(Usage.total_cost_usd).label("total_cost_usd"),
                    ).filter(
                        Usage.created_at >= today_start,
                        Usage.created_at <= today_end,
                    )

                if user_id:
                    today_query = today_query.filter(Usage.user_id == user_id)

                today_row = today_query.first()
                if today_row and today_row.requests:
                    aggregated[today.isoformat()] = {
                        "requests": int(today_row.requests or 0),
                        "total_tokens": int(today_row.total_tokens or 0),
                        "total_cost_usd": float(today_row.total_cost_usd or 0.0),
                    }
                    if include_actual_cost:
                        aggregated[today.isoformat()]["actual_total_cost_usd"] = float(
                            today_row.actual_total_cost_usd or 0.0
                        )

        # 3. 构建返回结果
        days: list[dict[str, Any]] = []
        cursor = start_dt.date()
        end_date_only = end_dt.date()
        max_requests = 0

        while cursor <= end_date_only:
            iso_date = cursor.isoformat()
            stats = aggregated.get(iso_date, {})
            requests = stats.get("requests", 0)
            total_tokens = stats.get("total_tokens", 0)
            total_cost = stats.get("total_cost_usd", 0.0)

            entry: dict[str, Any] = {
                "date": iso_date,
                "requests": requests,
                "total_tokens": total_tokens,
                "total_cost": total_cost,
            }

            if include_actual_cost:
                entry["actual_total_cost"] = stats.get("actual_total_cost_usd", 0.0)

            days.append(entry)
            max_requests = max(max_requests, requests)
            cursor += timedelta(days=1)

        return {
            "start_date": start_dt.date().isoformat(),
            "end_date": end_dt.date().isoformat(),
            "total_days": len(days),
            "max_requests": max_requests,
            "days": days,
        }

    @staticmethod
    def get_top_users(
        db: Session,
        limit: int = 10,
        start_date: datetime | None = None,
        end_date: datetime | None = None,
        order_by: str = "cost",  # cost, tokens, requests
    ) -> list[dict[str, Any]]:
        """获取使用量最高的用户"""

        query = (
            db.query(
                User.id,
                User.email,
                User.username,
                func.count(Usage.id).label("requests"),
                func.sum(Usage.total_tokens).label("tokens"),
                func.sum(Usage.total_cost_usd).label("cost_usd"),
            )
            .join(Usage, User.id == Usage.user_id)
            .filter(Usage.user_id.isnot(None))
        )

        if start_date:
            query = query.filter(Usage.created_at >= start_date)
        if end_date:
            query = query.filter(Usage.created_at <= end_date)

        query = query.group_by(User.id, User.email, User.username)

        # 排序
        if order_by == "cost":
            query = query.order_by(func.sum(Usage.total_cost_usd).desc())
        elif order_by == "tokens":
            query = query.order_by(func.sum(Usage.total_tokens).desc())
        else:
            query = query.order_by(func.count(Usage.id).desc())

        results = query.limit(limit).all()

        return [
            {
                "user_id": row.id,
                "email": row.email,
                "username": row.username,
                "requests": row.requests,
                "tokens": row.tokens,
                "cost_usd": float(row.cost_usd),
            }
            for row in results
        ]

    @staticmethod
    def cleanup_old_usage_records(
        db: Session, days_to_keep: int = 90, batch_size: int = 1000
    ) -> int:
        """清理旧的使用记录（分批删除避免长事务锁定）

        Args:
            db: 数据库会话
            days_to_keep: 保留天数，默认 90 天
            batch_size: 每批删除数量，默认 1000 条

        Returns:
            删除的总记录数
        """
        cutoff_date = datetime.now(timezone.utc) - timedelta(days=days_to_keep)
        total_deleted = 0

        while True:
            # 查询待删除的 ID（使用新索引 idx_usage_user_created）
            batch_ids = (
                db.query(Usage.id).filter(Usage.created_at < cutoff_date).limit(batch_size).all()
            )

            if not batch_ids:
                break

            # 批量删除
            deleted_count = (
                db.query(Usage)
                .filter(Usage.id.in_([row.id for row in batch_ids]))
                .delete(synchronize_session=False)
            )
            db.commit()
            total_deleted += deleted_count

            logger.debug("清理使用记录: 本批删除 {} 条", deleted_count)

        logger.info("清理使用记录: 共删除 {} 条超过 {} 天的记录", total_deleted, days_to_keep)

        return total_deleted
