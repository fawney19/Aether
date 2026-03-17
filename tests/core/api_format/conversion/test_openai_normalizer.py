"""
OpenAINormalizer 单元测试

覆盖重点：
- system/developer -> instructions 的提取与还原
- tool_calls 与 tool role 的往返转换
- content parts（text/image/unknown）：UnknownBlock 内部保留、输出默认丢弃
- finish_reason/usage 的映射
- streaming chunk <-> InternalStreamEvent 的基础行为与状态
"""

from __future__ import annotations

import json
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
from src.core.api_format.conversion.normalizers.openai import OpenAINormalizer
from src.core.api_format.conversion.stream_events import (
    ContentBlockStartEvent,
    ContentDeltaEvent,
    MessageStartEvent,
    MessageStopEvent,
    ToolCallDeltaEvent,
)
from src.core.api_format.conversion.stream_state import StreamState


def _first_choice_message(response: dict[str, Any]) -> dict[str, Any]:
    choices = response.get("choices") or []
    assert isinstance(choices, list) and choices
    c0 = choices[0]
    assert isinstance(c0, dict)
    msg = c0.get("message")
    assert isinstance(msg, dict)
    return cast(dict[str, Any], msg)


def test_openai_request_instructions_roundtrip() -> None:
    n = OpenAINormalizer()

    req = {
        "model": "gpt-4o-mini",
        "messages": [
            {"role": "system", "content": "sys"},
            {"role": "developer", "content": [{"type": "text", "text": "dev"}]},
            {"role": "user", "content": "hi"},
            {"role": "assistant", "content": "ok"},
        ],
        "max_tokens": 12,
        "temperature": 0.2,
        "stop": ["A", "B"],
        "stream": True,
    }

    internal = n.request_to_internal(req)
    assert internal.model == "gpt-4o-mini"
    assert [seg.role.value for seg in internal.instructions] == ["system", "developer"]
    assert [seg.text for seg in internal.instructions] == ["sys", "dev"]
    assert internal.system == "sys\n\ndev"
    assert [m.role.value for m in internal.messages] == ["user", "assistant"]
    assert internal.stop_sequences == ["A", "B"]
    assert internal.stream is True

    out = n.request_from_internal(internal)
    assert out["model"] == "gpt-4o-mini"
    out_messages = out["messages"]
    assert [m["role"] for m in out_messages[:2]] == ["system", "developer"]
    assert [m["content"] for m in out_messages[:2]] == ["sys", "dev"]
    assert [m["role"] for m in out_messages[2:]] == ["user", "assistant"]
    assert out_messages[2]["content"] == "hi"
    assert out_messages[3]["content"] == "ok"


def test_openai_request_max_completion_tokens_support() -> None:
    """测试 max_completion_tokens 参数的兼容性（OpenAI API 新参数名）"""
    n = OpenAINormalizer()

    # 测试 max_completion_tokens 优先于 max_tokens
    req_new = {
        "model": "gpt-4o",
        "messages": [{"role": "user", "content": "hi"}],
        "max_completion_tokens": 100,
        "max_tokens": 50,  # 旧参数应被忽略
    }
    internal = n.request_to_internal(req_new)
    assert internal.max_tokens == 100

    # 测试仅使用 max_completion_tokens
    req_only_new = {
        "model": "gpt-4o",
        "messages": [{"role": "user", "content": "hi"}],
        "max_completion_tokens": 200,
    }
    internal = n.request_to_internal(req_only_new)
    assert internal.max_tokens == 200

    # 测试仅使用 max_tokens（向后兼容）
    req_only_old = {
        "model": "gpt-4o",
        "messages": [{"role": "user", "content": "hi"}],
        "max_tokens": 150,
    }
    internal = n.request_to_internal(req_only_old)
    assert internal.max_tokens == 150


