from __future__ import annotations

from types import SimpleNamespace
from typing import Any

import pytest

from src.services.health.monitor import HealthMonitor


class _SequentialQuery:
    def __init__(self, result: Any) -> None:
        self._result = result

    def filter(self, *_args: object, **_kwargs: object) -> "_SequentialQuery":
        return self

    def all(self) -> Any:
        return self._result

    def first(self) -> Any:
        return self._result


class _SequentialSession:
    def __init__(self, results: list[Any]) -> None:
        self._results = results
        self.calls = 0

    def query(self, *_entities: object) -> _SequentialQuery:
        result = self._results[self.calls]
        self.calls += 1
        return _SequentialQuery(result)


def test_get_all_health_status_aggregates_endpoint_health_from_key_formats() -> None:
    db = _SequentialSession(
        results=[
            [
                SimpleNamespace(provider_id="provider-1", api_format="openai:chat", is_active=True),
                SimpleNamespace(provider_id="provider-1", api_format="claude:chat", is_active=False),
                SimpleNamespace(provider_id="provider-2", api_format="openai:chat", is_active=True),
            ],
            [
                (
                    "provider-1",
                    ["openai:chat", "claude:chat"],
                    {
                        "openai:chat": {"health_score": 0.4},
                        "claude:chat": {"health_score": 0.7},
                    },
                ),
                (
                    "provider-2",
                    ["openai:chat"],
                    {"openai:chat": {"health_score": 0.9}},
                ),
            ],
            [
                (True, {"openai:chat": {"health_score": 0.4}}, {"openai:chat": {"open": True}}),
                (False, {"claude:chat": {"health_score": 0.7}}, {}),
                (True, {"openai:chat": {"health_score": 0.9}}, {}),
            ],
        ]
    )

    result = HealthMonitor.get_all_health_status(db)  # type: ignore[arg-type]

    assert result == {
        "endpoints": {"total": 3, "active": 2, "unhealthy": 1},
        "keys": {"total": 3, "active": 2, "unhealthy": 1, "circuit_open": 1},
    }


def test_get_endpoint_health_aggregates_matching_key_format_data() -> None:
    db = _SequentialSession(
        results=[
            SimpleNamespace(
                id="endpoint-1",
                provider_id="provider-1",
                api_format="openai:chat",
                is_active=True,
            ),
            [
                (
                    ["openai:chat"],
                    {
                        "openai:chat": {
                            "health_score": 0.4,
                            "consecutive_failures": 2,
                            "last_failure_at": "2026-03-23T01:00:00+00:00",
                        }
                    },
                ),
                (
                    ["openai:chat"],
                    {
                        "openai:chat": {
                            "health_score": 0.8,
                            "consecutive_failures": 1,
                            "last_failure_at": "2026-03-23T02:00:00+00:00",
                        }
                    },
                ),
                (
                    ["claude:chat"],
                    {
                        "claude:chat": {
                            "health_score": 0.1,
                            "consecutive_failures": 9,
                            "last_failure_at": "2026-03-23T03:00:00+00:00",
                        }
                    },
                ),
            ],
        ]
    )

    result = HealthMonitor.get_endpoint_health(db, "endpoint-1")  # type: ignore[arg-type]

    assert result is not None
    assert result["endpoint_id"] == "endpoint-1"
    assert result["is_active"] is True
    assert result["health_score"] == pytest.approx(0.6)
    assert result["consecutive_failures"] == 2
    assert result["last_failure_at"] == "2026-03-23T02:00:00+00:00"
