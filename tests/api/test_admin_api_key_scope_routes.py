from __future__ import annotations

from datetime import datetime, timezone
from types import SimpleNamespace
from unittest.mock import MagicMock

import pytest
from fastapi import FastAPI, HTTPException
from fastapi.testclient import TestClient

from src.api.admin.api_keys.routes import (
    AdminCreateStandaloneKeyAdapter,
    AdminGetFullKeyAdapter,
    AdminToggleApiKeyAdapter,
    AdminUpdateApiKeyAdapter,
)
from src.api.admin.api_keys.routes import router as admin_api_keys_router
from src.api.admin.users.routes import (
    AdminGetUserKeyFullKeyAdapter,
    AdminToggleUserKeyLockAdapter,
    AdminUpdateUserKeyAdapter,
)
from src.api.admin.users.routes import router as admin_users_router
from src.core.exceptions import InvalidRequestException, NotFoundException
from src.database import get_db
from src.models.api import CreateApiKeyRequest


def _build_context(db: MagicMock) -> SimpleNamespace:
    return SimpleNamespace(
        db=db,
        request=SimpleNamespace(state=SimpleNamespace()),
        add_audit_metadata=lambda **_: None,
    )


def _mock_query_first(db: MagicMock, value: object | None) -> None:
    db.query.return_value.filter.return_value.first.return_value = value


def _build_admin_users_app(db: MagicMock, monkeypatch: pytest.MonkeyPatch) -> TestClient:
    app = FastAPI()
    app.include_router(admin_users_router)
    app.dependency_overrides[get_db] = lambda: db

    async def _fake_pipeline_run(
        *, adapter: object, http_request: object, db: MagicMock, mode: object
    ) -> object:
        _ = http_request, mode
        try:
            payload = await http_request.json()
        except Exception:
            payload = {}
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


