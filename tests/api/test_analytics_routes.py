from __future__ import annotations

from datetime import date
from types import SimpleNamespace

import pytest
from fastapi import HTTPException

from src.api.analytics.routes import (
    AnalyticsActiveRequestsRequest,
    AnalyticsBaseRequest,
    AnalyticsBreakdownRequest,
    AnalyticsHeatmapRequest,
    AnalyticsIntervalTimelineRequest,
    AnalyticsLeaderboardRequest,
    AnalyticsRecordsRequest,
    analytics_active_requests,
    analytics_breakdown,
    analytics_filter_options,
    analytics_heatmap,
    analytics_interval_timeline,
    analytics_leaderboard,
    analytics_overview,
    analytics_performance,
    analytics_records,
)
from src.core.enums import UserRole


@pytest.mark.asyncio
async def test_analytics_overview_wraps_query_context(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(
        "src.api.analytics.routes.AnalyticsQueryService.overview",
        lambda *_args, **_kwargs: {
            "summary": {
                "requests_total": 1,
                "actual_total_cost_usd": 12.3,
                "actual_cache_cost_usd": 4.5,
            },
            "composition": {"token_segments": [], "cost_segments": []},
        },
    )

    user = SimpleNamespace(role=UserRole.USER)
    result = await analytics_overview(AnalyticsBaseRequest(), current_user=user, db=SimpleNamespace())

    assert result["query_context"]["scope"]["kind"] == "me"
    assert result["summary"]["requests_total"] == 1
    assert result["summary"]["actual_total_cost_usd"] == 0.0
    assert result["summary"]["actual_cache_cost_usd"] == 0.0


@pytest.mark.asyncio
async def test_analytics_overview_custom_single_day_hour_does_not_fallback_to_last30days(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    captured: dict[str, object] = {}

    def fake_overview(*_args, **kwargs):
        captured["time_range"] = kwargs["time_range"]
        return {"summary": {"requests_total": 1}, "composition": {"token_segments": [], "cost_segments": []}}

    monkeypatch.setattr("src.api.analytics.routes.AnalyticsQueryService.overview", fake_overview)

    body = AnalyticsBaseRequest(
        time_range={
            "start_date": date(2026, 3, 18),
            "end_date": date(2026, 3, 18),
            "granularity": "hour",
            "timezone": "Asia/Shanghai",
            "tz_offset_minutes": 480,
        }
    )
    user = SimpleNamespace(role=UserRole.USER)

    result = await analytics_overview(body, current_user=user, db=SimpleNamespace())

    time_range = captured["time_range"]
    assert time_range.start_date == date(2026, 3, 18)
    assert time_range.end_date == date(2026, 3, 18)
    assert time_range.preset is None
    assert time_range.granularity == "hour"
    assert result["summary"]["requests_total"] == 1


@pytest.mark.asyncio
async def test_analytics_global_scope_requires_admin() -> None:
    body = AnalyticsBaseRequest(scope={"kind": "global"})
    user = SimpleNamespace(role=UserRole.USER)

    with pytest.raises(HTTPException) as exc:
        await analytics_overview(body, current_user=user, db=SimpleNamespace())

    assert exc.value.status_code == 403


@pytest.mark.asyncio
async def test_analytics_records_delegates_with_pagination(monkeypatch: pytest.MonkeyPatch) -> None:
    called: dict[str, object] = {}

    def fake_records(*_args, **kwargs):
        called.update(kwargs)
        return {"total": 0, "limit": kwargs["limit"], "offset": kwargs["offset"], "records": []}

    monkeypatch.setattr("src.api.analytics.routes.AnalyticsQueryService.records", fake_records)

    body = AnalyticsRecordsRequest(
        pagination={"limit": 25, "offset": 50},
        search={"text": "gpt"},
        filters={"api_key_ids": ["key-1"]},
    )
    user = SimpleNamespace(role=UserRole.USER)
    result = await analytics_records(body, current_user=user, db=SimpleNamespace())

    assert called["limit"] == 25
    assert called["offset"] == 50
    assert called["filters"].api_key_ids == ["key-1"]
    assert result["records"] == []


@pytest.mark.asyncio
async def test_analytics_records_redacts_actual_cost_fields_for_non_admin(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        "src.api.analytics.routes.AnalyticsQueryService.records",
        lambda *_args, **_kwargs: {
            "total": 1,
            "limit": 1,
            "offset": 0,
            "records": [
                {
                    "id": "usage-1",
                    "actual_total_cost_usd": 8.8,
                    "actual_cache_cost_usd": 1.1,
                    "rate_multiplier": 2.0,
                    "provider_name": "openai",
                    "provider_api_key_name": "pool-key-a",
                }
            ],
        },
    )

    user = SimpleNamespace(role=UserRole.USER)
    result = await analytics_records(AnalyticsRecordsRequest(), current_user=user, db=SimpleNamespace())

    assert result["records"][0]["actual_total_cost_usd"] == 0.0
    assert result["records"][0]["actual_cache_cost_usd"] == 0.0
    assert result["records"][0]["rate_multiplier"] == 1.0
    assert result["records"][0]["provider_name"] is None
    assert result["records"][0]["provider_api_key_name"] is None


@pytest.mark.asyncio
async def test_analytics_heatmap_resolves_deleted_scope_targets(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    captured: dict[str, object] = {}

    async def fake_get_cached_heatmap(**kwargs):
        captured.update(kwargs)
        return {"days": []}

    monkeypatch.setattr("src.api.analytics.routes.UsageService.get_cached_heatmap", fake_get_cached_heatmap)

    admin = SimpleNamespace(role=UserRole.ADMIN, id="admin-1")
    body = AnalyticsHeatmapRequest(
        scope={"kind": "user", "user_id": "__deleted_user__"},
        api_key_id="__deleted_api_key__",
    )

    result = await analytics_heatmap(body, current_user=admin, db=SimpleNamespace())

    assert result == {"days": []}
    assert captured["user_id"] is None
    assert captured["api_key_id"] is None
    assert captured["deleted_user_only"] is True
    assert captured["deleted_api_key_only"] is True
    assert captured["include_actual_cost"] is True


@pytest.mark.asyncio
async def test_analytics_active_requests_uses_me_scope_and_normalizes_ids(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    captured: dict[str, object] = {}

    def fake_get_active_requests_status(**kwargs):
        captured.update(kwargs)
        return [{"id": "req-1", "provider_name": "openai", "actual_total_cost_usd": 3.2}]

    monkeypatch.setattr(
        "src.api.analytics.routes.UsageService.get_active_requests_status",
        fake_get_active_requests_status,
    )

    user = SimpleNamespace(role=UserRole.USER, id="user-1")
    body = AnalyticsActiveRequestsRequest(ids=[" req-1 ", "", "req-1", "req-2"])

    result = await analytics_active_requests(body, current_user=user, db=SimpleNamespace())

    assert captured["user_id"] == "user-1"
    assert captured["api_key_id"] is None
    assert captured["ids"] == ["req-1", "req-2"]
    assert captured["include_admin_fields"] is False
    assert captured["maintain_status"] is True
    assert result["requests"][0]["provider_name"] is None
    assert result["requests"][0]["actual_total_cost_usd"] == 0.0


@pytest.mark.asyncio
@pytest.mark.parametrize("dimension", ["provider", "api_key", "user"])
async def test_analytics_breakdown_passes_dimension(
    monkeypatch: pytest.MonkeyPatch,
    dimension: str,
) -> None:
    called: dict[str, object] = {}

    def fake_breakdown(*_args, **kwargs):
        called.update(kwargs)
        return {"dimension": kwargs["dimension"], "metric": kwargs["metric"], "rows": []}

    monkeypatch.setattr("src.api.analytics.routes.AnalyticsQueryService.breakdown", fake_breakdown)

    body = AnalyticsBreakdownRequest(dimension=dimension, metric="actual_total_cost_usd", limit=12)
    admin = SimpleNamespace(role=UserRole.ADMIN)
    result = await analytics_breakdown(body, current_user=admin, db=SimpleNamespace())

    assert called["dimension"] == dimension
    assert called["metric"] == "actual_total_cost_usd"
    assert called["limit"] == 12
    assert result["dimension"] == dimension


@pytest.mark.asyncio
async def test_analytics_breakdown_forbids_provider_dimension_for_non_admin() -> None:
    body = AnalyticsBreakdownRequest(dimension="provider")
    user = SimpleNamespace(role=UserRole.USER)

    with pytest.raises(HTTPException) as exc:
        await analytics_breakdown(body, current_user=user, db=SimpleNamespace())

    assert exc.value.status_code == 403


@pytest.mark.asyncio
async def test_analytics_breakdown_forbids_actual_cost_metric_for_non_admin() -> None:
    body = AnalyticsBreakdownRequest(dimension="model", metric="actual_total_cost_usd")
    user = SimpleNamespace(role=UserRole.USER)

    with pytest.raises(HTTPException) as exc:
        await analytics_breakdown(body, current_user=user, db=SimpleNamespace())

    assert exc.value.status_code == 403


@pytest.mark.asyncio
async def test_analytics_filter_options_returns_service_payload(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(
        "src.api.analytics.routes.AnalyticsQueryService.filter_options",
        lambda *_args, **_kwargs: {"models": [{"value": "gpt-5.1", "label": "gpt-5.1"}]},
    )

    admin = SimpleNamespace(role=UserRole.ADMIN)
    result = await analytics_filter_options(AnalyticsBaseRequest(scope={"kind": "global"}), current_user=admin, db=SimpleNamespace())

    assert result["models"][0]["value"] == "gpt-5.1"


@pytest.mark.asyncio
async def test_analytics_interval_timeline_rejects_api_key_scope() -> None:
    admin = SimpleNamespace(role=UserRole.ADMIN, id="admin-1")
    body = AnalyticsIntervalTimelineRequest(scope={"kind": "api_key", "api_key_id": "key-1"})

    with pytest.raises(HTTPException) as exc:
        await analytics_interval_timeline(body, current_user=admin, db=SimpleNamespace())

    assert exc.value.status_code == 400


@pytest.mark.asyncio
async def test_analytics_interval_timeline_passes_include_user_info_for_admin_global(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    captured: dict[str, object] = {}

    def fake_get_interval_timeline(**kwargs):
        captured.update(kwargs)
        return {"points": [], "analysis_period_hours": 24, "total_points": 0}

    monkeypatch.setattr(
        "src.api.analytics.routes.UsageService.get_interval_timeline",
        fake_get_interval_timeline,
    )

    admin = SimpleNamespace(role=UserRole.ADMIN, id="admin-1")
    body = AnalyticsIntervalTimelineRequest(
        scope={"kind": "global"},
        hours=48,
        limit=1500,
        include_user_info=True,
    )

    result = await analytics_interval_timeline(body, current_user=admin, db=SimpleNamespace())

    assert result["analysis_period_hours"] == 24
    assert captured["hours"] == 48
    assert captured["limit"] == 1500
    assert captured["user_id"] is None
    assert captured["include_user_info"] is True


@pytest.mark.asyncio
async def test_analytics_filter_options_hides_provider_options_for_non_admin(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        "src.api.analytics.routes.AnalyticsQueryService.filter_options",
        lambda *_args, **_kwargs: {
            "providers": [{"value": "openai", "label": "OpenAI"}],
            "models": [{"value": "gpt-5.1", "label": "gpt-5.1"}],
        },
    )

    user = SimpleNamespace(role=UserRole.USER)
    result = await analytics_filter_options(
        AnalyticsBaseRequest(),
        current_user=user,
        db=SimpleNamespace(),
    )

    assert result["providers"] == []
    assert result["models"][0]["value"] == "gpt-5.1"


@pytest.mark.asyncio
async def test_analytics_leaderboard_delegates_entity_and_metric(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    called: dict[str, object] = {}

    def fake_leaderboard(*_args, **kwargs):
        called.update(kwargs)
        return {"entity": kwargs["entity"], "metric": kwargs["metric"], "items": []}

    monkeypatch.setattr("src.api.analytics.routes.AnalyticsQueryService.leaderboard", fake_leaderboard)

    admin = SimpleNamespace(role=UserRole.ADMIN)
    body = AnalyticsLeaderboardRequest(scope={"kind": "global"}, entity="api_key", metric="total_tokens", limit=15)
    result = await analytics_leaderboard(body, current_user=admin, db=SimpleNamespace())

    assert called["entity"] == "api_key"
    assert called["metric"] == "total_tokens"
    assert called["limit"] == 15
    assert result["entity"] == "api_key"


@pytest.mark.asyncio
async def test_analytics_performance_delegates_service(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(
        "src.api.analytics.routes.AnalyticsQueryService.performance",
        lambda *_args, **_kwargs: {
            "latency": {
                "response_time_ms": {"avg": 10, "p50": 10, "p90": 20, "p99": 30},
                "first_byte_time_ms": {"avg": 5, "p50": 5, "p90": 10, "p99": 15},
            },
            "percentiles": [],
            "errors": {"total": 0, "rate": 0, "categories": [], "trend": []},
            "provider_health": [],
        },
    )

    admin = SimpleNamespace(role=UserRole.ADMIN)
    result = await analytics_performance(AnalyticsBaseRequest(scope={"kind": "global"}), current_user=admin, db=SimpleNamespace())

    assert result["latency"]["response_time_ms"]["avg"] == 10


@pytest.mark.asyncio
async def test_analytics_performance_hides_provider_health_for_non_admin(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        "src.api.analytics.routes.AnalyticsQueryService.performance",
        lambda *_args, **_kwargs: {
            "latency": {
                "response_time_ms": {"avg": 10, "p50": 10, "p90": 20, "p99": 30},
                "first_byte_time_ms": {"avg": 5, "p50": 5, "p90": 10, "p99": 15},
            },
            "percentiles": [],
            "errors": {"total": 0, "rate": 0, "categories": [], "trend": []},
            "provider_health": [{"provider_name": "openai", "requests_total": 1}],
        },
    )

    user = SimpleNamespace(role=UserRole.USER)
    result = await analytics_performance(
        AnalyticsBaseRequest(),
        current_user=user,
        db=SimpleNamespace(),
    )

    assert result["provider_health"] == []
