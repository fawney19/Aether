from __future__ import annotations

from datetime import datetime, timezone
from types import SimpleNamespace

from src.api.admin.pool import routes as pool_routes


def test_derive_oauth_expires_at_from_auth_config_seconds(monkeypatch) -> None:
    key = SimpleNamespace(auth_type="oauth", auth_config="enc", expires_at=None)

    monkeypatch.setattr(
        pool_routes.crypto_service,
        "decrypt",
        lambda _v: '{"expires_at": 1710000000}',
    )

    assert pool_routes._derive_oauth_expires_at(key) == 1710000000


def test_derive_oauth_expires_at_from_auth_config_milliseconds(monkeypatch) -> None:
    key = SimpleNamespace(auth_type="oauth", auth_config="enc", expires_at=None)

    monkeypatch.setattr(
        pool_routes.crypto_service,
        "decrypt",
        lambda _v: '{"expires_at": 1710000000000}',
    )

    assert pool_routes._derive_oauth_expires_at(key) == 1710000000


def test_derive_oauth_expires_at_fallback_to_legacy_datetime() -> None:
    key = SimpleNamespace(
        auth_type="oauth",
        auth_config=None,
        expires_at=datetime(2026, 3, 4, 1, 2, 3, tzinfo=timezone.utc),
    )

    assert pool_routes._derive_oauth_expires_at(key) == 1772586123


def test_derive_oauth_account_name_from_auth_config() -> None:
    assert (
        pool_routes._derive_oauth_account_name({"account_name": " Workspace Alpha "})
        == "Workspace Alpha"
    )
