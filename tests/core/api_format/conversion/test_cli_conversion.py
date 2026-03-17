"""
CLI 格式参与转换的单元测试

覆盖：
- OPENAI_CLI（Responses）与其他格式的 request/response/stream 基础互转
- CLAUDE_CLI / GEMINI_CLI 的 registry 注册与互转能力
"""

from __future__ import annotations

from typing import Any, cast

from src.core.api_format.conversion.normalizers.claude import ClaudeNormalizer
from src.core.api_format.conversion.normalizers.gemini import GeminiNormalizer
from src.core.api_format.conversion.normalizers.openai import OpenAINormalizer
from src.core.api_format.conversion.normalizers.openai_cli import OpenAICliNormalizer
from src.core.api_format.conversion.registry import FormatConversionRegistry
from src.core.api_format.conversion.stream_events import UnknownStreamEvent
from src.core.api_format.conversion.stream_state import StreamState


def _make_registry_with_cli() -> FormatConversionRegistry:
    reg = FormatConversionRegistry()
    reg.register(OpenAINormalizer())
    reg.register(OpenAICliNormalizer())
    reg.register(ClaudeNormalizer())
    reg.register(GeminiNormalizer())
    # claude:cli / gemini:cli 通过 data_format_id 回退自动复用对应的 Chat normalizer
    return reg


def test_registry_can_convert_full_with_cli_stream() -> None:
    reg = _make_registry_with_cli()
    assert reg.can_convert_full("openai:cli", "openai:chat", require_stream=True) is True
    assert reg.can_convert_full("openai:cli", "claude:cli", require_stream=True) is True
    assert reg.can_convert_full("gemini:cli", "claude:chat", require_stream=True) is True


def test_openai_cli_request_to_claude() -> None:
    reg = _make_registry_with_cli()

    openai_cli_req = {
        "model": "gpt-4o-mini",
        "input": [{"role": "user", "content": [{"type": "input_text", "text": "hi"}]}],
        "stream": True,
        "max_output_tokens": 12,
    }

    claude_req = reg.convert_request(openai_cli_req, "openai:cli", "claude:chat")
    assert claude_req["model"] == "gpt-4o-mini"
    assert claude_req["stream"] is True
    assert isinstance(claude_req.get("messages"), list)
    assert claude_req["messages"][0]["role"] == "user"
    assert claude_req["messages"][0]["content"] == "hi"


def test_claude_request_to_openai_cli_places_instructions_before_input() -> None:
    reg = _make_registry_with_cli()

    claude_req = {
        "model": "claude-3-5-sonnet-latest",
        "system": "You are precise.",
        "messages": [{"role": "user", "content": "hi"}],
        "max_tokens": 12,
    }

    openai_cli_req = reg.convert_request(claude_req, "claude:chat", "openai:cli")

    assert openai_cli_req["instructions"] == "You are precise."
    assert list(openai_cli_req.keys())[:3] == ["model", "instructions", "input"]
    assert openai_cli_req["input"][0]["role"] == "user"


def test_claude_request_to_openai_cli_drops_anthropic_billing_header() -> None:
    reg = _make_registry_with_cli()

    claude_req = {
        "model": "claude-3-5-sonnet-latest",
        "system": [
            {
                "type": "text",
                "text": "x-anthropic-billing-header: cc_version=2.1.74.705; cc_entrypoint=claude-vscode; cch=4fb9f;",
            },
            {"type": "text", "text": "You are Claude Code."},
            {"type": "text", "text": "Keep answers terse."},
        ],
        "messages": [{"role": "user", "content": "hi"}],
        "max_tokens": 12,
    }

    openai_cli_req = reg.convert_request(claude_req, "claude:chat", "openai:cli")

    assert openai_cli_req["instructions"] == "You are Claude Code.\n\nKeep answers terse."
    assert "x-anthropic-billing-header" not in openai_cli_req["instructions"]


def test_claude_request_to_openai_cli_keeps_prompt_after_billing_header_prefix() -> None:
    reg = _make_registry_with_cli()

    claude_req = {
        "model": "claude-3-5-sonnet-latest",
        "system": [
            {
                "type": "text",
                "text": "x-anthropic-billing-header: cc_version=2.1.74.705\n\nYou are Claude Code.",
            },
            {"type": "text", "text": "Keep answers terse."},
        ],
        "messages": [{"role": "user", "content": "hi"}],
        "max_tokens": 12,
    }

    openai_cli_req = reg.convert_request(claude_req, "claude:chat", "openai:cli")

    assert openai_cli_req["instructions"] == "You are Claude Code.\n\nKeep answers terse."
    assert "x-anthropic-billing-header" not in openai_cli_req["instructions"]


def test_claude_request_to_openai_cli_strips_billing_header_from_string_system() -> None:
    reg = _make_registry_with_cli()

    claude_req = {
        "model": "claude-3-5-sonnet-latest",
        "system": "x-anthropic-billing-header: cc_version=2.1.74.705\n\nYou are Claude Code.\nKeep answers terse.",
        "messages": [{"role": "user", "content": "hi"}],
        "max_tokens": 12,
    }

    openai_cli_req = reg.convert_request(claude_req, "claude:chat", "openai:cli")

    assert openai_cli_req["instructions"] == "You are Claude Code.\nKeep answers terse."
    assert "x-anthropic-billing-header" not in openai_cli_req["instructions"]


def test_claude_response_to_openai_cli() -> None:
    reg = _make_registry_with_cli()

    claude_resp = {
        "id": "msg_1",
        "type": "message",
        "role": "assistant",
        "model": "claude-3-5-sonnet-latest",
        "content": [{"type": "text", "text": "hello"}],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 5, "output_tokens": 7},
    }

    openai_cli_resp = reg.convert_response(claude_resp, "claude:chat", "openai:cli")
    assert openai_cli_resp["object"] == "response"
    assert isinstance(openai_cli_resp.get("output"), list)
    msg = cast(dict[str, Any], openai_cli_resp["output"][0])
    assert msg["type"] == "message"
    assert msg["role"] == "assistant"
    content = cast(list[dict[str, Any]], msg.get("content") or [])
    assert content and content[0]["type"] == "output_text"
    assert content[0]["text"] == "hello"


