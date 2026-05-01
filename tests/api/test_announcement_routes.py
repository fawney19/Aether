from __future__ import annotations

import json
from types import SimpleNamespace
from typing import Any
from unittest.mock import MagicMock

import pytest
from fastapi import FastAPI
from fastapi.testclient import TestClient

from src.api.announcements.routes import router as announcements_router
from src.database import get_db


def _build_announcements_app(
    db: MagicMock,
    monkeypatch: pytest.MonkeyPatch,
) -> TestClient:
    app = FastAPI()
    app.include_router(announcements_router)
    app.dependency_overrides[get_db] = lambda: db

    async def _fake_pipeline_run(
        *,
        adapter: Any,
        http_request: object,
        db: MagicMock,
        mode: object,
    ) -> Any:
        _ = mode
        request_body = await http_request.body()
        payload = json.loads(request_body) if request_body else {}
        context = SimpleNamespace(
            db=db,
            request=SimpleNamespace(state=SimpleNamespace(), headers=http_request.headers),
            user=SimpleNamespace(id="user-1"),
            ensure_json_body=lambda: payload,
            add_audit_metadata=lambda **_: None,
            extra={},
        )
        return await adapter.handle(context)

    monkeypatch.setattr("src.api.announcements.routes.pipeline.run", _fake_pipeline_run)
    return TestClient(app)


def test_mark_all_announcements_as_read_returns_marked_count(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    client = _build_announcements_app(db, monkeypatch)
    captured: dict[str, Any] = {}

    def _fake_mark_all_as_read(db_arg: Any, user_id: str, active_only: bool = True) -> int:
        captured["db"] = db_arg
        captured["user_id"] = user_id
        captured["active_only"] = active_only
        return 3

    monkeypatch.setattr(
        "src.api.announcements.routes.AnnouncementService.mark_all_as_read",
        _fake_mark_all_as_read,
    )

    response = client.post("/api/announcements/read-all")

    assert response.status_code == 200
    assert response.json() == {"message": "已全部标记为已读", "marked_count": 3}
    assert captured == {"db": db, "user_id": "user-1", "active_only": True}
