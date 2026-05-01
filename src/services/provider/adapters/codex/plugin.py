"""Codex provider plugin — 统一注册入口。

将 Codex 对各通用 registry / capability registry 的注册集中在一个文件中：
- Transport Hook (URL 构建)
- Auth Enricher (OAuth enrichment)
- Provider Format Capability（默认 body_rules）
- Model Fetcher (fixed catalog — Codex has no /v1/models endpoint)

新增 provider 时参照此文件创建对应的 plugin.py 即可。
"""

from __future__ import annotations

import hashlib
from collections.abc import Mapping
from typing import Any
from urllib.parse import urlencode

from src.core.logger import logger

# ---------------------------------------------------------------------------
# Preset model catalog
# ---------------------------------------------------------------------------
# Codex upstream (chatgpt.com/backend-api/codex) has no /v1/models endpoint.
# We use the unified preset models registry from preset_models.py.
from src.services.provider.preset_models import create_preset_models_fetcher

fetch_models_codex = create_preset_models_fetcher("codex")


# ---------------------------------------------------------------------------
# Transport Hook
# ---------------------------------------------------------------------------


def _get_header_value(headers: Mapping[str, Any] | None, header_name: str) -> str | None:
    if not isinstance(headers, Mapping):
        return None

    target = str(header_name or "").strip().lower()
    if not target:
        return None

    for name, value in headers.items():
        if str(name or "").strip().lower() != target:
            continue
        normalized = str(value or "").strip()
        return normalized or None
    return None


def _build_short_header_id(seed: str) -> str:
    return hashlib.sha256(seed.encode()).hexdigest()[:16]


def _normalize_codex_debug_organizations(value: Any) -> list[dict[str, Any]]:
    if not isinstance(value, list):
        return []

    items: list[dict[str, Any]] = []
    for raw in value:
        if not isinstance(raw, Mapping):
            continue

        normalized: dict[str, Any] = {}
        org_id = str(raw.get("id") or "").strip()
        if org_id:
            normalized["id"] = org_id

        title = str(raw.get("title") or "").strip()
        if title:
            normalized["title"] = title

        role = str(raw.get("role") or "").strip()
        if role:
            normalized["role"] = role

        if "is_default" in raw:
            normalized["is_default"] = bool(raw.get("is_default"))

        if normalized:
            items.append(normalized)

    return items


def _build_safe_codex_debug_snapshot(values: Mapping[str, Any] | None) -> dict[str, Any]:
    if not isinstance(values, Mapping):
        return {}

    snapshot: dict[str, Any] = {}
    for field in (
        "email",
        "account_id",
        "account_user_id",
        "plan_type",
        "user_id",
        "account_name",
    ):
        raw = values.get(field)
        if isinstance(raw, str):
            normalized = raw.strip()
            if normalized:
                snapshot[field] = normalized
        elif raw is not None:
            snapshot[field] = raw

    organizations = _normalize_codex_debug_organizations(values.get("organizations"))
    if organizations:
        snapshot["organizations"] = organizations

    return snapshot


def _build_codex_headers(
    request_body: Any,
    original_headers: Mapping[str, Any] | None,
    *,
    include_conversation_id: bool,
    decrypted_auth_config: dict[str, Any] | None = None,
) -> dict[str, str]:
    """Codex upstream: chatgpt-account-id + session_id + conversation_id."""
    headers: dict[str, str] = {}

    if decrypted_auth_config:
        account_id = str(decrypted_auth_config.get("account_id") or "").strip()
        if account_id:
            headers["chatgpt-account-id"] = account_id

    if isinstance(request_body, dict):
        cache_key = str(request_body.get("prompt_cache_key") or "").strip()
        if cache_key:
            short_id = _build_short_header_id(cache_key)
            if not _get_header_value(original_headers, "session_id"):
                headers["session_id"] = short_id
            if include_conversation_id and not _get_header_value(
                original_headers, "conversation_id"
            ):
                headers["conversation_id"] = short_id

    return headers


