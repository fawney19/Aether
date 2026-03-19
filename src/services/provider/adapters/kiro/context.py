from __future__ import annotations

import contextvars
from dataclasses import dataclass, replace


@dataclass(frozen=True, slots=True)
class KiroRequestContext:
    """Per-request context for the Kiro adapter.

    This bridges data from `KiroEnvelope.wrap_request()` (which receives the
    decrypted auth_config + original request body) to other layers that only
    expose parameterless hooks (extra_headers) or transport hooks.
    """

    region: str
    machine_id: str
    kiro_version: str | None = None
    system_version: str | None = None
    node_version: str | None = None
    thinking_enabled: bool = False
    last_http_status: int | None = None
    last_http_error_category: str | None = None
    last_connection_error_category: str | None = None
    last_connection_error_summary: str | None = None


_kiro_request_context: contextvars.ContextVar[KiroRequestContext | None] = contextvars.ContextVar(
    "kiro_request_context",
    default=None,
)


def set_kiro_request_context(ctx: KiroRequestContext | None) -> None:
    _kiro_request_context.set(ctx)


def get_kiro_request_context() -> KiroRequestContext | None:
    return _kiro_request_context.get()


def update_kiro_http_status(
    *,
    status_code: int,
    category: str,
) -> None:
    ctx = get_kiro_request_context()
    if ctx is None:
        return
    set_kiro_request_context(
        replace(
            ctx,
            last_http_status=int(status_code),
            last_http_error_category=str(category),
        )
    )


def update_kiro_connection_error(
    *,
    category: str,
    summary: str,
) -> None:
    ctx = get_kiro_request_context()
    if ctx is None:
        return
    set_kiro_request_context(
        replace(
            ctx,
            last_connection_error_category=str(category),
            last_connection_error_summary=str(summary),
        )
    )


__all__ = [
    "KiroRequestContext",
    "get_kiro_request_context",
    "set_kiro_request_context",
    "update_kiro_connection_error",
    "update_kiro_http_status",
]