def _build_admin_api_keys_app(db: MagicMock, monkeypatch: pytest.MonkeyPatch) -> TestClient:
    app = FastAPI()
    app.include_router(admin_api_keys_router)
    app.dependency_overrides[get_db] = lambda: db

    async def _fake_pipeline_run(
        *, adapter: object, http_request: object, db: MagicMock, mode: object
    ) -> object:
        _ = http_request, mode
        context = SimpleNamespace(
            db=db,
            request=SimpleNamespace(state=SimpleNamespace()),
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


@pytest.mark.asyncio
async def test_update_user_key_adapter_passes_rate_limit_and_name(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    captured: dict[str, object] = {}

    def _update_user_key_sync(user_id: str, key_id: str, request: object) -> tuple[dict[str, object], dict[str, object]]:
        captured["user_id"] = user_id
        captured["key_id"] = key_id
        captured["name"] = getattr(request, "name", None)
        captured["rate_limit"] = getattr(request, "rate_limit", None)
        return {"id": key_id, "name": captured["name"], "rate_limit": captured["rate_limit"]}, {}

    monkeypatch.setattr("src.api.admin.users.routes._update_user_key_sync", _update_user_key_sync)

    adapter = AdminUpdateUserKeyAdapter(user_id="user-1", key_id="key-7")
    context = SimpleNamespace(
        db=MagicMock(),
        request=SimpleNamespace(state=SimpleNamespace()),
        ensure_json_body=lambda: {"name": "Renamed Key", "rate_limit": 12},
        add_audit_metadata=lambda **_: None,
    )

    result = await adapter.handle(context)

    assert result["id"] == "key-7"
    assert captured == {
        "user_id": "user-1",
        "key_id": "key-7",
        "name": "Renamed Key",
        "rate_limit": 12,
    }


def test_update_user_key_route_path_smoke(monkeypatch: pytest.MonkeyPatch) -> None:
    captured: dict[str, object] = {}

    def _update_user_key_sync(user_id: str, key_id: str, request: object) -> tuple[dict[str, object], dict[str, object]]:
        captured["user_id"] = user_id
        captured["key_id"] = key_id
        captured["name"] = getattr(request, "name", None)
        captured["rate_limit"] = getattr(request, "rate_limit", None)
        return {"id": key_id, "name": captured["name"], "rate_limit": captured["rate_limit"]}, {}

    monkeypatch.setattr("src.api.admin.users.routes._update_user_key_sync", _update_user_key_sync)
    client = _build_admin_users_app(MagicMock(), monkeypatch)

    response = client.put(
        "/api/admin/users/user-2/api-keys/key-8",
        json={"name": "Updated", "rate_limit": 9},
    )

    assert response.status_code == 200
    assert response.json()["rate_limit"] == 9
    assert captured == {
        "user_id": "user-2",
        "key_id": "key-8",
        "name": "Updated",
        "rate_limit": 9,
    }


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
        lambda _db, user_id=None, api_key_id=None, user=None, api_key=None: SimpleNamespace(
            id="w-1"
        ),
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


@pytest.mark.asyncio
async def test_create_standalone_key_adapter_preserves_empty_restriction_lists(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    captured: dict[str, object] = {}
    created_key = SimpleNamespace(
        id="sa-key-3",
        name="Standalone Key 3",
        get_display_key=lambda: "sk-stand...9012",
        is_active=True,
        rate_limit=None,
        expires_at=None,
        created_at=datetime.now(timezone.utc),
        allowed_providers=[],
        allowed_api_formats=[],
        allowed_models=[],
    )

    def _create_api_key(**kwargs: object) -> tuple[SimpleNamespace, str]:
        captured.update(kwargs)
        return created_key, "sk-created"

    monkeypatch.setattr(
        "src.api.admin.api_keys.routes.ApiKeyService.create_api_key", _create_api_key
    )
    monkeypatch.setattr(
        "src.api.admin.api_keys.routes.WalletService.initialize_api_key_wallet",
        lambda *_a, **_k: SimpleNamespace(id="wallet-1"),
    )
    monkeypatch.setattr(
        "src.api.admin.api_keys.routes.WalletService.serialize_wallet_summary",
        lambda _wallet: {"id": "wallet-1"},
    )

    adapter = AdminCreateStandaloneKeyAdapter(
        CreateApiKeyRequest(
            name="Standalone Key 3",
            initial_balance_usd=10,
            allowed_providers=[],
            allowed_api_formats=[],
            allowed_models=[],
        )
    )
    context = SimpleNamespace(
        db=db,
        user=SimpleNamespace(id="admin-1"),
        request=SimpleNamespace(state=SimpleNamespace()),
        add_audit_metadata=lambda **_: None,
    )

    result = await adapter.handle(context)

    assert result["id"] == "sa-key-3"
    assert captured["allowed_providers"] == []
    assert captured["allowed_api_formats"] == []
    assert captured["allowed_models"] == []


@pytest.mark.asyncio
async def test_update_standalone_key_adapter_preserves_empty_restriction_lists(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    existing_key = SimpleNamespace(id="sa-key-4", is_standalone=True)
    _mock_query_first(db, existing_key)

    updated_key = SimpleNamespace(
        id="sa-key-4",
        name="Standalone Key 4",
        get_display_key=lambda: "sk-stand...3456",
        is_active=True,
        rate_limit=None,
        expires_at=None,
        updated_at=datetime.now(timezone.utc),
    )
    captured: dict[str, object] = {}

    def _update_api_key(_db: MagicMock, _key_id: str, **kwargs: object) -> SimpleNamespace:
        captured.update(kwargs)
        return updated_key

    monkeypatch.setattr(
        "src.api.admin.api_keys.routes.ApiKeyService.update_api_key", _update_api_key
    )
    monkeypatch.setattr(
        "src.api.admin.api_keys.routes._ensure_standalone_wallet",
        lambda *_a, **_k: SimpleNamespace(id="wallet-2"),
    )
    monkeypatch.setattr(
        "src.api.admin.api_keys.routes.WalletService.serialize_wallet_summary",
        lambda _wallet: {"id": "wallet-2"},
    )

    adapter = AdminUpdateApiKeyAdapter(
        key_id="sa-key-4",
        key_data=CreateApiKeyRequest(
            allowed_providers=[],
            allowed_api_formats=[],
            allowed_models=[],
        ),
    )

    result = await adapter.handle(_build_context(db))

    assert result["id"] == "sa-key-4"
    assert captured["allowed_providers"] == []
    assert captured["allowed_api_formats"] == []
    assert captured["allowed_models"] == []
