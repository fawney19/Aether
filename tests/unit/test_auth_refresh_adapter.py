from __future__ import annotations

from datetime import datetime, timezone
from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock, patch

import pytest

from src.api.auth.routes import AuthRefreshAdapter
from src.config import config


@pytest.mark.asyncio
async def test_auth_refresh_adapter_skips_rotation_for_grace_window_token() -> None:
    adapter = AuthRefreshAdapter()
    created_at = datetime.now(timezone.utc)
    user = SimpleNamespace(
        id="user-1",
        role=SimpleNamespace(value="user"),
        created_at=created_at,
        is_active=True,
        is_deleted=False,
    )
    session = SimpleNamespace(id="session-1", client_device_id="device-1")
    db = MagicMock()
    db.query.return_value.filter.return_value.first.return_value = user
    request = SimpleNamespace(
        headers={"content-length": "0", "user-agent": "pytest"},
        cookies={config.auth_refresh_cookie_name: "refresh-old"},
        query_params={},
        state=SimpleNamespace(),
    )
    context = SimpleNamespace(db=db, request=request)

    with (
        patch(
            "src.api.auth.routes.AuthService.verify_token",
            new=AsyncMock(
                return_value={
                    "user_id": "user-1",
                    "session_id": "session-1",
                    "created_at": created_at.isoformat(),
                }
            ),
        ),
        patch(
            "src.api.auth.routes.AuthService.token_identity_matches_user",
            return_value=True,
        ),
        patch(
            "src.api.auth.routes.SessionService.extract_client_device_id",
            return_value="device-1",
        ),
        patch(
            "src.api.auth.routes.SessionService.validate_refresh_session",
            return_value=(session, True),
        ),
        patch("src.api.auth.routes.SessionService.assert_session_device_matches"),
        patch(
            "src.api.auth.routes.AuthService.create_access_token",
            return_value="access-new",
        ),
        patch("src.api.auth.routes.AuthService.create_refresh_token") as mock_create_refresh,
        patch("src.api.auth.routes.SessionService.rotate_refresh_token") as mock_rotate,
        patch(
            "src.api.auth.routes.get_client_ip",
            return_value="127.0.0.1",
        ),
        patch(
            "src.api.auth.routes.get_user_agent",
            return_value="pytest-agent",
        ),
    ):
        result = await adapter.handle(context)

    assert result == {
        "access_token": "access-new",
        "token_type": "bearer",
        "expires_in": 86400,
    }
    mock_create_refresh.assert_not_called()
    mock_rotate.assert_not_called()
    db.commit.assert_called_once()
    assert context.request.state.tx_committed_by_route is True
