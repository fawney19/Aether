from __future__ import annotations

from typing import Any

from src.services.payment.gateway.base import PaymentGateway


class AlipayGateway(PaymentGateway):
    payment_method = "alipay"
    display_name = "支付宝"

    def create_checkout_payload(self, *, order: Any) -> dict[str, Any]:
        gateway_order_id = getattr(order, "gateway_order_id", None) or f"ali_{order.order_no}"
        return {
            "gateway": self.payment_method,
            "display_name": self.display_name,
            "gateway_order_id": gateway_order_id,
            "payment_url": f"/pay/mock/alipay/{order.order_no}",
            "qr_code": f"mock://alipay/{order.order_no}",
            "expires_at": getattr(order, "expires_at", None),
        }
