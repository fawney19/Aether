from __future__ import annotations

from datetime import datetime, timezone
from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock, patch

import pytest
from fastapi import HTTPException

from src.api.auth.routes import AuthLoginAdapter
from src.core.enums import UserRole
from src.services.auth.service import AuthenticatedUserSnapshot


def _build_login_context(db: MagicMock) -> SimpleNamespace:
    return SimpleNamespace(
        db=db,
        request=SimpleNamespace(state=SimpleNamespace(), headers={}),
        ensure_json_body=lambda: {
            "email": "user@example.com",
            "password": "password123",
            "auth_type": "local",
        },
    )


@pytest.mark.asyncio
async def test_auth_login_adapter_commits_login_success_metadata_with_session() -> None:
    adapter = AuthLoginAdapter()
    db = MagicMock()
    db_user = SimpleNamespace(id="user-1", email="user@example.com", last_login_at=None)
    db.query.return_value.filter.return_value.first.return_value = db_user
    context = _build_login_context(db)
    snapshot = AuthenticatedUserSnapshot(
        user_id="user-1",
        email="user@example.com",
        username="tester",
        role=UserRole.USER,
        created_at=datetime.now(timezone.utc),
    )

    with (
        patch(
            "src.api.auth.routes.IPRateLimiter.check_limit",
            new=AsyncMock(return_value=(True, 19, 0)),
        ),
        patch(
            "src.api.auth.routes.AuthService.authenticate_user_threadsafe",
            new=AsyncMock(return_value=snapshot),
        ),
        patch(
            "src.api.auth.routes.get_client_ip",
            return_value="127.0.0.1",
        ),
        patch(
            "src.api.auth.routes.get_user_agent",
            return_value="pytest-agent",
        ),
        patch(
            "src.api.auth.routes.AuditService.log_login_attempt",
        ) as mock_log,
        patch(
            "src.api.auth.routes.UserCacheService.invalidate_user_cache",
            new=AsyncMock(),
        ) as mock_invalidate,
    ):

        def _issue_tokens(**_kwargs: object) -> tuple[str, str, str]:
            assert mock_log.call_count == 0
            return ("session-1", "access-1", "refresh-1")

        with patch(
            "src.api.auth.routes._issue_session_bound_tokens",
            side_effect=_issue_tokens,
        ):
            result = await adapter.handle(context)

    assert result["access_token"] == "access-1"
    assert result["_refresh_token"] == "refresh-1"
    assert db_user.last_login_at is not None
    db.commit.assert_called_once()
    assert context.request.state.tx_committed_by_route is True
    mock_log.assert_called_once_with(
        db=db,
        email="user@example.com",
        success=True,
        ip_address="127.0.0.1",
        user_agent="pytest-agent",
        user_id="user-1",
    )
    mock_invalidate.assert_awaited_once_with("user-1", "user@example.com")


@pytest.mark.asyncio
async def test_auth_login_adapter_skips_success_audit_when_session_creation_fails() -> None:
    adapter = AuthLoginAdapter()
    db = MagicMock()
    db_user = SimpleNamespace(id="user-1", email="user@example.com", last_login_at=None)
    db.query.return_value.filter.return_value.first.return_value = db_user
    context = _build_login_context(db)
    snapshot = AuthenticatedUserSnapshot(
        user_id="user-1",
        email="user@example.com",
        username="tester",
        role=UserRole.USER,
        created_at=datetime.now(timezone.utc),
    )

    with (
        patch(
            "src.api.auth.routes.IPRateLimiter.check_limit",
            new=AsyncMock(return_value=(True, 19, 0)),
        ),
        patch(
            "src.api.auth.routes.AuthService.authenticate_user_threadsafe",
            new=AsyncMock(return_value=snapshot),
        ),
        patch(
            "src.api.auth.routes.get_client_ip",
            return_value="127.0.0.1",
        ),
        patch(
            "src.api.auth.routes.get_user_agent",
            return_value="pytest-agent",
        ),
        patch(
            "src.api.auth.routes.AuditService.log_login_attempt",
        ) as mock_log,
        patch(
            "src.api.auth.routes.UserCacheService.invalidate_user_cache",
            new=AsyncMock(),
        ) as mock_invalidate,
        patch(
            "src.api.auth.routes._issue_session_bound_tokens",
            side_effect=HTTPException(status_code=400, detail="缺少或无效的设备标识"),
        ),
    ):
        with pytest.raises(HTTPException, match="缺少或无效的设备标识"):
            await adapter.handle(context)

    db.commit.assert_not_called()
    mock_log.assert_not_called()
    mock_invalidate.assert_not_awaited()
    assert not hasattr(context.request.state, "tx_committed_by_route")
