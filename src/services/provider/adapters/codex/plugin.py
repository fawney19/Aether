"""Codex provider plugin — 统一注册入口。

将 Codex 对各通用 registry 的注册集中在一个文件中：
- Envelope (OAuth headers)
- Transport Hook (URL 构建)
- Auth Enricher (OAuth enrichment)
- Behavior Variants (格式变体)
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

    from src.services.provider.adapters.codex.context import get_codex_request_context

    ctx = get_codex_request_context()
    is_compact = ctx.is_compact if ctx else False

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
    """Codex auth_config enrichment: parse id_token -> email/account_id/plan_type/user_id."""
    from src.core.provider_oauth_utils import parse_codex_id_token

    id_token = token_response.get("id_token")
    logger.debug(
        "Codex enrich_auth_config: id_token_present={} token_keys={}",
        bool(id_token),
        list(token_response.keys()),
    )
    # parse_codex_id_token 仅返回非空有效字段，直接 update 即可
    codex_info = parse_codex_id_token(id_token)
    if codex_info:
        logger.debug("Codex parsed id_token fields: {}", list(codex_info.keys()))
        auth_config.update(codex_info)
    return auth_config


# ---------------------------------------------------------------------------
# Unified Registration
# ---------------------------------------------------------------------------


def register_all() -> None:
    """一次性注册 Codex 的所有 hooks 到各通用 registry。"""
    from src.core.provider_oauth_utils import register_auth_enricher
    from src.services.model.upstream_fetcher import UpstreamModelsFetcherRegistry
    from src.services.provider.adapters.codex.envelope import codex_oauth_envelope
    from src.services.provider.behavior import register_behavior_variant
    from src.services.provider.envelope import register_envelope
    from src.services.provider.transport import register_transport_hook

    # Envelope
    register_envelope("codex", "openai:cli", codex_oauth_envelope)
    register_envelope("codex", "", codex_oauth_envelope)

    # Transport
    register_transport_hook("codex", "openai:cli", build_codex_url)

    # Auth
    register_auth_enricher("codex", enrich_codex)

    # Behavior
    register_behavior_variant("codex", same_format=True, cross_format=True)

    # Export: Codex uses the default export builder (strip null + temp fields)
    # No need to register a custom one — the default in export.py suffices.

    # Model Fetcher
    UpstreamModelsFetcherRegistry.register(
        provider_types=["codex"],
        fetcher=fetch_models_codex,
    )
