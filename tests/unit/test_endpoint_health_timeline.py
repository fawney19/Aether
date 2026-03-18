from __future__ import annotations

from datetime import datetime, timedelta, timezone
from types import SimpleNamespace
from typing import Any, cast
from unittest.mock import MagicMock

import pytest

from src.services.health.endpoint import EndpointHealthService


class _FakeQuery:
    def __init__(self, rows: list[SimpleNamespace]) -> None:
        self._rows = rows

    def filter(self, *args: Any, **kwargs: Any) -> _FakeQuery:
        return self

    def group_by(self, *args: Any, **kwargs: Any) -> _FakeQuery:
        return self

    def all(self) -> list[SimpleNamespace]:
        return self._rows


class _FakeDb:
    def __init__(self, rows: list[SimpleNamespace]) -> None:
        self._rows = rows

    def query(self, *args: Any, **kwargs: Any) -> _FakeQuery:
        return _FakeQuery(self._rows)


def _expr_texts(query: MagicMock) -> list[str]:
    return [str(arg) for arg in query.filter.call_args.args]


def test_generate_timeline_batch_keeps_compact_and_cli_isolated() -> None:
    now = datetime(2026, 3, 18, 12, 0, tzinfo=timezone.utc)
    db = _FakeDb(
        [
            SimpleNamespace(
                endpoint_id="endpoint-compact",
                segment_idx=0,
                total_count=2,
                success_count=2,
                failed_count=0,
                min_time=now - timedelta(minutes=55),
                max_time=now - timedelta(minutes=40),
            ),
            SimpleNamespace(
                endpoint_id="endpoint-cli",
                segment_idx=0,
                total_count=3,
                success_count=0,
                failed_count=3,
                min_time=now - timedelta(minutes=54),
                max_time=now - timedelta(minutes=39),
            ),
        ]
    )

    result = EndpointHealthService._generate_timeline_batch(
        db=cast(Any, db),
        format_endpoint_mapping={
            "openai:compact": ["endpoint-compact"],
            "openai:cli": ["endpoint-cli"],
        },
        now=now,
        lookback_hours=1,
        segments=4,
    )

    assert result["openai:compact"]["timeline"][0] == "healthy"
    assert result["openai:cli"]["timeline"][0] == "unhealthy"


def test_generate_timeline_from_usage_uses_endpoint_ids_directly(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    expected = {
        "timeline": ["healthy", "warning"],
        "time_range_start": "start",
        "time_range_end": "end",
    }

    captured: dict[str, object] = {}

    def _fake_generate_timeline_batch(
        db: Any,
        format_endpoint_mapping: dict[str, list[str]],
        now: datetime,
        lookback_hours: int,
        segments: int,
    ) -> dict[str, dict[str, Any]]:
        captured["db"] = db
        captured["mapping"] = format_endpoint_mapping
        captured["lookback_hours"] = lookback_hours
        captured["segments"] = segments
        return {"_single": expected}

    monkeypatch.setattr(
        EndpointHealthService,
        "_generate_timeline_batch",
        staticmethod(_fake_generate_timeline_batch),
    )

    db = cast(Any, object())
    now = datetime(2026, 3, 18, 12, 0, tzinfo=timezone.utc)

    result = EndpointHealthService._generate_timeline_from_usage(
        db=db,
        endpoint_ids=["endpoint-compact"],
        now=now,
        lookback_hours=6,
        segments=2,
    )

    assert result == expected
    assert captured["db"] is db
    assert captured["mapping"] == {"_single": ["endpoint-compact"]}
    assert captured["lookback_hours"] == 6
    assert captured["segments"] == 2


def test_get_endpoint_health_by_format_filters_inactive_endpoints(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    endpoint_query = MagicMock()
    endpoint_query.join.return_value = endpoint_query
    endpoint_query.filter.return_value = endpoint_query
    endpoint_query.all.return_value = [
        SimpleNamespace(
            id="endpoint-compact",
            provider_id="provider-1",
            api_format="openai:compact",
            is_active=True,
        )
    ]

    key_query = MagicMock()
    key_query.filter.return_value = key_query
    key_query.options.return_value = key_query
    key_query.all.return_value = []

    db = MagicMock()
    db.query.side_effect = [endpoint_query, key_query]

    monkeypatch.setattr(
        EndpointHealthService,
        "_generate_timeline_batch",
        staticmethod(
            lambda db, format_endpoint_mapping, now, lookback_hours: {
                "openai:compact": {
                    "timeline": ["unknown"] * 100,
                    "time_range_start": None,
                    "time_range_end": None,
                }
            }
        ),
    )

    EndpointHealthService.get_endpoint_health_by_format(
        db=cast(Any, db),
        lookback_hours=6,
        include_admin_fields=False,
        use_cache=False,
    )

    filters = _expr_texts(endpoint_query)
    assert any("provider_endpoints.is_active" in expr for expr in filters)
    assert any("providers.is_active" in expr for expr in filters)
