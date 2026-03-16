"""Provider request/response envelope hooks.

Some upstreams expose an API that is *almost* compatible with an existing
endpoint signature (family:kind), but wrap the wire format in an extra envelope
or require small transport-level behaviors.

This module provides a small hook mechanism so handlers can stay generic while
provider-specific envelopes live in their own service modules.
"""

from __future__ import annotations

import importlib
import threading
from collections.abc import Iterable
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
        user_api_key_id: str | None = None,
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
    ensure_providers_bootstrapped(provider_types=[provider_type] if provider_type else None)

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
_bootstrap_lock = threading.Lock()
_bootstrap_condition = threading.Condition(_bootstrap_lock)
_bootstrap_in_progress = False
_bootstrapped_provider_types: set[str] = set()
_auto_detected_provider_types: frozenset[str] | None = None

_PROVIDER_PLUGIN_MODULES: dict[str, str] = {
    "antigravity": "src.services.provider.adapters.antigravity.plugin",
    "claude_code": "src.services.provider.adapters.claude_code.plugin",
    "codex": "src.services.provider.adapters.codex.plugin",
    "gemini_cli": "src.services.provider.adapters.gemini_cli.plugin",
    "kiro": "src.services.provider.adapters.kiro.plugin",
    "vertex_ai": "src.services.provider.adapters.vertex_ai.plugin",
}


def _normalize_bootstrap_targets(provider_types: Iterable[str] | None) -> set[str]:
    from src.core.provider_types import normalize_provider_type

    if provider_types is None:
        return set()
    if isinstance(provider_types, str):
        provider_types = [provider_types]

    targets: set[str] = set()
    for raw in provider_types:
        pt = normalize_provider_type(raw)
        if pt in _PROVIDER_PLUGIN_MODULES:
            targets.add(pt)
    return targets


def _discover_active_provider_types() -> set[str]:
    """从数据库读取活跃 provider_type，用于按需 bootstrap。"""
    from src.core.provider_types import normalize_provider_type
    from src.database.database import create_session
    from src.models.database import Provider

    db = create_session()
    try:
        rows = (
            db.query(Provider.provider_type).filter(Provider.is_active.is_(True)).distinct().all()
        )
    finally:
        db.close()

    discovered: set[str] = set()
    for (raw_provider_type,) in rows:
        pt = normalize_provider_type(raw_provider_type)
        if pt in _PROVIDER_PLUGIN_MODULES:
            discovered.add(pt)
    return discovered


def _bootstrap_provider_type(provider_type: str) -> None:
    module_path = _PROVIDER_PLUGIN_MODULES[provider_type]
    module = importlib.import_module(module_path)
    register_all = getattr(module, "register_all", None)
    if callable(register_all):
        register_all()


def ensure_providers_bootstrapped(provider_types: Iterable[str] | None = None) -> None:
    """确保 provider plugins 已注册（幂等，支持按 provider_type 精准注册）。"""
    global _auto_detected_provider_types, _bootstrap_in_progress  # noqa: PLW0603

    targets = _normalize_bootstrap_targets(provider_types)

    # DB 查询在锁外执行，避免慢查询阻塞其他线程的 bootstrap 操作。
    need_discover = not targets and _auto_detected_provider_types is None
    if need_discover:
        try:
            detected = _discover_active_provider_types()
        except Exception:
            detected = set()
    else:
        detected = set()

    with _bootstrap_condition:
        if not targets:
            if _auto_detected_provider_types is None:
                # 回退策略：DB 不可用/无记录时，保持原有全量 bootstrap 语义。
                _auto_detected_provider_types = frozenset(
                    detected if detected else _PROVIDER_PLUGIN_MODULES.keys()
                )
            targets = set(_auto_detected_provider_types)

        while _bootstrap_in_progress:
            _bootstrap_condition.wait()

        missing = targets - _bootstrapped_provider_types
        if not missing:
            return
        _bootstrap_in_progress = True

    bootstrapped_now: set[str] = set()
    try:
        for pt in sorted(missing):
            _bootstrap_provider_type(pt)
            bootstrapped_now.add(pt)
    finally:
        with _bootstrap_condition:
            _bootstrapped_provider_types.update(bootstrapped_now)
            _bootstrap_in_progress = False
            _bootstrap_condition.notify_all()


__all__ = [
    "ProviderEnvelope",
    "ensure_providers_bootstrapped",
    "get_provider_envelope",
    "register_envelope",
]
