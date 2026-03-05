from __future__ import annotations

from decimal import Decimal
from types import SimpleNamespace
from unittest.mock import MagicMock

import pytest
from fastapi import FastAPI
from fastapi.testclient import TestClient

from src.api.admin.payments.routes import AdminPaymentOrderCreditAdapter
from src.api.payment.routes import router as payment_router
from src.config import config
from src.database import get_db
from src.models.database import PaymentOrder
from src.services.payment.gateway import get_payment_gateway

CALLBACK_SECRET = "test-callback-secret"


def _build_payment_app(db: MagicMock) -> TestClient:
    app = FastAPI()
    app.include_router(payment_router)
    app.dependency_overrides[get_db] = lambda: db
    return TestClient(app)


def _sign_payload(payload: dict[str, object]) -> str:
    gateway = get_payment_gateway("alipay")
    signature = gateway.build_callback_signature(payload=payload, callback_secret=CALLBACK_SECRET)
    assert signature is not None
    return signature


def test_specific_wechat_callback_route_is_not_shadowed(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    client = _build_payment_app(db)
    monkeypatch.setattr(config, "payment_callback_secret", CALLBACK_SECRET)

    captured_kwargs: dict[str, object] = {}

    def _fake_handle_callback(*args: object, **kwargs: object) -> dict[str, object]:
        captured_kwargs.update(kwargs)
        return {
            "ok": True,
            "credited": True,
            "duplicate": False,
            "payment_method_seen": kwargs["payment_method"],
        }

    monkeypatch.setattr("src.api.payment.routes.PaymentService.handle_callback", _fake_handle_callback)

    callback_payload = {"callback_key": "cb-wechat", "amount_usd": 1.0}
    response = client.post(
        "/api/payment/callback/wechat",
        json=callback_payload,
        headers={
            "x-payment-callback-token": CALLBACK_SECRET,
            "x-payment-callback-signature": _sign_payload(callback_payload),
        },
    )

    assert response.status_code == 200
    payload = response.json()
    assert payload["payment_method"] == "wechat"
    assert payload["payment_method_seen"] == "wechat"
    assert payload["request_path"] == "/api/payment/callback/wechat"
    assert captured_kwargs["callback_signature"] == _sign_payload(callback_payload)
    assert captured_kwargs["callback_secret"] == CALLBACK_SECRET
    assert "signature_valid" not in captured_kwargs
    db.commit.assert_called_once()


def test_generic_payment_callback_route_still_handles_custom_methods(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    client = _build_payment_app(db)
    monkeypatch.setattr(config, "payment_callback_secret", CALLBACK_SECRET)

    captured_kwargs: dict[str, object] = {}

    def _fake_handle_callback(*args: object, **kwargs: object) -> dict[str, object]:
        captured_kwargs.update(kwargs)
        return {
            "ok": True,
            "credited": False,
            "duplicate": False,
            "payment_method_seen": kwargs["payment_method"],
        }

    monkeypatch.setattr("src.api.payment.routes.PaymentService.handle_callback", _fake_handle_callback)

    callback_payload = {"callback_key": "cb-generic", "amount_usd": 1.0}
    response = client.post(
        "/api/payment/callback/mockpay",
        json=callback_payload,
        headers={
            "x-payment-callback-token": CALLBACK_SECRET,
            "x-payment-callback-signature": _sign_payload(callback_payload),
        },
    )

    assert response.status_code == 200
    payload = response.json()
    assert payload["payment_method"] == "mockpay"
    assert payload["payment_method_seen"] == "mockpay"
    assert captured_kwargs["callback_signature"] == _sign_payload(callback_payload)
    assert captured_kwargs["callback_secret"] == CALLBACK_SECRET
    assert "signature_valid" not in captured_kwargs


def test_callback_requires_shared_token(monkeypatch: pytest.MonkeyPatch) -> None:
    db = MagicMock()
    client = _build_payment_app(db)
    monkeypatch.setattr(config, "payment_callback_secret", CALLBACK_SECRET)

    response = client.post(
        "/api/payment/callback/alipay",
        json={"callback_key": "cb-missing-token", "amount_usd": 1.0},
    )

    assert response.status_code == 401
    db.commit.assert_not_called()


def test_callback_rejects_invalid_shared_token(monkeypatch: pytest.MonkeyPatch) -> None:
    db = MagicMock()
    client = _build_payment_app(db)
    monkeypatch.setattr(config, "payment_callback_secret", CALLBACK_SECRET)

    callback_payload = {"callback_key": "cb-invalid-token", "amount_usd": 1.0}

    response = client.post(
        "/api/payment/callback/alipay",
        json=callback_payload,
        headers={
            "x-payment-callback-token": "wrong-token",
            "x-payment-callback-signature": _sign_payload(callback_payload),
        },
    )

    assert response.status_code == 401
    db.commit.assert_not_called()


def test_callback_rejects_missing_signature(monkeypatch: pytest.MonkeyPatch) -> None:
    db = MagicMock()
    client = _build_payment_app(db)
    monkeypatch.setattr(config, "payment_callback_secret", CALLBACK_SECRET)

    response = client.post(
        "/api/payment/callback/alipay",
        json={"callback_key": "cb-missing-signature", "amount_usd": 1.0},
        headers={"x-payment-callback-token": CALLBACK_SECRET},
    )

    assert response.status_code == 401
    db.commit.assert_not_called()


def test_callback_disabled_when_secret_not_configured(monkeypatch: pytest.MonkeyPatch) -> None:
    db = MagicMock()
    client = _build_payment_app(db)
    monkeypatch.setattr(config, "payment_callback_secret", "")

    response = client.post(
        "/api/payment/callback/alipay",
        json={"callback_key": "cb-secret-missing", "amount_usd": 1.0},
    )

    assert response.status_code == 503
    db.commit.assert_not_called()


@pytest.mark.asyncio
async def test_admin_payment_credit_adapter_marks_manual_credit(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    order = PaymentOrder(
        id="po-credit",
        order_no="order-credit",
        wallet_id="w1",
        user_id="u1",
        amount_usd=Decimal("8.00000000"),
        refunded_amount_usd=Decimal("0"),
        refundable_amount_usd=Decimal("8.00000000"),
        payment_method="alipay",
        status="pending",
        gateway_response={"existing": True},
    )
    adapter = AdminPaymentOrderCreditAdapter(order_id=order.id)
    context = SimpleNamespace(
        db=db,
        raw_body=b"{}",
        ensure_json_body=lambda: {
            "pay_amount": 58.0,
            "pay_currency": "CNY",
            "exchange_rate": 7.25,
        },
        user=SimpleNamespace(id="admin-1"),
    )

    monkeypatch.setattr(
        "src.api.admin.payments.routes.PaymentService.get_order",
        lambda _db, order_id: order if order_id == "po-credit" else None,
    )

    captured: dict[str, object] = {}

    def _fake_credit_order(_db: MagicMock, **kwargs: object) -> tuple[PaymentOrder, bool]:
        captured.update(kwargs)
        return order, True

    monkeypatch.setattr(
        "src.api.admin.payments.routes.PaymentService.credit_order",
        _fake_credit_order,
    )

    result = await adapter.handle(context)

    assert result["credited"] is True
    assert result["order"]["id"] == "po-credit"
    gateway_response = captured["gateway_response"]
    assert isinstance(gateway_response, dict)
    assert gateway_response["existing"] is True
    assert gateway_response["manual_credit"] is True
    assert gateway_response["credited_by"] == "admin-1"
    db.commit.assert_called_once()
