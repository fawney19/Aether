from __future__ import annotations

from src.services.provider.adapters.claude_code.retry_policy import (
    is_claude_code_auth_error,
    is_claude_code_provider,
    is_claude_code_request_error,
    is_retryable_status,
    should_retry_same_candidate,
)


def test_is_claude_code_provider() -> None:
    assert is_claude_code_provider("claude_code") is True
    assert is_claude_code_provider("CLAUDE_CODE") is True
    assert is_claude_code_provider("antigravity") is False


def test_retryable_status_and_same_candidate_policy() -> None:
    assert is_retryable_status(529) is True
    assert should_retry_same_candidate(529, None) is True
    assert should_retry_same_candidate(429, "rate limit") is True
    assert should_retry_same_candidate(403, "temporarily unavailable") is True
    assert should_retry_same_candidate(403, "oauth authentication is currently not allowed") is False
    assert should_retry_same_candidate(400, "invalid request") is False


def test_auth_and_request_error_detection() -> None:
    assert is_claude_code_auth_error(401, None) is True
    assert is_claude_code_auth_error(
        403, "OAuth authentication is currently not allowed for this organization"
    )
    assert is_claude_code_request_error(400, "invalid_request_error: messages is required")
    assert is_claude_code_request_error(422, "validation_error")
    assert is_claude_code_request_error(503, "service unavailable") is False
