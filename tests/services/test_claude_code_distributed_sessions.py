from __future__ import annotations

import uuid

import pytest

from src.core.exceptions import ConcurrencyLimitError
from src.services.provider.adapters.claude_code.context import (
    ClaudeCodeRequestContext,
    set_claude_code_request_context,
)
from src.services.provider.adapters.claude_code.envelope import (
    claude_code_envelope,
    enforce_distributed_session_controls,
)


class _StubRedis:
    def __init__(self, *, result=None, exc: Exception | None = None) -> None:
        self._result = result
        self._exc = exc
        self.calls: list[tuple] = []

    async def eval(self, *args):
        self.calls.append(args)
        if self._exc is not None:
            raise self._exc
        return self._result


@pytest.fixture(autouse=True)
def _reset_claude_code_context():
    set_claude_code_request_context(None)
    yield
    set_claude_code_request_context(None)


@pytest.mark.asyncio
async def test_distributed_session_controls_accept_when_redis_allows(monkeypatch: pytest.MonkeyPatch):
    stub = _StubRedis(result=[1, 1])

    async def _fake_get_redis_client(*, require_redis: bool = False):
        _ = require_redis
        return stub

    monkeypatch.setattr(
        "src.services.provider.adapters.claude_code.envelope.get_redis_client",
        _fake_get_redis_client,
    )

    ctx = ClaudeCodeRequestContext(
        scope_key=f"key:test-dist-ok-{uuid.uuid4()}",
        key_id="key-ok",
        max_sessions=1,
        session_idle_timeout_minutes=5,
    )
    request_body = {
        "metadata": {"user_id": "user_a_account_b_session_11111111-1111-1111-1111-111111111111"}
    }

    await enforce_distributed_session_controls(request_body, ctx)
    assert len(stub.calls) == 1


@pytest.mark.asyncio
async def test_distributed_session_controls_reject_when_redis_denies(monkeypatch: pytest.MonkeyPatch):
    stub = _StubRedis(result=[0, 1])

    async def _fake_get_redis_client(*, require_redis: bool = False):
        _ = require_redis
        return stub

    monkeypatch.setattr(
        "src.services.provider.adapters.claude_code.envelope.get_redis_client",
        _fake_get_redis_client,
    )

    ctx = ClaudeCodeRequestContext(
        scope_key=f"key:test-dist-deny-{uuid.uuid4()}",
        key_id="key-deny",
        max_sessions=1,
        session_idle_timeout_minutes=5,
    )
    request_body = {
        "metadata": {"user_id": "user_a_account_b_session_22222222-2222-2222-2222-222222222222"}
    }

    with pytest.raises(ConcurrencyLimitError):
        await enforce_distributed_session_controls(request_body, ctx)


@pytest.mark.asyncio
async def test_distributed_session_controls_fallback_to_local_when_redis_error(
    monkeypatch: pytest.MonkeyPatch,
):
    stub = _StubRedis(exc=RuntimeError("redis unavailable"))

    async def _fake_get_redis_client(*, require_redis: bool = False):
        _ = require_redis
        return stub

    monkeypatch.setattr(
        "src.services.provider.adapters.claude_code.envelope.get_redis_client",
        _fake_get_redis_client,
    )

    ctx = ClaudeCodeRequestContext(
        scope_key=f"key:test-dist-fallback-{uuid.uuid4()}",
        key_id="key-fallback",
        max_sessions=1,
        session_idle_timeout_minutes=5,
    )

    first = {
        "metadata": {"user_id": "user_a_account_b_session_aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"}
    }
    second = {
        "metadata": {"user_id": "user_a_account_b_session_bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb"}
    }

    await enforce_distributed_session_controls(first, ctx)
    with pytest.raises(ConcurrencyLimitError):
        await enforce_distributed_session_controls(second, ctx)


def test_wrap_request_skips_local_limit_when_distributed_store_available(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        "src.services.provider.adapters.claude_code.envelope.get_redis_client_sync",
        lambda: object(),
    )
    scope_key = f"key:test-wrap-skip-{uuid.uuid4()}"
    set_claude_code_request_context(
        ClaudeCodeRequestContext(
            is_stream=False,
            scope_key=scope_key,
            key_id="key-wrap",
            max_sessions=1,
            session_idle_timeout_minutes=5,
        )
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
    claude_code_envelope.wrap_request(
        second,
        model="claude-sonnet-4-5-20250929",
        url_model=None,
        decrypted_auth_config=None,
    )
