from __future__ import annotations

from datetime import datetime, timezone
from types import SimpleNamespace
from unittest.mock import MagicMock

import pytest
from fastapi import FastAPI, HTTPException
from fastapi.testclient import TestClient

from src.api.admin.api_keys.routes import (
    AdminGetFullKeyAdapter,
    AdminToggleApiKeyAdapter,
    router as admin_api_keys_router,
)
from src.api.admin.users.routes import (
    AdminGetUserKeyFullKeyAdapter,
    AdminToggleUserKeyLockAdapter,
    router as admin_users_router,
)
from src.core.exceptions import InvalidRequestException, NotFoundException
from src.database import get_db


def _build_context(db: MagicMock) -> SimpleNamespace:
    return SimpleNamespace(
        db=db,
        add_audit_metadata=lambda **_: None,
    )


def _mock_query_first(db: MagicMock, value: object | None) -> None:
    db.query.return_value.filter.return_value.first.return_value = value


def _build_admin_users_app(db: MagicMock, monkeypatch: pytest.MonkeyPatch) -> TestClient:
    app = FastAPI()
    app.include_router(admin_users_router)
    app.dependency_overrides[get_db] = lambda: db

    async def _fake_pipeline_run(*, adapter: object, http_request: object, db: MagicMock, mode: object) -> object:
        _ = http_request, mode
        context = SimpleNamespace(
            db=db,
            user=SimpleNamespace(id="admin-1"),
            ensure_json_body=lambda: {},
            add_audit_metadata=lambda **_: None,
        )
        return await adapter.handle(context)

    monkeypatch.setattr("src.api.admin.users.routes.pipeline.run", _fake_pipeline_run)
    return TestClient(app)


def _build_admin_api_keys_app(db: MagicMock, monkeypatch: pytest.MonkeyPatch) -> TestClient:
    app = FastAPI()
    app.include_router(admin_api_keys_router)
    app.dependency_overrides[get_db] = lambda: db

    async def _fake_pipeline_run(*, adapter: object, http_request: object, db: MagicMock, mode: object) -> object:
        _ = http_request, mode
        context = SimpleNamespace(
            db=db,
            user=SimpleNamespace(id="admin-1"),
            ensure_json_body=lambda: {},
            add_audit_metadata=lambda **_: None,
        )
        return await adapter.handle(context)

    monkeypatch.setattr("src.api.admin.api_keys.routes.pipeline.run", _fake_pipeline_run)
    return TestClient(app)


@pytest.mark.asyncio
async def test_toggle_user_key_lock_adapter_success() -> None:
    db = MagicMock()
    api_key = SimpleNamespace(id="key-1", user_id="user-1", is_standalone=False, is_locked=False)
    _mock_query_first(db, api_key)

    adapter = AdminToggleUserKeyLockAdapter(user_id="user-1", key_id="key-1")
    result = await adapter.handle(_build_context(db))

    assert result["id"] == "key-1"
    assert result["is_locked"] is True
    assert "锁定" in result["message"]
    db.commit.assert_called_once()
    db.refresh.assert_called_once_with(api_key)


@pytest.mark.asyncio
async def test_toggle_user_key_lock_adapter_not_found_for_standalone_or_wrong_owner() -> None:
    db = MagicMock()
    _mock_query_first(db, None)

    adapter = AdminToggleUserKeyLockAdapter(user_id="user-1", key_id="key-standalone")
    with pytest.raises(NotFoundException):
        await adapter.handle(_build_context(db))

    db.commit.assert_not_called()


@pytest.mark.asyncio
async def test_get_user_key_full_key_adapter_success(monkeypatch: pytest.MonkeyPatch) -> None:
    db = MagicMock()
    api_key = SimpleNamespace(
        id="key-2",
        user_id="user-1",
        is_standalone=False,
        key_encrypted="encrypted-value",
    )
    _mock_query_first(db, api_key)

    monkeypatch.setattr("src.core.crypto.crypto_service.decrypt", lambda _v: "sk-user-full-key")

    adapter = AdminGetUserKeyFullKeyAdapter(user_id="user-1", key_id="key-2")
    result = await adapter.handle(_build_context(db))

    assert result == {"key": "sk-user-full-key"}


@pytest.mark.asyncio
async def test_get_user_key_full_key_adapter_requires_encrypted_key() -> None:
    db = MagicMock()
    api_key = SimpleNamespace(
        id="key-3",
        user_id="user-1",
        is_standalone=False,
        key_encrypted=None,
    )
    _mock_query_first(db, api_key)

    adapter = AdminGetUserKeyFullKeyAdapter(user_id="user-1", key_id="key-3")
    with pytest.raises(InvalidRequestException):
        await adapter.handle(_build_context(db))


