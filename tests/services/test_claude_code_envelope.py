from __future__ import annotations

from types import SimpleNamespace

import pytest

from src.api.handlers.base.request_builder import PassthroughRequestBuilder
from src.config.settings import config
from src.services.provider.adapters.claude_code.constants import (
    BETA_CLAUDE_CODE,
    BETA_CONTEXT_1M,
    BETA_INTERLEAVED_THINKING,
    BETA_OAUTH,
    CLAUDE_CODE_REQUIRED_BETA_TOKENS,
    DEFAULT_ACCEPT,
    DEFAULT_ANTHROPIC_VERSION,
    STREAM_HELPER_METHOD,
)
from src.services.provider.adapters.claude_code.context import set_claude_code_request_context
from src.services.provider.adapters.claude_code.envelope import (
    claude_code_envelope,
    merge_anthropic_beta_tokens,
)
from src.services.provider.fingerprint import load_fingerprint
from src.services.provider.request_context import set_current_fingerprint


@pytest.fixture(autouse=True)
def _reset_claude_code_context() -> None:  # type: ignore[misc]
    set_claude_code_request_context(None)
    set_current_fingerprint(None)
    yield
    set_claude_code_request_context(None)
    set_current_fingerprint(None)


def test_merge_anthropic_beta_tokens_adds_required_and_deduplicates() -> None:
    merged = merge_anthropic_beta_tokens(
        "context-1m-2025-08-07,oauth-2025-04-20,custom-beta,claude-code-20250219"
    )

    assert merged.split(",") == [
        BETA_CLAUDE_CODE,
        BETA_OAUTH,
        BETA_INTERLEAVED_THINKING,
        "context-1m-2025-08-07",
        "custom-beta",
    ]


