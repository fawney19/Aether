from __future__ import annotations

from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock, call

import pytest

from src.models.database import (
    ApiKey,
    PaymentOrder,
    RefundRequest,
    RequestCandidate,
    Usage,
    User,
    Wallet,
)
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
        UserService.delete_user(db, "user-1")

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
        UserService.delete_user(db, "user-2")

    db.delete.assert_not_called()


def test_delete_user_precleans_large_tables_before_final_delete(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    user = SimpleNamespace(id="user-3", email="u3@example.com")

    user_query = MagicMock()
    user_query.filter.return_value = user_query
    user_query.first.return_value = user

    wallet_ids_query = MagicMock()
    wallet_ids_query.outerjoin.return_value = wallet_ids_query
    wallet_ids_query.filter.return_value = wallet_ids_query
    wallet_ids_query.all.return_value = []

    api_key_ids_query = MagicMock()
    api_key_ids_query.filter.return_value = api_key_ids_query
    api_key_ids_query.all.return_value = [("key-1",), ("key-2",)]

    delete_query = MagicMock()
    delete_query.filter.return_value = delete_query
    delete_query.delete.return_value = 2

    db = MagicMock()

    def _query(model: object) -> MagicMock:
        if model is User:
            return user_query
        if model is Wallet.id:
            return wallet_ids_query
        if model is ApiKey.id:
            return api_key_ids_query
        return delete_query

    db.query.side_effect = _query

    pre_clean_api_key = MagicMock()
    batch_nullify_fk = MagicMock()
    invalidate_user_cache = AsyncMock()
    create_task = MagicMock()

    monkeypatch.setattr("src.services.user.service.pre_clean_api_key", pre_clean_api_key)
    monkeypatch.setattr("src.services.user.service.batch_nullify_fk", batch_nullify_fk)
    monkeypatch.setattr(
        "src.services.user.service.UserCacheService.invalidate_user_cache",
        invalidate_user_cache,
    )
    monkeypatch.setattr("src.services.user.service.safe_create_task", create_task)

    assert UserService.delete_user(db, "user-3") is True

    assert pre_clean_api_key.call_args_list == [call(db, "key-1"), call(db, "key-2")]
    assert batch_nullify_fk.call_args_list == [
        call(db, Usage, "user_id", "user-3"),
        call(db, RequestCandidate, "user_id", "user-3"),
    ]
    db.delete.assert_called_once_with(user)
    db.commit.assert_called_once()
    db.rollback.assert_not_called()
    invalidate_user_cache.assert_called_once_with("user-3", "u3@example.com")
    create_task.assert_called_once()
