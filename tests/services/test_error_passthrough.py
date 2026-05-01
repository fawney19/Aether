from types import SimpleNamespace

import httpx
import pytest

from src.core.exceptions import ProviderNotAvailableException
from src.services.provider.error_passthrough import (
    extract_upstream_error_message,
    match_error_passthrough_rule,
)
from src.services.request.result import RequestResult
from src.services.task.execute.failure import TaskFailureOperationsService


def _make_candidate(*, provider_config: dict | None = None) -> SimpleNamespace:
    return SimpleNamespace(
        provider=SimpleNamespace(id="provider-1", name="Provider 1", config=provider_config or {}),
        endpoint=SimpleNamespace(id="endpoint-1"),
        key=SimpleNamespace(id="key-1"),
    )


def test_match_error_passthrough_rule_honors_status_code_and_regex() -> None:
    provider_config = {
        "error_passthrough_rules": {
            "patterns": [
                {
                    "pattern": "content_policy_violation",
                    "status_codes": [400],
                }
            ]
        }
    }

    match = match_error_passthrough_rule(
        provider_config,
        response_text='{"error":{"code":"content_policy_violation","message":"Blocked by policy"}}',
        status_code=400,
    )

    assert match is not None
    assert match.pattern == "content_policy_violation"
    assert match.message == "content_policy_violation: Blocked by policy"


def test_extract_upstream_error_message_redacts_sensitive_tokens() -> None:
    message = extract_upstream_error_message("authorization: Bearer sk-secret-value")

    assert message == "authorization: [REDACTED]"


def test_raise_all_failed_exception_uses_passthrough_message_when_rule_matches() -> None:
    candidate = _make_candidate(
        provider_config={
            "error_passthrough_rules": {
                "patterns": [
                    {
                        "pattern": "content_policy_violation",
                        "status_codes": [400],
                    }
                ]
            }
        }
    )
    request = httpx.Request("POST", "https://example.com/v1/messages")
    response = httpx.Response(
        400,
        request=request,
        text='{"error":{"code":"content_policy_violation","message":"Blocked by policy"}}',
    )
    error = httpx.HTTPStatusError("HTTP 400", request=request, response=response)
    error.upstream_response = response.text  # type: ignore[attr-defined]

    with pytest.raises(ProviderNotAvailableException) as exc_info:
        TaskFailureOperationsService.raise_all_failed_exception(
            request_id="req-1",
            max_attempts=1,
            last_candidate=candidate,
            model_name="claude-3-7-sonnet",
            api_format="claude:chat",
            last_error=error,
        )

    exc = exc_info.value
    assert exc.message == "content_policy_violation: Blocked by policy"
    assert exc.upstream_status == 400
    assert exc.client_error_passthrough is True
    assert exc.matched_passthrough_pattern == "content_policy_violation"


def test_request_result_uses_upstream_status_when_passthrough_enabled() -> None:
    result = RequestResult.from_exception(
        exception=ProviderNotAvailableException(
            "content_policy_violation: Blocked by policy",
            upstream_status=400,
            upstream_response='{"error":{"code":"content_policy_violation","message":"Blocked by policy"}}',
            client_error_passthrough=True,
            matched_passthrough_pattern="content_policy_violation",
        ),
        api_format="claude:chat",
        model="claude-3-7-sonnet",
        response_time_ms=123,
        is_stream=False,
    )

    assert result.status_code == 400
    assert result.error_type == "upstream_client_error"
    assert result.error_message == "content_policy_violation: Blocked by policy"
