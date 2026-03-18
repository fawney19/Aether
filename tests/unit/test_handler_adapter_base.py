from typing import Any

import pytest

from src.api.handlers.claude.adapter import ClaudeChatAdapter
from src.api.handlers.gemini.adapter import GeminiChatAdapter


@pytest.mark.asyncio
async def test_check_endpoint_rejects_base_url_dict() -> None:
    with pytest.raises(TypeError, match="base_url must be a non-empty string"):
        await ClaudeChatAdapter.check_endpoint(
            client=None,  # type: ignore[arg-type]
            base_url={"base_url": "https://api.anthropic.com"},  # type: ignore[arg-type]
            api_key="test-key",
            request_data={
                "model": "claude-sonnet-4-5-20250929",
                "messages": [{"role": "user", "content": "hello"}],
                "max_tokens": 32,
                "stream": False,
            },
        )


def test_validate_test_base_url_trims_whitespace() -> None:
    assert ClaudeChatAdapter._validate_test_base_url("  https://api.anthropic.com/v1  ") == (
        "https://api.anthropic.com/v1"
    )


@pytest.mark.asyncio
async def test_claude_check_endpoint_passes_original_body_to_body_rules(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from src.api.handlers.base import endpoint_checker as endpoint_checker_module
    from src.api.handlers.base import request_builder as request_builder_module

    captured: dict[str, Any] = {}

    def fake_apply_body_rules(
        body: dict[str, Any],
        body_rules: list[dict[str, Any]],
        original_body: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        captured["body_rules"] = body_rules
        captured["original_body"] = original_body
        return body

    async def fake_run_endpoint_check(**kwargs: Any) -> dict[str, Any]:
        captured["json_body"] = kwargs["json_body"]
        return {"status_code": 200, "json_body": kwargs["json_body"]}

    def fake_build_request_body(
        cls: type[ClaudeChatAdapter],
        request_data: dict[str, Any] | None = None,
        *,
        base_url: str | None = None,
        provider_type: str | None = None,
    ) -> dict[str, Any]:
        del request_data, base_url, provider_type
        return {
            "messages": [{"role": "user", "content": "hello"}],
            "system": "keep",
        }

    monkeypatch.setattr(request_builder_module, "apply_body_rules", fake_apply_body_rules)
    monkeypatch.setattr(endpoint_checker_module, "run_endpoint_check", fake_run_endpoint_check)
    monkeypatch.setattr(
        ClaudeChatAdapter, "build_request_body", classmethod(fake_build_request_body)
    )

    result = await ClaudeChatAdapter.check_endpoint(
        client=None,  # type: ignore[arg-type]
        base_url="https://api.anthropic.com/v1",
        api_key="test-key",
        request_data={"model": "claude-sonnet-4-5-20250929", "stream": False},
        body_rules=[{"action": "set", "path": "messages", "value": []}],
    )

    assert captured["original_body"] == captured["json_body"]
    assert result["status_code"] == 200
    assert result["json_body"] == captured["json_body"]


@pytest.mark.asyncio
async def test_gemini_check_endpoint_passes_original_body_to_body_rules(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from src.api.handlers.base import endpoint_checker as endpoint_checker_module
    from src.api.handlers.base import request_builder as request_builder_module

    captured: dict[str, Any] = {}

    def fake_apply_body_rules(
        body: dict[str, Any],
        body_rules: list[dict[str, Any]],
        original_body: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        captured["body_rules"] = body_rules
        captured["original_body"] = original_body
        return body

    async def fake_run_endpoint_check(**kwargs: Any) -> dict[str, Any]:
        captured["json_body"] = kwargs["json_body"]
        return {"status_code": 200, "json_body": kwargs["json_body"]}

    def fake_build_request_body(
        cls: type[GeminiChatAdapter], request_data: dict[str, Any] | None = None
    ) -> dict[str, Any]:
        _ = cls, request_data
        return {
            "contents": [{"role": "user", "parts": [{"text": "hello"}]}],
            "systemInstruction": {"parts": [{"text": "system"}]},
            "toolConfig": {"functionCallingConfig": {"mode": "AUTO"}},
            "generationConfig": {"temperature": 0.1},
        }

    monkeypatch.setattr(request_builder_module, "apply_body_rules", fake_apply_body_rules)
    monkeypatch.setattr(endpoint_checker_module, "run_endpoint_check", fake_run_endpoint_check)
    monkeypatch.setattr(
        GeminiChatAdapter, "build_request_body", classmethod(fake_build_request_body)
    )

    result = await GeminiChatAdapter.check_endpoint(
        client=None,  # type: ignore[arg-type]
        base_url="https://generativelanguage.googleapis.com",
        api_key="test-key",
        request_data={"model": "gemini-2.5-pro", "stream": False},
        body_rules=[{"action": "drop", "path": "toolConfig"}],
    )

    assert captured["original_body"] == captured["json_body"]
    assert result["status_code"] == 200
    assert result["json_body"] == captured["json_body"]
