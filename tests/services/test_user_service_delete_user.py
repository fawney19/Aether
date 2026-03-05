from __future__ import annotations

from types import SimpleNamespace
from unittest.mock import MagicMock

import pytest

from src.models.database import PaymentOrder, RefundRequest, User, Wallet
from src.services.user.service import UserService


def test_delete_user_blocks_when_unfinished_refund_exists() -> None:
    user = SimpleNamespace(id="user-1", email="u1@example.com")

    user_query = MagicMock()
    user_query.filter.return_value = user_query
    user_query.first.return_value = user

    wallet_ids_query = MagicMock()
    wallet_ids_query.outerjoin.return_value = wallet_ids_query
    wallet_ids_query.filter.return_value = wallet_ids_query
    wallet_ids_query.all.return_value = [("wallet-1",)]

    refund_query = MagicMock()
    refund_query.filter.return_value = refund_query
    refund_query.count.return_value = 1

    db = MagicMock()
    db.in_transaction.return_value = True

    def _query(model: object) -> MagicMock:
        if model is User:
            return user_query
        if model is Wallet.id:
            return wallet_ids_query
        if model is RefundRequest:
            return refund_query
        raise AssertionError(f"unexpected query target: {model}")

    db.query.side_effect = _query

    with pytest.raises(ValueError, match="未完结退款"):
        UserService.delete_user.__wrapped__(db, "user-1")

    db.delete.assert_not_called()


def test_delete_user_blocks_when_unfinished_payment_order_exists() -> None:
    user = SimpleNamespace(id="user-2", email="u2@example.com")

    user_query = MagicMock()
    user_query.filter.return_value = user_query
    user_query.first.return_value = user

    wallet_ids_query = MagicMock()
    wallet_ids_query.outerjoin.return_value = wallet_ids_query
    wallet_ids_query.filter.return_value = wallet_ids_query
    wallet_ids_query.all.return_value = [("wallet-2",)]

    refund_query = MagicMock()
    refund_query.filter.return_value = refund_query
    refund_query.count.return_value = 0

    order_query = MagicMock()
    order_query.filter.return_value = order_query
    order_query.count.return_value = 2

    db = MagicMock()
    db.in_transaction.return_value = True

    def _query(model: object) -> MagicMock:
        if model is User:
            return user_query
        if model is Wallet.id:
            return wallet_ids_query
        if model is RefundRequest:
            return refund_query
        if model is PaymentOrder:
            return order_query
        raise AssertionError(f"unexpected query target: {model}")

    db.query.side_effect = _query

    with pytest.raises(ValueError, match="未完结充值订单"):
        UserService.delete_user.__wrapped__(db, "user-2")

    db.delete.assert_not_called()
