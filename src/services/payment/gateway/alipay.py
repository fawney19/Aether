from __future__ import annotations

import json
import logging
from typing import Any

from alipay.aop.api.AlipayClientConfig import AlipayClientConfig
from alipay.aop.api.DefaultAlipayClient import DefaultAlipayClient
from alipay.aop.api.domain.AlipayTradeQueryModel import AlipayTradeQueryModel
from alipay.aop.api.domain.AlipayTradePagePayModel import AlipayTradePagePayModel
from alipay.aop.api.domain.AlipayTradeWapPayModel import AlipayTradeWapPayModel
from alipay.aop.api.request.AlipayTradeQueryRequest import AlipayTradeQueryRequest
from alipay.aop.api.request.AlipayTradePagePayRequest import AlipayTradePagePayRequest
from alipay.aop.api.request.AlipayTradeWapPayRequest import AlipayTradeWapPayRequest
from alipay.aop.api.util.SignatureUtils import verify_with_rsa

from src.config.settings import config
from src.services.payment.gateway.base import (
    CheckoutPayload,
    ParsedCallbackPayload,
    PaymentGateway,
    PaymentGatewayCapabilities,
    PaymentStatusSnapshot,
)


class AlipayGateway(PaymentGateway):
    payment_method = "alipay"
    display_name = "支付宝"
    success_channel_statuses = ("TRADE_SUCCESS", "TRADE_FINISHED")
    expired_channel_statuses = ("TRADE_CLOSED",)
    default_pay_currency = "CNY"
    default_exchange_rate = "1"
    capabilities = PaymentGatewayCapabilities(
        supports_webhook=True,
        supports_active_query=True,
        supports_return_url=True,
        supports_refund=False,
        checkout_mode="redirect",
    )

    def validate_config(self) -> None:
        if not config.alipay_app_id or not config.alipay_private_key or not config.alipay_public_key:
            raise ValueError("Alipay configuration is missing in secure local files.")

    def _get_alipay_client(self) -> DefaultAlipayClient:
        self.validate_config()
        private_key = config.alipay_private_key
        public_key = config.alipay_public_key

        client_config = AlipayClientConfig()
        client_config.server_url = "https://openapi-sandbox.dl.alipaydev.com/gateway.do" if config.alipay_debug else "https://openapi.alipay.com/gateway.do"
        #  if config.alipay_debug else "https://openapi.alipay.com/gateway.do"
        client_config.app_id = config.alipay_app_id
        client_config.app_private_key = private_key
        client_config.alipay_public_key = public_key
        client_config.charset = "utf-8"
        client_config.sign_type = "RSA2"
        #定义DefaultAlipayClient对象后，alipay_client_config不得修改
        return DefaultAlipayClient(alipay_client_config=client_config)

    def create_checkout(self, *, order: Any) -> CheckoutPayload:
        gateway_order_id = getattr(order, "gateway_order_id", None)
        client = self._get_alipay_client()

        payable_amount = getattr(order, "pay_amount", None)
        if payable_amount is None:
            payable_amount = getattr(order, "amount_usd", 0)
        cny_amount = float(payable_amount)
        amount_str = f"{cny_amount:.2f}"
        
        notify_url = f"{config.alipay_base_url}{config.alipay_notify_path}"

        gateway_response = getattr(order, "gateway_response", {}) or {}
        client_type = gateway_response.get("client_type", "pc")

        if client_type == "h5":
            model = AlipayTradeWapPayModel()
            model.out_trade_no = getattr(order, "order_no")
            model.total_amount = amount_str
            model.subject = f"充值订单 - {getattr(order, 'order_no')}"
            model.product_code = "QUICK_WAP_WAY"

            request = AlipayTradeWapPayRequest(biz_model=model)
            request.notify_url = notify_url
            request.return_url = f"{config.alipay_base_url}{config.alipay_return_path}?client=h5"

            payment_url = client.page_execute(request, http_method="GET")
        else:
            model = AlipayTradePagePayModel()
            model.out_trade_no = getattr(order, "order_no")
            model.total_amount = amount_str
            model.subject = f"充值订单 - {getattr(order, 'order_no')}"
            model.product_code = "FAST_INSTANT_TRADE_PAY"

            request = AlipayTradePagePayRequest(biz_model=model)
            request.notify_url = notify_url
            request.return_url = f"{config.alipay_base_url}{config.alipay_return_path}"

            payment_url = client.page_execute(request, http_method="GET")
        return CheckoutPayload(
            gateway=self.payment_method,
            display_name=self.display_name,
            gateway_order_id=gateway_order_id,
            payment_url=payment_url,
            qr_code=None,
            expires_at=getattr(order, "expires_at").isoformat() if getattr(order, "expires_at", None) else None,
        )

    def query_order_status(
        self,
        *,
        order_no: str,
        gateway_order_id: str | None = None,
    ) -> dict[str, Any]:
        client = self._get_alipay_client()

        model = AlipayTradeQueryModel()
        model.out_trade_no = order_no
        if gateway_order_id:
            model.trade_no = gateway_order_id

        request = AlipayTradeQueryRequest(biz_model=model)
        response_text = client.execute(request)

        if not response_text:
            raise ValueError("empty alipay trade query response")

        response = json.loads(response_text)
        if not isinstance(response, dict):
            raise ValueError("unexpected alipay trade query response")
        return response

    def normalize_query_response(
        self,
        *,
        order: Any,
        query_response: dict[str, Any],
    ) -> PaymentStatusSnapshot:
        response_code = str(query_response.get("code") or "")
        sub_code = str(query_response.get("sub_code") or "")
        sub_msg = str(query_response.get("sub_msg") or query_response.get("msg") or "")
        trade_status = str(query_response.get("trade_status") or "")

        if response_code and response_code != "10000":
            return self.build_status_snapshot(
                channel_status=trade_status or None,
                gateway_order_id=str(query_response.get("trade_no") or "") or None,
                error=sub_msg or sub_code or response_code,
                raw=query_response,
            )

        return self.build_status_snapshot(
            channel_status=trade_status or None,
            gateway_order_id=str(query_response.get("trade_no") or "") or None,
            pay_amount=(
                str(query_response.get("total_amount"))
                if query_response.get("total_amount") is not None
                else None
            ),
            pay_currency=str(query_response.get("pay_currency") or "CNY"),
            exchange_rate="1",
            raw=query_response,
        )

    def parse_callback_payload(self, *, payload: dict[str, Any] | None) -> ParsedCallbackPayload:
        data = dict(payload or {})
        trade_status = str(data.get("trade_status") or "")
        callback_key = str(
            data.get("notify_id")
            or data.get("out_trade_no")
            or data.get("trade_no")
            or "alipay_callback"
        )
        return self.build_parsed_callback_payload(
            callback_key=f"alipay_{callback_key}",
            order_no=str(data.get("out_trade_no") or "") or None,
            gateway_order_id=str(data.get("trade_no") or "") or None,
            pay_amount=str(data.get("total_amount")) if data.get("total_amount") is not None else None,
            pay_currency="CNY",
            exchange_rate="1",
            channel_status=trade_status or None,
            raw=data,
        )

    def verify_callback_payload(
        self,
        *,
        payload: dict[str, Any] | None,
        callback_signature: str | None,
        callback_secret: str | None,
    ) -> bool:
        if not payload:
            return False
            
        data = dict(payload)
        signature = data.pop("sign", None)
        data.pop("sign_type", None) # Excluded from string to sign
        
        if not signature:
            return False
            
        try:
            public_key = config.alipay_public_key
            # Build string to sign: sort keys, filter empty/None, join with & and =
            sorted_keys = sorted([(str(k), str(v)) for k, v in data.items() if v is not None and v != ""])
            message = "&".join(f"{k}={v}" for k, v in sorted_keys)
            
            # verify_with_rsa expects a string for public_key
            return verify_with_rsa(public_key, message.encode("utf-8"), signature)
        except Exception as e:
            logging.error(f"Alipay signature verification failed: {e}")
            return False
