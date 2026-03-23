"""管理员监控与审计端点。"""

from __future__ import annotations

import os
import time
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from typing import Any

from fastapi import APIRouter, Depends, HTTPException, Query, Request
from sqlalchemy import case, func, text
from sqlalchemy.orm import Session

try:
    import psutil
except ModuleNotFoundError:
    psutil = None

from src.api.base.admin_adapter import AdminApiAdapter
from src.api.base.context import ApiRequestContext
from src.api.base.pagination import PaginationMeta, build_pagination_payload
from src.api.base.pipeline import get_pipeline
from src.config.constants import CacheTTL
from src.core.logger import logger
from src.database import get_db
from src.database.database import get_pool_status
from src.models.database import (
    ApiKey,
    AuditEventType,
    AuditLog,
    Provider,
    Usage,
)
from src.models.database import User as DBUser
from src.clients.redis_client import get_redis_client
from src.services.health.monitor import HealthMonitor
from src.services.system.config import SystemConfigService
from src.services.system.audit import audit_service
from src.utils.cache_decorator import cache_result
from src.utils.database_helpers import escape_like_pattern

router = APIRouter(prefix="/api/admin/monitoring", tags=["Admin - Monitoring"])
pipeline = get_pipeline()


def _unavailable_metric(message: str, **fields: Any) -> dict[str, Any]:
    return {
        "status": "unknown",
        "label": _label_for_status("unknown"),
        **fields,
        "message": message,
    }


def _round_metric(value: float | int | None, digits: int = 1) -> float | None:
    if value is None:
        return None
    return round(float(value), digits)


def _metric_status(percent: float | None, *, warning: float, danger: float) -> str:
    if percent is None:
        return "unknown"
    if percent >= danger:
        return "danger"
    if percent >= warning:
        return "warning"
    return "ok"


def _remaining_status(percent: float | None, *, warning: float, danger: float) -> str:
    if percent is None:
        return "unknown"
    if percent <= danger:
        return "danger"
    if percent <= warning:
        return "warning"
    return "ok"


def _label_for_status(status: str) -> str:
    return {
        "ok": "正常",
        "warning": "偏高",
        "danger": "紧张",
        "degraded": "降级",
        "error": "异常",
        "unknown": "未知",
    }.get(status, "未知")


def _optional_int(value: Any) -> int | None:
    if value is None:
        return None
    try:
        return int(value)
    except (TypeError, ValueError):
        return None


def _get_manual_capacity_bytes(db: Session, key: str) -> int | None:
    value = _optional_int(SystemConfigService.get_config(db, key, default=0))
    if value is None or value <= 0:
        return None
    return value


def _build_cpu_metric() -> dict[str, Any]:
    core_count = int((psutil.cpu_count() if psutil is not None else os.cpu_count()) or 0)
    load_percent: float | None = None

    try:
        if hasattr(os, "getloadavg") and core_count > 0:
            load_percent = _round_metric((os.getloadavg()[0] / core_count) * 100)
    except OSError:
        load_percent = None

    if psutil is None:
        status = _metric_status(load_percent, warning=70, danger=85)
        return {
            "status": status,
            "label": _label_for_status(status),
            "usage_percent": None,
            "load_percent": load_percent,
            "core_count": core_count,
            "message": "psutil is not installed",
        }

    cpu_percent = _round_metric(psutil.cpu_percent(interval=0.05))
    status = _metric_status(cpu_percent, warning=70, danger=85)
    return {
        "status": status,
        "label": _label_for_status(status),
        "usage_percent": cpu_percent,
        "load_percent": load_percent,
        "core_count": core_count,
    }


def _build_memory_metric() -> dict[str, Any]:
    if psutil is None:
        return _unavailable_metric(
            "psutil is not installed",
            used_percent=None,
            used_bytes=None,
            available_bytes=None,
            total_bytes=None,
        )

    memory = psutil.virtual_memory()
    used_percent = _round_metric(memory.percent)
    status = _metric_status(used_percent, warning=75, danger=90)
    return {
        "status": status,
        "label": _label_for_status(status),
        "used_percent": used_percent,
        "used_bytes": int(memory.used),
        "available_bytes": int(memory.available),
        "total_bytes": int(memory.total),
    }


