from __future__ import annotations

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
