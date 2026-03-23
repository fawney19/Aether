from __future__ import annotations

from datetime import date, datetime
from types import SimpleNamespace
from typing import Any, cast

from sqlalchemy.dialects import sqlite

from src.core.enums import ErrorCategory, UserRole
from src.services.analytics.query_service import (
    DELETED_API_KEY_FILTER,
    DELETED_USER_FILTER,
    AnalyticsFilters,
    AnalyticsQueryService,
)
from src.services.system.time_range import TimeRangeParams


class _SequentialQuery:
    def __init__(self, result: list[Any]) -> None:
        self._result = result

    def join(self, *_args: object, **_kwargs: object) -> "_SequentialQuery":
        return self

    def filter(self, *_args: object, **_kwargs: object) -> "_SequentialQuery":
        return self

    def order_by(self, *_args: object, **_kwargs: object) -> "_SequentialQuery":
        return self

    def all(self) -> list[Any]:
        return self._result


class _SequentialSession:
    def __init__(self, results: list[list[Any]]) -> None:
        self._results = results
        self.calls = 0

    def query(self, *_entities: object) -> _SequentialQuery:
        result = self._results[self.calls]
        self.calls += 1
        return _SequentialQuery(result)


class _FakeBreakdownQuery:
    def __init__(self, rows: list[Any]) -> None:
        self._rows = rows

    def with_entities(self, *_args: object, **_kwargs: object) -> "_FakeBreakdownQuery":
        return self

    def group_by(self, *_args: object, **_kwargs: object) -> "_FakeBreakdownQuery":
        return self

    def order_by(self, *_args: object, **_kwargs: object) -> "_FakeBreakdownQuery":
        return self

    def limit(self, *_args: object, **_kwargs: object) -> "_FakeBreakdownQuery":
        return self

    def all(self) -> list[Any]:
        return self._rows


class _CapturedFilterQuery:
    def __init__(self) -> None:
        self.filters: list[Any] = []

    def filter(self, *conditions: Any) -> "_CapturedFilterQuery":
        self.filters.extend(conditions)
        return self


class _CapturedFilterSession:
    def __init__(self) -> None:
        self.query_obj = _CapturedFilterQuery()

    def query(self, *_entities: object) -> _CapturedFilterQuery:
        return self.query_obj


class _FakeRecordsQuery:
    def __init__(self, rows: list[Any]) -> None:
        self._rows = rows
        self._offset = 0
        self._limit = len(rows)

    def filter(self, *_args: object, **_kwargs: object) -> "_FakeRecordsQuery":
        return self

    def with_entities(self, *_args: object, **_kwargs: object) -> "_FakeRecordsQuery":
        return self

    def scalar(self) -> int:
        return len(self._rows)

    def order_by(self, *_args: object, **_kwargs: object) -> "_FakeRecordsQuery":
        return self

    def offset(self, value: int) -> "_FakeRecordsQuery":
        self._offset = value
        return self

    def limit(self, value: int) -> "_FakeRecordsQuery":
        self._limit = value
        return self

    def all(self) -> list[Any]:
        return self._rows[self._offset:self._offset + self._limit]


class _FakePerformanceQuery:
    def __init__(self, rows: list[Any]) -> None:
        self._rows = rows

    def with_entities(self, *_args: object, **_kwargs: object) -> "_FakePerformanceQuery":
        return self

    def all(self) -> list[Any]:
        return self._rows


def _empty_filters() -> AnalyticsFilters:
    return AnalyticsFilters(
        user_ids=[],
        provider_names=[],
        models=[],
        target_models=[],
        api_key_ids=[],
        api_formats=[],
        request_types=[],
        statuses=[],
        error_categories=[],
        is_stream=None,
        has_format_conversion=None,
    )


def test_resolve_model_display_names_uses_catalog_mapping() -> None:
    db = _SequentialSession(
        results=[
            [("claude-sonnet-4-5-20250929", "Claude Sonnet 4.5")],
            [],
            [([{"name": "gpt5"}], "GPT-5.1", "gpt-5.1")],
        ]
    )

    resolved = AnalyticsQueryService._resolve_model_display_names(
        cast(Any, db),
        ["claude-sonnet-4-5-20250929", "gpt5"],
    )

    assert resolved == {
        "claude-sonnet-4-5-20250929": "Claude Sonnet 4.5",
        "gpt5": "GPT-5.1",
    }
    assert db.calls == 3