async def _build_redis_metric(db: Session) -> dict[str, Any]:
    started = time.perf_counter()
    try:
        redis = await get_redis_client(require_redis=False)
        if redis is None:
            status = "degraded"
            return {
                "status": status,
                "label": _label_for_status(status),
                "latency_ms": None,
                "memory_status": "unknown",
                "memory_label": _label_for_status("unknown"),
                "used_memory_bytes": None,
                "peak_memory_bytes": None,
                "maxmemory_bytes": None,
                "memory_ceiling_bytes": None,
                "memory_source": "unknown",
                "available_memory_bytes": None,
                "memory_percent": None,
                "message": "Redis client not initialized",
            }

        await redis.ping()
        latency_ms = _round_metric((time.perf_counter() - started) * 1000)
        # 优先使用 Redis 自身的 INFO 输出，避免依赖应用所在机器的本地内存视角。
        memory_info = await redis.info(section="memory")
        used_memory_bytes = _optional_int(memory_info.get("used_memory"))
        peak_memory_bytes = _optional_int(memory_info.get("used_memory_peak"))
        maxmemory_bytes = _optional_int(memory_info.get("maxmemory"))
        total_system_memory_bytes = _optional_int(memory_info.get("total_system_memory"))
        configured_memory_bytes = _get_manual_capacity_bytes(db, "redis_memory_total_bytes")
        if maxmemory_bytes is not None and maxmemory_bytes <= 0:
            maxmemory_bytes = None

        memory_ceiling_bytes = configured_memory_bytes or maxmemory_bytes or total_system_memory_bytes
        if configured_memory_bytes is not None:
            memory_source = "configured"
        elif maxmemory_bytes is not None:
            memory_source = "maxmemory"
        elif total_system_memory_bytes is not None:
            memory_source = "system"
        else:
            memory_source = "unknown"
        available_memory_bytes = (
            max(int(memory_ceiling_bytes) - int(used_memory_bytes), 0)
            if memory_ceiling_bytes is not None and used_memory_bytes is not None
            else None
        )
        memory_percent = _round_metric(
            (used_memory_bytes / memory_ceiling_bytes * 100)
            if used_memory_bytes is not None and memory_ceiling_bytes not in (None, 0)
            else None
        )
        status = "warning" if latency_ms is not None and latency_ms >= 80 else "ok"
        memory_status = _metric_status(memory_percent, warning=75, danger=90)
        return {
            "status": status,
            "label": _label_for_status(status),
            "latency_ms": latency_ms,
            "memory_status": memory_status,
            "memory_label": _label_for_status(memory_status),
            "used_memory_bytes": used_memory_bytes,
            "peak_memory_bytes": peak_memory_bytes,
            "maxmemory_bytes": maxmemory_bytes,
            "memory_ceiling_bytes": memory_ceiling_bytes,
            "memory_source": memory_source,
            "available_memory_bytes": available_memory_bytes,
            "memory_percent": memory_percent,
            "message": None,
        }
    except Exception as exc:
        return {
            "status": "error",
            "label": _label_for_status("error"),
            "latency_ms": None,
            "memory_status": "unknown",
            "memory_label": _label_for_status("unknown"),
            "used_memory_bytes": None,
            "peak_memory_bytes": None,
            "maxmemory_bytes": None,
            "memory_ceiling_bytes": None,
            "memory_source": "unknown",
            "available_memory_bytes": None,
            "memory_percent": None,
            "message": str(exc),
        }


