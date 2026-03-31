"""普通用户可访问的监控与审计端点。"""

from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from typing import Any

from fastapi import APIRouter, Depends, HTTPException, Query, Request
from sqlalchemy.orm import Session

from src.api.base.adapter import ApiAdapter, ApiMode
from src.api.base.context import ApiRequestContext
from src.api.base.pagination import PaginationMeta, build_pagination_payload, paginate_query
from src.api.base.pipeline import get_pipeline
from src.core.logger import logger
from src.core.user_access import resolve_user_rate_limit
from src.database import get_db
from src.models.database import ApiKey, AuditLog
from src.services.rate_limit.user_rpm_limiter import SYSTEM_RPM_CONFIG_KEY, get_user_rpm_limiter
from src.services.system.config import SystemConfigService

router = APIRouter(prefix="/api/monitoring", tags=["Monitoring"])
pipeline = get_pipeline()


@router.get("/my-audit-logs")
async def get_my_audit_logs(
    request: Request,
    event_type: str | None = Query(None, description="事件类型筛选"),
    days: int = Query(30, description="查询天数"),
    limit: int = Query(50, description="返回数量限制"),
    offset: int = Query(0, ge=0, description="偏移量"),
    db: Session = Depends(get_db),
) -> Any:
    """
    获取我的审计日志

    获取当前用户的审计日志记录。需要登录。

    **查询参数**:
    - `event_type`: 可选，事件类型筛选
    - `days`: 查询最近多少天的日志，默认 30 天
    - `limit`: 返回数量限制，默认 50
    - `offset`: 分页偏移量，默认 0

    **返回字段**:
    - `items`: 审计日志列表，每条日志包含：
      - `id`: 日志 ID
      - `event_type`: 事件类型
      - `description`: 事件描述
      - `ip_address`: IP 地址
      - `status_code`: HTTP 状态码
      - `created_at`: 创建时间
    - `meta`: 分页元数据（total, limit, offset, count）
    - `filters`: 筛选条件
    """
    adapter = UserAuditLogsAdapter(event_type=event_type, days=days, limit=limit, offset=offset)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/rate-limit-status")
async def get_rate_limit_status(request: Request, db: Session = Depends(get_db)) -> Any:
    """
    获取速率限制状态

    获取当前用户所有活跃 API Key 的速率限制状态。需要登录。

    **返回字段**:
    - `user_id`: 用户 ID
    - `api_keys`: API Key 限流状态列表，每个包含：
      - `api_key_name`: API Key 名称
      - `limit`: 速率限制上限
      - `remaining`: 剩余可用次数
      - `reset_time`: 限制重置时间
      - `window`: 时间窗口
    """
    adapter = UserRateLimitStatusAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


class AuthenticatedApiAdapter(ApiAdapter):
    """需要用户登录的适配器基类。"""

    mode = ApiMode.USER

    def authorize(self, context: ApiRequestContext) -> None:  # type: ignore[override]
        if not context.user:
            raise HTTPException(status_code=401, detail="未登录")


@dataclass
class UserAuditLogsAdapter(AuthenticatedApiAdapter):
    event_type: str | None
    days: int
    limit: int
    offset: int

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        user = context.user
        if not user:
            raise HTTPException(status_code=401, detail="未登录")

        query = db.query(AuditLog).filter(AuditLog.user_id == user.id)
        if self.event_type:
            query = query.filter(AuditLog.event_type == self.event_type)

        cutoff_time = datetime.now(timezone.utc) - timedelta(days=self.days)
        query = query.filter(AuditLog.created_at >= cutoff_time)
        query = query.order_by(AuditLog.created_at.desc())

        total, logs = paginate_query(query, self.limit, self.offset)

        items = [
            {
                "id": log.id,
                "event_type": log.event_type,
                "description": log.description,
                "ip_address": log.ip_address,
                "status_code": log.status_code,
                "created_at": log.created_at.isoformat() if log.created_at else None,
            }
            for log in logs
        ]

        meta = PaginationMeta(
            total=total,
            limit=self.limit,
            offset=self.offset,
            count=len(items),
        )

        return build_pagination_payload(
            items,
            meta,
            filters={
                "event_type": self.event_type,
                "days": self.days,
            },
        )


class UserRateLimitStatusAdapter(AuthenticatedApiAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        user = context.user
        if not user:
            raise HTTPException(status_code=401, detail="未登录")

        api_keys = (
            db.query(ApiKey)
            .filter(ApiKey.user_id == user.id, ApiKey.is_active.is_(True))
            .order_by(ApiKey.created_at.desc())
            .all()
        )

        try:
            limiter = await get_user_rpm_limiter()
            system_default_raw = SystemConfigService.get_config(
                db, SYSTEM_RPM_CONFIG_KEY, default=0
            )
            system_default = max(int(system_default_raw or 0), 0)
            reset_at = limiter.get_reset_at()
            window = f"{limiter.bucket_seconds}s"
        except Exception as exc:
            logger.warning("读取新 RPM 限流状态失败，回退插件状态接口: {}", exc)
            limiter = None
            system_default = 0
            reset_at = None
            window = None

        rate_limit_info = []
        for key in api_keys:
            if limiter is not None:
                if key.is_standalone:
                    user_limit = key.rate_limit if key.rate_limit is not None else system_default
                    user_scope_key = limiter.get_standalone_rpm_key(key.id)
                    key_limit = 0
                else:
                    effective_user_rate_limit = resolve_user_rate_limit(user)
                    user_limit = (
                        effective_user_rate_limit
                        if effective_user_rate_limit is not None
                        else system_default
                    )
                    user_scope_key = limiter.get_user_rpm_key(user.id)
                    key_limit = max(int(key.rate_limit or 0), 0)

                user_count = (
                    await limiter.get_scope_count(user_scope_key)
                    if user_limit and user_limit > 0
                    else 0
                )
                key_count = (
                    await limiter.get_scope_count(limiter.get_key_rpm_key(key.id))
                    if key_limit > 0
                    else 0
                )
                user_remaining = max(user_limit - user_count, 0) if user_limit > 0 else None
                key_remaining = max(key_limit - key_count, 0) if key_limit > 0 else None

                scoped_statuses: list[tuple[str, int, int]] = []
                if user_limit > 0 and user_remaining is not None:
                    scoped_statuses.append(("user", user_limit, user_remaining))
                if key_limit > 0 and key_remaining is not None:
                    scoped_statuses.append(("key", key_limit, key_remaining))

                primary_scope = (
                    min(scoped_statuses, key=lambda item: item[2]) if scoped_statuses else None
                )
                rate_limit_info.append(
                    {
                        "api_key_name": key.name or f"Key-{key.id}",
                        "limit": primary_scope[1] if primary_scope else None,
                        "remaining": primary_scope[2] if primary_scope else None,
                        "scope": primary_scope[0] if primary_scope else None,
                        "reset_time": reset_at.isoformat() if reset_at else None,
                        "window": window,
                        "user_limit": user_limit,
                        "user_remaining": user_remaining,
                        "key_limit": key_limit if key_limit > 0 else None,
                        "key_remaining": key_remaining,
                    }
                )
                continue

            rate_limit_info.append(
                {
                    "api_key_name": key.name or f"Key-{key.id}",
                    "limit": None,
                    "remaining": None,
                    "scope": None,
                    "reset_time": None,
                    "window": None,
                    "user_limit": None,
                    "user_remaining": None,
                    "key_limit": None,
                    "key_remaining": None,
                }
            )

        return {"user_id": user.id, "api_keys": rate_limit_info}
