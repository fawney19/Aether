"""
Provider Query API 端点
用于查询提供商的模型列表等信息
"""

from __future__ import annotations

import asyncio
import json
import time
from typing import TYPE_CHECKING, Any

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
    UpstreamModelsFetcherRegistry,
    build_format_to_config,
    fetch_models_for_key,
    get_adapter_for_format,
)
from src.services.provider.oauth_token import resolve_oauth_access_token
from src.services.proxy_node.resolver import resolve_effective_proxy
from src.utils.auth_utils import get_current_user

if TYPE_CHECKING:
    from src.services.scheduling.schemas import ProviderCandidate

router = APIRouter(prefix="/api/admin/provider-query", tags=["Provider Query"])

_ADMIN_TEST_CONCURRENCY = 8
_admin_test_semaphore = asyncio.Semaphore(max(_ADMIN_TEST_CONCURRENCY, 1))


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


class TestModelFailoverRequest(BaseModel):
    """带故障转移的模型测试请求"""

    provider_id: str
    mode: str  # "global" = 模拟外部请求(用全局模型名), "direct" = 直接测试(用provider_model_name)
    model_name: str  # global 模式传 global_model_name, direct 模式传 provider_model_name
    api_format: str | None = None  # 指定 API 格式（endpoint signature）
    endpoint_id: str | None = None  # 指定仅使用该端点测试
    message: str | None = "Hello"


class TestAttemptDetail(BaseModel):
    """单次测试尝试的详情"""

    candidate_index: int
    endpoint_api_format: str
    endpoint_base_url: str
    key_name: str | None = None
    key_id: str
    auth_type: str
    effective_model: str | None = None  # 实际发送的模型名（映射后）
    status: str  # "success" | "failed" | "skipped"
    skip_reason: str | None = None
    error_message: str | None = None
    status_code: int | None = None
    latency_ms: int | None = None


