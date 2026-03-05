from __future__ import annotations

from datetime import datetime, timedelta, timezone
from decimal import Decimal
from types import SimpleNamespace
from unittest.mock import MagicMock

import pytest

from src.models.database import PaymentOrder, Wallet
from src.services.payment.gateway import get_payment_gateway
from src.services.payment import PaymentService

CALLBACK_SECRET = "test-callback-secret"


def _sign_payload(payload: dict[str, object]) -> str:
    gateway = get_payment_gateway("alipay")
    signature = gateway.build_callback_signature(payload=payload, callback_secret=CALLBACK_SECRET)
    assert signature is not None
    return signature


def test_create_recharge_order_creates_pending_order(monkeypatch: pytest.MonkeyPatch) -> None:
    db = MagicMock()
    user = SimpleNamespace(id="u1")
    wallet = SimpleNamespace(id="w1", status="active")

    monkeypatch.setattr(
        "src.services.payment.service.WalletService.get_or_create_wallet",
        lambda _db, user: wallet,
    )

    order = PaymentService.create_recharge_order(
        db,
        user=user,
        amount_usd="12.5",
        payment_method="alipay",
        pay_amount="88.00",
        pay_currency="CNY",
        exchange_rate="7.04",
    )

    db.add.assert_called_once()
    db.flush.assert_called_once()
    assert order.wallet_id == "w1"
    assert order.user_id == "u1"
    assert order.status == "pending"
    assert order.payment_method == "alipay"
    assert order.gateway_order_id == f"ali_{order.order_no}"
    assert isinstance(order.gateway_response, dict)
    assert order.gateway_response["gateway"] == "alipay"
    assert Decimal(order.amount_usd) == Decimal("12.50000000")
    assert Decimal(order.refundable_amount_usd) == Decimal("12.50000000")


def test_refresh_order_status_marks_expired_pending_order() -> None:
    order = PaymentOrder(
        id="po-expired",
        order_no="order-expired",
        wallet_id="w1",
        user_id="u1",
        amount_usd=Decimal("2.00000000"),
        refunded_amount_usd=Decimal("0"),
        refundable_amount_usd=Decimal("2.00000000"),
        payment_method="wechat",
        status="pending",
    )
    order.expires_at = datetime(2000, 1, 1, tzinfo=timezone.utc)

    changed = PaymentService.refresh_order_status(order)

    assert changed is True
    assert order.status == "expired"


