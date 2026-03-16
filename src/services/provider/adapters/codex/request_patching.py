"""Codex provider request patching helpers.

Codex requests are mostly passthrough:
- Do not mutate client payload fields unless Codex-specific compatibility requires it.
- Strip internal sentinel fields that must never reach upstream.
- When the caller's user API key is known and the request did not provide one,
  synthesize a stable ``prompt_cache_key`` so prompt caching can be reused.
"""

from __future__ import annotations

import uuid
from typing import Any

from src.core.provider_types import ProviderType


def build_stable_codex_prompt_cache_key(user_api_key_id: str | None) -> str | None:
    """Build a deterministic Codex prompt cache key from the caller's user API key id."""
    normalized = str(user_api_key_id or "").strip()
    if not normalized:
        return None
    return str(uuid.uuid5(uuid.NAMESPACE_OID, f"aether:codex:prompt-cache:user:{normalized}"))


def patch_openai_cli_request_for_codex(
    request_body: dict[str, Any],
    *,
    user_api_key_id: str | None = None,
) -> dict[str, Any]:
    """
    Patch an OpenAI CLI (Responses API style) request body for Codex gateways.

    This function never mutates the input object.
    """
    out: dict[str, Any] = dict(request_body)
    # Internal routing marker; never send upstream.
    out.pop("_aether_compact", None)
    prompt_cache_key = str(out.get("prompt_cache_key") or "").strip()
    if not prompt_cache_key:
        stable_key = build_stable_codex_prompt_cache_key(user_api_key_id)
        if stable_key:
            out["prompt_cache_key"] = stable_key
    return out


def maybe_patch_request_for_codex(
    *,
    provider_type: str | None,
    provider_api_format: str | None,
    request_body: Any,
    user_api_key_id: str | None = None,
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
    return patch_openai_cli_request_for_codex(request_body, user_api_key_id=user_api_key_id)


__all__ = [
    "build_stable_codex_prompt_cache_key",
    "maybe_patch_request_for_codex",
    "patch_openai_cli_request_for_codex",
]