def test_stream_openai_to_openai_cli_delta() -> None:
    reg = _make_registry_with_cli()
    state = StreamState()

    chunk = {
        "id": "chatcmpl_1",
        "object": "chat.completion.chunk",
        "created": 1,
        "model": "gpt-4o-mini",
        "choices": [
            {"index": 0, "delta": {"role": "assistant", "content": "hi"}, "finish_reason": None}
        ],
    }

    out_events = reg.convert_stream_chunk(chunk, "openai:chat", "openai:cli", state=state)
    assert isinstance(out_events, list) and out_events
    created = [e for e in out_events if e.get("type") == "response.created"]
    assert created
    deltas = [e for e in out_events if e.get("type") == "response.output_text.delta"]
    assert deltas
    assert deltas[0].get("delta") == "hi"


def test_stream_openai_cli_to_openai_delta() -> None:
    reg = _make_registry_with_cli()
    state = StreamState()

    chunk = {
        "type": "response.output_text.delta",
        "delta": "hi",
        "response": {"id": "resp_1", "model": "gpt-4o-mini"},
    }

    out_events = reg.convert_stream_chunk(chunk, "openai:cli", "openai:chat", state=state)
    assert isinstance(out_events, list) and out_events

    # 第一个 chunk 先补齐 assistant role
    assert out_events[0].get("object") == "chat.completion.chunk"
    # 第二个 chunk 才是文本增量
    choices = out_events[1].get("choices") or []
    assert isinstance(choices, list) and choices
    delta = cast(dict[str, Any], choices[0]).get("delta") or {}
    assert cast(dict[str, Any], delta).get("content") == "hi"


def test_openai_cli_function_call_to_claude() -> None:
    """测试 OpenAI CLI 的 function_call/function_call_output 转换为 Claude tool_use/tool_result"""
    reg = _make_registry_with_cli()

    openai_cli_req = {
        "model": "gpt-5",
        "instructions": "You are helpful.",
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "列出当前目录"}],
            },
            {
                "type": "function_call",
                "name": "shell_command",
                "arguments": '{"command": "ls -la"}',
                "call_id": "call_abc123",
            },
            {
                "type": "function_call_output",
                "call_id": "call_abc123",
                "output": "file1.txt\nfile2.txt",
            },
        ],
        "stream": True,
    }

    claude_req = reg.convert_request(openai_cli_req, "openai:cli", "claude:chat")

    messages = claude_req.get("messages", [])
    assert len(messages) == 3

    # 第一条：user 消息
    assert messages[0]["role"] == "user"
    assert messages[0]["content"] == "列出当前目录"

    # 第二条：assistant + tool_use
    assert messages[1]["role"] == "assistant"
    content1 = messages[1]["content"]
    assert isinstance(content1, list) and len(content1) == 1
    assert content1[0]["type"] == "tool_use"
    assert content1[0]["name"] == "shell_command"
    assert content1[0]["id"] == "call_abc123"
    assert content1[0]["input"] == {"command": "ls -la"}

    # 第三条：user + tool_result
    assert messages[2]["role"] == "user"
    content2 = messages[2]["content"]
    assert isinstance(content2, list) and len(content2) == 1
    assert content2[0]["type"] == "tool_result"
    assert content2[0]["tool_use_id"] == "call_abc123"
    assert content2[0]["content"] == "file1.txt\nfile2.txt"


def test_openai_cli_empty_call_id_repaired_when_convert_to_openai_chat() -> None:
    """空 call_id 会在转换链路中被自动修复，并保持 tool 调用关联。"""
    reg = _make_registry_with_cli()

    openai_cli_req = {
        "model": "gpt-5",
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "请读取 config"}],
            },
            {
                "type": "function_call",
                "name": "read_file",
                "arguments": '{"path": "config.yaml"}',
                "call_id": "",
            },
            {
                "type": "function_call_output",
                "call_id": "",
                "output": "ok",
            },
        ],
    }

    out = reg.convert_request(openai_cli_req, "openai:cli", "openai:chat")
    messages = cast(list[dict[str, Any]], out.get("messages") or [])

    assistant_msg = next((m for m in messages if m.get("role") == "assistant"), {})
    tool_calls = cast(list[dict[str, Any]], assistant_msg.get("tool_calls") or [])
    assert len(tool_calls) == 1

    generated_id = str(tool_calls[0].get("id") or "")
    assert generated_id.startswith("call_auto_")

    tool_msg = next((m for m in messages if m.get("role") == "tool"), {})
    assert tool_msg.get("tool_call_id") == generated_id


def test_openai_chat_empty_tool_call_id_repaired_when_convert_to_openai_cli() -> None:
    """openai:chat 的空 tool_call_id 转 openai:cli 时应生成有效 call_id。"""
    reg = _make_registry_with_cli()

    openai_chat_req = {
        "model": "gpt-5",
        "messages": [
            {"role": "user", "content": "帮我读取 README"},
            {
                "role": "assistant",
                "content": "",
                "tool_calls": [
                    {
                        "id": "",
                        "type": "function",
                        "function": {
                            "name": "read_file",
                            "arguments": '{"path":"README.md"}',
                        },
                    }
                ],
            },
            {
                "role": "tool",
                "tool_call_id": "",
                "content": "done",
            },
        ],
    }

    out = reg.convert_request(openai_chat_req, "openai:chat", "openai:cli")
    input_items = cast(list[dict[str, Any]], out.get("input") or [])

    function_call = next((i for i in input_items if i.get("type") == "function_call"), {})
    function_call_output = next(
        (i for i in input_items if i.get("type") == "function_call_output"), {}
    )

    generated_id = str(function_call.get("call_id") or "")
    assert generated_id.startswith("call_auto_")
    assert function_call_output.get("call_id") == generated_id


