from __future__ import annotations

import json

import jwt
from contextlib import contextmanager
from types import SimpleNamespace
from unittest.mock import MagicMock

import pytest

from src.api.admin import provider_oauth as module
from src.core.exceptions import InvalidRequestException


def test_parse_standard_oauth_import_entries_keeps_codex_hints() -> None:
    entries = module._parse_standard_oauth_import_entries(
        '[{"refresh_token":"rt_1","accountId":"acc-1","chatgptAccountUserId":"u-1__acc-1","planType":"TEAM","userId":"u-1","email":"u@example.com"}]'
    )

    assert entries == [
        {
            "refresh_token": "rt_1",
            "account_id": "acc-1",
            "account_user_id": "u-1__acc-1",
            "plan_type": "team",
            "user_id": "u-1",
            "email": "u@example.com",
        }
    ]


def test_parse_tokens_input_compatibility_wrapper() -> None:
    tokens = module._parse_tokens_input("token_a\ntoken_b")
    assert tokens == ["token_a", "token_b"]


def test_apply_codex_import_hints_only_fills_missing_fields() -> None:
    auth_config = {
        "account_id": "existing-account",
        "plan_type": "",
    }
    module._apply_codex_import_hints(
        auth_config,
        {
            "account_user_id": "u-1__acc-1",
            "account_id": "acc-1",
            "plan_type": "plus",
            "user_id": "user-1",
            "email": "u@example.com",
        },
    )

    assert auth_config["account_id"] == "existing-account"
    assert auth_config["account_user_id"] == "u-1__acc-1"
    assert auth_config["plan_type"] == "plus"
    assert auth_config["user_id"] == "user-1"
    assert auth_config["email"] == "u@example.com"


class _DummyQuery:
    def __init__(self, keys: list[SimpleNamespace]) -> None:
        self._keys = keys

    def filter(self, *_args: object, **_kwargs: object) -> "_DummyQuery":
        return self

    def all(self) -> list[SimpleNamespace]:
        return self._keys


class _DummyDB:
    def __init__(self, keys: list[SimpleNamespace]) -> None:
        self._keys = keys

    def query(self, _model: object) -> _DummyQuery:
        return _DummyQuery(self._keys)


def _make_oauth_key(*, key_id: str, name: str, auth_config: dict[str, object]) -> SimpleNamespace:
    return SimpleNamespace(
        id=key_id,
        name=name,
        provider_id="provider-1",
        auth_type="oauth",
        auth_config=json.dumps(auth_config),
        is_active=True,
    )


class _SingleKeyQuery:
    def __init__(self, key: SimpleNamespace | None) -> None:
        self._key = key

    def filter(self, *_args: object, **_kwargs: object) -> "_SingleKeyQuery":
        return self

    def first(self) -> SimpleNamespace | None:
        return self._key


class _SingleKeyDB:
    def __init__(self, key: SimpleNamespace | None) -> None:
        self._key = key

    def query(self, _model: object) -> _SingleKeyQuery:
        return _SingleKeyQuery(self._key)


@contextmanager
def _fake_db_context(db: _SingleKeyDB):
    yield db


