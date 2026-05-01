"""Vertex AI URL 构建（Transport Hook）。

Vertex AI Gemini / Imagen 支持两种认证路径：

- API Key + Gemini/Imagen (Express mode):
  https://aiplatform.googleapis.com/v1/publishers/google/models/{model}:{action}?key={API_KEY}
- Service Account + Gemini/Imagen:
  https://{region}-aiplatform.googleapis.com/v1/projects/{project_id}/locations/{region}/publishers/google/models/{model}:{action}

Claude 仍走标准 Vertex AI Service Account 路径：

- Service Account + Claude:
  https://{region}-aiplatform.googleapis.com/v1/projects/{project_id}/locations/{region}/publishers/anthropic/models/{model}:{action}
"""

from __future__ import annotations

import json
from typing import Any
from urllib.parse import urlencode

from src.core.logger import logger
from src.core.provider_types import ProviderType, normalize_provider_type
from src.services.provider.adapters.vertex_ai.constants import (
    API_KEY_BASE_URL,
    DEFAULT_FORMAT,
    DEFAULT_MODEL_REGIONS,
    MODEL_FORMAT_MAPPING,
)
from src.services.provider.format import normalize_endpoint_signature
from src.services.provider.transport import looks_like_vertex_ai_host, redact_url_for_log


def is_vertex_ai_context(
    *,
    base_url: str | None = None,
    provider_type: Any = None,
    endpoint: Any = None,
    key: Any = None,
) -> bool:
    """Best-effort 判断当前测试/请求上下文是否应视为 Vertex AI。"""
    if normalize_provider_type(provider_type) == ProviderType.VERTEX_AI.value:
        return True

    for obj in (endpoint, key):
        provider = getattr(obj, "provider", None) if obj is not None else None
        if normalize_provider_type(getattr(provider, "provider_type", None)) == (
            ProviderType.VERTEX_AI.value
        ):
            return True

    candidate_base_url = str(base_url or getattr(endpoint, "base_url", "") or "").strip()
    if not candidate_base_url:
        return False

    endpoint_sig = str(getattr(endpoint, "api_format", "") or "").strip()
    auth_type = str(getattr(key, "auth_type", "") or "").strip()
    return looks_like_vertex_ai_host(candidate_base_url, endpoint_sig, auth_type)


def get_effective_format(
    model: str,
    auth_config: dict[str, Any] | None = None,
) -> str:
    """获取 Vertex AI 模式下模型的实际 API 格式。

    优先级：
    1. auth_config.model_format_mapping 中的精确匹配
    2. auth_config.model_format_mapping 中的前缀匹配
    3. 内置 MODEL_FORMAT_MAPPING 前缀匹配
    4. auth_config.default_format
    5. 内置 DEFAULT_FORMAT
    """
    user_format_mapping: dict[str, str] = {}
    user_default_format: str | None = None

    if auth_config:
        user_format_mapping = auth_config.get("model_format_mapping", {})
        user_default_format = auth_config.get("default_format")

    # 1. 用户配置：精确匹配
    if model in user_format_mapping:
        try:
            return normalize_endpoint_signature(user_format_mapping[model])
        except Exception:
            logger.warning(
                "Invalid vertex_ai model_format_mapping value for model '{}': {!r}",
                model,
                user_format_mapping[model],
            )

    # 2. 用户配置：前缀匹配
    for prefix, api_format in user_format_mapping.items():
        if prefix.endswith("-") and model.startswith(prefix):
            try:
                return normalize_endpoint_signature(api_format)
            except Exception:
                logger.warning(
                    "Invalid vertex_ai model_format_mapping value for prefix '{}': {!r}",
                    prefix,
                    api_format,
                )
                break

    # 3. 内置配置：前缀匹配
    for prefix, api_format in MODEL_FORMAT_MAPPING.items():
        if model.startswith(prefix):
            return normalize_endpoint_signature(api_format)

    # 4. 用户默认格式
    if user_default_format:
        try:
            return normalize_endpoint_signature(user_default_format)
        except Exception:
            logger.warning("Invalid vertex_ai default_format: {!r}", user_default_format)

    # 5. 内置默认格式
    return DEFAULT_FORMAT


def build_vertex_ai_url(
    endpoint: Any,
    *,
    is_stream: bool,
    effective_query_params: dict[str, Any],
    path_params: dict[str, Any] | None = None,
    key: Any = None,
    decrypted_auth_config: dict[str, Any] | None = None,
) -> str:
    """Vertex AI transport hook — 统一 URL 构建入口。"""
    from src.core.exceptions import InvalidRequestException

    model = str((path_params or {}).get("model", "") or "").strip()
    if not model:
        raise InvalidRequestException("Vertex AI 请求缺少 model 参数")

    auth_type = str(getattr(key, "auth_type", "api_key") or "api_key").strip().lower()
    is_claude_model = model.startswith("claude-")

    if auth_type == "api_key":
        if is_claude_model:
            raise InvalidRequestException(
                "Vertex API Key 不支持 Claude 模型，请改用 Service Account 认证。"
            )
        return _build_api_key_url(
            key=key,
            path_params=path_params,
            query_params=effective_query_params,
            is_stream=is_stream,
        )

    # service_account（以及向后兼容旧的 "vertex_ai" auth_type）
    return _build_service_account_url(
        key=key,
        path_params=path_params,
        query_params=effective_query_params,
        is_stream=is_stream,
        decrypted_auth_config=decrypted_auth_config,
    )