def _build_postgres_metric(db: Session) -> dict[str, Any]:
    try:
        pool_status = get_pool_status()
        checked_out = int(pool_status.get("checked_out") or 0)
        max_capacity = int(pool_status.get("max_capacity") or 0)
        pool_usage_percent = _round_metric(
            (checked_out / max_capacity * 100) if max_capacity > 0 else 0
        )
        usage_percent = pool_usage_percent
        storage_total_bytes: int | None = None
        storage_free_bytes: int | None = None
        storage_free_percent: float | None = None
        database_size_bytes: int | None = None
        server_connections: int | None = None
        server_max_connections: int | None = None
        server_usage_percent: float | None = None
        message: str | None = None
        storage_message: str | None = None
        storage_total_bytes = _get_manual_capacity_bytes(db, "postgres_storage_total_bytes")

        bind = db.get_bind()
        dialect_name = bind.dialect.name if bind is not None else ""
        if dialect_name == "postgresql":
            try:
                # 只读取 PostgreSQL 原生 SQL 可见的指标，避免依赖本机数据目录或宿主机磁盘。
                stats = (
                    db.execute(
                        text(
                            """
                            SELECT
                                pg_database_size(current_database()) AS database_size_bytes,
                                current_setting('max_connections')::int AS server_max_connections,
                                COALESCE(
                                    (
                                        SELECT numbackends
                                        FROM pg_stat_database
                                        WHERE datname = current_database()
                                    ),
                                    0
                                ) AS server_connections
                            """
                        )
                    )
                    .mappings()
                    .one()
                )
                database_size_bytes = _optional_int(stats.get("database_size_bytes"))
                server_max_connections = _optional_int(stats.get("server_max_connections"))
                server_connections = _optional_int(stats.get("server_connections"))
                server_usage_percent = _round_metric(
                    (server_connections / server_max_connections * 100)
                    if server_connections is not None and server_max_connections not in (None, 0)
                    else None
                )
                if server_usage_percent is not None:
                    usage_percent = server_usage_percent
                if storage_total_bytes is not None and database_size_bytes is not None:
                    storage_free_bytes = max(storage_total_bytes - database_size_bytes, 0)
                    storage_free_percent = _round_metric(
                        (storage_free_bytes / storage_total_bytes * 100)
                        if storage_total_bytes > 0
                        else None
                    )
                else:
                    storage_message = "请在系统设置中填写 PostgreSQL 总空间"
            except Exception as exc:
                message = f"PostgreSQL 原生指标不可用: {exc}"
                if storage_total_bytes is not None:
                    storage_message = "已配置 PostgreSQL 总空间，但当前库体积不可用"
                else:
                    storage_message = "请在系统设置中填写 PostgreSQL 总空间"
        else:
            storage_message = "当前数据库不是 PostgreSQL"

        status = _metric_status(usage_percent, warning=70, danger=90)
        storage_status = _remaining_status(storage_free_percent, warning=20, danger=10)
        return {
            "status": status,
            "label": _label_for_status(status),
            "usage_percent": usage_percent,
            "pool_usage_percent": pool_usage_percent,
            "checked_out": checked_out,
            "pool_size": int(pool_status.get("pool_size") or 0),
            "overflow": int(pool_status.get("overflow") or 0),
            "max_capacity": max_capacity,
            "pool_timeout": int(pool_status.get("pool_timeout") or 0),
            "server_connections": server_connections,
            "server_max_connections": server_max_connections,
            "server_usage_percent": server_usage_percent,
            "storage_status": storage_status,
            "storage_label": _label_for_status(storage_status),
            "storage_total_bytes": storage_total_bytes,
            "storage_free_bytes": storage_free_bytes,
            "storage_free_percent": storage_free_percent,
            "database_size_bytes": database_size_bytes,
            "storage_message": storage_message,
            "message": message,
        }
    except Exception as exc:
        return {
            "status": "error",
            "label": _label_for_status("error"),
            "usage_percent": None,
            "pool_usage_percent": None,
            "checked_out": 0,
            "pool_size": 0,
            "overflow": 0,
            "max_capacity": 0,
            "pool_timeout": 0,
            "server_connections": None,
            "server_max_connections": None,
            "server_usage_percent": None,
            "storage_status": "unknown",
            "storage_label": _label_for_status("unknown"),
            "storage_total_bytes": None,
            "storage_free_bytes": None,
            "storage_free_percent": None,
            "database_size_bytes": None,
            "storage_message": None,
            "message": str(exc),
        }


