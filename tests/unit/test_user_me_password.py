from __future__ import annotations

from contextlib import contextmanager
from types import SimpleNamespace
from unittest.mock import MagicMock

import pytest

from src.api.user_me.routes import _change_password_sync
from src.core.exceptions import InvalidRequestException
from src.core.validators import PasswordPolicyLevel
from src.models.database import User


def test_verify_password_returns_false_for_password_over_72_bytes() -> None:
    user = User(email=None, email_verified=False, username="tester")
    user.set_password("abc12345")

    assert user.verify_password("a" * 80) is False


def test_change_password_rejects_same_as_current_password(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    user = SimpleNamespace(
        id="user-1",
        email="user@example.com",
        password_hash="hashed",
        auth_source=SimpleNamespace(value="local"),
        verify_password=MagicMock(side_effect=lambda password: password == "Abcd1234!"),
        set_password=MagicMock(),
        updated_at=None,
    )

    db = MagicMock()
    db.query.return_value.filter.return_value.first.return_value = user

    @contextmanager
    def _fake_get_db_context() -> MagicMock:
        yield db

    monkeypatch.setattr("src.api.user_me.routes.get_db_context", _fake_get_db_context)
    monkeypatch.setattr(
        "src.api.user_me.routes.SystemConfigService.get_password_policy_level",
        lambda _db: PasswordPolicyLevel.STRONG.value,
    )

    with pytest.raises(InvalidRequestException, match="新密码不能与当前密码相同"):
        _change_password_sync(
            "user-1",
            SimpleNamespace(old_password="Abcd1234!", new_password="Abcd1234!"),
        )

    user.set_password.assert_not_called()
