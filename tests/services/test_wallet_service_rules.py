from decimal import Decimal
from types import SimpleNamespace
from unittest.mock import MagicMock, patch

import pytest
from sqlalchemy.exc import IntegrityError

from src.services.wallet.service import WalletService


def _build_wallet(
    *,
    wallet_id: str = "wallet-1",
    recharge: str = "0",
    gift: str = "0",
    limit_mode: str = "finite",
    status: str = "active",
    api_key_id: str | None = None,
) -> SimpleNamespace:
    return SimpleNamespace(
        id=wallet_id,
        balance=Decimal(recharge),
        gift_balance=Decimal(gift),
        total_recharged=Decimal("0"),
        total_consumed=Decimal("0"),
        total_refunded=Decimal("0"),
        total_adjusted=Decimal("0"),
        limit_mode=limit_mode,
        currency="USD",
        status=status,
        api_key_id=api_key_id,
        version=0,
        updated_at=None,
    )


def _build_locked_db(wallet: SimpleNamespace) -> MagicMock:
    db = MagicMock()
    query = MagicMock()
    query.filter.return_value = query
    query.with_for_update.return_value = query
    query.one_or_none.return_value = wallet
    db.query.return_value = query
    return db


def test_get_or_create_wallet_prefers_user_owner_for_non_standalone_key() -> None:
    db = MagicMock()
    db.query.return_value.filter.return_value.first.return_value = None
    db.begin_nested.return_value.__enter__.return_value = None
    db.begin_nested.return_value.__exit__.return_value = None

    user = SimpleNamespace(id="user-1")
    api_key = SimpleNamespace(id="key-1", is_standalone=False)

    wallet = WalletService.get_or_create_wallet(db, user=user, api_key=api_key)

    assert wallet is not None
    assert wallet.user_id == "user-1"
    assert wallet.api_key_id is None


def test_get_or_create_wallet_uses_api_key_owner_for_standalone_key() -> None:
    db = MagicMock()
    db.query.return_value.filter.return_value.first.return_value = None
    db.begin_nested.return_value.__enter__.return_value = None
    db.begin_nested.return_value.__exit__.return_value = None

    user = SimpleNamespace(id="user-1")
    api_key = SimpleNamespace(id="key-1", is_standalone=True)

    wallet = WalletService.get_or_create_wallet(db, user=user, api_key=api_key)

    assert wallet is not None
    assert wallet.user_id is None
    assert wallet.api_key_id == "key-1"


def test_check_request_allowed_denies_when_recharge_negative_even_total_positive() -> None:
    wallet = _build_wallet(recharge="-1", gift="10", limit_mode="finite")
    db = MagicMock()
    api_key = MagicMock()

    with patch.object(WalletService, "get_or_create_wallet", return_value=wallet):
        result = WalletService.check_request_allowed(db, user=None, api_key=api_key)

    assert result.allowed is False
    assert result.message == "钱包欠费，请先充值"
    assert result.remaining == Decimal("-1.00000000")


def test_get_balance_snapshot_returns_negative_recharge_for_unlimited_wallet() -> None:
    wallet = _build_wallet(recharge="-2", gift="100", limit_mode="unlimited")
    db = MagicMock()

    with patch.object(WalletService, "get_or_create_wallet", return_value=wallet):
        snapshot = WalletService.get_balance_snapshot(db, user=None, api_key=MagicMock())

    assert snapshot == Decimal("-2.00000000")


def test_admin_adjust_balance_negative_from_gift_spills_to_recharge() -> None:
    wallet = _build_wallet(recharge="2", gift="3")
    db = _build_locked_db(wallet)

    tx = WalletService.admin_adjust_balance(
        db,
        wallet=wallet,
        amount_usd=Decimal("-10"),
        balance_type="gift",
        operator_id="admin-1",
        description="test",
    )

    assert wallet.gift_balance == Decimal("0E-8")
    assert wallet.balance == Decimal("-5.00000000")
    assert wallet.total_adjusted == Decimal("-10.00000000")
    assert tx.recharge_balance_after == Decimal("-5.00000000")
    assert tx.gift_balance_after == Decimal("0E-8")
    assert tx.amount == Decimal("-10.00000000")
    assert tx.category == "adjust"
    assert tx.reason_code == "adjust_admin"


def test_admin_adjust_balance_negative_from_recharge_then_gift() -> None:
    wallet = _build_wallet(recharge="2", gift="3")
    db = _build_locked_db(wallet)

    tx = WalletService.admin_adjust_balance(
        db,
        wallet=wallet,
        amount_usd=Decimal("-4"),
        balance_type="recharge",
        operator_id="admin-1",
        description="test",
    )

    assert wallet.balance == Decimal("0E-8")
    assert wallet.gift_balance == Decimal("1.00000000")
    assert wallet.total_adjusted == Decimal("-4.00000000")
    assert tx.recharge_balance_after == Decimal("0E-8")
    assert tx.gift_balance_after == Decimal("1.00000000")
    assert tx.amount == Decimal("-4.00000000")


