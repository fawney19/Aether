"""Codex provider request patching helpers.

Codex requests are mostly passthrough:
- Do not mutate client payload fields unless Codex-specific compatibility requires it.
- Strip internal sentinel fields that must never reach upstream.
"""

from __future__ import annotations

from typing import Any

from src.core.provider_types import ProviderType


def patch_openai_cli_request_for_codex(
    request_body: dict[str, Any],
) -> dict[str, Any]:
    """
    Patch an OpenAI CLI (Responses API style) request body for Codex gateways.

    This function never mutates the input object.
    """
    out: dict[str, Any] = dict(request_body)
    # Internal routing marker; never send upstream.
    out.pop("_aether_compact", None)
    return out


def maybe_patch_request_for_codex(
    *,
    provider_type: str | None,
    provider_api_format: str | None,
    request_body: Any,
) -> Any:
    """
    Conditionally patch request body for Codex gateways.

    No-op for:
    - Non-Codex providers
    - Non OpenAI CLI / Responses-style endpoints
    - Non-dict request bodies
    """
    if (provider_type or "").lower() != ProviderType.CODEX:
        return request_body
    if (provider_api_format or "").lower() not in {"openai:cli", "openai:compact"}:
        return request_body
    if not isinstance(request_body, dict):
        return request_body
    return patch_openai_cli_request_for_codex(request_body)


__all__ = [
    "maybe_patch_request_for_codex",
    "patch_openai_cli_request_for_codex",
]
