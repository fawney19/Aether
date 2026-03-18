from __future__ import annotations

import hashlib

from src.services.provider.adapters.codex.context import (
    CodexRequestContext,
    set_codex_request_context,
)
from src.services.provider.upstream_headers import build_upstream_extra_headers


def test_build_upstream_extra_headers_for_codex_openai_cli() -> None:
    request_body = {"model": "gpt-5", "input": [], "prompt_cache_key": "pcache-123"}

    headers = build_upstream_extra_headers(
        provider_type="codex",
        endpoint_sig="openai:cli",
        request_body=request_body,
        original_headers={},
        decrypted_auth_config={"account_id": "acc-1"},
    )

    short_id = hashlib.sha256(b"pcache-123").hexdigest()[:16]
    assert headers == {
        "chatgpt-account-id": "acc-1",
        "session_id": short_id,
        "conversation_id": short_id,
    }


def test_build_upstream_extra_headers_respects_existing_session_headers() -> None:
    headers = build_upstream_extra_headers(
        provider_type="codex",
        endpoint_sig="openai:cli",
        request_body={"model": "gpt-5", "input": [], "prompt_cache_key": "pcache-123"},
        original_headers={
            "Session_ID": "client-session",
            "Conversation_ID": "client-conversation",
        },
        decrypted_auth_config={"account_id": "acc-1"},
    )

    assert headers == {"chatgpt-account-id": "acc-1"}


def test_build_upstream_extra_headers_for_codex_openai_compact() -> None:
    request_body = {"model": "gpt-5", "input": [], "prompt_cache_key": "pcache-123"}

    headers = build_upstream_extra_headers(
        provider_type="codex",
        endpoint_sig="openai:compact",
        request_body=request_body,
        original_headers={},
        decrypted_auth_config={"account_id": "acc-1"},
    )

    short_id = hashlib.sha256(b"pcache-123").hexdigest()[:16]
    assert headers == {
        "chatgpt-account-id": "acc-1",
        "session_id": short_id,
    }


def test_build_upstream_extra_headers_for_legacy_codex_compact_context() -> None:
    request_body = {"model": "gpt-5", "input": [], "prompt_cache_key": "pcache-123"}

    try:
        set_codex_request_context(CodexRequestContext(is_compact=True))
        headers = build_upstream_extra_headers(
            provider_type="codex",
            endpoint_sig="openai:cli",
            request_body=request_body,
            original_headers={},
            decrypted_auth_config={"account_id": "acc-1"},
        )
    finally:
        set_codex_request_context(None)

    short_id = hashlib.sha256(b"pcache-123").hexdigest()[:16]
    assert headers == {
        "chatgpt-account-id": "acc-1",
        "session_id": short_id,
    }


def test_build_upstream_extra_headers_returns_empty_without_match() -> None:
    headers = build_upstream_extra_headers(
        provider_type="codex",
        endpoint_sig="openai:chat",
        request_body={"model": "gpt-5", "messages": []},
        original_headers={},
        decrypted_auth_config={"account_id": "acc-1"},
    )

    assert headers == {}