def test_admin_adjust_balance_positive_adds_to_selected_bucket_without_offset() -> None:
    wallet = _build_wallet(recharge="-2", gift="5")
    db = _build_locked_db(wallet)

    tx = WalletService.admin_adjust_balance(
        db,
        wallet=wallet,
        amount_usd=Decimal("1"),
        balance_type="gift",
        operator_id="admin-1",
        description="test",
    )

    assert wallet.balance == Decimal("-2.00000000")
    assert wallet.gift_balance == Decimal("6.00000000")
    assert wallet.total_adjusted == Decimal("1.00000000")
    assert tx.recharge_balance_after == Decimal("-2.00000000")
    assert tx.gift_balance_after == Decimal("6.00000000")
    assert tx.amount == Decimal("1.00000000")
    assert tx.category == "adjust"
    assert tx.reason_code == "adjust_admin"


def test_apply_usage_charge_prefers_gift_then_recharge() -> None:
    wallet = _build_wallet(recharge="5", gift="3", limit_mode="finite")
    usage = SimpleNamespace(
        wallet_id=None,
        api_key_id=None,
        user_id="user-1",
        wallet_balance_before=None,
        wallet_balance_after=None,
        wallet_recharge_balance_before=None,
        wallet_recharge_balance_after=None,
        wallet_gift_balance_before=None,
        wallet_gift_balance_after=None,
    )
    db = _build_locked_db(wallet)

    with patch.object(WalletService, "_resolve_wallet_for_usage", return_value=wallet):
        before, after = WalletService.apply_usage_charge(db, usage=usage, amount_usd=Decimal("6"))

    assert before == Decimal("8.00000000")
    assert after == Decimal("2.00000000")
    assert wallet.gift_balance == Decimal("0E-8")
    assert wallet.balance == Decimal("2.00000000")
    assert wallet.total_consumed == Decimal("6.00000000")
    assert usage.wallet_balance_before == Decimal("8.00000000")
    assert usage.wallet_balance_after == Decimal("2.00000000")
    assert usage.wallet_recharge_balance_before == Decimal("5.00000000")
    assert usage.wallet_recharge_balance_after == Decimal("2.00000000")
    assert usage.wallet_gift_balance_before == Decimal("3.00000000")
    assert usage.wallet_gift_balance_after == Decimal("0E-8")


def test_apply_usage_charge_unlimited_wallet_keeps_balances() -> None:
    wallet = _build_wallet(recharge="5", gift="3", limit_mode="unlimited")
    usage = SimpleNamespace(
        wallet_id=None,
        api_key_id=None,
        user_id="user-1",
        wallet_balance_before=None,
        wallet_balance_after=None,
        wallet_recharge_balance_before=None,
        wallet_recharge_balance_after=None,
        wallet_gift_balance_before=None,
        wallet_gift_balance_after=None,
    )
    db = _build_locked_db(wallet)

    with patch.object(WalletService, "_resolve_wallet_for_usage", return_value=wallet):
        before, after = WalletService.apply_usage_charge(db, usage=usage, amount_usd=Decimal("4"))

    assert before == Decimal("8.00000000")
    assert after == Decimal("8.00000000")
    assert wallet.balance == Decimal("5")
    assert wallet.gift_balance == Decimal("3")
    assert wallet.total_consumed == Decimal("4.00000000")
    assert usage.wallet_balance_before == Decimal("8.00000000")
    assert usage.wallet_balance_after == Decimal("8.00000000")
    assert usage.wallet_recharge_balance_before == Decimal("5")
    assert usage.wallet_recharge_balance_after == Decimal("5")
    assert usage.wallet_gift_balance_before == Decimal("3")
    assert usage.wallet_gift_balance_after == Decimal("3")


def test_complete_refund_requires_processing_status() -> None:
    refund = SimpleNamespace(id="refund-1", status="failed")
    db = MagicMock()
    query = MagicMock()
    query.filter.return_value = query
    query.with_for_update.return_value = query
    query.one_or_none.return_value = refund
    db.query.return_value = query

    with pytest.raises(ValueError, match="processing"):
        WalletService.complete_refund(db, refund=refund)


def test_get_or_create_wallet_reuses_existing_after_integrity_error() -> None:
    existing_wallet = _build_wallet(wallet_id="wallet-existing", recharge="1", gift="0")
    db = MagicMock()
    nested = MagicMock()
    nested.__enter__.return_value = None
    nested.__exit__.return_value = None
    db.begin_nested.return_value = nested
    db.flush.side_effect = IntegrityError("insert", {}, Exception("duplicate key"))

    with patch.object(WalletService, "get_wallet", side_effect=[None, existing_wallet]):
        wallet = WalletService.get_or_create_wallet(
            db,
            user=SimpleNamespace(id="user-1"),
            api_key=None,
        )

    assert wallet is existing_wallet


