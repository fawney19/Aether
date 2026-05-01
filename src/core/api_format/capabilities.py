from __future__ import annotations

import re
from copy import deepcopy
from dataclasses import dataclass
from typing import Any, Awaitable, Callable, Sequence

import httpx

from src.config.settings import config
from src.core.api_format.enums import ApiFamily, EndpointKind
from src.core.api_format.headers import (
    BROWSER_FINGERPRINT_HEADERS,
    build_adapter_headers_for_endpoint,
)
from src.core.api_format.signature import EndpointSignature, make_signature_key, parse_signature_key
from src.core.logger import logger
from src.core.provider_types import normalize_provider_type

ModelFetcher = Callable[
    [httpx.AsyncClient, str, str, str, dict[str, str] | None],
    Awaitable[tuple[list[dict[str, Any]], str | None]],
]
TotalInputContextResolver = Callable[[int, int, int], int]

_SENSITIVE_QUERY_PARAMS_PATTERN = re.compile(
    r"([?&])(key|api_key|apikey|token|secret|password|credential)=([^&]*)",
    re.IGNORECASE,
)


def _redact_url_for_log(url: str) -> str:
    return _SENSITIVE_QUERY_PARAMS_PATTERN.sub(r"\1\2=***", url)


def _default_total_input_context(
    input_tokens: int,
    cache_read_input_tokens: int,
    _cache_creation_input_tokens: int = 0,
) -> int:
    return input_tokens + cache_read_input_tokens


def _claude_total_input_context(
    input_tokens: int,
    cache_read_input_tokens: int,
    cache_creation_input_tokens: int = 0,
) -> int:
    return input_tokens + cache_creation_input_tokens + cache_read_input_tokens


@dataclass(frozen=True, slots=True)
class ApiFormatCapability:
    api_format: str
    billing_template: str | None = None
    total_input_context_resolver: TotalInputContextResolver = _default_total_input_context
    model_fetcher: ModelFetcher | None = None


@dataclass(frozen=True, slots=True)
class ProviderFormatCapability:
    provider_type: str
    endpoint_sig: str = ""
    same_format_variant: str | None = None
    cross_format_variant: str | None = None
    default_body_rules: tuple[dict[str, Any], ...] | None = None


@dataclass(frozen=True, slots=True)
class ProviderFormatBehavior:
    provider_type: str
    same_format_variant: str | None = None
    cross_format_variant: str | None = None


_registry: dict[str, ApiFormatCapability] = {}
_provider_registry: dict[tuple[str, str], ProviderFormatCapability] = {}


def _normalize_api_format(api_format: str | None) -> str:
    return str(api_format or "").strip().lower()


def _normalize_endpoint_sig(
    value: str | EndpointSignature | tuple[ApiFamily, EndpointKind] | tuple[Any, Any],
) -> str:
    if isinstance(value, str):
        try:
            return parse_signature_key(value).key
        except Exception:
            return value.strip().lower()
    if isinstance(value, EndpointSignature):
        return value.key
    if isinstance(value, tuple) and len(value) == 2:
        return make_signature_key(value[0], value[1])
    return str(value).strip().lower()


def register_api_format_capability(capability: ApiFormatCapability) -> None:
    """注册或覆盖 api_format 能力。"""
    fmt = _normalize_api_format(capability.api_format)
    if not fmt:
        raise ValueError("api_format 不能为空")
    _registry[fmt] = ApiFormatCapability(
        api_format=fmt,
        billing_template=capability.billing_template,
        total_input_context_resolver=capability.total_input_context_resolver,
        model_fetcher=capability.model_fetcher,
    )


def get_api_format_capability(api_format: str | None) -> ApiFormatCapability | None:
    """按 api_format 获取能力定义。"""
    return _registry.get(_normalize_api_format(api_format))


def list_api_format_capabilities() -> list[ApiFormatCapability]:
    """列出已注册能力。"""
    return list(_registry.values())


