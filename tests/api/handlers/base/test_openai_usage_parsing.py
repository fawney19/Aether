from __future__ import annotations

from typing import Any

from src.api.handlers.base.parsers import OpenAICliResponseParser, OpenAIResponseParser
from src.api.handlers.base.response_parser import (
    ParsedChunk,
    ParsedResponse,
    ResponseParser,
    StreamStats,
)
from src.api.handlers.base.stream_context import StreamContext
from src.api.handlers.base.stream_processor import StreamProcessor


class _DummyParser(ResponseParser):
    def parse_sse_line(self, line: str, stats: StreamStats) -> ParsedChunk | None:
        return None

    def parse_response(self, response: dict[str, Any], status_code: int) -> ParsedResponse:
        return ParsedResponse(raw_response=response, status_code=status_code)

    def extract_usage_from_response(self, response: dict[str, Any]) -> dict[str, int]:
        return {}

    def extract_text_content(self, response: dict[str, Any]) -> str:
        return ""


def test_openai_response_parser_extracts_cached_tokens_from_prompt_tokens_details() -> None:
    parser = OpenAIResponseParser()

    usage = parser.extract_usage_from_response(
        {
            "usage": {
                "prompt_tokens": 120,
                "completion_tokens": 18,
                "prompt_tokens_details": {"cached_tokens": 96},
            }
        }
    )

    assert usage["input_tokens"] == 120
    assert usage["output_tokens"] == 18
    assert usage["cache_read_tokens"] == 96


def test_openai_cli_response_parser_extracts_cached_tokens_from_input_tokens_details() -> None:
    parser = OpenAICliResponseParser()

    usage = parser.extract_usage_from_response(
        {
            "type": "response.completed",
            "response": {
                "usage": {
                    "input_tokens": 2048,
                    "output_tokens": 128,
                    "input_tokens_details": {"cached_tokens": 1792},
                }
            },
        }
    )

    assert usage["input_tokens"] == 2048
    assert usage["output_tokens"] == 128
    assert usage["cache_read_tokens"] == 1792


def test_stream_processor_extracts_cached_tokens_from_openai_cli_converted_event() -> None:
    processor = StreamProcessor(request_id="req_test", default_parser=_DummyParser())
    ctx = StreamContext(model="gpt-5", api_format="openai:chat")

    processor._extract_usage_from_converted_event(
        ctx,
        {
            "type": "response.completed",
            "response": {
                "usage": {
                    "input_tokens": 4096,
                    "output_tokens": 64,
                    "input_tokens_details": {"cached_tokens": 3584},
                }
            },
        },
        "response.completed",
    )

    assert ctx.input_tokens == 4096
    assert ctx.output_tokens == 64
    assert ctx.cached_tokens == 3584


def test_stream_processor_extracts_cached_tokens_from_openai_chat_converted_event() -> None:
    processor = StreamProcessor(request_id="req_test", default_parser=_DummyParser())
    ctx = StreamContext(model="gpt-5", api_format="openai:chat")

    processor._extract_usage_from_converted_event(
        ctx,
        {
            "object": "chat.completion.chunk",
            "choices": [],
            "usage": {
                "prompt_tokens": 512,
                "completion_tokens": 21,
                "prompt_tokens_details": {"cached_tokens": 480},
            },
        },
        "chat.completion.chunk",
    )

    assert ctx.input_tokens == 512
    assert ctx.output_tokens == 21
    assert ctx.cached_tokens == 480