def test_move_refund_to_processing_rejects_double_transition() -> None:
    refund = SimpleNamespace(
        id="refund-1",
        wallet_id="wallet-1",
        payment_order_id="order-1",
        amount_usd=Decimal("2"),
        status="pending_approval",
        approved_by=None,
        processed_by=None,
        processed_at=None,
        updated_at=None,
    )
    wallet = _build_wallet(wallet_id="wallet-1", recharge="10", gift="0")
    payment_order = SimpleNamespace(
        id="order-1",
        refunded_amount_usd=Decimal("0"),
        refundable_amount_usd=Decimal("10"),
    )
    refund_query = MagicMock()
    refund_query.filter.return_value = refund_query
    refund_query.with_for_update.return_value = refund_query
    refund_query.one_or_none.return_value = refund
    wallet_query = MagicMock()
    wallet_query.filter.return_value = wallet_query
    wallet_query.first.return_value = wallet
    order_query = MagicMock()
    order_query.filter.return_value = order_query
    order_query.with_for_update.return_value = order_query
    order_query.one_or_none.return_value = payment_order

    db = MagicMock()

    def _query(model: object) -> MagicMock:
        name = getattr(model, "__name__", "")
        if name == "RefundRequest":
            return refund_query
        if name == "Wallet":
            return wallet_query
        if name == "PaymentOrder":
            return order_query
        raise AssertionError(f"unexpected model query: {name}")

    db.query.side_effect = _query
    tx = SimpleNamespace(id="tx-1")

    with patch.object(WalletService, "create_wallet_transaction", return_value=tx) as create_tx:
        first_tx = WalletService.move_refund_to_processing(db, refund=refund, operator_id="admin-1")
        with pytest.raises(ValueError, match="not approvable"):
            WalletService.move_refund_to_processing(db, refund=refund, operator_id="admin-1")

    assert first_tx is tx
    assert create_tx.call_count == 1
    assert refund.status == "processing"
    assert payment_order.refunded_amount_usd == Decimal("2.00000000")
    assert payment_order.refundable_amount_usd == Decimal("8.00000000")


def test_fail_refund_rejects_invalid_status_after_first_failure() -> None:
    refund = SimpleNamespace(
        id="refund-2",
        wallet_id="wallet-1",
        payment_order_id=None,
        amount_usd=Decimal("1"),
        status="processing",
        failure_reason=None,
        updated_at=None,
    )
    wallet = _build_wallet(wallet_id="wallet-1", recharge="10", gift="0")
    refund_query = MagicMock()
    refund_query.filter.return_value = refund_query
    refund_query.with_for_update.return_value = refund_query
    refund_query.one_or_none.return_value = refund
    wallet_query = MagicMock()
    wallet_query.filter.return_value = wallet_query
    wallet_query.first.return_value = wallet

    db = MagicMock()

    def _query(model: object) -> MagicMock:
        name = getattr(model, "__name__", "")
        if name == "RefundRequest":
            return refund_query
        if name == "Wallet":
            return wallet_query
        raise AssertionError(f"unexpected model query: {name}")

    db.query.side_effect = _query
    tx = SimpleNamespace(id="tx-revert")

    with patch.object(WalletService, "create_wallet_transaction", return_value=tx) as create_tx:
        first_tx = WalletService.fail_refund(
            db,
            refund=refund,
            reason="first-failure",
            operator_id="admin-1",
        )
        with pytest.raises(ValueError, match="cannot fail refund in status: failed"):
            WalletService.fail_refund(
                db,
                refund=refund,
                reason="retry-failure",
                operator_id="admin-1",
            )

    assert first_tx is tx
    assert create_tx.call_count == 1
    assert refund.status == "failed"
    assert refund.failure_reason == "first-failure"


def test_fail_refund_rejects_succeeded_status() -> None:
    refund = SimpleNamespace(
        id="refund-succeeded",
        wallet_id="wallet-1",
        payment_order_id=None,
        amount_usd=Decimal("1"),
        status="succeeded",
        failure_reason=None,
        updated_at=None,
    )
    refund_query = MagicMock()
    refund_query.filter.return_value = refund_query
    refund_query.with_for_update.return_value = refund_query
    refund_query.one_or_none.return_value = refund

    db = MagicMock()

    def _query(model: object) -> MagicMock:
        name = getattr(model, "__name__", "")
        if name == "RefundRequest":
            return refund_query
        raise AssertionError(f"unexpected model query: {name}")

    db.query.side_effect = _query

    with pytest.raises(ValueError, match="cannot fail refund in status: succeeded"):
        WalletService.fail_refund(
            db,
            refund=refund,
            reason="should-not-override",
            operator_id="admin-1",
        )

    assert refund.status == "succeeded"
    assert refund.failure_reason is None