@router.get("/audit-logs")
async def get_audit_logs(
    request: Request,
    username: str | None = Query(None, description="用户名筛选 (模糊匹配)"),
    event_type: str | None = Query(None, description="事件类型筛选"),
    days: int = Query(7, description="查询天数"),
    limit: int = Query(100, description="返回数量限制"),
    offset: int = Query(0, description="偏移量"),
    db: Session = Depends(get_db),
) -> Any:
    """
    获取审计日志

    获取系统审计日志列表，支持按用户名、事件类型、时间范围筛选。需要管理员权限。

    **查询参数**:
    - `username`: 可选，用户名筛选（模糊匹配）
    - `event_type`: 可选，事件类型筛选
    - `days`: 查询最近多少天的日志，默认 7 天
    - `limit`: 返回数量限制，默认 100
    - `offset`: 分页偏移量，默认 0

    **返回字段**:
    - `items`: 审计日志列表，每条日志包含：
      - `id`: 日志 ID
      - `event_type`: 事件类型
      - `user_id`: 用户 ID
      - `user_email`: 用户邮箱
      - `user_username`: 用户名
      - `description`: 事件描述
      - `ip_address`: IP 地址
      - `status_code`: HTTP 状态码
      - `error_message`: 错误信息
      - `metadata`: 事件元数据
      - `created_at`: 创建时间
    - `meta`: 分页元数据（total, limit, offset, count）
    - `filters`: 筛选条件
    """
    adapter = AdminGetAuditLogsAdapter(
        username=username,
        event_type=event_type,
        days=days,
        limit=limit,
        offset=offset,
    )
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/system-status")
async def get_system_status(request: Request, db: Session = Depends(get_db)) -> Any:
    """
    获取系统状态

    获取系统当前的运行状态和关键指标。需要管理员权限。

    **返回字段**:
    - `timestamp`: 当前时间戳
    - `users`: 用户统计（total: 总用户数, active: 活跃用户数）
    - `providers`: 提供商统计（total: 总提供商数, active: 活跃提供商数）
    - `api_keys`: API Key 统计（total: 总数, active: 活跃数）
    - `today_stats`: 今日统计（requests: 请求数, tokens: token 数, cost_usd: 成本）
    - `recent_errors`: 最近 1 小时内的错误数
    - `system_metrics`: 系统资源摘要（cpu / memory / redis / postgres）
    """
    adapter = AdminSystemStatusAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/suspicious-activities")
async def get_suspicious_activities(
    request: Request,
    hours: int = Query(24, description="时间范围（小时）"),
    db: Session = Depends(get_db),
) -> Any:
    """
    获取可疑活动记录

    获取系统检测到的可疑活动记录。需要管理员权限。

    **查询参数**:
    - `hours`: 时间范围（小时），默认 24 小时

    **返回字段**:
    - `activities`: 可疑活动列表，每条记录包含：
      - `id`: 记录 ID
      - `event_type`: 事件类型
      - `user_id`: 用户 ID
      - `description`: 事件描述
      - `ip_address`: IP 地址
      - `metadata`: 事件元数据
      - `created_at`: 创建时间
    - `count`: 活动总数
    - `time_range_hours`: 查询的时间范围（小时）
    """
    adapter = AdminSuspiciousActivitiesAdapter(hours=hours)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/user-behavior/{user_id}")
async def analyze_user_behavior(
    user_id: str,
    request: Request,
    days: int = Query(30, description="分析天数"),
    db: Session = Depends(get_db),
) -> Any:
    """
    分析用户行为

    分析指定用户的行为模式和使用情况。需要管理员权限。

    **路径参数**:
    - `user_id`: 用户 ID

    **查询参数**:
    - `days`: 分析最近多少天的数据，默认 30 天

    **返回字段**:
    - 用户行为分析结果，包括活动频率、使用模式、异常行为等
    """
    adapter = AdminUserBehaviorAdapter(user_id=user_id, days=days)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/resilience-status")
