"""
Provider Query API 端点
用于查询提供商的模型列表等信息
"""

from __future__ import annotations

import asyncio
import json
import time
from collections.abc import Awaitable, Callable
from datetime import datetime, timezone
from typing import TYPE_CHECKING, Any
from uuid import uuid4

import httpx
from fastapi import APIRouter, Depends, HTTPException, Request
from pydantic import BaseModel, Field
from sqlalchemy import update
from sqlalchemy.orm import Session, joinedload, make_transient

from src.api.handlers.base.chat_adapter_base import get_adapter_class
from src.api.handlers.base.cli_adapter_base import get_cli_adapter_class
from src.config.constants import TimeoutDefaults
from src.core.api_format import get_extra_headers_from_endpoint
from src.core.cache_service import CacheService
from src.core.crypto import crypto_service
from src.core.logger import logger
from src.core.provider_types import ProviderType
from src.database import create_session
from src.database.database import get_db
from src.models.database import Provider, ProviderAPIKey, ProviderEndpoint, RequestCandidate, User
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
)
from src.services.provider.oauth_token import resolve_oauth_access_token
from src.services.proxy_node.resolver import resolve_effective_proxy
from src.services.request.candidate import RequestCandidateService
from src.utils.auth_utils import get_current_user

if TYPE_CHECKING:
    from src.services.scheduling.schemas import ProviderCandidate

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


def _get_adapter_for_format(api_format: str) -> Any:
    """按 api_format 获取 Chat/CLI adapter 类。"""
    return get_adapter_class(api_format) or get_cli_adapter_class(api_format)


def _require_test_endpoint_base_url(endpoint: Any) -> str:
    """校验测试链路里的 endpoint.base_url。"""
    base_url = getattr(endpoint, "base_url", None)
    if not isinstance(base_url, str):
        endpoint_id = str(getattr(endpoint, "id", "") or "unknown")
        api_format = str(getattr(endpoint, "api_format", "") or "unknown")
        raise ValueError(
            f"Endpoint {endpoint_id} ({api_format}) has invalid base_url type: "
            f"expected str, got {type(base_url).__name__}"
        )

    normalized = base_url.strip()
    if not normalized:
        endpoint_id = str(getattr(endpoint, "id", "") or "unknown")
        api_format = str(getattr(endpoint, "api_format", "") or "unknown")
        raise ValueError(f"Endpoint {endpoint_id} ({api_format}) has empty base_url")
    return normalized


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
    request_id: str | None = None
    concurrency: int = Field(default=1, ge=1, le=20)


class TestAttemptDetail(BaseModel):
    """单次测试尝试的详情"""

    candidate_index: int
    retry_index: int = 0
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


# ============ Internal helpers ============


def _test_check_response_has_error(resp: dict[str, Any]) -> bool:
    """快速判断 check_endpoint 结果是否失败。"""
    if resp.get("error"):
        return True
    if int(resp.get("status_code", 0) or 0) != 200:
        return True
    resp_data = resp.get("response", {})
    if isinstance(resp_data, dict) and resp_data.get("error"):
        return True
    resp_body = resp_data.get("response_body", {}) if isinstance(resp_data, dict) else {}
    parsed = resp_body
    if isinstance(resp_body, str):
        try:
            parsed = json.loads(resp_body)
        except (json.JSONDecodeError, ValueError):
            pass
    return isinstance(parsed, dict) and bool(parsed.get("error"))


def _test_check_extract_error_message(resp: dict[str, Any]) -> str:
    """从 check_endpoint 结果中提取错误消息。"""
    resp_data = resp.get("response", {}) if isinstance(resp, dict) else {}

    # 直接检查 response dict 中的 error（check_endpoint 返回的结构）
    if isinstance(resp_data, dict):
        err = resp_data.get("error")
        if isinstance(err, dict):
            msg = err.get("message")
            if isinstance(msg, str):
                return msg
        if isinstance(err, str):
            return err

    # 兼容 response_body 包装层
    body = resp_data.get("response_body", {}) if isinstance(resp_data, dict) else {}
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