def test_openai_request_tool_calls_and_tool_role_roundtrip() -> None:
    n = OpenAINormalizer()

    req = {
        "model": "gpt-4o-mini",
        "messages": [
            {"role": "user", "content": "weather?"},
            {
                "role": "assistant",
                "content": "",
                "tool_calls": [
                    {
                        "id": "call_1",
                        "type": "function",
                        "function": {"name": "get_weather", "arguments": '{"city":"SF"}'},
                    }
                ],
            },
            {
                "role": "tool",
                "tool_call_id": "call_1",
                "content": '{"temp_c": 20, "unit": "C"}',
            },
            {"role": "assistant", "content": "done"},
        ],
        "tools": [
            {
                "type": "function",
                "function": {
                    "name": "get_weather",
                    "description": "Get weather",
                    "parameters": {"type": "object", "properties": {"city": {"type": "string"}}},
                },
            }
        ],
        "tool_choice": "auto",
    }

    internal = n.request_to_internal(req)
    assert [m.role.value for m in internal.messages] == ["user", "assistant", "user", "assistant"]

    assistant_msg = internal.messages[1]
    assert any(isinstance(b, ToolUseBlock) for b in assistant_msg.content)
    tool_use = next(b for b in assistant_msg.content if isinstance(b, ToolUseBlock))
    assert tool_use.tool_id == "call_1"
    assert tool_use.tool_name == "get_weather"
    assert tool_use.tool_input == {"city": "SF"}

    tool_result_msg = internal.messages[2]
    assert any(isinstance(b, ToolResultBlock) for b in tool_result_msg.content)
    tool_result = next(b for b in tool_result_msg.content if isinstance(b, ToolResultBlock))
    assert tool_result.tool_use_id == "call_1"
    assert tool_result.output == {"temp_c": 20, "unit": "C"}

    out = n.request_from_internal(internal)
    out_messages: list[dict[str, Any]] = out["messages"]

    roles = [m.get("role") for m in out_messages]
    assert roles == ["user", "assistant", "tool", "assistant"]

    assistant_out = out_messages[1]
    assert "tool_calls" in assistant_out
    assert assistant_out["tool_calls"][0]["function"]["name"] == "get_weather"

    tool_out = out_messages[2]
    assert tool_out["role"] == "tool"
    assert tool_out["tool_call_id"] == "call_1"
    assert json.loads(tool_out["content"]) == {"temp_c": 20, "unit": "C"}


def test_openai_request_preserves_empty_string_tool_call_arguments() -> None:
    n = OpenAINormalizer()

    req = {
        "model": "gpt-4o-mini",
        "messages": [
            {"role": "user", "content": "ping"},
            {
                "role": "assistant",
                "content": "",
                "tool_calls": [
                    {
                        "id": "call_empty",
                        "type": "function",
                        "function": {"name": "noop", "arguments": ""},
                    }
                ],
            },
        ],
    }

    internal = n.request_to_internal(req)
    tool_use = next(b for b in internal.messages[1].content if isinstance(b, ToolUseBlock))
    assert tool_use.tool_input == {}
    assert tool_use.extra["raw"]["arguments"] == ""

    out = n.request_from_internal(internal)
    assert out["messages"][1]["tool_calls"][0]["function"]["arguments"] == ""


def test_openai_request_preserves_empty_string_legacy_function_call_arguments() -> None:
    n = OpenAINormalizer()

    req = {
        "model": "gpt-4o-mini",
        "messages": [
            {"role": "user", "content": "ping"},
            {
                "role": "assistant",
                "content": "",
                "function_call": {"name": "noop", "arguments": ""},
            },
        ],
    }

    internal = n.request_to_internal(req)
    tool_use = next(b for b in internal.messages[1].content if isinstance(b, ToolUseBlock))
    assert tool_use.tool_input == {}
    assert tool_use.extra["raw"]["arguments"] == ""

    out = n.request_from_internal(internal)
    assert out["messages"][1]["tool_calls"][0]["function"]["arguments"] == ""


def test_openai_request_content_image_and_unknown_drop() -> None:
    n = OpenAINormalizer()

    req = {
        "model": "gpt-4o-mini",
        "messages": [
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": "look"},
                    {"type": "image_url", "image_url": {"url": "https://example.com/a.png"}},
                    {"type": "foo", "x": 1},
                ],
            }
        ],
    }

    internal = n.request_to_internal(req)
    assert len(internal.messages) == 1
    blocks = internal.messages[0].content
    assert any(isinstance(b, TextBlock) for b in blocks)
    assert any(isinstance(b, ImageBlock) for b in blocks)
    assert any(isinstance(b, UnknownBlock) for b in blocks)
    u = next(b for b in blocks if isinstance(b, UnknownBlock))
    assert u.raw_type == "foo"

    dropped = (internal.extra.get("raw") or {}).get("dropped_blocks") or {}
    assert dropped.get("openai_part:foo") == 1

    out = n.request_from_internal(internal)
    out_msg = out["messages"][0]
    assert out_msg["role"] == "user"
    assert isinstance(out_msg["content"], list)
    parts = cast(list[dict[str, Any]], out_msg["content"])
    assert any(p.get("type") == "image_url" for p in parts)
    assert all(p.get("type") != "foo" for p in parts)


