"""
Provider Query API 端点
用于查询提供商的模型列表等信息
"""

from __future__ import annotations

import asyncio
from typing import Any

import httpx
from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel
from sqlalchemy.orm import Session, joinedload

from src.config.constants import TimeoutDefaults
from src.core.api_format import get_extra_headers_from_endpoint
from src.core.cache_service import CacheService
from src.core.crypto import crypto_service
from src.core.logger import logger
from src.core.provider_types import ProviderType
from src.database.database import get_db
from src.models.database import Provider, ProviderEndpoint, User
from src.services.model.fetch_scheduler import (
    MODEL_FETCH_HTTP_TIMEOUT,
    UPSTREAM_MODELS_CACHE_TTL_SECONDS,
    get_upstream_models_from_cache,
    set_upstream_models_to_cache,
)
from src.services.model.upstream_fetcher import (
    EndpointFetchConfig,
    UpstreamModelsFetchContext,
    build_format_to_config,
    fetch_models_for_key,
    get_adapter_for_format,
)
from src.services.provider.oauth_token import resolve_oauth_access_token
from src.services.proxy_node.resolver import resolve_effective_proxy, resolve_proxy_param
from src.utils import json_helper as json
from src.utils.auth_utils import get_current_user

router = APIRouter(prefix="/api/admin/provider-query", tags=["Provider Query"])


# ---------------------------------------------------------------------------
# Provider-level upstream models cache (for multi-key ordered fetch)
# ---------------------------------------------------------------------------


async def _get_provider_upstream_models_cache(provider_id: str) -> list[dict] | None:
    cache_key = f"upstream_models_provider:{provider_id}"
    cached = await CacheService.get(cache_key)
    return cached  # type: ignore[return-value]


async def _set_provider_upstream_models_cache(provider_id: str, models: list[dict]) -> None:
    cache_key = f"upstream_models_provider:{provider_id}"
    await CacheService.set(cache_key, models, UPSTREAM_MODELS_CACHE_TTL_SECONDS)


# ---------------------------------------------------------------------------
# Antigravity: tier / availability sorting for upstream model fetching
# ---------------------------------------------------------------------------

# tier 排序权重（数值越大越优先）
_ANTIGRAVITY_TIER_PRIORITY: dict[str, int] = {"ultra": 3, "pro": 2, "free": 1}


def _antigravity_sort_keys(api_keys: list[Any]) -> list[Any]:
    """按 tier/可用性对 Antigravity Key 降序排列。

    预计算排序键避免排序过程中重复解密。

    排序维度（优先级从高到低）:
    1. 可用性: oauth_invalid_at 为空 = 1（优先）, 非空 = 0
    2. 付费级别: Ultra=3 > Pro=2 > Free=1 > 未知=0
    """
    sort_keys: list[tuple[tuple[int, int], Any]] = []
    for api_key in api_keys:
        availability = 0 if getattr(api_key, "oauth_invalid_at", None) else 1
        tier_weight = 0
        encrypted_auth_config = getattr(api_key, "auth_config", None)
        if encrypted_auth_config:
            try:
                decrypted = crypto_service.decrypt(encrypted_auth_config)
                auth_config = json.loads(decrypted)
                tier = (auth_config.get("tier") or "").lower()
                tier_weight = _ANTIGRAVITY_TIER_PRIORITY.get(tier, 0)
            except Exception:
                pass
        sort_keys.append(((availability, tier_weight), api_key))

    sort_keys.sort(key=lambda x: x[0], reverse=True)
    return [item[1] for item in sort_keys]


# ---------------------------------------------------------------------------
# Key Auth Resolution (shared by multi-key and single-key paths)
# ---------------------------------------------------------------------------


class _KeyAuthError(Exception):
    """Key 认证解析失败（调用方决定是返回错误还是抛 HTTPException）。"""

    def __init__(self, message: str) -> None:
        self.message = message
        super().__init__(message)


