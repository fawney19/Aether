from __future__ import annotations

import sys
import types
from datetime import datetime, timezone
from types import SimpleNamespace
from typing import Any

import httpx
import pytest

from src.services.provider import auth as module


class _FakeQuery:
    def __init__(self, row: Any | None) -> None:
        self._row = row

    def filter(self, *_args: Any, **_kwargs: Any) -> "_FakeQuery":
        return self

    def first(self) -> Any | None:
        return self._row


class _FakeDB:
    def __init__(self, row: Any | None) -> None:
        self.row = row
        self.committed = False

    def query(self, _model: Any) -> _FakeQuery:
        return _FakeQuery(self.row)

    def commit(self) -> None:
        self.committed = True


class _FakeSessionCtx:
    def __init__(self, db: _FakeDB) -> None:
        self.db = db

    def __enter__(self) -> _FakeDB:
        return self.db

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> bool:
        _ = exc_type, exc, tb
        return False


def _install_module(
    monkeypatch: pytest.MonkeyPatch, name: str, attrs: dict[str, Any]
) -> None:
    fake_module = types.ModuleType(name)
    for key, value in attrs.items():
        setattr(fake_module, key, value)
    monkeypatch.setitem(sys.modules, name, fake_module)


def test_persist_refreshed_token_detached_key_does_not_raise(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    key = SimpleNamespace(
        id="key-1",
        api_key="old-api",
        auth_config="old-config",
        oauth_invalid_at=datetime.now(timezone.utc),
        oauth_invalid_reason="[REFRESH_FAILED] stale",
    )

    monkeypatch.setattr(
        module, "object_session", lambda _key: (_ for _ in ()).throw(RuntimeError())
    )
    monkeypatch.setattr(module.crypto_service, "encrypt", lambda value: f"enc:{value}")

    module._persist_refreshed_token(key, "new-token", {"refresh_token": "rt-2"})

    assert key.api_key == "enc:new-token"
    assert key.auth_config == 'enc:{"refresh_token": "rt-2"}'
    assert key.oauth_invalid_at is None
    assert key.oauth_invalid_reason is None


def test_mark_refresh_token_invalid_persists_detached_key(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    key = SimpleNamespace(id="key-1")
    row = SimpleNamespace(id="key-1", oauth_invalid_at=None, oauth_invalid_reason=None)
    fake_db = _FakeDB(row)

    monkeypatch.setattr(
        module, "object_session", lambda _key: (_ for _ in ()).throw(RuntimeError())
    )
    _install_module(
        monkeypatch,
        "src.database",
        {"create_session": lambda: _FakeSessionCtx(fake_db)},
    )
    _install_module(
        monkeypatch,
        "src.models.database",
        {"ProviderAPIKey": type("ProviderAPIKey", (), {"id": "id"})},
    )

    module._mark_refresh_token_invalid(
        key,
        401,
        '{"error": {"code": "refresh_token_reused", "message": "used"}}',
    )

    assert fake_db.committed is True
    assert key.oauth_invalid_at is not None
    assert row.oauth_invalid_at is not None
    assert str(key.oauth_invalid_reason).startswith(
        "[REFRESH_FAILED] Token 续期失败 (401)"
    )
    assert "refresh_token_reused" in str(row.oauth_invalid_reason)


def test_account_block_token_invalidated_is_refresh_recoverable() -> None:
    from src.services.provider.oauth_token import is_account_level_block

    assert (
        is_account_level_block(
            "[ACCOUNT_BLOCK] Authentication token has been invalidated. Please sign in again."
        )
        is False
    )


def test_persist_refreshed_token_clears_legacy_token_invalidated_account_block(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    key = SimpleNamespace(
        id="key-1",
        api_key="old-api",
        auth_config="old-config",
        oauth_invalid_at=datetime.now(timezone.utc),
        oauth_invalid_reason=(
            "[ACCOUNT_BLOCK] Authentication token has been invalidated. Please sign in again."
        ),
    )

    monkeypatch.setattr(
        module, "object_session", lambda _key: (_ for _ in ()).throw(RuntimeError())
    )
    monkeypatch.setattr(module.crypto_service, "encrypt", lambda value: f"enc:{value}")

    module._persist_refreshed_token(key, "new-token", {"refresh_token": "rt-2"})

    assert key.api_key == "enc:new-token"
    assert key.auth_config == 'enc:{"refresh_token": "rt-2"}'
    assert key.oauth_invalid_at is None
    assert key.oauth_invalid_reason is None


@pytest.mark.asyncio
async def test_refresh_generic_oauth_token_persists_enriched_account_name(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    key = SimpleNamespace(id="key-1")
    endpoint = SimpleNamespace()
    template = SimpleNamespace(
        oauth=SimpleNamespace(
            token_url="https://example.com/oauth/token",
            client_id="client-id",
            client_secret=None,
            scopes=[],
        )
    )
    persisted: dict[str, Any] = {}

    async def _fake_post_oauth_token(**_kwargs: Any) -> httpx.Response:
        return httpx.Response(
            200,
            json={
                "access_token": "new-token",
                "refresh_token": "rt-2",
                "expires_in": 3600,
                "token_type": "Bearer",
            },
            request=httpx.Request("POST", "https://example.com/oauth/token"),
        )

    async def _fake_enrich_auth_config(**kwargs: Any) -> dict[str, Any]:
        auth_config = dict(kwargs["auth_config"])
        auth_config["account_name"] = "Workspace Alpha"
        return auth_config

    monkeypatch.setattr(module, "_get_proxy_config", lambda *_args: None)
    monkeypatch.setattr(module, "post_oauth_token", _fake_post_oauth_token)
    monkeypatch.setattr(module, "enrich_auth_config", _fake_enrich_auth_config)
    monkeypatch.setattr(
        module,
        "_persist_refreshed_token",
        lambda _key, _access_token, token_meta: persisted.update(
            {"access_token": _access_token, "token_meta": dict(token_meta)}
        ),
    )

    token_meta = {
        "provider_type": "codex",
        "refresh_token": "rt-1",
    }

    refreshed = await module._refresh_generic_oauth_token(
        key,
        endpoint,
        template,
        "codex",
        "rt-1",
        token_meta,
    )

    assert refreshed["account_name"] == "Workspace Alpha"
    assert persisted["access_token"] == "new-token"
    assert persisted["token_meta"]["account_name"] == "Workspace Alpha"
