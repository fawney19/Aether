"""Tests for pool_manager.py — candidate reordering, success/error hooks."""

from __future__ import annotations

from types import SimpleNamespace
from typing import Any
from unittest.mock import AsyncMock, patch

import pytest

from src.services.provider.pool.config import PoolConfig
from src.services.provider.pool.manager import PoolManager


def _make_candidate(
    key_id: str,
    *,
    is_skipped: bool = False,
    provider_type: str | None = None,
    upstream_metadata: dict[str, Any] | None = None,
) -> Any:
    return SimpleNamespace(
        provider=SimpleNamespace(provider_type=provider_type),
        key=SimpleNamespace(id=key_id, upstream_metadata=upstream_metadata or {}),
        is_skipped=is_skipped,
        skip_reason=None,
    )


def _make_key(
    key_id: str,
    *,
    provider_type: str | None = None,
    upstream_metadata: dict[str, Any] | None = None,
) -> Any:
    return SimpleNamespace(
        id=key_id,
        provider=SimpleNamespace(provider_type=provider_type),
        upstream_metadata=upstream_metadata or {},
    )


@pytest.fixture()
def pool() -> PoolManager:
    return PoolManager("provider-1", PoolConfig())


# ---------------------------------------------------------------------------
# reorder_candidates
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_reorder_empty_candidates(pool: PoolManager) -> None:
    result = await pool.reorder_candidates("sess-1", [])
    assert result == []


@pytest.mark.asyncio
async def test_reorder_sticky_hit_moves_to_front(pool: PoolManager) -> None:
    c1 = _make_candidate("key-1")
    c2 = _make_candidate("key-2")
    c3 = _make_candidate("key-3")

    with (
        patch(
            "src.services.provider.pool.redis_ops.get_sticky_binding",
            new_callable=AsyncMock,
            return_value="key-2",
        ),
        patch(
            "src.services.provider.pool.redis_ops.batch_get_cooldowns",
            new_callable=AsyncMock,
            return_value={"key-1": None, "key-2": None, "key-3": None},
        ),
        patch(
            "src.services.provider.pool.redis_ops.get_lru_scores",
            new_callable=AsyncMock,
            return_value={"key-1": 100.0, "key-2": 200.0, "key-3": 50.0},
        ),
    ):
        result = await pool.reorder_candidates("sess-1", [c1, c2, c3])

    assert result[0].key.id == "key-2"  # sticky hit first


@pytest.mark.asyncio
async def test_reorder_cooldown_keys_are_skipped(pool: PoolManager) -> None:
    c1 = _make_candidate("key-1")
    c2 = _make_candidate("key-2")

    with (
        patch(
            "src.services.provider.pool.redis_ops.get_sticky_binding",
            new_callable=AsyncMock,
            return_value=None,
        ),
        patch(
            "src.services.provider.pool.redis_ops.batch_get_cooldowns",
            new_callable=AsyncMock,
            return_value={"key-1": "rate_limited_429", "key-2": None},
        ),
        patch(
            "src.services.provider.pool.redis_ops.get_lru_scores",
            new_callable=AsyncMock,
            return_value={},
        ),
    ):
        result = await pool.reorder_candidates(None, [c1, c2])

    # key-1 should be skipped
    assert c1.is_skipped is True
    assert "cooldown" in (c1.skip_reason or "")
    # key-2 first in available
    assert result[0].key.id == "key-2"


@pytest.mark.asyncio
async def test_reorder_metadata_quota_exhausted_keys_are_skipped() -> None:
    pool = PoolManager("provider-1", PoolConfig(), provider_type="codex")
    exhausted_meta = {"codex": {"primary_used_percent": 100, "secondary_used_percent": 20}}
    healthy_meta = {"codex": {"primary_used_percent": 20, "secondary_used_percent": 10}}
    c1 = _make_candidate("key-1", provider_type="codex", upstream_metadata=exhausted_meta)
    c2 = _make_candidate("key-2", provider_type="codex", upstream_metadata=healthy_meta)

    with (
        patch(
            "src.services.provider.pool.redis_ops.batch_get_cooldowns",
            new_callable=AsyncMock,
            return_value={"key-1": (None, None), "key-2": (None, None)},
        ),
        patch(
            "src.services.provider.pool.redis_ops.get_lru_scores",
            new_callable=AsyncMock,
            return_value={},
        ),
    ):
        result = await pool.reorder_candidates(None, [c1, c2])

    assert c1.is_skipped is True
    assert c1.skip_reason == "pool cooldown: quota_exhausted"
    assert result[0].key.id == "key-2"


