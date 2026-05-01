from __future__ import annotations

import json
from datetime import datetime, timezone
from types import SimpleNamespace
from typing import Any
from unittest.mock import MagicMock

import pytest
from fastapi import FastAPI
from fastapi.testclient import TestClient

from src.api.admin.users.routes import AdminBatchUserGroupBindingAdapter
from src.api.admin.users.routes import AdminCreateUserAdapter
from src.api.admin.users.routes import router as admin_users_router
from src.core.exceptions import InvalidRequestException
from src.database import get_db


def _build_admin_users_app(db: MagicMock, monkeypatch: pytest.MonkeyPatch) -> TestClient:
    app = FastAPI()
    app.include_router(admin_users_router)
    app.dependency_overrides[get_db] = lambda: db

    async def _fake_pipeline_run(
        *, adapter: Any, http_request: object, db: MagicMock, mode: object
    ) -> Any:
        _ = mode
        request_body = await http_request.body()
        payload = json.loads(request_body) if request_body else {}
        context = SimpleNamespace(
            db=db,
            request=SimpleNamespace(state=SimpleNamespace()),
            user=SimpleNamespace(id="admin-1"),
            ensure_json_body=lambda: payload,
            add_audit_metadata=lambda **_: None,
        )
        return await adapter.handle(context)

    monkeypatch.setattr("src.api.admin.users.routes.pipeline.run", _fake_pipeline_run)
    return TestClient(app)


def test_list_users_uses_wallet_batch_lookup(monkeypatch: pytest.MonkeyPatch) -> None:
    db = MagicMock()
    client = _build_admin_users_app(db, monkeypatch)
    now = datetime.now(timezone.utc)
    users = [
        SimpleNamespace(
            id="user-1",
            email="u1@example.com",
            username="user1",
            role=SimpleNamespace(value="user"),
            group_id=None,
            group=None,
            is_active=True,
            created_at=now,
            updated_at=now,
            last_login_at=None,
        ),
        SimpleNamespace(
            id="user-2",
            email="u2@example.com",
            username="user2",
            role=SimpleNamespace(value="admin"),
            group_id=None,
            group=None,
            is_active=True,
            created_at=now,
            updated_at=None,
            last_login_at=None,
        ),
    ]
    wallets_by_user_id = {
        "user-1": SimpleNamespace(limit_mode="unlimited"),
    }
    batch_getter = MagicMock(return_value=wallets_by_user_id)

    monkeypatch.setattr(
        "src.api.admin.users.routes.UserService.list_users", lambda *_a, **_k: users
    )
    monkeypatch.setattr(
        "src.api.admin.users.routes.WalletService.get_wallets_by_user_ids",
        batch_getter,
    )
    monkeypatch.setattr(
        "src.api.admin.users.routes.WalletService.get_wallet",
        lambda *_a, **_k: (_ for _ in ()).throw(AssertionError("不应回退到逐个钱包查询")),
    )

    response = client.get("/api/admin/users")

    assert response.status_code == 200
    assert response.json()[0]["unlimited"] is True
    assert response.json()[1]["unlimited"] is False
    batch_getter.assert_called_once()
    assert batch_getter.call_args.args[1] == ["user-1", "user-2"]


def test_list_user_sessions_route_returns_sessions(monkeypatch: pytest.MonkeyPatch) -> None:
    db = MagicMock()
    client = _build_admin_users_app(db, monkeypatch)
    sessions = [
        {
            "id": "session-1",
            "device_label": "Chrome / macOS",
            "device_type": "desktop",
            "created_at": datetime.now(timezone.utc).isoformat(),
            "is_current": False,
        }
    ]

    monkeypatch.setattr(
        "src.api.admin.users.routes._list_user_sessions_sync",
        lambda user_id: (
            sessions,
            {"action": "list_user_sessions", "target_user_id": user_id},
        ),
    )

    response = client.get("/api/admin/users/user-1/sessions")

    assert response.status_code == 200
    assert response.json() == sessions


