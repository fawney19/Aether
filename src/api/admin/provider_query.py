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
from src.core.crypto import crypto_service
from src.core.logger import logger
from src.database.database import get_db
from src.models.database import Provider, ProviderEndpoint, User
from src.services.model.fetch_scheduler import (
    MODEL_FETCH_HTTP_TIMEOUT,
    get_upstream_models_from_cache,
    set_upstream_models_to_cache,
)
from src.services.model.upstream_fetcher import (
    _get_adapter_for_format,
    build_all_format_configs,
    fetch_models_from_endpoints,
)
from src.utils.auth_utils import get_current_user
from src.utils.ssl_utils import get_ssl_context

router = APIRouter(prefix="/api/admin/provider-query", tags=["Provider Query"])


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

    # 构建 api_format -> endpoint 映射
    format_to_endpoint: dict[str, ProviderEndpoint] = {}
    for endpoint in provider.endpoints:
        if endpoint.is_active:
            format_to_endpoint[endpoint.api_format] = endpoint

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

    # 并发获取所有 Key 的模型
    async def fetch_for_key(api_key: Any) -> Any:
        # 非强制刷新时，先检查缓存
        if not request.force_refresh:
            cached_models = await get_upstream_models_from_cache(request.provider_id, api_key.id)
            if cached_models is not None:
                return cached_models, None, True  # models, error, from_cache

        # 缓存未命中或强制刷新，实时获取
        try:
            api_key_value = crypto_service.decrypt(api_key.api_key)
        except Exception as e:
            logger.error(f"Failed to decrypt API key {api_key.id}: {e}")
            return [], f"Key {api_key.name or api_key.id}: decrypt failed", False

        endpoint_configs = build_all_format_configs(api_key_value, format_to_endpoint)
        models, errors, has_success = await fetch_models_from_endpoints(
            endpoint_configs, timeout=MODEL_FETCH_HTTP_TIMEOUT
        )

        # 写入缓存
        if models:
            await set_upstream_models_to_cache(request.provider_id, api_key.id, models)

        error = f"Key {api_key.name or api_key.id}: {'; '.join(errors)}" if errors else None
        return models, error, False  # models, error, from_cache

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


async def _fetch_models_for_single_key(
    provider: Provider,
    api_key_id: str,
    format_to_endpoint: dict[str, ProviderEndpoint],
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
            return {
                "success": True,
                "data": {"models": cached_models, "error": None, "from_cache": True},
                "provider": {
                    "id": provider.id,
                    "name": provider.name,
                },
            }

    # 缓存未命中或强制刷新，实时获取
    try:
        api_key_value = crypto_service.decrypt(api_key.api_key)
    except Exception as e:
        logger.error(f"Failed to decrypt API key: {e}")
        raise HTTPException(status_code=500, detail="Failed to decrypt API key")

    endpoint_configs = build_all_format_configs(api_key_value, format_to_endpoint)
    all_models, errors, has_success = await fetch_models_from_endpoints(
        endpoint_configs, timeout=MODEL_FETCH_HTTP_TIMEOUT
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

    try:
        api_key_value = crypto_service.decrypt(api_key.api_key)
    except Exception as e:
        logger.error(f"[test-model] Failed to decrypt API key: {e}")
        raise HTTPException(status_code=500, detail="Failed to decrypt API key")

    # 构建请求配置
    endpoint_config = {
        "api_key": api_key_value,
        "api_key_id": api_key.id,  # 添加API Key ID用于用量记录
        "base_url": endpoint.base_url,
        "api_format": endpoint.api_format,
        "extra_headers": get_extra_headers_from_endpoint(endpoint),
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

        # 准备测试请求数据
        check_request = {
            "model": request.model_name,
            "messages": [
                {"role": "user", "content": request.message or "Hello! This is a test message."}
            ],
            "max_tokens": 30,
            "temperature": 0.7,
        }

        # 获取端点规则（不在此处应用，传递给 check_endpoint 在格式转换后应用）
        body_rules = getattr(endpoint, "body_rules", None)
        header_rules = getattr(endpoint, "header_rules", None)
        extra_headers = endpoint_config.get("extra_headers") or {}

        if body_rules:
            logger.debug(f"[test-model] 将传递 body_rules 给 check_endpoint: {body_rules}")
        if header_rules:
            logger.debug(f"[test-model] 将传递 header_rules 给 check_endpoint: {header_rules}")

        # 发送测试请求
        async with httpx.AsyncClient(
            timeout=endpoint_config["timeout"], verify=get_ssl_context()
        ) as client:
            # 非流式测试
            logger.debug(f"[test-model] 开始非流式测试...")

            response = await adapter_class.check_endpoint(
                client,
                endpoint_config["base_url"],
                endpoint_config["api_key"],
                check_request,
                extra_headers if extra_headers else None,
                # 端点规则（在 check_endpoint 内部格式转换后应用）
                body_rules=body_rules,
                header_rules=header_rules,
                # 用量计算参数（现在强制记录）
                db=db,
                user=current_user,
                provider_name=provider.name,
                provider_id=provider.id,
                api_key_id=endpoint_config.get("api_key_id"),
                model_name=request.model_name,
            )

            # 记录提供商返回信息
            logger.debug(f"[test-model] 非流式测试结果:")
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

            if isinstance(parsed_body, dict) and "error" in parsed_body:
                error_obj = parsed_body["error"]
                # 兼容 error 可能是字典或字符串的情况
                if isinstance(error_obj, dict):
                    logger.debug(f"[test-model] Error Message: {error_obj.get('message')}")
                    raise HTTPException(status_code=500, detail=error_obj.get("message"))
                else:
                    logger.debug(f"[test-model] Error: {error_obj}")
                    raise HTTPException(status_code=500, detail=error_obj)
            elif "error" in response:
                logger.debug(f"[test-model] Error: {response['error']}")
                raise HTTPException(status_code=500, detail=response["error"])
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
                    "stream": False,
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
