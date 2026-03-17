from __future__ import annotations

from datetime import datetime, timezone
from decimal import Decimal
from types import SimpleNamespace
from typing import Any

import pytest

from src.api.admin.usage.routes import (
    AdminUsageDetailAdapter,
    AdminUsageRecordsAdapter,
    _resolve_replay_model_name,
)


class _FakeQuery:
    def __init__(
        self,
        *,
        scalar_result: int | None = None,
        all_result: list[Any] | None = None,
        first_result: Any = None,
    ) -> None:
        self.scalar_result = scalar_result
        self.all_result = all_result or []
        self.first_result = first_result
        self.options_args: tuple[Any, ...] = ()

    def outerjoin(self, *args: Any, **kwargs: Any) -> _FakeQuery:
        return self

    def join(self, *args: Any, **kwargs: Any) -> _FakeQuery:
        return self

    def filter(self, *args: Any, **kwargs: Any) -> _FakeQuery:
        return self

    def options(self, *args: Any) -> _FakeQuery:
        self.options_args = args
        return self

    def order_by(self, *args: Any, **kwargs: Any) -> _FakeQuery:
        return self

    def offset(self, *args: Any, **kwargs: Any) -> _FakeQuery:
        return self

    def limit(self, *args: Any, **kwargs: Any) -> _FakeQuery:
        return self

    def scalar(self) -> int | None:
        return self.scalar_result

    def all(self) -> list[Any]:
        return self.all_result

    def first(self) -> Any:
        return self.first_result


class _FakeDb:
    def __init__(self, queries: list[_FakeQuery]) -> None:
        self._queries = queries
        self.query_calls: list[tuple[Any, ...]] = []

    def query(self, *args: Any) -> _FakeQuery:
        self.query_calls.append(args)
        return self._queries[len(self.query_calls) - 1]