async def _resolve_key_auth(
    api_key: Any,
    provider: Any,
    provider_proxy_config: dict[str, Any] | None = None,
) -> tuple[str, dict[str, Any] | None]:
    """统一解析 Key 的 api_key_value 和 auth_config。

    Args:
        api_key: ProviderAPIKey 对象
        provider: Provider 对象
        provider_proxy_config: 已解析的有效代理配置（key > provider 级别）

    Returns:
        (api_key_value, auth_config)

    Raises:
        _KeyAuthError: 解析失败（含可读消息）
    """
    auth_type = str(getattr(api_key, "auth_type", "api_key") or "api_key").lower()
    provider_type = str(getattr(provider, "provider_type", "") or "").lower()

    api_key_value: str | None = None
    auth_config: dict[str, Any] | None = None
    if auth_type == "oauth":
        endpoint_api_format = "gemini:chat" if provider_type == ProviderType.ANTIGRAVITY else None
        try:
            resolved = await resolve_oauth_access_token(
                key_id=str(api_key.id),
                encrypted_api_key=str(api_key.api_key or ""),
                encrypted_auth_config=(
                    str(api_key.auth_config)
                    if getattr(api_key, "auth_config", None) is not None
                    else None
                ),
                provider_proxy_config=provider_proxy_config,
                endpoint_api_format=endpoint_api_format,
            )
            api_key_value = resolved.access_token
            auth_config = resolved.decrypted_auth_config
        except Exception as e:
            logger.error("[provider-query] OAuth auth failed for key {}: {}", api_key.id, e)
            raise _KeyAuthError("oauth auth failed") from e

        if not api_key_value:
            raise _KeyAuthError("oauth token missing")
    else:
        try:
            api_key_value = crypto_service.decrypt(api_key.api_key)
        except Exception as e:
            logger.error("Failed to decrypt API key {}: {}", api_key.id, e)
            raise _KeyAuthError("decrypt failed") from e

        # Best-effort: 解密 auth_config 元数据（如 Antigravity project_id）
        if getattr(api_key, "auth_config", None):
            try:
                decrypted = crypto_service.decrypt(api_key.auth_config)
                parsed = json.loads(decrypted)
                auth_config = parsed if isinstance(parsed, dict) else None
            except Exception:
                auth_config = None

    return api_key_value, auth_config


# ============ Request/Response Models ============


class ModelsQueryRequest(BaseModel):
    """模型列表查询请求"""

    provider_id: str
    api_key_id: str | None = None
    force_refresh: bool = False  # 强制刷新，跳过缓存


class TestModelRequest(BaseModel):
    """模型测试请求"""

    provider_id: str
    model_name: str
    api_key_id: str | None = None
    endpoint_id: str | None = None  # 指定使用的端点ID
    stream: bool = False
    message: str | None = "你好"
    api_format: str | None = None  # 指定使用的API格式，如果不指定则使用端点的默认格式


# ============ API Endpoints ============


