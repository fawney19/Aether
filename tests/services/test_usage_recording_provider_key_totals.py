from __future__ import annotations

from typing import Any
from unittest.mock import AsyncMock, MagicMock

import pytest

import src.services.usage.recording as recording_module
from src.services.usage.service import UsageService


class DummyQuery:
    def __init__(self, result: list[Any]) -> None:
        self._result = result

    def options(self, *args: Any, **kwargs: Any) -> "DummyQuery":
        return self

    def filter(self, *args: Any, **kwargs: Any) -> "DummyQuery":
        return self

    def with_for_update(self) -> "DummyQuery":
        return self

    def all(self) -> list[Any]:
        return self._result

    def first(self) -> Any | None:
        return self._result[0] if self._result else None


@pytest.mark.asyncio
async def test_record_usage_updates_provider_key_totals(monkeypatch: Any) -> None:
    db = MagicMock()
    db.query.side_effect = lambda _model: DummyQuery([])

    usage_params = {
        "request_id": "req-provider-key-1",
        "provider_name": "openai",
        "model": "gpt-4o",
        "status": "completed",
        "total_tokens": 123,
        "actual_total_cost_usd": 1.75,
    }

    monkeypatch.setattr(
        UsageService,
        "_prepare_usage_record",
        AsyncMock(return_value=(usage_params, 1.25)),
    )
    monkeypatch.setattr(
        UsageService,
        "_finalize_usage_billing",
        MagicMock(return_value=(True, True)),
    )
    helper = MagicMock()
    monkeypatch.setattr(recording_module, "_increment_provider_api_key_totals", helper)
    monkeypatch.setattr(
        recording_module,
        "dispatch_codex_quota_sync_from_response_headers",
        MagicMock(),
    )

    await UsageService.record_usage(
        db=db,
        user=None,
        api_key=None,
        provider="openai",
        model="gpt-4o",
        input_tokens=100,
        output_tokens=23,
        provider_api_key_id="provider-key-1",
        request_id="req-provider-key-1",
        status="completed",
    )

    helper.assert_called_once()
    _, provider_key_id = helper.call_args.args
    assert provider_key_id == "provider-key-1"
    assert helper.call_args.kwargs["total_tokens"] == 123
    assert float(helper.call_args.kwargs["total_cost"]) == 1.75


@pytest.mark.asyncio
async def test_record_usage_async_updates_provider_key_totals(monkeypatch: Any) -> None:
    db = MagicMock()

    usage_params = {
        "request_id": "req-provider-key-async",
        "provider_name": "openai",
        "model": "gpt-4o-mini",
        "status": "completed",
        "total_tokens": 77,
        "actual_total_cost_usd": 0.75,
    }

    monkeypatch.setattr(
        UsageService,
        "_prepare_usage_record",
        AsyncMock(return_value=(usage_params, 0.5)),
    )
    monkeypatch.setattr(
        UsageService,
        "_finalize_usage_billing",
        MagicMock(return_value=(True, False)),
    )
    helper = MagicMock()
    monkeypatch.setattr(recording_module, "_increment_provider_api_key_totals", helper)
    monkeypatch.setattr(
        recording_module,
        "dispatch_codex_quota_sync_from_response_headers",
        MagicMock(),
    )

    await UsageService.record_usage_async(
        db=db,
        user=None,
        api_key=None,
        provider="openai",
        model="gpt-4o-mini",
        input_tokens=50,
        output_tokens=27,
        provider_api_key_id="provider-key-async",
        request_id="req-provider-key-async",
        status="completed",
    )

    helper.assert_called_once()
    _, provider_key_id = helper.call_args.args
    assert provider_key_id == "provider-key-async"
    assert helper.call_args.kwargs["total_tokens"] == 77
    assert helper.call_args.kwargs["total_cost"] == 0.75


@pytest.mark.asyncio
async def test_record_usage_batch_aggregates_provider_key_totals(monkeypatch: Any) -> None:
    db = MagicMock()
    db.query.side_effect = lambda _model: DummyQuery([])

    usage_params_1 = {
        "request_id": "req-provider-key-batch-1",
        "provider_name": "anthropic",
        "model": "claude-sonnet",
        "status": "completed",
        "total_tokens": 321,
        "actual_total_cost_usd": 2.5,
    }
    usage_params_2 = {
        "request_id": "req-provider-key-batch-2",
        "provider_name": "anthropic",
        "model": "claude-sonnet",
        "status": "completed",
        "total_tokens": 79,
        "actual_total_cost_usd": 0.75,
    }

    monkeypatch.setattr(
        UsageService,
        "_prepare_usage_records_batch",
        AsyncMock(
            return_value=[
                (usage_params_1, 2.0, None),
                (usage_params_2, 0.5, None),
            ]
        ),
    )
    monkeypatch.setattr(
        UsageService,
        "_finalize_usage_billing",
        MagicMock(return_value=(True, True)),
    )
    helper = MagicMock()
    monkeypatch.setattr(recording_module, "_increment_provider_api_key_totals", helper)
    monkeypatch.setattr(
        recording_module,
        "dispatch_codex_quota_sync_from_response_headers",
        MagicMock(),
    )

    await UsageService.record_usage_batch(
        db,
        [
            {
                "request_id": "req-provider-key-batch-1",
                "provider": "anthropic",
                "model": "claude-sonnet",
                "status": "completed",
                "provider_api_key_id": "provider-key-batch",
            },
            {
                "request_id": "req-provider-key-batch-2",
                "provider": "anthropic",
                "model": "claude-sonnet",
                "status": "completed",
                "provider_api_key_id": "provider-key-batch",
            },
        ],
    )

    helper.assert_called_once()
    _, provider_key_id = helper.call_args.args
    assert provider_key_id == "provider-key-batch"
    assert helper.call_args.kwargs["total_tokens"] == 400
    assert helper.call_args.kwargs["total_cost"] == 3.25
