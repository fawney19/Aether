from __future__ import annotations

from types import SimpleNamespace
from typing import Any
from unittest.mock import MagicMock

from fastapi import FastAPI
from fastapi.testclient import TestClient
import pytest

from src.api.user_me.routes import UpdateMyApiKeyAdapter, router as me_router
from src.database import get_db


def _build_me_app(db: MagicMock, monkeypatch: Any) -> TestClient:
    app = FastAPI()
    app.include_router(me_router)
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
            user=SimpleNamespace(id="user-1", email="u@example.com"),
            request=SimpleNamespace(state=SimpleNamespace()),
            ensure_json_body=lambda: payload,
            add_audit_metadata=lambda **_: None,
        )
        return await adapter.handle(context)

    monkeypatch.setattr("src.api.user_me.routes.pipeline.run", _fake_pipeline_run)
    return TestClient(app)


async def _fake_update_my_api_key_sync(
    user_id: str,
    key_id: str,
    request: object,
    captured: dict[str, object],
) -> dict[str, object]:
    captured["user_id"] = user_id
    captured["key_id"] = key_id
    captured["name"] = getattr(request, "name", None)
    captured["rate_limit"] = getattr(request, "rate_limit", None)
    return {"id": key_id, "name": captured["name"], "rate_limit": captured["rate_limit"]}


def test_update_my_api_key_route_path_smoke(monkeypatch: Any) -> None:
    captured: dict[str, object] = {}

    def _sync(user_id: str, key_id: str, request: object) -> dict[str, object]:
        captured["user_id"] = user_id
        captured["key_id"] = key_id
        captured["name"] = getattr(request, "name", None)
        captured["rate_limit"] = getattr(request, "rate_limit", None)
        return {"id": key_id, "name": captured["name"], "rate_limit": captured["rate_limit"]}

    monkeypatch.setattr("src.api.user_me.routes._update_my_api_key_sync", _sync)
    client = _build_me_app(MagicMock(), monkeypatch)

    response = client.put("/api/users/me/api-keys/key-1", json={"name": "Edited", "rate_limit": 6})

    assert response.status_code == 200
    assert response.json()["rate_limit"] == 6
    assert captured == {
        "user_id": "user-1",
        "key_id": "key-1",
        "name": "Edited",
        "rate_limit": 6,
    }


@pytest.mark.asyncio
async def test_update_my_api_key_adapter_passes_rate_limit_and_name(monkeypatch: Any) -> None:
    captured: dict[str, object] = {}

    def _sync(user_id: str, key_id: str, request: object) -> dict[str, object]:
        captured["user_id"] = user_id
        captured["key_id"] = key_id
        captured["name"] = getattr(request, "name", None)
        captured["rate_limit"] = getattr(request, "rate_limit", None)
        return {"id": key_id, "name": captured["name"], "rate_limit": captured["rate_limit"]}

    monkeypatch.setattr("src.api.user_me.routes._update_my_api_key_sync", _sync)

    adapter = UpdateMyApiKeyAdapter(key_id="key-2")
    context = SimpleNamespace(
        db=MagicMock(),
        user=SimpleNamespace(id="user-1"),
        request=SimpleNamespace(state=SimpleNamespace()),
        ensure_json_body=lambda: {"name": "Edited Again", "rate_limit": 15},
        add_audit_metadata=lambda **_: None,
    )

    result = await adapter.handle(context)

    assert result["id"] == "key-2"
    assert captured == {
        "user_id": "user-1",
        "key_id": "key-2",
        "name": "Edited Again",
        "rate_limit": 15,
    }