@router.post("/models")
async def query_available_models(
    request: ModelsQueryRequest,
    db: Session = Depends(get_db),
    current_user: User = Depends(get_current_user),
) -> Any:
    """
    查询提供商可用模型

    优先从缓存获取（缓存由定时任务刷新），缓存未命中时实时调用上游 API。
    从所有 API 格式尝试获取模型，然后聚合去重。

    行为:
    - 指定 api_key_id: 只获取该 Key 能访问的模型
    - 不指定 api_key_id: 遍历所有活跃的 Key，聚合所有模型（每个 Key 独立缓存）

    Args:
        request: 查询请求

    Returns:
        所有端点的模型列表（合并）
    """
    # 获取提供商基本信息
    provider = (
        db.query(Provider)
        .options(
            joinedload(Provider.endpoints),
            joinedload(Provider.api_keys),
        )
        .filter(Provider.id == request.provider_id)
        .first()
    )

    if not provider:
        raise HTTPException(status_code=404, detail="Provider not found")

    # 构建 api_format -> EndpointFetchConfig 映射（纯数据，不依赖 ORM session）
    format_to_endpoint = build_format_to_config(provider.endpoints)

    if not format_to_endpoint:
        raise HTTPException(status_code=400, detail="No active endpoints found for this provider")

    # 如果指定了 api_key_id，只获取该 Key 的模型
    if request.api_key_id:
        return await _fetch_models_for_single_key(
            provider=provider,
            api_key_id=request.api_key_id,
            format_to_endpoint=format_to_endpoint,
            force_refresh=request.force_refresh,
        )

    # 未指定 api_key_id，遍历所有活跃的 Key 并聚合结果
    active_keys = [key for key in provider.api_keys if key.is_active]
    if not active_keys:
        raise HTTPException(status_code=400, detail="No active API Key found for this provider")

    # Antigravity: 按 tier/可用性排序后逐个尝试，成功即停止
    provider_type = str(getattr(provider, "provider_type", "") or "").lower()
    if provider_type == ProviderType.ANTIGRAVITY:
        return await _fetch_models_antigravity_ordered(
            provider=provider,
            active_keys=active_keys,
            format_to_endpoint=format_to_endpoint,
            force_refresh=request.force_refresh,
        )

    # 其他类型: 并发获取所有 Key 的模型
    async def fetch_for_key(api_key: Any) -> Any:
        # 非强制刷新时，先检查缓存
        if not request.force_refresh:
            cached_models = await get_upstream_models_from_cache(request.provider_id, api_key.id)
            if cached_models is not None:
                return cached_models, None, True  # models, error, from_cache

        # 缓存未命中或强制刷新，实时获取
        try:
            effective_proxy = resolve_effective_proxy(
                getattr(provider, "proxy", None), getattr(api_key, "proxy", None)
            )
            api_key_value, auth_config = await _resolve_key_auth(
                api_key, provider, provider_proxy_config=effective_proxy
            )
        except _KeyAuthError as e:
            return [], f"Key {api_key.name or api_key.id}: {e.message}", False

        fetch_ctx = UpstreamModelsFetchContext(
            provider_type=str(getattr(provider, "provider_type", "") or ""),
            api_key_value=str(api_key_value or ""),
            format_to_endpoint=format_to_endpoint,
            proxy_config=effective_proxy,
            auth_config=auth_config,
        )
        models, errors, has_success, _meta = await fetch_models_for_key(
            fetch_ctx, timeout_seconds=MODEL_FETCH_HTTP_TIMEOUT
        )

        # 写入缓存（按 model id 聚合，保证返回 api_formats 数组，避免前端 schema 不一致）
        unique_models = _aggregate_models_by_id([m for m in models if isinstance(m, dict)])
        if unique_models:
            await set_upstream_models_to_cache(request.provider_id, api_key.id, unique_models)

        error = f"Key {api_key.name or api_key.id}: {'; '.join(errors)}" if errors else None
        return unique_models, error, False  # models, error, from_cache

    # 并发执行所有 Key 的获取
    results = await asyncio.gather(*[fetch_for_key(key) for key in active_keys])

    # 合并结果
    all_models: list = []
    all_errors: list[str] = []
    cache_hit_count = 0
    fetch_count = 0
    for models, error, from_cache in results:
        all_models.extend(models)
        if error:
            all_errors.append(error)
        if from_cache:
            cache_hit_count += 1
        else:
            fetch_count += 1

    # 按 model id 聚合，合并所有 api_format 到 api_formats 数组
    unique_models = _aggregate_models_by_id(all_models)

    error = "; ".join(all_errors) if all_errors else None
    if not unique_models and not error:
        error = "No models returned from any key"

    return {
        "success": len(unique_models) > 0,
        "data": {
            "models": unique_models,
            "error": error,
            "from_cache": fetch_count == 0 and cache_hit_count > 0,
            "keys_total": len(active_keys),
            "keys_cached": cache_hit_count,
            "keys_fetched": fetch_count,
        },
        "provider": {
            "id": provider.id,
            "name": provider.name,
        },
    }