def build_codex_cli_headers(
    request_body: Any,
    original_headers: Mapping[str, Any] | None,
    *,
    decrypted_auth_config: dict[str, Any] | None = None,
) -> dict[str, str]:
    from src.services.provider.adapters.codex.context import is_codex_compact_request

    return _build_codex_headers(
        request_body,
        original_headers,
        include_conversation_id=not is_codex_compact_request(endpoint_sig="openai:cli"),
        decrypted_auth_config=decrypted_auth_config,
    )


def build_codex_compact_headers(
    request_body: Any,
    original_headers: Mapping[str, Any] | None,
    *,
    decrypted_auth_config: dict[str, Any] | None = None,
) -> dict[str, str]:
    return _build_codex_headers(
        request_body,
        original_headers,
        include_conversation_id=False,
        decrypted_auth_config=decrypted_auth_config,
    )


def build_codex_url(
    endpoint: Any,
    *,
    is_stream: bool,
    effective_query_params: dict[str, Any],
    **_kwargs: Any,
) -> str:
    """构建 Codex OAuth URL。

    Codex upstream (chatgpt.com/backend-api/codex) 使用 /responses
    而非标准 OpenAI 的 /v1/responses。compact 模式使用 /responses/compact。
    base_url 应只填到 codex namespace（不含 /responses），由本函数补齐 path。
    """
    _ = is_stream  # Codex 不需要根据 stream 切换路径

    from src.services.provider.adapters.codex.context import is_codex_compact_request
    from src.utils.url_utils import join_url

    endpoint_sig = str(getattr(endpoint, "api_format", "") or "").strip().lower()
    is_compact = is_codex_compact_request(endpoint_sig=endpoint_sig)

    suffix = "/responses/compact" if is_compact else "/responses"
    url = join_url(endpoint.base_url, suffix)
    if effective_query_params:
        query_string = urlencode(effective_query_params, doseq=True)
        if query_string:
            url = f"{url}?{query_string}"
    return url


# ---------------------------------------------------------------------------
# Auth Enricher
# ---------------------------------------------------------------------------


async def enrich_codex(
    auth_config: dict[str, Any],
    token_response: dict[str, Any],
    access_token: str,
    proxy_config: dict[str, Any] | None,
) -> dict[str, Any]:
    """Codex auth_config enrichment: parse token claims -> account/team identity metadata."""
    from src.core.provider_oauth_utils import (
        fetch_openai_account_name,
        parse_codex_id_token,
    )

    def _read_non_empty_str(*values: Any) -> str | None:
        for value in values:
            if isinstance(value, str):
                normalized = value.strip()
                if normalized:
                    return normalized
        return None

    # Prefer explicit fields if token endpoint returns them directly.
    direct_account_id = _read_non_empty_str(
        token_response.get("account_id"),
        token_response.get("accountId"),
        token_response.get("chatgpt_account_id"),
        token_response.get("chatgptAccountId"),
    )
    if direct_account_id and not auth_config.get("account_id"):
        auth_config["account_id"] = direct_account_id

    direct_account_user_id = _read_non_empty_str(
        token_response.get("account_user_id"),
        token_response.get("accountUserId"),
        token_response.get("chatgpt_account_user_id"),
        token_response.get("chatgptAccountUserId"),
    )
    if direct_account_user_id and not auth_config.get("account_user_id"):
        auth_config["account_user_id"] = direct_account_user_id

    direct_plan_type = _read_non_empty_str(
        token_response.get("plan_type"),
        token_response.get("planType"),
        token_response.get("chatgpt_plan_type"),
        token_response.get("chatgptPlanType"),
    )
    if direct_plan_type and not auth_config.get("plan_type"):
        auth_config["plan_type"] = direct_plan_type

    direct_user_id = _read_non_empty_str(
        token_response.get("user_id"),
        token_response.get("userId"),
        token_response.get("chatgpt_user_id"),
        token_response.get("chatgptUserId"),
    )
    if direct_user_id and not auth_config.get("user_id"):
        auth_config["user_id"] = direct_user_id

    direct_email = _read_non_empty_str(token_response.get("email"))
    if direct_email and not auth_config.get("email"):
        auth_config["email"] = direct_email

    direct_snapshot = _build_safe_codex_debug_snapshot(
        {
            "email": direct_email,
            "account_id": direct_account_id,
            "account_user_id": direct_account_user_id,
            "plan_type": direct_plan_type,
            "user_id": direct_user_id,
        }
    )

    logger.debug(
        "Codex enrich_auth_config: id_token_present={} access_token_present={} token_keys={}",
        bool(token_response.get("id_token") or token_response.get("idToken")),
        bool(token_response.get("access_token") or token_response.get("accessToken")),
        list(token_response.keys()),
    )
    logger.debug("Codex direct token response values: {}", direct_snapshot)

    token_candidates = [
        ("id_token", token_response.get("id_token")),
        ("idToken", token_response.get("idToken")),
        ("access_token", token_response.get("access_token")),
        ("accessToken", token_response.get("accessToken")),
    ]
    for source_name, token_payload in token_candidates:
        codex_info = parse_codex_id_token(token_payload)
        if not codex_info:
            continue
        logger.debug(
            "Codex parsed token values: source={} fields={} values={}",
            source_name,
            list(codex_info.keys()),
            _build_safe_codex_debug_snapshot(codex_info),
        )
        for key, value in codex_info.items():
            if not auth_config.get(key):
                auth_config[key] = value

    account_id = _read_non_empty_str(auth_config.get("account_id"))
    if account_id:
        account_name = await fetch_openai_account_name(
            access_token,
            account_id,
            proxy_config=proxy_config,
            timeout_seconds=10.0,
        )
        logger.debug(
            "Codex account_name lookup: account_id={} resolved_account_name={}",
            account_id,
            account_name,
        )
        if account_name:
            auth_config["account_name"] = account_name

    logger.debug(
        "Codex enrich_auth_config final metadata: {}",
        _build_safe_codex_debug_snapshot(auth_config),
    )

    return auth_config


