from __future__ import annotations

from types import SimpleNamespace
from typing import Any
from unittest.mock import MagicMock

import pytest

from src.services.usage._billing_integration import UsageBillingIntegrationMixin
from src.services.usage._types import UsageRecordParams


class _TestUsageBillingIntegration(UsageBillingIntegrationMixin):
    @classmethod
    async def _get_rate_multiplier_and_free_tier(
        cls,
        db: Any,  # noqa: ARG003
        provider_api_key_id: str | None,  # noqa: ARG003
        provider_id: str | None,  # noqa: ARG003
        api_format: str | None = None,  # noqa: ARG003
    ) -> tuple[float, bool]:
        return 1.0, False


class _DummyBillingService:
    last_dimensions: dict[str, Any] | None = None

    def __init__(self, db: Any) -> None:  # noqa: D107, ARG002
        pass

    def calculate(
        self,
        *,
        task_type: str,  # noqa: ARG002
        model: str,  # noqa: ARG002
        provider_id: str,  # noqa: ARG002
        dimensions: dict[str, Any],
        strict_mode: bool | None,  # noqa: ARG002
    ) -> Any:
        _DummyBillingService.last_dimensions = dict(dimensions)
        snapshot = SimpleNamespace(
            cost_breakdown={
                "input_cost": 0.0,
                "output_cost": 0.0,
                "cache_creation_cost": 0.0,
                "cache_read_cost": 0.0,
                "request_cost": 0.0,
            },
            total_cost=0.0,
            resolved_variables={},
            to_dict=lambda: {},
        )
        return SimpleNamespace(snapshot=snapshot)


class _SplitAwareBillingService:
    calls: list[dict[str, Any]] = []

    def __init__(self, db: Any) -> None:  # noqa: D107, ARG002
        pass

    def calculate(
        self,
        *,
        task_type: str,  # noqa: ARG002
        model: str,  # noqa: ARG002
        provider_id: str,  # noqa: ARG002
        dimensions: dict[str, Any],
        strict_mode: bool | None,  # noqa: ARG002
    ) -> Any:
        dims = dict(dimensions)
        _SplitAwareBillingService.calls.append(dims)

        cache_creation_tokens = int(dims.get("cache_creation_input_tokens") or 0)
        input_tokens = int(dims.get("input_tokens") or 0)
        output_tokens = int(dims.get("output_tokens") or 0)
        cache_read_tokens = int(dims.get("cache_read_input_tokens") or 0)
        request_count = int(dims.get("request_count") or 0)
        ttl = dims.get("cache_ttl_minutes")

        if cache_creation_tokens > 0:
            if ttl == 5:
                cache_creation_price = 3.75
            elif ttl == 60:
                cache_creation_price = 6.0
            else:
                cache_creation_price = 4.5
            cache_creation_cost = cache_creation_tokens * cache_creation_price / 1_000_000
            snapshot = SimpleNamespace(
                cost_breakdown={
                    "input_cost": 0.0,
                    "output_cost": 0.0,
                    "cache_creation_cost": cache_creation_cost,
                    "cache_read_cost": 0.0,
                    "request_cost": 0.0,
                },
                total_cost=cache_creation_cost,
                resolved_variables={
                    "cache_creation_price_per_1m": cache_creation_price,
                },
                to_dict=lambda: {
                    "resolved_dimensions": dims,
                    "resolved_variables": {
                        "cache_creation_price_per_1m": cache_creation_price,
                    },
                    "cost_breakdown": {
                        "cache_creation_cost": cache_creation_cost,
                    },
                    "total_cost": cache_creation_cost,
                    "status": "complete",
                },
            )
            return SimpleNamespace(snapshot=snapshot)

        input_price = 3.0
        output_price = 15.0
        cache_read_price = 0.3
        request_price = 0.01
        input_cost = input_tokens * input_price / 1_000_000
        output_cost = output_tokens * output_price / 1_000_000
        cache_read_cost = cache_read_tokens * cache_read_price / 1_000_000
        request_cost_value = request_count * request_price
        total_cost = input_cost + output_cost + cache_read_cost + request_cost_value
        snapshot = SimpleNamespace(
            cost_breakdown={
                "input_cost": input_cost,
                "output_cost": output_cost,
                "cache_creation_cost": 0.0,
                "cache_read_cost": cache_read_cost,
                "request_cost": request_cost_value,
            },
            total_cost=total_cost,
            resolved_variables={
                "input_price_per_1m": input_price,
                "output_price_per_1m": output_price,
                "cache_read_price_per_1m": cache_read_price,
                "price_per_request": request_price,
            },
            to_dict=lambda: {
                "resolved_dimensions": dims,
                "resolved_variables": {
                    "input_price_per_1m": input_price,
                    "output_price_per_1m": output_price,
                    "cache_read_price_per_1m": cache_read_price,
                    "price_per_request": request_price,
                },
                "cost_breakdown": {
                    "input_cost": input_cost,
                    "output_cost": output_cost,
                    "cache_read_cost": cache_read_cost,
                    "request_cost": request_cost_value,
                },
                "total_cost": total_cost,
                "status": "complete",
            },
        )
        return SimpleNamespace(snapshot=snapshot)


