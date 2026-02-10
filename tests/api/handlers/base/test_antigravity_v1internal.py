from __future__ import annotations

import json
from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock, patch

import pytest

from src.api.handlers.base.stream_context import StreamContext
from src.api.handlers.gemini_cli.handler import GeminiCliMessageHandler
from src.services.provider.adapters.antigravity.envelope import (
    unwrap_v1internal_response,
    wrap_v1internal_request,
)


def _make_handler() -> GeminiCliMessageHandler:
    return GeminiCliMessageHandler(
        db=MagicMock(),  # type: ignore[arg-type]
        user=SimpleNamespace(id=1),  # type: ignore[arg-type]
        api_key=SimpleNamespace(id=1),  # type: ignore[arg-type]
        request_id="req_1",
        client_ip="127.0.0.1",
        user_agent="pytest",
        start_time=0.0,
    )


def test_wrap_v1internal_request_removes_inner_model() -> None:
    gemini_request = {
        "model": "gemini-2.0-flash",
        "contents": [{"role": "user", "parts": [{"text": "hi"}]}],
    }

    wrapped = wrap_v1internal_request(
        gemini_request,
        project_id="project-123",
        model="gemini-2.0-flash",
    )

    assert wrapped["project"] == "project-123"
    assert wrapped["model"] == "gemini-2.0-flash"
    assert "request" in wrapped
    assert "model" not in wrapped["request"]


def test_unwrap_v1internal_response() -> None:
    v1_resp = {
        "response": {"candidates": [{"content": {"parts": [{"text": "Hello"}]}}]},
        "responseId": "resp-123",
    }

    unwrapped = unwrap_v1internal_response(v1_resp)

    assert "response" not in unwrapped
    assert "candidates" in unwrapped
    assert unwrapped["_v1internal_response_id"] == "resp-123"


def test_convert_sse_line_unwraps_before_convert_stream_chunk() -> None:
    handler = _make_handler()

    ctx = StreamContext(model="gemini-2.0-flash", api_format="gemini:cli")
    ctx.provider_type = "antigravity"
    ctx.provider_api_format = "gemini:chat"
    ctx.client_api_format = "gemini:cli"

    v1_line = (
        'data: {"response": {"candidates": [{"content": {"parts": [{"text": "Hi"}]}}]},'
        ' "responseId": "123"}'
    )

    seen: dict[str, object] = {}

    class _DummyRegistry:
        def convert_stream_chunk(
            self, data_obj: object, *_args: object, **_kwargs: object
        ) -> list[str]:
            seen["data_obj"] = data_obj
            return []

    with patch(
        "src.api.handlers.base.cli_handler_base.get_format_converter_registry",
        return_value=_DummyRegistry(),
    ):
        _lines, _events = handler._convert_sse_line(ctx, v1_line, [])

    assert isinstance(seen.get("data_obj"), dict)
    assert "response" not in seen["data_obj"]  # type: ignore[operator]
    assert "_v1internal_response_id" in seen["data_obj"]  # type: ignore[operator]


def test_handle_sse_event_unwraps_for_antigravity() -> None:
    handler = _make_handler()

    ctx = StreamContext(model="gemini-2.0-flash", api_format="gemini:cli")
    ctx.provider_type = "antigravity"

    v1_data = {
        "response": {
            "candidates": [
                {
                    "content": {"parts": [{"text": "Hello"}], "role": "model"},
                    "finishReason": "STOP",
                }
            ],
            "modelVersion": "gemini-2.0-flash-001",
        },
        "responseId": "123",
    }

    with patch.object(handler, "_process_event_data") as mock_process:
        handler._handle_sse_event(ctx, None, json.dumps(v1_data), record_chunk=False)

    assert mock_process.call_count == 1
    passed_data = mock_process.call_args[0][2]
    assert isinstance(passed_data, dict)
    assert "response" not in passed_data
    assert "candidates" in passed_data


