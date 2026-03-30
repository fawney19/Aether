from __future__ import annotations

from typing import Any

from src.services.payment.gateway.base import CheckoutPayload, PaymentGateway, PaymentGatewayCapabilities


class WeChatGateway(PaymentGateway):
    payment_method = "wechat"
    display_name = "微信支付"
    capabilities = PaymentGatewayCapabilities(
        supports_webhook=True,
        supports_active_query=False,
        supports_return_url=True,
        supports_refund=False,
        checkout_mode="qr_code",
    )

    def create_checkout(self, *, order: Any) -> CheckoutPayload:
        gateway_order_id = getattr(order, "gateway_order_id", None) or f"wx_{order.order_no}"
        expires_at = getattr(order, "expires_at", None)
        return CheckoutPayload(
            gateway=self.payment_method,
            display_name=self.display_name,
            gateway_order_id=gateway_order_id,
            payment_url=f"/pay/mock/wechat/{order.order_no}",
            qr_code=f"mock://wechat/{order.order_no}",
            expires_at=expires_at.isoformat() if expires_at else None,
        )
