from __future__ import annotations

from typing import Any
from unittest.mock import AsyncMock, MagicMock

import pytest
from sqlalchemy.exc import IntegrityError

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


class DummyDiag:
    def __init__(self, constraint_name: str) -> None:
        self.constraint_name = constraint_name


class DummyDbOrig(Exception):
    def __init__(self, constraint_name: str) -> None:
        super().__init__(constraint_name)
        self.diag = DummyDiag(constraint_name)


def _integrity_error(constraint_name: str) -> IntegrityError:
    return IntegrityError("UPDATE usage", {}, DummyDbOrig(constraint_name))


def test_increment_provider_api_key_totals_refreshes_last_used_at_on_zero_delta() -> None:
    db = MagicMock()

    recording_module._increment_provider_api_key_totals(
        db,
        "provider-key-zero-delta",
        total_tokens=0,
        total_cost=0.0,
    )

    db.execute.assert_called_once()
    stmt = db.execute.call_args.args[0]
    sql = str(stmt)
    assert "last_used_at" in sql
    assert "updated_at" in sql


def test_increment_provider_api_key_totals_skips_when_key_id_missing() -> None:
    db = MagicMock()

    recording_module._increment_provider_api_key_totals(
        db,
        None,
        total_tokens=100,
        total_cost=1.0,
    )

    db.execute.assert_not_called()


def test_clear_stale_usage_model_group_references_nulls_missing_refs() -> None:
    db = MagicMock()
    db.query.side_effect = lambda _model: DummyQuery([])
    usage_params = {
        "request_id": "req-stale-route",
        "model_group_id": "missing-group",
        "model_group_route_id": "missing-route",
    }

    recording_module._clear_stale_usage_model_group_references(db, usage_params)

    assert usage_params["model_group_id"] is None
    assert usage_params["model_group_route_id"] is None
    assert db.query.call_count == 2


def test_clear_stale_usage_model_group_references_uses_cache() -> None:
    db = MagicMock()
    group_cache = {"existing-group": True}
    route_cache = {"missing-route": False}
    usage_params = {
        "request_id": "req-cached-route",
        "model_group_id": "existing-group",
        "model_group_route_id": "missing-route",
    }

    recording_module._clear_stale_usage_model_group_references(
        db,
        usage_params,
        existing_model_group_ids=group_cache,
        existing_model_group_route_ids=route_cache,
    )

    assert usage_params["model_group_id"] == "existing-group"
    assert usage_params["model_group_route_id"] is None
    db.query.assert_not_called()


def test_clear_usage_model_group_refs_after_route_fk_violation() -> None:
    usage_params = {
        "request_id": "req-route-fk",
        "model_group_id": "group-1",
        "model_group_route_id": "route-1",
    }

    assert recording_module._clear_usage_model_group_refs_after_fk_violation(
        usage_params,
        _integrity_error("usage_model_group_route_id_fkey"),
    )

    assert usage_params["model_group_id"] == "group-1"
    assert usage_params["model_group_route_id"] is None


def test_clear_usage_model_group_refs_after_group_fk_violation() -> None:
    usage_params = {
        "request_id": "req-group-fk",
        "model_group_id": "group-1",
        "model_group_route_id": "route-1",
    }

    assert recording_module._clear_usage_model_group_refs_after_fk_violation(
        usage_params,
        _integrity_error("usage_model_group_id_fkey"),
    )

    assert usage_params["model_group_id"] is None
    assert usage_params["model_group_route_id"] is None


def test_clear_usage_model_group_refs_ignores_unrelated_integrity_error() -> None:
    usage_params = {
        "request_id": "req-other-fk",
        "model_group_id": "group-1",
        "model_group_route_id": "route-1",
    }

    assert not recording_module._clear_usage_model_group_refs_after_fk_violation(
        usage_params,
        _integrity_error("usage_provider_id_fkey"),
    )

    assert usage_params["model_group_id"] == "group-1"
    assert usage_params["model_group_route_id"] == "route-1"


@pytest.mark.asyncio
async def test_record_usage_retries_without_stale_route_after_commit_fk(
    monkeypatch: Any,
) -> None:
    db = MagicMock()

    def query_for(entity: Any) -> DummyQuery:
        owner = getattr(entity, "class_", None)
        if owner in {recording_module.ModelGroup, recording_module.ModelGroupRoute}:
            return DummyQuery([object()])
        return DummyQuery([])

    db.query.side_effect = query_for
    db.commit.side_effect = [
        _integrity_error("usage_model_group_route_id_fkey"),
        None,
    ]

    usage_params = {
        "request_id": "req-route-race",
        "provider_name": "openai",
        "model": "gpt-4o",
        "status": "completed",
        "total_tokens": 0,
        "actual_total_cost_usd": 0.0,
        "model_group_id": "group-race",
        "model_group_route_id": "route-race",
    }

    monkeypatch.setattr(
        UsageService,
        "_prepare_usage_record",
        AsyncMock(return_value=(usage_params, 0.0)),
    )
    monkeypatch.setattr(
        UsageService,
        "_finalize_usage_billing",
        MagicMock(return_value=(False, False)),
    )
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
        input_tokens=0,
        output_tokens=0,
        request_id="req-route-race",
        model_group_id="group-race",
        model_group_route_id="route-race",
        status="completed",
    )

    assert db.commit.call_count == 2
    db.rollback.assert_called_once()
    first_usage = db.add.call_args_list[0].args[0]
    second_usage = db.add.call_args_list[1].args[0]
    assert first_usage.model_group_route_id == "route-race"
    assert second_usage.model_group_id == "group-race"
    assert second_usage.model_group_route_id is None


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
