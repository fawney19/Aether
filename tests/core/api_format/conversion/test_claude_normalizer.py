"""
ClaudeNormalizer 单元测试

覆盖重点：
- system -> instructions 的提取与还原
- tool_use/tool_result 的往返转换
- UnknownBlock 内部保留、输出默认丢弃
- stop_reason/usage 的映射
- streaming event <-> InternalStreamEvent 的基础行为与状态
"""

from __future__ import annotations

from typing import Any, cast

from src.core.api_format.conversion.internal import (
    ErrorType,
    ImageBlock,
    StopReason,
    TextBlock,
    ToolResultBlock,
    ToolUseBlock,
    UnknownBlock,
)
from src.core.api_format.conversion.normalizers.claude import ClaudeNormalizer
from src.core.api_format.conversion.stream_events import (
    ContentBlockStartEvent,
    ContentDeltaEvent,
    MessageStartEvent,
    MessageStopEvent,
    ToolCallDeltaEvent,
)
from src.core.api_format.conversion.stream_state import StreamState


def test_claude_request_system_roundtrip() -> None:
    n = ClaudeNormalizer()

    req = {
        "model": "claude-3-opus",
        "system": "sys",
        "messages": [
            {"role": "user", "content": "hi"},
            {"role": "assistant", "content": [{"type": "text", "text": "ok"}]},
        ],
        "max_tokens": 10,
        "stop_sequences": ["A"],
        "stream": True,
    }

    internal = n.request_to_internal(req)
    assert internal.model == "claude-3-opus"
    assert [seg.role.value for seg in internal.instructions] == ["system"]
    assert internal.instructions[0].text == "sys"
    assert internal.system == "sys"
    assert [m.role.value for m in internal.messages] == ["user", "assistant"]
    assert internal.max_tokens == 10
    assert internal.stop_sequences == ["A"]
    assert internal.stream is True

    out = n.request_from_internal(internal)
    assert out["model"] == "claude-3-opus"
    assert out["system"] == "sys"

    out_messages: list[dict[str, Any]] = out["messages"]
    assert [m["role"] for m in out_messages] == ["user", "assistant"]
    assert out_messages[0]["content"] == "hi"
    assert out_messages[1]["content"] == "ok"


def test_claude_request_tool_blocks_roundtrip() -> None:
    n = ClaudeNormalizer()

    req = {
        "model": "claude-3-sonnet",
        "system": "sys",
        "messages": [
            {"role": "user", "content": "weather?"},
            {
                "role": "assistant",
                "content": [
                    {
                        "type": "tool_use",
                        "id": "toolu_1",
                        "name": "get_weather",
                        "input": {"city": "SF"},
                    }
                ],
            },
            {
                "role": "user",
                "content": [
                    {
                        "type": "tool_result",
                        "tool_use_id": "toolu_1",
                        "content": {"temp_c": 20},
                        "is_error": False,
                    }
                ],
            },
        ],
        "tools": [
            {
                "name": "get_weather",
                "description": "Get weather",
                "input_schema": {"type": "object", "properties": {"city": {"type": "string"}}},
            }
        ],
        "tool_choice": {"type": "any"},
        "max_tokens": 10,
    }

    internal = n.request_to_internal(req)
    assert [m.role.value for m in internal.messages] == ["user", "assistant", "user"]

    assistant_msg = internal.messages[1]
    assert any(isinstance(b, ToolUseBlock) for b in assistant_msg.content)
    tool_use = next(b for b in assistant_msg.content if isinstance(b, ToolUseBlock))
    assert tool_use.tool_id == "toolu_1"
    assert tool_use.tool_name == "get_weather"
    assert tool_use.tool_input == {"city": "SF"}

    tool_result_msg = internal.messages[2]
    assert any(isinstance(b, ToolResultBlock) for b in tool_result_msg.content)
    tool_result = next(b for b in tool_result_msg.content if isinstance(b, ToolResultBlock))
    assert tool_result.tool_use_id == "toolu_1"
    assert tool_result.output == {"temp_c": 20}
    assert tool_result.is_error is False

    out = n.request_from_internal(internal)
    out_messages: list[dict[str, Any]] = out["messages"]
    assert [m["role"] for m in out_messages] == ["user", "assistant", "user"]

    assistant_out = out_messages[1]
    assert isinstance(assistant_out["content"], list)
    a_blocks = cast(list[dict[str, Any]], assistant_out["content"])
    assert a_blocks[0]["type"] == "tool_use"
    assert a_blocks[0]["id"] == "toolu_1"
    assert a_blocks[0]["name"] == "get_weather"

    user_out = out_messages[2]
    assert isinstance(user_out["content"], list)
    u_blocks = cast(list[dict[str, Any]], user_out["content"])
    assert u_blocks[0]["type"] == "tool_result"
    assert u_blocks[0]["tool_use_id"] == "toolu_1"
    assert u_blocks[0]["content"] == {"temp_c": 20}


