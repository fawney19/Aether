from __future__ import annotations

from types import SimpleNamespace
from typing import Any, cast
from unittest.mock import AsyncMock, MagicMock

import pytest

from src.models.database import Provider, ProviderAPIKey, ProviderEndpoint
from src.services.orchestration.request_dispatcher import RequestDispatcher
from src.services.scheduling.affinity_manager import CacheAffinity
from src.services.scheduling.aware_scheduler import CacheAwareScheduler
from src.services.scheduling.schemas import ProviderCandidate


def _candidate(
    *,
    provider_id: str,
    endpoint_id: str,
    key_id: str,
    priority: int,
    cache_ttl_minutes: int = 5,
) -> ProviderCandidate:
    provider = cast(
        Provider,
        SimpleNamespace(id=provider_id, name=provider_id, provider_priority=priority),
    )
    endpoint = cast(ProviderEndpoint, SimpleNamespace(id=endpoint_id))
    key = cast(
        ProviderAPIKey,
        SimpleNamespace(
            id=key_id,
            name=key_id,
            internal_priority=0,
            cache_ttl_minutes=cache_ttl_minutes,
        ),
    )
    return ProviderCandidate(
        provider=provider,
        endpoint=endpoint,
        key=key,
        effective_provider_priority=priority,
    )


@pytest.mark.asyncio
async def test_cache_affinity_hit_below_base_first_is_marked_degraded() -> None:
    first = _candidate(provider_id="p1", endpoint_id="e1", key_id="k1", priority=1)
    second = _candidate(provider_id="p2", endpoint_id="e2", key_id="k2", priority=2)
    scheduler = CacheAwareScheduler()
    scheduler._affinity_manager = cast(
        Any,
        SimpleNamespace(
            get_affinity=AsyncMock(
                return_value=CacheAffinity(
                    provider_id="p2",
                    endpoint_id="e2",
                    key_id="k2",
                    api_format="openai:chat",
                    model_name="gm1",
                    created_at=1.0,
                    expire_at=999.0,
                    request_count=3,
                )
            )
        ),
    )

    result = await scheduler._apply_cache_affinity(
        candidates=[first, second],
        db=MagicMock(),
        affinity_key="user-key-1",
        api_format="openai:chat",
        global_model_id="gm1",
    )

    assert result[0] is second
    assert second.is_cached is True
    assert second.cache_affinity_degraded is True
    assert first.cache_affinity_degraded is False


@pytest.mark.asyncio
async def test_cache_affinity_hit_on_base_first_is_not_degraded() -> None:
    first = _candidate(provider_id="p1", endpoint_id="e1", key_id="k1", priority=1)
    second = _candidate(provider_id="p2", endpoint_id="e2", key_id="k2", priority=2)
    scheduler = CacheAwareScheduler()
    scheduler._affinity_manager = cast(
        Any,
        SimpleNamespace(
            get_affinity=AsyncMock(
                return_value=CacheAffinity(
                    provider_id="p1",
                    endpoint_id="e1",
                    key_id="k1",
                    api_format="openai:chat",
                    model_name="gm1",
                    created_at=1.0,
                    expire_at=999.0,
                    request_count=3,
                )
            )
        ),
    )

    result = await scheduler._apply_cache_affinity(
        candidates=[first, second],
        db=MagicMock(),
        affinity_key="user-key-1",
        api_format="openai:chat",
        global_model_id="gm1",
    )

    assert result[0] is first
    assert first.is_cached is True
    assert first.cache_affinity_degraded is False


@pytest.mark.asyncio
async def test_dispatcher_does_not_refresh_degraded_cache_affinity() -> None:
    candidate = _candidate(provider_id="p2", endpoint_id="e2", key_id="k2", priority=2)
    candidate.cache_affinity_degraded = True

    request_executor = MagicMock()
    request_executor.execute = AsyncMock(
        return_value=SimpleNamespace(
            context=SimpleNamespace(elapsed_ms=12),
            response=SimpleNamespace(first_byte_time_ms=7),
        )
    )
    cache_scheduler = MagicMock()
    cache_scheduler.set_cache_affinity = AsyncMock()

    dispatcher = RequestDispatcher(
        db=MagicMock(),
        request_executor=request_executor,
        cache_scheduler=cache_scheduler,
    )

    await dispatcher.dispatch(
        candidate=candidate,
        candidate_index=0,
        retry_index=0,
        candidate_record_id="cand-1",
        user_api_key=None,
        user_id="user-1",
        request_func=AsyncMock(),
        request_id="req-1",
        api_format="openai:chat",
        model_name="gpt-test",
        affinity_key="user-key-1",
        global_model_id="gm1",
        attempt_counter=1,
        max_attempts=3,
    )

    cache_scheduler.set_cache_affinity.assert_not_awaited()