def test_resolve_current_user_options_prefers_current_usernames() -> None:
    db = _SequentialSession(
        results=[
            [
                ("69e5702f-5b12-4f3c-83f4-4f3e97deec76", "admin"),
                ("6a63312d-1e9b-4106-a3af-00ab6f24b089", "NyaDoo"),
            ],
        ]
    )

    resolved = AnalyticsQueryService._resolve_current_user_options(
        cast(Any, db),
        [
            "69e5702f-5b12-4f3c-83f4-4f3e97deec76",
            "6a63312d-1e9b-4106-a3af-00ab6f24b089",
        ],
    )

    assert resolved == [
        {"value": "69e5702f-5b12-4f3c-83f4-4f3e97deec76", "label": "admin"},
        {"value": "6a63312d-1e9b-4106-a3af-00ab6f24b089", "label": "NyaDoo"},
    ]
    assert db.calls == 1


def test_resolve_current_api_key_options_prefers_current_key_names() -> None:
    db = _SequentialSession(
        results=[
            [
                ("d49f7f3b-d379-44f8-9a5d-6425c251960e", "DEBUG-KEY"),
                ("20b864a1-476c-4dab-aabf-0e25f3919786", "Key-2026-03-16"),
            ],
        ]
    )

    resolved = AnalyticsQueryService._resolve_current_api_key_options(
        cast(Any, db),
        [
            "d49f7f3b-d379-44f8-9a5d-6425c251960e",
            "20b864a1-476c-4dab-aabf-0e25f3919786",
        ],
    )

    assert resolved == [
        {"value": "d49f7f3b-d379-44f8-9a5d-6425c251960e", "label": "DEBUG-KEY"},
        {"value": "20b864a1-476c-4dab-aabf-0e25f3919786", "label": "Key-2026-03-16"},
    ]
    assert db.calls == 1


def test_breakdown_formats_model_dimension_labels(monkeypatch) -> None:
    rows = [
        SimpleNamespace(
            dimension_key="claude-sonnet-4-5-20250929",
            requests_total=2,
            total_tokens=1200,
            total_cost_usd=12.5,
        )
    ]

    monkeypatch.setattr(
        AnalyticsQueryService,
        "build_usage_query",
        lambda *_args, **_kwargs: _FakeBreakdownQuery(rows),
    )
    monkeypatch.setattr(
        AnalyticsQueryService,
        "overview",
        lambda *_args, **_kwargs: {"summary": {"total_cost_usd": 25.0, "total_tokens": 2400}},
    )
    monkeypatch.setattr(
        AnalyticsQueryService,
        "_resolve_model_display_names",
        staticmethod(lambda _db, _names: {"claude-sonnet-4-5-20250929": "Claude Sonnet 4.5"}),
    )

    result = AnalyticsQueryService.breakdown(
        cast(Any, SimpleNamespace()),
        SimpleNamespace(id="user-1", role=UserRole.USER),
        time_range=TimeRangeParams(start_date=date(2026, 3, 18), end_date=date(2026, 3, 18)),
        scope_kind="me",
        scope_user_id=None,
        scope_api_key_id=None,
        filters=_empty_filters(),
        dimension="model",
        limit=10,
    )

    assert result["rows"][0]["key"] == "claude-sonnet-4-5-20250929"
    assert result["rows"][0]["label"] == "Claude Sonnet 4.5"