def _test_check_should_fallback_to_non_stream(resp: dict[str, Any]) -> bool:
    """仅在流式特有失败时回退到非流式，避免无意义重试。"""
    status = int(resp.get("status_code") or 0)
    if status in {404, 405, 415, 501}:
        return True

    message = _test_check_extract_error_message(resp).lower()
    if not message:
        return False

    stream_markers = ("stream", "sse", "streamgeneratecontent")
    unsupported_markers = ("not support", "unsupported", "invalid argument")
    if any(token in message for token in stream_markers) and any(
        token in message for token in unsupported_markers
    ):
        return True

    decode_markers = (
        "decompress",
        "incorrect header check",
        "decoding error",
        "content decoding",
        "contentdecodingerror",
    )
    return any(token in message for token in decode_markers)


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
        "base_url": _require_test_endpoint_base_url(endpoint),
        "api_format": endpoint.api_format,
        "extra_headers": extra_headers if extra_headers else None,
        "timeout": TimeoutDefaults.HTTP_REQUEST,
    }

    try:
        # 获取对应的 Adapter 类
        adapter_class = _get_adapter_for_format(endpoint.api_format)
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
            if resp.get("error"):
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
            if isinstance(parsed, dict) and parsed.get("error"):
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

        # 策略：优先流式，若失败回退到非流式
        used_stream = True
        logger.debug("[test-model] 尝试流式请求...")
        response = await _do_check(check_request)

        if _response_has_error(response) and _should_fallback_to_non_stream(response):
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
        import json

        if isinstance(response_body, str):
            try:
                parsed_body = json.loads(response_body)
            except json.JSONDecodeError:
                pass

        if isinstance(parsed_body, dict) and parsed_body.get("error"):
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
        elif response.get("error"):
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
    按可用性排序：熔断器关闭 > 健康度高 > 连续失败少 > Key 优先级。
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

    candidates.sort(key=lambda c: _direct_candidate_sort_key(c))
    return candidates


def _direct_candidate_sort_key(candidate: ProviderCandidate) -> tuple[int, float, int, int]:
    """
    按可用性排序候选：
    1. 熔断器状态：关闭(0) > 打开(2)
    2. 健康度评分：越高越好（取负值以升序排列）
    3. 连续失败次数：越少越好
    4. Key 优先级：数字越小越优先
    """
    key = candidate.key
    ep_format = candidate.provider_api_format

    # 熔断器状态
    circuit_breaker_order = 0
    cb_data = getattr(key, "circuit_breaker_by_format", None) or {}
    cb_entry = cb_data.get(ep_format, {}) if isinstance(cb_data, dict) else {}
    if isinstance(cb_entry, dict) and cb_entry.get("open"):
        circuit_breaker_order = 2

    # 健康度评分（默认 1.0 表示完全健康）
    health_score = 1.0
    consecutive_failures = 0
    health_data = getattr(key, "health_by_format", None) or {}
    health_entry = health_data.get(ep_format, {}) if isinstance(health_data, dict) else {}
    if isinstance(health_entry, dict):
        health_score = health_entry.get("health_score", 1.0)
        consecutive_failures = health_entry.get("consecutive_failures", 0)

    # Key 优先级
    internal_priority_raw = getattr(key, "internal_priority", None)
    try:
        internal_priority = (
            int(internal_priority_raw) if internal_priority_raw is not None else 999999
        )
    except (TypeError, ValueError):
        internal_priority = 999999

    return (circuit_breaker_order, -health_score, consecutive_failures, internal_priority)


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


def _parse_jsonish(value: Any) -> Any:
    if isinstance(value, str):
        try:
            return json.loads(value)
        except (json.JSONDecodeError, TypeError, ValueError):
            return value
    return value


def _resolve_test_effective_model(
    *,
    provider: Provider,
    candidate: Any,
    request: TestModelFailoverRequest,
    gm_obj: Any,
    key: Any | None = None,
) -> str:
    effective_model = request.model_name
    if request.mode != "global":
        return effective_model

    current_key = key or getattr(candidate, "key", None)
    pool_mapping = (
        getattr(current_key, "_pool_mapping_matched_model", None) if current_key else None
    )
    mapping_matched_model = pool_mapping or getattr(candidate, "mapping_matched_model", None)
    if mapping_matched_model:
        return str(mapping_matched_model)

    if not gm_obj:
        return effective_model

    gm_id_str = str(gm_obj.id)
    endpoint = getattr(candidate, "endpoint", None)
    ep_format = str(getattr(endpoint, "api_format", "") or "")
    for model in provider.models or []:
        if not getattr(model, "is_active", False):
            continue
        if str(getattr(model, "global_model_id", "") or "") != gm_id_str:
            continue
        selected = model.select_provider_model_name(affinity_key=None, api_format=ep_format)
        if selected:
            return str(selected)
    return effective_model


def _build_test_candidate_meta(
    *,
    candidates: list[ProviderCandidate],
    provider: Provider,
    request: TestModelFailoverRequest,
    gm_obj: Any,
) -> tuple[dict[tuple[int, str], dict[str, Any]], dict[int, dict[str, Any]]]:
    from src.services.scheduling.schemas import PoolCandidate

    by_pair: dict[tuple[int, str], dict[str, Any]] = {}
    by_candidate: dict[int, dict[str, Any]] = {}

    for candidate_index, candidate in enumerate(candidates):
        endpoint = candidate.endpoint
        base_meta = {
            "endpoint_api_format": str(getattr(endpoint, "api_format", "") or ""),
            "endpoint_base_url": str(getattr(endpoint, "base_url", "") or "")[:80],
            "effective_model": _resolve_test_effective_model(
                provider=provider,
                candidate=candidate,
                request=request,
                gm_obj=gm_obj,
            ),
        }
        by_candidate[candidate_index] = base_meta

        key = getattr(candidate, "key", None)
        if key is not None and getattr(key, "id", None):
            by_pair[(candidate_index, str(key.id))] = dict(base_meta)

        if isinstance(candidate, PoolCandidate):
            for pool_key in candidate.pool_keys or []:
                if not getattr(pool_key, "id", None):
                    continue
                by_pair[(candidate_index, str(pool_key.id))] = {
                    "endpoint_api_format": base_meta["endpoint_api_format"],
                    "endpoint_base_url": base_meta["endpoint_base_url"],
                    "effective_model": _resolve_test_effective_model(
                        provider=provider,
                        candidate=candidate,
                        request=request,
                        gm_obj=gm_obj,
                        key=pool_key,
                    ),
                }

    return by_pair, by_candidate


