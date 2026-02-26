import json
from typing import Any, AsyncIterator

import httpx
import pytest

from src.api.handlers.base.response_parser import (
    ParsedChunk,
    ParsedResponse,
    ResponseParser,
    StreamStats,
)
from src.api.handlers.base.stream_context import StreamContext
from src.api.handlers.base.stream_processor import StreamProcessor
from src.utils.sse_parser import SSEEventParser


class DummyParser(ResponseParser):
    def parse_sse_line(self, line: str, stats: StreamStats) -> ParsedChunk | None:
        return None

    def parse_response(self, response: dict[str, Any], status_code: int) -> ParsedResponse:
        return ParsedResponse(raw_response=response, status_code=status_code)

    def extract_usage_from_response(self, response: dict[str, Any]) -> dict[str, int]:
        return {}

    def extract_text_content(self, response: dict[str, Any]) -> str:
        return ""


def test_process_line_strips_newlines_and_finalizes_event() -> None:
    ctx = StreamContext(model="test-model", api_format="openai:chat")
    processor = StreamProcessor(request_id="test-request", default_parser=DummyParser())
    sse_parser = SSEEventParser()

    processor._process_line(ctx, sse_parser, 'data: {"type":"response.completed"}\n')
    processor._process_line(ctx, sse_parser, "\n")

    assert ctx.has_completion is True


def test_process_line_updates_openai_usage_from_usage_only_chunk() -> None:
    ctx = StreamContext(model="test-model", api_format="openai:chat")
    ctx.provider_api_format = "openai:chat"
    processor = StreamProcessor(request_id="test-request", default_parser=DummyParser())
    sse_parser = SSEEventParser()

    usage_chunk = {
        "id": "chatcmpl_test",
        "object": "chat.completion.chunk",
        "choices": [],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15},
    }

    processor._process_line(ctx, sse_parser, f"data: {json.dumps(usage_chunk)}\n")
    processor._process_line(ctx, sse_parser, "\n")

    assert ctx.input_tokens == 10
    assert ctx.output_tokens == 5


def test_process_line_handles_openai_usage_chunk_followed_by_done_without_blank_line() -> None:
    ctx = StreamContext(model="test-model", api_format="openai:chat")
    ctx.provider_api_format = "openai:chat"
    processor = StreamProcessor(request_id="test-request", default_parser=DummyParser())
    sse_parser = SSEEventParser()

    usage_chunk = {
        "id": "chatcmpl_test",
        "object": "chat.completion.chunk",
        "choices": [],
        "usage": {"prompt_tokens": 7, "completion_tokens": 3, "total_tokens": 10},
    }

    # Some SSE implementations may emit consecutive data lines without an empty separator.
    processor._process_line(ctx, sse_parser, f"data: {json.dumps(usage_chunk)}\n")
    processor._process_line(ctx, sse_parser, "data: [DONE]\n")
    processor._process_line(ctx, sse_parser, "\n")

    assert ctx.input_tokens == 7
    assert ctx.output_tokens == 3
    assert ctx.has_completion is True


def test_extract_usage_from_claude_message_delta_delta_usage() -> None:
    ctx = StreamContext(model="test-model", api_format="claude:chat")
    processor = StreamProcessor(request_id="test-request", default_parser=DummyParser())

    processor._extract_usage_from_converted_event(
        ctx,
        {
            "type": "message_delta",
            "delta": {
                "usage": {
                    "input_tokens": "12",
                    "output_tokens": "6",
                    "cache_read_input_tokens": "4",
                    "cache_creation_input_tokens": "9",
                }
            },
        },
        "message_delta",
    )

    assert ctx.input_tokens == 12
    assert ctx.output_tokens == 6
    assert ctx.cached_tokens == 4
    assert ctx.cache_creation_tokens == 9
    assert ctx.final_usage == {
        "input_tokens": 12,
        "output_tokens": 6,
        "cache_read_tokens": 4,
        "cache_creation_tokens": 9,
    }


def test_normalize_usage_payload_supports_legacy_keys_and_clamps_values() -> None:
    normalized = StreamProcessor._normalize_usage_payload(
        {
            "prompt_tokens": "11",
            "completion_tokens": "5",
            "cached_tokens": "3",
            "cache_creation": {"ephemeral_5m_input_tokens": 7},
        }
    )

    assert normalized == {
        "input_tokens": 11,
        "output_tokens": 5,
        "cache_read_tokens": 3,
        "cache_creation_tokens": 7,
    }


def test_normalize_usage_payload_negative_values_are_clamped_to_zero() -> None:
    normalized = StreamProcessor._normalize_usage_payload(
        {
            "input_tokens": "-1",
            "output_tokens": -5,
            "cache_read_tokens": "-9",
            "cache_creation_tokens": "-8",
        }
    )

    assert normalized == {
        "input_tokens": 0,
        "output_tokens": 0,
        "cache_read_tokens": 0,
        "cache_creation_tokens": 0,
    }


class _DummyResponseCtx:
    async def __aexit__(self, exc_type: type | None, exc: BaseException | None, tb: object) -> None:
        return None


@pytest.mark.asyncio
async def test_create_response_stream_flushes_usage_on_remote_protocol_error() -> None:
    ctx = StreamContext(model="test-model", api_format="openai:chat")
    ctx.provider_api_format = "openai:chat"
    processor = StreamProcessor(request_id="test-request", default_parser=DummyParser())

    usage_chunk = {
        "id": "chatcmpl_test",
        "object": "chat.completion.chunk",
        "choices": [],
        "usage": {"prompt_tokens": 11, "completion_tokens": 4, "total_tokens": 15},
    }

    async def _iter_bytes_then_remote_protocol_error() -> AsyncIterator[bytes]:
        yield f"data: {json.dumps(usage_chunk)}\n".encode("utf-8")
        raise httpx.RemoteProtocolError("boom")

    out = b""
    async for b in processor.create_response_stream(
        ctx=ctx,
        byte_iterator=_iter_bytes_then_remote_protocol_error(),
        response_ctx=_DummyResponseCtx(),
        prefetched_chunks=[],
        start_time=None,
    ):
        out += b

    # Stream ends gracefully (no exception), but usage is best-effort captured and request is marked failed.
    assert b"data:" in out
    assert ctx.input_tokens == 11
    assert ctx.output_tokens == 4
    assert ctx.status_code == 502
    assert (ctx.error_message or "").startswith("upstream_stream_error:")
