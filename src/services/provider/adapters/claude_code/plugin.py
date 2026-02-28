"""Claude Code provider plugin."""

from __future__ import annotations

from typing import Any
from urllib.parse import urlencode

from src.services.provider.adapters.claude_code.constants import CLAUDE_MESSAGES_PATH
from src.services.provider.preset_models import create_preset_models_fetcher

fetch_models_claude_code = create_preset_models_fetcher("claude_code")


def build_claude_code_url(
    endpoint: Any,
    *,
    is_stream: bool,
    effective_query_params: dict[str, Any],
    **_kwargs: Any,
) -> str:
    """Build Claude Code upstream URL and avoid duplicate /v1/messages suffix."""
    _ = is_stream

    base = str(getattr(endpoint, "base_url", "") or "").rstrip("/")
    if base.endswith(CLAUDE_MESSAGES_PATH) or base.endswith("/messages"):
        url = base
    elif base.endswith("/v1"):
        url = f"{base}/messages"
    else:
        url = f"{base}{CLAUDE_MESSAGES_PATH}"

    if effective_query_params:
        query_string = urlencode(effective_query_params, doseq=True)
        if query_string:
            url = f"{url}?{query_string}"

    return url


def register_all() -> None:
    """Register Claude Code hooks into shared registries."""
    from src.services.model.upstream_fetcher import UpstreamModelsFetcherRegistry
    from src.services.provider.adapters.claude_code.envelope import claude_code_envelope
    from src.services.provider.envelope import register_envelope
    from src.services.provider.transport import register_transport_hook

    register_envelope("claude_code", "claude:cli", claude_code_envelope)
    register_envelope("claude_code", "", claude_code_envelope)

    register_transport_hook("claude_code", "claude:cli", build_claude_code_url)

    UpstreamModelsFetcherRegistry.register(
        provider_types=["claude_code"],
        fetcher=fetch_models_claude_code,
    )

    from src.services.provider.adapters.claude_code.pool_hook import claude_code_pool_hook
    from src.services.provider.pool.hooks import register_pool_hook

    register_pool_hook("claude_code", claude_code_pool_hook)


__all__ = ["build_claude_code_url", "fetch_models_claude_code", "register_all"]
