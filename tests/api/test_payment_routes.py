from __future__ import annotations

import json
from contextlib import contextmanager
from datetime import datetime, timezone
from decimal import Decimal
from types import SimpleNamespace
from typing import Any
from unittest.mock import MagicMock

import pytest
from fastapi import FastAPI
from fastapi.testclient import TestClient

from src.api.admin.payments.routes import (
    AdminPaymentOrderCreditAdapter,
    router as admin_payments_router,
)
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


def _build_admin_payments_app(
    db: MagicMock,
    monkeypatch: pytest.MonkeyPatch,
) -> TestClient:
    app = FastAPI()
    app.include_router(admin_payments_router)
    app.dependency_overrides[get_db] = lambda: db

    async def _fake_pipeline_run(
        *,
        adapter: Any,
        http_request: object,
        db: MagicMock,
        mode: object,
    ) -> Any:
        _ = mode
        request_body = await http_request.body()
        payload = json.loads(request_body) if request_body else {}
        context = SimpleNamespace(
            db=db,
            request=SimpleNamespace(state=SimpleNamespace()),
            user=SimpleNamespace(id="admin-1"),
            raw_body=request_body,
            ensure_json_body=lambda: payload,
            add_audit_metadata=lambda **_: None,
        )
        return await adapter.handle(context)

    monkeypatch.setattr("src.api.admin.payments.routes.pipeline.run", _fake_pipeline_run)

    @contextmanager
    def _fake_get_db_context() -> MagicMock:
        try:
            yield db
            db.commit()
        except Exception:
            db.rollback()
            raise

    monkeypatch.setattr("src.api.admin.payments.routes.get_db_context", _fake_get_db_context)
    return TestClient(app)


def _make_payment_order(
    *,
    order_id: str = "order-1",
    order_type: str = "topup",
    status: str = "pending",
    payment_method: str = "alipay",
) -> PaymentOrder:
    now = datetime.now(timezone.utc)
    return PaymentOrder(
        id=order_id,
        order_no=f"po-{order_id}",
        wallet_id="wallet-1",
        user_id="user-1",
        amount_usd=Decimal("8.00000000"),
        refunded_amount_usd=Decimal("0"),
        refundable_amount_usd=Decimal("8.00000000"),
        payment_method=payment_method,
        order_type=order_type,
        status=status,
        gateway_response={"existing": True},
        created_at=now,
        paid_at=None,
        credited_at=None,
        expires_at=None,
    )


def _make_payment_callback(*, callback_id: str = "cb-1") -> SimpleNamespace:
    now = datetime.now(timezone.utc)
    return SimpleNamespace(
        id=callback_id,
        payment_order_id="order-1",
        payment_method="alipay",
        callback_key=f"key-{callback_id}",
        order_no="po-order-1",
        gateway_order_id=f"gw-{callback_id}",
        payload_hash="hash-1",
        signature_valid=True,
        status="processed",
        payload={"foo": "bar"},
        error_message=None,
        created_at=now,
        processed_at=now,
    )


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


