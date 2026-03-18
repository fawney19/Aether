from __future__ import annotations

from datetime import datetime, timezone
from types import SimpleNamespace
from typing import Any, cast
from unittest.mock import MagicMock

import pytest

from src.api.admin.endpoints.health import AdminApiFormatHealthMonitorAdapter
from src.api.public.catalog import PublicApiFormatHealthMonitorAdapter


def _build_query(result: object) -> MagicMock:
    query = MagicMock()
    query.join.return_value = query
    query.distinct.return_value = query
    query.filter.return_value = query
    query.group_by.return_value = query
    query.order_by.return_value = query
    query.limit.return_value = query
    query.all.return_value = result
    return query


def _expr_texts(query: MagicMock) -> list[str]:
    return [str(arg) for arg in query.filter.call_args.args]


@pytest.mark.asyncio
async def test_admin_api_format_health_monitor_filters_inactive_sources(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    endpoint_query = _build_query([("openai:compact", "ep-active", "provider-active")])
    key_query = _build_query([("provider-active", ["openai:compact"])])
    status_query = _build_query([("openai:compact", "success", 3)])
    rows_query = _build_query([])

    db = MagicMock()
    db.query.side_effect = [endpoint_query, key_query, status_query, rows_query]

    monkeypatch.setattr(
        "src.api.admin.endpoints.health.EndpointHealthService._generate_timeline_from_usage",
        lambda **_: {
            "timeline": ["healthy"] * 100,
            "time_range_start": None,
            "time_range_end": None,
        },
    )

    context = SimpleNamespace(
        db=db,
        request=SimpleNamespace(state=SimpleNamespace()),
        add_audit_metadata=lambda **kwargs: None,
    )

    adapter = AdminApiFormatHealthMonitorAdapter(lookback_hours=6, per_format_limit=20)
    await adapter.handle(cast(Any, context))

    status_filters = _expr_texts(status_query)
    rows_filters = _expr_texts(rows_query)

    assert any("provider_endpoints.is_active" in expr for expr in status_filters)
    assert any("providers.is_active" in expr for expr in status_filters)
    assert any("provider_endpoints.is_active" in expr for expr in rows_filters)
    assert any("providers.is_active" in expr for expr in rows_filters)


@pytest.mark.asyncio
async def test_public_api_format_health_monitor_uses_real_counts_not_sampled_events(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    now = datetime.now(timezone.utc)
    active_formats_query = _build_query([("openai:compact",)])
    endpoint_rows_query = _build_query([("openai:compact", "ep-active")])
    status_query = _build_query(
        [
            ("openai:compact", "success", 7),
            ("openai:compact", "failed", 3),
            ("openai:compact", "skipped", 5),
        ]
    )
    rows_query = _build_query(
        [
            (
                SimpleNamespace(
                    status="failed",
                    status_code=500,
                    latency_ms=321,
                    error_type="provider_error",
                    finished_at=now,
                    started_at=None,
                    created_at=now,
                ),
                "openai:compact",
            ),
            (
                SimpleNamespace(
                    status="success",
                    status_code=200,
                    latency_ms=123,
                    error_type=None,
                    finished_at=now,
                    started_at=None,
                    created_at=now,
                ),
                "openai:compact",
            ),
        ]
    )

    db = MagicMock()
    db.query.side_effect = [
        active_formats_query,
        endpoint_rows_query,
        status_query,
        rows_query,
    ]

    monkeypatch.setattr(
        "src.api.public.catalog.EndpointHealthService._generate_timeline_from_usage",
        lambda **_: {
            "timeline": ["healthy"] * 100,
            "time_range_start": None,
            "time_range_end": now,
        },
    )
    monkeypatch.setattr(
        "src.core.api_format.get_local_path_for_endpoint",
        lambda api_format: f"/{api_format}",
    )

    context = SimpleNamespace(
        db=db,
        request=SimpleNamespace(state=SimpleNamespace()),
    )

    adapter = PublicApiFormatHealthMonitorAdapter(lookback_hours=6, per_format_limit=20)
    result = await adapter.handle(cast(Any, context))

    monitor = result["formats"][0]
    assert monitor["api_format"] == "openai:compact"
    assert monitor["total_attempts"] == 15
    assert monitor["success_count"] == 7
    assert monitor["failed_count"] == 3
    assert monitor["skipped_count"] == 5
    assert monitor["success_rate"] == pytest.approx(0.7)
    assert len(monitor["events"]) == 2

    rows_filters = _expr_texts(rows_query)
    assert any("provider_endpoints.is_active" in expr for expr in rows_filters)
    assert any("providers.is_active" in expr for expr in rows_filters)
