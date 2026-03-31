from __future__ import annotations

from decimal import Decimal
from unittest.mock import MagicMock

from src.services.usage._recording_helpers import build_usage_params
from src.services.usage._types import UsageCostInfo


def test_build_usage_params_uses_single_cache_ttl(
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
        response_body={
            "usage": {
                "input_tokens": 100,
                "output_tokens": 50,
                "claude_cache_creation_1_h_tokens": 300,
            }
        },
        client_response_body=None,
        request_id="req-1",
        provider_id="provider-1",
        provider_endpoint_id="endpoint-1",
        provider_api_key_id="pak-1",
        model_group_id=None,
        model_group_route_id=None,
        status="completed",
        target_model=None,
        cost=UsageCostInfo(
            cache_creation_cost=1.35,
            cache_creation_price=4.5,
            actual_rate_multiplier=2.0,
            is_free_tier=False,
        ),
    )

    assert result["cache_ttl_minutes"] == 60
    assert result["cache_creation_cost_usd"] == Decimal("1.35000000")
    assert result["actual_cache_creation_cost_usd"] == Decimal("2.70000000")
    assert result["cache_creation_price_per_1m"] == 4.5
    assert "cache_creation_cost_usd_5m" not in result
    assert "cache_creation_cost_usd_1h" not in result