def test_handle_callback_is_idempotent_for_processed_callback(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    callback = SimpleNamespace(status="processed", payment_order_id="po1")

    monkeypatch.setattr(
        "src.services.payment.service.PaymentService.log_callback",
        lambda *args, **kwargs: (callback, False),
    )

    result = PaymentService.handle_callback(
        db,
        payment_method="alipay",
        callback_key="cb1",
        payload={"foo": "bar"},
        callback_signature=_sign_payload({"foo": "bar"}),
        callback_secret=CALLBACK_SECRET,
        order_no="order-1",
    )

    assert result["ok"] is True
    assert result["duplicate"] is True
    assert result["credited"] is False
    assert result["order_id"] == "po1"


def test_handle_callback_fails_on_amount_mismatch(monkeypatch: pytest.MonkeyPatch) -> None:
    db = MagicMock()
    callback = SimpleNamespace(
        status="received",
        payment_order_id=None,
        order_no=None,
        gateway_order_id=None,
        error_message=None,
        processed_at=None,
    )
    order = SimpleNamespace(
        id="po1",
        order_no="order-1",
        gateway_order_id=None,
        amount_usd=Decimal("10.00000000"),
    )

    monkeypatch.setattr(
        "src.services.payment.service.PaymentService.log_callback",
        lambda *args, **kwargs: (callback, True),
    )
    monkeypatch.setattr(
        "src.services.payment.service.PaymentService.get_order",
        lambda *args, **kwargs: order,
    )

    result = PaymentService.handle_callback(
        db,
        payment_method="wechat",
        callback_key="cb2",
        payload={"foo": "bar"},
        callback_signature=_sign_payload({"foo": "bar"}),
        callback_secret=CALLBACK_SECRET,
        order_no="order-1",
        amount_usd="9.99",
    )

    assert result["ok"] is False
    assert "mismatch" in result["error"]
    assert callback.status == "failed"


def test_handle_callback_rejects_invalid_signature() -> None:
    db = MagicMock()

    result = PaymentService.handle_callback(
        db,
        payment_method="alipay",
        callback_key="cb-invalid-signature",
        payload={"foo": "bar"},
        callback_signature="invalid-signature",
        callback_secret=CALLBACK_SECRET,
        order_no="order-1",
        amount_usd="9.99",
    )

    assert result["ok"] is False
    assert "signature" in result["error"]


def test_credit_order_applies_wallet_recharge_once(monkeypatch: pytest.MonkeyPatch) -> None:
    order = PaymentOrder(
        id="po1",
        order_no="order-1",
        wallet_id="w1",
        user_id="u1",
        amount_usd=Decimal("5.00000000"),
        refunded_amount_usd=Decimal("0"),
        refundable_amount_usd=Decimal("5.00000000"),
        payment_method="alipay",
        status="pending",
    )
    wallet = Wallet(
        id="w1",
        user_id="u1",
        balance=Decimal("1.00000000"),
        status="active",
        total_recharged=Decimal("1.00000000"),
        total_consumed=Decimal("0"),
        total_refunded=Decimal("0"),
        total_adjusted=Decimal("0"),
        version=0,
        currency="USD",
    )

    class DummyOrderQuery:
        def filter(self, *args: object, **kwargs: object) -> "DummyOrderQuery":
            return self

        def with_for_update(self) -> "DummyOrderQuery":
            return self

        def one_or_none(self) -> PaymentOrder:
            return order

    class DummyWalletQuery:
        def filter(self, *args: object, **kwargs: object) -> "DummyWalletQuery":
            return self

        def first(self) -> Wallet:
            return wallet

    db = MagicMock()
    db.query.side_effect = [DummyOrderQuery(), DummyWalletQuery()]
    create_tx = MagicMock()
    monkeypatch.setattr(
        "src.services.payment.service.WalletService.create_wallet_transaction",
        create_tx,
    )

    updated, credited = PaymentService.credit_order(
        db,
        order=order,
        gateway_order_id="gw-1",
        pay_amount="36.00",
        pay_currency="CNY",
        exchange_rate="7.20",
    )

    assert credited is True
    assert updated.status == "credited"
    assert updated.gateway_order_id == "gw-1"
    assert updated.paid_at is not None
    assert updated.credited_at is not None
    create_tx.assert_called_once()


def test_expire_order_marks_pending_order_expired() -> None:
    order = PaymentOrder(
        id="po2",
        order_no="order-2",
        wallet_id="w1",
        user_id="u1",
        amount_usd=Decimal("3.00000000"),
        refunded_amount_usd=Decimal("0"),
        refundable_amount_usd=Decimal("3.00000000"),
        payment_method="wechat",
        status="pending",
    )
    order.expires_at = datetime.now(timezone.utc) + timedelta(minutes=30)

    class DummyOrderQuery:
        def filter(self, *args: object, **kwargs: object) -> "DummyOrderQuery":
            return self

        def with_for_update(self) -> "DummyOrderQuery":
            return self

        def one_or_none(self) -> PaymentOrder:
            return order

    db = MagicMock()
    db.query.return_value = DummyOrderQuery()

    updated, changed = PaymentService.expire_order(db, order=order, reason="ops_close")

    assert changed is True
    assert updated.status == "expired"
    assert updated.gateway_response["expire_reason"] == "ops_close"
    assert "expired_at" in updated.gateway_response


def test_expire_order_is_idempotent_for_existing_expired_order() -> None:
    order = PaymentOrder(
        id="po3",
        order_no="order-3",
        wallet_id="w1",
        user_id="u1",
        amount_usd=Decimal("1.00000000"),
        refunded_amount_usd=Decimal("0"),
        refundable_amount_usd=Decimal("1.00000000"),
        payment_method="alipay",
        status="expired",
    )

    class DummyOrderQuery:
        def filter(self, *args: object, **kwargs: object) -> "DummyOrderQuery":
            return self

        def with_for_update(self) -> "DummyOrderQuery":
            return self

        def one_or_none(self) -> PaymentOrder:
            return order

    db = MagicMock()
    db.query.return_value = DummyOrderQuery()

    updated, changed = PaymentService.expire_order(db, order=order)

    assert changed is False
    assert updated.status == "expired"
