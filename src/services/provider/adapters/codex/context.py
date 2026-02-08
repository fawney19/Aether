"""Codex request context using contextvars.

Similar to Kiro's context pattern, this bridges data from `CodexOAuthEnvelope.wrap_request()`
(which receives the decrypted auth_config) to `extra_headers()` which is parameterless.
"""

from __future__ import annotations

import contextvars
from dataclasses import dataclass


@dataclass(frozen=True, slots=True)
class CodexRequestContext:
    """Per-request context for the Codex adapter.

    This bridges data from `CodexOAuthEnvelope.wrap_request()` (which receives the
    decrypted auth_config) to other layers that only expose parameterless hooks
    (extra_headers).
    """

    account_id: str | None = None
    user_id: str | None = None


_codex_request_context: contextvars.ContextVar[CodexRequestContext | None] = contextvars.ContextVar(
    "codex_request_context",
    default=None,
)


def set_codex_request_context(ctx: CodexRequestContext | None) -> None:
    _codex_request_context.set(ctx)


def get_codex_request_context() -> CodexRequestContext | None:
    return _codex_request_context.get()


__all__ = [
    "CodexRequestContext",
    "get_codex_request_context",
    "set_codex_request_context",
]
