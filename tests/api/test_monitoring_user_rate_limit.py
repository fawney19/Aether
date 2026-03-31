from __future__ import annotations

from datetime import datetime, timezone
from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock

import pytest

from src.api.monitoring.user import UserRateLimitStatusAdapter


def _build_query_with_keys(keys: list[object]) -> MagicMock:
    query = MagicMock()
    query.filter.return_value.order_by.return_value.all.return_value = keys
    return query


@pytest.mark.asyncio
async def test_rate_limit_status_adapter_reports_user_and_key_layers(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    now = datetime(2026, 3, 13, 12, 0, tzinfo=timezone.utc)
    db = MagicMock()
    user = SimpleNamespace(id="user-1", group=None)
    key = SimpleNamespace(id="key-1", name="Primary", is_standalone=False, rate_limit=10)
    standalone = SimpleNamespace(
        id="skey-1", name="Standalone", is_standalone=True, rate_limit=None
    )

    db.query.return_value = _build_query_with_keys([key, standalone])

    limiter = MagicMock()
    limiter.bucket_seconds = 60
    limiter.get_reset_at.return_value = now
    limiter.get_user_rpm_key.return_value = "rpm:user:user-1:bucket"
    limiter.get_standalone_rpm_key.return_value = "rpm:ukey:skey-1:bucket"
    limiter.get_key_rpm_key.side_effect = lambda key_id: f"rpm:key:{key_id}:bucket"

    async def _get_scope_count(scope_key: str) -> int:
        counts = {
            "rpm:user:user-1:bucket": 55,
            "rpm:key:key-1:bucket": 7,
            "rpm:ukey:skey-1:bucket": 12,
        }
        return counts[scope_key]

    limiter.get_scope_count = AsyncMock(side_effect=_get_scope_count)

    monkeypatch.setattr(
        "src.api.monitoring.user.get_user_rpm_limiter",
        AsyncMock(return_value=limiter),
    )
    monkeypatch.setattr(
        "src.api.monitoring.user.SystemConfigService.get_config",
        lambda *_a, **_k: 60,
    )

    context = SimpleNamespace(db=db, user=user)
    result = await UserRateLimitStatusAdapter().handle(context)

    assert result["user_id"] == "user-1"
    assert result["api_keys"][0] == {
        "api_key_name": "Primary",
        "limit": 10,
        "remaining": 3,
        "scope": "key",
        "reset_time": now.isoformat(),
        "window": "60s",
        "user_limit": 60,
        "user_remaining": 5,
        "key_limit": 10,
        "key_remaining": 3,
    }
    assert result["api_keys"][1] == {
        "api_key_name": "Standalone",
        "limit": 60,
        "remaining": 48,
        "scope": "user",
        "reset_time": now.isoformat(),
        "window": "60s",
        "user_limit": 60,
        "user_remaining": 48,
        "key_limit": None,
        "key_remaining": None,
    }


@pytest.mark.asyncio
async def test_rate_limit_status_adapter_reports_unlimited_key_without_counts(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    user = SimpleNamespace(id="user-1", group=SimpleNamespace(rate_limit=0))
    key = SimpleNamespace(id="key-1", name="Unlimited", is_standalone=False, rate_limit=0)

    db.query.return_value = _build_query_with_keys([key])

    limiter = MagicMock()
    limiter.bucket_seconds = 60
    limiter.get_reset_at.return_value = datetime.now(timezone.utc)
    limiter.get_user_rpm_key.return_value = "rpm:user:user-1:bucket"
    limiter.get_key_rpm_key.return_value = "rpm:key:key-1:bucket"
    limiter.get_scope_count = AsyncMock()

    monkeypatch.setattr(
        "src.api.monitoring.user.get_user_rpm_limiter",
        AsyncMock(return_value=limiter),
    )
    monkeypatch.setattr(
        "src.api.monitoring.user.SystemConfigService.get_config",
        lambda *_a, **_k: 60,
    )

    context = SimpleNamespace(db=db, user=user)
    result = await UserRateLimitStatusAdapter().handle(context)

    assert result["api_keys"][0]["limit"] is None
    assert result["api_keys"][0]["remaining"] is None
    assert result["api_keys"][0]["scope"] is None
    limiter.get_scope_count.assert_not_awaited()


@pytest.mark.asyncio
async def test_rate_limit_status_adapter_uses_group_default_for_user_scope(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    now = datetime(2026, 3, 13, 12, 0, tzinfo=timezone.utc)
    db = MagicMock()
    user = SimpleNamespace(
        id="user-1",
        group=SimpleNamespace(rate_limit=25),
    )
    key = SimpleNamespace(id="key-1", name="Primary", is_standalone=False, rate_limit=0)

    db.query.return_value = _build_query_with_keys([key])

    limiter = MagicMock()
    limiter.bucket_seconds = 60
    limiter.get_reset_at.return_value = now
    limiter.get_user_rpm_key.return_value = "rpm:user:user-1:bucket"
    limiter.get_key_rpm_key.return_value = "rpm:key:key-1:bucket"
    limiter.get_scope_count = AsyncMock(
        side_effect=lambda scope_key: {"rpm:user:user-1:bucket": 8}[scope_key]
    )

    monkeypatch.setattr(
        "src.api.monitoring.user.get_user_rpm_limiter",
        AsyncMock(return_value=limiter),
    )
    monkeypatch.setattr(
        "src.api.monitoring.user.SystemConfigService.get_config",
        lambda *_a, **_k: 60,
    )

    context = SimpleNamespace(db=db, user=user)
    result = await UserRateLimitStatusAdapter().handle(context)

    assert result["api_keys"][0]["user_limit"] == 25
    assert result["api_keys"][0]["user_remaining"] == 17
