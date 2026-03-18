"""Codex provider plugin — 统一注册入口。

将 Codex 对各通用 registry / capability registry 的注册集中在一个文件中：
- Transport Hook (URL 构建)
- Auth Enricher (OAuth enrichment)
- Provider Format Capability（默认 body_rules）
- Model Fetcher (fixed catalog — Codex has no /v1/models endpoint)

新增 provider 时参照此文件创建对应的 plugin.py 即可。
"""

from __future__ import annotations

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
    """
    _ = is_stream  # Codex 不需要根据 stream 切换路径

    endpoint_sig = str(getattr(endpoint, "api_format", "") or "").strip().lower()
    from src.services.provider.adapters.codex.context import is_codex_compact_request

    is_compact = is_codex_compact_request(endpoint_sig=endpoint_sig)

    base = str(endpoint.base_url).rstrip("/")
    # 如果用户已在 base_url 中包含了 /responses，不要重复追加
    if base.endswith("/responses"):
        url = f"{base}/compact" if is_compact else base
    elif base.endswith("/responses/compact"):
        url = base if is_compact else base.removesuffix("/compact")
    else:
        suffix = "/responses/compact" if is_compact else "/responses"
        url = f"{base}{suffix}"
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
    access_token: str,  # noqa: ARG001
    proxy_config: dict[str, Any] | None,  # noqa: ARG001
) -> dict[str, Any]:
    """Codex auth_config enrichment: parse token claims -> account/team identity metadata."""
    from src.core.provider_oauth_utils import parse_codex_id_token

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

    logger.debug(
        "Codex enrich_auth_config: id_token_present={} access_token_present={} token_keys={}",
        bool(token_response.get("id_token") or token_response.get("idToken")),
        bool(token_response.get("access_token") or token_response.get("accessToken")),
        list(token_response.keys()),
    )

    token_candidates = [
        token_response.get("id_token"),
        token_response.get("idToken"),
        token_response.get("access_token"),
        token_response.get("accessToken"),
    ]
    for token_payload in token_candidates:
        codex_info = parse_codex_id_token(token_payload)
        if not codex_info:
            continue
        logger.debug("Codex parsed token fields: {}", list(codex_info.keys()))
        for key, value in codex_info.items():
            if not auth_config.get(key):
                auth_config[key] = value

    return auth_config


# ---------------------------------------------------------------------------
# Unified Registration
# ---------------------------------------------------------------------------


def register_all() -> None:
    """一次性注册 Codex 的所有 hooks 到各通用 registry。"""
    from src.core.api_format.capabilities import register_provider_default_body_rules
    from src.core.provider_oauth_utils import register_auth_enricher
    from src.services.model.upstream_fetcher import UpstreamModelsFetcherRegistry
    from src.services.provider.transport import register_transport_hook

    # Transport
    register_transport_hook("codex", "openai:cli", build_codex_url)
    register_transport_hook("codex", "openai:compact", build_codex_url)

    # Auth
    register_auth_enricher("codex", enrich_codex)

    # Provider Format Capability：默认 body_rules
    from src.core.api_format.metadata import CODEX_DEFAULT_BODY_RULES

    register_provider_default_body_rules("codex", "openai:cli", CODEX_DEFAULT_BODY_RULES)

    # Export: Codex uses the default export builder (strip null + temp fields)
    # No need to register a custom one — the default in export.py suffices.

    # Model Fetcher
    UpstreamModelsFetcherRegistry.register(
        provider_types=["codex"],
        fetcher=fetch_models_codex,
    )
