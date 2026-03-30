from __future__ import annotations

from typing import Any

from src.services.payment.gateway.base import CheckoutPayload, PaymentGateway, PaymentGatewayCapabilities


class ManualGateway(PaymentGateway):
    payment_method = "manual"
    display_name = "人工打款"
    capabilities = PaymentGatewayCapabilities(
        supports_webhook=False,
        supports_active_query=False,
        supports_return_url=False,
        supports_refund=False,
        checkout_mode="offline",
    )

    def create_checkout(self, *, order: Any) -> CheckoutPayload:
        gateway_order_id = getattr(order, "gateway_order_id", None) or f"manual_{order.order_no}"
        expires_at = getattr(order, "expires_at", None)
        return CheckoutPayload(
            gateway=self.payment_method,
            display_name=self.display_name,
            gateway_order_id=gateway_order_id,
            payment_url=None,
            qr_code=None,
            instructions="请线下确认到账后由管理员处理",
            expires_at=expires_at.isoformat() if expires_at else None,
        )