@pytest.mark.asyncio
async def test_admin_usage_records_returns_model_version_without_request_metadata(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr("src.utils.cache_decorator.get_redis_client_sync", lambda: None)

    usage = SimpleNamespace(
        id="usage-1",
        request_id=None,
        user_id="user-1",
        api_key_id=None,
        provider_name="google",
        provider_id=None,
        provider_endpoint_id=None,
        provider_api_key_id=None,
        model="gemini-2.5-pro",
        target_model=None,
        input_tokens=120,
        output_tokens=80,
        cache_creation_input_tokens=0,
        cache_read_input_tokens=0,
        total_tokens=200,
        total_cost_usd=Decimal("1.25"),
        actual_total_cost_usd=Decimal("1.25"),
        rate_multiplier=Decimal("1.0"),
        response_time_ms=850,
        first_byte_time_ms=230,
        created_at=datetime(2026, 3, 9, 8, 30, tzinfo=timezone.utc),
        is_stream=False,
        status_code=200,
        error_message=None,
        status="completed",
        api_format="gemini:chat",
        endpoint_api_format=None,
        has_format_conversion=False,
        input_price_per_1m=Decimal("0.10"),
        output_price_per_1m=Decimal("0.30"),
        cache_creation_price_per_1m=None,
        cache_read_price_per_1m=None,
    )
    user = SimpleNamespace(id="user-1", email="user@example.com", username="tester")

    count_query = _FakeQuery(scalar_result=1)
    data_query = _FakeQuery(
        all_result=[
            (usage, user, None, None, None, "gemini-2.5-pro-001"),
        ]
    )
    db = _FakeDb([count_query, data_query])
    context = SimpleNamespace(
        db=db,
        user=SimpleNamespace(id="admin-1"),
        add_audit_metadata=lambda **_: None,
    )

    adapter = AdminUsageRecordsAdapter(
        time_range=None,
        search=None,
        user_id=None,
        username=None,
        model=None,
        provider=None,
        api_format=None,
        status=None,
        limit=100,
        offset=0,
    )

    result = await adapter.handle(context)

    assert len(db.query_calls) == 2
    assert len(db.query_calls[1]) == 6
    assert getattr(db.query_calls[1][-1], "name", None) == "model_version"

    record = result["records"][0]
    assert record["model_version"] == "gemini-2.5-pro-001"
    assert "request_metadata" not in record

    usage_load_only = data_query.options_args[0]
    usage_paths = {str(option.path) for option in usage_load_only.context}
    assert "ORM Path[Mapper[Usage(usage)] -> Usage.request_metadata]" not in usage_paths


@pytest.mark.asyncio
async def test_admin_usage_detail_defers_large_body_columns_when_bodies_excluded(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    async def _fake_get_tiered_pricing_info(
        self: AdminUsageDetailAdapter,
        db: Any,
        usage_record: Any,
    ) -> None:
        return None

    monkeypatch.setattr(
        AdminUsageDetailAdapter,
        "_get_tiered_pricing_info",
        _fake_get_tiered_pricing_info,
    )
    monkeypatch.setattr(
        AdminUsageDetailAdapter,
        "_extract_video_billing_info",
        lambda self, usage_record: None,
    )

    class _UsageRecord:
        id = "usage-1"
        request_id = "req-1"
        user_id = "user-1"
        api_key_id = "key-1"
        provider_name = "openai"
        api_format = "openai:cli"
        model = "gpt-5.4"
        target_model = None
        input_tokens = 10
        output_tokens = 20
        total_tokens = 30
        cache_creation_input_tokens = 0
        cache_read_input_tokens = 0
        cache_creation_input_tokens_5m = 0
        cache_creation_input_tokens_1h = 0
        input_cost_usd = Decimal("0.001")
        output_cost_usd = Decimal("0.002")
        total_cost_usd = Decimal("0.003")
        cache_creation_cost_usd = Decimal("0")
        cache_read_cost_usd = Decimal("0")
        request_cost_usd = Decimal("0")
        input_price_per_1m = Decimal("0.1")
        output_price_per_1m = Decimal("0.2")
        cache_creation_price_per_1m = None
        cache_read_price_per_1m = None
        price_per_request = None
        request_type = "chat"
        is_stream = True
        status_code = 200
        error_message = None
        status = "completed"
        response_time_ms = 1200
        first_byte_time_ms = 200
        created_at = datetime(2026, 3, 12, 7, 0, tzinfo=timezone.utc)
        request_headers = {"x-test": "1"}
        provider_request_headers = {"authorization": "***"}
        response_headers = {"content-type": "text/event-stream"}
        client_response_headers = {"content-type": "text/event-stream"}
        request_metadata = {"trace_id": "trace-1"}

        def get_request_body(self) -> Any:
            raise AssertionError("request body should not be loaded")

        def get_provider_request_body(self) -> Any:
            raise AssertionError("provider request body should not be loaded")

        def get_response_body(self) -> Any:
            raise AssertionError("response body should not be loaded")

        def get_client_response_body(self) -> Any:
            raise AssertionError("client response body should not be loaded")

    class _ApiKeyRecord:
        id = "key-1"
        name = "Primary"

        def get_display_key(self) -> str:
            return "sk-test"

    usage_query = _FakeQuery(
        first_result=(_UsageRecord(), True, True, True, True),
    )
    user_query = _FakeQuery(
        first_result=SimpleNamespace(id="user-1", username="tester", email="u@example.com"),
    )
    api_key_query = _FakeQuery(first_result=_ApiKeyRecord())
    db = _FakeDb([usage_query, user_query, api_key_query])
    context = SimpleNamespace(
        db=db,
        user=SimpleNamespace(id="admin-1"),
        add_audit_metadata=lambda **_: None,
    )

    adapter = AdminUsageDetailAdapter(usage_id="usage-1", include_bodies=False)
    result = await adapter.handle(context)  # type: ignore[arg-type]

    assert result["request_body"] is None
    assert result["provider_request_body"] is None
    assert result["response_body"] is None
    assert result["client_response_body"] is None
    assert result["has_request_body"] is True
    assert result["has_provider_request_body"] is True
    assert result["has_response_body"] is True
    assert result["has_client_response_body"] is True

    deferred_paths = {
        str(context.path)
        for option in usage_query.options_args
        for context in getattr(option, "context", ())
    }
    assert "ORM Path[Mapper[Usage(usage)] -> Usage.request_body]" in deferred_paths
    assert "ORM Path[Mapper[Usage(usage)] -> Usage.provider_request_body]" in deferred_paths
    assert "ORM Path[Mapper[Usage(usage)] -> Usage.response_body]" in deferred_paths
    assert "ORM Path[Mapper[Usage(usage)] -> Usage.client_response_body]" in deferred_paths
    assert "ORM Path[Mapper[Usage(usage)] -> Usage.request_body_compressed]" in deferred_paths
    assert (
        "ORM Path[Mapper[Usage(usage)] -> Usage.provider_request_body_compressed]" in deferred_paths
    )
    assert "ORM Path[Mapper[Usage(usage)] -> Usage.response_body_compressed]" in deferred_paths
    assert (
        "ORM Path[Mapper[Usage(usage)] -> Usage.client_response_body_compressed]" in deferred_paths
    )


@pytest.mark.asyncio
@pytest.mark.parametrize(
    ("same_provider", "same_endpoint"),
    [
        (True, False),
        (False, False),
    ],
    ids=["same-provider-cross-endpoint", "cross-provider"],
)
async def test_resolve_replay_model_name_falls_back_to_source_model_when_mapping_missing(
    monkeypatch: pytest.MonkeyPatch,
    same_provider: bool,
    same_endpoint: bool,
) -> None:
    class _FakeMapper:
        def __init__(self, db: Any) -> None:
            self.db = db

        async def get_mapping(self, source_model: str, provider_id: str) -> None:
            return None

    monkeypatch.setattr("src.services.model.mapper.ModelMapperMiddleware", _FakeMapper)

    resolved_model, mapping_source = await _resolve_replay_model_name(
        SimpleNamespace(),
        source_model="gpt-4o-mini",
        original_target_model="provider-specific-model",
        target_provider=SimpleNamespace(id="provider-2", name="OpenAI Compatible"),
        target_endpoint=SimpleNamespace(id="endpoint-2", api_format="openai:responses"),
        target_api_key=None,
        same_provider=same_provider,
        same_endpoint=same_endpoint,
        force_remap=False,
    )

    assert resolved_model == "gpt-4o-mini"
    assert mapping_source == "none"