def _flatten_test_candidates_for_concurrency(
    candidates: list[ProviderCandidate],
) -> list[ProviderCandidate]:
    from src.services.scheduling.schemas import (
        PoolCandidate,
    )
    from src.services.scheduling.schemas import ProviderCandidate as SchedulerCandidate

    flattened: list[ProviderCandidate] = []
    for candidate in candidates:
        if not isinstance(candidate, PoolCandidate):
            flattened.append(candidate)
            continue

        for pool_key in candidate.pool_keys or []:
            key_skipped = candidate.is_skipped or bool(getattr(pool_key, "_pool_skipped", False))
            key_skip_reason_raw = (
                getattr(pool_key, "_pool_skip_reason", None) if key_skipped else None
            )
            key_skip_reason = (
                str(key_skip_reason_raw)
                if key_skip_reason_raw
                else (str(candidate.skip_reason) if candidate.skip_reason else None)
            )
            flattened.append(
                SchedulerCandidate(
                    provider=candidate.provider,
                    endpoint=candidate.endpoint,
                    key=pool_key,
                    is_cached=bool(getattr(candidate, "is_cached", False)),
                    is_skipped=key_skipped,
                    skip_reason=key_skip_reason,
                    mapping_matched_model=(
                        getattr(pool_key, "_pool_mapping_matched_model", None)
                        or getattr(candidate, "mapping_matched_model", None)
                    ),
                    needs_conversion=bool(getattr(candidate, "needs_conversion", False)),
                    provider_api_format=(
                        getattr(candidate, "provider_api_format", "")
                        or str(getattr(candidate.endpoint, "api_format", "") or "")
                    ),
                    output_limit=getattr(candidate, "output_limit", None),
                    capability_miss_count=int(getattr(candidate, "capability_miss_count", 0) or 0),
                )
            )

    return flattened


def _build_test_candidate_extra_data(candidate: ProviderCandidate) -> dict[str, Any]:
    extra_data: dict[str, Any] = {
        "needs_conversion": bool(getattr(candidate, "needs_conversion", False)),
        "provider_api_format": (
            getattr(candidate, "provider_api_format", None)
            or getattr(getattr(candidate, "endpoint", None), "api_format", None)
        ),
        "mapping_matched_model": (
            getattr(candidate, "mapping_matched_model", None)
            or getattr(getattr(candidate, "key", None), "_pool_mapping_matched_model", None)
        ),
    }
    key_extra = getattr(getattr(candidate, "key", None), "_pool_extra_data", None)
    if isinstance(key_extra, dict):
        extra_data.update(key_extra)
    return extra_data


def _precreate_concurrent_test_records(
    *,
    db: Session,
    request_id: str,
    candidates: list[ProviderCandidate],
    user: User | None,
) -> dict[int, str]:
    record_map: dict[int, str] = {}
    rows: list[dict[str, Any]] = []

    user_id = str(getattr(user, "id", "") or "") or None
    now = datetime.now(timezone.utc)
    for candidate_index, candidate in enumerate(candidates):
        record_id = str(uuid4())
        record_map[candidate_index] = record_id
        rows.append(
            {
                "id": record_id,
                "request_id": request_id,
                "candidate_index": candidate_index,
                "retry_index": 0,
                "user_id": user_id,
                "api_key_id": None,
                "provider_id": str(getattr(candidate.provider, "id", "") or "") or None,
                "endpoint_id": str(getattr(candidate.endpoint, "id", "") or "") or None,
                "key_id": str(getattr(candidate.key, "id", "") or "") or None,
                "status": (
                    "skipped" if bool(getattr(candidate, "is_skipped", False)) else "available"
                ),
                "skip_reason": getattr(candidate, "skip_reason", None),
                "is_cached": bool(getattr(candidate, "is_cached", False)),
                "extra_data": _build_test_candidate_extra_data(candidate),
                "required_capabilities": None,
                "created_at": now,
            }
        )

    if rows:
        db.bulk_insert_mappings(RequestCandidate, rows)  # type: ignore[arg-type]
        db.commit()

    return record_map


def _mark_concurrent_test_record_cancelled(record_id: str) -> None:
    if not record_id:
        return
    with create_session() as local_db:
        RequestCandidateService.mark_candidate_cancelled(
            db=local_db,
            candidate_id=record_id,
            status_code=499,
        )