def register_provider_format_capability(
    provider_type: str,
    endpoint_sig: str | EndpointSignature | tuple[ApiFamily, EndpointKind] | tuple[Any, Any] = "",
    *,
    same_format_variant: str | None = None,
    cross_format_variant: str | None = None,
    default_body_rules: Sequence[dict[str, Any]] | None = None,
) -> None:
    """注册 provider + endpoint 维度的格式能力。"""
    pt = normalize_provider_type(provider_type)
    if not pt:
        raise ValueError("provider_type 不能为空")
    sig = _normalize_endpoint_sig(endpoint_sig)
    current = _provider_registry.get((pt, sig))
    _provider_registry[(pt, sig)] = ProviderFormatCapability(
        provider_type=pt,
        endpoint_sig=sig,
        same_format_variant=(
            same_format_variant
            if same_format_variant is not None
            else (current.same_format_variant if current else None)
        ),
        cross_format_variant=(
            cross_format_variant
            if cross_format_variant is not None
            else (current.cross_format_variant if current else None)
        ),
        default_body_rules=(
            tuple(deepcopy(list(default_body_rules)))
            if default_body_rules is not None
            else (current.default_body_rules if current else None)
        ),
    )


def get_provider_format_capability(
    provider_type: str | None,
    endpoint_sig: str | EndpointSignature | tuple[ApiFamily, EndpointKind] | tuple[Any, Any] = "",
) -> ProviderFormatCapability | None:
    """获取 provider + endpoint 维度能力，未命中时回退 provider 级默认能力。"""
    pt = normalize_provider_type(provider_type)
    if not pt:
        return None
    sig = _normalize_endpoint_sig(endpoint_sig)
    return _provider_registry.get((pt, sig)) or _provider_registry.get((pt, ""))


def register_provider_behavior_variant(
    provider_type: str,
    *,
    same_format: bool = False,
    cross_format: bool = False,
) -> None:
    """注册 provider 维度的格式变体标志。"""
    pt = normalize_provider_type(provider_type)
    current = get_provider_format_capability(pt)
    register_provider_format_capability(
        pt,
        same_format_variant=(
            pt if same_format else (current.same_format_variant if current else None)
        ),
        cross_format_variant=(
            pt if cross_format else (current.cross_format_variant if current else None)
        ),
    )


def register_provider_format_behavior(
    provider_type: str,
    *,
    same_format_variant: str | None = None,
    cross_format_variant: str | None = None,
) -> None:
    """兼容接口：按显式 variant 名称注册 provider 行为。"""
    register_provider_format_capability(
        provider_type,
        same_format_variant=same_format_variant,
        cross_format_variant=cross_format_variant,
    )


def get_provider_format_behavior(provider_type: str | None) -> ProviderFormatBehavior | None:
    """兼容接口：获取 provider 维度的格式变体能力。"""
    capability = get_provider_format_capability(provider_type)
    if capability is None:
        return None
    return ProviderFormatBehavior(
        provider_type=capability.provider_type,
        same_format_variant=capability.same_format_variant,
        cross_format_variant=capability.cross_format_variant,
    )


def get_provider_behavior_variants(
    provider_type: str | None,
    endpoint_sig: str | EndpointSignature | tuple[ApiFamily, EndpointKind] | tuple[Any, Any] = "",
) -> tuple[str | None, str | None]:
    capability = get_provider_format_capability(provider_type, endpoint_sig)
    if capability is None:
        return None, None
    return capability.same_format_variant, capability.cross_format_variant


def resolve_provider_variants_for_endpoint(
    provider_type: str | None,
    endpoint_sig: str | EndpointSignature | tuple[ApiFamily, EndpointKind] | tuple[Any, Any] = "",
) -> tuple[str | None, str | None]:
    return get_provider_behavior_variants(provider_type, endpoint_sig)


def register_provider_default_body_rules(
    provider_type: str,
    endpoint_sig: str | EndpointSignature | tuple[ApiFamily, EndpointKind] | tuple[Any, Any],
    rules: Sequence[dict[str, Any]],
) -> None:
    """注册 provider + endpoint 维度的默认 body_rules。"""
    register_provider_format_capability(
        provider_type,
        endpoint_sig,
        default_body_rules=rules,
    )