def test_claude_unknown_block_drop_on_output() -> None:
    n = ClaudeNormalizer()

    req = {
        "model": "claude-3-sonnet",
        "messages": [
            {
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "ok"},
                    {"type": "thinking", "text": "secret"},
                ],
            }
        ],
        "max_tokens": 10,
    }

    internal = n.request_to_internal(req)
    assert len(internal.messages) == 1
    blocks = internal.messages[0].content
    assert any(isinstance(b, TextBlock) for b in blocks)
    assert any(isinstance(b, UnknownBlock) for b in blocks)
    u = next(b for b in blocks if isinstance(b, UnknownBlock))
    assert u.raw_type == "thinking"

    out = n.request_from_internal(internal)
    # Claude 要求以 user 开头，且会做最小修复：插入空 user
    out_messages: list[dict[str, Any]] = out["messages"]
    assert out_messages[0]["role"] == "user"
    assert out_messages[1]["role"] == "assistant"
    assert out_messages[1]["content"] == "ok"


def test_claude_response_stop_reason_and_usage_roundtrip() -> None:
    n = ClaudeNormalizer()

    resp = {
        "id": "msg_1",
        "type": "message",
        "role": "assistant",
        "model": "claude-3-sonnet",
        "content": [{"type": "text", "text": "hi"}],
        "stop_reason": "tool_use",
        "usage": {
            "input_tokens": 5,
            "output_tokens": 7,
            "cache_read_input_tokens": 1,
            "cache_creation_input_tokens": 2,
        },
    }

    internal = n.response_to_internal(resp)
    assert internal.id == "msg_1"
    assert internal.stop_reason == StopReason.TOOL_USE
    assert internal.usage is not None
    assert internal.usage.input_tokens == 5
    assert internal.usage.output_tokens == 7
    assert internal.usage.total_tokens == 12
    assert internal.usage.cache_read_tokens == 1
    assert internal.usage.cache_write_tokens == 2

    out = n.response_from_internal(internal)
    assert out["id"] == "msg_1"
    assert out["stop_reason"] == "tool_use"
    assert out["usage"]["cache_read_input_tokens"] == 1
    assert out["usage"]["cache_creation_input_tokens"] == 2


def test_claude_stream_chunk_and_event_roundtrip_basic() -> None:
    n = ClaudeNormalizer()
    state = StreamState()

    chunks: list[dict[str, Any]] = [
        {
            "type": "message_start",
            "message": {
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "model": "claude-3-sonnet",
                "content": [],
                "stop_reason": None,
                "stop_sequence": None,
                "usage": {"input_tokens": 1, "output_tokens": 0},
            },
        },
        {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}},
        {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Hel"}},
        {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "lo"}},
        {
            "type": "content_block_start",
            "index": 1,
            "content_block": {"type": "tool_use", "id": "toolu_1", "name": "get_weather"},
        },
        {
            "type": "content_block_delta",
            "index": 1,
            "delta": {"type": "input_json_delta", "partial_json": '{"city":"SF"}'},
        },
        {"type": "content_block_stop", "index": 0},
        {"type": "content_block_stop", "index": 1},
        {
            "type": "message_delta",
            "delta": {"stop_reason": "end_turn"},
            "usage": {"input_tokens": 1, "output_tokens": 2},
        },
        {"type": "message_stop"},
    ]

    events: list[Any] = []
    for ch in chunks:
        events.extend(n.stream_chunk_to_internal(ch, state))

    assert any(isinstance(e, MessageStartEvent) for e in events)
    assert [e.text_delta for e in events if isinstance(e, ContentDeltaEvent)] == ["Hel", "lo"]
    assert any(isinstance(e, ToolCallDeltaEvent) and e.tool_id == "toolu_1" for e in events)
    assert any(
        isinstance(e, MessageStopEvent) and e.stop_reason == StopReason.END_TURN for e in events
    )

    # internal events -> Claude events
    state2 = StreamState()
    out_events: list[dict[str, Any]] = []
    for e in events:
        out_events.extend(n.stream_event_from_internal(e, state2))

    assert out_events[0]["type"] == "message_start"
    assert out_events[0]["message"]["id"] == "msg_1"

    assert any(
        ev.get("type") == "content_block_delta"
        and ev.get("delta", {}).get("type") == "input_json_delta"
        for ev in out_events
    )
    assert out_events[-1]["type"] == "message_stop"


