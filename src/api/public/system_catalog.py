"""
System Catalog / 健康检查相关端点

这些是系统工具端点，不需要复杂的 Adapter 抽象。
"""

from __future__ import annotations

from datetime import datetime, timezone
from typing import Any

from fastapi import APIRouter, Depends, HTTPException
from sqlalchemy import func
from sqlalchemy.orm import Session
from src.clients.redis_client import get_redis_client
from src.database import get_db
from src.database.database import get_pool_status
from src.models.database import Model, Provider

router = APIRouter(tags=["System Catalog"])


# ============== 端点 ==============


@router.get("/v1/health")
async def service_health(db: Session = Depends(get_db)) -> Any:
    """返回服务健康状态与依赖信息"""
    active_providers = (
        db.query(func.count(Provider.id)).filter(Provider.is_active.is_(True)).scalar() or 0
    )
    active_models = db.query(func.count(Model.id)).filter(Model.is_active.is_(True)).scalar() or 0

    redis_info: dict[str, Any] = {"status": "unknown"}
    try:
        redis = await get_redis_client()
        if redis:
            await redis.ping()
            redis_info = {"status": "ok"}
        else:
            redis_info = {"status": "degraded", "message": "Redis client not initialized"}
    except Exception as exc:
        redis_info = {"status": "error", "message": str(exc)}

    return {
        "status": "ok",
        "timestamp": datetime.now(timezone.utc).isoformat(),
        "stats": {
            "active_providers": active_providers,
            "active_models": active_models,
        },
        "dependencies": {
            "database": {"status": "ok"},
            "redis": redis_info,
        },
    }


@router.get("/health")
async def health_check() -> Any:
    """简单健康检查端点（无需认证）"""
    try:
        pool_status = get_pool_status()
        pool_health = {
            "checked_out": pool_status["checked_out"],
            "pool_size": pool_status["pool_size"],
            "overflow": pool_status["overflow"],
            "max_capacity": pool_status["max_capacity"],
            "usage_rate": (
                f"{(pool_status['checked_out'] / pool_status['max_capacity'] * 100):.1f}%"
                if pool_status["max_capacity"] > 0
                else "0.0%"
            ),
        }
    except Exception as e:
        pool_health = {"error": str(e)}

    return {
        "status": "healthy",
        "timestamp": datetime.now(timezone.utc).isoformat(),
        "database_pool": pool_health,
    }


@router.get("/")
async def root() -> Any:
    """Root endpoint - 服务信息概览"""
    return {
        "message": "AI Proxy with Modular Architecture v4.0.0",
        "status": "running",
        "config": {},
        "endpoints": {
            "messages": "/v1/messages",
            "count_tokens": "/v1/messages/count_tokens",
            "health": "/v1/health",
        },
    }


@router.get("/v1/providers", include_in_schema=False)
async def list_providers() -> Any:
    raise HTTPException(status_code=404, detail="Not Found")


@router.get("/v1/providers/{provider_identifier}", include_in_schema=False)
async def provider_detail(provider_identifier: str) -> Any:
    raise HTTPException(status_code=404, detail="Not Found")


@router.get("/v1/test-connection", include_in_schema=False)
@router.get("/test-connection", include_in_schema=False)
async def test_connection() -> Any:
    raise HTTPException(status_code=404, detail="Not Found")