def test_openai_chat_prompt_cache_key_preserved_when_convert_to_openai_cli() -> None:
    reg = _make_registry_with_cli()

    openai_chat_req = {
        "model": "gpt-5",
        "messages": [{"role": "user", "content": "hi"}],
        "prompt_cache_key": "cache-key-123",
    }

    out = reg.convert_request(openai_chat_req, "openai:chat", "openai:cli")

    assert out["prompt_cache_key"] == "cache-key-123"


def test_openai_chat_tool_payload_preserves_raw_strings_when_convert_to_openai_cli() -> None:
    reg = _make_registry_with_cli()

    openai_chat_req = {
        "model": "gpt-5",
        "messages": [
            {"role": "user", "content": "帮我读取 README"},
            {
                "role": "assistant",
                "content": "",
                "tool_calls": [
                    {
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "read_file",
                            "arguments": '{"b":2,"a":1}',
                        },
                    }
                ],
            },
            {
                "role": "tool",
                "tool_call_id": "call_1",
                "content": '{"z":1,"a":2}',
            },
        ],
    }

    out = reg.convert_request(openai_chat_req, "openai:chat", "openai:cli")
    input_items = cast(list[dict[str, Any]], out.get("input") or [])

    function_call = next((i for i in input_items if i.get("type") == "function_call"), {})
    function_call_output = next(
        (i for i in input_items if i.get("type") == "function_call_output"), {}
    )

    assert function_call["arguments"] == '{"b":2,"a":1}'
    assert function_call_output["output"] == '{"z":1,"a":2}'


def test_openai_chat_text_config_maps_to_openai_cli_text_block() -> None:
    reg = _make_registry_with_cli()

    openai_chat_req = {
        "model": "gpt-5",
        "messages": [{"role": "user", "content": "hi"}],
        "response_format": {
            "type": "json_schema",
            "json_schema": {"name": "answer", "schema": {"type": "object"}},
        },
        "verbosity": "low",
        "logit_bias": {"42": 3},
    }

    out = reg.convert_request(openai_chat_req, "openai:chat", "openai:cli")

    assert out["text"] == {
        "format": {
            "type": "json_schema",
            "json_schema": {"name": "answer", "schema": {"type": "object"}},
        },
        "verbosity": "low",
    }
    assert "response_format" not in out
    assert "verbosity" not in out
    assert "logit_bias" not in out


def test_openai_cli_text_config_maps_to_openai_chat_fields() -> None:
    reg = _make_registry_with_cli()

    openai_cli_req = {
        "model": "gpt-5",
        "input": [{"role": "user", "content": [{"type": "input_text", "text": "hi"}]}],
        "text": {
            "format": {
                "type": "json_schema",
                "json_schema": {"name": "answer", "schema": {"type": "object"}},
            },
            "verbosity": "high",
        },
        "prompt_cache_key": "cache-key-456",
        "service_tier": "flex",
        "top_logprobs": 4,
    }

    out = reg.convert_request(openai_cli_req, "openai:cli", "openai:chat")

    assert out["response_format"] == {
        "type": "json_schema",
        "json_schema": {"name": "answer", "schema": {"type": "object"}},
    }
    assert out["verbosity"] == "high"
    assert out["prompt_cache_key"] == "cache-key-456"
    assert out["service_tier"] == "flex"
    assert out["top_logprobs"] == 4
    assert "text" not in out


def test_openai_chat_custom_tool_and_choice_convert_to_openai_cli() -> None:
    reg = _make_registry_with_cli()

    openai_chat_req = {
        "model": "gpt-5",
        "messages": [{"role": "user", "content": "hi"}],
        "tools": [
            {
                "type": "custom",
                "custom": {
                    "name": "grep_repo",
                    "description": "Search repository text",
                    "format": {"type": "text"},
                },
            }
        ],
        "tool_choice": {"type": "custom", "custom": {"name": "grep_repo"}},
    }

    out = reg.convert_request(openai_chat_req, "openai:chat", "openai:cli")

    assert out["tools"] == [
        {
            "type": "custom",
            "name": "grep_repo",
            "description": "Search repository text",
            "format": {"type": "text"},
        }
    ]
    assert out["tool_choice"] == {"type": "custom", "name": "grep_repo"}


def test_openai_cli_custom_tool_and_choice_convert_to_openai_chat() -> None:
    reg = _make_registry_with_cli()

    openai_cli_req = {
        "model": "gpt-5",
        "input": [{"role": "user", "content": [{"type": "input_text", "text": "hi"}]}],
        "tools": [
            {
                "type": "custom",
                "name": "grep_repo",
                "description": "Search repository text",
                "format": {"type": "text"},
            }
        ],
        "tool_choice": {"type": "custom", "name": "grep_repo"},
    }

    out = reg.convert_request(openai_cli_req, "openai:cli", "openai:chat")

    assert list(out.keys())[:3] == ["model", "tools", "messages"]
    assert out["tools"] == [
        {
            "type": "custom",
            "custom": {
                "name": "grep_repo",
                "description": "Search repository text",
                "format": {"type": "text"},
            },
        }
    ]
    assert out["tool_choice"] == {"type": "custom", "custom": {"name": "grep_repo"}}


def test_openai_chat_allowed_tools_choice_convert_to_openai_cli() -> None:
    reg = _make_registry_with_cli()

    openai_chat_req = {
        "model": "gpt-5",
        "messages": [{"role": "user", "content": "hi"}],
        "tool_choice": {
            "type": "allowed_tools",
            "allowed_tools": {
                "mode": "required",
                "tools": [{"type": "function", "function": {"name": "grep_repo"}}],
            },
        },
    }

    out = reg.convert_request(openai_chat_req, "openai:chat", "openai:cli")

    assert out["tool_choice"] == {
        "type": "allowed_tools",
        "mode": "required",
        "tools": [{"type": "function", "function": {"name": "grep_repo"}}],
    }