class TestModelFailoverResponse(BaseModel):
    """带故障转移的模型测试响应"""

    success: bool
    model: str
    provider: dict[str, str]
    attempts: list[TestAttemptDetail]
    total_candidates: int
    total_attempts: int
    data: dict | None = None
    error: str | None = None


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

    # 检查是否有注册自定义 fetcher（如预设模型），有则不依赖活跃 endpoint
    provider_type = str(getattr(provider, "provider_type", "") or "").lower()
    # 延迟导入避免循环依赖（与 upstream_fetcher.fetch_models_for_key 保持一致）
    from src.services.provider.envelope import ensure_providers_bootstrapped

    ensure_providers_bootstrapped()
    has_custom_fetcher = UpstreamModelsFetcherRegistry.get(provider_type) is not None

    if not format_to_endpoint and not has_custom_fetcher:
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
    # 测试不依赖端点启用状态，禁用的端点也可以用于测试连通性
    format_to_endpoint: dict[str, ProviderEndpoint] = {}
    id_to_endpoint: dict[str, ProviderEndpoint] = {}
    for ep in provider.endpoints:
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
                detail=f"No endpoint found for API format: {request.api_format}",
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
            raise HTTPException(status_code=404, detail="Endpoint not found")

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

        # 准备测试请求数据（默认非流式，显式请求时才走流式）
        check_request = {
            "model": request.model_name,
            "messages": [
                {"role": "user", "content": request.message or "Hello! This is a test message."}
            ],
            "max_tokens": 30,
            "temperature": 0.7,
            "stream": bool(request.stream),
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
                provider_endpoint=endpoint,
                provider_api_key=api_key,
                proxy_config=test_proxy,
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

        def _extract_error_message(resp: dict) -> str:
            """从 check 响应中提取错误信息（用于判断是否值得回退）。"""
            resp_data = resp.get("response", {}) if isinstance(resp, dict) else {}
            body = resp_data.get("response_body", {})
            parsed = body
            if isinstance(body, str):
                try:
                    parsed = json.loads(body)
                except (json.JSONDecodeError, ValueError):
                    parsed = body

            if isinstance(parsed, dict):
                err = parsed.get("error")
                if isinstance(err, dict):
                    msg = err.get("message")
                    if isinstance(msg, str):
                        return msg
                if isinstance(err, str):
                    return err

            err_raw = resp.get("error")
            if isinstance(err_raw, str):
                return err_raw
            if isinstance(err_raw, dict):
                msg = err_raw.get("message")
                if isinstance(msg, str):
                    return msg
            return ""

        def _should_fallback_to_non_stream(resp: dict) -> bool:
            """仅在“流式特有失败”时回退到非流式，避免 429/鉴权错误的无效重试。"""
            status = int(resp.get("status_code") or 0)
            if status in {404, 405, 415, 501}:
                return True

            if status == 400:
                msg = _extract_error_message(resp).lower()
                stream_markers = ("stream", "sse", "streamgeneratecontent")
                unsupported_markers = ("not support", "unsupported", "invalid argument")
                if any(k in msg for k in stream_markers) and any(
                    k in msg for k in unsupported_markers
                ):
                    return True

            return False

        # 策略：默认非流式；仅显式流式请求时在流式特有失败下回退到非流式
        used_stream = bool(check_request.get("stream"))
        fallback_to_non_stream = False
        logger.info(
            "[admin-test-model] start | provider_id={} provider={} endpoint_id={} api_key_id={} api_format={} model={} stream={} auth_type={}",
            provider.id,
            provider.name,
            getattr(endpoint, "id", None),
            api_key.id,
            endpoint.api_format,
            request.model_name,
            used_stream,
            auth_type,
        )
        response = await _do_check(check_request)

        if (
            used_stream
            and _response_has_error(response)
            and _should_fallback_to_non_stream(response)
        ):
            fallback_to_non_stream = True
            logger.info(
                "[admin-test-model] fallback-to-non-stream | provider_id={} endpoint_id={} api_key_id={} api_format={} model={} status_code={} error={}",
                provider.id,
                getattr(endpoint, "id", None),
                api_key.id,
                endpoint.api_format,
                request.model_name,
                response.get("status_code", "?"),
                _extract_error_message(response)[:200],
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
        logger.info(
            "[admin-test-model] finish | provider_id={} endpoint_id={} api_key_id={} api_format={} model={} stream={} fallback_to_non_stream={} local_protocol_error={} status_code={} has_error={}",
            provider.id,
            getattr(endpoint, "id", None),
            api_key.id,
            endpoint.api_format,
            request.model_name,
            used_stream,
            fallback_to_non_stream,
            "LocalProtocolError" in (str(response.get("error") or "") + str(response.get("response") or "")),
            response.get("status_code", 0),
            _response_has_error(response),
        )
        # 尝试解析 response_body (通常是 JSON 字符串)
        parsed_body = response_body
        import json

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

                upstream_status = int(
                    response.get("status_code", 0) or error_obj.get("code", 0) or 500
                )
                if not (400 <= upstream_status <= 599):
                    upstream_status = 500
                raise HTTPException(
                    status_code=upstream_status,
                    detail=str(error_message)[:500] if error_message else "Provider error",
                )
            else:
                logger.debug(f"[test-model] Error: {error_obj}")
                # error_obj 可能是字符串，截断以避免泄露过多上游信息
                upstream_status = int(response.get("status_code", 0) or 500)
                if not (400 <= upstream_status <= 599):
                    upstream_status = 500
                raise HTTPException(
                    status_code=upstream_status,
                    detail=str(error_obj)[:500] if error_obj else "Provider error",
                )
        elif "error" in response:
            logger.debug(f"[test-model] Error: {response['error']}")
            upstream_status = int(response.get("status_code", 0) or 500)
            if not (400 <= upstream_status <= 599):
                upstream_status = 500
            raise HTTPException(
                status_code=upstream_status,
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


# ---------------------------------------------------------------------------
# 带故障转移的模型测试
# ---------------------------------------------------------------------------


def _build_direct_test_candidates(
    provider: Provider,
    api_format: str | None = None,
    endpoint_id: str | None = None,
) -> list[ProviderCandidate]:
    """
    为直接测试模式构建候选列表。

    遍历 Provider 的活跃 Endpoint 和 Key，不经过 GlobalModel 解析。
    """
    from src.services.scheduling.schemas import ProviderCandidate

    candidates: list[ProviderCandidate] = []
    for endpoint in provider.endpoints or []:
        if endpoint_id and str(getattr(endpoint, "id", "") or "") != str(endpoint_id):
            continue
        if not getattr(endpoint, "is_active", False):
            continue
        ep_format = str(getattr(endpoint, "api_format", "") or "")
        if not ep_format:
            continue
        if api_format and ep_format != api_format:
            continue

        for key in provider.api_keys or []:
            if not getattr(key, "is_active", False):
                continue
            key_formats = getattr(key, "api_formats", None)
            if key_formats is not None and ep_format not in key_formats:
                continue

            candidates.append(
                ProviderCandidate(
                    provider=provider,
                    endpoint=endpoint,
                    key=key,
                    is_skipped=False,
                    provider_api_format=ep_format,
                )
            )
    return candidates


def _filter_test_candidates_by_endpoint(
    candidates: list[ProviderCandidate],
    endpoint_id: str | None,
) -> list[ProviderCandidate]:
    if not endpoint_id:
        return list(candidates)

    target_id = str(endpoint_id)
    return [
        candidate
        for candidate in candidates
        if str(getattr(getattr(candidate, "endpoint", None), "id", "") or "") == target_id
    ]


@router.post("/test-model-failover")
async def test_model_failover(
    request: TestModelFailoverRequest,
    db: Session = Depends(get_db),
    current_user: User = Depends(get_current_user),
) -> Any:
    """
    带故障转移的模型测试

    支持两种模式:
    - global: 模拟外部请求，用全局模型名走候选解析（限定当前 Provider）
    - direct: 直接测试 provider_model_name，在当前 Provider 内多 Key 故障转移
    """
    from src.services.candidate.failover import FailoverEngine
    from src.services.candidate.policy import RetryMode, RetryPolicy, SkipPolicy
    from src.services.task.protocol import AttemptKind, AttemptResult

    # 1. 加载 Provider
    provider = (
        db.query(Provider)
        .options(
            joinedload(Provider.endpoints),
            joinedload(Provider.api_keys),
            joinedload(Provider.models),
        )
        .filter(Provider.id == request.provider_id)
        .first()
    )
    if not provider:
        raise HTTPException(status_code=404, detail="Provider not found")

    if request.mode not in ("global", "direct"):
        raise HTTPException(status_code=400, detail="mode must be 'global' or 'direct'")

    # 2. 构建候选列表
    candidates = []
    gm_obj = None  # GlobalModel 对象，global 模式下用于 fallback 映射
    endpoint_by_id = {
        str(getattr(ep, "id", "") or ""): ep
        for ep in (provider.endpoints or [])
        if getattr(ep, "id", None)
    }
    requested_endpoint = None
    if request.endpoint_id:
        requested_endpoint = endpoint_by_id.get(str(request.endpoint_id))
        if requested_endpoint is None:
            raise HTTPException(status_code=404, detail="Endpoint not found")
        ep_format = str(getattr(requested_endpoint, "api_format", "") or "")
        if request.api_format and ep_format != request.api_format:
            raise HTTPException(status_code=400, detail="endpoint_id does not match api_format")

    if request.mode == "global":
        # 模拟外部请求：走 CandidateBuilder 候选解析
        from src.services.scheduling.candidate_builder import CandidateBuilder
        from src.services.scheduling.candidate_sorter import CandidateSorter
        from src.services.scheduling.scheduling_config import SchedulingConfig

        sorter = CandidateSorter(SchedulingConfig())
        builder = CandidateBuilder(sorter)

        # 确定 client_format
        client_format = request.api_format
        if not client_format and requested_endpoint is not None:
            client_format = str(getattr(requested_endpoint, "api_format", "") or "")
        if not client_format:
            # 取第一个活跃端点的格式
            for ep in provider.endpoints or []:
                if getattr(ep, "is_active", False):
                    client_format = str(getattr(ep, "api_format", "") or "")
                    if client_format:
                        break
        if not client_format:
            raise HTTPException(
                status_code=400, detail="No active endpoint found to determine API format"
            )

        # 从 GlobalModel 提取 model_mappings（正则映射规则，用于 Key.allowed_models 匹配）
        from src.services.cache.model_cache import ModelCacheService

        model_mappings: list[str] = []
        gm_obj = None
        try:
            gm_obj = await ModelCacheService.get_global_model_by_name(db, request.model_name)
            if gm_obj and isinstance(gm_obj.config, dict):
                raw_mappings = gm_obj.config.get("model_mappings", [])
                if isinstance(raw_mappings, list):
                    model_mappings = raw_mappings
        except Exception as e:
            logger.warning("[test-model-failover] Failed to get GlobalModel mappings: {}", e)

        try:
            candidates = await builder._build_candidates(
                db=db,
                providers=[provider],
                client_format=client_format,
                model_name=request.model_name,
                model_mappings=model_mappings if model_mappings else None,
                affinity_key=None,
                is_stream=False,
            )
        except Exception as e:
            logger.warning("[test-model-failover] CandidateBuilder failed: {}", e)
            candidates = []
        candidates = _filter_test_candidates_by_endpoint(candidates, request.endpoint_id)
    else:
        # 直接测试：简单匹配 Endpoint + Key
        candidates = _build_direct_test_candidates(
            provider=provider,
            api_format=request.api_format,
            endpoint_id=request.endpoint_id,
        )

    if not candidates:
        return TestModelFailoverResponse(
            success=False,
            model=request.model_name,
            provider={"id": str(provider.id), "name": provider.name},
            attempts=[],
            total_candidates=0,
            total_attempts=0,
            error="No available candidates found for this model",
        ).model_dump()

    # 3. 定义 attempt_func
    attempts: list[TestAttemptDetail] = []
    p_type = str(getattr(provider, "provider_type", "") or "").lower()

    async def _attempt_func(candidate: Any) -> AttemptResult:
        start_time = time.monotonic()
        endpoint = candidate.endpoint
        key = candidate.key
        candidate_idx = getattr(candidate, "_utf_candidate_index", 0)

        auth_type = str(getattr(key, "auth_type", "api_key") or "api_key").lower()
        extra_headers: dict[str, str] = {}
        oauth_meta: dict = {}
        effective_model = request.model_name
        attempt_recorded = False

        try:
            # 解析 Key（复用统一的认证解析逻辑）
            effective_proxy = resolve_effective_proxy(
                getattr(provider, "proxy", None), getattr(key, "proxy", None)
            )
            try:
                api_key_value, auth_config = await _resolve_key_auth(
                    key, provider, provider_proxy_config=effective_proxy
                )
            except _KeyAuthError as e:
                raise Exception(e.message) from e
            oauth_meta = auth_config or {}

            # OAuth 额外头
            if auth_type == "oauth":
                account_id = oauth_meta.get("account_id")
                if account_id:
                    extra_headers["chatgpt-account-id"] = str(account_id)

            ep_extra = get_extra_headers_from_endpoint(endpoint) or {}
            extra_headers.update(ep_extra)

            # 确定实际模型名
            effective_model = request.model_name
            if request.mode == "global":
                if candidate.mapping_matched_model:
                    effective_model = candidate.mapping_matched_model
                elif gm_obj:
                    # Fallback: 从 Provider.Model.provider_model_mappings 获取映射
                    # 与正常请求流程中 _get_mapped_model() 的逻辑一致
                    gm_id_str = str(gm_obj.id)
                    for m in provider.models or []:
                        if not getattr(m, "is_active", False):
                            continue
                        if str(getattr(m, "global_model_id", "")) != gm_id_str:
                            continue
                        ep_format = str(getattr(endpoint, "api_format", "") or "")
                        effective_model = m.select_provider_model_name(
                            affinity_key=None, api_format=ep_format
                        )
                        logger.info(
                            "[test-failover] Fallback mapping: {} -> {} "
                            "(provider_model_name={}, has_provider_model_mappings={})",
                            request.model_name,
                            effective_model,
                            m.provider_model_name,
                            bool(m.provider_model_mappings),
                        )
                        break
                    else:
                        logger.info(
                            "[test-failover] No matching Model found for gm_id={} in provider={}",
                            gm_id_str,
                            provider.name,
                        )

            # 获取 adapter
            adapter_class = get_adapter_for_format(endpoint.api_format)
            if not adapter_class:
                raise Exception(f"Unknown API format: {endpoint.api_format}")

            # 构建测试请求（默认非流式）
            check_request = {
                "model": effective_model,
                "messages": [{"role": "user", "content": request.message or "Hello"}],
                "max_tokens": 30,
                "temperature": 0.7,
                "stream": False,
            }

            body_rules = getattr(endpoint, "body_rules", None)
            header_rules = getattr(endpoint, "header_rules", None)

            logger.info(
                "[admin-test-failover] attempt-start | provider_id={} provider={} endpoint_id={} endpoint_format={} key_id={} key_name={} model={} stream={} candidate_index={} auth_type={}",
                provider.id,
                provider.name,
                getattr(endpoint, "id", None),
                endpoint.api_format,
                key.id,
                getattr(key, "name", None),
                effective_model,
                check_request["stream"],
                candidate_idx,
                auth_type,
            )

            # 执行检查
            async with _admin_test_semaphore:
                response = await adapter_class.check_endpoint(
                    None,
                    endpoint.base_url,
                    api_key_value,
                    check_request,
                    extra_headers if extra_headers else None,
                    body_rules=body_rules,
                    header_rules=header_rules,
                    db=db,
                    user=current_user,
                    provider_name=provider.name,
                    provider_id=str(provider.id),
                    api_key_id=str(key.id),
                    model_name=effective_model,
                    auth_type=auth_type,
                    provider_type=p_type if p_type else None,
                    decrypted_auth_config=oauth_meta if oauth_meta else None,
                    provider_endpoint=endpoint,
                    provider_api_key=key,
                    proxy_config=effective_proxy,
                )

            latency_ms = int((time.monotonic() - start_time) * 1000)
            status_code = response.get("status_code", 0)

            # 检查响应是否有错误
            has_error = bool(response.get("error")) or status_code != 200
            if not has_error:
                resp_data = response.get("response", {})
                resp_body = resp_data.get("response_body", {})
                if isinstance(resp_body, str):
                    try:
                        parsed = json.loads(resp_body)
                    except (json.JSONDecodeError, ValueError):
                        parsed = resp_body
                else:
                    parsed = resp_body
                if isinstance(parsed, dict) and "error" in parsed:
                    has_error = True

            if has_error:
                error_msg = str(response.get("error", ""))[:300]
                if not error_msg and status_code != 200:
                    error_msg = f"HTTP {status_code}"
                if not error_msg and isinstance(parsed, dict) and "error" in parsed:
                    err_val = parsed["error"]
                    error_msg = str(
                        err_val.get("message", err_val) if isinstance(err_val, dict) else err_val
                    )[:300]
                logger.warning(
                    "[admin-test-failover] attempt-failed | provider_id={} endpoint_id={} key_id={} model={} stream={} candidate_index={} status_code={} local_protocol_error={} error={}",
                    provider.id,
                    getattr(endpoint, "id", None),
                    key.id,
                    effective_model,
                    check_request["stream"],
                    candidate_idx,
                    status_code,
                    "LocalProtocolError" in error_msg,
                    error_msg,
                )
                attempts.append(
                    TestAttemptDetail(
                        candidate_index=candidate_idx,
                        endpoint_api_format=str(endpoint.api_format),
                        endpoint_base_url=str(endpoint.base_url)[:80],
                        key_name=getattr(key, "name", None),
                        key_id=str(key.id),
                        auth_type=auth_type,
                        effective_model=effective_model,
                        status="failed",
                        error_message=error_msg,
                        status_code=status_code,
                        latency_ms=latency_ms,
                    )
                )
                attempt_recorded = True
                raise Exception(f"Upstream error: status={status_code}, error={error_msg}")

            # 成功
            logger.info(
                "[admin-test-failover] attempt-success | provider_id={} endpoint_id={} key_id={} model={} stream={} candidate_index={} status_code={} latency_ms={}",
                provider.id,
                getattr(endpoint, "id", None),
                key.id,
                effective_model,
                check_request["stream"],
                candidate_idx,
                status_code,
                latency_ms,
            )
            attempts.append(
                TestAttemptDetail(
                    candidate_index=candidate_idx,
                    endpoint_api_format=str(endpoint.api_format),
                    endpoint_base_url=str(endpoint.base_url)[:80],
                    key_name=getattr(key, "name", None),
                    key_id=str(key.id),
                    auth_type=auth_type,
                    effective_model=effective_model,
                    status="success",
                    status_code=status_code,
                    latency_ms=latency_ms,
                )
            )

            return AttemptResult(
                kind=AttemptKind.SYNC_RESPONSE,
                http_status=status_code,
                http_headers={},
                response_body=response.get("response", response),
            )

        except Exception as exc:
            latency_ms = int((time.monotonic() - start_time) * 1000)
            logger.warning(
                "[admin-test-failover] attempt-exception | provider_id={} endpoint_id={} key_id={} model={} candidate_index={} local_protocol_error={} error={}",
                provider.id,
                getattr(endpoint, "id", None),
                getattr(key, "id", None),
                effective_model,
                candidate_idx,
                "LocalProtocolError" in str(exc),
                str(exc)[:300],
            )
            # has_error 路径已记录带 status_code 的详细 attempt，此处仅补录早期异常
            if not attempt_recorded:
                attempts.append(
                    TestAttemptDetail(
                        candidate_index=candidate_idx,
                        endpoint_api_format=str(endpoint.api_format),
                        endpoint_base_url=str(endpoint.base_url)[:80],
                        key_name=getattr(key, "name", None),
                        key_id=str(key.id),
                        auth_type=auth_type,
                        effective_model=effective_model,
                        status="failed",
                        error_message=str(exc)[:300],
                        latency_ms=latency_ms,
                    )
                )
            raise

    # 4. 预设 candidate index（FailoverEngine 也会 setattr，此处兜底防止 setattr 失败）
    for i, cand in enumerate(candidates):
        cand._utf_candidate_index = i  # type: ignore[attr-defined]

    # 5. 执行故障转移
    try:
        engine = FailoverEngine(db)
        result = await engine.execute(
            candidates=candidates,
            attempt_func=_attempt_func,
            retry_policy=RetryPolicy(mode=RetryMode.DISABLED),
            skip_policy=SkipPolicy(),
            request_id=None,
        )

        # 补充 skipped 候选到 attempts
        for i, cand in enumerate(candidates):
            if cand.is_skipped and not any(a.candidate_index == i for a in attempts):
                attempts.append(
                    TestAttemptDetail(
                        candidate_index=i,
                        endpoint_api_format=str(cand.endpoint.api_format),
                        endpoint_base_url=str(cand.endpoint.base_url)[:80],
                        key_name=getattr(cand.key, "name", None),
                        key_id=str(cand.key.id),
                        auth_type=str(getattr(cand.key, "auth_type", "") or ""),
                        status="skipped",
                        skip_reason=cand.skip_reason,
                    )
                )

        attempts.sort(key=lambda a: a.candidate_index)

        # 提取成功时的数据
        data = None
        if result.success and result.attempt_result:
            data = {
                "stream": False,
                "response": result.attempt_result.response_body,
            }
            logger.info(
                "[admin-test-failover] finish | provider_id={} model={} total_candidates={} total_attempts={} success={} stream={} local_protocol_error={}",
                provider.id,
                request.model_name,
                len(candidates),
                result.attempt_count,
                result.success,
                False,
                "LocalProtocolError" in str(result.attempt_result.response_body),
            )

        return TestModelFailoverResponse(
            success=result.success,
            model=request.model_name,
            provider={"id": str(provider.id), "name": provider.name},
            attempts=attempts,
            total_candidates=len(candidates),
            total_attempts=result.attempt_count,
            data=data,
            error=result.error_message if not result.success else None,
        ).model_dump()

    except Exception as e:
        logger.error(
            "[admin-test-failover] error | provider_id={} model={} total_candidates={} local_protocol_error={} error={}",
            request.provider_id,
            request.model_name,
            len(candidates),
            "LocalProtocolError" in str(e),
            str(e)[:500],
        )
        return TestModelFailoverResponse(
            success=False,
            model=request.model_name,
            provider={"id": str(provider.id), "name": provider.name},
            attempts=attempts,
            total_candidates=len(candidates),
            total_attempts=0,
            error=str(e)[:500],
        ).model_dump()
