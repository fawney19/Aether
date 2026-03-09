"""Provider request/response envelope hooks.

Some upstreams expose an API that is *almost* compatible with an existing
endpoint signature (family:kind), but wrap the wire format in an extra envelope
or require small transport-level behaviors.

This module provides a small hook mechanism so handlers can stay generic while
provider-specific envelopes live in their own service modules.
"""

from __future__ import annotations

import threading
from typing import Any, Protocol


class ProviderEnvelope(Protocol):
    """Provider-specific envelope transformation and side-effects."""

    name: str

    def extra_headers(self) -> dict[str, str] | None:
        """Extra upstream request headers to merge into the RequestBuilder."""

    def wrap_request(
        self,
        request_body: dict[str, Any],
        *,
        model: str,
        url_model: str | None,
        decrypted_auth_config: dict[str, Any] | None,
    ) -> tuple[dict[str, Any], str | None]:
        """Wrap request payload and optionally override url_model (e.g. move model into body)."""

    def unwrap_response(self, data: Any) -> Any:
        """Unwrap upstream response payload (streaming chunk or full JSON)."""

    def postprocess_unwrapped_response(self, *, model: str, data: Any) -> None:
        """Best-effort post processing after unwrap (e.g. cache signatures)."""

    def capture_selected_base_url(self) -> str | None:
        """Capture the base_url selected by transport layer (if any)."""

    def on_http_status(self, *, base_url: str | None, status_code: int) -> None:
        """Called after receiving upstream HTTP status code."""

    def on_connection_error(self, *, base_url: str | None, exc: Exception) -> None:
        """Called when a connection-type exception happens."""

    def force_stream_rewrite(self) -> bool:
        """Whether streaming should always go through the rewrite/conversion path."""

    # ------------------------------------------------------------------
    # Optional lifecycle hooks (checked via hasattr before calling)
    # ------------------------------------------------------------------

    def prepare_context(
        self,
        *,
        provider_config: Any,
        key_id: str,
        is_stream: bool,
        provider_id: str | None = None,
        key: Any = None,
    ) -> str | None:
        """Pre-wrap hook: build provider-specific request context.

        Called before wrap_request(). Returns tls_profile (or None).
        Implementations typically set contextvars that wrap_request()
        and extra_headers() will read.
        """

    async def post_wrap_request(self, request_body: dict[str, Any]) -> None:
        """Post-wrap hook: async processing after wrap_request().

        Called after wrap_request() completes. Use for async operations
        like distributed session control that cannot run in sync wrap_request().
        """

    def excluded_beta_tokens(self) -> frozenset[str]:
        """Beta tokens to strip from the merged anthropic-beta header.

        Called by the request builder after merging envelope extra_headers
        with client original headers. Return an empty frozenset to keep all.
        """


# ---------------------------------------------------------------------------
# Envelope Registry
# ---------------------------------------------------------------------------
# key: (provider_type, endpoint_sig) — endpoint_sig="" 表示通配
_envelope_registry: dict[tuple[str, str], ProviderEnvelope] = {}


def register_envelope(
    provider_type: str,
    endpoint_sig: str,
    envelope: ProviderEnvelope,
) -> None:
    """注册 provider 特有的 envelope。

    Args:
        provider_type: 如 "antigravity"
        endpoint_sig: 如 "gemini:cli"，传 "" 表示该 provider 的所有 endpoint
        envelope: 实现了 ProviderEnvelope 协议的实例
    """
    from src.core.provider_types import normalize_provider_type

    pt = normalize_provider_type(provider_type)
    sig = str(endpoint_sig or "").strip().lower()
    _envelope_registry[(pt, sig)] = envelope


def get_provider_envelope(
    *,
    provider_type: str | None,
    endpoint_sig: str | None,
) -> ProviderEnvelope | None:
    """Return envelope hooks for the given provider_type + endpoint signature."""
    ensure_providers_bootstrapped()

    from src.core.provider_types import normalize_provider_type

    pt = normalize_provider_type(provider_type)
    sig = str(endpoint_sig or "").strip().lower()

    if not pt:
        return None

    # 精确匹配优先，再尝试通配
    return _envelope_registry.get((pt, sig)) or _envelope_registry.get((pt, ""))


# ---------------------------------------------------------------------------
# Provider Bootstrap（惰性 + 幂等）
# ---------------------------------------------------------------------------
# 所有 registry 共享同一个 bootstrap，首次访问任何 registry 时自动触发。
# 不再依赖模块 import 顺序。
_bootstrapped = False
_bootstrap_lock = threading.Lock()


def ensure_providers_bootstrapped() -> None:
    """确保所有 provider plugin 已注册（幂等，只执行一次）。"""
    global _bootstrapped  # noqa: PLW0603
    if _bootstrapped:
        return
    with _bootstrap_lock:
        if _bootstrapped:
            return
        _bootstrapped = True

        from src.services.provider.adapters.antigravity.plugin import (
            register_all as _reg_antigravity,
        )
        from src.services.provider.adapters.claude_code.plugin import (
            register_all as _reg_claude_code,
        )
        from src.services.provider.adapters.codex.plugin import register_all as _reg_codex
        from src.services.provider.adapters.gemini_cli.plugin import (
            register_all as _reg_gemini_cli,
        )
        from src.services.provider.adapters.kiro.plugin import register_all as _reg_kiro
        from src.services.provider.adapters.vertex_ai.plugin import register_all as _reg_vertex_ai

        _reg_antigravity()
        _reg_claude_code()
        _reg_codex()
        _reg_gemini_cli()
        _reg_kiro()
        _reg_vertex_ai()


__all__ = [
    "ProviderEnvelope",
    "ensure_providers_bootstrapped",
    "get_provider_envelope",
    "register_envelope",
]
