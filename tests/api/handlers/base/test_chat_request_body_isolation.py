from __future__ import annotations

import copy
from types import SimpleNamespace
from typing import Any

import pytest

import src.api.handlers.base.chat_handler_base as chatmod
from src.api.handlers.base.chat_handler_base import ChatHandlerBase
from src.api.handlers.base.chat_sync_executor import ChatSyncExecutor
from src.api.handlers.base.stream_context import StreamContext
from src.services.task.request_state import MutableRequestBodyState


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


class _DummyChatHandler(ChatHandlerBase):
    FORMAT_ID = "openai:chat"

    def __init__(self) -> None:
        self.request_id = "req-test"
        self.api_key = SimpleNamespace(id="user-key-1")
        self._request_builder = _CaptureBuilder()
        self.allowed_api_formats = ["openai:chat"]
        self.api_family = None
        self.endpoint_kind = None

    async def _convert_request(self, request: Any) -> Any:
        return request

    def _extract_usage(self, response: dict) -> dict[str, int]:
        return {}

    async def _get_mapped_model(
        self,
        source_model: str,
        provider_id: str,
        api_format: str | None = None,
    ) -> str | None:
        del source_model, provider_id, api_format
        return None

    def apply_mapped_model(self, request_body: dict[str, Any], mapped_model: str) -> dict[str, Any]:
        out = dict(request_body)
        out["model"] = mapped_model
        return out

    def prepare_provider_request_body(self, request_body: dict[str, Any]) -> dict[str, Any]:
        request_body["messages"][0]["content"] = "prepared"
        return request_body

    def finalize_provider_request(
        self,
        request_body: dict[str, Any],
        *,
        mapped_model: str | None,
        provider_api_format: str | None,
    ) -> dict[str, Any]:
        del mapped_model, provider_api_format
        request_body["messages"].append({"role": "assistant", "content": "finalized"})
        return request_body

    def get_model_for_url(
        self,
        request_body: dict[str, Any],
        mapped_model: str | None,
    ) -> str | None:
        return mapped_model or str(request_body.get("model") or "")


def _patch_chat_upstream(monkeypatch: pytest.MonkeyPatch) -> None:
    async def _fake_get_provider_auth(endpoint: Any, key: Any) -> _DummyAuthInfo:
        return _DummyAuthInfo()

    monkeypatch.setattr(chatmod, "get_provider_auth", _fake_get_provider_auth)
    monkeypatch.setattr(
        chatmod,
        "get_provider_behavior",
        lambda **kwargs: SimpleNamespace(
            envelope=None,
            same_format_variant=None,
            cross_format_variant=None,
        ),
    )
    monkeypatch.setattr(chatmod, "get_upstream_stream_policy", lambda *args, **kwargs: None)
    monkeypatch.setattr(
        chatmod,
        "resolve_upstream_is_stream",
        lambda *, client_is_stream, policy: client_is_stream,
    )
    monkeypatch.setattr(chatmod, "enforce_stream_mode_for_upstream", lambda *args, **kwargs: None)
    monkeypatch.setattr(
        chatmod,
        "maybe_patch_request_with_prompt_cache_key",
        lambda request_body, **kwargs: request_body,
    )


@pytest.mark.asyncio
async def test_chat_execute_stream_request_does_not_mutate_original_request_body(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _patch_chat_upstream(monkeypatch)

    handler = _DummyChatHandler()
    ctx = StreamContext(model="gpt-test", api_format="openai:chat")
    ctx.client_api_format = "openai:chat"

    provider = SimpleNamespace(name="provider", id="provider-1", provider_type="", proxy=None)
    endpoint = SimpleNamespace(id="endpoint-1", api_format="openai:chat", base_url="https://x")
    key = SimpleNamespace(id="key-1", proxy=None)
    candidate = SimpleNamespace(
        mapping_matched_model=None, needs_conversion=False, output_limit=None
    )

    original_request_body = {
        "model": "gpt-test",
        "messages": [{"role": "user", "content": "hello"}],
    }
    snapshot = copy.deepcopy(original_request_body)
    request_state = MutableRequestBodyState(original_request_body)

    with pytest.raises(_StopBuild):
        await handler._execute_stream_request(
            ctx,
            object(),
            provider,
            endpoint,
            key,
            request_state.build_attempt_body(),
            {},
            candidate=candidate,
        )

    assert original_request_body == snapshot
    assert handler._request_builder.request_body is not None
    assert handler._request_builder.request_body["messages"][0]["content"] == "prepared"
    assert handler._request_builder.request_body["messages"][-1]["content"] == "finalized"


@pytest.mark.asyncio
async def test_chat_sync_request_func_does_not_mutate_original_request_body(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _patch_chat_upstream(monkeypatch)

    handler = _DummyChatHandler()
    executor = ChatSyncExecutor(handler)

    provider = SimpleNamespace(name="provider", id="provider-1", provider_type="", proxy=None)
    endpoint = SimpleNamespace(id="endpoint-1", api_format="openai:chat", base_url="https://x")
    key = SimpleNamespace(id="key-1", proxy=None)
    candidate = SimpleNamespace(
        mapping_matched_model=None, needs_conversion=False, output_limit=None
    )

    original_request_body = {
        "model": "gpt-test",
        "messages": [{"role": "user", "content": "hello"}],
    }
    snapshot = copy.deepcopy(original_request_body)
    request_state = MutableRequestBodyState(original_request_body)

    with pytest.raises(_StopBuild):
        await executor._sync_request_func(
            provider,
            endpoint,
            key,
            candidate,
            model="gpt-test",
            api_format="openai:chat",
            original_headers={},
            request_state=request_state,
        )

    assert original_request_body == snapshot
    assert handler._request_builder.request_body is not None
    assert handler._request_builder.request_body["messages"][0]["content"] == "prepared"
    assert handler._request_builder.request_body["messages"][-1]["content"] == "finalized"
