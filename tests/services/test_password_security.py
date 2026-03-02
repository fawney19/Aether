"""
密码安全相关测试
"""

from __future__ import annotations

from pathlib import Path

import pytest

from src.core.validators import PasswordValidator
from src.services.rate_limit.account_lockout import AccountLockoutService


class _FakeRedis:
    def __init__(self) -> None:
        self._store: dict[str, int | str] = {}
        self._ttl: dict[str, int] = {}

    async def incr(self, key: str) -> int:
        current = int(self._store.get(key, 0))
        current += 1
        self._store[key] = current
        return current

    async def expire(self, key: str, seconds: int) -> bool:
        if key in self._store:
            self._ttl[key] = seconds
            return True
        return False

    async def ttl(self, key: str) -> int:
        return self._ttl.get(key, -1)

    async def setex(self, key: str, seconds: int, value: str) -> bool:
        self._store[key] = value
        self._ttl[key] = seconds
        return True

    async def delete(self, *keys: str) -> int:
        deleted = 0
        for key in keys:
            if key in self._store:
                del self._store[key]
                deleted += 1
            if key in self._ttl:
                del self._ttl[key]
        return deleted

    async def exists(self, key: str) -> int:
        return 1 if key in self._store else 0


@pytest.fixture(autouse=True)
def _reset_password_validator_state() -> None:
    original_path = PasswordValidator._COMMON_PASSWORDS_PATH
    original_cache = PasswordValidator._COMMON_PASSWORDS
    PasswordValidator._COMMON_PASSWORDS = None
    yield
    PasswordValidator._COMMON_PASSWORDS_PATH = original_path
    PasswordValidator._COMMON_PASSWORDS = original_cache


def test_password_validator_rejects_short_password() -> None:
    valid, error = PasswordValidator.validate("Abc1234")
    assert valid is False
    assert error is not None
    assert "至少为8个字符" in error


def test_password_validator_rejects_blacklisted_password(tmp_path: Path) -> None:
    blacklist = tmp_path / "common_passwords.txt"
    blacklist.write_text("password123\n12345678\n", encoding="utf-8")

    PasswordValidator._COMMON_PASSWORDS_PATH = blacklist
    PasswordValidator._COMMON_PASSWORDS = None

    valid, error = PasswordValidator.validate("Password123")
    assert valid is False
    assert error is not None
    assert "密码过于简单" in error


def test_password_validator_accepts_strong_password(tmp_path: Path) -> None:
    blacklist = tmp_path / "common_passwords.txt"
    blacklist.write_text("password123\n12345678\n", encoding="utf-8")

    PasswordValidator._COMMON_PASSWORDS_PATH = blacklist
    PasswordValidator._COMMON_PASSWORDS = None

    valid, error = PasswordValidator.validate("StrongPass123")
    assert valid is True
    assert error is None


@pytest.mark.asyncio
async def test_account_lockout_triggers_after_max_failed_attempts(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    fake_redis = _FakeRedis()

    async def _get_redis_client(*, require_redis: bool = False) -> _FakeRedis:
        return fake_redis

    monkeypatch.setattr(
        "src.services.rate_limit.account_lockout.get_redis_client", _get_redis_client
    )

    identifier = "test@example.com"

    for expected_count in range(1, AccountLockoutService.MAX_FAILED_ATTEMPTS):
        locked, count = await AccountLockoutService.record_failed_attempt(identifier)
        assert locked is False
        assert count == expected_count

    locked, count = await AccountLockoutService.record_failed_attempt(identifier)
    assert locked is True
    assert count == AccountLockoutService.MAX_FAILED_ATTEMPTS

    is_locked, remaining = await AccountLockoutService.is_locked(identifier)
    assert is_locked is True
    assert remaining == AccountLockoutService.LOCKOUT_DURATION_SECONDS


@pytest.mark.asyncio
async def test_account_lockout_reset_failed_attempts(monkeypatch: pytest.MonkeyPatch) -> None:
    fake_redis = _FakeRedis()

    async def _get_redis_client(*, require_redis: bool = False) -> _FakeRedis:
        return fake_redis

    monkeypatch.setattr(
        "src.services.rate_limit.account_lockout.get_redis_client", _get_redis_client
    )

    identifier = "locked-user"

    for _ in range(AccountLockoutService.MAX_FAILED_ATTEMPTS):
        await AccountLockoutService.record_failed_attempt(identifier)

    await AccountLockoutService.reset_failed_attempts(identifier)

    is_locked, remaining = await AccountLockoutService.is_locked(identifier)
    assert is_locked is False
    assert remaining == 0

    locked, count = await AccountLockoutService.record_failed_attempt(identifier)
    assert locked is False
    assert count == 1


@pytest.mark.asyncio
async def test_account_lockout_fail_open_when_redis_unavailable(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    async def _get_redis_client(*, require_redis: bool = False) -> None:
        return None

    monkeypatch.setattr(
        "src.services.rate_limit.account_lockout.get_redis_client", _get_redis_client
    )

    identifier = "no-redis-user"

    is_locked, remaining = await AccountLockoutService.is_locked(identifier)
    assert is_locked is False
    assert remaining == 0

    locked, count = await AccountLockoutService.record_failed_attempt(identifier)
    assert locked is False
    assert count == 0

    await AccountLockoutService.reset_failed_attempts(identifier)
