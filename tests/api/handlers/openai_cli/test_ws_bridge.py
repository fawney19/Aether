from __future__ import annotations

import json
from collections.abc import AsyncIterator
from types import SimpleNamespace
from typing import Any

import pytest
from fastapi import FastAPI, HTTPException
from fastapi.responses import StreamingResponse
from fastapi.testclient import TestClient

from src.api.handlers.openai_cli.ws_bridge import (
    CodexWsProtocolError,
    extract_sse_payloads,
    iter_sse_payloads,
    parse_codex_ws_request,
)
from src.api.public import openai as openai_routes


class FakeDb:
    def __init__(self) -> None:
        self.commits = 0
        self.rollbacks = 0
        self.closed = False

    def commit(self) -> None:
        self.commits += 1

    def rollback(self) -> None:
        self.rollbacks += 1

    def close(self) -> None:
        self.closed = True


def test_parse_response_create_removes_ws_type() -> None:
    request = parse_codex_ws_request(
        json.dumps(
            {
                "type": "response.create",
                "model": "gpt-5",
                "stream": True,
                "input": [{"role": "user", "content": "hello"}],
                "generate": False,
            }
        )
    )

    assert request.kind == "response.create"
    assert request.body == {
        "model": "gpt-5",
        "stream": True,
        "input": [{"role": "user", "content": "hello"}],
        "generate": False,
    }


def test_parse_response_processed_is_noop_request() -> None:
    request = parse_codex_ws_request(
        json.dumps({"type": "response.processed", "response_id": "resp_123"})
    )

    assert request.kind == "response.processed"
    assert request.body is None


def test_parse_invalid_frame_raises_protocol_error() -> None:
    with pytest.raises(CodexWsProtocolError):
        parse_codex_ws_request("{not-json")

    with pytest.raises(CodexWsProtocolError):
        parse_codex_ws_request(json.dumps({"type": "response.unknown"}))


def test_extract_sse_payloads_skips_done() -> None:
    assert extract_sse_payloads('event: response.created\ndata: {"type":"response.created"}') == [
        '{"type":"response.created"}'
    ]
    assert extract_sse_payloads("data: [DONE]") == []
    assert extract_sse_payloads('{"type":"response.completed"}') == [
        '{"type":"response.completed"}'
    ]


@pytest.mark.asyncio
async def test_iter_sse_payloads_handles_split_chunks() -> None:
    async def chunks() -> AsyncIterator[bytes]:
        yield b'data: {"type":"response.created"}\n'
        yield b'\ndata: {"type":"response.completed"}\n\n'
        yield b"data: [DONE]\n\n"

    payloads = [payload async for payload in iter_sse_payloads(chunks())]

    assert payloads == [
        '{"type":"response.created"}',
        '{"type":"response.completed"}',
    ]


def _build_app(monkeypatch: pytest.MonkeyPatch, pipeline: Any, db: FakeDb | None = None) -> FastAPI:
    app = FastAPI()
    app.include_router(openai_routes.router)
    monkeypatch.setattr(openai_routes, "pipeline", pipeline)
    if db is not None:
        import src.api.handlers.openai_cli.ws_bridge as ws_bridge

        monkeypatch.setattr(ws_bridge, "create_session", lambda: db)
    return app


