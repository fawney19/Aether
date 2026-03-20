from __future__ import annotations

from typing import Any

from src.services.health.monitor import HealthMonitor


class _FakeQuery:
    def __init__(
        self,
        *,
        first_result: Any = None,
        all_result: list[Any] | None = None,
    ) -> None:
        self._first_result = first_result
        self._all_result = all_result or []

    def join(self, *args: Any, **kwargs: Any) -> _FakeQuery:
        return self

    def filter(self, *args: Any, **kwargs: Any) -> _FakeQuery:
        return self

    def first(self) -> Any:
        return self._first_result

    def all(self) -> list[Any]:
        return self._all_result


class _FakeHealthSummaryDB:
    def __init__(
        self,
        *,
        endpoint_stats: Any,
        active_endpoint_rows: list[tuple[str, str]],
        key_rows: list[tuple[Any, ...]],
    ) -> None:
        self._endpoint_stats = endpoint_stats
        self._active_endpoint_rows = active_endpoint_rows
        self._key_rows = key_rows
        self._query_count = 0

    def query(self, *models: Any) -> _FakeQuery:
        self._query_count += 1
        if self._query_count == 1:
            return _FakeQuery(first_result=self._endpoint_stats)
        if self._query_count == 2:
            return _FakeQuery(all_result=self._active_endpoint_rows)
        if self._query_count == 3:
            return _FakeQuery(all_result=self._key_rows)
        raise AssertionError(f"unexpected query #{self._query_count}: {models}")


def test_get_all_health_status_counts_only_current_schedulable_keys() -> None:
    endpoint_stats = type("EndpointStats", (), {"total": 3, "active": 3})()
    db = _FakeHealthSummaryDB(
        endpoint_stats=endpoint_stats,
        active_endpoint_rows=[
            ("provider-a", "openai:chat"),
            ("provider-a", "openai:cli"),
            ("provider-b", "claude:chat"),
        ],
        key_rows=[
            (
                "provider-a",
                True,
                ["openai:chat", "openai:cli"],
                {"openai:chat": {"health_score": 0.4}},
                {"openai:chat": {"open": True}, "openai:cli": {"open": False}},
                True,
            ),
            (
                "provider-a",
                True,
                ["openai:chat"],
                {"openai:chat": {"health_score": 0.8}},
                {"openai:chat": {"open": True}},
                True,
            ),
            (
                "provider-b",
                True,
                ["claude:chat"],
                {"claude:chat": {"health_score": 0.9}},
                {"claude:chat": {"open": False}},
                False,
            ),
            (
                "provider-c",
                True,
                ["gemini:chat"],
                {"gemini:chat": {"health_score": 1.0}},
                {"gemini:chat": {"open": False}},
                True,
            ),
            (
                "provider-a",
                False,
                ["openai:chat"],
                {"openai:chat": {"health_score": 1.0}},
                {"openai:chat": {"open": False}},
                True,
            ),
        ],
    )

    summary = HealthMonitor.get_all_health_status(db)

    assert summary["endpoints"] == {"total": 3, "active": 3, "unhealthy": 2}
    assert summary["keys"]["total"] == 5
    assert summary["keys"]["active"] == 1
    assert summary["keys"]["unhealthy"] == 1
    assert summary["keys"]["circuit_open"] == 2
