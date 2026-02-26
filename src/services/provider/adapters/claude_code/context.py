"""Claude Code request context using contextvars."""

from __future__ import annotations

import contextvars
from dataclasses import dataclass
from typing import Any

from src.core.logger import logger
from src.models.admin_requests import ClaudeCodeAdvancedConfig
from src.services.provider.adapters.claude_code.constants import TLS_PROFILE_CLAUDE_CODE


@dataclass(frozen=True, slots=True)
class ClaudeCodeRequestContext:
    is_stream: bool = False
    # 使用 Key 级别作用域，确保会话限制按 OAuth 账号隔离。
    scope_key: str | None = None
    key_id: str | None = None
    max_sessions: int | None = None
    session_idle_timeout_minutes: int = 5
    enable_tls_fingerprint: bool = False
    session_id_masking_enabled: bool = False


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


def build_claude_code_request_context(
    *,
    provider_config: Any,
    key_id: str | None,
    is_stream: bool,
) -> ClaudeCodeRequestContext:
    """根据 Provider.config 构建 Claude Code 请求上下文。"""
    normalized_key_id = str(key_id or "").strip() or None

    advanced_config: ClaudeCodeAdvancedConfig | None = None
    provider_config_dict = provider_config if isinstance(provider_config, dict) else {}
    raw_advanced = provider_config_dict.get("claude_code_advanced")

    if raw_advanced is not None:
        try:
            if isinstance(raw_advanced, ClaudeCodeAdvancedConfig):
                advanced_config = raw_advanced
            elif isinstance(raw_advanced, dict):
                advanced_config = ClaudeCodeAdvancedConfig.model_validate(raw_advanced)
            else:
                logger.warning(
                    "Claude Code advanced config 类型无效: {}，已忽略",
                    type(raw_advanced).__name__,
                )
        except Exception as exc:
            logger.warning("Claude Code advanced config 解析失败，已忽略: {}", str(exc))

    max_sessions = advanced_config.max_sessions if advanced_config else None
    idle_timeout_minutes = (
        advanced_config.session_idle_timeout_minutes
        if advanced_config and advanced_config.session_idle_timeout_minutes is not None
        else 5
    )
    enable_tls_fingerprint = bool(advanced_config.enable_tls_fingerprint) if advanced_config else False
    session_id_masking_enabled = (
        bool(advanced_config.session_id_masking_enabled) if advanced_config else False
    )

    return ClaudeCodeRequestContext(
        is_stream=bool(is_stream),
        scope_key=f"key:{normalized_key_id}" if normalized_key_id else None,
        key_id=normalized_key_id,
        max_sessions=max_sessions,
        session_idle_timeout_minutes=idle_timeout_minutes,
        enable_tls_fingerprint=enable_tls_fingerprint,
        session_id_masking_enabled=session_id_masking_enabled,
    )


def resolve_claude_code_tls_profile(
    ctx: ClaudeCodeRequestContext | None,
) -> str | None:
    if ctx and ctx.enable_tls_fingerprint:
        return TLS_PROFILE_CLAUDE_CODE
    return None


def build_and_set_claude_code_request_context(
    *,
    provider_config: Any,
    key_id: str | None,
    is_stream: bool,
) -> tuple[ClaudeCodeRequestContext, str | None]:
    """构建并写入 Claude Code 上下文，同时返回对应 TLS profile。"""
    ctx = build_claude_code_request_context(
        provider_config=provider_config,
        key_id=key_id,
        is_stream=is_stream,
    )
    set_claude_code_request_context(ctx)
    return ctx, resolve_claude_code_tls_profile(ctx)


__all__ = [
    "build_and_set_claude_code_request_context",
    "build_claude_code_request_context",
    "ClaudeCodeRequestContext",
    "get_claude_code_request_context",
    "resolve_claude_code_tls_profile",
    "set_claude_code_request_context",
]
