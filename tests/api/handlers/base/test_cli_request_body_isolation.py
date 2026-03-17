from __future__ import annotations

import copy
from types import SimpleNamespace
from typing import Any

import pytest

import src.api.handlers.base.cli_stream_mixin as mixmod
from src.api.handlers.base.cli_stream_mixin import CliStreamMixin
from src.api.handlers.base.stream_context import StreamContext


class _StopBuild(Exception):
    pass


class _DummyAuthInfo:
    auth_header = "authorization"
    auth_value = "Bearer test"
    decrypted_auth_config = None

    def as_tuple(self) -> tuple[str, str]:
        return self.auth_header, self.auth_value


class _CaptureBuilder:
    def __init__(self) -> None:
        self.request_body: dict[str, Any] | None = None

    def build(self, request_body: dict[str, Any], *args: Any, **kwargs: Any) -> Any:
        self.request_body = request_body
        raise _StopBuild()


class _DummyCliStreamHandler(CliStreamMixin):
    FORMAT_ID = "openai:cli"

    def __init__(self) -> None:
        self.primary_api_format = "openai:cli"
        self.request_id = "req-test"
        self.api_key = SimpleNamespace(id="user-key-1")
        self._request_builder = _CaptureBuilder()

    async def _get_mapped_model(self, source_model: str, provider_id: str) -> str | None:
        return None

    def apply_mapped_model(self, request_body: dict[str, Any], mapped_model: str) -> dict[str, Any]:
        out = dict(request_body)
        out["model"] = mapped_model
        return out

    def prepare_provider_request_body(self, request_body: dict[str, Any]) -> dict[str, Any]:
        request_body.pop("_aether_compact", None)
        request_body["input"][0]["content"][0]["text"] = "prepared"
        return request_body

    def finalize_provider_request(
        self,
        request_body: dict[str, Any],
        *,
        mapped_model: str | None,
        provider_api_format: str | None,
    ) -> dict[str, Any]:
        request_body["input"][0]["content"].append({"type": "input_text", "text": "finalized"})
        return request_body

    def get_model_for_url(
        self,
        request_body: dict[str, Any],
        mapped_model: str | None,
    ) -> str | None:
        return mapped_model or str(request_body.get("model") or "")


@pytest.mark.asyncio
async def test_execute_stream_request_does_not_mutate_original_request_body(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    async def _fake_get_provider_auth(endpoint: Any, key: Any) -> _DummyAuthInfo:
        return _DummyAuthInfo()

    monkeypatch.setattr(mixmod, "get_provider_auth", _fake_get_provider_auth)
    monkeypatch.setattr(
        mixmod,
        "get_provider_behavior",
        lambda **kwargs: SimpleNamespace(
            envelope=None,
            same_format_variant=None,
            cross_format_variant=None,
        ),
    )
    monkeypatch.setattr(mixmod, "get_upstream_stream_policy", lambda *args, **kwargs: None)
    monkeypatch.setattr(
        mixmod,
        "resolve_upstream_is_stream",
        lambda *, client_is_stream, policy: client_is_stream,
    )
    monkeypatch.setattr(mixmod, "enforce_stream_mode_for_upstream", lambda *args, **kwargs: None)
    monkeypatch.setattr(
        mixmod,
        "maybe_patch_request_with_prompt_cache_key",
        lambda request_body, **kwargs: request_body,
    )

    handler = _DummyCliStreamHandler()
    ctx = StreamContext(model="gpt-test", api_format="openai:cli")
    ctx.client_api_format = "openai:cli"

    provider = SimpleNamespace(name="provider", id="provider-1", provider_type="", proxy=None)
    endpoint = SimpleNamespace(id="endpoint-1", api_format="openai:cli", base_url="https://x")
    key = SimpleNamespace(id="key-1", proxy=None)
    candidate = SimpleNamespace(
        mapping_matched_model=None, needs_conversion=False, output_limit=None
    )

    original_request_body = {
        "model": "gpt-test",
        "_aether_compact": True,
        "input": [
            {
                "role": "user",
                "content": [
                    {"type": "input_text", "text": "hello"},
                ],
            }
        ],
    }
    snapshot = copy.deepcopy(original_request_body)

    with pytest.raises(_StopBuild):
        await handler._execute_stream_request(
            ctx,
            provider,
            endpoint,
            key,
            original_request_body,
            {},
            candidate=candidate,
        )

    assert original_request_body == snapshot
    assert handler._request_builder.request_body is not None
    assert "_aether_compact" not in handler._request_builder.request_body
    assert handler._request_builder.request_body["input"][0]["content"][0]["text"] == "prepared"
    assert handler._request_builder.request_body["input"][0]["content"][-1]["text"] == "finalized"