def test_breakdown_user_dimension_prefers_current_usernames(monkeypatch) -> None:
    rows = [
        SimpleNamespace(
            dimension_key="69e5702f-5b12-4f3c-83f4-4f3e97deec76",
            requests_total=3,
            total_tokens=1800,
            total_cost_usd=18.0,
            actual_total_cost_usd=9.0,
        )
    ]

    monkeypatch.setattr(
        AnalyticsQueryService,
        "build_usage_query",
        lambda *_args, **_kwargs: _FakeBreakdownQuery(rows),
    )
    monkeypatch.setattr(
        AnalyticsQueryService,
        "overview",
        lambda *_args, **_kwargs: {
            "summary": {
                "requests_total": 6,
                "total_tokens": 3600,
                "total_cost_usd": 36.0,
                "actual_total_cost_usd": 18.0,
            }
        },
    )

    db = _SequentialSession(
        results=[
            [("69e5702f-5b12-4f3c-83f4-4f3e97deec76", "admin")],
        ]
    )

    result = AnalyticsQueryService.breakdown(
        cast(Any, db),
        SimpleNamespace(id="admin-1", role=UserRole.ADMIN),
        time_range=TimeRangeParams(start_date=date(2026, 3, 18), end_date=date(2026, 3, 18)),
        scope_kind="global",
        scope_user_id=None,
        scope_api_key_id=None,
        filters=_empty_filters(),
        dimension="user",
        metric="actual_total_cost_usd",
        limit=10,
    )

    assert result["metric"] == "actual_total_cost_usd"
    assert result["rows"][0]["key"] == "69e5702f-5b12-4f3c-83f4-4f3e97deec76"
    assert result["rows"][0]["label"] == "admin"
    assert result["rows"][0]["share_of_selected_metric"] == 50.0


def test_build_usage_query_excludes_endpoint_test_requests() -> None:
    db = _CapturedFilterSession()

    AnalyticsQueryService.build_usage_query(
        cast(Any, db),
        SimpleNamespace(id="user-1", role=UserRole.USER),
        time_range=TimeRangeParams(start_date=date(2026, 3, 18), end_date=date(2026, 3, 18)),
        scope_kind="me",
        scope_user_id=None,
        scope_api_key_id=None,
        filters=_empty_filters(),
    )

    compiled_filters = [
        str(condition.compile(dialect=sqlite.dialect(), compile_kwargs={"literal_binds": True}))
        for condition in db.query_obj.filters
    ]

    assert any(
        "usage.request_type IS NULL OR (usage.request_type NOT IN ('endpoint_test'))" in condition
        for condition in compiled_filters
    )


def test_build_usage_query_applies_api_key_filters() -> None:
    db = _CapturedFilterSession()
    filters = _empty_filters()
    filters.api_key_ids = ["key-1", "key-2"]

    AnalyticsQueryService.build_usage_query(
        cast(Any, db),
        SimpleNamespace(id="user-1", role=UserRole.USER),
        time_range=TimeRangeParams(start_date=date(2026, 3, 18), end_date=date(2026, 3, 18)),
        scope_kind="me",
        scope_user_id=None,
        scope_api_key_id=None,
        filters=filters,
    )

    compiled_filters = [
        str(condition.compile(dialect=sqlite.dialect(), compile_kwargs={"literal_binds": True}))
        for condition in db.query_obj.filters
    ]

    assert any("usage.api_key_id IN ('key-1', 'key-2')" in condition for condition in compiled_filters)


def test_build_usage_query_applies_user_filters() -> None:
    db = _CapturedFilterSession()
    filters = _empty_filters()
    filters.user_ids = ["user-1", "user-2"]

    AnalyticsQueryService.build_usage_query(
        cast(Any, db),
        SimpleNamespace(id="admin-1", role=UserRole.ADMIN),
        time_range=TimeRangeParams(start_date=date(2026, 3, 18), end_date=date(2026, 3, 18)),
        scope_kind="global",
        scope_user_id=None,
        scope_api_key_id=None,
        filters=filters,
    )

    compiled_filters = [
        str(condition.compile(dialect=sqlite.dialect(), compile_kwargs={"literal_binds": True}))
        for condition in db.query_obj.filters
    ]

    assert any("usage.user_id IN ('user-1', 'user-2')" in condition for condition in compiled_filters)