def test_openai_cli_allowed_tools_choice_convert_to_openai_chat() -> None:
    reg = _make_registry_with_cli()

    openai_cli_req = {
        "model": "gpt-5",
        "input": [{"role": "user", "content": [{"type": "input_text", "text": "hi"}]}],
        "tool_choice": {
            "type": "allowed_tools",
            "mode": "required",
            "tools": [{"type": "function", "name": "grep_repo"}],
        },
    }

    out = reg.convert_request(openai_cli_req, "openai:cli", "openai:chat")

    assert out["tool_choice"] == {
        "type": "allowed_tools",
        "allowed_tools": {
            "mode": "required",
            "tools": [{"type": "function", "name": "grep_repo"}],
        },
    }


def test_openai_chat_web_search_options_convert_to_openai_cli_tools() -> None:
    reg = _make_registry_with_cli()

    openai_chat_req = {
        "model": "gpt-5",
        "messages": [{"role": "user", "content": "hi"}],
        "web_search_options": {
            "user_location": {
                "type": "approximate",
                "approximate": {"country": "US", "city": "San Francisco"},
            },
            "search_context_size": "high",
        },
    }

    out = reg.convert_request(openai_chat_req, "openai:chat", "openai:cli")

    assert list(out.keys())[:3] == ["model", "tools", "input"]
    assert out["tools"] == [
        {
            "type": "web_search",
            "user_location": {
                "type": "approximate",
                "country": "US",
                "city": "San Francisco",
            },
            "search_context_size": "high",
        }
    ]
    assert "web_search_options" not in out


def test_openai_cli_web_search_tool_convert_to_openai_chat_options() -> None:
    reg = _make_registry_with_cli()

    openai_cli_req = {
        "model": "gpt-5",
        "input": [{"role": "user", "content": [{"type": "input_text", "text": "hi"}]}],
        "tools": [
            {
                "type": "web_search",
                "user_location": {
                    "type": "approximate",
                    "country": "US",
                    "city": "San Francisco",
                },
                "search_context_size": "high",
            }
        ],
    }

    out = reg.convert_request(openai_cli_req, "openai:cli", "openai:chat")

    assert out["web_search_options"] == {
        "user_location": {
            "type": "approximate",
            "approximate": {"country": "US", "city": "San Francisco"},
        },
        "search_context_size": "high",
    }
    assert "tools" not in out


def test_openai_cli_reasoning_preserved_in_roundtrip() -> None:
    """测试 OpenAI CLI 的 reasoning block 在 roundtrip 中被保留"""
    reg = _make_registry_with_cli()

    openai_cli_req = {
        "model": "gpt-5",
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "思考一下"}],
            },
            {
                "type": "reasoning",
                "summary": [{"type": "summary_text", "text": "I am thinking about the problem..."}],
                "content": None,
                "encrypted_content": "xxx_encrypted",
            },
            {
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "我想好了"}],
            },
        ],
    }

    # 转换到 internal 再转回 OPENAI_CLI
    converted = reg.convert_request(openai_cli_req, "openai:cli", "openai:cli")

    input_items = converted.get("input", [])
    # 应该有 user message, reasoning, assistant message
    assert len(input_items) >= 2

    # 找到 reasoning block
    reasoning_items = [
        i for i in input_items if isinstance(i, dict) and i.get("type") == "reasoning"
    ]
    assert len(reasoning_items) == 1
    assert "summary" in reasoning_items[0]


def test_claude_tool_use_to_openai_cli() -> None:
    """测试 Claude tool_use/tool_result 转换为 OpenAI CLI function_call/function_call_output"""
    reg = _make_registry_with_cli()

    claude_req = {
        "model": "claude-3",
        "messages": [
            {"role": "user", "content": "查看文件"},
            {
                "role": "assistant",
                "content": [
                    {
                        "type": "tool_use",
                        "id": "tool_123",
                        "name": "read_file",
                        "input": {"path": "/tmp/test.txt"},
                    }
                ],
            },
            {
                "role": "user",
                "content": [
                    {
                        "type": "tool_result",
                        "tool_use_id": "tool_123",
                        "content": "Hello World",
                    }
                ],
            },
        ],
    }

    openai_cli_req = reg.convert_request(claude_req, "claude:chat", "openai:cli")

    input_items = openai_cli_req.get("input", [])
    assert len(input_items) >= 3

    # 找到 function_call
    fc_items = [i for i in input_items if isinstance(i, dict) and i.get("type") == "function_call"]
    assert len(fc_items) == 1
    assert fc_items[0]["name"] == "read_file"
    assert fc_items[0]["call_id"] == "tool_123"

    # 找到 function_call_output
    fco_items = [
        i for i in input_items if isinstance(i, dict) and i.get("type") == "function_call_output"
    ]
    assert len(fco_items) == 1
    assert fco_items[0]["call_id"] == "tool_123"
    assert fco_items[0]["output"] == "Hello World"


def test_openai_cli_request_omits_implicit_empty_defaults_for_standard_responses() -> None:
    normalizer = OpenAICliNormalizer()
    internal = normalizer.request_to_internal({"model": "gpt-test", "input": []})

    out = normalizer.request_from_internal(internal)

    assert "instructions" not in out
    assert "stream" not in out
    assert "store" not in out


def test_openai_cli_request_preserves_explicit_empty_instructions_and_stream_false() -> None:
    normalizer = OpenAICliNormalizer()
    internal = normalizer.request_to_internal(
        {"model": "gpt-test", "input": [], "instructions": "", "stream": False}
    )

    out = normalizer.request_from_internal(internal)

    assert out["instructions"] == ""
    assert out["stream"] is False
    assert "store" not in out


def test_openai_cli_request_tool_choice_flat_function_roundtrip_preserved() -> None:
    normalizer = OpenAICliNormalizer()
    request = {
        "model": "gpt-test",
        "input": [],
        "tool_choice": {"type": "function", "name": "read_file"},
    }

    internal = normalizer.request_to_internal(request)
    out = normalizer.request_from_internal(internal)

    assert out["tool_choice"] == {"type": "function", "name": "read_file"}


