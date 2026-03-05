from __future__ import annotations

from typing import Any

from src.services.payment.gateway.base import PaymentGateway


class ManualGateway(PaymentGateway):
    payment_method = "manual"
    display_name = "人工打款"

    def create_checkout_payload(self, *, order: Any) -> dict[str, Any]:
        gateway_order_id = getattr(order, "gateway_order_id", None) or f"manual_{order.order_no}"
        return {
            "gateway": self.payment_method,
            "display_name": self.display_name,
            "gateway_order_id": gateway_order_id,
            "payment_url": None,
            "qr_code": None,
            "instructions": "请线下确认到账后由管理员处理",
            "expires_at": getattr(order, "expires_at", None),
        }