def _cancel_remaining_concurrent_test_records(request_id: str) -> None:
    if not request_id:
        return
    with create_session() as local_db:
        local_db.execute(
            update(RequestCandidate)
            .where(RequestCandidate.request_id == request_id)
            .where(RequestCandidate.status.in_(["available", "pending"]))
            .values(
                status="cancelled",
                status_code=499,
                finished_at=datetime.now(timezone.utc),
            )
        )
        local_db.commit()


async def _execute_test_check(
    *,
    provider_obj: Any,
    endpoint: Any,
    key: Any,
    effective_model: str,
    request_payload: dict[str, Any],
    request_timeout: float,
    provider_type: str,
    user: User | None,
    db: Session | None,
) -> tuple[dict[str, Any], str]:
    effective_proxy = resolve_effective_proxy(
        getattr(provider_obj, "proxy", None), getattr(key, "proxy", None)
    )
    try:
        api_key_value, auth_config = await _resolve_key_auth(
            key,
            provider_obj,
            provider_proxy_config=effective_proxy,
        )
    except _KeyAuthError as e:
        raise RuntimeError(e.message) from e

    auth_type = str(getattr(key, "auth_type", "api_key") or "api_key").lower()
    extra_headers = get_extra_headers_from_endpoint(endpoint) or {}
    if auth_type == "oauth":
        account_id = (auth_config or {}).get("account_id")
        if account_id:
            extra_headers["chatgpt-account-id"] = str(account_id)

    adapter_class = _get_adapter_for_format(endpoint.api_format)
    if not adapter_class:
        raise ValueError(f"Unknown API format: {endpoint.api_format}")

    async def _run_check(stream: bool) -> dict[str, Any]:
        return await adapter_class.check_endpoint(
            None,
            _require_test_endpoint_base_url(endpoint),
            api_key_value,
            {
                **request_payload,
                "model": effective_model,
                "stream": stream,
            },
            extra_headers if extra_headers else None,
            body_rules=getattr(endpoint, "body_rules", None),
            header_rules=getattr(endpoint, "header_rules", None),
            db=db,
            user=user,
            provider_name=provider_obj.name,
            provider_id=str(provider_obj.id),
            api_key_id=str(key.id),
            model_name=effective_model,
            auth_type=auth_type,
            provider_type=provider_type if provider_type else None,
            decrypted_auth_config=auth_config if auth_config else None,
            provider_endpoint=endpoint,
            provider_api_key=key,
            proxy_config=effective_proxy,
            timeout_seconds=request_timeout,
        )

    use_stream = bool(request_payload.get("stream", False))
    response = await _run_check(use_stream)

    if use_stream and _test_check_response_has_error(response):
        if _test_check_should_fallback_to_non_stream(response):
            logger.info(
                "[test-model-failover] Stream check failed for provider={} model={}, fallback to non-stream",
                provider_obj.name,
                effective_model,
            )
            response = await _run_check(False)

    return response, auth_type


