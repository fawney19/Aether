from types import SimpleNamespace
from typing import Any
from unittest.mock import AsyncMock, MagicMock

import pytest

from src.config.settings import config
from src.core.api_format.conversion.internal_video import VideoStatus
from src.services.billing.formula_engine import BillingIncompleteError
from src.services.task.service import TaskService


def _make_task(**overrides: Any) -> SimpleNamespace:
    task = SimpleNamespace(
        id="t1",
        request_id="req-1",
        user_id="u1",
        api_key_id="ak1",
        provider_id="p1",
        endpoint_id="e1",
        key_id="k1",
        external_task_id="ext-1",
        client_api_format="openai:video",
        provider_api_format="openai:video",
        format_converted=False,
        model="sora",
        original_request_body={"model": "sora"},
        duration_seconds=4,
        resolution="720p",
        aspect_ratio="16:9",
        size="1024x1024",
        retry_count=0,
        video_size_bytes=None,
        video_url="https://example.com/v.mp4",
        video_urls=["https://example.com/v.mp4"],
        submitted_at=None,
        completed_at=None,
        error_code=None,
        error_message=None,
        status=VideoStatus.COMPLETED.value,
        request_metadata={"poll_raw_response": {"foo": "bar"}},
    )
    for k, v in overrides.items():
        setattr(task, k, v)
    return task


def _make_db() -> MagicMock:
    db = MagicMock()

    user_obj = SimpleNamespace(id="u1")
    api_key_obj = SimpleNamespace(id="ak1")
    provider_obj = SimpleNamespace(id="p1", name="prov1")
    usage_obj = SimpleNamespace(
        id="usage-1",
        request_id="req-1",
        billing_status="pending",
        request_metadata=None,
    )

    q_usage = MagicMock()
    q_usage.filter.return_value.first.return_value = usage_obj
    q_user = MagicMock()
    q_user.filter.return_value.first.return_value = user_obj
    q_key = MagicMock()
    q_key.filter.return_value.first.return_value = api_key_obj
    q_provider = MagicMock()
    q_provider.filter.return_value.first.return_value = provider_obj

    db.query.side_effect = [q_usage, q_user, q_key, q_provider]
    return db


@pytest.mark.asyncio
async def test_video_finalize_failed_records_cost_zero(monkeypatch: pytest.MonkeyPatch) -> None:
    db = _make_db()
    task = _make_task(status=VideoStatus.FAILED.value, error_message="boom")

    monkeypatch.setattr(
        "src.services.billing.dimension_collector_service.DimensionCollectorService.collect_dimensions",
        lambda _self, **_kwargs: {"duration_seconds": 4},
    )
    monkeypatch.setattr(
        "src.services.billing.rule_service.BillingRuleService.find_rule",
        lambda *_args, **_kwargs: None,
    )
    # Mock update_settled_billing (used by finalize_video_task)
    update_settled = MagicMock(return_value=True)
    monkeypatch.setattr(
        "src.services.usage.service.UsageService.update_settled_billing",
        update_settled,
    )

    svc = TaskService(db)
    await svc.finalize_video_task(task)

    # billing_snapshot should be written back to task.request_metadata
    assert task.request_metadata["billing_snapshot"]["cost"] == 0.0

    assert update_settled.call_count == 1
    kwargs = update_settled.call_args.kwargs
    assert kwargs["total_cost_usd"] == 0.0
    assert kwargs["status"] == "failed"


@pytest.mark.asyncio
async def test_video_finalize_completed_no_rule(monkeypatch: pytest.MonkeyPatch) -> None:
    db = _make_db()
    task = _make_task(status=VideoStatus.COMPLETED.value)

    monkeypatch.setattr(
        "src.services.billing.dimension_collector_service.DimensionCollectorService.collect_dimensions",
        lambda _self, **_kwargs: {"duration_seconds": 4},
    )
    monkeypatch.setattr(
        "src.services.billing.rule_service.BillingRuleService.find_rule",
        lambda *_args, **_kwargs: None,
    )
    # Mock update_settled_billing (used by finalize_video_task)
    update_settled = MagicMock(return_value=True)
    monkeypatch.setattr(
        "src.services.usage.service.UsageService.update_settled_billing",
        update_settled,
    )

    svc = TaskService(db)
    await svc.finalize_video_task(task)

    assert task.request_metadata["billing_snapshot"]["status"] == "no_rule"

    kwargs = update_settled.call_args.kwargs
    assert kwargs["total_cost_usd"] == 0.0
    assert kwargs["status"] == "completed"


@pytest.mark.asyncio
async def test_video_finalize_strict_mode_missing_required_marks_failed(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = _make_db()
    task = _make_task(
        status=VideoStatus.COMPLETED.value,
        request_metadata={
            "poll_raw_response": {"foo": "bar"},
            "billing_rule_snapshot": {
                "status": "ok",
                "rule_id": "r1",
                "rule_name": "video",
                "scope": "model",
                "expression": "duration_seconds",
                "variables": {},
                "dimension_mappings": {},
            },
        },
    )

    monkeypatch.setattr(
        "src.services.billing.dimension_collector_service.DimensionCollectorService.collect_dimensions",
        lambda _self, **_kwargs: {"duration_seconds": None},
    )
    # Mock update_settled_billing (used by finalize_video_task)
    update_settled = MagicMock(return_value=True)
    monkeypatch.setattr(
        "src.services.usage.service.UsageService.update_settled_billing",
        update_settled,
    )

    old = config.billing_strict_mode
    try:
        config.billing_strict_mode = True
        monkeypatch.setattr(
            "src.services.billing.formula_engine.FormulaEngine.evaluate",
            MagicMock(
                side_effect=BillingIncompleteError(
                    "Missing required dimensions", missing_required=["duration_seconds"]
                )
            ),
        )
        svc = TaskService(db)
        await svc.finalize_video_task(task)
    finally:
        config.billing_strict_mode = old

    assert task.status == VideoStatus.FAILED.value
    assert task.video_url is None
    assert task.video_urls is None
    assert "billing_incomplete" in (task.error_code or "")

    kwargs = update_settled.call_args.kwargs
    assert kwargs["total_cost_usd"] == 0.0
    assert kwargs["status"] == "failed"


@pytest.mark.asyncio
async def test_video_finalize_fallback_reads_headers_alias(monkeypatch: pytest.MonkeyPatch) -> None:
    usage_query = MagicMock()
    usage_query.filter.return_value.first.return_value = None
    user_query = MagicMock()
    user_query.filter.return_value.first.return_value = SimpleNamespace(id="u1")
    api_key_query = MagicMock()
    api_key_query.filter.return_value.first.return_value = SimpleNamespace(id="ak1")
    provider_query = MagicMock()
    provider_query.filter.return_value.first.return_value = SimpleNamespace(
        id="p1",
        name="prov1",
    )

    db = MagicMock()
    db.query.side_effect = [usage_query, user_query, api_key_query, provider_query]

    task = _make_task(
        status=VideoStatus.FAILED.value,
        error_message="boom",
        request_metadata={"headers": {"x-test-header": "1"}},
    )

    record_usage = AsyncMock(return_value=SimpleNamespace(id="usage-1"))
    monkeypatch.setattr(
        "src.services.usage.service.UsageService.record_usage_with_custom_cost",
        record_usage,
    )

    svc = TaskService(db)
    ok = await svc.finalize_video_task(task)

    assert ok is True
    kwargs = record_usage.call_args.kwargs
    assert kwargs["request_headers"] == {"x-test-header": "1"}
