"""Per-request context shared across layers.

We use `contextvars` so the transport layer (URL builder) can pass small bits of
state to the handler layer without changing existing return types.

This is intentionally minimal; only add fields that are safe and cheap to carry
per request.
"""

from __future__ import annotations

import contextvars
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from src.services.provider.fingerprint import FingerprintProfile

_selected_base_url: contextvars.ContextVar[str | None] = contextvars.ContextVar(
    "provider_selected_base_url",
    default=None,
)
_current_fingerprint: contextvars.ContextVar[FingerprintProfile | None] = contextvars.ContextVar(
    "provider_current_fingerprint",
    default=None,
)


def set_selected_base_url(url: str | None) -> None:
    _selected_base_url.set(url)


def get_selected_base_url() -> str | None:
    return _selected_base_url.get()


def set_current_fingerprint(fp: FingerprintProfile | None) -> None:
    _current_fingerprint.set(fp)


def get_current_fingerprint() -> FingerprintProfile | None:
    return _current_fingerprint.get()


__all__ = [
    "get_current_fingerprint",
    "get_selected_base_url",
    "set_current_fingerprint",
    "set_selected_base_url",
]