def test_admin_payment_orders_route_only_lists_recharge_orders(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    client = _build_admin_payments_app(db, monkeypatch)
    order = _make_payment_order(order_id="topup-1", order_type="topup")
    captured: dict[str, object] = {}

    def _fake_list_orders(_db: MagicMock, **kwargs: object) -> tuple[list[PaymentOrder], int, bool]:
        captured.update(kwargs)
        return [order], 1, False

    monkeypatch.setattr("src.api.admin.payments.routes.PaymentService.list_orders", _fake_list_orders)

    response = client.get(
        "/api/admin/payments/orders",
        params={"status": "pending", "payment_method": "alipay"},
    )

    assert response.status_code == 200
    assert captured["status"] == "pending"
    assert captured["payment_method"] == "alipay"
    assert captured["order_types"] == "topup"
    assert response.json()["items"][0]["order_type"] == "topup"


def test_admin_payment_callbacks_route_only_lists_recharge_callbacks(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    client = _build_admin_payments_app(db, monkeypatch)
    callback = _make_payment_callback()
    captured: dict[str, object] = {}

    def _fake_list_callbacks(_db: MagicMock, **kwargs: object) -> tuple[list[SimpleNamespace], int]:
        captured.update(kwargs)
        return [callback], 1

    monkeypatch.setattr(
        "src.api.admin.payments.routes.PaymentService.list_callbacks",
        _fake_list_callbacks,
    )

    response = client.get(
        "/api/admin/payments/callbacks",
        params={"payment_method": "alipay", "limit": 10, "offset": 5},
    )

    assert response.status_code == 200
    assert captured["payment_method"] == "alipay"
    assert captured["order_types"] == "topup"
    assert captured["limit"] == 10
    assert captured["offset"] == 5
    assert response.json()["items"][0]["callback_key"] == callback.callback_key


def test_admin_payment_order_detail_route_hides_subscription_orders(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    client = _build_admin_payments_app(db, monkeypatch)
    order = _make_payment_order(order_id="sub-1", order_type="subscription_renewal")

    monkeypatch.setattr(
        "src.api.admin.payments.routes.PaymentService.get_order",
        lambda _db, order_id: order if order_id == "sub-1" else None,
    )

    response = client.get("/api/admin/payments/orders/sub-1")

    assert response.status_code == 404
    assert response.json()["detail"] == "充值订单不存在"


def test_admin_payment_order_credit_route_hides_subscription_orders(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    client = _build_admin_payments_app(db, monkeypatch)
    order = _make_payment_order(order_id="sub-credit", order_type="subscription_initial")

    monkeypatch.setattr(
        "src.api.admin.payments.routes.PaymentService.get_order",
        lambda _db, order_id: order if order_id == "sub-credit" else None,
    )

    response = client.post("/api/admin/payments/orders/sub-credit/credit", json={})

    assert response.status_code == 404
    assert response.json()["detail"] == "充值订单不存在"


@pytest.mark.asyncio
async def test_admin_payment_credit_adapter_marks_manual_credit(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    order = _make_payment_order(order_id="po-credit", order_type="topup")
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

    @contextmanager
    def _fake_get_db_context() -> MagicMock:
        try:
            yield db
            db.commit()
        except Exception:
            db.rollback()
            raise

    monkeypatch.setattr("src.api.admin.payments.routes.get_db_context", _fake_get_db_context)

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


def test_admin_payment_approve_route_passes_operator_id_and_manual_markers(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    client = _build_admin_payments_app(db, monkeypatch)
    order = _make_payment_order(
        order_id="topup-approve",
        order_type="topup",
        status="pending_approval",
        payment_method="manual_review",
    )
    captured: dict[str, object] = {}

    monkeypatch.setattr(
        "src.api.admin.payments.routes.PaymentService.get_order",
        lambda _db, order_id: order if order_id == "topup-approve" else None,
    )

    def _fake_credit_order(_db: MagicMock, **kwargs: object) -> tuple[PaymentOrder, bool]:
        captured.update(kwargs)
        return order, True

    monkeypatch.setattr(
        "src.api.admin.payments.routes.PaymentService.credit_order",
        _fake_credit_order,
    )

    response = client.post("/api/admin/payments/orders/topup-approve/approve", json={})

    assert response.status_code == 200
    assert response.json()["order"]["id"] == "topup-approve"
    assert captured["operator_id"] == "admin-1"
    gateway_response = captured["gateway_response"]
    assert isinstance(gateway_response, dict)
    assert gateway_response["manual_credit"] is True
    assert gateway_response["credited_by"] == "admin-1"
    assert gateway_response["approved_by"] == "admin-1"


def test_admin_payment_reject_route_rejects_expired_order(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    client = _build_admin_payments_app(db, monkeypatch)
    order = _make_payment_order(
        order_id="topup-expired",
        order_type="topup",
        status="expired",
        payment_method="manual_review",
    )

    monkeypatch.setattr(
        "src.api.admin.payments.routes.PaymentService.get_order",
        lambda _db, order_id: order if order_id == "topup-expired" else None,
    )

    response = client.post("/api/admin/payments/orders/topup-expired/reject", json={})

    assert response.status_code == 400
    assert response.json()["detail"] == "当前订单不处于待审核状态"


def test_admin_payment_approve_route_rejects_expired_order(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    client = _build_admin_payments_app(db, monkeypatch)
    order = _make_payment_order(
        order_id="topup-expired-approve",
        order_type="topup",
        status="expired",
        payment_method="manual_review",
    )

    monkeypatch.setattr(
        "src.api.admin.payments.routes.PaymentService.get_order",
        lambda _db, order_id: order if order_id == "topup-expired-approve" else None,
    )

    response = client.post("/api/admin/payments/orders/topup-expired-approve/approve", json={})

    assert response.status_code == 400
    assert response.json()["detail"] == "当前订单不处于待审核状态"