def _aggregate_models_by_id(models: list[dict]) -> list[dict]:
    """
    按 model id 聚合模型，合并所有 api_format 到 api_formats 数组

    支持两种输入格式:
    - 原始模型: 有 api_format (singular) 字段
    - 已聚合模型: 有 api_formats (array) 字段（来自缓存）

    Args:
        models: 模型列表，每个模型可能有 api_format 或 api_formats 字段

    Returns:
        聚合后的模型列表，每个模型有 api_formats 数组
    """
    model_map: dict[str, dict] = {}

    for model in models:
        model_id = model.get("id")
        if not model_id:
            continue

        # 支持两种格式：api_format (singular) 或 api_formats (array)
        api_format = model.get("api_format", "")
        existing_formats = model.get("api_formats") or []

        if model_id not in model_map:
            # 第一次遇到这个模型，复制基础信息
            aggregated = {
                "id": model_id,
                "api_formats": [],
            }
            # 复制其他字段（排除 api_format 和 api_formats）
            for key, value in model.items():
                if key not in ("id", "api_format", "api_formats"):
                    aggregated[key] = value
            model_map[model_id] = aggregated

        # 添加 api_format 到列表（避免重复）
        if api_format and api_format not in model_map[model_id]["api_formats"]:
            model_map[model_id]["api_formats"].append(api_format)

        # 添加已有的 api_formats（处理缓存的聚合数据）
        for fmt in existing_formats:
            if fmt and fmt not in model_map[model_id]["api_formats"]:
                model_map[model_id]["api_formats"].append(fmt)

    # 对每个模型的 api_formats 排序
    result = list(model_map.values())
    for model in result:
        model["api_formats"].sort()

    # 按 model id 排序
    result.sort(key=lambda m: m["id"])
    return result


async def _fetch_models_antigravity_ordered(
    provider: Provider,
    active_keys: list[Any],
    format_to_endpoint: dict[str, EndpointFetchConfig],
    force_refresh: bool,
) -> Any:
    """Antigravity: 按账号 tier/可用性排序后逐个尝试获取上游模型，成功即停止。

    排序规则（降序）:
    1. 可用性: 无 oauth_invalid_at 的账号优先
    2. 付费级别: Ultra > Pro > Free
    """
    sorted_keys = _antigravity_sort_keys(active_keys)

    # 非强制刷新时，先检查 Provider 级别缓存
    if not force_refresh:
        cached_models = await _get_provider_upstream_models_cache(provider.id)
        if cached_models is not None:
            safe_models = [m for m in cached_models if isinstance(m, dict)]
            unique_models = _aggregate_models_by_id(safe_models)
            if unique_models:
                logger.info(
                    "Antigravity 上游模型命中 Provider 缓存: provider={}, models={}",
                    provider.name,
                    len(unique_models),
                )
                return {
                    "success": True,
                    "data": {
                        "models": unique_models,
                        "error": None,
                        "from_cache": True,
                        "keys_total": len(active_keys),
                        "keys_cached": 1,
                        "keys_fetched": 0,
                    },
                    "provider": {"id": provider.id, "name": provider.name},
                }

    all_errors: list[str] = []

    for api_key in sorted_keys:
        key_label = api_key.name or api_key.id

        # 实时获取
        try:
            effective_proxy = resolve_effective_proxy(
                getattr(provider, "proxy", None), getattr(api_key, "proxy", None)
            )
            api_key_value, auth_config = await _resolve_key_auth(
                api_key, provider, provider_proxy_config=effective_proxy
            )
        except _KeyAuthError as e:
            all_errors.append(f"Key {key_label}: {e.message}")
            continue

        fetch_ctx = UpstreamModelsFetchContext(
            provider_type=str(getattr(provider, "provider_type", "") or ""),
            api_key_value=str(api_key_value or ""),
            format_to_endpoint=format_to_endpoint,
            proxy_config=effective_proxy,
            auth_config=auth_config,
        )
        models, errors, has_success, _meta = await fetch_models_for_key(
            fetch_ctx, timeout_seconds=MODEL_FETCH_HTTP_TIMEOUT
        )

        if not has_success:
            err = f"Key {key_label}: {'; '.join(errors)}" if errors else f"Key {key_label}: failed"
            all_errors.append(err)
            logger.info("Antigravity 上游模型获取失败, 尝试下一个账号: {}", err)
            continue

        # 成功: 聚合并写入 Provider 级别缓存
        unique_models = _aggregate_models_by_id([m for m in models if isinstance(m, dict)])
        if unique_models:
            await _set_provider_upstream_models_cache(provider.id, unique_models)

        logger.info(
            "Antigravity 上游模型获取成功: key={}, models={}",
            key_label,
            len(unique_models),
        )
        return {
            "success": len(unique_models) > 0,
            "data": {
                "models": unique_models,
                "error": None,
                "from_cache": False,
                "keys_total": len(active_keys),
                "keys_cached": 0,
                "keys_fetched": 1,
            },
            "provider": {"id": provider.id, "name": provider.name},
        }

    # 所有 Key 均失败
    error = "; ".join(all_errors) if all_errors else "All keys failed"
    return {
        "success": False,
        "data": {
            "models": [],
            "error": error,
            "from_cache": False,
            "keys_total": len(active_keys),
            "keys_cached": 0,
            "keys_fetched": len(all_errors),
        },
        "provider": {"id": provider.id, "name": provider.name},
    }


