from __future__ import annotations

import uuid

import pytest

from src.core.exceptions import ConcurrencyLimitError
from src.services.provider.adapters.claude_code.constants import TLS_PROFILE_CLAUDE_CODE
from src.services.provider.adapters.claude_code.context import (
    ClaudeCodeRequestContext,
    build_and_set_claude_code_request_context,
    build_claude_code_request_context,
    get_claude_code_request_context,
    resolve_claude_code_tls_profile,
    set_claude_code_request_context,
)
from src.services.provider.adapters.claude_code.envelope import claude_code_envelope
from src.utils.ssl_utils import get_ssl_context, get_ssl_context_for_profile


@pytest.fixture(autouse=True)
def _reset_claude_code_context():
    set_claude_code_request_context(None)
    yield
    set_claude_code_request_context(None)


def _session_tail(user_id: str) -> str:
    return user_id.split("_session_")[-1]


def test_build_context_reads_claude_code_advanced_from_provider_config() -> None:
    ctx = build_claude_code_request_context(
        provider_config={
            "claude_code_advanced": {
                "max_sessions": 3,
                "session_idle_timeout_minutes": 7,
                "enable_tls_fingerprint": True,
                "session_id_masking_enabled": True,
            }
        },
        key_id="key-123",
        is_stream=False,
    )

    assert ctx.scope_key == "key:key-123"
    assert ctx.key_id == "key-123"
    assert ctx.max_sessions == 3
    assert ctx.session_idle_timeout_minutes == 7
    assert ctx.enable_tls_fingerprint is True
    assert ctx.session_id_masking_enabled is True


def test_build_and_set_context_returns_tls_profile_when_enabled() -> None:
    ctx, tls_profile = build_and_set_claude_code_request_context(
        provider_config={"claude_code_advanced": {"enable_tls_fingerprint": True}},
        key_id="key-tls",
        is_stream=True,
    )

    assert ctx.key_id == "key-tls"
    assert get_claude_code_request_context() == ctx
    assert tls_profile == TLS_PROFILE_CLAUDE_CODE
    assert resolve_claude_code_tls_profile(ctx) == TLS_PROFILE_CLAUDE_CODE


def test_get_ssl_context_for_claude_code_profile_is_cached() -> None:
    first = get_ssl_context_for_profile(TLS_PROFILE_CLAUDE_CODE)
    second = get_ssl_context_for_profile(TLS_PROFILE_CLAUDE_CODE)

    assert first is second


def test_get_ssl_context_for_unknown_profile_falls_back_to_default() -> None:
    assert get_ssl_context_for_profile("unknown_profile") is get_ssl_context()


def test_wrap_request_masks_session_id_when_enabled(monkeypatch: pytest.MonkeyPatch) -> None:
    # Force local session control mode (avoid interference from Redis initialized by other tests)
    monkeypatch.setattr(
        "src.services.provider.adapters.claude_code.envelope._is_distributed_session_control_available",
        lambda: False,
    )
    scope_key = f"key:test-mask-{uuid.uuid4()}"
    set_claude_code_request_context(
        ClaudeCodeRequestContext(
            is_stream=False,
            scope_key=scope_key,
            key_id="key-mask",
            session_id_masking_enabled=True,
        )
    )

    body1 = {
        "metadata": {
            "user_id": "user_client_account_main_session_11111111-1111-1111-1111-111111111111"
        }
    }
    body2 = {
        "metadata": {
            "user_id": "user_client_account_main_session_22222222-2222-2222-2222-222222222222"
        }
    }

    wrapped1, _ = claude_code_envelope.wrap_request(
        body1,
        model="claude-sonnet-4-5-20250929",
        url_model=None,
        decrypted_auth_config=None,
    )
    wrapped2, _ = claude_code_envelope.wrap_request(
        body2,
        model="claude-sonnet-4-5-20250929",
        url_model=None,
        decrypted_auth_config=None,
    )

    tail1 = _session_tail(wrapped1["metadata"]["user_id"])
    tail2 = _session_tail(wrapped2["metadata"]["user_id"])

    assert tail1 != "11111111-1111-1111-1111-111111111111"
    assert tail1 == tail2


def test_wrap_request_enforces_max_sessions(monkeypatch: pytest.MonkeyPatch) -> None:
    # Force local session control mode (avoid interference from Redis initialized by other tests)
    monkeypatch.setattr(
        "src.services.provider.adapters.claude_code.envelope._is_distributed_session_control_available",
        lambda: False,
    )
    scope_key = f"key:test-limit-{uuid.uuid4()}"
    set_claude_code_request_context(
        ClaudeCodeRequestContext(
            is_stream=False,
            scope_key=scope_key,
            key_id="key-limit",
            max_sessions=1,
            session_idle_timeout_minutes=5,
        )
    )

    first = {
        "metadata": {"user_id": "user_a_account_b_session_aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"}
    }
    second = {
        "metadata": {"user_id": "user_a_account_b_session_bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb"}
    }

    claude_code_envelope.wrap_request(
        first,
        model="claude-sonnet-4-5-20250929",
        url_model=None,
        decrypted_auth_config=None,
    )

    with pytest.raises(ConcurrencyLimitError):
        claude_code_envelope.wrap_request(
            second,
            model="claude-sonnet-4-5-20250929",
            url_model=None,
            decrypted_auth_config=None,
        )


def test_wrap_request_releases_expired_sessions(monkeypatch: pytest.MonkeyPatch) -> None:
    # Force local session control mode
    monkeypatch.setattr(
        "src.services.provider.adapters.claude_code.envelope._is_distributed_session_control_available",
        lambda: False,
    )
    scope_key = f"key:test-expire-{uuid.uuid4()}"
    set_claude_code_request_context(
        ClaudeCodeRequestContext(
            is_stream=False,
            scope_key=scope_key,
            key_id="key-expire",
            max_sessions=1,
            session_idle_timeout_minutes=1,
        )
    )

    ticks = iter([0.0, 61.0])
    monkeypatch.setattr(
        "src.services.provider.adapters.claude_code.envelope.time.monotonic",
        lambda: next(ticks),
    )

    first = {
        "metadata": {"user_id": "user_a_account_b_session_11111111-1111-1111-1111-111111111111"}
    }
    second = {
        "metadata": {"user_id": "user_a_account_b_session_22222222-2222-2222-2222-222222222222"}
    }

    claude_code_envelope.wrap_request(
        first,
        model="claude-sonnet-4-5-20250929",
        url_model=None,
        decrypted_auth_config=None,
    )
    # 61s > 1min idle timeout，旧会话应过期，第二个会话可进入。
    claude_code_envelope.wrap_request(
        second,
        model="claude-sonnet-4-5-20250929",
        url_model=None,
        decrypted_auth_config=None,
    )
