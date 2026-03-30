from __future__ import annotations

from types import SimpleNamespace

import pytest

from src.services.payment.gateway.base import (
    CheckoutPayload,
    PaymentGateway,
    PaymentGatewayCapabilities,
)
from src.services.payment.gateway.registry import PaymentGatewayRegistry


class _DummyGateway(PaymentGateway):
    payment_method = "dummy"
    display_name = "Dummy"
    capabilities = PaymentGatewayCapabilities(
        supports_webhook=True,
        supports_active_query=True,
        supports_refund=True,
        checkout_mode="redirect",
    )
    success_channel_statuses = ("SUCCESS",)
    expired_channel_statuses = ("CLOSED",)
    failed_channel_statuses = ("FAILED",)
    default_pay_currency = "USD"
    default_exchange_rate = "1"

    def create_checkout(self, *, order: object) -> CheckoutPayload:
        return CheckoutPayload(
            gateway=self.payment_method,
            display_name=self.display_name,
            gateway_order_id=f"gw_{getattr(order, 'order_no', 'order')}",
            payment_url="https://example.com/pay",
            qr_code=None,
            expires_at=None,
        )


def test_registry_filters_methods_by_capability() -> None:
    registry = PaymentGatewayRegistry()

    query_only = _DummyGateway()
    refund_only = _DummyGateway()
    refund_only.payment_method = "refund_only"
    refund_only.capabilities = PaymentGatewayCapabilities(
        supports_webhook=False,
        supports_active_query=False,
        supports_refund=True,
        supports_return_url=False,
        checkout_mode="offline",
    )

    registry.register(query_only)
    registry.register(refund_only)

    assert registry.methods_supporting_webhook() == ("dummy",)
    assert registry.methods_supporting_active_query() == ("dummy",)
    assert registry.methods_supporting_refund() == ("dummy", "refund_only")


def test_registry_rejects_invalid_gateway_definition() -> None:
    registry = PaymentGatewayRegistry()
    invalid = _DummyGateway()
    invalid.display_name = ""

    with pytest.raises(ValueError, match="display_name"):
        registry.register(invalid)


def test_gateway_helper_builds_status_snapshot_from_channel_status() -> None:
    gateway = _DummyGateway()

    snapshot = gateway.build_status_snapshot(
        channel_status="SUCCESS",
        gateway_order_id="trade-1",
        pay_amount="12.34",
        raw={"status": "SUCCESS"},
    )

    assert snapshot.order_status == "paid"
    assert snapshot.channel_status == "SUCCESS"
    assert snapshot.gateway_order_id == "trade-1"
    assert snapshot.pay_currency == "USD"
    assert snapshot.exchange_rate == "1"
    assert snapshot.raw == {"status": "SUCCESS"}


def test_gateway_helper_builds_callback_payload_from_channel_status() -> None:
    gateway = _DummyGateway()

    parsed = gateway.build_parsed_callback_payload(
        callback_key="cb-1",
        order_no="order-1",
        channel_status="CLOSED",
        raw={"status": "CLOSED"},
    )

    assert parsed.callback_key == "cb-1"
    assert parsed.order_no == "order-1"
    assert parsed.order_status == "expired"
    assert parsed.pay_currency == "USD"
    assert parsed.exchange_rate == "1"
    assert parsed.raw == {"status": "CLOSED"}


def test_gateway_default_parse_callback_payload_uses_common_fields() -> None:
    gateway = _DummyGateway()

    parsed = gateway.parse_callback_payload(
        payload={
            "callback_key": "cb-2",
            "order_no": "order-2",
            "gateway_order_id": "gw-2",
            "pay_amount": "6.66",
            "channel_status": "FAILED",
        }
    )

    assert parsed.callback_key == "cb-2"
    assert parsed.gateway_order_id == "gw-2"
    assert parsed.order_status == "failed"
    assert parsed.pay_amount == "6.66"
    assert parsed.pay_currency == "USD"


def test_gateway_create_checkout_payload_flattens_raw_fields() -> None:
    gateway = _DummyGateway()
    payload = gateway.create_checkout_payload(order=SimpleNamespace(order_no="order-3"))

    assert payload["gateway"] == "dummy"
    assert payload["display_name"] == "Dummy"
    assert payload["gateway_order_id"] == "gw_order-3"