def test_claude_explicit_effort_preserved_in_openai_cli() -> None:
    reg = _make_registry_with_cli()

    claude_req = {
        "model": "gpt-5.4",
        "messages": [{"role": "user", "content": "hi"}],
        "thinking": {"type": "enabled", "budget_tokens": 31999},
        "output_config": {"effort": "medium"},
    }

    out = reg.convert_request(claude_req, "claude:chat", "openai:cli")

    assert out["reasoning"] == {"effort": "medium"}


def test_claude_explicit_effort_preserved_in_openai_chat() -> None:
    reg = _make_registry_with_cli()

    claude_req = {
        "model": "gpt-5.4",
        "messages": [{"role": "user", "content": "hi"}],
        "thinking": {"type": "enabled", "budget_tokens": 31999},
        "output_config": {"effort": "medium"},
    }

    out = reg.convert_request(claude_req, "claude:chat", "openai:chat")

    assert out["reasoning_effort"] == "medium"


def test_stream_openai_cli_in_progress_event() -> None:
    """测试 OpenAI CLI 流式 response.in_progress 事件"""
    reg = _make_registry_with_cli()
    state = StreamState()

    # response.created 事件
    created_chunk = {
        "type": "response.created",
        "response": {
            "id": "resp_123",
            "object": "response",
            "model": "gpt-5",
            "status": "in_progress",
        },
    }

    events1 = reg.convert_stream_chunk(created_chunk, "openai:cli", "claude:chat", state=state)
    assert isinstance(events1, list) and events1
    assert events1[0].get("type") == "message_start"

    # response.in_progress 事件（不应产生内容事件）
    in_progress_chunk = {
        "type": "response.in_progress",
        "response": {
            "id": "resp_123",
            "object": "response",
            "model": "gpt-5",
            "status": "in_progress",
        },
    }

    events2 = reg.convert_stream_chunk(in_progress_chunk, "openai:cli", "claude:chat", state=state)
    # response.in_progress 不应产生任何事件
    assert events2 == []


def test_stream_openai_cli_noop_and_unknown_events() -> None:
    """覆盖 OpenAI CLI noop/unknown 事件处理器"""
    normalizer = OpenAICliNormalizer()
    state = StreamState()

    # 先触发 message_start 初始化
    normalizer.stream_chunk_to_internal(
        {"type": "response.created", "response": {"id": "resp_789", "model": "gpt-5"}}, state
    )

    # noop 事件不应产生内部事件
    assert (
        normalizer.stream_chunk_to_internal(
            {"type": "response.function_call_arguments.done"}, state
        )
        == []
    )
    assert normalizer.stream_chunk_to_internal({"type": "response.content_part.added"}, state) == []
    assert normalizer.stream_chunk_to_internal({"type": "response.content_part.done"}, state) == []

    # unknown 事件应返回 UnknownStreamEvent
    events = normalizer.stream_chunk_to_internal(
        {"type": "response.reasoning_summary_text.delta"}, state
    )
    assert len(events) == 1
    assert isinstance(events[0], UnknownStreamEvent)


def test_stream_openai_cli_function_call_events() -> None:
    """测试 OpenAI CLI 流式 function_call 相关事件"""
    reg = _make_registry_with_cli()
    state = StreamState()

    # 首先发送 response.created
    created_chunk = {
        "type": "response.created",
        "response": {"id": "resp_456", "model": "gpt-5"},
    }
    reg.convert_stream_chunk(created_chunk, "openai:cli", "claude:chat", state=state)

    # response.output_item.added (function_call)
    output_item_chunk = {
        "type": "response.output_item.added",
        "item": {
            "type": "function_call",
            "call_id": "call_xyz",
            "name": "get_weather",
        },
    }

    events1 = reg.convert_stream_chunk(output_item_chunk, "openai:cli", "claude:chat", state=state)
    assert isinstance(events1, list) and events1
    assert events1[0].get("type") == "content_block_start"

    # response.function_call_arguments.delta
    args_delta_chunk = {
        "type": "response.function_call_arguments.delta",
        "delta": '{"city":',
    }

    events2 = reg.convert_stream_chunk(args_delta_chunk, "openai:cli", "claude:chat", state=state)
    assert isinstance(events2, list) and events2
    # ToolCallDeltaEvent 转换为 Claude 的 content_block_delta
    assert events2[0].get("type") == "content_block_delta"
    delta_obj = events2[0].get("delta", {})
    assert delta_obj.get("type") == "input_json_delta"
    assert delta_obj.get("partial_json") == '{"city":'

    # response.output_item.done (function_call)
    output_done_chunk = {
        "type": "response.output_item.done",
        "item": {
            "type": "function_call",
            "call_id": "call_xyz",
            "name": "get_weather",
            "arguments": '{"city": "Beijing"}',
        },
    }

    events3 = reg.convert_stream_chunk(output_done_chunk, "openai:cli", "claude:chat", state=state)
    assert isinstance(events3, list) and events3
    assert events3[0].get("type") == "content_block_delta"
    assert events3[0].get("delta", {}).get("partial_json") == ' "Beijing"}'
    assert events3[-1].get("type") == "content_block_stop"


