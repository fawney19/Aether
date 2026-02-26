from __future__ import annotations

import json
from collections.abc import AsyncIterator

import pytest

from src.api.handlers.base.upstream_stream_bridge import aggregate_upstream_stream_to_internal_response
from src.core.api_format.conversion import register_default_normalizers
from src.core.api_format.conversion.internal import TextBlock


async def _iter_stream_lines(lines: list[str]) -> AsyncIterator[bytes]:
    for line in lines:
        yield line.encode("utf-8")


@pytest.mark.asyncio
async def test_aggregate_claude_stream_uses_message_start_usage_when_message_delta_absent() -> None:
    register_default_normalizers()

    lines = [
        "data: "
        + json.dumps(
            {
                "type": "message_start",
                "message": {
                    "id": "msg_bridge_usage",
                    "type": "message",
                    "role": "assistant",
                    "model": "claude-sonnet-4-5",
                    "content": [],
                    "usage": {
                        "input_tokens": 120,
                        "output_tokens": 0,
                        "cache_read_input_tokens": 11,
                    },
                },
            },
            ensure_ascii=False,
        )
        + "\n",
        "data: "
        + json.dumps(
            {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}},
            ensure_ascii=False,
        )
        + "\n",
        "data: "
        + json.dumps(
            {
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "text_delta", "text": "hello"},
            },
            ensure_ascii=False,
        )
        + "\n",
        "data: " + json.dumps({"type": "content_block_stop", "index": 0}, ensure_ascii=False) + "\n",
    ]

    internal = await aggregate_upstream_stream_to_internal_response(
        _iter_stream_lines(lines),
        provider_api_format="claude:cli",
        provider_name="claude_code",
        model="claude-sonnet-4-5",
        request_id="req_bridge_usage",
    )

    assert internal.usage is not None
    assert internal.usage.input_tokens == 120
    assert internal.usage.output_tokens == 0
    assert internal.usage.cache_read_tokens == 11
    assert len(internal.content) == 1
    assert isinstance(internal.content[0], TextBlock)
    assert internal.content[0].text == "hello"
