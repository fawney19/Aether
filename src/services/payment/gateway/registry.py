from __future__ import annotations

from typing import Iterable

from src.services.payment.gateway.base import PaymentGateway


class PaymentGatewayRegistry:
    def __init__(self) -> None:
        self._gateways: dict[str, PaymentGateway] = {}

    def register(self, gateway: PaymentGateway) -> None:
        gateway.validate_definition()
        key = (gateway.payment_method or "").strip().lower()
        self._gateways[key] = gateway

    def get(self, payment_method: str) -> PaymentGateway:
        key = (payment_method or "").strip().lower()
        gateway = self._gateways.get(key)
        if gateway is None:
            raise ValueError(f"unsupported payment_method: {payment_method}")
        return gateway

    def all(self) -> Iterable[PaymentGateway]:
        return tuple(self._gateways.values())

    def methods_supporting_webhook(self) -> tuple[str, ...]:
        return tuple(
            gateway.payment_method
            for gateway in self._gateways.values()
            if gateway.capabilities.supports_webhook
        )

    def methods_supporting_active_query(self) -> tuple[str, ...]:
        return tuple(
            gateway.payment_method
            for gateway in self._gateways.values()
            if gateway.capabilities.supports_active_query
        )

    def methods_supporting_refund(self) -> tuple[str, ...]:
        return tuple(
            gateway.payment_method
            for gateway in self._gateways.values()
            if gateway.capabilities.supports_refund
        )