def test_stream_openai_cli_tool_then_text_uses_distinct_claude_block_indices() -> None:
    """Responses 工具调用后再输出文本时，Claude block index 不能复用。"""
    reg = _make_registry_with_cli()
    state = StreamState()

    cli_chunks: list[dict[str, Any]] = [
        {
            "type": "response.created",
            "response": {
                "id": "resp_tool_then_text",
                "object": "response",
                "model": "gpt-5.4",
                "status": "in_progress",
                "output": [],
            },
        },
        {
            "type": "response.output_item.added",
            "output_index": 0,
            "item": {
                "type": "function_call",
                "call_id": "call_tool_then_text",
                "id": "fc_tool_then_text",
                "name": "read_file",
                "status": "in_progress",
                "arguments": "",
            },
        },
        {
            "type": "response.function_call_arguments.delta",
            "output_index": 0,
            "item_id": "fc_tool_then_text",
            "delta": '{"path":"README.md"}',
        },
        {
            "type": "response.output_text.delta",
            "output_index": 1,
            "item_id": "msg_tool_then_text",
            "content_index": 0,
            "delta": "done",
        },
        {
            "type": "response.output_item.done",
            "output_index": 0,
            "item": {
                "type": "function_call",
                "call_id": "call_tool_then_text",
                "id": "fc_tool_then_text",
                "name": "read_file",
                "status": "completed",
                "arguments": '{"path":"README.md"}',
            },
        },
        {
            "type": "response.completed",
            "response": {
                "id": "resp_tool_then_text",
                "object": "response",
                "model": "gpt-5.4",
                "status": "completed",
                "output": [],
                "usage": {"input_tokens": 8, "output_tokens": 4, "total_tokens": 12},
            },
        },
    ]

    all_events: list[dict[str, Any]] = []
    for chunk in cli_chunks:
        all_events.extend(reg.convert_stream_chunk(chunk, "openai:cli", "claude:cli", state=state))

    starts = [e for e in all_events if e.get("type") == "content_block_start"]
    assert len(starts) >= 2

    tool_start = next(e for e in starts if (e.get("content_block") or {}).get("type") == "tool_use")
    text_start = next(e for e in starts if (e.get("content_block") or {}).get("type") == "text")
    assert tool_start["index"] != text_start["index"]

    text_delta = next(
        e
        for e in all_events
        if e.get("type") == "content_block_delta"
        and (e.get("delta") or {}).get("type") == "text_delta"
    )
    assert text_delta["index"] == text_start["index"]
    assert text_delta["index"] != tool_start["index"]


def test_stream_openai_cli_function_call_done_without_delta_emits_full_args() -> None:
    """无 arguments.delta 时，done 快照也应补出完整 tool arguments。"""
    reg = _make_registry_with_cli()
    state = StreamState()

    cli_chunks: list[dict[str, Any]] = [
        {
            "type": "response.created",
            "response": {
                "id": "resp_done_only",
                "object": "response",
                "model": "gpt-5",
                "status": "in_progress",
                "output": [],
            },
        },
        {
            "type": "response.output_item.added",
            "output_index": 0,
            "item": {
                "type": "function_call",
                "call_id": "call_read_1",
                "id": "call_read_1",
                "name": "read",
                "status": "in_progress",
                "arguments": "",
            },
        },
        {
            "type": "response.function_call_arguments.done",
            "output_index": 0,
            "item_id": "call_read_1",
            "arguments": '{"filePath":"/tmp/demo.txt"}',
        },
        {
            "type": "response.output_item.done",
            "output_index": 0,
            "item": {
                "type": "function_call",
                "call_id": "call_read_1",
                "id": "call_read_1",
                "name": "read",
                "status": "completed",
                "arguments": '{"filePath":"/tmp/demo.txt"}',
            },
        },
        {
            "type": "response.completed",
            "response": {
                "id": "resp_done_only",
                "object": "response",
                "model": "gpt-5",
                "status": "completed",
                "output": [],
                "usage": {"input_tokens": 8, "output_tokens": 4, "total_tokens": 12},
            },
        },
    ]

    all_events: list[dict[str, Any]] = []
    for chunk in cli_chunks:
        all_events.extend(reg.convert_stream_chunk(chunk, "openai:cli", "openai:chat", state=state))

    collected_args = ""
    for event in all_events:
        for choice in event.get("choices", []):
            for tc in choice.get("delta", {}).get("tool_calls") or []:
                fn = tc.get("function") or {}
                collected_args += str(fn.get("arguments") or "")

    assert collected_args == '{"filePath":"/tmp/demo.txt"}'


def test_stream_openai_cli_uses_call_id_not_item_id_for_tool_deltas() -> None:
    """Responses API 中 item.id 与 call_id 不同时，应统一映射到 call_id。"""
    reg = _make_registry_with_cli()
    state = StreamState()

    cli_chunks: list[dict[str, Any]] = [
        {
            "type": "response.created",
            "response": {
                "id": "resp_call_alias",
                "object": "response",
                "model": "gpt-5.4",
                "status": "in_progress",
                "output": [],
            },
        },
        {
            "type": "response.output_item.added",
            "output_index": 0,
            "item": {
                "type": "function_call",
                "call_id": "call_CfAXHBAtvuxd1HEHGgHUUscU",
                "id": "fc_080c89b8d042bd430169b6c420833481918c61ed6112e83344",
                "name": "bash",
                "status": "in_progress",
                "arguments": "",
            },
        },
        {
            "type": "response.function_call_arguments.delta",
            "output_index": 0,
            "item_id": "fc_080c89b8d042bd430169b6c420833481918c61ed6112e83344",
            "delta": '{"command":"find . -maxdepth 2 | sort","timeout":10}',
        },
        {
            "type": "response.output_item.done",
            "output_index": 0,
            "item": {
                "type": "function_call",
                "call_id": "call_CfAXHBAtvuxd1HEHGgHUUscU",
                "id": "fc_080c89b8d042bd430169b6c420833481918c61ed6112e83344",
                "name": "bash",
                "status": "completed",
                "arguments": '{"command":"find . -maxdepth 2 | sort","timeout":10}',
            },
        },
        {
            "type": "response.completed",
            "response": {
                "id": "resp_call_alias",
                "object": "response",
                "model": "gpt-5.4",
                "status": "completed",
                "output": [],
                "usage": {"input_tokens": 8, "output_tokens": 4, "total_tokens": 12},
            },
        },
    ]

    all_events: list[dict[str, Any]] = []
    for chunk in cli_chunks:
        all_events.extend(reg.convert_stream_chunk(chunk, "openai:cli", "openai:chat", state=state))

    tc_indices: list[int] = []
    tc_ids: list[str] = []
    collected_args = ""
    for event in all_events:
        for choice in event.get("choices", []):
            for tc in choice.get("delta", {}).get("tool_calls") or []:
                tc_indices.append(int(tc.get("index", -1)))
                tc_ids.append(str(tc.get("id") or ""))
                collected_args += str((tc.get("function") or {}).get("arguments") or "")

    assert tc_indices
    assert set(tc_indices) == {0}, f"expected a single tool_call index, got {tc_indices}"
    assert set(tc_ids) == {"call_CfAXHBAtvuxd1HEHGgHUUscU"}, (
        "tool deltas should use call_id, not raw item.id, " f"got ids={tc_ids}"
    )
    assert collected_args == '{"command":"find . -maxdepth 2 | sort","timeout":10}'