def get_provider_default_body_rules(
    provider_type: str | None,
    endpoint_sig: str | EndpointSignature | tuple[ApiFamily, EndpointKind] | tuple[Any, Any],
) -> list[dict[str, Any]] | None:
    """获取 provider + endpoint 维度默认 body_rules。"""
    capability = get_provider_format_capability(provider_type, endpoint_sig)
    if capability is None or capability.default_body_rules is None:
        return None
    return deepcopy(list(capability.default_body_rules))


def get_provider_default_body_rules_for_endpoint(
    provider_type: str | None,
    endpoint_sig: str | EndpointSignature | tuple[ApiFamily, EndpointKind] | tuple[Any, Any] = "",
) -> list[dict[str, Any]] | None:
    return get_provider_default_body_rules(provider_type, endpoint_sig)


def resolve_billing_template_for_api_format(api_format: str | None) -> str | None:
    """解析 api_format 对应的计费模板。"""
    capability = get_api_format_capability(api_format)
    if capability and capability.billing_template:
        return capability.billing_template

    family = _normalize_api_format(api_format).split(":", 1)[0]
    if family in {"claude", "openai", "gemini"}:
        return family
    return None


def compute_total_input_context_for_api_format(
    api_format: str | None,
    input_tokens: int,
    cache_read_input_tokens: int,
    cache_creation_input_tokens: int = 0,
) -> int:
    """按 api_format 计算阶梯计费口径中的总输入上下文。"""
    capability = get_api_format_capability(api_format)
    if capability is not None:
        return capability.total_input_context_resolver(
            input_tokens,
            cache_read_input_tokens,
            cache_creation_input_tokens,
        )

    if resolve_billing_template_for_api_format(api_format) == "claude":
        return _claude_total_input_context(
            input_tokens,
            cache_read_input_tokens,
            cache_creation_input_tokens,
        )

    return _default_total_input_context(
        input_tokens,
        cache_read_input_tokens,
        cache_creation_input_tokens,
    )


def _build_v1_models_url(base_url: str) -> str:
    from src.utils.url_utils import join_url

    return join_url(base_url, "/v1/models")


async def _fetch_openai_models(
    client: httpx.AsyncClient,
    base_url: str,
    api_key: str,
    api_format: str,
    extra_headers: dict[str, str] | None,
) -> tuple[list[dict[str, Any]], str | None]:
    headers = build_adapter_headers_for_endpoint(api_format, api_key, extra_headers)
    models_url = _build_v1_models_url(base_url)

    try:
        response = await client.get(models_url, headers=headers)
        logger.debug("OpenAI models request to {}: status={}", models_url, response.status_code)
        if response.status_code == 200:
            data = response.json()
            models: list[dict[str, Any]] = []
            if isinstance(data, dict) and isinstance(data.get("data"), list):
                models = [m for m in data["data"] if isinstance(m, dict)]
            elif isinstance(data, list):
                models = [m for m in data if isinstance(m, dict)]

            for model in models:
                model.setdefault("api_format", api_format)
            return models, None

        error_body = response.text[:500] if response.text else "(empty)"
        error_msg = f"HTTP {response.status_code}: {error_body}"
        logger.warning("OpenAI models request to {} failed: {}", models_url, error_msg)
        return [], error_msg
    except Exception as exc:
        error_msg = f"Request error: {str(exc)}"
        logger.warning("Failed to fetch models from {}: {}", models_url, exc)
        return [], error_msg


async def _fetch_openai_cli_models(
    client: httpx.AsyncClient,
    base_url: str,
    api_key: str,
    api_format: str,
    extra_headers: dict[str, str] | None,
) -> tuple[list[dict[str, Any]], str | None]:
    headers = {"User-Agent": config.internal_user_agent_openai_cli}
    if extra_headers:
        headers.update(extra_headers)
    return await _fetch_openai_models(client, base_url, api_key, api_format, headers)