async def get_resilience_status(request: Request, db: Session = Depends(get_db)) -> Any:
    """
    获取韧性系统状态

    获取系统韧性管理的当前状态，包括错误统计、熔断器状态等。需要管理员权限。

    **返回字段**:
    - `timestamp`: 当前时间戳
    - `health_score`: 健康评分（0-100）
    - `status`: 系统状态（healthy: 健康，degraded: 降级，critical: 严重）
    - `error_statistics`: 错误统计信息
    - `recent_errors`: 最近的错误列表（最多 10 条）
    - `recommendations`: 系统建议
    """
    adapter = AdminResilienceStatusAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.delete("/resilience/error-stats")
async def reset_error_stats(request: Request, db: Session = Depends(get_db)) -> None:
    """
    重置错误统计

    重置韧性系统的错误统计数据。需要管理员权限。

    **返回字段**:
    - `message`: 操作结果信息
    - `previous_stats`: 重置前的统计数据
    - `reset_by`: 执行重置的管理员邮箱
    - `reset_at`: 重置时间
    """
    adapter = AdminResetErrorStatsAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/resilience/circuit-history")
async def get_circuit_history(
    request: Request,
    limit: int = Query(50, ge=1, le=200),
    db: Session = Depends(get_db),
) -> Any:
    """
    获取熔断器历史记录

    获取熔断器的状态变更历史记录。需要管理员权限。

    **查询参数**:
    - `limit`: 返回数量限制，默认 50，最大 200

    **返回字段**:
    - `items`: 熔断器历史记录列表
    - `count`: 记录总数
    """
    adapter = AdminCircuitHistoryAdapter(limit=limit)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@dataclass
class AdminGetAuditLogsAdapter(AdminApiAdapter):
    username: str | None
    event_type: str | None
    days: int
    limit: int
    offset: int

    # 查看审计日志本身不应该产生审计记录，避免刷新页面时产生大量无意义的日志
    audit_log_enabled: bool = False

    @cache_result(
        key_prefix="admin:monitoring:audit-logs",
        ttl=CacheTTL.ADMIN_USAGE_RECORDS,
        user_specific=False,
        vary_by=["username", "event_type", "days", "limit", "offset"],
    )
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        cutoff_time = datetime.now(timezone.utc) - timedelta(days=self.days)

        count_query = db.query(func.count(AuditLog.id)).filter(AuditLog.created_at >= cutoff_time)
        if self.username:
            escaped = escape_like_pattern(self.username)
            count_query = count_query.outerjoin(DBUser, AuditLog.user_id == DBUser.id).filter(
                DBUser.username.ilike(f"%{escaped}%", escape="\\")
            )
        if self.event_type:
            count_query = count_query.filter(AuditLog.event_type == self.event_type)
        total = int(count_query.scalar() or 0)

        base_query = (
            db.query(AuditLog, DBUser)
            .outerjoin(DBUser, AuditLog.user_id == DBUser.id)
            .filter(AuditLog.created_at >= cutoff_time)
        )
        if self.username:
            escaped = escape_like_pattern(self.username)
            base_query = base_query.filter(DBUser.username.ilike(f"%{escaped}%", escape="\\"))
        if self.event_type:
            base_query = base_query.filter(AuditLog.event_type == self.event_type)

        logs_with_users = (
            base_query.order_by(AuditLog.created_at.desc())
            .offset(self.offset)
            .limit(self.limit)
            .all()
        )

        items = [
            {
                "id": log.id,
                "event_type": log.event_type,
                "user_id": log.user_id,
                "user_email": user.email if user else None,
                "user_username": user.username if user else None,
                "description": log.description,
                "ip_address": log.ip_address,
                "status_code": log.status_code,
                "error_message": log.error_message,
                "metadata": log.event_metadata,
                "created_at": log.created_at.isoformat() if log.created_at else None,
            }
            for log, user in logs_with_users
        ]
        meta = PaginationMeta(
            total=total,
            limit=self.limit,
            offset=self.offset,
            count=len(items),
        )

        payload = build_pagination_payload(
            items,
            meta,
            filters={
                "username": self.username,
                "event_type": self.event_type,
                "days": self.days,
            },
        )
        context.add_audit_metadata(
            action="monitor_audit_logs",
            filter_username=self.username,
            filter_event_type=self.event_type,
            days=self.days,
            limit=self.limit,
            offset=self.offset,
            total=total,
            result_count=meta.count,
        )
        return payload


