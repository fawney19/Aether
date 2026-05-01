from __future__ import annotations

from unittest.mock import MagicMock

import pytest
from sqlalchemy.exc import IntegrityError

from src.utils.transaction_manager import retry_on_database_error


def test_retry_on_database_error_rolls_back_before_retry(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    attempts = {"count": 0}

    monkeypatch.setattr("src.utils.transaction_manager._find_db_session", lambda args, kwargs: db)
    monkeypatch.setattr("time.sleep", lambda *_args, **_kwargs: None)
    monkeypatch.setattr("random.uniform", lambda *_args, **_kwargs: 0.0)

    @retry_on_database_error(max_retries=2, delay=0)
    def operation(db_session: object) -> str:
        attempts["count"] += 1
        if attempts["count"] == 1:
            raise IntegrityError("INSERT", {}, Exception("boom"))
        return "ok"

    result = operation(db)

    assert result == "ok"
    assert attempts["count"] == 2
    assert db.rollback.call_count == 1


def test_retry_on_database_error_rolls_back_before_final_reraise(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    attempts = {"count": 0}

    monkeypatch.setattr("src.utils.transaction_manager._find_db_session", lambda args, kwargs: db)
    monkeypatch.setattr("time.sleep", lambda *_args, **_kwargs: None)
    monkeypatch.setattr("random.uniform", lambda *_args, **_kwargs: 0.0)

    @retry_on_database_error(max_retries=2, delay=0)
    def operation(db_session: object) -> str:
        attempts["count"] += 1
        raise IntegrityError("INSERT", {}, Exception("boom"))

    with pytest.raises(IntegrityError):
        operation(db)

    assert attempts["count"] == 2
    assert db.rollback.call_count == 2