def test_websocket_response_create_streams_sse_as_text_frames(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    seen_bodies: list[dict[str, Any]] = []
    seen_headers: list[dict[str, str]] = []
    db = FakeDb()

    class FakePipeline:
        async def run(self, *, http_request: Any, **_kwargs: Any) -> StreamingResponse:
            seen_bodies.append(json.loads((await http_request.body()).decode("utf-8")))
            seen_headers.append(dict(http_request.headers))

            async def body() -> AsyncIterator[bytes]:
                yield b'data: {"type":"response.created","response":{"id":"resp_1"}}\n\n'
                yield b'data: {"type":"response.output_text.delta","delta":"hi"}\n\n'
                yield (
                    b'data: {"type":"response.completed",'
                    b'"response":{"id":"resp_1","status":"completed"}}\n\n'
                )
                yield b"data: [DONE]\n\n"

            return StreamingResponse(body(), media_type="text/event-stream")

    client = TestClient(_build_app(monkeypatch, FakePipeline(), db))

    with client.websocket_connect(
        "/v1/responses",
        headers={
            "Authorization": "Bearer test-key",
            "OpenAI-Beta": "responses_websockets=2026-02-06",
            "x-codex-window-id": "window-1",
        },
    ) as websocket:
        websocket.send_json(
            {
                "type": "response.create",
                "model": "gpt-5",
                "stream": True,
                "input": [{"role": "user", "content": "hello"}],
            }
        )

        assert websocket.receive_json() == {
            "type": "response.created",
            "response": {"id": "resp_1"},
        }
        assert websocket.receive_json() == {
            "type": "response.output_text.delta",
            "delta": "hi",
        }
        assert websocket.receive_json() == {
            "type": "response.completed",
            "response": {"id": "resp_1", "status": "completed"},
        }

    assert seen_bodies == [
        {
            "model": "gpt-5",
            "stream": True,
            "input": [{"role": "user", "content": "hello"}],
        }
    ]
    assert seen_headers[0]["authorization"] == "Bearer test-key"
    assert seen_headers[0]["openai-beta"] == "responses_websockets=2026-02-06"
    assert seen_headers[0]["x-codex-window-id"] == "window-1"
    assert "sec-websocket-key" not in seen_headers[0]
    assert db.commits == 1
    assert db.rollbacks == 0
    assert db.closed is True


def test_websocket_response_processed_does_not_call_pipeline(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = FakeDb()
    calls = SimpleNamespace(count=0)

    class FakePipeline:
        async def run(self, **_kwargs: Any) -> StreamingResponse:
            calls.count += 1

            async def body() -> AsyncIterator[bytes]:
                yield b'data: {"type":"response.completed","response":{"id":"resp_2"}}\n\n'

            return StreamingResponse(body(), media_type="text/event-stream")

    client = TestClient(_build_app(monkeypatch, FakePipeline(), db))

    with client.websocket_connect("/v1/responses") as websocket:
        websocket.send_json({"type": "response.processed", "response_id": "resp_1"})
        websocket.send_json({"type": "response.create", "model": "gpt-5", "stream": True})
        assert websocket.receive_json() == {
            "type": "response.completed",
            "response": {"id": "resp_2"},
        }

    assert calls.count == 1


def test_websocket_invalid_json_returns_wrapped_error(monkeypatch: pytest.MonkeyPatch) -> None:
    class FakePipeline:
        async def run(self, **_kwargs: Any) -> None:
            raise AssertionError("pipeline should not be called")

    client = TestClient(_build_app(monkeypatch, FakePipeline(), FakeDb()))

    with client.websocket_connect("/v1/responses") as websocket:
        websocket.send_text("{bad-json")
        assert websocket.receive_json() == {
            "type": "error",
            "status": 400,
            "error": {
                "type": "invalid_request_error",
                "message": "Request frame must be valid JSON",
            },
        }


def test_websocket_http_exception_returns_wrapped_error(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = FakeDb()

    class FakePipeline:
        async def run(self, **_kwargs: Any) -> None:
            raise HTTPException(
                status_code=429,
                detail={
                    "error": {
                        "type": "rate_limit_error",
                        "message": "slow down",
                        "code": "rate_limited",
                    }
                },
                headers={"Retry-After": "1"},
            )

    client = TestClient(_build_app(monkeypatch, FakePipeline(), db))

    with client.websocket_connect("/v1/responses") as websocket:
        websocket.send_json({"type": "response.create", "model": "gpt-5", "stream": True})
        assert websocket.receive_json() == {
            "type": "error",
            "status": 429,
            "error": {
                "type": "rate_limit_error",
                "message": "slow down",
                "code": "rate_limited",
            },
            "headers": {"Retry-After": "1"},
        }

    assert db.commits == 0
    assert db.rollbacks == 1
    assert db.closed is True