def test_handle_sse_event_caches_thought_signature_for_antigravity() -> None:
    from src.services.provider.adapters.antigravity.signature_cache import signature_cache

    signature_cache.clear()

    handler = _make_handler()
    ctx = StreamContext(model="claude-sonnet-4-5", api_format="gemini:cli")
    ctx.provider_type = "antigravity"

    # 签名须 >= MIN_SIGNATURE_LENGTH(50)，否则会被忽略
    long_sig = "a" * 60

    payload = {
        "candidates": [
            {
                "content": {
                    "parts": [
                        {
                            "text": "t1",
                            "thought": True,
                            "thoughtSignature": long_sig,
                        }
                    ]
                }
            }
        ]
    }

    with patch.object(handler, "_process_event_data") as _mock_process:
        handler._handle_sse_event(ctx, None, json.dumps(payload), record_chunk=False)

    assert signature_cache.get_or_dummy("claude-sonnet-4-5", "t1") == long_sig


def test_provider_type_drives_antigravity_usage_path() -> None:
    handler = _make_handler()

    event = {"usageMetadata": {"promptTokenCount": 10, "candidatesTokenCount": 2}}
    usage = handler._extract_usage_from_event(event, provider_type="antigravity")

    assert usage["input_tokens"] == 10
    assert usage["output_tokens"] == 2


@pytest.mark.asyncio
async def test_antigravity_forces_conversion_path_in_stream_with_prefetch() -> None:
    handler = _make_handler()

    ctx = StreamContext(model="gemini-2.0-flash", api_format="gemini:cli")
    ctx.provider_type = "antigravity"
    ctx.needs_conversion = False  # same-format case normally would passthrough

    class _AsyncIter:
        def __init__(self, items: list[bytes]) -> None:
            self._it = iter(items)

        def __aiter__(self) -> _AsyncIter:
            return self

        async def __anext__(self) -> bytes:
            try:
                return next(self._it)
            except StopIteration as e:
                raise StopAsyncIteration from e

    prefetched = [
        b'data: {"response": {"candidates": []}, "responseId": "1"}\n',
    ]
    byte_iter = _AsyncIter([])  # no more bytes after prefetch
    response_ctx = SimpleNamespace(__aexit__=AsyncMock(return_value=None))
    http_client = SimpleNamespace(aclose=AsyncMock(return_value=None))

    with patch.object(
        handler,
        "_convert_sse_line",
        return_value=(["data: {}"], []),
    ) as mock_convert:
        out = []
        async for chunk in handler._create_response_stream_with_prefetch(
            ctx, byte_iter, response_ctx, http_client, prefetched  # type: ignore[arg-type]
        ):
            out.append(chunk)

    assert mock_convert.call_count >= 1
    assert any(b"data: {}" in c for c in out)


def test_wrap_v1internal_request_injects_thought_signature_from_tool_cache() -> None:
    from src.services.provider.adapters.antigravity.signature_cache import signature_cache

    signature_cache.clear()

    tool_id = "toolu_123"
    sig = "a" * 60
    signature_cache.cache_tool_signature(tool_id, sig)

    gemini_request = {
        "model": "gemini-2.0-flash",
        "contents": [
            {"role": "user", "parts": [{"text": "hi"}]},
            {"role": "model", "parts": [{"function_call": {"name": "do", "id": tool_id}}]},
        ],
    }

    wrapped = wrap_v1internal_request(
        gemini_request,
        project_id="project-123",
        model="gemini-2.0-flash",
    )

    model_turn = wrapped["request"]["contents"][1]
    part = model_turn["parts"][0]
    assert part["thoughtSignature"] == sig


def test_wrap_v1internal_request_injects_session_signature_when_tool_cache_missing() -> None:
    from src.services.provider.adapters.antigravity.signature_cache import signature_cache

    signature_cache.clear()

    session_id = "sid-123"
    sig = "b" * 60
    signature_cache.cache_session_signature(session_id, sig, message_count=2)

    gemini_request = {
        "model": "gemini-2.0-flash",
        "sessionId": session_id,
        "contents": [
            {"role": "user", "parts": [{"text": "hi"}]},
            {
                "role": "model",
                "parts": [{"functionCall": {"name": "do", "id": "toolu_missing"}}],
            },
        ],
    }

    wrapped = wrap_v1internal_request(
        gemini_request,
        project_id="project-123",
        model="gemini-2.0-flash",
    )

    model_turn = wrapped["request"]["contents"][1]
    part = model_turn["parts"][0]
    assert part["thoughtSignature"] == sig

