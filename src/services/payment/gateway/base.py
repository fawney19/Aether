from __future__ import annotations

import hashlib
import hmac
import json
from abc import ABC, abstractmethod
from dataclasses import asdict, dataclass, field
from typing import Any


@dataclass(slots=True)
class PaymentGatewayCapabilities:
    supports_webhook: bool = True
    supports_active_query: bool = False
    supports_return_url: bool = True
    supports_refund: bool = False
    checkout_mode: str = "redirect"


@dataclass(slots=True)
class CheckoutPayload:
    gateway: str
    display_name: str
    gateway_order_id: str | None
    payment_url: str | None
    qr_code: str | None
    expires_at: str | None
    instructions: str | None = None
    raw: dict[str, Any] = field(default_factory=dict)

    def to_dict(self) -> dict[str, Any]:
        payload = asdict(self)
        raw = payload.pop("raw", None) or {}
        payload.update(raw)
        return payload


@dataclass(slots=True)
class PaymentStatusSnapshot:
    order_status: str
    channel_status: str | None = None
    gateway_order_id: str | None = None
    pay_amount: str | None = None
    pay_currency: str | None = None
    exchange_rate: str | None = None
    error: str | None = None
    raw: dict[str, Any] = field(default_factory=dict)


@dataclass(slots=True)
class ParsedCallbackPayload:
    callback_key: str
    order_no: str | None = None
    gateway_order_id: str | None = None
    pay_amount: str | None = None
    pay_currency: str | None = None
    exchange_rate: str | None = None
    order_status: str | None = None
    channel_status: str | None = None
    raw: dict[str, Any] = field(default_factory=dict)