def _build_params(
    db: Any,
    *,
    cache_creation_input_tokens: int = 0,
    cache_read_input_tokens: int = 0,
    cache_creation_input_tokens_5m: int = 0,
    cache_creation_input_tokens_1h: int = 0,
    cache_ttl_minutes: int | None = None,
    provider_api_key_id: str | None = "pak-test",
) -> UsageRecordParams:
    return UsageRecordParams(
        db=db,
        user=None,
        api_key=None,
        provider="provider-x",
        model="claude-sonnet",
        input_tokens=100,
        output_tokens=50,
        cache_creation_input_tokens=cache_creation_input_tokens,
        cache_read_input_tokens=cache_read_input_tokens,
        cache_creation_input_tokens_5m=cache_creation_input_tokens_5m,
        cache_creation_input_tokens_1h=cache_creation_input_tokens_1h,
        request_type="chat",
        api_format="claude:chat",
        api_family="claude",
        endpoint_kind="chat",
        endpoint_api_format="claude:chat",
        has_format_conversion=False,
        is_stream=False,
        response_time_ms=123,
        first_byte_time_ms=None,
        status_code=200,
        error_message=None,
        metadata={},
        request_headers=None,
        request_body=None,
        provider_request_headers=None,
        provider_request_body=None,
        response_headers=None,
        client_response_headers=None,
        response_body=None,
        client_response_body=None,
        request_id="req-test",
        provider_id="provider-id",
        provider_endpoint_id="endpoint-id",
        provider_api_key_id=provider_api_key_id,
        status="completed",
        cache_ttl_minutes=cache_ttl_minutes,
        use_tiered_pricing=True,
        target_model=None,
    )