def test_real_claude_cli_stream_response_conversion() -> None:
    """测试真实的 Claude CLI 流式响应转换（完整事件序列）

    使用来自 Claude Code 的真实流式响应数据，验证：
    - message_start, content_block_start, ping, content_block_delta,
      content_block_stop, message_delta, message_stop 的完整处理链路
    - 文本增量正确拼接
    - usage 和 stop_reason 正确提取
    """
    reg = _make_registry_with_cli()
    state = StreamState()

    # 真实的 Claude CLI 流式响应事件序列
    chunks: list[dict[str, Any]] = [
        {
            "type": "message_start",
            "message": {
                "model": "claude-opus-4-5-20251101",
                "id": "msg_01JEmQ1u53gZndRGBUBLvVZ9",
                "type": "message",
                "role": "assistant",
                "content": [],
                "stop_reason": None,
                "stop_sequence": None,
                "usage": {
                    "input_tokens": 3,
                    "cache_creation_input_tokens": 32760,
                    "cache_read_input_tokens": 61777,
                    "cache_creation": {
                        "ephemeral_5m_input_tokens": 32760,
                        "ephemeral_1h_input_tokens": 0,
                    },
                    "output_tokens": 2,
                    "service_tier": "standard",
                },
            },
        },
        {
            "type": "content_block_start",
            "index": 0,
            "content_block": {"type": "text", "text": ""},
        },
        {"type": "ping"},
        {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "明"}},
        {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "白"}},
        {
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "text_delta", "text": "了，请"},
        },
        {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "把"}},
        {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "包"}},
        {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "含 "}},
        {
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "text_delta", "text": "tools"},
        },
        {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": " 具"}},
        {
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "text_delta", "text": "体内"},
        },
        {
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "text_delta", "text": "容的 "},
        },
        {
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "text_delta", "text": "Claude"},
        },
        {
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "text_delta", "text": " CLI"},
        },
        {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": " 请"}},
        {
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "text_delta", "text": "求体"},
        },
        {
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "text_delta", "text": "发给我，"},
        },
        {
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "text_delta", "text": "我会一"},
        },
        {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "并"}},
        {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "审"}},
        {
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "text_delta", "text": "查整"},
        },
        {
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "text_delta", "text": "个改"},
        },
        {
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "text_delta", "text": "动。"},
        },
        {"type": "content_block_stop", "index": 0},
        {
            "type": "message_delta",
            "delta": {"stop_reason": "end_turn", "stop_sequence": None},
            "usage": {
                "input_tokens": 3,
                "cache_creation_input_tokens": 32760,
                "cache_read_input_tokens": 61777,
                "output_tokens": 42,
            },
        },
        {"type": "message_stop"},
    ]

    # 收集所有转换后的 OpenAI 格式事件
    all_openai_events: list[dict[str, Any]] = []
    for chunk in chunks:
        events = reg.convert_stream_chunk(chunk, "claude:cli", "openai:chat", state=state)
        all_openai_events.extend(events)

    # 验证转换结果
    assert len(all_openai_events) > 0

    # 第一个事件应该是 chat.completion.chunk（来自 message_start）
    assert all_openai_events[0].get("object") == "chat.completion.chunk"
    assert all_openai_events[0].get("model") == "claude-opus-4-5-20251101"

    # 收集所有文本增量
    text_deltas = []
    for evt in all_openai_events:
        choices = evt.get("choices") or []
        if choices:
            delta = choices[0].get("delta", {})
            content = delta.get("content")
            if content:
                text_deltas.append(content)

    # 验证文本拼接结果
    full_text = "".join(text_deltas)
    assert "明白了" in full_text
    assert "tools" in full_text
    assert "Claude CLI" in full_text
    assert "请求体发给我" in full_text

    # 验证最后一个事件有 finish_reason
    last_with_finish = [
        e for e in all_openai_events if (e.get("choices") or [{}])[0].get("finish_reason")
    ]
    assert len(last_with_finish) > 0
    assert last_with_finish[-1]["choices"][0]["finish_reason"] == "stop"


def test_real_claude_cli_stream_to_openai_cli() -> None:
    """测试 Claude CLI 流式响应转换为 OpenAI CLI (Responses API) 格式"""
    reg = _make_registry_with_cli()
    state = StreamState()

    # 简化的真实事件序列
    chunks: list[dict[str, Any]] = [
        {
            "type": "message_start",
            "message": {
                "model": "claude-opus-4-5-20251101",
                "id": "msg_test123",
                "type": "message",
                "role": "assistant",
                "content": [],
                "stop_reason": None,
                "usage": {"input_tokens": 10, "output_tokens": 0},
            },
        },
        {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}},
        {"type": "ping"},
        {
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "text_delta", "text": "Hello"},
        },
        {
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "text_delta", "text": " World"},
        },
        {"type": "content_block_stop", "index": 0},
        {
            "type": "message_delta",
            "delta": {"stop_reason": "end_turn"},
            "usage": {"input_tokens": 10, "output_tokens": 5},
        },
        {"type": "message_stop"},
    ]

    all_events: list[dict[str, Any]] = []
    for chunk in chunks:
        events = reg.convert_stream_chunk(chunk, "claude:cli", "openai:cli", state=state)
        all_events.extend(events)

    # 验证 OpenAI CLI 格式事件
    assert len(all_events) > 0

    # 应该有 response.created 事件
    created_events = [e for e in all_events if e.get("type") == "response.created"]
    assert len(created_events) == 1

    # 应该有 response.output_text.delta 事件
    delta_events = [e for e in all_events if e.get("type") == "response.output_text.delta"]
    assert len(delta_events) >= 2
    deltas = [e.get("delta") for e in delta_events]
    assert "Hello" in deltas
    assert " World" in deltas

    # 应该有 response.completed 或 response.done 事件
    done_events = [
        e for e in all_events if e.get("type") in ("response.completed", "response.done")
    ]
    assert len(done_events) >= 1