async def _run_concurrent_test(
    *,
    candidates: list[ProviderCandidate],
    concurrency: int,
    is_cancelled: Callable[[], Awaitable[bool]],
    request_id: str,
    request_payload: dict[str, Any],
    effective_model_by_candidate_index: dict[int, str],
    request_timeout: float,
    provider_type: str,
    user: User | None,
    db: Session,
) -> dict[str, Any]:
    from src.core.exceptions import EmbeddedErrorException
    from src.services.candidate.recorder import CandidateRecorder
    from src.services.task.service import pool_on_error

    semaphore = asyncio.Semaphore(max(1, concurrency))
    record_map = _precreate_concurrent_test_records(
        db=db,
        request_id=request_id,
        candidates=candidates,
        user=user,
    )

    # 预加载所有候选的 provider/endpoint/key，避免每个 worker 重复查询
    _preloaded: dict[int, tuple[Provider, ProviderEndpoint, ProviderAPIKey]] = {}
    with create_session() as preload_db:
        provider_ids = {str(getattr(c.provider, "id", "") or "") for c in candidates}
        endpoint_ids = {str(getattr(c.endpoint, "id", "") or "") for c in candidates}
        key_ids = {str(getattr(c.key, "id", "") or "") for c in candidates}
        providers_by_id = {
            str(p.id): p
            for p in preload_db.query(Provider).filter(Provider.id.in_(provider_ids)).all()
        }
        endpoints_by_id = {
            str(e.id): e
            for e in preload_db.query(ProviderEndpoint)
            .filter(ProviderEndpoint.id.in_(endpoint_ids))
            .all()
        }
        keys_by_id = {
            str(k.id): k
            for k in preload_db.query(ProviderAPIKey).filter(ProviderAPIKey.id.in_(key_ids)).all()
        }
        _already_detached: set[int] = set()
        for idx, cand in enumerate(candidates):
            p = providers_by_id.get(str(getattr(cand.provider, "id", "") or ""))
            e = endpoints_by_id.get(str(getattr(cand.endpoint, "id", "") or ""))
            k = keys_by_id.get(str(getattr(cand.key, "id", "") or ""))
            if p is not None and e is not None and k is not None:
                # make_transient 将对象脱离 session 并保留已加载属性，
                # 避免 expired 状态导致跨协程访问时触发 lazy load 报错。
                # 同一个对象（多个 candidate 可能共享同一 provider/endpoint）
                # 只需处理一次。
                for obj in (p, e, k):
                    obj_id = id(obj)
                    if obj_id not in _already_detached:
                        make_transient(obj)
                        _already_detached.add(obj_id)
                _preloaded[idx] = (p, e, k)

    success_payload: dict[str, Any] = {}
    success_event = asyncio.Event()
    candidate_recorder = CandidateRecorder(db)
    last_error: Exception | None = None

    async def _worker(candidate_index: int) -> dict[str, Any]:
        nonlocal last_error
        record_id = record_map[candidate_index]

        started = False
        started_at = 0.0

        try:
            preloaded = _preloaded.get(candidate_index)
            if preloaded is None:
                raise RuntimeError("并发测试目标不存在或已被删除")
            local_provider, local_endpoint, local_key = preloaded
            if success_event.is_set() or await is_cancelled():
                _mark_concurrent_test_record_cancelled(record_id)
                return {"status": "cancelled"}

            async with semaphore:
                if success_event.is_set() or await is_cancelled():
                    _mark_concurrent_test_record_cancelled(record_id)
                    return {"status": "cancelled"}

                with create_session() as update_db:
                    RequestCandidateService.mark_candidate_started(update_db, record_id)

                started = True
                started_at = time.perf_counter()
                response, auth_type = await _execute_test_check(
                    provider_obj=local_provider,
                    endpoint=local_endpoint,
                    key=local_key,
                    effective_model=effective_model_by_candidate_index.get(
                        candidate_index,
                        str(request_payload.get("model", "") or ""),
                    ),
                    request_payload=request_payload,
                    request_timeout=request_timeout,
                    provider_type=provider_type,
                    user=user,
                    db=None,
                )
                elapsed_ms = max(0, int((time.perf_counter() - started_at) * 1000))

                with create_session() as parse_db:
                    parse_key = (
                        parse_db.query(ProviderAPIKey)
                        .filter(ProviderAPIKey.id == str(getattr(local_key, "id", "") or ""))
                        .first()
                    )
                    parsed = _extract_test_response_or_raise(
                        response=response,
                        endpoint=local_endpoint,
                        provider_name=str(local_provider.name),
                        auth_type=auth_type,
                        api_key=parse_key or local_key,
                        db=parse_db,
                    )

                with create_session() as update_db:
                    RequestCandidateService.mark_candidate_success(
                        db=update_db,
                        candidate_id=record_id,
                        status_code=200,
                        latency_ms=elapsed_ms,
                    )

                if not success_event.is_set():
                    success_payload.update(
                        {
                            "response": parsed,
                            "candidate_index": candidate_index,
                            "key_id": str(getattr(local_key, "id", "") or "") or None,
                        }
                    )
                    success_event.set()
                return {"status": "success"}
        except asyncio.CancelledError:
            if started or not success_event.is_set():
                _mark_concurrent_test_record_cancelled(record_id)
            return {"status": "cancelled"}
        except Exception as exc:
            last_error = exc
            elapsed_ms = max(0, int((time.perf_counter() - started_at) * 1000)) if started else None
            status_code = None
            if isinstance(exc, httpx.HTTPStatusError):
                status_code = int(exc.response.status_code)
            elif isinstance(exc, httpx.TimeoutException):
                status_code = 408
            elif isinstance(exc, EmbeddedErrorException):
                status_code = int(exc.error_code or 200)

            loaded = _preloaded.get(candidate_index)
            if loaded is not None and status_code is not None:
                await pool_on_error(loaded[0], loaded[2], status_code, exc)

            with create_session() as update_db:
                RequestCandidateService.mark_candidate_failed(
                    db=update_db,
                    candidate_id=record_id,
                    error_type=type(exc).__name__,
                    error_message=str(
                        getattr(exc, "error_message", None)
                        or getattr(exc, "upstream_response", None)
                        or exc
                    ),
                    status_code=status_code,
                    latency_ms=elapsed_ms,
                )
            return {"status": "failed", "error": exc}

    async def _watch_disconnect() -> bool:
        while not success_event.is_set():
            if await is_cancelled():
                return True
            await asyncio.sleep(0.1)
        return False

    tasks = [
        asyncio.create_task(_worker(candidate_index))
        for candidate_index, candidate in enumerate(candidates)
        if not bool(getattr(candidate, "is_skipped", False))
    ]
    disconnect_task = asyncio.create_task(_watch_disconnect())
    pending: set[asyncio.Task[Any]] = set(tasks)
    pending.add(disconnect_task)

    try:
        while pending:
            if pending == {disconnect_task}:
                disconnect_task.cancel()
                pending.clear()
                break

            done, pending = await asyncio.wait(pending, return_when=asyncio.FIRST_COMPLETED)
            if disconnect_task in done and disconnect_task.result() is True:
                for task in pending:
                    task.cancel()
                break

            for finished in done:
                if finished is disconnect_task:
                    continue
                result = finished.result()
                if result.get("status") == "success":
                    for task in pending:
                        task.cancel()
                    pending.discard(disconnect_task)
                    disconnect_task.cancel()
                    break
            if success_event.is_set():
                break
    finally:
        await asyncio.gather(*pending, return_exceptions=True)
        if not disconnect_task.done():
            disconnect_task.cancel()
        await asyncio.gather(disconnect_task, return_exceptions=True)

    if success_event.is_set():
        _cancel_remaining_concurrent_test_records(request_id)
    elif await is_cancelled():
        _cancel_remaining_concurrent_test_records(request_id)

    try:
        db.expire_all()
        candidate_keys = candidate_recorder.get_candidate_keys(request_id)
    except Exception:
        candidate_keys = []

    attempt_count = sum(
        1
        for item in candidate_keys
        if str(getattr(item, "status", "") or "")
        not in {"skipped", "cancelled", "available", "unused"}
    )
    return {
        "success": success_event.is_set(),
        "candidate_keys": candidate_keys,
        "attempt_count": attempt_count,
        "run_error": last_error,
        "response": success_payload.get("response"),
    }