@pytest.mark.asyncio
async def test_get_user_key_full_key_adapter_returns_500_on_decrypt_error(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    api_key = SimpleNamespace(
        id="key-4",
        user_id="user-1",
        is_standalone=False,
        key_encrypted="encrypted-value",
    )
    _mock_query_first(db, api_key)

    def _raise(_: str) -> str:
        raise ValueError("decrypt failed")

    monkeypatch.setattr("src.core.crypto.crypto_service.decrypt", _raise)

    adapter = AdminGetUserKeyFullKeyAdapter(user_id="user-1", key_id="key-4")
    with pytest.raises(HTTPException) as exc_info:
        await adapter.handle(_build_context(db))

    assert exc_info.value.status_code == 500


@pytest.mark.asyncio
async def test_standalone_toggle_adapters_reject_normal_user_key() -> None:
    db = MagicMock()
    normal_key = SimpleNamespace(
        id="key-user",
        user_id="user-1",
        is_standalone=False,
        is_active=True,
        is_locked=False,
        key_encrypted="encrypted-value",
        updated_at=datetime.now(timezone.utc),
    )
    _mock_query_first(db, normal_key)
    context = _build_context(db)

    with pytest.raises(InvalidRequestException):
        await AdminToggleApiKeyAdapter(key_id="key-user").handle(context)

    with pytest.raises(InvalidRequestException):
        await AdminGetFullKeyAdapter(key_id="key-user").handle(context)


def test_user_key_lock_route_path_smoke(monkeypatch: pytest.MonkeyPatch) -> None:
    db = MagicMock()
    api_key = SimpleNamespace(id="key-5", user_id="user-2", is_standalone=False, is_locked=False)
    _mock_query_first(db, api_key)
    client = _build_admin_users_app(db, monkeypatch)

    response = client.patch("/api/admin/users/user-2/api-keys/key-5/lock")
    assert response.status_code == 200
    assert response.json()["id"] == "key-5"
    assert response.json()["is_locked"] is True


def test_user_key_full_key_route_path_smoke(monkeypatch: pytest.MonkeyPatch) -> None:
    db = MagicMock()
    api_key = SimpleNamespace(
        id="key-6",
        user_id="user-2",
        is_standalone=False,
        key_encrypted="enc",
    )
    _mock_query_first(db, api_key)
    monkeypatch.setattr("src.core.crypto.crypto_service.decrypt", lambda _v: "sk-user-route-key")
    client = _build_admin_users_app(db, monkeypatch)

    response = client.get("/api/admin/users/user-2/api-keys/key-6/full-key")
    assert response.status_code == 200
    assert response.json() == {"key": "sk-user-route-key"}


def test_standalone_lock_route_removed(monkeypatch: pytest.MonkeyPatch) -> None:
    client = _build_admin_api_keys_app(MagicMock(), monkeypatch)
    response = client.patch("/api/admin/api-keys/key-1/lock")
    assert response.status_code == 404


def test_standalone_list_route_does_not_expose_is_locked(monkeypatch: pytest.MonkeyPatch) -> None:
    db = MagicMock()
    api_key = SimpleNamespace(
        id="sa-key-1",
        user_id="admin-1",
        name="Standalone Key",
        get_display_key=lambda: "sk-stand...1234",
        is_active=True,
        is_standalone=True,
        total_requests=0,
        total_cost_usd=0,
        rate_limit=None,
        allowed_providers=None,
        allowed_api_formats=None,
        allowed_models=None,
        last_used_at=None,
        expires_at=None,
        created_at=datetime.now(timezone.utc),
        updated_at=None,
        auto_delete_on_expiry=False,
    )
    query = db.query.return_value.filter.return_value
    query.count.return_value = 1
    query.order_by.return_value.offset.return_value.limit.return_value.all.return_value = [api_key]

    monkeypatch.setattr(
        "src.api.admin.api_keys.routes.WalletService.get_wallet",
        lambda _db, user_id=None, api_key_id=None, user=None, api_key=None: SimpleNamespace(id="w-1"),
    )
    client = _build_admin_api_keys_app(db, monkeypatch)

    response = client.get("/api/admin/api-keys")
    assert response.status_code == 200
    payload = response.json()
    assert len(payload["api_keys"]) == 1
    assert "is_locked" not in payload["api_keys"][0]


def test_standalone_detail_route_does_not_expose_is_locked(monkeypatch: pytest.MonkeyPatch) -> None:
    db = MagicMock()
    api_key = SimpleNamespace(
        id="sa-key-2",
        user_id="admin-1",
        name="Standalone Key 2",
        get_display_key=lambda: "sk-stand...5678",
        is_active=True,
        is_standalone=True,
        total_requests=0,
        total_cost_usd=0,
        rate_limit=None,
        allowed_providers=[],
        allowed_api_formats=[],
        allowed_models=[],
        last_used_at=None,
        expires_at=None,
        created_at=datetime.now(timezone.utc),
        updated_at=None,
    )
    _mock_query_first(db, api_key)

    monkeypatch.setattr(
        "src.api.admin.api_keys.routes.WalletService.get_wallet",
        lambda _db, user_id=None, api_key_id=None, user=None, api_key=None: None,
    )
    client = _build_admin_api_keys_app(db, monkeypatch)

    response = client.get("/api/admin/api-keys/sa-key-2")
    assert response.status_code == 200
    payload = response.json()
    assert payload["id"] == "sa-key-2"
    assert "is_locked" not in payload
