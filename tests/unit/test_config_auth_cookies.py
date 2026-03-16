from __future__ import annotations

import pytest

from src.config.settings import Config


def test_production_defaults_refresh_cookie_to_cross_site(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("ENVIRONMENT", "production")
    monkeypatch.delenv("AUTH_REFRESH_COOKIE_SAMESITE", raising=False)
    monkeypatch.delenv("AUTH_REFRESH_COOKIE_SECURE", raising=False)

    cfg = Config()

    assert cfg.auth_refresh_cookie_samesite == "none"
    assert cfg.auth_refresh_cookie_secure is True


def test_validate_security_config_rejects_insecure_none_cookie(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("ENVIRONMENT", "production")
    monkeypatch.setenv("AUTH_REFRESH_COOKIE_SAMESITE", "none")
    monkeypatch.setenv("AUTH_REFRESH_COOKIE_SECURE", "false")

    cfg = Config()

    assert (
        "AUTH_REFRESH_COOKIE_SECURE must be true when AUTH_REFRESH_COOKIE_SAMESITE=none."
        in cfg.validate_security_config()
    )


def test_validate_security_config_rejects_invalid_refresh_cookie_samesite(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("AUTH_REFRESH_COOKIE_SAMESITE", "invalid-value")

    cfg = Config()

    assert "AUTH_REFRESH_COOKIE_SAMESITE must be one of: lax, strict, none." in (
        cfg.validate_security_config()
    )
