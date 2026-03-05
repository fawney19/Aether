from __future__ import annotations

from abc import ABC, abstractmethod
import hashlib
import hmac
import json
from typing import Any


class PaymentGateway(ABC):
    """支付网关抽象。

    当前阶段只提供统一结构和占位返回，便于后续接入真实 SDK。
    """

    payment_method: str
    display_name: str

    @abstractmethod
    def create_checkout_payload(self, *, order: Any) -> dict[str, Any]:
        """为前端返回统一的支付指引结构。"""

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