async def _fetch_models_for_single_key(
    provider: Provider,
    api_key_id: str,
    format_to_endpoint: dict[str, EndpointFetchConfig],
    force_refresh: bool,
) -> Any:
    """获取单个 Key 的模型列表"""
    # 查找指定的 Key
    api_key = next((key for key in provider.api_keys if key.id == api_key_id), None)
    if not api_key:
        raise HTTPException(status_code=404, detail="API Key not found")

    # 非强制刷新时，优先从缓存获取
    if not force_refresh:
        cached_models = await get_upstream_models_from_cache(provider.id, api_key_id)
        if cached_models is not None:
            safe_models = [m for m in cached_models if isinstance(m, dict)]
            unique_cached = _aggregate_models_by_id(safe_models)
            # 修复遗留缓存格式（以前可能缓存了未聚合的 api_format 版本）
            if unique_cached and (
                not safe_models or "api_formats" not in safe_models[0]  # type: ignore[operator]
            ):
                await set_upstream_models_to_cache(provider.id, api_key_id, unique_cached)
            return {
                "success": True,
                "data": {"models": unique_cached, "error": None, "from_cache": True},
                "provider": {
                    "id": provider.id,
                    "name": provider.name,
                },
            }

    # 缓存未命中或强制刷新，实时获取
    try:
        effective_proxy = resolve_effective_proxy(
            getattr(provider, "proxy", None), getattr(api_key, "proxy", None)
        )
        api_key_value, auth_config = await _resolve_key_auth(
            api_key, provider, provider_proxy_config=effective_proxy
        )
    except _KeyAuthError as e:
        raise HTTPException(status_code=500, detail=e.message)

    fetch_ctx = UpstreamModelsFetchContext(
        provider_type=str(getattr(provider, "provider_type", "") or ""),
        api_key_value=str(api_key_value or ""),
        format_to_endpoint=format_to_endpoint,
        proxy_config=effective_proxy,
        auth_config=auth_config,
    )
    all_models, errors, has_success, _meta = await fetch_models_for_key(
        fetch_ctx, timeout_seconds=MODEL_FETCH_HTTP_TIMEOUT
    )

    # 按 model id 聚合，合并所有 api_format
    unique_models = _aggregate_models_by_id(all_models)

    error = "; ".join(errors) if errors else None
    if not unique_models and not error:
        error = "No models returned from any endpoint"

    # 获取成功时写入缓存
    if unique_models:
        await set_upstream_models_to_cache(provider.id, api_key_id, unique_models)

    return {
        "success": len(unique_models) > 0,
        "data": {"models": unique_models, "error": error, "from_cache": False},
        "provider": {
            "id": provider.id,
            "name": provider.name,
        },
    }


