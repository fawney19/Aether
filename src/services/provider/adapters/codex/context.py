"""Codex request-scoped context.

Codex only needs a small amount of per-request runtime state that does not belong in
the outbound payload itself. Today that state is the compact-mode flag used by the
transport and upstream stream-policy layers.
"""

from __future__ import annotations

import contextvars
from dataclasses import dataclass


@dataclass(frozen=True, slots=True)
class CodexRequestContext:
    """Per-request context for the Codex adapter."""

    is_compact: bool = False


_codex_request_context: contextvars.ContextVar[CodexRequestContext | None] = contextvars.ContextVar(
    "codex_request_context",
    default=None,
)


def set_codex_request_context(ctx: CodexRequestContext | None) -> None:
    _codex_request_context.set(ctx)


def get_codex_request_context() -> CodexRequestContext | None:
    return _codex_request_context.get()


def is_codex_compact_request(*, endpoint_sig: str | None = None) -> bool:
    """Return whether the current Codex request should use compact semantics.

    Modern configurations use a dedicated ``openai:compact`` endpoint. Older ones may
    still route compact traffic through ``openai:cli`` and rely on request-scoped
    context instead.
    """
    normalized_sig = str(endpoint_sig or "").strip().lower()
    if normalized_sig == "openai:compact":
        return True

    ctx = get_codex_request_context()
    return bool(ctx and ctx.is_compact)


__all__ = [
    "CodexRequestContext",
    "get_codex_request_context",
    "is_codex_compact_request",
    "set_codex_request_context",
]
