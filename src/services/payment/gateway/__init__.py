from __future__ import annotations

from src.services.payment.gateway.alipay import AlipayGateway
from src.services.payment.gateway.base import (
    CheckoutPayload,
    ParsedCallbackPayload,
    PaymentGateway,
    PaymentGatewayCapabilities,
    PaymentStatusSnapshot,
)
from src.services.payment.gateway.manual import ManualGateway
from src.services.payment.gateway.registry import PaymentGatewayRegistry
from src.services.payment.gateway.wechat import WeChatGateway

_REGISTRY = PaymentGatewayRegistry()
for _gateway in (AlipayGateway(), WeChatGateway(), ManualGateway()):
    _REGISTRY.register(_gateway)


def get_payment_gateway(payment_method: str) -> PaymentGateway:
    return _REGISTRY.get(payment_method)


def get_payment_gateway_registry() -> PaymentGatewayRegistry:
    return _REGISTRY


__all__ = [
    "CheckoutPayload",
    "ParsedCallbackPayload",
    "PaymentGateway",
    "PaymentGatewayCapabilities",
    "PaymentStatusSnapshot",
    "get_payment_gateway",
    "get_payment_gateway_registry",
]