def test_revoke_user_session_route_returns_message(monkeypatch: pytest.MonkeyPatch) -> None:
    db = MagicMock()
    client = _build_admin_users_app(db, monkeypatch)

    monkeypatch.setattr(
        "src.api.admin.users.routes._revoke_user_session_sync",
        lambda user_id, session_id, admin_user_id: (
            {"message": f"{user_id}:{session_id}:revoked"},
            {"action": "revoke_user_session", "target_user_id": user_id, "session_id": session_id},
        ),
    )

    response = client.delete("/api/admin/users/user-1/sessions/session-1")

    assert response.status_code == 200
    assert response.json() == {"message": "user-1:session-1:revoked"}


def test_revoke_all_user_sessions_route_returns_count(monkeypatch: pytest.MonkeyPatch) -> None:
    db = MagicMock()
    client = _build_admin_users_app(db, monkeypatch)

    monkeypatch.setattr(
        "src.api.admin.users.routes._revoke_all_user_sessions_sync",
        lambda user_id, admin_user_id: (
            {"message": "done", "revoked_count": 2},
            {"action": "revoke_all_user_sessions", "target_user_id": user_id, "revoked_count": 2},
        ),
    )

    response = client.delete("/api/admin/users/user-1/sessions")

    assert response.status_code == 200
    assert response.json() == {"message": "done", "revoked_count": 2}


def test_batch_user_group_binding_route_forwards_validated_request(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    client = _build_admin_users_app(db, monkeypatch)
    captured: dict[str, Any] = {}

    def _fake_batch_update(request: Any) -> tuple[dict[str, Any], dict[str, Any]]:
        captured["request"] = request
        return (
            {
                "action": request.action,
                "group_id": request.group_id,
                "source_group_id": request.source_group_id,
                "updated_count": 2,
                "skipped_count": 0,
                "users": [],
            },
            {"action": "batch_user_group_binding"},
        )

    monkeypatch.setattr(
        "src.api.admin.users.routes._batch_update_user_group_binding_sync",
        _fake_batch_update,
    )

    response = client.post(
        "/api/admin/users/groups/bindings/batch",
        json={
            "action": "bind",
            "user_ids": ["user-1", "user-2", "user-1"],
            "group_id": "group-1",
        },
    )

    assert response.status_code == 200
    assert response.json()["action"] == "bind"
    assert response.json()["updated_count"] == 2
    assert captured["request"].action == "bind"
    assert captured["request"].user_ids == ["user-1", "user-2"]
    assert captured["request"].group_id == "group-1"


@pytest.mark.asyncio
async def test_batch_user_group_binding_adapter_requires_group_id() -> None:
    context = SimpleNamespace(
        db=MagicMock(),
        request=SimpleNamespace(state=SimpleNamespace()),
        ensure_json_body=lambda: {
            "action": "bind",
            "user_ids": ["user-1"],
        },
        add_audit_metadata=lambda **_: None,
    )

    with pytest.raises(InvalidRequestException, match="group_id"):
        await AdminBatchUserGroupBindingAdapter().handle(context)


@pytest.mark.asyncio
async def test_create_user_adapter_accepts_group_id(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    captured: dict[str, Any] = {}

    def _fake_create_user_sync(request: Any, role: Any) -> tuple[dict[str, Any], dict[str, Any]]:
        captured["request"] = request
        captured["role"] = role
        return {"id": "user-3"}, {"action": "create_user", "target_user_id": "user-3"}

    monkeypatch.setattr("src.api.admin.users.routes._create_user_sync", _fake_create_user_sync)

    context = SimpleNamespace(
        db=db,
        request=SimpleNamespace(state=SimpleNamespace()),
        ensure_json_body=lambda: {
            "username": "user3",
            "password": "Abcd12",
            "email": "u3@example.com",
            "role": "user",
            "initial_gift_usd": 10,
            "group_id": "group-1",
        },
        add_audit_metadata=lambda **_: None,
    )

    result = await AdminCreateUserAdapter().handle(context)

    assert result == {"id": "user-3"}
    assert captured["request"].group_id == "group-1"