def test_claude_code_envelope_extra_headers_include_required_defaults(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(config, "internal_user_agent_claude_cli", "claude-code/test")

    headers = claude_code_envelope.extra_headers() or {}

    assert headers.get("anthropic-version") == DEFAULT_ANTHROPIC_VERSION
    assert headers.get("anthropic-beta") == ",".join(CLAUDE_CODE_REQUIRED_BETA_TOKENS)
    assert headers.get("Accept") == DEFAULT_ACCEPT
    assert headers.get("X-App") == "cli"
    assert headers.get("X-Stainless-Lang") == "js"
    assert headers.get("Anthropic-Dangerous-Direct-Browser-Access") == "true"
    assert headers.get("User-Agent") == "claude-code/test"
    assert "x-stainless-helper-method" not in headers


def test_claude_code_envelope_extra_headers_use_current_fingerprint() -> None:
    fp = load_fingerprint(
        {
            "stainless_package_version": "1.0.5",
            "stainless_os": "Windows",
            "stainless_arch": "x64",
            "stainless_runtime_version": "v22.12.0",
            "stainless_timeout": "900",
            "user_agent": "Mozilla/5.0 test-fingerprint",
        },
        "key-fingerprint-1",
    )
    set_current_fingerprint(fp)

    headers = claude_code_envelope.extra_headers() or {}
    assert headers.get("X-Stainless-Package-Version") == "1.0.5"
    assert headers.get("X-Stainless-OS") == "Windows"
    assert headers.get("X-Stainless-Arch") == "x64"
    assert headers.get("X-Stainless-Runtime-Version") == "v22.12.0"
    assert headers.get("X-Stainless-Timeout") == "900"
    assert headers.get("User-Agent") == "Mozilla/5.0 test-fingerprint"


def test_claude_code_prepare_context_prefers_fingerprint_tls_profile() -> None:
    fp = load_fingerprint({"impersonate": "chrome124"}, "key-fingerprint-2")
    set_current_fingerprint(fp)

    tls_profile = claude_code_envelope.prepare_context(
        provider_config={"claude_code_advanced": {"enable_tls_fingerprint": True}},
        key_id="key-fingerprint-2",
        is_stream=False,
        provider_id="provider-1",
    )

    assert tls_profile == "chrome124"


def test_claude_code_envelope_adds_stream_helper_header_for_stream_request() -> None:
    _, _ = claude_code_envelope.wrap_request(
        {"stream": True},
        model="claude-sonnet-4-5-20250929",
        url_model=None,
        decrypted_auth_config=None,
    )
    headers = claude_code_envelope.extra_headers() or {}
    assert headers.get("Accept") == DEFAULT_ACCEPT
    assert headers.get("x-stainless-helper-method") == STREAM_HELPER_METHOD


def test_passthrough_request_builder_drops_context_1m_for_claude_code_oauth() -> None:
    builder = PassthroughRequestBuilder()
    endpoint = SimpleNamespace(header_rules=None)
    key = SimpleNamespace(api_key="unused")

    headers = builder.build_headers(
        original_headers={"anthropic-beta": BETA_CONTEXT_1M},
        endpoint=endpoint,
        key=key,
        extra_headers={"anthropic-beta": ",".join(CLAUDE_CODE_REQUIRED_BETA_TOKENS)},
        pre_computed_auth=("Authorization", "Bearer test-token"),
        envelope=claude_code_envelope,
    )

    assert headers.get("anthropic-beta") == ",".join(CLAUDE_CODE_REQUIRED_BETA_TOKENS)


def test_passthrough_request_builder_keeps_context_1m_for_non_claude_code_provider() -> None:
    builder = PassthroughRequestBuilder()
    endpoint = SimpleNamespace(header_rules=None)
    key = SimpleNamespace(api_key="unused")

    headers = builder.build_headers(
        original_headers={"anthropic-beta": BETA_CONTEXT_1M},
        endpoint=endpoint,
        key=key,
        extra_headers={"anthropic-beta": ",".join(CLAUDE_CODE_REQUIRED_BETA_TOKENS)},
        pre_computed_auth=("Authorization", "Bearer test-token"),
        envelope=None,
    )

    assert BETA_CONTEXT_1M in str(headers.get("anthropic-beta") or "")


def test_claude_code_envelope_filters_invalid_thinking_blocks_when_enabled() -> None:
    body = {
        "thinking": {"type": "enabled"},
        "messages": [
            {
                "role": "assistant",
                "content": [
                    {"type": "thinking", "thinking": "keep", "signature": "sig_valid"},
                    {"type": "thinking", "thinking": "drop-empty-signature", "signature": ""},
                    {
                        "type": "thinking",
                        "thinking": "drop-dummy-signature",
                        "signature": "skip_thought_signature_validator",
                    },
                    {"type": "redacted_thinking", "data": "keep", "signature": "sig_redacted"},
                    {"type": "redacted_thinking", "data": "drop-no-signature"},
                    {"thinking": "drop-no-type"},
                    {"type": "text", "text": "ok"},
                ],
            }
        ],
    }

    wrapped, _ = claude_code_envelope.wrap_request(
        body,
        model="claude-sonnet-4-5-20250929",
        url_model=None,
        decrypted_auth_config=None,
    )

    content = wrapped["messages"][0]["content"]
    assert content == [
        {"type": "thinking", "thinking": "keep", "signature": "sig_valid"},
        {"type": "redacted_thinking", "data": "keep", "signature": "sig_redacted"},
        {"type": "text", "text": "ok"},
    ]


def test_claude_code_envelope_drops_all_thinking_blocks_when_disabled() -> None:
    body = {
        "thinking": {"type": "disabled"},
        "messages": [
            {
                "role": "assistant",
                "content": [
                    {"type": "thinking", "thinking": "remove", "signature": "sig_valid"},
                    {"type": "redacted_thinking", "data": "remove", "signature": "sig_redacted"},
                    {"type": "text", "text": "keep"},
                ],
            },
            {
                "role": "user",
                "content": [
                    {"type": "thinking", "thinking": "remove-user", "signature": "sig_user"},
                    {"type": "text", "text": "keep-user"},
                ],
            },
        ],
    }

    wrapped, _ = claude_code_envelope.wrap_request(
        body,
        model="claude-sonnet-4-5-20250929",
        url_model=None,
        decrypted_auth_config=None,
    )

    assert wrapped["messages"][0]["content"] == [{"type": "text", "text": "keep"}]
    assert wrapped["messages"][1]["content"] == [{"type": "text", "text": "keep-user"}]
