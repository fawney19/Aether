"""Provider-specific upstream request header hooks."""

from __future__ import annotations

from collections.abc import Mapping
from typing import Any, Callable

from src.core.provider_types import normalize_provider_type
from src.services.provider.envelope import ensure_providers_bootstrapped

UpstreamHeadersHookFn = Callable[..., dict[str, str]]

_hooks: dict[tuple[str, str], UpstreamHeadersHookFn] = {}


def register_upstream_headers_hook(
    provider_type: str,
    endpoint_sig: str,
    hook: UpstreamHeadersHookFn,
) -> None:
    """Register a provider-specific extra upstream headers builder."""
    pt = normalize_provider_type(provider_type)
    sig = str(endpoint_sig or "").strip().lower()
    if not pt or not sig:
        return
    _hooks[(pt, sig)] = hook


def build_upstream_extra_headers(
    *,
    provider_type: str | None,
    endpoint_sig: str | None,
    request_body: Any,
    original_headers: Mapping[str, Any] | None,
    decrypted_auth_config: dict[str, Any] | None,
) -> dict[str, str]:
    """Build provider-specific extra upstream headers for the current request."""
    pt = normalize_provider_type(provider_type)
    sig = str(endpoint_sig or "").strip().lower()
    if not pt or not sig:
        return {}

    ensure_providers_bootstrapped(provider_types=[pt])
    hook = _hooks.get((pt, sig))
    if hook is None:
        return {}

    return hook(
        request_body,
        original_headers,
        decrypted_auth_config=decrypted_auth_config,
    )


__all__ = [
    "UpstreamHeadersHookFn",
    "build_upstream_extra_headers",
    "register_upstream_headers_hook",
]
