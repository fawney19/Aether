from __future__ import annotations

from types import SimpleNamespace
from unittest.mock import MagicMock

import pytest

from src.models.database import User, UserGroup
from src.services.user.service import UserService


def test_create_user_assigns_default_group_when_group_not_provided(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    default_group = SimpleNamespace(id="group-default")

    user_query = MagicMock()
    user_query.filter.return_value = user_query
    user_query.first.return_value = None

    db = MagicMock()
    db.in_transaction.return_value = True
    db.query.return_value = user_query

    initialize_wallet = MagicMock()

    monkeypatch.setattr("src.utils.transaction_manager._find_db_session", lambda args, kwargs: db)
    monkeypatch.setattr(
        "src.services.user.service.EmailValidator.validate",
        lambda *_a, **_k: (True, None),
    )
    monkeypatch.setattr(
        "src.services.user.service.UsernameValidator.validate",
        lambda *_a, **_k: (True, None),
    )
    monkeypatch.setattr(
        "src.services.user.service.PasswordValidator.validate",
        lambda *_a, **_k: (True, None),
    )
    monkeypatch.setattr(
        "src.services.user.service.UserGroupService.get_or_create_default_group",
        lambda _db: default_group,
    )
    monkeypatch.setattr(
        "src.services.wallet.WalletService.initialize_user_wallet",
        initialize_wallet,
    )

    user = UserService.create_user(
        db,
        email="user@example.com",
        username="user1",
        password="Abcd1234",
    )

    assert user.group_id == "group-default"
    initialize_wallet.assert_called_once()
    db.commit.assert_called_once()


def test_batch_update_user_group_binding_sets_group_on_bind(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    group = SimpleNamespace(id="group-1")
    user = SimpleNamespace(
        id="user-1",
        email="u1@example.com",
        group_id=None,
        group=None,
        updated_at=None,
    )

    group_query = MagicMock()
    group_query.filter.return_value = group_query
    group_query.first.return_value = group

    user_query = MagicMock()
    user_query.options.return_value = user_query
    user_query.filter.return_value = user_query
    user_query.all.return_value = [user]

    db = MagicMock()
    db.in_transaction.return_value = True
    db.query.side_effect = lambda model: group_query if model is UserGroup else user_query

    invalidate_user_cache = MagicMock(return_value="invalidate-task")
    create_task = MagicMock()

    monkeypatch.setattr("src.utils.transaction_manager._find_db_session", lambda args, kwargs: db)
    monkeypatch.setattr(
        "src.services.user.service.UserCacheService.invalidate_user_cache",
        invalidate_user_cache,
    )
    monkeypatch.setattr("src.services.user.service.safe_create_task", create_task)

    updated_users, skipped_count = UserService.batch_update_user_group_binding(
        db,
        user_ids=["user-1"],
        action="bind",
        group_id="group-1",
    )

    assert updated_users == [user]
    assert skipped_count == 0
    assert user.group_id == "group-1"
    assert user.group is group
    db.commit.assert_called_once()
    invalidate_user_cache.assert_called_once_with("user-1", "u1@example.com")
    create_task.assert_called_once()


def test_batch_update_user_group_binding_unbind_respects_source_group(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    default_group = SimpleNamespace(id="group-default")
    current_group = SimpleNamespace(id="group-1")
    other_group = SimpleNamespace(id="group-2")
    matched_user = SimpleNamespace(
        id="user-1",
        email="u1@example.com",
        group_id="group-1",
        group=current_group,
        updated_at=None,
    )
    other_user = SimpleNamespace(
        id="user-2",
        email="u2@example.com",
        group_id="group-2",
        group=other_group,
        updated_at=None,
    )

    user_query = MagicMock()
    user_query.options.return_value = user_query
    user_query.filter.return_value = user_query
    user_query.all.return_value = [matched_user, other_user]

    db = MagicMock()
    db.in_transaction.return_value = True
    default_group_query = MagicMock()
    default_group_query.filter.return_value = default_group_query
    default_group_query.order_by.return_value = default_group_query
    default_group_query.first.return_value = default_group

    db.query.side_effect = lambda model: user_query if model is User else default_group_query

    invalidate_user_cache = MagicMock(return_value="invalidate-task")
    create_task = MagicMock()

    monkeypatch.setattr("src.utils.transaction_manager._find_db_session", lambda args, kwargs: db)
    monkeypatch.setattr(
        "src.services.user.service.UserCacheService.invalidate_user_cache",
        invalidate_user_cache,
    )
    monkeypatch.setattr("src.services.user.service.safe_create_task", create_task)

    updated_users, skipped_count = UserService.batch_update_user_group_binding(
        db,
        user_ids=["user-1", "user-2"],
        action="unbind",
        source_group_id="group-1",
    )

    assert updated_users == [matched_user]
    assert skipped_count == 1
    assert matched_user.group_id == "group-default"
    assert matched_user.group is default_group
    assert other_user.group_id == "group-2"
    db.commit.assert_called_once()
    invalidate_user_cache.assert_called_once_with("user-1", "u1@example.com")
    create_task.assert_called_once()