def test_build_usage_query_ignores_provider_filters_for_non_admin() -> None:
    db = _CapturedFilterSession()
    filters = _empty_filters()
    filters.provider_names = ["openai"]

    AnalyticsQueryService.build_usage_query(
        cast(Any, db),
        SimpleNamespace(id="user-1", role=UserRole.USER),
        time_range=TimeRangeParams(start_date=date(2026, 3, 18), end_date=date(2026, 3, 18)),
        scope_kind="me",
        scope_user_id=None,
        scope_api_key_id=None,
        filters=filters,
    )

    compiled_filters = [
        str(condition.compile(dialect=sqlite.dialect(), compile_kwargs={"literal_binds": True}))
        for condition in db.query_obj.filters
    ]

    assert not any("usage.provider_name IN ('openai')" in condition for condition in compiled_filters)


def test_build_usage_query_applies_provider_filters_for_admin() -> None:
    db = _CapturedFilterSession()
    filters = _empty_filters()
    filters.provider_names = ["openai"]

    AnalyticsQueryService.build_usage_query(
        cast(Any, db),
        SimpleNamespace(id="admin-1", role=UserRole.ADMIN),
        time_range=TimeRangeParams(start_date=date(2026, 3, 18), end_date=date(2026, 3, 18)),
        scope_kind="global",
        scope_user_id=None,
        scope_api_key_id=None,
        filters=filters,
    )

    compiled_filters = [
        str(condition.compile(dialect=sqlite.dialect(), compile_kwargs={"literal_binds": True}))
        for condition in db.query_obj.filters
    ]

    assert any("usage.provider_name IN ('openai')" in condition for condition in compiled_filters)


def test_build_usage_query_applies_deleted_api_key_filter() -> None:
    db = _CapturedFilterSession()
    filters = _empty_filters()
    filters.api_key_ids = [DELETED_API_KEY_FILTER]

    AnalyticsQueryService.build_usage_query(
        cast(Any, db),
        SimpleNamespace(id="user-1", role=UserRole.USER),
        time_range=TimeRangeParams(start_date=date(2026, 3, 18), end_date=date(2026, 3, 18)),
        scope_kind="me",
        scope_user_id=None,
        scope_api_key_id=None,
        filters=filters,
    )

    compiled_filters = [
        str(condition.compile(dialect=sqlite.dialect(), compile_kwargs={"literal_binds": True}))
        for condition in db.query_obj.filters
    ]

    assert any("usage.api_key_id IS NULL" in condition for condition in compiled_filters)


def test_build_usage_query_applies_deleted_user_scope() -> None:
    db = _CapturedFilterSession()

    AnalyticsQueryService.build_usage_query(
        cast(Any, db),
        SimpleNamespace(id="admin-1", role=UserRole.ADMIN),
        time_range=TimeRangeParams(start_date=date(2026, 3, 18), end_date=date(2026, 3, 18)),
        scope_kind="user",
        scope_user_id=DELETED_USER_FILTER,
        scope_api_key_id=None,
        filters=_empty_filters(),
    )

    compiled_filters = [
        str(condition.compile(dialect=sqlite.dialect(), compile_kwargs={"literal_binds": True}))
        for condition in db.query_obj.filters
    ]

    assert any("usage.user_id IS NULL" in condition for condition in compiled_filters)


def test_build_usage_query_applies_deleted_user_filter() -> None:
    db = _CapturedFilterSession()
    filters = _empty_filters()
    filters.user_ids = [DELETED_USER_FILTER]

    AnalyticsQueryService.build_usage_query(
        cast(Any, db),
        SimpleNamespace(id="admin-1", role=UserRole.ADMIN),
        time_range=TimeRangeParams(start_date=date(2026, 3, 18), end_date=date(2026, 3, 18)),
        scope_kind="global",
        scope_user_id=None,
        scope_api_key_id=None,
        filters=filters,
    )

    compiled_filters = [
        str(condition.compile(dialect=sqlite.dialect(), compile_kwargs={"literal_binds": True}))
        for condition in db.query_obj.filters
    ]

    assert any("usage.user_id IS NULL" in condition for condition in compiled_filters)