@pytest.mark.asyncio
async def test_reorder_redis_cooldown_reason_has_priority_over_metadata() -> None:
    pool = PoolManager("provider-1", PoolConfig(), provider_type="codex")
    exhausted_meta = {"codex": {"primary_used_percent": 100, "secondary_used_percent": 100}}
    c1 = _make_candidate("key-1", provider_type="codex", upstream_metadata=exhausted_meta)

    with patch(
        "src.services.provider.pool.redis_ops.batch_get_cooldowns",
        new_callable=AsyncMock,
        return_value={"key-1": ("rate_limited_429", 120)},
    ):
        result = await pool.reorder_candidates(None, [c1])

    assert result[0].is_skipped is True
    assert result[0].skip_reason == "pool cooldown: rate_limited_429"
    extra = getattr(result[0], "_pool_extra_data", None) or {}
    assert extra["pool_skip"]["cooldown_reason"] == "rate_limited_429"
    assert extra["pool_skip"]["cooldown_ttl"] == 120


@pytest.mark.asyncio
async def test_reorder_cost_exhausted_keys_are_skipped() -> None:
    pool = PoolManager(
        "provider-1",
        PoolConfig(cost_limit_per_key_tokens=1000, lru_enabled=False),
    )
    c1 = _make_candidate("key-1")
    c2 = _make_candidate("key-2")

    with (
        patch(
            "src.services.provider.pool.redis_ops.get_sticky_binding",
            new_callable=AsyncMock,
            return_value=None,
        ),
        patch(
            "src.services.provider.pool.redis_ops.batch_get_cooldowns",
            new_callable=AsyncMock,
            return_value={"key-1": None, "key-2": None},
        ),
        patch(
            "src.services.provider.pool.redis_ops.batch_get_cost_totals",
            new_callable=AsyncMock,
            return_value={"key-1": 1500, "key-2": 200},
        ),
    ):
        result = await pool.reorder_candidates(None, [c1, c2])

    assert c1.is_skipped is True
    assert "cost" in (c1.skip_reason or "")
    assert result[0].key.id == "key-2"


@pytest.mark.asyncio
async def test_reorder_lru_sorts_least_recently_used_first(
    pool: PoolManager,
) -> None:
    c1 = _make_candidate("key-1")
    c2 = _make_candidate("key-2")
    c3 = _make_candidate("key-3")

    with (
        patch(
            "src.services.provider.pool.redis_ops.get_sticky_binding",
            new_callable=AsyncMock,
            return_value=None,
        ),
        patch(
            "src.services.provider.pool.redis_ops.batch_get_cooldowns",
            new_callable=AsyncMock,
            return_value={"key-1": None, "key-2": None, "key-3": None},
        ),
        patch(
            "src.services.provider.pool.redis_ops.get_lru_scores",
            new_callable=AsyncMock,
            return_value={"key-1": 300.0, "key-2": 100.0, "key-3": 200.0},
        ),
    ):
        result = await pool.reorder_candidates(None, [c1, c2, c3])

    # Least recently used (lowest score) first
    available = [r for r in result if not r.is_skipped]
    assert available[0].key.id == "key-2"
    assert available[1].key.id == "key-3"
    assert available[2].key.id == "key-1"