async def _fetch_claude_models_paginated(
    client: httpx.AsyncClient,
    base_url: str,
    headers: dict[str, str],
    api_format: str,
) -> tuple[list[dict[str, Any]], str | None]:
    models_url = _build_v1_models_url(base_url)

    try:
        all_models: list[dict[str, Any]] = []
        seen_ids: set[str] = set()
        after_id: str | None = None
        limit = 100
        max_pages = 20

        for _ in range(max_pages):
            params: dict[str, Any] = {"limit": limit}
            if after_id:
                params["after_id"] = after_id

            response = await client.get(models_url, headers=headers, params=params)
            logger.debug(
                "Claude models request to {}: status={}, after_id={}",
                models_url,
                response.status_code,
                after_id,
            )
            if response.status_code != 200:
                error_body = response.text[:500] if response.text else "(empty)"
                error_msg = f"HTTP {response.status_code}: {error_body}"
                logger.warning("Claude models request to {} failed: {}", models_url, error_msg)
                return [], error_msg

            data = response.json()
            page_models: list[dict[str, Any]] = []
            if isinstance(data, dict) and isinstance(data.get("data"), list):
                page_models = [m for m in data["data"] if isinstance(m, dict)]
            elif isinstance(data, list):
                page_models = [m for m in data if isinstance(m, dict)]

            for model in page_models:
                model_id = model.get("id")
                if isinstance(model_id, str) and model_id and model_id in seen_ids:
                    continue
                if isinstance(model_id, str) and model_id:
                    seen_ids.add(model_id)
                model.setdefault("api_format", api_format)
                all_models.append(model)

            if not isinstance(data, dict):
                break

            has_more = bool(data.get("has_more"))
            last_id = data.get("last_id")
            if not has_more:
                break
            if not isinstance(last_id, str) or not last_id:
                break
            if after_id == last_id:
                break
            after_id = last_id

        return all_models, None
    except Exception as exc:
        error_msg = f"Request error: {str(exc)}"
        logger.warning("Failed to fetch Claude models from {}: {}", models_url, exc)
        return [], error_msg


async def _fetch_claude_models(
    client: httpx.AsyncClient,
    base_url: str,
    api_key: str,
    api_format: str,
    extra_headers: dict[str, str] | None,
    *,
    force_bearer_fallback: bool,
) -> tuple[list[dict[str, Any]], str | None]:
    headers = build_adapter_headers_for_endpoint(api_format, api_key, extra_headers)
    if force_bearer_fallback and "authorization" not in {k.lower() for k in headers}:
        headers["Authorization"] = f"Bearer {api_key}"
    return await _fetch_claude_models_paginated(client, base_url, headers, api_format)


async def _fetch_claude_chat_models(
    client: httpx.AsyncClient,
    base_url: str,
    api_key: str,
    api_format: str,
    extra_headers: dict[str, str] | None,
) -> tuple[list[dict[str, Any]], str | None]:
    return await _fetch_claude_models(
        client,
        base_url,
        api_key,
        api_format,
        extra_headers,
        force_bearer_fallback=True,
    )


async def _fetch_claude_cli_models(
    client: httpx.AsyncClient,
    base_url: str,
    api_key: str,
    api_format: str,
    extra_headers: dict[str, str] | None,
) -> tuple[list[dict[str, Any]], str | None]:
    headers = {"User-Agent": config.internal_user_agent_claude_cli}
    if extra_headers:
        headers.update(extra_headers)
    return await _fetch_claude_models(
        client,
        base_url,
        api_key,
        api_format,
        headers,
        force_bearer_fallback=False,
    )


async def _fetch_gemini_models(
    client: httpx.AsyncClient,
    base_url: str,
    api_key: str,
    api_format: str,
    extra_headers: dict[str, str] | None,
) -> tuple[list[dict[str, Any]], str | None]:
    from src.utils.url_utils import join_url

    models_url = f"{join_url(base_url, '/v1beta/models')}?key={api_key}"

    headers: dict[str, str] = {**BROWSER_FINGERPRINT_HEADERS}
    if extra_headers:
        headers.update(extra_headers)

    try:
        response = await client.get(models_url, headers=headers)
        logger.debug(
            "Gemini models request to {}: status={}",
            _redact_url_for_log(models_url),
            response.status_code,
        )
        if response.status_code == 200:
            data = response.json()
            if isinstance(data, dict) and isinstance(data.get("models"), list):
                out: list[dict[str, Any]] = []
                for model in data["models"]:
                    if not isinstance(model, dict):
                        continue
                    out.append(
                        {
                            "id": str(model.get("name", "")).replace("models/", ""),
                            "owned_by": "google",
                            "display_name": model.get("displayName", ""),
                            "api_format": api_format,
                        }
                    )
                return out, None
            return [], None

        error_body = response.text[:500] if response.text else "(empty)"
        error_msg = f"HTTP {response.status_code}: {error_body}"
        logger.warning(
            "Gemini models request to {} failed: {}",
            _redact_url_for_log(models_url),
            error_msg,
        )
        return [], error_msg
    except Exception as exc:
        sanitized_error = _redact_url_for_log(str(exc))
        error_msg = f"Request error: {sanitized_error}"
        logger.warning(
            "Failed to fetch Gemini models from {}: {}",
            _redact_url_for_log(models_url),
            sanitized_error,
        )
        return [], error_msg


