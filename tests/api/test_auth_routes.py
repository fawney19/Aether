from __future__ import annotations

from types import SimpleNamespace
from typing import Any
from unittest.mock import AsyncMock, MagicMock

import pytest
from fastapi import FastAPI, HTTPException
from fastapi.testclient import TestClient

from src.api.auth.routes import _logout_with_refresh_cookie_fallback
from src.api.auth.routes import router as auth_router
from src.config import config
from src.database import get_db


def _build_auth_app(
    db: MagicMock,
    monkeypatch: pytest.MonkeyPatch,
    pipeline_result: Any = None,
    pipeline_exception: Exception | None = None,
) -> TestClient:
    app = FastAPI()
    app.include_router(auth_router)
    app.dependency_overrides[get_db] = lambda: db

    async def _fake_pipeline_run(
        *, adapter: Any, http_request: object, db: MagicMock, mode: object
    ) -> Any:
        _ = adapter, http_request, db, mode
        if pipeline_exception is not None:
            raise pipeline_exception
        return pipeline_result

    monkeypatch.setattr("src.api.auth.routes.pipeline.run", _fake_pipeline_run)
    return TestClient(app)


def test_login_route_sets_refresh_cookie_and_hides_token(monkeypatch: pytest.MonkeyPatch) -> None:
    db = MagicMock()
    client = _build_auth_app(
        db,
        monkeypatch,
        pipeline_result={
            "access_token": "access-1",
            "token_type": "bearer",
            "expires_in": 86400,
            "user_id": "user-1",
            "username": "tester",
            "role": "user",
            "_refresh_token": "refresh-1",
        },
    )

    response = client.post("/api/auth/login", json={"email": "user@example.com", "password": "pw"})

    assert response.status_code == 200
    assert response.json() == {
        "access_token": "access-1",
        "token_type": "bearer",
        "expires_in": 86400,
        "user_id": "user-1",
        "username": "tester",
        "role": "user",
    }
    set_cookie = response.headers.get("set-cookie", "")
    assert config.auth_refresh_cookie_name in set_cookie
    assert "refresh-1" in set_cookie
    assert "HttpOnly" in set_cookie


def test_refresh_route_sets_cookie_and_hides_token(monkeypatch: pytest.MonkeyPatch) -> None:
    db = MagicMock()
    client = _build_auth_app(
        db,
        monkeypatch,
        pipeline_result={
            "access_token": "access-2",
            "token_type": "bearer",
            "expires_in": 86400,
            "_refresh_token": "refresh-2",
        },
    )

    response = client.post("/api/auth/refresh")

    assert response.status_code == 200
    assert response.json() == {
        "access_token": "access-2",
        "token_type": "bearer",
        "expires_in": 86400,
    }
    set_cookie = response.headers.get("set-cookie", "")
    assert config.auth_refresh_cookie_name in set_cookie
    assert "refresh-2" in set_cookie
    assert "HttpOnly" in set_cookie


def test_refresh_route_clears_cookie_on_error(monkeypatch: pytest.MonkeyPatch) -> None:
    db = MagicMock()
    client = _build_auth_app(
        db,
        monkeypatch,
        pipeline_exception=HTTPException(status_code=401, detail="登录会话已失效，请重新登录"),
    )

    response = client.post("/api/auth/refresh")

    assert response.status_code == 401
    assert response.json()["error"]["message"] == "登录会话已失效，请重新登录"
    set_cookie = response.headers.get("set-cookie", "")
    assert config.auth_refresh_cookie_name in set_cookie
    assert "Max-Age=0" in set_cookie


def test_logout_route_clears_cookie_on_error(monkeypatch: pytest.MonkeyPatch) -> None:
    db = MagicMock()
    client = _build_auth_app(
        db,
        monkeypatch,
        pipeline_exception=HTTPException(status_code=401, detail="缺少认证令牌"),
    )

    response = client.post("/api/auth/logout")

    assert response.status_code == 401
    assert response.json()["error"]["message"] == "缺少认证令牌"
    set_cookie = response.headers.get("set-cookie", "")
    assert config.auth_refresh_cookie_name in set_cookie
    assert "Max-Age=0" in set_cookie


def test_logout_route_uses_refresh_cookie_fallback_on_auth_error(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    client = _build_auth_app(
        db,
        monkeypatch,
        pipeline_exception=HTTPException(status_code=401, detail="Token已过期"),
    )

    async def _fake_fallback(_request: object, _db: MagicMock) -> dict[str, Any]:
        return {"message": "登出成功", "success": True}

    monkeypatch.setattr(
        "src.api.auth.routes._logout_with_refresh_cookie_fallback",
        _fake_fallback,
    )

    response = client.post(
        "/api/auth/logout",
        cookies={config.auth_refresh_cookie_name: "refresh-1"},
    )

    assert response.status_code == 200
    assert response.json() == {"message": "登出成功", "success": True}
    set_cookie = response.headers.get("set-cookie", "")
    assert config.auth_refresh_cookie_name in set_cookie
    assert "Max-Age=0" in set_cookie


@pytest.mark.asyncio
async def test_logout_with_refresh_cookie_fallback_revokes_session(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    user = SimpleNamespace(id="user-1", email="user@example.com")
    session = SimpleNamespace(id="session-1", is_revoked=False, is_expired=False)
    db.query.return_value.filter.return_value.first.return_value = user
    request = SimpleNamespace(
        cookies={config.auth_refresh_cookie_name: "refresh-1"},
        headers={},
        query_params={},
        state=SimpleNamespace(),
    )

    monkeypatch.setattr(
        "src.api.auth.routes.AuthService.verify_token",
        AsyncMock(
            return_value={
                "user_id": "user-1",
                "session_id": "session-1",
            }
        ),
    )
    monkeypatch.setattr(
        "src.api.auth.routes.SessionService.extract_client_device_id",
        lambda _request: "device-1",
    )
    monkeypatch.setattr(
        "src.api.auth.routes.SessionService.get_session_for_user",
        lambda *_args, **_kwargs: session,
    )
    assert_session_device_matches = MagicMock()
    revoke_session = MagicMock()
    log_event = MagicMock()
    monkeypatch.setattr(
        "src.api.auth.routes.SessionService.assert_session_device_matches",
        assert_session_device_matches,
    )
    monkeypatch.setattr(
        "src.api.auth.routes.SessionService.revoke_session",
        revoke_session,
    )
    monkeypatch.setattr("src.api.auth.routes.AuditService.log_event", log_event)
    monkeypatch.setattr("src.api.auth.routes.get_client_ip", lambda _request: "127.0.0.1")
    monkeypatch.setattr(
        "src.api.auth.routes.get_user_agent",
        lambda _request: "pytest-agent",
    )

    result = await _logout_with_refresh_cookie_fallback(request, db)

    assert result == {"message": "登出成功", "success": True}
    assert_session_device_matches.assert_called_once_with(session, "device-1")
    revoke_session.assert_called_once()
    log_event.assert_called_once()
    db.commit.assert_called_once()
    assert request.state.tx_committed_by_route is True