def test_check_duplicate_oauth_account_codex_allows_same_user_different_account_id(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(module.crypto_service, "decrypt", lambda value, silent=True: value)

    existing_key = _make_oauth_key(
        key_id="key-1",
        name="existing",
        auth_config={
            "provider_type": "codex",
            "email": "u@example.com",
            "user_id": "user-1",
            "account_id": "acc-1",
            "account_user_id": "user-1__acc-1",
            "plan_type": "team",
        },
    )
    db = _DummyDB([existing_key])

    result = module._check_duplicate_oauth_account(
        db,  # type: ignore[arg-type]
        "provider-1",
        {
            "provider_type": "codex",
            "email": "u@example.com",
            "user_id": "user-1",
            "account_id": "acc-2",
            "account_user_id": "user-1__acc-2",
            "plan_type": "team",
        },
    )

    assert result is None


def test_check_duplicate_oauth_account_codex_rejects_same_account_user_identity(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(module.crypto_service, "decrypt", lambda value, silent=True: value)

    existing_key = _make_oauth_key(
        key_id="key-1",
        name="existing",
        auth_config={
            "provider_type": "codex",
            "email": "u@example.com",
            "user_id": "user-1",
            "account_id": "acc-1",
            "account_user_id": "user-1__acc-1",
            "plan_type": "team",
        },
    )
    db = _DummyDB([existing_key])

    with pytest.raises(InvalidRequestException, match="已存在"):
        module._check_duplicate_oauth_account(
            db,  # type: ignore[arg-type]
            "provider-1",
            {
                "provider_type": "codex",
                "email": "u@example.com",
                "user_id": "user-1",
                "account_id": "acc-1",
                "account_user_id": "user-1__acc-1",
                "plan_type": "team",
            },
        )


def test_mark_refresh_failed_sync_preserves_existing_account_block(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    key = SimpleNamespace(
        id="key-1",
        oauth_invalid_at="old-invalid-at",
        oauth_invalid_reason="[ACCOUNT_BLOCK] account has been deactivated",
    )
    db = _SingleKeyDB(key)
    monkeypatch.setattr(module, "get_db_context", lambda: _fake_db_context(db))

    module._mark_refresh_failed_sync(
        "key-1",
        "[REFRESH_FAILED] Token 续期失败 (400): refresh_token_reused",
    )

    assert key.oauth_invalid_at == "old-invalid-at"
    assert key.oauth_invalid_reason == "[ACCOUNT_BLOCK] account has been deactivated"


@pytest.mark.asyncio
async def test_refresh_account_state_after_oauth_update_refreshes_supported_provider(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    fake_db = SimpleNamespace(close=MagicMock())
    captured: dict[str, object] = {}

    async def _fake_refresh_provider_quota_for_provider(**kwargs: object) -> dict[str, object]:
        captured.update(kwargs)
        return {"success": 1}

    monkeypatch.setattr(module, "create_session", lambda: fake_db)

    from src.services.provider_keys import key_quota_service as quota_module

    monkeypatch.setattr(
        quota_module,
        "refresh_provider_quota_for_provider",
        _fake_refresh_provider_quota_for_provider,
    )

    attempted, error = await module._refresh_account_state_after_oauth_update(
        provider_id="provider-1",
        provider_type="codex",
        key_ids=["key-1"],
    )

    assert attempted is True
    assert error is None
    assert captured["provider_id"] == "provider-1"
    assert captured["key_ids"] == ["key-1"]
    fake_db.close.assert_called_once()


@pytest.mark.asyncio
async def test_refresh_account_state_after_oauth_update_returns_error_when_refresh_fails(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    fake_db = SimpleNamespace(close=MagicMock())

    async def _fake_refresh_provider_quota_for_provider(**_kwargs: object) -> dict[str, object]:
        raise RuntimeError("quota refresh failed")

    monkeypatch.setattr(module, "create_session", lambda: fake_db)

    from src.services.provider_keys import key_quota_service as quota_module

    monkeypatch.setattr(
        quota_module,
        "refresh_provider_quota_for_provider",
        _fake_refresh_provider_quota_for_provider,
    )

    attempted, error = await module._refresh_account_state_after_oauth_update(
        provider_id="provider-1",
        provider_type="codex",
        key_ids=["key-1"],
    )

    assert attempted is True
    assert "quota refresh failed" in error
    fake_db.close.assert_called_once()


def test_parse_standard_oauth_import_entries_accepts_codex_access_token_only() -> None:
    entries = module._parse_standard_oauth_import_entries(
        '[{"access_token":"at_1","accountId":"acc-1","email":"u@example.com"}]'
    )

    assert entries == [
        {
            "access_token": "at_1",
            "account_id": "acc-1",
            "email": "u@example.com",
        }
    ]


def test_parse_standard_oauth_import_entries_keeps_refresh_and_access_token() -> None:
    entries = module._parse_standard_oauth_import_entries(
        '{"refreshToken":"rt_1","accessToken":"at_1","planType":"PLUS"}'
    )

    assert entries == [
        {
            "refresh_token": "rt_1",
            "access_token": "at_1",
            "plan_type": "plus",
        }
    ]


def test_decode_access_token_expires_at_reads_jwt_exp() -> None:
    token = jwt.encode({"exp": 2_000_000_000}, key="", algorithm="none")
    if isinstance(token, bytes):
        token = token.decode("utf-8")

    assert module._decode_access_token_expires_at(token) == 2_000_000_000


def test_normalize_single_import_tokens_treats_plain_jwt_as_access_token() -> None:
    token = jwt.encode(
        {
            "iss": "https://auth.openai.com",
            "aud": ["https://api.openai.com/v1"],
            "exp": 2_000_000_000,
            "scp": ["openid", "offline_access"],
        },
        key="",
        algorithm="none",
    )
    if isinstance(token, bytes):
        token = token.decode("utf-8")
    token = f"{token}signature"

    refresh_token, access_token = module._normalize_single_import_tokens(
        refresh_token=token,
        access_token=None,
    )

    assert refresh_token == ""
    assert access_token == token
