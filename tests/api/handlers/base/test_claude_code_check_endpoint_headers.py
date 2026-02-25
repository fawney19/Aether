from __future__ import annotations

from typing import Any
from unittest.mock import patch

import pytest

from src.api.handlers.claude_cli.adapter import ClaudeCliAdapter


def _make_request_data() -> dict[str, Any]:
    return {
        "model": "claude-sonnet-4-6",
        "messages": [{"role": "user", "content": "hi"}],
        "max_tokens": 8,
    }


@pytest.mark.asyncio
async def test_check_endpoint_claude_code_api_key_mode_uses_x_api_key() -> None:
    captured: dict[str, Any] = {}

    async def _fake_run_endpoint_check(**kwargs: Any) -> dict[str, Any]:
        captured.update(kwargs)
        return {"status_code": 200}

    with patch(
        "src.api.handlers.base.endpoint_checker.run_endpoint_check",
        side_effect=_fake_run_endpoint_check,
    ):
        await ClaudeCliAdapter.check_endpoint(
            None,  # type: ignore[arg-type]
            "https://api.anthropic.com",
            "sk-ant-test",
            _make_request_data(),
            auth_type="api_key",
            provider_type="claude_code",
        )

    headers = {k.lower(): v for k, v in captured["headers"].items()}
    assert headers.get("x-api-key") == "sk-ant-test"
    assert "authorization" not in headers
    assert headers.get("anthropic-version") == "2023-06-01"


@pytest.mark.asyncio
async def test_check_endpoint_claude_code_oauth_mode_keeps_authorization() -> None:
    captured: dict[str, Any] = {}

    async def _fake_run_endpoint_check(**kwargs: Any) -> dict[str, Any]:
        captured.update(kwargs)
        return {"status_code": 200}

    with patch(
        "src.api.handlers.base.endpoint_checker.run_endpoint_check",
        side_effect=_fake_run_endpoint_check,
    ):
        await ClaudeCliAdapter.check_endpoint(
            None,  # type: ignore[arg-type]
            "https://api.anthropic.com",
            "oauth-access-token",
            _make_request_data(),
            auth_type="oauth",
            provider_type="claude_code",
        )

    headers = {k.lower(): v for k, v in captured["headers"].items()}
    assert headers.get("authorization") == "Bearer oauth-access-token"
    assert headers.get("anthropic-version") == "2023-06-01"
