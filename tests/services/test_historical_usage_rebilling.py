from __future__ import annotations

from datetime import datetime
from types import SimpleNamespace
from unittest.mock import MagicMock

import pytest

import src.services.usage.historical_rebilling as historical_rebilling_module
from src.services.usage.historical_rebilling import HistoricalUsageRebillingService


def test_run_single_step_initializes_state_when_missing(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    saved: dict[str, object] = {}

    monkeypatch.setattr(historical_rebilling_module, "create_session", lambda: db)

    def fake_get_config(_db, key: str, default=None):  # type: ignore[no-untyped-def]
        if key == HistoricalUsageRebillingService.ENABLED_KEY:
            return True
        if key == HistoricalUsageRebillingService.STATE_KEY:
            return None
        return default

    def fake_set_config(_db, key: str, value, description=None):  # type: ignore[no-untyped-def]
        saved["key"] = key
        saved["value"] = value
        saved["description"] = description
        return MagicMock()

    monkeypatch.setattr(
        historical_rebilling_module.SystemConfigService,
        "get_config",
        fake_get_config,
    )
    monkeypatch.setattr(
        historical_rebilling_module.SystemConfigService,
        "set_config",
        fake_set_config,
    )

    finished = HistoricalUsageRebillingService._run_single_step()

    assert finished is False
    assert saved["key"] == HistoricalUsageRebillingService.STATE_KEY
    assert isinstance(saved["value"], dict)
    assert saved["value"]["version"] == HistoricalUsageRebillingService.TARGET_VERSION
    assert saved["value"]["phase"] == "usage_reprice"
    assert saved["value"]["history_days"] == 40
    window_start = datetime.fromisoformat(saved["value"]["window_start_created_at"])
    cutoff = datetime.fromisoformat(saved["value"]["cutoff_created_at"])
    assert (cutoff - window_start).days == 40


def test_run_single_step_returns_true_when_disabled(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    monkeypatch.setattr(historical_rebilling_module, "create_session", lambda: db)
    monkeypatch.setattr(
        historical_rebilling_module.SystemConfigService,
        "get_config",
        lambda _db, key, default=None: False
        if key == HistoricalUsageRebillingService.ENABLED_KEY
        else default,
    )

    finished = HistoricalUsageRebillingService._run_single_step()

    assert finished is True


def test_build_token_payload_forces_claude_endpoint_to_1h_pricing(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    captured: dict[str, object] = {}

    class FakeSnapshot:
        status = "complete"
        cost_breakdown = {
            "input_cost": 0.001,
            "output_cost": 0.002,
            "cache_creation_cost": 0.006,
            "cache_read_cost": 0.0001,
            "request_cost": 0.0,
        }
        total_cost = 0.0091
        resolved_variables = {
            "input_price_per_1m": 1.0,
            "output_price_per_1m": 2.0,
            "cache_creation_price_per_1m": 6.0,
            "cache_read_price_per_1m": 0.5,
            "price_per_request": 0.0,
        }

        def to_dict(self) -> dict[str, object]:
            return {
                "status": self.status,
                "cost_breakdown": self.cost_breakdown,
                "resolved_variables": self.resolved_variables,
                "total_cost": self.total_cost,
            }

    class FakeBillingService:
        def __init__(self, _db: object) -> None:
            pass

        def calculate(self, **kwargs: object) -> SimpleNamespace:
            captured.update(kwargs)
            return SimpleNamespace(snapshot=FakeSnapshot())

    monkeypatch.setattr(historical_rebilling_module, "BillingService", FakeBillingService)

    usage = SimpleNamespace(
        provider_id="provider-1",
        model="claude-sonnet",
        endpoint_api_format="claude:chat",
        api_format="openai:chat",
        provider_api_family=None,
        provider_endpoint_kind=None,
        has_format_conversion=True,
        input_context_tokens=None,
        input_tokens=1000,
        output_tokens=500,
        cache_creation_input_tokens=300,
        cache_read_input_tokens=50,
        cache_ttl_minutes=5,
        status_code=200,
        error_message=None,
        request_metadata={},
        rate_multiplier=1,
        user_billing_multiplier=1,
        total_cost_usd=0,
        actual_total_cost_usd=0,
        is_stream=False,
        provider_api_key_id=None,
        get_response_body=lambda: None,
        get_client_response_body=lambda: None,
    )

    payload = HistoricalUsageRebillingService._build_token_payload(
        MagicMock(),
        usage,  # type: ignore[arg-type]
        "chat",
    )

    assert payload is not None
    assert captured["dimensions"]["cache_ttl_minutes"] == 60  # type: ignore[index]
    assert payload["cache_ttl_minutes"] == 60
    assert str(payload["cache_creation_price_per_1m"]) == "6.00000000"
    assert (
        payload["request_metadata"]["billing_recalc_cache_ttl_policy"]
        == "force_1h_for_claude_endpoint"
    )