async def _fetch_gemini_cli_models(
    client: httpx.AsyncClient,
    base_url: str,
    api_key: str,
    api_format: str,
    extra_headers: dict[str, str] | None,
) -> tuple[list[dict[str, Any]], str | None]:
    headers = {"User-Agent": config.internal_user_agent_gemini_cli}
    if extra_headers:
        headers.update(extra_headers)
    return await _fetch_gemini_models(client, base_url, api_key, api_format, headers)


async def fetch_models_for_api_format(
    client: httpx.AsyncClient,
    *,
    api_format: str,
    base_url: str,
    api_key: str,
    extra_headers: dict[str, str] | None = None,
) -> tuple[list[dict[str, Any]], str | None]:
    """按 api_format 获取模型列表。"""
    normalized_api_format = _normalize_api_format(api_format)
    capability = get_api_format_capability(normalized_api_format)
    if capability is None or capability.model_fetcher is None:
        return [], f"Unknown API format: {api_format}"

    return await capability.model_fetcher(
        client,
        base_url,
        api_key,
        normalized_api_format,
        extra_headers,
    )


def _register_builtin_capabilities() -> None:
    register_api_format_capability(
        ApiFormatCapability(
            api_format="openai:chat",
            billing_template="openai",
            model_fetcher=_fetch_openai_models,
        )
    )
    register_api_format_capability(
        ApiFormatCapability(
            api_format="openai:cli",
            billing_template="openai",
            model_fetcher=_fetch_openai_cli_models,
        )
    )
    register_api_format_capability(
        ApiFormatCapability(
            api_format="openai:compact",
            billing_template="openai",
            model_fetcher=_fetch_openai_cli_models,
        )
    )
    register_api_format_capability(
        ApiFormatCapability(
            api_format="claude:chat",
            billing_template="claude",
            total_input_context_resolver=_claude_total_input_context,
            model_fetcher=_fetch_claude_chat_models,
        )
    )
    register_api_format_capability(
        ApiFormatCapability(
            api_format="claude:cli",
            billing_template="claude",
            total_input_context_resolver=_claude_total_input_context,
            model_fetcher=_fetch_claude_cli_models,
        )
    )
    register_api_format_capability(
        ApiFormatCapability(
            api_format="gemini:chat",
            billing_template="gemini",
            model_fetcher=_fetch_gemini_models,
        )
    )
    register_api_format_capability(
        ApiFormatCapability(
            api_format="gemini:cli",
            billing_template="gemini",
            model_fetcher=_fetch_gemini_cli_models,
        )
    )


_register_builtin_capabilities()


__all__ = [
    "ApiFormatCapability",
    "ProviderFormatBehavior",
    "ProviderFormatCapability",
    "compute_total_input_context_for_api_format",
    "fetch_models_for_api_format",
    "get_api_format_capability",
    "get_provider_behavior_variants",
    "get_provider_default_body_rules",
    "get_provider_default_body_rules_for_endpoint",
    "get_provider_format_behavior",
    "get_provider_format_capability",
    "list_api_format_capabilities",
    "register_api_format_capability",
    "register_provider_behavior_variant",
    "register_provider_default_body_rules",
    "register_provider_format_behavior",
    "register_provider_format_capability",
    "resolve_billing_template_for_api_format",
    "resolve_provider_variants_for_endpoint",
]
