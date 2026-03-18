from __future__ import annotations

import pytest

from src.plugins.rate_limit.token_bucket import TokenBucketStrategy


@pytest.mark.asyncio
async def test_token_bucket_cleans_up_expired_buckets(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("RATE_LIMIT_BACKEND", "memory")
    strategy = TokenBucketStrategy()
    strategy.configure({"bucket_expiry": 1, "cleanup_interval": 0})

    await strategy.check_limit("api_key:stale")
    strategy.buckets["api_key:stale"].last_access_time -= 3600

    await strategy.check_limit("api_key:fresh")

    assert "api_key:stale" not in strategy.buckets
    assert "api_key:fresh" in strategy.buckets


@pytest.mark.asyncio
async def test_token_bucket_reconfigures_existing_bucket_when_rate_limit_changes(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("RATE_LIMIT_BACKEND", "memory")
    strategy = TokenBucketStrategy()

    await strategy.check_limit("user:42", rate_limit=120)
    bucket = strategy.buckets["user:42"]
    bucket.tokens = 90

    await strategy.check_limit("user:42", rate_limit=30)

    updated_bucket = strategy.buckets["user:42"]
    assert updated_bucket.capacity == 30
    assert updated_bucket.refill_rate == 0.5
    assert updated_bucket.tokens <= 30