def test_claude_stream_message_start_preserves_usage() -> None:
    n = ClaudeNormalizer()
    state = StreamState(model="claude-3-sonnet")

    chunk = {
        "type": "message_start",
        "message": {
            "id": "msg_usage_start",
            "type": "message",
            "role": "assistant",
            "model": "claude-3-sonnet",
            "content": [],
            "usage": {
                "input_tokens": 9,
                "output_tokens": 0,
                "cache_read_input_tokens": 3,
            },
        },
    }

    events = n.stream_chunk_to_internal(chunk, state)
    assert len(events) == 1
    start_event = events[0]
    assert isinstance(start_event, MessageStartEvent)
    assert start_event.usage is not None
    assert start_event.usage.input_tokens == 9
    assert start_event.usage.cache_read_tokens == 3

    out = n.stream_event_from_internal(start_event, StreamState(model="claude-3-sonnet"))
    assert out[0]["type"] == "message_start"
    assert out[0]["message"]["usage"]["input_tokens"] == 9
    assert out[0]["message"]["usage"]["cache_read_input_tokens"] == 3


def test_claude_error_conversion() -> None:
    n = ClaudeNormalizer()
    err_resp = {
        "type": "error",
        "error": {"type": "rate_limit_error", "message": "slow down"},
    }

    assert n.is_error_response(err_resp) is True
    internal = n.error_to_internal(err_resp)
    assert internal.type == ErrorType.RATE_LIMIT
    assert internal.retryable is True

    out = n.error_from_internal(internal)
    assert out["type"] == "error"
    assert out["error"]["type"] == "rate_limit_error"


def test_claude_request_metadata_preserved() -> None:
    """测试 Claude 请求中 metadata 字段的保留"""
    n = ClaudeNormalizer()

    req = {
        "model": "claude-haiku-4-5-20251001",
        "messages": [{"role": "user", "content": [{"type": "text", "text": "hi"}]}],
        "system": [
            {"type": "text", "text": "System prompt 1"},
            {"type": "text", "text": "System prompt 2"},
        ],
        "metadata": {"user_id": "user_abc123_session_xyz456"},
        "max_tokens": 32000,
        "stream": True,
    }

    internal = n.request_to_internal(req)

    # system 数组应该被正确合并
    assert internal.system == "System prompt 1\n\nSystem prompt 2"

    # metadata 应该在 extra 中
    assert "claude" in internal.extra
    assert "metadata" in internal.extra["claude"]
    assert internal.extra["claude"]["metadata"]["user_id"] == "user_abc123_session_xyz456"

    # 往返转换后 metadata 应该被恢复
    out = n.request_from_internal(internal)
    assert "metadata" in out
    assert out["metadata"]["user_id"] == "user_abc123_session_xyz456"


def test_claude_system_array_format() -> None:
    """测试 Claude CLI 风格的 system 数组格式"""
    n = ClaudeNormalizer()

    req = {
        "model": "claude-3-opus",
        "messages": [{"role": "user", "content": "hello"}],
        "system": [
            {"type": "text", "text": "x-anthropic-billing-header: cc_version=2.1.19"},
            {"type": "text", "text": "You are Claude Code."},
            {"type": "text", "text": "Extract file paths."},
        ],
        "max_tokens": 4096,
    }

    internal = n.request_to_internal(req)

    # system 数组中的多个 text 应该用 \n\n 连接
    assert internal.system is not None
    assert "x-anthropic-billing-header" in internal.system
    assert "You are Claude Code" in internal.system
    assert "Extract file paths" in internal.system
    assert "\n\n" in internal.system
