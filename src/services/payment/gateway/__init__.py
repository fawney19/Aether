from __future__ import annotations

from src.services.payment.gateway.alipay import AlipayGateway
from src.services.payment.gateway.base import PaymentGateway
from src.services.payment.gateway.manual import ManualGateway
from src.services.payment.gateway.wechat import WeChatGateway

_GATEWAYS: dict[str, PaymentGateway] = {
    "alipay": AlipayGateway(),
    "wechat": WeChatGateway(),
    "manual": ManualGateway(payment_method="manual", display_name="人工充值"),
    "manual_review": ManualGateway(payment_method="manual_review", display_name="人工充值"),
}


def get_payment_gateway(payment_method: str) -> PaymentGateway:
    key = (payment_method or "").strip().lower()
    gateway = _GATEWAYS.get(key)
    if gateway is None:
        raise ValueError(f"unsupported payment_method: {payment_method}")
    return gateway


__all__ = ["PaymentGateway", "get_payment_gateway"]
