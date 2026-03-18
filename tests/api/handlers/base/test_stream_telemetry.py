from __future__ import annotations

import os
import time
from types import SimpleNamespace
from typing import Any
from unittest.mock import AsyncMock

import pytest

os.environ.setdefault("JWT_SECRET_KEY", "test-secret-key")

from src.api.handlers.base import stream_telemetry as stream_telemetry_module
from src.api.handlers.base.stream_context import StreamContext
from src.api.handlers.base.stream_telemetry import StreamTelemetryRecorder


class _DummyDb:
    def close(self) -> None:
        pass


@pytest.mark.asyncio
async def test_record_stream_stats_estimates_tokens_for_failed_partial_stream(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    recorder = StreamTelemetryRecorder(
        request_id="req-telemetry",
        user_id="1",
        api_key_id="2",
        client_ip="127.0.0.1",
        format_id="openai:chat",
    )
    recorder._get_telemetry_writer = AsyncMock(  # type: ignore[method-assign]
        return_value=SimpleNamespace(include_bodies=False)
    )
    recorder._dispatch_record = AsyncMock()  # type: ignore[method-assign]
    recorder._update_candidate_status = AsyncMock()  # type: ignore[method-assign]

    ctx = StreamContext(
        model="test-model",
        api_format="openai:chat",
        request_id="req-telemetry",
        user_id=1,
        api_key_id=2,
    )
    ctx.provider_name = "test-provider"
    ctx.status_code = 503
    ctx.data_count = 2
    ctx.chunk_count = 4
    ctx.append_text("partial output")

    monkeypatch.setattr(stream_telemetry_module, "get_db", lambda: iter([_DummyDb()]))
    monkeypatch.setattr(
        stream_telemetry_module.SystemConfigService,
        "should_log_body",
        lambda _db: False,
    )
    monkeypatch.setattr(stream_telemetry_module.config, "stream_stats_delay", 0)

    await recorder.record_stream_stats(
        ctx,
        original_headers={},
        original_request_body={"input": [{"content": "hello world"}]},
        start_time=time.time(),
    )

    assert ctx.input_tokens > 0
    assert ctx.output_tokens == max(1, len("partial output") // 4)
    recorder._dispatch_record.assert_awaited_once()  # type: ignore[attr-defined]


@pytest.mark.asyncio
async def test_record_stream_stats_releases_parsed_chunks_after_dispatch(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    recorder = StreamTelemetryRecorder(
        request_id="req-release",
        user_id="1",
        api_key_id="2",
        client_ip="127.0.0.1",
        format_id="openai:chat",
    )
    recorder._get_telemetry_writer = AsyncMock(  # type: ignore[method-assign]
        return_value=SimpleNamespace(include_bodies=True)
    )
    recorder._update_candidate_status = AsyncMock()  # type: ignore[method-assign]

    dispatch_payloads: list[dict[str, Any]] = []

    async def _capture_dispatch(*args: Any, **kwargs: Any) -> None:
        dispatch_payloads.append(
            {
                "response_body": args[5],
                "client_response_body": kwargs.get("client_response_body"),
            }
        )

    recorder._dispatch_record = _capture_dispatch  # type: ignore[method-assign]

    ctx = StreamContext(
        model="test-model",
        api_format="openai:chat",
        request_id="req-release",
        user_id=1,
        api_key_id=2,
    )
    ctx.provider_name = "test-provider"
    ctx.parsed_chunks.extend([{"type": "chunk-1"}, {"type": "chunk-2"}])
    ctx.data_count = 2
    ctx.chunk_count = 2

    monkeypatch.setattr(stream_telemetry_module, "get_db", lambda: iter([_DummyDb()]))
    monkeypatch.setattr(
        stream_telemetry_module.SystemConfigService,
        "should_log_body",
        lambda _db: True,
    )
    monkeypatch.setattr(stream_telemetry_module.config, "stream_stats_delay", 0)

    await recorder.record_stream_stats(
        ctx,
        original_headers={},
        original_request_body={"input": [{"content": "hello world"}]},
        start_time=time.time(),
    )

    assert len(dispatch_payloads) == 1
    response_body = dispatch_payloads[0]["response_body"]
    assert response_body["chunks"] == [{"type": "chunk-1"}, {"type": "chunk-2"}]
    assert response_body["metadata"]["stream"] is True
    assert response_body["metadata"]["total_chunks"] == 2
    assert response_body["metadata"]["data_count"] == 2
    assert dispatch_payloads[0]["client_response_body"] is None
    assert ctx.parsed_chunks == []
    assert ctx.provider_parsed_chunks == []
