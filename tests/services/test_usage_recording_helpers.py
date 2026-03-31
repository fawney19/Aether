from __future__ import annotations

from decimal import Decimal
from unittest.mock import MagicMock

from src.services.usage._recording_helpers import build_usage_params
from src.services.usage._types import UsageCostInfo


def test_build_usage_params_preserves_explicit_cache_creation_split_costs(
    monkeypatch,
) -> None:
    monkeypatch.setattr(
        "src.services.usage._recording_helpers.SystemConfigService.should_log_headers",
        lambda db: False,
    )
    monkeypatch.setattr(
        "src.services.usage._recording_helpers.SystemConfigService.should_log_body",
        lambda db: False,
    )

    result = build_usage_params(
        db=MagicMock(),
        user=None,
        api_key=None,
        provider="anthropic",
        model="claude-sonnet",
        input_tokens=100,
        output_tokens=50,
        cache_creation_input_tokens=300,
        cache_read_input_tokens=0,
        cache_creation_input_tokens_5m=200,
        cache_creation_input_tokens_1h=100,
        request_type="chat",
        api_format="claude:chat",
        api_family="claude",
        endpoint_kind="chat",
        endpoint_api_format="claude:chat",
        has_format_conversion=False,
        is_stream=False,
        response_time_ms=120,
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
        request_id="req-1",
        provider_id="provider-1",
        provider_endpoint_id="endpoint-1",
        provider_api_key_id="pak-1",
        status="completed",
        target_model=None,
        cost=UsageCostInfo(
            cache_creation_cost=1.35,
            cache_creation_cost_5m=0.75,
            cache_creation_cost_1h=0.6,
            cache_creation_price_5m=3.75,
            cache_creation_price_1h=6.0,
            actual_rate_multiplier=2.0,
            is_free_tier=False,
        ),
    )

    assert result["cache_creation_cost_usd_5m"] == Decimal("0.75000000")
    assert result["cache_creation_cost_usd_1h"] == Decimal("0.60000000")
    assert result["actual_cache_creation_cost_usd_5m"] == Decimal("1.50000000")
    assert result["actual_cache_creation_cost_usd_1h"] == Decimal("1.20000000")
    assert result["cache_creation_price_per_1m_5m"] == 3.75
    assert result["cache_creation_price_per_1m_1h"] == 6.0