def _maybe_mark_test_oauth_key_invalid(
    *,
    db: Session,
    key: Any,
    auth_type: str,
    error_payload: Any,
) -> None:
    if auth_type != "oauth" or not isinstance(error_payload, dict):
        return

    error_obj = error_payload.get("error")
    if not isinstance(error_obj, dict):
        return

    error_message = str(error_obj.get("message", "") or "")
    if error_obj.get("code") != 403:
        return
    if (
        "verify" not in error_message.lower()
        and "permission" not in str(error_obj.get("status", "") or "").lower()
    ):
        return

    from datetime import datetime, timezone

    from src.services.provider.oauth_token import OAUTH_ACCOUNT_BLOCK_PREFIX

    key.oauth_invalid_at = datetime.now(timezone.utc)
    key.oauth_invalid_reason = f"{OAUTH_ACCOUNT_BLOCK_PREFIX}Google 要求验证账号"
    key.is_active = False
    db.commit()


def _extract_test_response_or_raise(
    *,
    response: dict[str, Any],
    endpoint: Any,
    provider_name: str,
    auth_type: str,
    api_key: Any,
    db: Session,
) -> dict[str, Any]:
    status_code = int(response.get("status_code", 0) or 0)
    response_payload = response.get("response", {})
    parsed_payload = _parse_jsonish(response_payload)
    if isinstance(parsed_payload, dict) and "response_body" in parsed_payload:
        parsed_payload = _parse_jsonish(parsed_payload.get("response_body"))

    if isinstance(parsed_payload, dict) and parsed_payload.get("error"):
        _maybe_mark_test_oauth_key_invalid(
            db=db,
            key=api_key,
            auth_type=auth_type,
            error_payload=parsed_payload,
        )
        error_obj = parsed_payload["error"]
        error_code = error_obj.get("code") if isinstance(error_obj, dict) else status_code or 500
        error_message = (
            error_obj.get("message") if isinstance(error_obj, dict) else str(error_obj or "")
        )
        error_status = error_obj.get("status") if isinstance(error_obj, dict) else None
        from src.core.exceptions import EmbeddedErrorException

        raise EmbeddedErrorException(
            provider_name=provider_name,
            error_code=int(error_code) if error_code else None,
            error_message=str(error_message or ""),
            error_status=str(error_status) if error_status else None,
        )

    if status_code == 200 and not response.get("error"):
        return parsed_payload if isinstance(parsed_payload, dict) else response_payload

    error_meta = response_payload if isinstance(response_payload, dict) else {}
    error_type = str(error_meta.get("error_type", "") or "")
    error_message = str(response.get("error", "") or "")
    if not error_message and isinstance(parsed_payload, dict):
        embedded_error = parsed_payload.get("error")
        if isinstance(embedded_error, dict):
            error_message = str(embedded_error.get("message", "") or "")
        elif embedded_error:
            error_message = str(embedded_error)
    if not error_message and isinstance(parsed_payload, str):
        error_message = parsed_payload
    if not error_message and status_code:
        error_message = f"HTTP {status_code}"

    request_obj = httpx.Request("POST", str(getattr(endpoint, "base_url", "") or ""))
    if status_code > 0:
        body_text = error_message[:4000] if error_message else ""
        synthetic_response = httpx.Response(
            status_code=status_code,
            request=request_obj,
            text=body_text,
            headers=response.get("headers", {}),
        )
        http_error = httpx.HTTPStatusError(
            message=body_text or f"HTTP {status_code}",
            request=request_obj,
            response=synthetic_response,
        )
        http_error.upstream_response = body_text  # type: ignore[attr-defined]
        raise http_error

    if error_type == "timeout":
        raise httpx.TimeoutException(error_message or "Request timeout")

    if error_type in {"network_error", "connection_failed"}:
        raise httpx.ConnectError(error_message or "Connection failed", request=request_obj)

    from src.core.exceptions import ProviderNotAvailableException

    raise ProviderNotAvailableException(
        error_message or "服务暂时不可用，请稍后重试",
        provider_name=provider_name,
        upstream_response=error_message or None,
    )