def test_openai_response_finish_reason_and_usage_roundtrip() -> None:
    n = OpenAINormalizer()

    resp = {
        "id": "chatcmpl_1",
        "object": "chat.completion",
        "created": 1,
        "model": "gpt-4o-mini",
        "choices": [
            {
                "index": 0,
                "message": {"role": "assistant", "content": "hi"},
                "finish_reason": "length",
            }
        ],
        "usage": {"prompt_tokens": 5, "completion_tokens": 7, "total_tokens": 12},
    }

    internal = n.response_to_internal(resp)
    assert internal.id == "chatcmpl_1"
    assert internal.stop_reason == StopReason.MAX_TOKENS
    assert internal.usage is not None
    assert internal.usage.input_tokens == 5
    assert internal.usage.output_tokens == 7
    assert internal.usage.total_tokens == 12

    out = n.response_from_internal(internal)
    out_msg = _first_choice_message(out)
    assert out_msg["role"] == "assistant"
    assert out_msg["content"] == "hi"
    assert out["choices"][0]["finish_reason"] == "length"
    assert out["usage"] == {"prompt_tokens": 5, "completion_tokens": 7, "total_tokens": 12}


def test_openai_stream_chunk_and_event_roundtrip_basic() -> None:
    n = OpenAINormalizer()
    state = StreamState()

    chunks: list[dict[str, Any]] = [
        {
            "id": "chatcmpl_stream_1",
            "object": "chat.completion.chunk",
            "model": "gpt-4o-mini",
            "choices": [{"index": 0, "delta": {"content": "Hel"}, "finish_reason": None}],
        },
        {"choices": [{"index": 0, "delta": {"content": "lo"}, "finish_reason": None}]},
        {
            "choices": [
                {
                    "index": 0,
                    "delta": {
                        "tool_calls": [
                            {
                                "index": 0,
                                "id": "call_1",
                                "type": "function",
                                "function": {"name": "get_weather", "arguments": '{"city":"SF"}'},
                            }
                        ]
                    },
                    "finish_reason": None,
                }
            ]
        },
        {"choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]},
    ]

    events: list[Any] = []
    for ch in chunks:
        events.extend(n.stream_chunk_to_internal(ch, state))

    assert any(isinstance(e, MessageStartEvent) for e in events)
    assert any(isinstance(e, ContentBlockStartEvent) for e in events)
    assert [e.text_delta for e in events if isinstance(e, ContentDeltaEvent)] == ["Hel", "lo"]
    assert any(isinstance(e, ToolCallDeltaEvent) for e in events)
    assert any(
        isinstance(e, MessageStopEvent) and e.stop_reason == StopReason.TOOL_USE for e in events
    )

    # internal events -> OpenAI chunks（验证关键字段与 tool_calls index 稳定）
    state2 = StreamState()
    out_chunks: list[dict[str, Any]] = []
    for e in events:
        out_chunks.extend(n.stream_event_from_internal(e, state2))

    # 第一个 chunk 应包含 role=assistant
    assert out_chunks[0]["choices"][0]["delta"].get("role") == "assistant"

    # 至少包含一个 content delta
    assert any(c["choices"][0]["delta"].get("content") == "Hel" for c in out_chunks)

    # tool_calls start chunk
    tool_start = next(
        c
        for c in out_chunks
        if c["choices"][0]["delta"].get("tool_calls")
        and c["choices"][0]["delta"]["tool_calls"][0]["function"].get("name")
    )
    assert tool_start["choices"][0]["delta"]["tool_calls"][0]["id"] == "call_1"
    assert tool_start["choices"][0]["delta"]["tool_calls"][0]["index"] == 0

    # tool_calls delta chunk（arguments 片段）
    tool_delta = next(
        c
        for c in out_chunks
        if c["choices"][0]["delta"].get("tool_calls")
        and "arguments" in c["choices"][0]["delta"]["tool_calls"][0]["function"]
    )
    assert tool_delta["choices"][0]["delta"]["tool_calls"][0]["id"] == "call_1"
    assert tool_delta["choices"][0]["delta"]["tool_calls"][0]["index"] == 0
    assert tool_delta["choices"][0]["delta"]["tool_calls"][0]["function"]["name"] == "get_weather"

    # 最终 stop chunk finish_reason=tool_calls
    assert out_chunks[-1]["choices"][0]["finish_reason"] == "tool_calls"


def test_openai_error_conversion() -> None:
    n = OpenAINormalizer()
    err_resp = {
        "error": {
            "message": "bad request",
            "type": "invalid_request_error",
            "code": "bad_request",
            "param": "messages",
        }
    }

    internal = n.error_to_internal(err_resp)
    assert internal.type == ErrorType.INVALID_REQUEST
    assert internal.message == "bad request"
    assert internal.retryable is False

    out = n.error_from_internal(internal)
    assert out["error"]["type"] == "invalid_request_error"
    assert out["error"]["message"] == "bad request"
