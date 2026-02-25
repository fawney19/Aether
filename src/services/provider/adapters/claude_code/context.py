from __future__ import annotations

import contextvars
from dataclasses import dataclass, field


@dataclass(frozen=True, slots=True)
class ClaudeCodeRequestContext:
    """Claude Code 请求级上下文。"""

    auth_method: str = "bearer"  # "bearer" | "api_key" | "oauth"
    email: str | None = None
    is_stream: bool = False
    tool_prefix_enabled: bool = False
    body_betas: tuple[str, ...] = field(default_factory=tuple)


_claude_code_request_context: contextvars.ContextVar[ClaudeCodeRequestContext | None] = (
    contextvars.ContextVar(
        "claude_code_request_context",
        default=None,
    )
)


def set_claude_code_request_context(ctx: ClaudeCodeRequestContext | None) -> None:
    _claude_code_request_context.set(ctx)


def get_claude_code_request_context() -> ClaudeCodeRequestContext | None:
    return _claude_code_request_context.get()


__all__ = [
    "ClaudeCodeRequestContext",
    "get_claude_code_request_context",
    "set_claude_code_request_context",
]