class PaymentGateway(ABC):
    """支付网关抽象。

    当前阶段只提供统一结构和占位返回，便于后续接入真实 SDK。
    """

    payment_method: str
    display_name: str
    capabilities = PaymentGatewayCapabilities()
    success_channel_statuses: tuple[str, ...] = ()
    expired_channel_statuses: tuple[str, ...] = ()
    failed_channel_statuses: tuple[str, ...] = ()
    default_pay_currency: str | None = None
    default_exchange_rate: str | None = None

    def validate_config(self) -> None:
        """校验通道配置。默认无需额外校验。"""

    def validate_definition(self) -> None:
        payment_method = (self.payment_method or "").strip()
        display_name = (self.display_name or "").strip()
        if not payment_method:
            raise ValueError("gateway payment_method is required")
        if not display_name:
            raise ValueError(f"gateway display_name is required: {payment_method}")
        if not isinstance(self.capabilities, PaymentGatewayCapabilities):
            raise ValueError(
                f"gateway capabilities must be PaymentGatewayCapabilities: {payment_method}"
            )

    @abstractmethod
    def create_checkout(self, *, order: Any) -> CheckoutPayload:
        """为前端返回统一的支付指引结构。"""

    def create_checkout_payload(self, *, order: Any) -> dict[str, Any]:
        return self.create_checkout(order=order).to_dict()

    def query_order_status(
        self,
        *,
        order_no: str,
        gateway_order_id: str | None = None,
    ) -> dict[str, Any]:
        raise NotImplementedError(f"{self.payment_method} does not support active order status query")

    @staticmethod
    def build_callback_signature(
        *,
        payload: dict[str, Any] | None,
        callback_secret: str | None,
    ) -> str | None:
        if payload is None:
            return None
        if not callback_secret:
            return None
        canonical = json.dumps(
            payload,
            sort_keys=True,
            ensure_ascii=False,
            separators=(",", ":"),
            default=str,
        )
        return hmac.new(
            callback_secret.encode("utf-8"),
            canonical.encode("utf-8"),
            hashlib.sha256,
        ).hexdigest()

    def verify_callback_payload(
        self,
        *,
        payload: dict[str, Any] | None,
        callback_signature: str | None = None,
        callback_secret: str | None = None,
    ) -> bool:
        """校验回调。

        默认使用 HMAC-SHA256 对 payload 进行签名校验。
        真实接入时可由各支付渠道覆写该方法使用官方 SDK 验签。
        """
        expected_signature = self.build_callback_signature(
            payload=payload,
            callback_secret=callback_secret,
        )
        if expected_signature is None:
            return False
        provided = (callback_signature or "").strip()
        if not provided:
            return False
        if provided.lower().startswith("sha256="):
            provided = provided.split("=", 1)[1]
        return hmac.compare_digest(provided.lower(), expected_signature.lower())

    def parse_callback_payload(self, *, payload: dict[str, Any] | None) -> ParsedCallbackPayload:
        data = dict(payload or {})
        callback_key = str(
            data.get("callback_key")
            or data.get("notify_id")
            or data.get("event_id")
            or data.get("id")
            or data.get("order_no")
            or data.get("out_trade_no")
            or "callback"
        )
        pay_amount = data.get("pay_amount")
        if pay_amount is None:
            pay_amount = data.get("amount_usd")
        return self.build_parsed_callback_payload(
            callback_key=callback_key,
            order_no=_string_or_none(data.get("order_no")),
            gateway_order_id=_string_or_none(data.get("gateway_order_id")),
            pay_amount=_string_or_none(pay_amount),
            pay_currency=_string_or_none(data.get("pay_currency")),
            exchange_rate=_string_or_none(data.get("exchange_rate")),
            order_status=_string_or_none(data.get("order_status")),
            channel_status=_string_or_none(data.get("channel_status")),
            raw=data,
        )

    def normalize_query_response(
        self,
        *,
        order: Any,
        query_response: dict[str, Any],
    ) -> PaymentStatusSnapshot:
        raise NotImplementedError(
            f"{self.payment_method} does not support standardized status mapping"
        )

    def map_channel_status_to_order_status(
        self,
        *,
        channel_status: str | None,
        default: str = "pending",
    ) -> str:
        normalized = (channel_status or "").strip()
        if not normalized:
            return default
        if normalized in self.success_channel_statuses:
            return "paid"
        if normalized in self.expired_channel_statuses:
            return "expired"
        if normalized in self.failed_channel_statuses:
            return "failed"
        return default

    def build_status_snapshot(
        self,
        *,
        channel_status: str | None,
        gateway_order_id: str | None = None,
        pay_amount: str | None = None,
        pay_currency: str | None = None,
        exchange_rate: str | None = None,
        error: str | None = None,
        raw: dict[str, Any] | None = None,
        order_status: str | None = None,
        default_order_status: str = "pending",
    ) -> PaymentStatusSnapshot:
        resolved_channel_status = _string_or_none(channel_status)
        resolved_order_status = order_status or self.map_channel_status_to_order_status(
            channel_status=resolved_channel_status,
            default=default_order_status,
        )
        return PaymentStatusSnapshot(
            order_status=resolved_order_status,
            channel_status=resolved_channel_status,
            gateway_order_id=_string_or_none(gateway_order_id),
            pay_amount=_string_or_none(pay_amount),
            pay_currency=_string_or_none(pay_currency) or self.default_pay_currency,
            exchange_rate=_string_or_none(exchange_rate) or self.default_exchange_rate,
            error=_string_or_none(error),
            raw=dict(raw or {}),
        )

    def build_parsed_callback_payload(
        self,
        *,
        callback_key: str,
        order_no: str | None = None,
        gateway_order_id: str | None = None,
        pay_amount: str | None = None,
        pay_currency: str | None = None,
        exchange_rate: str | None = None,
        order_status: str | None = None,
        channel_status: str | None = None,
        raw: dict[str, Any] | None = None,
        default_order_status: str | None = None,
    ) -> ParsedCallbackPayload:
        resolved_channel_status = _string_or_none(channel_status)
        resolved_order_status = _string_or_none(order_status)
        if resolved_order_status is None and resolved_channel_status is not None:
            resolved_order_status = self.map_channel_status_to_order_status(
                channel_status=resolved_channel_status,
                default=default_order_status or "pending",
            )
        return ParsedCallbackPayload(
            callback_key=str(callback_key).strip() or "callback",
            order_no=_string_or_none(order_no),
            gateway_order_id=_string_or_none(gateway_order_id),
            pay_amount=_string_or_none(pay_amount),
            pay_currency=_string_or_none(pay_currency) or self.default_pay_currency,
            exchange_rate=_string_or_none(exchange_rate) or self.default_exchange_rate,
            order_status=resolved_order_status,
            channel_status=resolved_channel_status,
            raw=dict(raw or {}),
        )


def _string_or_none(value: Any) -> str | None:
    if value is None:
        return None
    text = str(value).strip()
    return text or None
