from __future__ import annotations

from unittest.mock import AsyncMock, MagicMock

import pytest

from src.services.rate_limit.user_rpm_limiter import RpmCheckResult, UserRpmLimiter


@pytest.fixture
async def limiter() -> UserRpmLimiter:
    limiter = UserRpmLimiter()
    limiter._redis = None
    limiter._memory_counts.clear()
    await limiter.close()
    yield limiter
    limiter._memory_counts.clear()
    limiter._redis = None
    await limiter.close()


@pytest.mark.asyncio
async def test_check_and_consume_enforces_user_scope_in_memory(
    limiter: UserRpmLimiter,
) -> None:
    user_key = limiter.get_user_rpm_key("user-1")
    key_key = limiter.get_key_rpm_key("key-1")

    first = await limiter.check_and_consume(
        user_rpm_key=user_key,
        user_rpm_limit=1,
        key_rpm_key=key_key,
        key_rpm_limit=0,
    )
    second = await limiter.check_and_consume(
        user_rpm_key=user_key,
        user_rpm_limit=1,
        key_rpm_key=key_key,
        key_rpm_limit=0,
    )

    assert first == RpmCheckResult(allowed=True, remaining=0)
    assert second.allowed is False
    assert second.scope == "user"
    assert second.limit == 1
    assert second.remaining == 0


@pytest.mark.asyncio
async def test_check_and_consume_enforces_key_scope_in_memory(
    limiter: UserRpmLimiter,
) -> None:
    user_key = limiter.get_user_rpm_key("user-1")
    key_key = limiter.get_key_rpm_key("key-1")

    first = await limiter.check_and_consume(
        user_rpm_key=user_key,
        user_rpm_limit=3,
        key_rpm_key=key_key,
        key_rpm_limit=1,
    )
    second = await limiter.check_and_consume(
        user_rpm_key=user_key,
        user_rpm_limit=3,
        key_rpm_key=key_key,
        key_rpm_limit=1,
    )

    assert first.allowed is True
    assert second.allowed is False
    assert second.scope == "key"
    assert second.limit == 1


@pytest.mark.asyncio
async def test_check_and_consume_redis_failure_falls_back_to_memory_when_fail_close(
    monkeypatch: pytest.MonkeyPatch,
    limiter: UserRpmLimiter,
) -> None:
    fake_redis = MagicMock()
    fake_redis.eval = AsyncMock(side_effect=RuntimeError("redis down"))
    limiter._redis = fake_redis

    monkeypatch.setattr(
        "src.services.rate_limit.user_rpm_limiter.config.rate_limit_fail_open", False
    )

    user_key = limiter.get_user_rpm_key("user-1")
    key_key = limiter.get_key_rpm_key("key-1")

    first = await limiter.check_and_consume(
        user_rpm_key=user_key,
        user_rpm_limit=1,
        key_rpm_key=key_key,
        key_rpm_limit=0,
    )
    second = await limiter.check_and_consume(
        user_rpm_key=user_key,
        user_rpm_limit=1,
        key_rpm_key=key_key,
        key_rpm_limit=0,
    )

    assert first.allowed is True
    assert second.allowed is False
    assert second.scope == "user"


@pytest.mark.asyncio
async def test_check_and_consume_redis_failure_fail_open_allows_request(
    monkeypatch: pytest.MonkeyPatch,
    limiter: UserRpmLimiter,
) -> None:
    fake_redis = MagicMock()
    fake_redis.eval = AsyncMock(side_effect=RuntimeError("redis down"))
    limiter._redis = fake_redis

    monkeypatch.setattr(
        "src.services.rate_limit.user_rpm_limiter.config.rate_limit_fail_open", True
    )

    result = await limiter.check_and_consume(
        user_rpm_key=limiter.get_user_rpm_key("user-1"),
        user_rpm_limit=1,
        key_rpm_key=limiter.get_key_rpm_key("key-1"),
        key_rpm_limit=0,
    )

    assert result.allowed is True


@pytest.mark.asyncio
async def test_check_and_consume_skips_when_all_limits_are_zero(
    limiter: UserRpmLimiter,
) -> None:
    result = await limiter.check_and_consume(
        user_rpm_key=limiter.get_user_rpm_key("user-1"),
        user_rpm_limit=0,
        key_rpm_key=limiter.get_key_rpm_key("key-1"),
        key_rpm_limit=0,
    )

    assert result.allowed is True
    assert result.scope is None
    assert limiter._memory_counts == {}