def _build_test_attempts_from_candidate_keys(
    *,
    candidate_keys: list[Any],
    candidate_meta_by_pair: dict[tuple[int, str], dict[str, Any]],
    candidate_meta_by_index: dict[int, dict[str, Any]],
) -> list[TestAttemptDetail]:
    attempts: list[TestAttemptDetail] = []

    for candidate_key in candidate_keys:
        status = str(getattr(candidate_key, "status", "") or "").strip().lower()
        if status in {"", "available", "unused"}:
            continue

        candidate_index = int(getattr(candidate_key, "candidate_index", 0) or 0)
        retry_index = int(getattr(candidate_key, "retry_index", 0) or 0)
        key_id = str(getattr(candidate_key, "key_id", "") or "")
        meta = candidate_meta_by_pair.get((candidate_index, key_id)) or candidate_meta_by_index.get(
            candidate_index, {}
        )

        attempts.append(
            TestAttemptDetail(
                candidate_index=candidate_index,
                retry_index=retry_index,
                endpoint_api_format=str(meta.get("endpoint_api_format", "") or ""),
                endpoint_base_url=str(meta.get("endpoint_base_url", "") or ""),
                key_name=getattr(candidate_key, "key_name", None),
                key_id=key_id,
                auth_type=str(getattr(candidate_key, "auth_type", "") or ""),
                effective_model=(
                    str(meta.get("effective_model")) if meta.get("effective_model") else None
                ),
                status=status,
                skip_reason=getattr(candidate_key, "skip_reason", None),
                error_message=getattr(candidate_key, "error_message", None),
                status_code=getattr(candidate_key, "status_code", None),
                latency_ms=getattr(candidate_key, "latency_ms", None),
            )
        )

    attempts.sort(key=lambda attempt: (attempt.candidate_index, attempt.retry_index))
    return attempts


