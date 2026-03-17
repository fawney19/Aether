"""
GeminiNormalizer 单元测试

覆盖重点：
- systemInstruction/system_instruction -> instructions 的提取与还原
- parts（text/inline_data/function_call/function_response/unknown）转换
- finishReason/usageMetadata 映射
- streaming chunk <-> InternalStreamEvent 的基础行为与状态
- error <-> InternalError
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
from src.core.api_format.conversion.normalizers.gemini import (
    GeminiNormalizer,
    compact_gemini_contents,
)
from src.core.api_format.conversion.stream_events import (
    ContentDeltaEvent,
    MessageStartEvent,
    MessageStopEvent,
    ToolCallDeltaEvent,
)
from src.core.api_format.conversion.stream_state import StreamState
from src.core.api_format.schema_utils import clean_gemini_schema


def test_gemini_request_system_and_generation_config_roundtrip() -> None:
    n = GeminiNormalizer()

    req = {
        "model": "gemini-1.5",
        "systemInstruction": {"parts": [{"text": "sys"}]},
        "contents": [
            {"role": "user", "parts": [{"text": "hi"}]},
            {"role": "model", "parts": [{"text": "ok"}]},
        ],
        "generationConfig": {
            "maxOutputTokens": 10,
            "temperature": 0.2,
            "topP": 0.9,
            "topK": 1,
            "stopSequences": ["A", "B"],
        },
        "tools": [
            {
                "functionDeclarations": [
                    {
                        "name": "get_weather",
                        "description": "Get weather",
                        "parameters": {
                            "type": "object",
                            "properties": {"city": {"type": "string"}},
                        },
                    }
                ]
            }
        ],
        "toolConfig": {"functionCallingConfig": {"mode": "ANY"}},
    }

    internal = n.request_to_internal(req)
    assert internal.model == "gemini-1.5"
    assert [seg.role.value for seg in internal.instructions] == ["system"]
    assert internal.instructions[0].text == "sys"
    assert internal.system == "sys"
    assert internal.max_tokens == 10
    assert internal.temperature == 0.2
    assert internal.top_p == 0.9
    assert internal.top_k == 1
    assert internal.stop_sequences == ["A", "B"]
    assert internal.tools is not None and internal.tools[0].name == "get_weather"
    assert internal.tool_choice is not None and internal.tool_choice.type.value == "required"

    out = n.request_from_internal(internal)
    assert out["system_instruction"]["parts"][0]["text"] == "sys"
    assert out["generation_config"]["max_output_tokens"] == 10
    assert out["generation_config"]["stop_sequences"] == ["A", "B"]
    assert out["tools"][0]["function_declarations"][0]["name"] == "get_weather"
    assert out["tool_config"] == {"functionCallingConfig": {"mode": "ANY"}}


def test_gemini_request_parts_image_tool_and_unknown_drop() -> None:
    n = GeminiNormalizer()

    req = {
        "contents": [
            {
                "role": "user",
                "parts": [
                    {"text": "look"},
                    {"inline_data": {"mime_type": "image/png", "data": "AAAA"}},
                    {"foo": 1},
                ],
            },
            {
                "role": "model",
                "parts": [{"function_call": {"name": "get_weather", "args": {"city": "SF"}}}],
            },
            {
                "role": "user",
                "parts": [
                    {
                        "function_response": {
                            "name": "call_1",
                            "response": {"result": {"temp_c": 20}},
                        }
                    }
                ],
            },
        ]
    }

    internal = n.request_to_internal(req)
    assert [m.role.value for m in internal.messages] == ["user", "assistant", "user"]

    blocks0 = internal.messages[0].content
    assert any(isinstance(b, TextBlock) for b in blocks0)
    assert any(isinstance(b, ImageBlock) for b in blocks0)
    assert any(isinstance(b, UnknownBlock) for b in blocks0)

    dropped = (internal.extra.get("raw") or {}).get("dropped_blocks") or {}
    assert dropped.get("gemini_part:foo") == 1

    blocks1 = internal.messages[1].content
    tool_use = next(b for b in blocks1 if isinstance(b, ToolUseBlock))
    assert tool_use.tool_name == "get_weather"
    assert tool_use.tool_input == {"city": "SF"}

    blocks2 = internal.messages[2].content
    tool_result = next(b for b in blocks2 if isinstance(b, ToolResultBlock))
    assert tool_result.tool_use_id == "call_1"
    assert tool_result.output == {"temp_c": 20}

    out = n.request_from_internal(internal)
    out_contents: list[dict[str, Any]] = out["contents"]

    # unknown 被丢弃
    user_parts = cast(list[dict[str, Any]], out_contents[0]["parts"])
    assert any(p.get("text") == "look" for p in user_parts)
    assert any(p.get("inline_data", {}).get("mime_type") == "image/png" for p in user_parts)
    assert all("foo" not in p for p in user_parts)

    model_parts = cast(list[dict[str, Any]], out_contents[1]["parts"])
    assert model_parts[0]["function_call"]["name"] == "get_weather"

    tool_parts = cast(list[dict[str, Any]], out_contents[2]["parts"])
    assert tool_parts[0]["function_response"]["name"] == "call_1"
    assert tool_parts[0]["function_response"]["response"]["result"] == {"temp_c": 20}


def test_gemini_response_finish_reason_and_usage_roundtrip() -> None:
    n = GeminiNormalizer()

    resp = {
        "candidates": [
            {
                "content": {"parts": [{"text": "hi"}], "role": "model"},
                "finishReason": "MAX_TOKENS",
                "index": 0,
            }
        ],
        "usageMetadata": {
            "promptTokenCount": 5,
            "candidatesTokenCount": 7,
            "totalTokenCount": 12,
            "cachedContentTokenCount": 2,
        },
        "modelVersion": "gemini-1.5",
    }

    internal = n.response_to_internal(resp)
    assert internal.model == "gemini-1.5"
    assert internal.stop_reason == StopReason.MAX_TOKENS
    assert internal.usage is not None
    assert internal.usage.input_tokens == 5
    assert internal.usage.output_tokens == 7
    assert internal.usage.total_tokens == 12
    assert internal.usage.cache_read_tokens == 2

    out = n.response_from_internal(internal)
    assert out["candidates"][0]["finishReason"] == "MAX_TOKENS"
    assert out["usageMetadata"]["cachedContentTokenCount"] == 2


def test_gemini_stream_chunk_and_event_roundtrip_basic() -> None:
    n = GeminiNormalizer()
    state = StreamState()

    chunks: list[dict[str, Any]] = [
        {
            "candidates": [{"content": {"parts": [{"text": "Hel"}], "role": "model"}, "index": 0}],
            "modelVersion": "gemini-1.5",
        },
        {"candidates": [{"content": {"parts": [{"text": "lo"}], "role": "model"}, "index": 0}]},
        {
            "candidates": [
                {
                    "content": {
                        "parts": [
                            {"functionCall": {"name": "get_weather", "args": {"city": "SF"}}}
                        ],
                        "role": "model",
                    },
                    "index": 0,
                }
            ],
            "modelVersion": "gemini-1.5",
        },
        {
            "candidates": [
                {
                    "content": {"parts": [], "role": "model"},
                    "finishReason": "STOP",
                    "index": 0,
                }
            ],
            "usageMetadata": {
                "promptTokenCount": 1,
                "candidatesTokenCount": 2,
                "totalTokenCount": 3,
            },
            "modelVersion": "gemini-1.5",
        },
    ]

    events: list[Any] = []
    for ch in chunks:
        events.extend(n.stream_chunk_to_internal(ch, state))

    assert any(isinstance(e, MessageStartEvent) for e in events)
    assert [e.text_delta for e in events if isinstance(e, ContentDeltaEvent)] == ["Hel", "lo"]
    assert any(
        isinstance(e, ToolCallDeltaEvent) and json.loads(e.input_delta) == {"city": "SF"}
        for e in events
    )
    assert any(
        isinstance(e, MessageStopEvent) and e.stop_reason == StopReason.END_TURN for e in events
    )

    state2 = StreamState()
    out_chunks: list[dict[str, Any]] = []
    for e in events:
        out_chunks.extend(n.stream_event_from_internal(e, state2))

    assert any(c["candidates"][0]["content"]["parts"][0].get("text") == "Hel" for c in out_chunks)

    tool_chunk = next(
        c
        for c in out_chunks
        if c["candidates"][0]["content"]["parts"]
        and "functionCall" in c["candidates"][0]["content"]["parts"][0]
    )
    assert (
        tool_chunk["candidates"][0]["content"]["parts"][0]["functionCall"]["name"] == "get_weather"
    )

    assert out_chunks[-1]["candidates"][0]["finishReason"] == "STOP"


def test_gemini_error_conversion() -> None:
    n = GeminiNormalizer()

    err_resp = {"error": {"code": 429, "message": "slow down", "status": "RESOURCE_EXHAUSTED"}}
    assert n.is_error_response(err_resp) is True

    internal = n.error_to_internal(err_resp)
    assert internal.type == ErrorType.RATE_LIMIT
    assert internal.retryable is True

    out = n.error_from_internal(internal)
    assert out["error"]["status"] == "RESOURCE_EXHAUSTED"
    assert out["error"]["message"] == "slow down"


def test_gemini_compact_contents_drops_invalid_and_merges_same_role() -> None:
    contents: list[dict[str, Any]] = [
        {"role": "user", "parts": [{"text": "hi"}, {"invalid": True}]},
        {"role": "user", "parts": [{"text": "again"}]},
        {"role": "model", "parts": [{"bad": 1}]},
        {"role": "model", "parts": [{"text": "ok"}]},
        {"role": "user", "parts": "bad"},
    ]

    compacted = compact_gemini_contents(contents)
    assert compacted == [
        {"role": "user", "parts": [{"text": "hi"}, {"text": "again"}]},
        {"role": "model", "parts": [{"text": "ok"}]},
    ]


def test_gemini_request_reuses_raw_tool_config_and_cleans_schema_deterministically() -> None:
    n = GeminiNormalizer()
    req = {
        "model": "gemini-1.5",
        "contents": [{"role": "user", "parts": [{"text": "hi"}]}],
        "tools": [
            {
                "functionDeclarations": [
                    {
                        "name": "read_file",
                        "description": "Read file",
                        "parameters": {
                            "allOf": [
                                {
                                    "type": "object",
                                    "properties": {"b": {"type": "string"}},
                                    "required": ["b", "a"],
                                },
                                {
                                    "type": "object",
                                    "properties": {"a": {"type": "string"}},
                                    "required": ["a", "c"],
                                },
                            ]
                        },
                    }
                ]
            }
        ],
        "toolConfig": {
            "functionCallingConfig": {
                "mode": "ANY",
                "allowedFunctionNames": ["read_file"],
            }
        },
    }

    internal = n.request_to_internal(req)
    out = n.request_from_internal(internal)

    assert out["tool_config"] == {
        "functionCallingConfig": {
            "mode": "ANY",
            "allowedFunctionNames": ["read_file"],
        }
    }
    params = out["tools"][0]["function_declarations"][0]["parameters"]
    assert params["required"] == ["b", "a"]
    assert list(params["properties"].keys()) == ["b", "a"]


def test_clean_gemini_schema_allof_required_order_is_deterministic() -> None:
    schema = {
        "allOf": [
            {"type": "object", "properties": {"z": {"type": "string"}}, "required": ["z", "a"]},
            {"type": "object", "properties": {"a": {"type": "string"}}, "required": ["a", "m"]},
        ]
    }
    clean_gemini_schema(schema)
    assert schema["required"] == ["z", "a"]