class AdminSystemStatusAdapter(AdminApiAdapter):
    @cache_result(
        key_prefix="admin:monitoring:system-status",
        ttl=CacheTTL.ADMIN_USAGE_RECORDS,
        user_specific=False,
    )
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db

        user_stats = db.query(
            func.count(DBUser.id).label("total"),
            func.sum(case((DBUser.is_active.is_(True), 1), else_=0)).label("active"),
        ).first()
        total_users = int((user_stats.total if user_stats else 0) or 0)
        active_users = int((user_stats.active if user_stats else 0) or 0)

        provider_stats = db.query(
            func.count(Provider.id).label("total"),
            func.sum(case((Provider.is_active.is_(True), 1), else_=0)).label("active"),
        ).first()
        total_providers = int((provider_stats.total if provider_stats else 0) or 0)
        active_providers = int((provider_stats.active if provider_stats else 0) or 0)

        api_key_stats = db.query(
            func.count(ApiKey.id).label("total"),
            func.sum(case((ApiKey.is_active.is_(True), 1), else_=0)).label("active"),
        ).first()
        total_api_keys = int((api_key_stats.total if api_key_stats else 0) or 0)
        active_api_keys = int((api_key_stats.active if api_key_stats else 0) or 0)

        today_start = datetime.now(timezone.utc).replace(hour=0, minute=0, second=0, microsecond=0)
        today_stats = (
            db.query(
                func.count(Usage.id).label("requests"),
                func.coalesce(func.sum(Usage.total_tokens), 0).label("tokens"),
                func.coalesce(func.sum(Usage.total_cost_usd), 0.0).label("cost"),
            )
            .filter(Usage.created_at >= today_start)
            .first()
        )
        today_requests = int((today_stats.requests if today_stats else 0) or 0)
        today_tokens = int((today_stats.tokens if today_stats else 0) or 0)
        today_cost = float((today_stats.cost if today_stats else 0.0) or 0.0)

        recent_errors = (
            db.query(func.count(AuditLog.id))
            .filter(
                AuditLog.event_type.in_(
                    [
                        AuditEventType.REQUEST_FAILED.value,
                        AuditEventType.SUSPICIOUS_ACTIVITY.value,
                    ]
                ),
                AuditLog.created_at >= datetime.now(timezone.utc) - timedelta(hours=1),
            )
            .scalar()
            or 0
        )

        context.add_audit_metadata(
            action="system_status_snapshot",
            total_users=total_users,
            active_users=active_users,
            total_providers=total_providers,
            active_providers=active_providers,
            total_api_keys=total_api_keys,
            active_api_keys=active_api_keys,
            today_requests=today_requests,
            today_tokens=today_tokens,
            today_cost=today_cost,
            recent_errors=int(recent_errors or 0),
        )

        cpu_metric = _build_cpu_metric()
        memory_metric = _build_memory_metric()
        redis_metric = await _build_redis_metric(db)
        postgres_metric = _build_postgres_metric(db)

        return {
            "timestamp": datetime.now(timezone.utc).isoformat(),
            "users": {"total": total_users, "active": active_users},
            "providers": {"total": total_providers, "active": active_providers},
            "api_keys": {"total": total_api_keys, "active": active_api_keys},
            "today_stats": {
                "requests": today_requests,
                "tokens": today_tokens,
                "cost_usd": f"${today_cost:.4f}",
            },
            "recent_errors": recent_errors,
            "system_metrics": {
                "cpu": cpu_metric,
                "memory": memory_metric,
                "redis": redis_metric,
                "postgres": postgres_metric,
            },
        }


@dataclass
class AdminSuspiciousActivitiesAdapter(AdminApiAdapter):
    hours: int

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        activities = audit_service.get_suspicious_activities(db=db, hours=self.hours, limit=100)
        response = {
            "activities": [
                {
                    "id": activity.id,
                    "event_type": activity.event_type,
                    "user_id": activity.user_id,
                    "description": activity.description,
                    "ip_address": activity.ip_address,
                    "metadata": activity.event_metadata,
                    "created_at": activity.created_at.isoformat() if activity.created_at else None,
                }
                for activity in activities
            ],
            "count": len(activities),
            "time_range_hours": self.hours,
        }
        context.add_audit_metadata(
            action="monitor_suspicious_activity",
            hours=self.hours,
            result_count=len(activities),
        )
        return response