@router.post("/test-model")
async def test_model(
    request: TestModelRequest,
    db: Session = Depends(get_db),
    current_user: User = Depends(get_current_user),
) -> Any:
    """
    测试模型连接性

    向指定提供商的指定模型发送测试请求，验证模型是否可用
    """
    # 获取提供商及其端点和 Keys
    provider = (
        db.query(Provider)
        .options(
            joinedload(Provider.endpoints),
            joinedload(Provider.api_keys),
        )
        .filter(Provider.id == request.provider_id)
        .first()
    )

    if not provider:
        raise HTTPException(status_code=404, detail="Provider not found")

    # 构建 api_format -> endpoint 映射 和 id -> endpoint 映射
    format_to_endpoint: dict[str, ProviderEndpoint] = {}
    id_to_endpoint: dict[str, ProviderEndpoint] = {}
    for ep in provider.endpoints:
        if ep.is_active:
            format_to_endpoint[ep.api_format] = ep
            id_to_endpoint[ep.id] = ep

    # 找到合适的端点和 API Key
    endpoint = None
    api_key = None

    # 优先级: api_format > endpoint_id > api_key_id > 自动选择
    # 如果指定了 api_format，优先使用该格式对应的 endpoint
    if request.api_format:
        endpoint = format_to_endpoint.get(request.api_format)
        if not endpoint:
            raise HTTPException(
                status_code=404,
                detail=f"No active endpoint found for API format: {request.api_format}",
            )

        if request.api_key_id:
            # 使用指定的 Key，但需要校验是否支持该格式
            api_key = next(
                (
                    key
                    for key in provider.api_keys
                    if key.id == request.api_key_id and key.is_active
                ),
                None,
            )
            if api_key and request.api_format not in (api_key.api_formats or []):
                raise HTTPException(
                    status_code=400, detail=f"API Key does not support format: {request.api_format}"
                )
        else:
            # 找支持该格式的第一个可用 Key
            for key in provider.api_keys:
                if not key.is_active:
                    continue
                if request.api_format in (key.api_formats or []):
                    api_key = key
                    break
    elif request.endpoint_id:
        # 使用指定的端点
        endpoint = id_to_endpoint.get(request.endpoint_id)
        if not endpoint:
            raise HTTPException(status_code=404, detail="Endpoint not found or not active")

        if request.api_key_id:
            # 同时指定了 Key，需要校验是否支持该端点格式
            api_key = next(
                (
                    key
                    for key in provider.api_keys
                    if key.id == request.api_key_id and key.is_active
                ),
                None,
            )
            if api_key and endpoint.api_format not in (api_key.api_formats or []):
                raise HTTPException(
                    status_code=400,
                    detail=f"API Key does not support endpoint format: {endpoint.api_format}",
                )
        else:
            # 找支持该端点格式的第一个可用 Key
            for key in provider.api_keys:
                if not key.is_active:
                    continue
                if endpoint.api_format in (key.api_formats or []):
                    api_key = key
                    break
    elif request.api_key_id:
        # 使用指定的 API Key
        api_key = next(
            (key for key in provider.api_keys if key.id == request.api_key_id and key.is_active),
            None,
        )
        if api_key:
            # 找到该 Key 支持的第一个活跃 Endpoint
            for fmt in api_key.api_formats or []:
                if fmt in format_to_endpoint:
                    endpoint = format_to_endpoint[fmt]
                    break
    else:
        # 使用第一个可用的端点和密钥
        for ep in provider.endpoints:
            if not ep.is_active:
                continue
            # 找支持该格式的第一个可用 Key
            for key in provider.api_keys:
                if not key.is_active:
                    continue
                if ep.api_format in (key.api_formats or []):
                    endpoint = ep
                    api_key = key
                    break
            if endpoint:
                break

    if not endpoint or not api_key:
        raise HTTPException(status_code=404, detail="No active endpoint or API key found")

    auth_type = str(getattr(api_key, "auth_type", "api_key") or "api_key").lower()

    try:
        if auth_type == "oauth":
            resolved = await resolve_oauth_access_token(
                key_id=str(api_key.id),
                encrypted_api_key=str(api_key.api_key or ""),
                encrypted_auth_config=(
                    str(api_key.auth_config) if getattr(api_key, "auth_config", None) else None
                ),
                provider_proxy_config=resolve_effective_proxy(
                    getattr(provider, "proxy", None), getattr(api_key, "proxy", None)
                ),
                endpoint_api_format=str(getattr(endpoint, "api_format", "") or ""),
            )
            api_key_value = resolved.access_token
            oauth_meta = resolved.decrypted_auth_config or {}
            if not api_key_value:
                raise HTTPException(status_code=500, detail="OAuth token missing")
        else:
            api_key_value = crypto_service.decrypt(api_key.api_key)
            oauth_meta = {}
    except HTTPException:
        raise
    except Exception as e:
        logger.error(f"[test-model] Failed to resolve API key: {e}")
        raise HTTPException(status_code=500, detail="Failed to resolve API key")

    # 构建请求配置
    extra_headers = get_extra_headers_from_endpoint(endpoint) or {}

    # OAuth 认证：Codex 需要 chatgpt-account-id
    if auth_type == "oauth":
        try:
            account_id = oauth_meta.get("account_id")
            if account_id:
                extra_headers["chatgpt-account-id"] = str(account_id)
                logger.debug("[test-model] Added chatgpt-account-id header: {}", account_id)
        except Exception as e:
            logger.warning("[test-model] Failed to apply OAuth extra headers: {}", e)

    endpoint_config = {
        "api_key": api_key_value,
        "api_key_id": api_key.id,  # 添加API Key ID用于用量记录
        "base_url": endpoint.base_url,
        "api_format": endpoint.api_format,
        "extra_headers": extra_headers if extra_headers else None,
        "timeout": TimeoutDefaults.HTTP_REQUEST,
    }

    try:
        # 获取对应的 Adapter 类
        adapter_class = get_adapter_for_format(endpoint.api_format)
        if not adapter_class:
            return {
                "success": False,
                "error": f"Unknown API format: {endpoint.api_format}",
                "provider": {
                    "id": provider.id,
                    "name": provider.name,
                },
                "model": request.model_name,
            }

        logger.debug(f"[test-model] 使用 Adapter: {adapter_class.__name__}")
        logger.debug(f"[test-model] 端点 API Format: {endpoint.api_format}")
        logger.debug(f"[test-model] 使用 Key: {api_key.name or api_key.id} (auth_type={auth_type})")

        # 准备测试请求数据（优先使用流式）
        check_request = {
            "model": request.model_name,
            "messages": [
                {"role": "user", "content": request.message or "Hello! This is a test message."}
            ],
            "max_tokens": 30,
            "temperature": 0.7,
            "stream": True,
        }

        # 获取端点规则（不在此处应用，传递给 check_endpoint 在格式转换后应用）
        body_rules = getattr(endpoint, "body_rules", None)
        header_rules = getattr(endpoint, "header_rules", None)
        extra_headers = endpoint_config.get("extra_headers") or {}

        if body_rules:
            logger.debug(f"[test-model] 将传递 body_rules 给 check_endpoint: {body_rules}")
        if header_rules:
            logger.debug(f"[test-model] 将传递 header_rules 给 check_endpoint: {header_rules}")

        # 发送测试请求（使用代理配置）
        test_proxy = resolve_effective_proxy(
            getattr(provider, "proxy", None), getattr(api_key, "proxy", None)
        )
        test_proxy_param = resolve_proxy_param(test_proxy)

        logger.debug("[test-model] 开始端点测试...")

        # Provider 上下文：auth_type 用于 OAuth 认证头处理，provider_type 用于特殊路由
        p_type = str(getattr(provider, "provider_type", "") or "").lower()

        async def _do_check(req: dict) -> dict:
            return await adapter_class.check_endpoint(
                None,  # client 参数已不被 run_endpoint_check 使用
                endpoint_config["base_url"],
                endpoint_config["api_key"],
                req,
                extra_headers if extra_headers else None,
                body_rules=body_rules,
                header_rules=header_rules,
                db=db,
                user=current_user,
                provider_name=provider.name,
                provider_id=provider.id,
                api_key_id=endpoint_config.get("api_key_id"),
                model_name=request.model_name,
                auth_type=auth_type,
                provider_type=p_type if p_type else None,
                decrypted_auth_config=oauth_meta if oauth_meta else None,
                proxy_param=test_proxy_param,
            )

        def _response_has_error(resp: dict) -> bool:
            """快速判断响应是否包含错误"""
            if "error" in resp:
                return True
            if resp.get("status_code", 0) != 200:
                return True
            resp_data = resp.get("response", {})
            resp_body = resp_data.get("response_body", {})
            parsed = resp_body
            if isinstance(resp_body, str):
                try:
                    parsed = json.loads(resp_body)
                except (json.JSONDecodeError, ValueError):
                    pass
            if isinstance(parsed, dict) and "error" in parsed:
                return True
            return False

        # 策略：优先流式，若失败回退到非流式
        used_stream = True
        logger.debug("[test-model] 尝试流式请求...")
        response = await _do_check(check_request)

        if _response_has_error(response):
            logger.info(
                "[test-model] 流式请求失败 (status={})，回退到非流式请求",
                response.get("status_code", "?"),
            )
            check_request["stream"] = False
            used_stream = False
            response = await _do_check(check_request)

        # 记录提供商返回信息
        logger.debug("[test-model] 端点测试结果:")
        logger.debug(f"[test-model] Status Code: {response.get('status_code')}")
        logger.debug(f"[test-model] Response Headers: {response.get('headers', {})}")
        response_data = response.get("response", {})
        response_body = response_data.get("response_body", {})
        logger.debug(f"[test-model] Response Data: {response_data}")
        logger.debug(f"[test-model] Response Body: {response_body}")
        # 尝试解析 response_body (通常是 JSON 字符串)
        parsed_body = response_body
        from src.utils import json_helper as json

        if isinstance(response_body, str):
            try:
                parsed_body = json.loads(response_body)
            except json.JSONDecodeError:
                pass

        if isinstance(parsed_body, dict) and "error" in parsed_body:
            error_obj = parsed_body["error"]
            # 兼容 error 可能是字典或字符串的情况
            if isinstance(error_obj, dict):
                error_message = error_obj.get("message", "")
                logger.debug(f"[test-model] Error Message: {error_message}")

                # Antigravity 403 "verify your account" → 标记账号异常
                if (
                    api_key
                    and auth_type == "oauth"
                    and error_obj.get("code") == 403
                    and (
                        "verify" in error_message.lower()
                        or "permission" in str(error_obj.get("status", "")).lower()
                    )
                ):
                    from datetime import datetime, timezone

                    from src.services.provider.oauth_token import (
                        OAUTH_ACCOUNT_BLOCK_PREFIX,
                    )

                    api_key.oauth_invalid_at = datetime.now(timezone.utc)
                    api_key.oauth_invalid_reason = (
                        f"{OAUTH_ACCOUNT_BLOCK_PREFIX}Google 要求验证账号"
                    )
                    api_key.is_active = False
                    db.commit()
                    oauth_email = None
                    if getattr(api_key, "auth_config", None):
                        try:
                            decrypted = crypto_service.decrypt(api_key.auth_config)
                            parsed = json.loads(decrypted)
                            if isinstance(parsed, dict):
                                email_val = parsed.get("email")
                                if isinstance(email_val, str) and email_val.strip():
                                    oauth_email = email_val.strip()
                        except Exception:
                            oauth_email = None
                    if oauth_email:
                        logger.warning(
                            "[test-model] Key {} (email={}) 因 403 verify 已标记为异常",
                            api_key.id,
                            oauth_email,
                        )
                    else:
                        logger.warning("[test-model] Key {} 因 403 verify 已标记为异常", api_key.id)

                raise HTTPException(
                    status_code=500,
                    detail=str(error_message)[:500] if error_message else "Provider error",
                )
            else:
                logger.debug(f"[test-model] Error: {error_obj}")
                # error_obj 可能是字符串，截断以避免泄露过多上游信息
                raise HTTPException(
                    status_code=500,
                    detail=str(error_obj)[:500] if error_obj else "Provider error",
                )
        elif "error" in response:
            logger.debug(f"[test-model] Error: {response['error']}")
            raise HTTPException(
                status_code=500,
                detail=str(response["error"])[:500],
            )
        else:
            # 如果有选择或消息，记录内容预览
            if isinstance(response_data, dict):
                if "choices" in response_data and response_data["choices"]:
                    choice = response_data["choices"][0]
                    if "message" in choice:
                        content = choice["message"].get("content", "")
                        logger.debug(f"[test-model] Content Preview: {content[:200]}...")
                elif "content" in response_data and response_data["content"]:
                    content = str(response_data["content"])
                    logger.debug(f"[test-model] Content Preview: {content[:200]}...")

        # 检查测试是否成功（基于HTTP状态码）
        status_code = response.get("status_code", 0)
        is_success = status_code == 200 and "error" not in response

        return {
            "success": is_success,
            "data": {
                "stream": used_stream,
                "response": response,
            },
            "provider": {
                "id": provider.id,
                "name": provider.name,
            },
            "model": request.model_name,
            "endpoint": {
                "id": endpoint.id,
                "api_format": endpoint.api_format,
                "base_url": endpoint.base_url,
            },
        }

    except Exception as e:
        logger.error(f"[test-model] Error testing model {request.model_name}: {e}")
        return {
            "success": False,
            "error": str(e),
            "provider": {
                "id": provider.id,
                "name": provider.name,
            },
            "model": request.model_name,
            "endpoint": (
                {
                    "id": endpoint.id,
                    "api_format": endpoint.api_format,
                    "base_url": endpoint.base_url,
                }
                if endpoint
                else None
            ),
        }