@pytest.mark.asyncio
async def test_prepare_usage_record_uses_provider_key_ttl_for_cache_read(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    db.query.return_value.filter.return_value.scalar.return_value = 60

    monkeypatch.setattr("src.services.billing.service.BillingService", _DummyBillingService)
    monkeypatch.setattr(
        "src.services.usage._billing_integration.sanitize_request_metadata",
        lambda metadata: metadata,
    )
    monkeypatch.setattr(
        "src.services.usage._billing_integration.build_usage_params",
        lambda **kwargs: {"total_cost_usd": 0.0, "actual_total_cost_usd": 0.0},
    )

    params = _build_params(db, cache_read_input_tokens=321)
    await _TestUsageBillingIntegration._prepare_usage_record(params)

    assert _DummyBillingService.last_dimensions is not None
    assert _DummyBillingService.last_dimensions.get("cache_ttl_minutes") == 60


@pytest.mark.asyncio
async def test_prepare_usage_record_prefers_explicit_cache_ttl(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    db.query.return_value.filter.return_value.scalar.return_value = 60

    monkeypatch.setattr("src.services.billing.service.BillingService", _DummyBillingService)
    monkeypatch.setattr(
        "src.services.usage._billing_integration.sanitize_request_metadata",
        lambda metadata: metadata,
    )
    monkeypatch.setattr(
        "src.services.usage._billing_integration.build_usage_params",
        lambda **kwargs: {"total_cost_usd": 0.0, "actual_total_cost_usd": 0.0},
    )

    params = _build_params(db, cache_read_input_tokens=123, cache_ttl_minutes=5)
    await _TestUsageBillingIntegration._prepare_usage_record(params)

    assert _DummyBillingService.last_dimensions is not None
    assert _DummyBillingService.last_dimensions.get("cache_ttl_minutes") == 5


@pytest.mark.asyncio
async def test_prepare_usage_record_infers_ttl_from_1h_cache_split(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()

    monkeypatch.setattr("src.services.billing.service.BillingService", _DummyBillingService)
    monkeypatch.setattr(
        "src.services.usage._billing_integration.sanitize_request_metadata",
        lambda metadata: metadata,
    )
    monkeypatch.setattr(
        "src.services.usage._billing_integration.build_usage_params",
        lambda **kwargs: {"total_cost_usd": 0.0, "actual_total_cost_usd": 0.0},
    )

    params = _build_params(
        db,
        provider_api_key_id=None,
        cache_creation_input_tokens=1000,
        cache_creation_input_tokens_1h=1000,
    )
    await _TestUsageBillingIntegration._prepare_usage_record(params)

    assert _DummyBillingService.last_dimensions is not None
    assert _DummyBillingService.last_dimensions.get("cache_ttl_minutes") == 60


@pytest.mark.asyncio
async def test_prepare_usage_record_prefers_cache_split_over_provider_key_ttl(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    db.query.return_value.filter.return_value.scalar.return_value = 60

    monkeypatch.setattr("src.services.billing.service.BillingService", _DummyBillingService)
    monkeypatch.setattr(
        "src.services.usage._billing_integration.sanitize_request_metadata",
        lambda metadata: metadata,
    )
    monkeypatch.setattr(
        "src.services.usage._billing_integration.build_usage_params",
        lambda **kwargs: {"total_cost_usd": 0.0, "actual_total_cost_usd": 0.0},
    )

    params = _build_params(
        db,
        cache_creation_input_tokens=258,
        cache_creation_input_tokens_5m=258,
    )
    await _TestUsageBillingIntegration._prepare_usage_record(params)

    assert _DummyBillingService.last_dimensions is not None
    assert _DummyBillingService.last_dimensions.get("cache_ttl_minutes") == 5


@pytest.mark.asyncio
async def test_prepare_usage_record_bills_mixed_cache_creation_ttls_separately(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    _SplitAwareBillingService.calls = []

    monkeypatch.setattr("src.services.billing.service.BillingService", _SplitAwareBillingService)
    monkeypatch.setattr(
        "src.services.usage._billing_integration.sanitize_request_metadata",
        lambda metadata: metadata,
    )

    captured: dict[str, Any] = {}

    def _capture_build_usage_params(**kwargs: Any) -> dict[str, Any]:
        captured.update(kwargs)
        return {"total_cost_usd": 0.0, "actual_total_cost_usd": 0.0}

    monkeypatch.setattr(
        "src.services.usage._billing_integration.build_usage_params",
        _capture_build_usage_params,
    )

    params = _build_params(
        db,
        provider_api_key_id=None,
        cache_creation_input_tokens=300,
        cache_creation_input_tokens_5m=200,
        cache_creation_input_tokens_1h=100,
    )
    await _TestUsageBillingIntegration._prepare_usage_record(params)

    cost = captured["cost"]
    assert len(_SplitAwareBillingService.calls) == 3
    assert all(call.get("total_input_context") == 400 for call in _SplitAwareBillingService.calls)
    assert any(
        call.get("cache_creation_input_tokens") == 200 and call.get("cache_ttl_minutes") == 5
        for call in _SplitAwareBillingService.calls
    )
    assert any(
        call.get("cache_creation_input_tokens") == 100 and call.get("cache_ttl_minutes") == 60
        for call in _SplitAwareBillingService.calls
    )
    assert cost.cache_creation_cost_5m == pytest.approx(0.00075)
    assert cost.cache_creation_cost_1h == pytest.approx(0.0006)
    assert cost.cache_creation_cost == pytest.approx(0.00135)
    assert cost.cache_creation_price is None
    assert cost.cache_creation_price_5m == pytest.approx(3.75)
    assert cost.cache_creation_price_1h == pytest.approx(6.0)
    assert cost.total_cost == pytest.approx(0.01105 + 0.00075 + 0.0006)
    assert captured["metadata"]["billing_snapshot"]["cache_creation_split"]["5m"]["cost"] == pytest.approx(
        0.00075
    )
    assert captured["metadata"]["billing_snapshot"]["cache_creation_split"]["1h"]["cost"] == pytest.approx(
        0.0006
    )


@pytest.mark.asyncio
async def test_prepare_usage_record_deserializes_body_json_before_build_params(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()

    monkeypatch.setattr("src.services.billing.service.BillingService", _DummyBillingService)
    monkeypatch.setattr(
        "src.services.usage._billing_integration.sanitize_request_metadata",
        lambda metadata: metadata,
    )

    captured: dict[str, Any] = {}

    def _capture_build_usage_params(**kwargs: Any) -> dict[str, Any]:
        captured.update(kwargs)
        return {"total_cost_usd": 0.0, "actual_total_cost_usd": 0.0}

    monkeypatch.setattr(
        "src.services.usage._billing_integration.build_usage_params",
        _capture_build_usage_params,
    )

    params = _build_params(db)
    params.request_body = '{"messages":[{"role":"user","content":"hello"}]}'
    params.provider_request_body = '{"tools":[{"name":"calc"}]}'
    params.response_body = '{"choices":[{"index":0}]}'
    params.client_response_body = '{"output":[{"type":"text"}]}'

    await _TestUsageBillingIntegration._prepare_usage_record(params)

    assert isinstance(captured["request_body"], dict)
    assert captured["request_body"]["messages"][0]["content"] == "hello"
    assert isinstance(captured["provider_request_body"], dict)
    assert isinstance(captured["response_body"], dict)
    assert isinstance(captured["client_response_body"], dict)


@pytest.mark.asyncio
async def test_prepare_usage_record_keeps_invalid_json_body_string(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()

    monkeypatch.setattr("src.services.billing.service.BillingService", _DummyBillingService)
    monkeypatch.setattr(
        "src.services.usage._billing_integration.sanitize_request_metadata",
        lambda metadata: metadata,
    )

    captured: dict[str, Any] = {}

    def _capture_build_usage_params(**kwargs: Any) -> dict[str, Any]:
        captured.update(kwargs)
        return {"total_cost_usd": 0.0, "actual_total_cost_usd": 0.0}

    monkeypatch.setattr(
        "src.services.usage._billing_integration.build_usage_params",
        _capture_build_usage_params,
    )

    invalid_json = '{"content":"x...[truncated]'
    params = _build_params(db)
    params.request_body = invalid_json

    await _TestUsageBillingIntegration._prepare_usage_record(params)

    assert captured["request_body"] == invalid_json
