"""
System Catalog / 健康检查相关端点

这些是系统工具端点，不需要复杂的 Adapter 抽象。
"""

from datetime import datetime, timezone
from typing import Any, Dict, Optional

import httpx
from fastapi import APIRouter, Depends, HTTPException, Query, Request
from sqlalchemy import func
from sqlalchemy.orm import Session, selectinload

from src.api.handlers.base.request_builder import PassthroughRequestBuilder
from src.clients.redis_client import get_redis_client, get_redis_client_sync
from src.core.logger import logger
from src.database import get_db
from src.database.database import get_pool_status
from src.models.database import Model, Provider
from src.services.orchestration.fallback_orchestrator import FallbackOrchestrator
from src.services.provider.transport import build_provider_url

router = APIRouter(tags=["System Catalog"])


# ============== 辅助函数 ==============


def _as_bool(value: Optional[str], default: bool) -> bool:
    """将字符串转换为布尔值"""
    if value is None:
        return default
    return value.lower() in {"1", "true", "yes", "on"}


def _serialize_provider(
    provider: Provider,
    include_models: bool,
    include_endpoints: bool,
) -> Dict[str, Any]:
    """序列化 Provider 对象"""
    provider_data: Dict[str, Any] = {
        "id": provider.id,
        "name": provider.name,
        "is_active": provider.is_active,
        "provider_priority": provider.provider_priority,
    }

    if include_endpoints:
        provider_data["endpoints"] = [
            {
                "id": endpoint.id,
                "base_url": endpoint.base_url,
                "api_format": endpoint.api_format if endpoint.api_format else None,
                "is_active": endpoint.is_active,
            }
            for endpoint in provider.endpoints or []
        ]

    if include_models:
        provider_data["models"] = [
            {
                "id": model.id,
                "name": (
                    model.global_model.name if model.global_model else model.provider_model_name
                ),
                "display_name": (
                    model.global_model.display_name
                    if model.global_model
                    else model.provider_model_name
                ),
                "is_active": model.is_active,
                "supports_streaming": model.supports_streaming,
            }
            for model in provider.models or []
            if model.is_active
        ]

    return provider_data


def _select_provider(db: Session, provider_name: Optional[str]) -> Optional[Provider]:
    """选择 Provider（按 provider_priority 优先级选择）"""
    query = db.query(Provider).filter(Provider.is_active == True)
    if provider_name:
        provider = query.filter(Provider.name == provider_name).first()
        if provider:
            return provider

    # 按优先级选择（provider_priority 最小的优先）
    return query.order_by(Provider.provider_priority.asc()).first()


# ============== 端点 ==============


@router.get("/v1/health")
async def service_health(db: Session = Depends(get_db)):
    """返回服务健康状态与依赖信息"""
    active_providers = (
        db.query(func.count(Provider.id)).filter(Provider.is_active == True).scalar() or 0
    )
    active_models = db.query(func.count(Model.id)).filter(Model.is_active == True).scalar() or 0

    redis_info: Dict[str, Any] = {"status": "unknown"}
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
async def health_check():
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
async def root(db: Session = Depends(get_db)):
    """Root endpoint - 服务信息概览"""
    # 按优先级选择最高优先级的提供商
    top_provider = (
        db.query(Provider)
        .filter(Provider.is_active == True)
        .order_by(Provider.provider_priority.asc())
        .first()
    )
    active_providers = db.query(Provider).filter(Provider.is_active == True).count()

    return {
        "message": "AI Proxy with Modular Architecture v4.0.0",
        "status": "running",
        "current_provider": top_provider.name if top_provider else "None",
        "available_providers": active_providers,
        "config": {},
        "endpoints": {
            "messages": "/v1/messages",
            "count_tokens": "/v1/messages/count_tokens",
            "health": "/v1/health",
            "providers": "/v1/providers",
            "test_connection": "/v1/test-connection",
        },
    }


@router.get("/v1/providers")
async def list_providers(
    db: Session = Depends(get_db),
    include_models: bool = Query(False),
    include_endpoints: bool = Query(False),
    active_only: bool = Query(True),
):
    """列出所有 Provider"""
    load_options = []
    if include_models:
        load_options.append(selectinload(Provider.models).selectinload(Model.global_model))
    if include_endpoints:
        load_options.append(selectinload(Provider.endpoints))

    base_query = db.query(Provider)
    if load_options:
        base_query = base_query.options(*load_options)
    if active_only:
        base_query = base_query.filter(Provider.is_active == True)
    base_query = base_query.order_by(Provider.provider_priority.asc(), Provider.name.asc())

    providers = base_query.all()
    return {
        "providers": [
            _serialize_provider(provider, include_models, include_endpoints)
            for provider in providers
        ]
    }


@router.get("/v1/providers/{provider_identifier}")
async def provider_detail(
    provider_identifier: str,
    db: Session = Depends(get_db),
    include_models: bool = Query(False),
    include_endpoints: bool = Query(False),
):
    """获取单个 Provider 详情"""
    load_options = []
    if include_models:
        load_options.append(selectinload(Provider.models).selectinload(Model.global_model))
    if include_endpoints:
        load_options.append(selectinload(Provider.endpoints))

    base_query = db.query(Provider)
    if load_options:
        base_query = base_query.options(*load_options)

    provider = base_query.filter(
        (Provider.id == provider_identifier) | (Provider.name == provider_identifier)
    ).first()
    if not provider:
        raise HTTPException(status_code=404, detail="Provider not found")

    return _serialize_provider(provider, include_models, include_endpoints)


@router.get("/v1/test-connection")
@router.get("/test-connection")
async def test_connection(
    request: Request,
    db: Session = Depends(get_db),
    provider: Optional[str] = Query(None),
    model: str = Query("claude-3-haiku-20240307"),
    api_format: Optional[str] = Query(None),
):
    """测试 Provider 连接"""
    selected_provider = _select_provider(db, provider)
    if not selected_provider:
        raise HTTPException(status_code=503, detail="No active provider available")

    # 构建测试请求体
    payload = {
        "model": model,
        "messages": [{"role": "user", "content": "Health check"}],
        "max_tokens": 5,
    }

    # 确定 API 格式
    format_value = api_format or "CLAUDE"

    # 创建 FallbackOrchestrator
    redis_client = get_redis_client_sync()
    orchestrator = FallbackOrchestrator(db, redis_client)

    # 定义请求函数
    async def test_request_func(_prov, endpoint, key, _candidate):
        request_builder = PassthroughRequestBuilder()
        provider_payload, provider_headers = request_builder.build(
            payload, {}, endpoint, key, is_stream=False
        )

        url = build_provider_url(
            endpoint,
            query_params=dict(request.query_params),
            path_params={"model": model},
            is_stream=False,
        )

        async with httpx.AsyncClient(timeout=30.0) as client:
            resp = await client.post(url, json=provider_payload, headers=provider_headers)
            resp.raise_for_status()
            return resp.json()

    try:
        response, actual_provider, *_ = await orchestrator.execute_with_fallback(
            api_format=format_value,
            model_name=model,
            user_api_key=None,
            request_func=test_request_func,
            request_id=None,
        )
        return {
            "status": "success",
            "provider": actual_provider,
            "timestamp": datetime.now(timezone.utc).isoformat(),
            "response_id": response.get("id", "unknown"),
        }
    except Exception as exc:
        logger.error(f"API connectivity test failed: {exc}")
        raise HTTPException(status_code=503, detail=str(exc))
