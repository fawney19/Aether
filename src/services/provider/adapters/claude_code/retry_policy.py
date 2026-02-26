"""Claude Code 状态码与重试策略。"""

from __future__ import annotations

from typing import Final

_RETRYABLE_STATUS_CODES: Final[frozenset[int]] = frozenset(
    {
        408,  # request timeout
        409,  # conflict / transient race
        425,  # too early
        429,  # rate limit
        500,  # upstream error
        502,
        503,
        504,
        529,  # Anthropic overload
    }
)

_AUTH_ERROR_PATTERNS: Final[tuple[str, ...]] = (
    "oauth authentication is currently not allowed for this organization",
    "this organization has been disabled",
    "organization has been disabled",
    "invalid x-api-key",
    "invalid api key",
    "authentication",
    "unauthorized",
    "forbidden",
)

_CLIENT_ERROR_PATTERNS: Final[tuple[str, ...]] = (
    "invalid_request_error",
    "validation_error",
    "content_length_exceeded",
    "context_length_exceeded",
    "messages",
    "max_tokens",
    "tool_choice",
    "stop_sequences",
    "must be",
    "required",
    "unsupported",
    "not supported",
)

_RETRYABLE_403_PATTERNS: Final[tuple[str, ...]] = (
    "rate limit",
    "overloaded",
    "temporarily unavailable",
    "try again",
)


def is_claude_code_provider(provider_type: str | None) -> bool:
    """是否为 claude_code provider。"""
    return str(provider_type or "").strip().lower() == "claude_code"


def _normalize_error_text(error_text: str | None) -> str:
    return str(error_text or "").strip().lower()


def is_claude_code_auth_error(status_code: int, error_text: str | None) -> bool:
    """判断是否是 Claude Code 认证/账号级错误。"""
    if status_code == 401:
        return True
    if status_code != 403:
        return False
    text = _normalize_error_text(error_text)
    if not text:
        # 403 在 Claude Code 场景更偏向认证/账号问题，默认按 auth 处理
        return True
    return any(pattern in text for pattern in _AUTH_ERROR_PATTERNS)


def is_claude_code_request_error(status_code: int, error_text: str | None) -> bool:
    """判断是否是明确的客户端请求错误（不应在同候选重试）。"""
    if status_code not in {400, 404, 422}:
        return False
    text = _normalize_error_text(error_text)
    if not text:
        # 400/404/422 没有明确信息时，保守按请求错误处理，避免无意义重试
        return True
    return any(pattern in text for pattern in _CLIENT_ERROR_PATTERNS)


def should_retry_same_candidate(status_code: int, error_text: str | None) -> bool:
    """是否应该在同一候选上继续重试。"""
    if status_code in _RETRYABLE_STATUS_CODES:
        return True
    if status_code == 403:
        text = _normalize_error_text(error_text)
        return any(pattern in text for pattern in _RETRYABLE_403_PATTERNS)
    return False


def is_retryable_status(status_code: int) -> bool:
    """仅按状态码判断可重试性（不含文本模式）。"""
    return status_code in _RETRYABLE_STATUS_CODES


__all__ = [
    "is_claude_code_auth_error",
    "is_claude_code_provider",
    "is_claude_code_request_error",
    "is_retryable_status",
    "should_retry_same_candidate",
]