def test_build_usage_query_applies_stream_status_filter_without_failed_requests() -> None:
    db = _CapturedFilterSession()
    filters = _empty_filters()
    filters.statuses = ["stream"]

    AnalyticsQueryService.build_usage_query(
        cast(Any, db),
        SimpleNamespace(id="user-1", role=UserRole.USER),
        time_range=TimeRangeParams(start_date=date(2026, 3, 18), end_date=date(2026, 3, 18)),
        scope_kind="me",
        scope_user_id=None,
        scope_api_key_id=None,
        filters=filters,
        include_non_terminal=True,
    )

    compiled_filters = [
        str(condition.compile(dialect=sqlite.dialect(), compile_kwargs={"literal_binds": True}))
        for condition in db.query_obj.filters
    ]

    assert any("usage.is_stream IS 1" in condition for condition in compiled_filters)
    assert any("usage.status != 'failed'" in condition for condition in compiled_filters)
    assert any("usage.error_message IS NULL" in condition for condition in compiled_filters)


def test_build_usage_query_applies_failed_status_filter_with_legacy_error_fields() -> None:
    db = _CapturedFilterSession()
    filters = _empty_filters()
    filters.statuses = ["failed"]

    AnalyticsQueryService.build_usage_query(
        cast(Any, db),
        SimpleNamespace(id="user-1", role=UserRole.USER),
        time_range=TimeRangeParams(start_date=date(2026, 3, 18), end_date=date(2026, 3, 18)),
        scope_kind="me",
        scope_user_id=None,
        scope_api_key_id=None,
        filters=filters,
        include_non_terminal=True,
    )

    compiled_filters = [
        str(condition.compile(dialect=sqlite.dialect(), compile_kwargs={"literal_binds": True}))
        for condition in db.query_obj.filters
    ]

    assert any("usage.status = 'failed'" in condition for condition in compiled_filters)
    assert any("usage.status_code >= 400" in condition for condition in compiled_filters)
    assert any("usage.error_message IS NOT NULL" in condition for condition in compiled_filters)


def test_compose_status_options_only_returns_present_statuses() -> None:
    options = AnalyticsQueryService._compose_status_options(
        raw_statuses={"completed", "failed"},
        has_active=False,
        has_stream=False,
        has_standard=True,
        has_retry=False,
        has_fallback=True,
    )

    assert options == [
        {"value": "completed", "label": "completed"},
        {"value": "failed", "label": "failed"},
        {"value": "standard", "label": "standard"},
        {"value": "has_fallback", "label": "has_fallback"},
    ]


def test_records_prefers_current_user_and_key_names_over_usage_snapshots(monkeypatch) -> None:
    usage_row = SimpleNamespace(
        id="usage-1",
        request_id="req-1",
        created_at=datetime(2026, 3, 19, 12, 0, 0),
        user_id="69e5702f-5b12-4f3c-83f4-4f3e97deec76",
        username="69e5702f-5b12-4f3c-83f4-4f3e97deec76",
        api_key_id="d49f7f3b-d379-44f8-9a5d-6425c251960e",
        api_key_name="d49f7f3b-d379-44f8-9a5d-6425c251960e",
        provider_api_key_id="provider-key-1",
        provider_name="CRS",
        model="gpt-5.2",
        target_model=None,
        api_format="openai:chat",
        request_type="chat",
        status="completed",
        billing_status="completed",
        is_stream=False,
        has_format_conversion=False,
        status_code=200,
        error_message=None,
        error_category=None,
        response_time_ms=1234,
        first_byte_time_ms=321,
        input_tokens=100,
        output_tokens=50,
        input_output_total_tokens=150,
        cache_creation_input_tokens=0,
        cache_creation_input_tokens_5m=0,
        cache_creation_input_tokens_1h=0,
        cache_read_input_tokens=0,
        input_context_tokens=100,
        total_tokens=150,
        input_cost_usd=0.1,
        output_cost_usd=0.2,
        cache_creation_cost_usd=0.0,
        cache_creation_cost_usd_5m=0.0,
        cache_creation_cost_usd_1h=0.0,
        cache_read_cost_usd=0.0,
        cache_cost_usd=0.0,
        request_cost_usd=0.0,
        total_cost_usd=0.3,
        actual_total_cost_usd=0.3,
        actual_cache_cost_usd=0.0,
        rate_multiplier=1.0,
    )

    monkeypatch.setattr(
        AnalyticsQueryService,
        "build_usage_query",
        lambda *_args, **_kwargs: _FakeRecordsQuery([usage_row]),
    )
    monkeypatch.setattr(
        AnalyticsQueryService,
        "_load_request_execution_flags",
        lambda *_args, **_kwargs: ({}, {}),
    )

    db = _SequentialSession(
        results=[
            [("69e5702f-5b12-4f3c-83f4-4f3e97deec76", "admin")],
            [("d49f7f3b-d379-44f8-9a5d-6425c251960e", "DEBUG-KEY")],
            [("provider-key-1", "Pool-Key-A")],
        ]
    )

    result = AnalyticsQueryService.records(
        cast(Any, db),
        SimpleNamespace(id="admin-1", role=UserRole.ADMIN),
        time_range=TimeRangeParams(start_date=date(2026, 3, 19), end_date=date(2026, 3, 19)),
        scope_kind="global",
        scope_user_id=None,
        scope_api_key_id=None,
        filters=_empty_filters(),
        search=SimpleNamespace(text=None, request_id=None),
        limit=20,
        offset=0,
    )

    assert result["records"][0]["username"] == "admin"
    assert result["records"][0]["api_key_name"] == "DEBUG-KEY"
    assert result["records"][0]["provider_api_key_name"] == "Pool-Key-A"