@router.post("/test-model-failover")
async def test_model_failover(
    request: TestModelFailoverRequest,
    http_request: Request,
    db: Session = Depends(get_db),
    current_user: User = Depends(get_current_user),
) -> Any:
    """
    带故障转移的模型测试

    支持两种模式:
    - global: 模拟外部请求，用全局模型名走候选解析（限定当前 Provider）
    - direct: 直接测试 provider_model_name，在当前 Provider 内多 Key 故障转移
    """
    from src.core.exceptions import ProviderNotAvailableException
    from src.services.candidate.failover import FailoverEngine
    from src.services.candidate.policy import RetryMode, RetryPolicy, SkipPolicy
    from src.services.candidate.recorder import CandidateRecorder
    from src.services.scheduling.candidate_builder import CandidateBuilder
    from src.services.scheduling.candidate_sorter import CandidateSorter
    from src.services.scheduling.scheduling_config import SchedulingConfig
    from src.services.task import TaskService
    from src.services.task.core.protocol import AttemptKind, AttemptResult

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

    candidates: list[ProviderCandidate] = []
    gm_obj = None
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

    client_format = request.api_format
    if request.mode == "global":
        sorter = CandidateSorter(SchedulingConfig())
        builder = CandidateBuilder(sorter)

        if not client_format and requested_endpoint is not None:
            client_format = str(getattr(requested_endpoint, "api_format", "") or "")
        if not client_format:
            for ep in provider.endpoints or []:
                if getattr(ep, "is_active", False):
                    client_format = str(getattr(ep, "api_format", "") or "")
                    if client_format:
                        break
        if not client_format:
            raise HTTPException(
                status_code=400, detail="No active endpoint found to determine API format"
            )

        from src.services.cache.model_cache import ModelCacheService

        model_mappings: list[str] = []
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
        if not client_format and requested_endpoint is not None:
            client_format = str(getattr(requested_endpoint, "api_format", "") or "")
        if not client_format and provider.endpoints:
            client_format = str(getattr(provider.endpoints[0], "api_format", "") or "")
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

    request_payload = {
        "model": request.model_name,
        "messages": [{"role": "user", "content": request.message or "Hello"}],
        "max_tokens": 30,
        "temperature": 0.7,
        "stream": True,
    }
    request_id = str(request.request_id or f"provider-test-{uuid4().hex[:12]}")
    request_timeout = float(getattr(provider, "request_timeout", 0) or TimeoutDefaults.HTTP_REQUEST)
    provider_type = str(getattr(provider, "provider_type", "") or "").lower()

    async def _request_func(provider_obj: Any, endpoint: Any, key: Any, candidate: Any) -> Any:
        effective_model = _resolve_test_effective_model(
            provider=provider,
            candidate=candidate,
            request=request,
            gm_obj=gm_obj,
            key=key,
        )
        response, auth_type = await _execute_test_check(
            provider_obj=provider_obj,
            endpoint=endpoint,
            key=key,
            effective_model=effective_model,
            request_payload=request_payload,
            request_timeout=request_timeout,
            provider_type=provider_type,
            user=current_user,
            db=db,
        )
        return _extract_test_response_or_raise(
            response=response,
            endpoint=endpoint,
            provider_name=str(provider_obj.name),
            auth_type=auth_type,
            api_key=key,
            db=db,
        )

    candidate_recorder = CandidateRecorder(db)
    task_service = TaskService(db)
    exec_result = None
    run_error: Exception | None = None
    concurrent_result: dict[str, Any] | None = None
    result_candidates = candidates

    if request.concurrency > 1:
        result_candidates = _flatten_test_candidates_for_concurrency(candidates)

    candidate_meta_by_pair, candidate_meta_by_index = _build_test_candidate_meta(
        candidates=result_candidates,
        provider=provider,
        request=request,
        gm_obj=gm_obj,
    )
    effective_model_by_candidate_index = {
        index: str(meta.get("effective_model") or request.model_name)
        for index, meta in candidate_meta_by_index.items()
    }

    try:
        if request.concurrency > 1:
            concurrent_result = await _run_concurrent_test(
                candidates=result_candidates,
                concurrency=request.concurrency,
                is_cancelled=http_request.is_disconnected,
                request_id=request_id,
                request_payload=dict(request_payload),
                effective_model_by_candidate_index=effective_model_by_candidate_index,
                request_timeout=request_timeout,
                provider_type=provider_type,
                user=current_user,
                db=db,
            )
        else:
            exec_result = await task_service.execute_sync_candidates(
                api_format=client_format or "openai:chat",
                model_name=request.model_name,
                candidates=result_candidates,
                request_func=_request_func,
                request_id=request_id,
                current_user=current_user,
                user_api_key=None,
                is_stream=False,
                capability_requirements=None,
                request_body_ref={"body": dict(request_payload)},
                request_headers=None,
                request_body=dict(request_payload),
                affinity_key=f"provider-test:{provider.id}",
                create_pending_usage=False,
                enable_cache_affinity=False,
                is_cancelled=http_request.is_disconnected,
            )
    except Exception as exc:
        run_error = exc
        logger.error("[test-model-failover] Error: {}", exc)

    try:
        candidate_keys = (
            list(concurrent_result.get("candidate_keys", []))
            if concurrent_result is not None
            else candidate_recorder.get_candidate_keys(request_id)
        )
    except Exception:
        candidate_keys = list(exec_result.candidate_keys) if exec_result else []
    attempts = _build_test_attempts_from_candidate_keys(
        candidate_keys=candidate_keys,
        candidate_meta_by_pair=candidate_meta_by_pair,
        candidate_meta_by_index=candidate_meta_by_index,
    )
    total_attempts = (
        int(exec_result.attempt_count)
        if exec_result is not None
        else (
            int(concurrent_result.get("attempt_count", 0))
            if concurrent_result is not None
            else sum(1 for attempt in attempts if attempt.status not in {"skipped", "cancelled"})
        )
    )

    if concurrent_result is not None and concurrent_result.get("success"):
        return TestModelFailoverResponse(
            success=True,
            model=request.model_name,
            provider={"id": str(provider.id), "name": provider.name},
            attempts=attempts,
            total_candidates=len(result_candidates),
            total_attempts=total_attempts,
            data={
                "stream": True,
                "response": concurrent_result.get("response"),
            },
            error=None,
        ).model_dump()

    if exec_result and exec_result.success:
        return TestModelFailoverResponse(
            success=True,
            model=request.model_name,
            provider={"id": str(provider.id), "name": provider.name},
            attempts=attempts,
            total_candidates=len(result_candidates),
            total_attempts=exec_result.attempt_count,
            data={
                "stream": True,
                "response": exec_result.response,
            },
            error=None,
        ).model_dump()

    error_message = None
    if run_error is not None:
        if isinstance(run_error, ProviderNotAvailableException) and getattr(
            run_error, "upstream_response", None
        ):
            error_message = str(run_error.upstream_response)[:500]
        if not error_message:
            error_message = str(run_error)
    if not error_message and concurrent_result is not None and concurrent_result.get("run_error"):
        error_message = str(concurrent_result.get("run_error"))
    if not error_message and exec_result is not None and exec_result.error_message:
        error_message = str(exec_result.error_message)
    if not error_message:
        failed_attempt = next(
            (attempt for attempt in reversed(attempts) if attempt.error_message),
            None,
        )
        error_message = (
            failed_attempt.error_message if failed_attempt else "服务暂时不可用，请稍后重试"
        )

    return TestModelFailoverResponse(
        success=False,
        model=request.model_name,
        provider={"id": str(provider.id), "name": provider.name},
        attempts=attempts,
        total_candidates=len(result_candidates),
        total_attempts=total_attempts,
        error=str(error_message)[:500],
    ).model_dump()