def test_resp_id_normalized_to_chatcmpl_in_response_conversion() -> None:
    """openai:cli -> openai:chat 响应转换时 resp_ 前缀应转为 chatcmpl-。"""
    reg = _make_registry_with_cli()

    cli_response = {
        "id": "resp_67ccfcdd16748190a91872c75d38539e",
        "object": "response",
        "model": "gpt-4o",
        "status": "completed",
        "output": [
            {
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "hello"}],
            }
        ],
        "usage": {"input_tokens": 10, "output_tokens": 5, "total_tokens": 15},
    }

    chat_response = reg.convert_response(
        cli_response, "openai:cli", "openai:chat", requested_model="gpt-4o"
    )
    assert chat_response["id"].startswith("chatcmpl-")
    assert "resp_" not in chat_response["id"]


def test_resp_id_normalized_to_chatcmpl_in_stream_conversion() -> None:
    """openai:cli -> openai:chat 流式转换时 chunk id 应为 chatcmpl- 前缀。"""
    reg = _make_registry_with_cli()

    cli_chunks: list[dict[str, Any]] = [
        {
            "type": "response.created",
            "response": {
                "id": "resp_abc123",
                "object": "response",
                "model": "gpt-4o",
                "status": "in_progress",
                "output": [],
            },
        },
        {
            "type": "response.output_text.delta",
            "delta": "hi",
            "content_index": 0,
            "output_index": 0,
        },
        {
            "type": "response.completed",
            "response": {
                "id": "resp_abc123",
                "object": "response",
                "model": "gpt-4o",
                "status": "completed",
                "output": [],
                "usage": {"input_tokens": 5, "output_tokens": 2, "total_tokens": 7},
            },
        },
    ]

    state = StreamState()
    all_events: list[dict[str, Any]] = []
    for chunk in cli_chunks:
        events = reg.convert_stream_chunk(chunk, "openai:cli", "openai:chat", state=state)
        all_events.extend(events)

    # 所有 chunk 的 id 都应为 chatcmpl- 前缀
    for event in all_events:
        event_id = event.get("id", "")
        if event_id:
            assert event_id.startswith(
                "chatcmpl-"
            ), f"chunk id should start with chatcmpl-: {event_id}"


def test_tool_call_stream_index_stable_across_deltas() -> None:
    """openai:cli -> openai:chat 流式 tool_call 的所有 delta 应保持同一 index。"""
    reg = _make_registry_with_cli()

    cli_chunks: list[dict[str, Any]] = [
        {
            "type": "response.created",
            "response": {
                "id": "resp_tc_idx",
                "object": "response",
                "model": "gpt-4o",
                "status": "in_progress",
                "output": [],
            },
        },
        {
            "type": "response.output_item.added",
            "output_index": 0,
            "item": {
                "type": "function_call",
                "call_id": "fc_001",
                "id": "fc_001",
                "name": "get_weather",
                "status": "in_progress",
                "arguments": "",
            },
        },
        {
            "type": "response.function_call_arguments.delta",
            "output_index": 0,
            "item_id": "fc_001",
            "delta": '{"loc',
        },
        {
            "type": "response.function_call_arguments.delta",
            "output_index": 0,
            "item_id": "fc_001",
            "delta": 'ation": "Tokyo"}',
        },
        {
            "type": "response.function_call_arguments.done",
            "output_index": 0,
            "item_id": "fc_001",
            "arguments": '{"location": "Tokyo"}',
        },
        {
            "type": "response.output_item.done",
            "output_index": 0,
            "item": {
                "type": "function_call",
                "call_id": "fc_001",
                "id": "fc_001",
                "name": "get_weather",
                "status": "completed",
                "arguments": '{"location": "Tokyo"}',
            },
        },
        {
            "type": "response.completed",
            "response": {
                "id": "resp_tc_idx",
                "object": "response",
                "model": "gpt-4o",
                "status": "completed",
                "output": [],
                "usage": {"input_tokens": 20, "output_tokens": 10, "total_tokens": 30},
            },
        },
    ]

    state = StreamState()
    all_events: list[dict[str, Any]] = []
    for chunk in cli_chunks:
        events = reg.convert_stream_chunk(chunk, "openai:cli", "openai:chat", state=state)
        all_events.extend(events)

    # 收集所有 tool_calls chunk
    tc_indices: list[int] = []
    tc_ids: list[str] = []
    tc_names: list[str] = []
    for event in all_events:
        for choice in event.get("choices", []):
            tcs = choice.get("delta", {}).get("tool_calls")
            if isinstance(tcs, list):
                for tc in tcs:
                    tc_indices.append(tc["index"])
                    tc_ids.append(str(tc.get("id") or ""))
                    tc_names.append(str((tc.get("function") or {}).get("name") or ""))

    assert len(tc_indices) >= 2, f"expected at least 2 tool_call chunks, got {len(tc_indices)}"
    # 同一个 tool call 的所有 chunk 必须使用相同的 index
    assert all(
        idx == tc_indices[0] for idx in tc_indices
    ), f"tool_call index should be stable, got: {tc_indices}"
    assert all(tc_id == "fc_001" for tc_id in tc_ids), (
        "tool_call id should be repeated on every delta for strict OpenAI-compatible clients, "
        f"got: {tc_ids}"
    )
    assert all(tc_name == "get_weather" for tc_name in tc_names), (
        "tool_call function.name should be repeated on every delta for strict "
        f"OpenAI-compatible clients, got: {tc_names}"
    )