def test_records_fall_back_to_deleted_labels_when_user_or_key_missing(monkeypatch) -> None:
    usage_row = SimpleNamespace(
        id="usage-1",
        request_id="req-1",
        created_at=datetime(2026, 3, 19, 12, 0, 0),
        user_id=None,
        username=None,
        api_key_id=None,
        api_key_name=None,
        provider_api_key_id=None,
        provider_name="unknown",
        model="gpt-5.2",
        target_model=None,
        api_format="openai:chat",
        request_type="chat",
        status="failed",
        billing_status="completed",
        is_stream=False,
        has_format_conversion=False,
        status_code=500,
        error_message="boom",
        error_category="server_error",
        response_time_ms=1234,
        first_byte_time_ms=321,
        input_tokens=100,
        output_tokens=50,
        input_output_total_tokens=150,
        cache_creation_input_tokens=0,
        cache_creation_input_tokens_5m=0,
        cache_creation_input_tokens_1h=0,
        cache_read_input_tokens=0,
        input_context_tokens=100,
        total_tokens=150,
        input_cost_usd=0.1,
        output_cost_usd=0.2,
        cache_creation_cost_usd=0.0,
        cache_creation_cost_usd_5m=0.0,
        cache_creation_cost_usd_1h=0.0,
        cache_read_cost_usd=0.0,
        cache_cost_usd=0.0,
        request_cost_usd=0.0,
        total_cost_usd=0.3,
        actual_total_cost_usd=0.3,
        actual_cache_cost_usd=0.0,
        rate_multiplier=1.0,
    )

    monkeypatch.setattr(
        AnalyticsQueryService,
        "build_usage_query",
        lambda *_args, **_kwargs: _FakeRecordsQuery([usage_row]),
    )
    monkeypatch.setattr(
        AnalyticsQueryService,
        "_load_request_execution_flags",
        lambda *_args, **_kwargs: ({}, {}),
    )

    result = AnalyticsQueryService.records(
        cast(Any, SimpleNamespace()),
        SimpleNamespace(id="admin-1", role=UserRole.ADMIN),
        time_range=TimeRangeParams(start_date=date(2026, 3, 19), end_date=date(2026, 3, 19)),
        scope_kind="global",
        scope_user_id=None,
        scope_api_key_id=None,
        filters=_empty_filters(),
        search=SimpleNamespace(text=None, request_id=None),
        limit=20,
        offset=0,
    )

    assert result["records"][0]["username"] == "已删除用户"
    assert result["records"][0]["api_key_name"] == "已删除Key"


