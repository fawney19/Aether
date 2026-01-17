"""管理员监控与审计端点。"""

from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from typing import List, Optional

from fastapi import APIRouter, Depends, HTTPException, Query, Request
from sqlalchemy import func
from sqlalchemy.orm import Session

from src.api.base.admin_adapter import AdminApiAdapter
from src.api.base.pagination import PaginationMeta, build_pagination_payload, paginate_query
from src.api.base.pipeline import ApiRequestPipeline
from src.core.logger import logger
from src.database import get_db
from src.models.database import (
    ApiKey,
    AuditEventType,
    AuditLog,
    Provider,
    Usage,
)
from src.models.database import User as DBUser
from src.services.health.monitor import HealthMonitor
from src.services.system.audit import audit_service


router = APIRouter(prefix="/api/admin/monitoring", tags=["Admin - Monitoring"])
pipeline = ApiRequestPipeline()


@router.get("/audit-logs")
async def get_audit_logs(
    request: Request,
    username: Optional[str] = Query(None, description="用户名筛选 (模糊匹配)"),
    event_type: Optional[str] = Query(None, description="事件类型筛选"),
    days: int = Query(7, description="查询天数"),
    limit: int = Query(100, description="返回数量限制"),
    offset: int = Query(0, description="偏移量"),
    db: Session = Depends(get_db),
):
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
async def get_system_status(request: Request, db: Session = Depends(get_db)):
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
    """
    adapter = AdminSystemStatusAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/suspicious-activities")
async def get_suspicious_activities(
    request: Request,
    hours: int = Query(24, description="时间范围（小时）"),
    db: Session = Depends(get_db),
):
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
):
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
async def get_resilience_status(request: Request, db: Session = Depends(get_db)):
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
async def reset_error_stats(request: Request, db: Session = Depends(get_db)):
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
):
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
    username: Optional[str]
    event_type: Optional[str]
    days: int
    limit: int
    offset: int

    # 查看审计日志本身不应该产生审计记录，避免刷新页面时产生大量无意义的日志
    audit_log_enabled: bool = False

    async def handle(self, context):  # type: ignore[override]
        db = context.db
        cutoff_time = datetime.now(timezone.utc) - timedelta(days=self.days)

        base_query = (
            db.query(AuditLog, DBUser)
            .outerjoin(DBUser, AuditLog.user_id == DBUser.id)
            .filter(AuditLog.created_at >= cutoff_time)
        )
        if self.username:
            base_query = base_query.filter(DBUser.username.ilike(f"%{self.username}%"))
        if self.event_type:
            base_query = base_query.filter(AuditLog.event_type == self.event_type)

        ordered_query = base_query.order_by(AuditLog.created_at.desc())
        total, logs_with_users = paginate_query(ordered_query, self.limit, self.offset)

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
    async def handle(self, context):  # type: ignore[override]
        db = context.db

        total_users = db.query(func.count(DBUser.id)).scalar()
        active_users = db.query(func.count(DBUser.id)).filter(DBUser.is_active.is_(True)).scalar()

        total_providers = db.query(func.count(Provider.id)).scalar()
        active_providers = (
            db.query(func.count(Provider.id)).filter(Provider.is_active.is_(True)).scalar()
        )

        total_api_keys = db.query(func.count(ApiKey.id)).scalar()
        active_api_keys = (
            db.query(func.count(ApiKey.id)).filter(ApiKey.is_active.is_(True)).scalar()
        )

        today_start = datetime.now(timezone.utc).replace(hour=0, minute=0, second=0, microsecond=0)
        today_requests = (
            db.query(func.count(Usage.id)).filter(Usage.created_at >= today_start).scalar()
        )
        today_tokens = (
            db.query(func.sum(Usage.total_tokens)).filter(Usage.created_at >= today_start).scalar()
            or 0
        )
        today_cost = (
            db.query(func.sum(Usage.total_cost_usd))
            .filter(Usage.created_at >= today_start)
            .scalar()
            or 0
        )

        recent_errors = (
            db.query(AuditLog)
            .filter(
                AuditLog.event_type.in_(
                    [
                        AuditEventType.REQUEST_FAILED.value,
                        AuditEventType.SUSPICIOUS_ACTIVITY.value,
                    ]
                ),
                AuditLog.created_at >= datetime.now(timezone.utc) - timedelta(hours=1),
            )
            .count()
        )

        context.add_audit_metadata(
            action="system_status_snapshot",
            total_users=int(total_users or 0),
            active_users=int(active_users or 0),
            total_providers=int(total_providers or 0),
            active_providers=int(active_providers or 0),
            total_api_keys=int(total_api_keys or 0),
            active_api_keys=int(active_api_keys or 0),
            today_requests=int(today_requests or 0),
            today_tokens=int(today_tokens or 0),
            today_cost=float(today_cost or 0.0),
            recent_errors=int(recent_errors or 0),
        )

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
        }


@dataclass
class AdminSuspiciousActivitiesAdapter(AdminApiAdapter):
    hours: int

    async def handle(self, context):  # type: ignore[override]
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

    async def handle(self, context):  # type: ignore[override]
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
    async def handle(self, context):  # type: ignore[override]
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
    async def handle(self, context):  # type: ignore[override]
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

    async def handle(self, context):  # type: ignore[override]
        history = HealthMonitor.get_circuit_history(self.limit)
        context.add_audit_metadata(
            action="circuit_history",
            limit=self.limit,
            result_count=len(history),
        )
        return {"items": history, "count": len(history)}


def _get_health_recommendations(error_stats: dict, health_score: int) -> List[str]:
    recommendations: List[str] = []
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
