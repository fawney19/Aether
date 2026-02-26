from __future__ import annotations

import re
from typing import Any

import pytest

from src.services.provider.adapters.claude_code.context import (
    ClaudeCodeRequestContext,
    get_claude_code_request_context,
    set_claude_code_request_context,
)
from src.services.provider.adapters.claude_code.envelope import (
    ClaudeCodeEnvelope,
    apply_claude_tool_prefix,
    apply_claude_tool_prefix_with_alias_map,
    merge_anthropic_beta_headers,
    strip_claude_tool_prefix_from_response,
)


@pytest.fixture(autouse=True)
def _reset_claude_code_context() -> Any:
    set_claude_code_request_context(None)
    yield
    set_claude_code_request_context(None)


def test_apply_claude_tool_prefix_for_oauth_tools_and_history_blocks() -> None:
    request_body = {
        "stream": True,
        "tools": [
            {"name": "web_search", "type": "web_search_20250305"},
            {"name": "write_file"},
        ],
        "tool_choice": {"type": "tool", "name": "write_file"},
        "messages": [
            {
                "role": "assistant",
                "content": [
                    {"type": "tool_use", "name": "write_file", "id": "tool_1", "input": {}},
                    {"type": "tool_reference", "tool_name": "write_file"},
                    {
                        "type": "tool_result",
                        "content": [{"type": "tool_reference", "tool_name": "write_file"}],
                    },
                ],
            }
        ],
    }

    out = apply_claude_tool_prefix(request_body)

    assert out["tools"][0]["name"] == "web_search"
    assert out["tools"][1]["name"] == "proxy_write_file"
    assert out["tool_choice"]["name"] == "proxy_write_file"

    first_block = out["messages"][0]["content"][0]
    second_block = out["messages"][0]["content"][1]
    nested_block = out["messages"][0]["content"][2]["content"][0]
    assert first_block["name"] == "proxy_write_file"
    assert second_block["tool_name"] == "proxy_write_file"
    assert nested_block["tool_name"] == "proxy_write_file"


def test_claude_code_envelope_wrap_request_sets_context_and_headers_for_oauth() -> None:
    envelope = ClaudeCodeEnvelope()
    request_body = {
        "stream": True,
        "betas": ["custom-beta", "oauth-2025-04-20"],
        "tools": [{"name": "write_file"}],
    }

    wrapped, _ = envelope.wrap_request(
        request_body,
        model="claude-sonnet-4-6",
        url_model=None,
        decrypted_auth_config={
            "provider_type": "claude_code",
            "refresh_token": "refresh-token",
            "email": "user@example.com",
        },
    )

    assert "betas" not in wrapped
    wrapped_tool_name = wrapped["tools"][0]["name"]
    assert wrapped_tool_name.startswith("proxy_write_file_")
    assert re.match(r"^proxy_write_file_[a-z0-9]{4,}$", wrapped_tool_name)
    assert envelope.force_stream_rewrite() is True

    ctx = get_claude_code_request_context()
    assert ctx is not None
    assert ctx.tool_randomized is True
    assert dict(ctx.tool_aliases).get(wrapped_tool_name) == "write_file"

    headers = envelope.extra_headers() or {}
    lower_headers = {k.lower(): v for k, v in headers.items()}
    beta_tokens = [item.strip() for item in lower_headers["anthropic-beta"].split(",") if item.strip()]

    assert "custom-beta" in beta_tokens
    assert "oauth-2025-04-20" in beta_tokens
    assert "prompt-caching-2024-07-31" in beta_tokens
    assert lower_headers["accept"] == "text/event-stream"
    assert lower_headers["x-app"] == "cli"
    assert lower_headers["anthropic-version"] == "2023-06-01"


def test_claude_code_envelope_api_key_mode_keeps_tool_names() -> None:
    envelope = ClaudeCodeEnvelope()
    wrapped, _ = envelope.wrap_request(
        {"stream": False, "tools": [{"name": "write_file"}]},
        model="claude-sonnet-4-6",
        url_model=None,
        decrypted_auth_config={"auth_method": "api_key"},
    )

    assert wrapped["tools"][0]["name"] == "write_file"
    assert envelope.force_stream_rewrite() is False

    headers = envelope.extra_headers() or {}
    lower_headers = {k.lower(): v for k, v in headers.items()}
    assert lower_headers["accept"] == "application/json"