def _build_api_key_url(
    key: Any,
    *,
    path_params: dict[str, Any] | None = None,
    query_params: dict[str, Any] | None = None,
    is_stream: bool = False,
) -> str:
    """构建 API Key 认证的全局端点 URL。

    格式: https://aiplatform.googleapis.com/v1/publishers/google/models/{model}:{action}?key={API_KEY}
    """
    from src.core.crypto import crypto_service
    from src.core.exceptions import InvalidRequestException

    model = (path_params or {}).get("model", "")
    if not model:
        raise InvalidRequestException("Vertex AI 请求缺少 model 参数")

    from src.utils.url_utils import join_url

    action = "streamGenerateContent" if is_stream else "generateContent"
    path = f"/v1/publishers/google/models/{model}:{action}"
    url = join_url(API_KEY_BASE_URL, path)

    # 构建查询参数
    params = dict(query_params) if query_params else {}

    # 附加 API Key
    api_key_value = crypto_service.decrypt(key.api_key) if key else ""
    if api_key_value:
        params["key"] = api_key_value

    # Gemini 流式请求使用 SSE
    if is_stream:
        params.setdefault("alt", "sse")

    params.pop("beta", None)

    if params:
        query_string = urlencode(params, doseq=True)
        if query_string:
            url = f"{url}?{query_string}"

    logger.debug("Vertex AI (API Key) URL: {}", redact_url_for_log(url))
    return url


def _build_service_account_url(
    key: Any,
    *,
    path_params: dict[str, Any] | None = None,
    query_params: dict[str, Any] | None = None,
    is_stream: bool = False,
    decrypted_auth_config: dict[str, Any] | None = None,
) -> str:
    """构建 Service Account 认证的 Vertex AI 区域端点 URL。"""
    from src.core.crypto import crypto_service
    from src.core.exceptions import InvalidRequestException

    # 优先使用传入的已解密配置，避免重复解密
    auth_config: dict[str, Any] = {}
    if decrypted_auth_config:
        auth_config = decrypted_auth_config
    else:
        # 兜底：从 key.auth_config 解密（理论上不应走到这里）
        raw_auth_config = getattr(key, "auth_config", None) if key else None
        if raw_auth_config:
            try:
                if isinstance(raw_auth_config, dict):
                    auth_config = raw_auth_config
                else:
                    decrypted_config = crypto_service.decrypt(raw_auth_config)
                    auth_config = json.loads(decrypted_config)
            except Exception as e:
                logger.error("解密 Vertex AI auth_config 失败: {}", e)
                auth_config = {}

    # 获取必需的配置
    project_id = auth_config.get("project_id")
    if not project_id:
        raise InvalidRequestException(
            "Vertex AI 配置缺少 project_id（请在 Key 的 auth_config 中提供）"
        )

    # 获取模型名
    model = (path_params or {}).get("model", "")
    if not model:
        raise InvalidRequestException("Vertex AI 请求缺少 model 参数")

    # 确定 region（优先级：用户配置 > 内置默认 > 用户默认 > 兜底）
    user_model_regions = auth_config.get("model_regions", {})
    user_default_region = auth_config.get("region")

    if model in user_model_regions:
        region = user_model_regions[model]
    elif model in DEFAULT_MODEL_REGIONS:
        region = DEFAULT_MODEL_REGIONS[model]
    elif user_default_region:
        region = user_default_region
    else:
        region = "global"

    if model.startswith("claude-"):
        publisher = "anthropic"
        action = "streamRawPredict" if is_stream else "rawPredict"
    else:
        publisher = "google"
        action = "streamGenerateContent" if is_stream else "generateContent"

    from src.utils.url_utils import join_url

    # 构建 URL（global region 使用不同的 URL 格式）
    if region == "global":
        base_url = "https://aiplatform.googleapis.com"
    else:
        base_url = f"https://{region}-aiplatform.googleapis.com"
    path = f"/v1/projects/{project_id}/locations/{region}/publishers/{publisher}/models/{model}:{action}"
    url = join_url(base_url, path)

    # 添加查询参数
    effective_query_params = dict(query_params) if query_params else {}
    # Gemini 流式请求使用 SSE 格式，Claude 不需要
    if is_stream and not model.startswith("claude-"):
        effective_query_params.setdefault("alt", "sse")
    # 移除不适用于 Vertex AI 的参数
    effective_query_params.pop("beta", None)

    if effective_query_params:
        query_string = urlencode(effective_query_params, doseq=True)
        if query_string:
            url = f"{url}?{query_string}"

    logger.debug("Vertex AI (SA) URL: {} (region={})", redact_url_for_log(url), region)
    return url