# ---------------------------------------------------------------------------
# Unified Registration
# ---------------------------------------------------------------------------


def register_all() -> None:
    """一次性注册 Codex 的所有 hooks 到各通用 registry。"""
    from src.core.api_format.capabilities import register_provider_format_capability
    from src.core.provider_oauth_utils import register_auth_enricher
    from src.services.model.upstream_fetcher import UpstreamModelsFetcherRegistry
    from src.services.provider.body_transformer import register_body_transformer
    from src.services.provider.transport import register_transport_hook
    from src.services.provider.upstream_headers import register_upstream_headers_hook

    from src.services.provider.adapters.codex.image_transform import (
        transform_image_request_to_responses,
    )

    # Transport
    register_transport_hook("codex", "openai:cli", build_codex_url)
    register_transport_hook("codex", "openai:compact", build_codex_url)
    register_transport_hook("codex", "openai:image", build_codex_url)
    register_upstream_headers_hook("codex", "openai:cli", build_codex_cli_headers)
    register_upstream_headers_hook("codex", "openai:compact", build_codex_compact_headers)
    register_upstream_headers_hook("codex", "openai:image", build_codex_cli_headers)

    # Body transformer: Codex image reverse proxy rewrites the full request body
    register_body_transformer(
        "codex",
        "openai:image",
        transform_image_request_to_responses,
    )

    # Auth
    register_auth_enricher("codex", enrich_codex)

    # Provider Format Capability：默认 body_rules
    from src.core.api_format.metadata import CODEX_DEFAULT_BODY_RULES

    register_provider_format_capability(
        "codex",
        "openai:cli",
        same_format_variant="codex",
        cross_format_variant="codex",
        default_body_rules=CODEX_DEFAULT_BODY_RULES,
    )
    register_provider_format_capability(
        "codex",
        "openai:compact",
        same_format_variant="codex",
        cross_format_variant="codex",
        default_body_rules=CODEX_DEFAULT_BODY_RULES,
    )
    # Codex image 走 responses payload，不复用 CLI 的 body_rules
    register_provider_format_capability(
        "codex",
        "openai:image",
        same_format_variant="codex",
        cross_format_variant="codex",
        default_body_rules=(),
    )

    # Export: Codex uses the default export builder (strip null + temp fields)
    # No need to register a custom one — the default in export.py suffices.

    # Model Fetcher
    UpstreamModelsFetcherRegistry.register(
        provider_types=["codex"],
        fetcher=fetch_models_codex,
    )
