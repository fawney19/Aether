from __future__ import annotations

from types import SimpleNamespace
from unittest.mock import MagicMock

import pytest

from src.api.user_me.routes import GetUsageAdapter
from src.core.enums import UserRole


@pytest.mark.asyncio
async def test_get_usage_adapter_uses_coarse_summary_grouping(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    query = MagicMock()
    count_query = MagicMock()
    count_query.scalar.return_value = 0
    query.outerjoin.return_value = query
    query.filter.return_value = query
    query.with_entities.return_value = count_query
    query.options.return_value = query
    query.order_by.return_value = query
    query.offset.return_value = query
    query.limit.return_value = query
    query.all.return_value = []
    db.query.return_value = query

    summary_getter = MagicMock(
        return_value=[
            {
                "provider": "provider-a",
                "model": "gpt-4o",
                "requests": 2,
                "input_tokens": 10,
                "output_tokens": 5,
                "total_tokens": 15,
                "cache_read_tokens": 5,
                "total_cost_usd": 1.5,
                "actual_total_cost_usd": 1.2,
                "success_count": 2,
                "success_response_time_sum_ms": 1000.0,
                "success_response_time_count": 2,
            },
            {
                "provider": "pending",
                "model": "gpt-4o",
                "requests": 99,
                "input_tokens": 999,
                "output_tokens": 999,
                "total_tokens": 1998,
                "cache_read_tokens": 50,
                "total_cost_usd": 9.9,
                "actual_total_cost_usd": 9.9,
                "success_count": 0,
                "success_response_time_sum_ms": 0.0,
                "success_response_time_count": 0,
            },
        ]
    )
    monkeypatch.setattr("src.api.user_me.routes.UsageService.get_usage_summary", summary_getter)
    monkeypatch.setattr("src.api.user_me.routes.WalletService.get_wallet", lambda *_a, **_k: None)
    monkeypatch.setattr(
        "src.api.user_me.routes.WalletService.serialize_wallet_summary",
        lambda _wallet: {"limit_mode": "finite"},
    )

    adapter = GetUsageAdapter(time_range=None, limit=20, offset=0)
    context = SimpleNamespace(
        db=db,
        user=SimpleNamespace(id="user-1", role=UserRole.USER),
        request=SimpleNamespace(state=SimpleNamespace()),
    )

    result = await adapter.handle(context)

    assert result["total_requests"] == 2
    assert result["total_tokens"] == 15
    assert result["summary_by_model"] == [
        {
            "model": "gpt-4o",
            "requests": 2,
            "input_tokens": 10,
            "output_tokens": 5,
            "total_tokens": 15,
            "cache_read_tokens": 5,
            "cache_hit_rate": 33.33,
            "total_cost_usd": 1.5,
        }
    ]
    assert "total_actual_cost" not in result
    assert result["summary_by_provider"] == [
        {
            "provider": "provider-a",
            "requests": 2,
            "total_tokens": 15,
            "cache_read_tokens": 5,
            "cache_hit_rate": 33.33,
            "total_cost_usd": 1.5,
            "success_rate": 100.0,
            "avg_response_time_ms": 500.0,
        }
    ]
    assert summary_getter.call_args.kwargs["group_by"] is None


@pytest.mark.asyncio
async def test_get_usage_adapter_provider_success_rate_uses_success_count(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    query = MagicMock()
    count_query = MagicMock()
    count_query.scalar.return_value = 0
    query.outerjoin.return_value = query
    query.filter.return_value = query
    query.with_entities.return_value = count_query
    query.options.return_value = query
    query.order_by.return_value = query
    query.offset.return_value = query
    query.limit.return_value = query
    query.all.return_value = []
    db.query.return_value = query

    summary_getter = MagicMock(
        return_value=[
            {
                "provider": "provider-a",
                "model": "gpt-4o",
                "requests": 3,
                "input_tokens": 30,
                "output_tokens": 15,
                "total_tokens": 45,
                "cache_read_tokens": 15,
                "total_cost_usd": 4.5,
                "actual_total_cost_usd": 4.5,
                "success_count": 2,
                "success_response_time_sum_ms": 600.0,
                "success_response_time_count": 2,
            },
            {
                "provider": "provider-a",
                "model": "gpt-4.1",
                "requests": 1,
                "input_tokens": 10,
                "output_tokens": 5,
                "total_tokens": 15,
                "cache_read_tokens": 5,
                "total_cost_usd": 1.5,
                "actual_total_cost_usd": 1.5,
                "success_count": 0,
                "success_response_time_sum_ms": 0.0,
                "success_response_time_count": 0,
            },
        ]
    )
    monkeypatch.setattr("src.api.user_me.routes.UsageService.get_usage_summary", summary_getter)
    monkeypatch.setattr("src.api.user_me.routes.WalletService.get_wallet", lambda *_a, **_k: None)
    monkeypatch.setattr(
        "src.api.user_me.routes.WalletService.serialize_wallet_summary",
        lambda _wallet: {"limit_mode": "finite"},
    )

    adapter = GetUsageAdapter(time_range=None, limit=20, offset=0)
    context = SimpleNamespace(
        db=db,
        user=SimpleNamespace(id="user-1", role=UserRole.USER),
        request=SimpleNamespace(state=SimpleNamespace()),
    )

    result = await adapter.handle(context)

    assert result["summary_by_provider"] == [
        {
            "provider": "provider-a",
            "requests": 4,
            "total_tokens": 60,
            "cache_read_tokens": 20,
            "cache_hit_rate": 33.33,
            "total_cost_usd": 6.0,
            "success_rate": 50.0,
            "avg_response_time_ms": 300.0,
        }
    ]