@dataclass
class AdminUserBehaviorAdapter(AdminApiAdapter):
    user_id: str
    days: int

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        result = audit_service.analyze_user_behavior(
            db=context.db,
            user_id=self.user_id,
            days=self.days,
        )
        context.add_audit_metadata(
            action="monitor_user_behavior",
            target_user_id=self.user_id,
            days=self.days,
            contains_summary=bool(result),
        )
        return result


class AdminResilienceStatusAdapter(AdminApiAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        try:
            from src.core.resilience import resilience_manager
        except ImportError as exc:
            raise HTTPException(status_code=503, detail="韧性管理系统未启用") from exc

        error_stats = resilience_manager.get_error_stats()
        recent_errors = [
            {
                "error_id": info["error_id"],
                "error_type": info["error_type"],
                "operation": info["operation"],
                "timestamp": info["timestamp"].isoformat(),
                "context": info.get("context", {}),
            }
            for info in resilience_manager.last_errors[-10:]
        ]

        total_errors = error_stats.get("total_errors", 0)
        circuit_breakers = error_stats.get("circuit_breakers", {})
        circuit_breakers_open = sum(
            1 for status in circuit_breakers.values() if status.get("state") == "open"
        )
        health_score = max(0, 100 - (total_errors * 2) - (circuit_breakers_open * 20))

        response = {
            "timestamp": datetime.now(timezone.utc).isoformat(),
            "health_score": health_score,
            "status": (
                "healthy" if health_score > 80 else "degraded" if health_score > 50 else "critical"
            ),
            "error_statistics": error_stats,
            "recent_errors": recent_errors,
            "recommendations": _get_health_recommendations(error_stats, health_score),
        }
        context.add_audit_metadata(
            action="resilience_status",
            health_score=health_score,
            error_total=error_stats.get("total_errors") if isinstance(error_stats, dict) else None,
            open_circuit_breakers=circuit_breakers_open,
        )
        return response


class AdminResetErrorStatsAdapter(AdminApiAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        try:
            from src.core.resilience import resilience_manager
        except ImportError as exc:
            raise HTTPException(status_code=503, detail="韧性管理系统未启用") from exc

        old_stats = resilience_manager.get_error_stats()
        resilience_manager.error_stats.clear()
        resilience_manager.last_errors.clear()

        logger.info(f"管理员 {context.user.email if context.user else 'unknown'} 重置了错误统计")

        context.add_audit_metadata(
            action="reset_error_stats",
            previous_total_errors=(
                old_stats.get("total_errors") if isinstance(old_stats, dict) else None
            ),
        )

        return {
            "message": "错误统计已重置",
            "previous_stats": old_stats,
            "reset_by": context.user.email if context.user else None,
            "reset_at": datetime.now(timezone.utc).isoformat(),
        }


class AdminCircuitHistoryAdapter(AdminApiAdapter):
    def __init__(self, limit: int = 50):
        super().__init__()
        self.limit = limit

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        history = HealthMonitor.get_circuit_history(self.limit)
        context.add_audit_metadata(
            action="circuit_history",
            limit=self.limit,
            result_count=len(history),
        )
        return {"items": history, "count": len(history)}


def _get_health_recommendations(error_stats: dict, health_score: int) -> list[str]:
    recommendations: list[str] = []
    if health_score < 50:
        recommendations.append("系统健康状况严重，请立即检查错误日志")
    if error_stats.get("total_errors", 0) > 100:
        recommendations.append("错误频率过高，建议检查系统配置和外部依赖")

    circuit_breakers = error_stats.get("circuit_breakers", {})
    open_breakers = [k for k, v in circuit_breakers.items() if v.get("state") == "open"]
    if open_breakers:
        recommendations.append(f"以下服务熔断器已打开：{', '.join(open_breakers)}")

    if health_score > 90:
        recommendations.append("系统运行良好")
    return recommendations