def test_claude_code_envelope_wrap_request_normalizes_compatible_fields() -> None:
    envelope = ClaudeCodeEnvelope()
    wrapped, _ = envelope.wrap_request(
        {
            "stream": 1,
            "max_tokens_to_sample": "512",
            "stop": ["\n\n", ""],
            "tool_choice": "required",
            "frequency_penalty": 0.6,
            "presence_penalty": 0.4,
        },
        model="claude-sonnet-4-6",
        url_model=None,
        decrypted_auth_config={"auth_method": "api_key"},
    )

    assert wrapped["stream"] is True
    assert wrapped["max_tokens"] == 512
    assert wrapped["stop_sequences"] == ["\n\n"]
    assert wrapped["tool_choice"] == {"type": "any"}
    assert "frequency_penalty" not in wrapped
    assert "presence_penalty" not in wrapped


def test_strip_claude_tool_prefix_from_response_for_non_stream_and_stream_events() -> None:
    set_claude_code_request_context(
        ClaudeCodeRequestContext(
            auth_method="oauth",
            tool_prefix_enabled=True,
        )
    )

    envelope = ClaudeCodeEnvelope()

    non_stream_payload: dict[str, Any] = {
        "content": [
            {"type": "tool_use", "name": "proxy_write_file"},
            {"type": "tool_reference", "tool_name": "proxy_write_file"},
            {
                "type": "tool_result",
                "content": [{"type": "tool_reference", "tool_name": "proxy_write_file"}],
            },
        ]
    }
    stream_payload: dict[str, Any] = {
        "type": "content_block_start",
        "content_block": {
            "type": "tool_use",
            "name": "proxy_write_file",
        },
    }

    non_stream_out = envelope.unwrap_response(non_stream_payload)
    stream_out = envelope.unwrap_response(stream_payload)

    assert non_stream_out["content"][0]["name"] == "write_file"
    assert non_stream_out["content"][1]["tool_name"] == "write_file"
    assert non_stream_out["content"][2]["content"][0]["tool_name"] == "write_file"
    assert stream_out["content_block"]["name"] == "write_file"


def test_merge_anthropic_beta_headers_deduplicates_and_preserves_order() -> None:
    merged = merge_anthropic_beta_headers(
        "context-1m-2025-08-07,oauth-2025-04-20",
        "claude-code-20250219,oauth-2025-04-20,prompt-caching-2024-07-31",
    )
    tokens = [item.strip() for item in merged.split(",") if item.strip()]

    assert tokens[0] == "context-1m-2025-08-07"
    assert "claude-code-20250219" in tokens
    assert "prompt-caching-2024-07-31" in tokens
    assert tokens.count("oauth-2025-04-20") == 1


def test_strip_tool_prefix_helper_is_noop_for_non_dict_payload() -> None:
    assert strip_claude_tool_prefix_from_response("invalid") == "invalid"


def test_apply_claude_tool_prefix_with_alias_map_randomized_and_recoverable() -> None:
    request_body = {
        "tools": [{"name": "write_file"}],
        "messages": [
            {
                "role": "assistant",
                "content": [{"type": "tool_use", "name": "write_file"}],
            }
        ],
    }
    transformed, alias_map = apply_claude_tool_prefix_with_alias_map(
        request_body,
        randomize_suffix=True,
        random_suffix="unit01",
    )

    aliased_name = transformed["tools"][0]["name"]
    assert aliased_name == "proxy_write_file_unit01"
    assert alias_map == {"proxy_write_file_unit01": "write_file"}

    payload = {
        "type": "content_block_start",
        "content_block": {"type": "tool_use", "name": aliased_name},
    }
    restored = strip_claude_tool_prefix_from_response(payload, alias_to_original=alias_map)
    assert restored["content_block"]["name"] == "write_file"
