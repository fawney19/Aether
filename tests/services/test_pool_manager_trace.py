"""Tests for pool manager trace data collection and attachment."""

from __future__ import annotations

from types import SimpleNamespace
from unittest.mock import AsyncMock, patch

import pytest

from src.services.provider.pool.config import PoolConfig
from src.services.provider.pool.manager import PoolManager
from src.services.provider.pool.trace import PoolSchedulingTrace


def _make_candidate(key_id: str, *, is_skipped: bool = False) -> SimpleNamespace:
    return SimpleNamespace(
        key=SimpleNamespace(id=key_id),
        is_skipped=is_skipped,
        skip_reason=None,
    )


# ---------------------------------------------------------------------------
# Trace attachment to candidates
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_trace_attached_to_first_candidate() -> None:
    pool = PoolManager("prov-1", PoolConfig())
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
            "src.services.provider.pool.redis_ops.get_lru_scores",
            new_callable=AsyncMock,
            return_value={"key-1": 100.0, "key-2": 50.0},
        ),
    ):
        result = await pool.reorder_candidates("sess-1", [c1, c2])

    # First candidate should have _pool_scheduling_trace
    trace = getattr(result[0], "_pool_scheduling_trace", None)
    assert trace is not None
    assert isinstance(trace, PoolSchedulingTrace)
    assert trace.total_keys == 2
    assert trace.provider_id == "prov-1"


@pytest.mark.asyncio
async def test_pool_extra_data_on_selected_candidate() -> None:
    pool = PoolManager("prov-1", PoolConfig())
    c1 = _make_candidate("key-1")

    with (
        patch(
            "src.services.provider.pool.redis_ops.get_sticky_binding",
            new_callable=AsyncMock,
            return_value=None,
        ),
        patch(
            "src.services.provider.pool.redis_ops.batch_get_cooldowns",
            new_callable=AsyncMock,
            return_value={"key-1": None},
        ),
        patch(
            "src.services.provider.pool.redis_ops.get_lru_scores",
            new_callable=AsyncMock,
            return_value={},
        ),
    ):
        result = await pool.reorder_candidates(None, [c1])

    extra = getattr(result[0], "_pool_extra_data", None)
    assert extra is not None
    assert "pool_selection" in extra


@pytest.mark.asyncio
async def test_pool_extra_data_on_skipped_candidate() -> None:
    pool = PoolManager("prov-1", PoolConfig())
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
            return_value={
                "key-1": "rate_limited_429",
                "key-2": None,
            },
        ),
        patch(
            "src.services.provider.pool.redis_ops.get_lru_scores",
            new_callable=AsyncMock,
            return_value={},
        ),
    ):
        result = await pool.reorder_candidates(None, [c1, c2])

    # key-1 is skipped with pool_skip extra data
    skipped = [r for r in result if r.is_skipped]
    assert len(skipped) == 1
    extra = getattr(skipped[0], "_pool_extra_data", None)
    assert extra is not None
    assert extra["pool_skip"]["type"] == "cooldown"
    assert extra["pool_skip"]["cooldown_reason"] == "rate_limited_429"
    # TTL is not fetched on the scheduling hot path (include_ttl=False)
    assert "cooldown_ttl" not in extra["pool_skip"]


@pytest.mark.asyncio
async def test_trace_build_summary_matches() -> None:
    pool = PoolManager("prov-1", PoolConfig(cost_limit_per_key_tokens=1000))
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
            return_value={
                "key-1": "overloaded_529",
                "key-2": None,
                "key-3": None,
            },
        ),
        patch(
            "src.services.provider.pool.redis_ops.batch_get_cost_totals",
            new_callable=AsyncMock,
            return_value={"key-2": 1500, "key-3": 200},
        ),
    ):
        result = await pool.reorder_candidates(None, [c1, c2, c3])

    trace = getattr(result[0], "_pool_scheduling_trace", None)
    assert trace is not None

    summary = trace.build_summary(success_key_id="key-3")
    assert summary["total_keys"] == 3
    assert summary["skipped_cooldown"] == 1  # key-1
    assert summary["skipped_cost"] == 1  # key-2
    assert summary["attempted"] == 1  # key-3
    assert summary["success_key_id"] == "key-3"[:8]


@pytest.mark.asyncio
async def test_trace_build_summary_uses_attempted_key_ids_when_provided() -> None:
    pool = PoolManager("prov-1", PoolConfig())
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
            "src.services.provider.pool.redis_ops.get_lru_scores",
            new_callable=AsyncMock,
            return_value={},
        ),
    ):
        result = await pool.reorder_candidates(None, [c1, c2])

    trace = getattr(result[0], "_pool_scheduling_trace", None)
    assert trace is not None

    summary = trace.build_summary(
        success_key_id="key-1",
        attempted_key_ids={"key-1"},
    )
    assert summary["total_keys"] == 2
    assert summary["attempted"] == 1
    assert summary["success_key_id"] == "key-1"[:8]


@pytest.mark.asyncio
async def test_sticky_trace_info() -> None:
    pool = PoolManager("prov-1", PoolConfig(sticky_session_ttl_seconds=3600))
    c1 = _make_candidate("key-1")
    c2 = _make_candidate("key-2")

    with (
        patch(
            "src.services.provider.pool.redis_ops.get_sticky_binding",
            new_callable=AsyncMock,
            return_value="key-2",
        ),
        patch(
            "src.services.provider.pool.redis_ops.batch_get_cooldowns",
            new_callable=AsyncMock,
            return_value={"key-1": None, "key-2": None},
        ),
        patch(
            "src.services.provider.pool.redis_ops.get_lru_scores",
            new_callable=AsyncMock,
            return_value={},
        ),
    ):
        result = await pool.reorder_candidates("sess-1", [c1, c2])

    trace = getattr(result[0], "_pool_scheduling_trace", None)
    assert trace is not None
    assert trace.sticky_session_used is True

    # key-2 (sticky hit) should be first
    assert result[0].key.id == "key-2"
    extra = getattr(result[0], "_pool_extra_data", None)
    assert extra is not None
    assert extra["pool_selection"]["sticky_hit"] is True
