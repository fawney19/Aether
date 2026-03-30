from __future__ import annotations

from datetime import datetime, timedelta, timezone
from decimal import Decimal
from types import SimpleNamespace
from unittest.mock import MagicMock

import pytest

from src.config import config
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
    monkeypatch.setattr(
        "src.services.payment.gateway.alipay.AlipayGateway.create_checkout_payload",
        lambda _self, order: {
            "gateway": "alipay",
            "display_name": "支付宝",
            "gateway_order_id": None,
            "payment_url": f"https://example.com/pay/{order.order_no}",
            "expires_at": order.expires_at.isoformat() if order.expires_at else None,
        },
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
    assert order.gateway_order_id is None
    assert isinstance(order.gateway_response, dict)
    assert order.gateway_response["gateway"] == "alipay"
    assert order.next_status_check_at is not None
    assert order.last_status_check_result == "scheduled"
    assert Decimal(order.amount_usd) == Decimal("12.50000000")
    assert Decimal(order.refundable_amount_usd) == Decimal("0")


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
        refundable_amount_usd=Decimal("0"),
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
    assert Decimal(updated.refundable_amount_usd) == Decimal("5.00000000")
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


def test_list_orders_expires_overdue_pending_before_filtered_count() -> None:
    expiry_query = MagicMock()
    expiry_query.filter.return_value = expiry_query
    expiry_query.update.return_value = 2

    list_query = MagicMock()
    list_query.filter.return_value = list_query
    list_query.count.return_value = 1
    ordered_query = MagicMock()
    list_query.order_by.return_value = ordered_query
    ordered_query.offset.return_value = ordered_query
    ordered_query.limit.return_value = ordered_query
    ordered_query.all.return_value = ["order-1"]

    db = MagicMock()
    db.query.side_effect = [expiry_query, list_query]

    items, total, changed = PaymentService.list_orders(
        db,
        status="pending",
        payment_method="alipay",
        limit=5,
        offset=10,
    )

    assert items == ["order-1"]
    assert total == 1
    assert changed is True
    expiry_query.update.assert_called_once()
    ordered_query.offset.assert_called_once_with(10)
    ordered_query.limit.assert_called_once_with(5)


def test_calculate_next_status_check_at_returns_none_after_window(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(config, "payment_status_check_max_age_hours", 24)
    now = datetime.now(timezone.utc)
    order = PaymentOrder(
        id="po-window",
        order_no="order-window",
        wallet_id="w1",
        user_id="u1",
        amount_usd=Decimal("2.00000000"),
        refunded_amount_usd=Decimal("0"),
        refundable_amount_usd=Decimal("0"),
        payment_method="alipay",
        status="pending",
    )
    order.created_at = now - timedelta(hours=25)
    order.expires_at = now - timedelta(hours=24, minutes=20)

    assert PaymentService.calculate_next_status_check_at(order=order, now=now, attempts=0) is None


def test_sync_alipay_order_status_wait_buyer_pay_schedules_next_check(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(config, "payment_status_check_max_age_hours", 24)
    now = datetime.now(timezone.utc)
    order = PaymentOrder(
        id="po-wait",
        order_no="order-wait",
        wallet_id="w1",
        user_id="u1",
        amount_usd=Decimal("1.00000000"),
        refunded_amount_usd=Decimal("0"),
        refundable_amount_usd=Decimal("0"),
        payment_method="alipay",
        status="pending",
    )
    order.created_at = now
    order.expires_at = now + timedelta(minutes=30)
    order.status_check_attempts = 0

    monkeypatch.setattr(
        PaymentService,
        "_get_locked_order",
        lambda _db, order_id: order if order_id == order.id else None,
    )

    result = PaymentService.sync_alipay_order_status(
        MagicMock(),
        order=order,
        query_response={"code": "10000", "trade_status": "WAIT_BUYER_PAY"},
        checked_at=now,
    )

    assert result["ok"] is True
    assert result["trade_status"] == "WAIT_BUYER_PAY"
    assert order.status == "pending"
    assert order.status_check_attempts == 1
    assert order.last_status_check_result == "WAIT_BUYER_PAY"
    assert order.next_status_check_at == now + timedelta(minutes=2)


def test_sync_alipay_order_status_credits_successful_trade(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    now = datetime.now(timezone.utc)
    order = PaymentOrder(
        id="po-success",
        order_no="order-success",
        wallet_id="w1",
        user_id="u1",
        amount_usd=Decimal("1.00000000"),
        refunded_amount_usd=Decimal("0"),
        refundable_amount_usd=Decimal("0"),
        payment_method="alipay",
        status="pending",
        gateway_response={"payment_url": "https://example.com/pay"},
    )
    order.created_at = now
    order.expires_at = now + timedelta(minutes=30)

    monkeypatch.setattr(
        PaymentService,
        "_get_locked_order",
        lambda _db, order_id: order if order_id == order.id else None,
    )

    def _fake_credit_order(_db: MagicMock, **kwargs: object) -> tuple[PaymentOrder, bool]:
        order.status = "credited"
        order.gateway_order_id = kwargs["gateway_order_id"]
        order.gateway_response = kwargs["gateway_response"]
        return order, True

    monkeypatch.setattr(PaymentService, "credit_order", _fake_credit_order)

    result = PaymentService.sync_alipay_order_status(
        MagicMock(),
        order=order,
        query_response={
            "code": "10000",
            "trade_status": "TRADE_SUCCESS",
            "trade_no": "2026033100001",
            "total_amount": "1.00",
            "pay_currency": "CNY",
        },
        checked_at=now,
    )

    assert result["ok"] is True
    assert result["credited"] is True
    assert order.status == "credited"
    assert order.gateway_order_id == "2026033100001"
    assert order.last_status_check_result == "TRADE_SUCCESS"
    assert order.next_status_check_at is None
    assert order.gateway_response["payment_url"] == "https://example.com/pay"
    assert order.gateway_response["gateway_trade_status"] == "TRADE_SUCCESS"
