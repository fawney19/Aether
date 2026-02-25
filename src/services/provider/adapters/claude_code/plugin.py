"""Claude Code provider plugin — unified registration entry."""

from __future__ import annotations

from typing import Any
from urllib.parse import urlencode

from src.services.provider.preset_models import create_preset_models_fetcher

fetch_models_claude_code = create_preset_models_fetcher("claude_code")


def build_claude_code_url(
    endpoint: Any,
    *,
    is_stream: bool,
    effective_query_params: dict[str, Any],
) -> str:
    """构建 Claude Code API URL。"""
    _ = is_stream
    from src.services.provider.request_context import set_selected_base_url

    base = str(endpoint.base_url or "https://api.anthropic.com").rstrip("/")
    if base.endswith("/v1"):
        base = base[:-3]

    custom_path = getattr(endpoint, "custom_path", None)
    path = custom_path if custom_path else "/v1/messages"
    url = f"{base}{path}"

    set_selected_base_url(base)

    if effective_query_params:
        query_string = urlencode(effective_query_params, doseq=True)
        if query_string:
            url = f"{url}?{query_string}"

    return url


def claude_code_export_builder(
    auth_config: dict[str, Any],
    upstream_metadata: dict[str, Any] | None,
) -> dict[str, Any]:
    """Claude Code 导出构建器。"""
    _ = upstream_metadata
    skip_keys = {
        "access_token",
        "expires_at",
        "updated_at",
        "token_type",
        "scope",
    }
    return {
        k: v for k, v in auth_config.items() if k not in skip_keys and v is not None and v != ""
    }


def register_all() -> None:
    """一次性注册 Claude Code hooks。"""
    from src.services.model.upstream_fetcher import UpstreamModelsFetcherRegistry
    from src.services.provider.adapters.claude_code.envelope import claude_code_envelope
    from src.services.provider.behavior import register_behavior_variant
    from src.services.provider.envelope import register_envelope
    from src.services.provider.export import register_export_builder
    from src.services.provider.transport import register_transport_hook

    register_envelope("claude_code", "claude:cli", claude_code_envelope)
    register_envelope("claude_code", "", claude_code_envelope)

    register_transport_hook("claude_code", "claude:cli", build_claude_code_url)
    register_export_builder("claude_code", claude_code_export_builder)
    register_behavior_variant("claude_code", cross_format=True)

    UpstreamModelsFetcherRegistry.register(
        provider_types=["claude_code"],
        fetcher=fetch_models_claude_code,
    )


__all__ = [
    "build_claude_code_url",
    "claude_code_export_builder",
    "fetch_models_claude_code",
    "register_all",
]