# ---------------------------------------------------------------------------
# select_key
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_select_key_sticky_metadata_cooldown_falls_back() -> None:
    pool = PoolManager("provider-1", PoolConfig(), provider_type="codex")
    exhausted_meta = {"codex": {"primary_used_percent": 100, "secondary_used_percent": 50}}
    healthy_meta = {"codex": {"primary_used_percent": 10, "secondary_used_percent": 20}}
    k1 = _make_key("key-1", provider_type="codex", upstream_metadata=exhausted_meta)
    k2 = _make_key("key-2", provider_type="codex", upstream_metadata=healthy_meta)

    with (
        patch(
            "src.services.provider.pool.redis_ops.get_sticky_binding",
            new_callable=AsyncMock,
            return_value="key-1",
        ),
        patch(
            "src.services.provider.pool.redis_ops.batch_get_cooldowns",
            new_callable=AsyncMock,
            return_value={"key-1": None, "key-2": None},
        ),
        patch(
            "src.services.provider.pool.redis_ops.get_lru_scores",
            new_callable=AsyncMock,
            return_value={"key-1": 100.0, "key-2": 10.0},
        ),
    ):
        selected = await pool.select_key("sess-1", [k1, k2])

    assert selected is not None
    assert selected.id == "key-2"


# ---------------------------------------------------------------------------
# on_request_success
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_on_request_success_binds_sticky_and_touches_lru(
    pool: PoolManager,
) -> None:
    with (
        patch(
            "src.services.provider.pool.redis_ops.set_sticky_binding",
            new_callable=AsyncMock,
        ) as mock_sticky,
        patch(
            "src.services.provider.pool.redis_ops.touch_lru",
            new_callable=AsyncMock,
        ) as mock_lru,
        patch(
            "src.services.provider.pool.redis_ops.add_cost_entry",
            new_callable=AsyncMock,
        ) as mock_cost,
    ):
        await pool.on_request_success(session_uuid="sess-1", key_id="key-1", tokens_used=0)

    mock_sticky.assert_called_once_with("provider-1", "sess-1", "key-1", 3600)
    mock_lru.assert_called_once_with("provider-1", "key-1")
    mock_cost.assert_not_called()  # tokens_used=0


@pytest.mark.asyncio
async def test_on_request_success_records_cost_when_configured() -> None:
    pool = PoolManager(
        "provider-1",
        PoolConfig(cost_limit_per_key_tokens=50000),
    )
    with (
        patch(
            "src.services.provider.pool.redis_ops.set_sticky_binding",
            new_callable=AsyncMock,
        ),
        patch(
            "src.services.provider.pool.redis_ops.touch_lru",
            new_callable=AsyncMock,
        ),
        patch(
            "src.services.provider.pool.redis_ops.add_cost_entry",
            new_callable=AsyncMock,
        ) as mock_cost,
    ):
        await pool.on_request_success(session_uuid="sess-1", key_id="key-1", tokens_used=500)

    mock_cost.assert_called_once_with("provider-1", "key-1", 500, 18000)


# ---------------------------------------------------------------------------
# on_request_error
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_on_request_error_delegates_to_health_policy(
    pool: PoolManager,
) -> None:
    with patch(
        "src.services.provider.pool.health_policy.apply_health_policy",
        new_callable=AsyncMock,
    ) as mock_hp:
        await pool.on_request_error(
            key_id="key-1", status_code=429, error_body=None, response_headers=None
        )

    mock_hp.assert_called_once()
    call_kwargs = mock_hp.call_args.kwargs
    assert call_kwargs["key_id"] == "key-1"
    assert call_kwargs["status_code"] == 429


# ---------------------------------------------------------------------------
# is_key_schedulable
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_is_key_schedulable_returns_false_when_in_cooldown(
    pool: PoolManager,
) -> None:
    with patch(
        "src.services.provider.pool.redis_ops.get_cooldown",
        new_callable=AsyncMock,
        return_value="rate_limited_429",
    ):
        ok, reason = await pool.is_key_schedulable("key-1")

    assert ok is False
    assert "cooldown" in (reason or "")


@pytest.mark.asyncio
async def test_is_key_schedulable_returns_true_when_healthy(
    pool: PoolManager,
) -> None:
    with patch(
        "src.services.provider.pool.redis_ops.get_cooldown",
        new_callable=AsyncMock,
        return_value=None,
    ):
        ok, reason = await pool.is_key_schedulable("key-1")

    assert ok is True
    assert reason is None