def test_records_hide_provider_fields_for_non_admin(monkeypatch) -> None:
    usage_row = SimpleNamespace(
        id="usage-1",
        request_id="req-1",
        created_at=datetime(2026, 3, 19, 12, 0, 0),
        user_id="user-1",
        username="NyaDoo",
        api_key_id="key-1",
        api_key_name="Demo Key",
        provider_api_key_id="provider-key-1",
        provider_name="openai",
        model="gpt-5.2",
        target_model=None,
        api_format="openai:chat",
        request_type="chat",
        status="completed",
        billing_status="completed",
        is_stream=False,
        has_format_conversion=False,
        status_code=200,
        error_message=None,
        error_category=None,
        response_time_ms=1234,
        first_byte_time_ms=321,
        input_tokens=100,
        output_tokens=50,
        input_output_total_tokens=150,
        cache_creation_input_tokens=0,
        cache_creation_input_tokens_5m=0,
        cache_creation_input_tokens_1h=0,
        cache_read_input_tokens=0,
        input_context_tokens=100,
        total_tokens=150,
        input_cost_usd=0.1,
        output_cost_usd=0.2,
        cache_creation_cost_usd=0.0,
        cache_creation_cost_usd_5m=0.0,
        cache_creation_cost_usd_1h=0.0,
        cache_read_cost_usd=0.0,
        cache_cost_usd=0.0,
        request_cost_usd=0.0,
        total_cost_usd=0.3,
        actual_total_cost_usd=0.3,
        actual_cache_cost_usd=0.0,
        rate_multiplier=1.0,
    )

    monkeypatch.setattr(
        AnalyticsQueryService,
        "build_usage_query",
        lambda *_args, **_kwargs: _FakeRecordsQuery([usage_row]),
    )
    monkeypatch.setattr(
        AnalyticsQueryService,
        "_load_request_execution_flags",
        lambda *_args, **_kwargs: ({}, {}),
    )

    db = _SequentialSession(
        results=[
            [("user-1", "NyaDoo")],
            [("key-1", "Demo Key")],
        ]
    )

    result = AnalyticsQueryService.records(
        cast(Any, db),
        SimpleNamespace(id="user-1", role=UserRole.USER),
        time_range=TimeRangeParams(start_date=date(2026, 3, 19), end_date=date(2026, 3, 19)),
        scope_kind="me",
        scope_user_id=None,
        scope_api_key_id=None,
        filters=_empty_filters(),
        search=SimpleNamespace(text=None, request_id=None),
        limit=20,
        offset=0,
    )

    assert result["records"][0]["provider_name"] is None
    assert result["records"][0]["provider_api_key_name"] is None


def test_performance_returns_readable_error_category_labels(monkeypatch) -> None:
    rows = [
        SimpleNamespace(
            created_at=datetime(2026, 3, 19, 12, 0, 0),
            provider_name="openai",
            error_category=ErrorCategory.SERVER_ERROR.value,
            status="failed",
            response_time_ms=1200,
            first_byte_time_ms=320,
        ),
        SimpleNamespace(
            created_at=datetime(2026, 3, 19, 13, 0, 0),
            provider_name="anthropic",
            error_category=None,
            status="failed",
            response_time_ms=900,
            first_byte_time_ms=210,
        ),
    ]

    monkeypatch.setattr(
        AnalyticsQueryService,
        "build_usage_query",
        lambda *_args, **_kwargs: _FakePerformanceQuery(rows),
    )

    result = AnalyticsQueryService.performance(
        cast(Any, SimpleNamespace()),
        SimpleNamespace(id="admin-1", role=UserRole.ADMIN),
        time_range=TimeRangeParams(start_date=date(2026, 3, 19), end_date=date(2026, 3, 19)),
        scope_kind="global",
        scope_user_id=None,
        scope_api_key_id=None,
        filters=_empty_filters(),
    )

    assert result["errors"]["categories"] == [
        {
            "category": ErrorCategory.SERVER_ERROR.value,
            "label": "服务端错误",
            "count": 1,
        },
        {
            "category": ErrorCategory.UNKNOWN.value,
            "label": "未知错误",
            "count": 1,
        },
    ]
