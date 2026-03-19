from __future__ import annotations

from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock
from urllib.parse import parse_qs, urlparse

import pytest

from src.core.enums import AuthSource
from src.services.auth.oauth.service import OAuthService
from src.services.auth.oauth.state import OAuthStateData


@pytest.mark.asyncio
async def test_build_bind_authorize_url_includes_client_device_id(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    user = SimpleNamespace(id="user-1", auth_source=AuthSource.LOCAL)
    provider = SimpleNamespace(
        get_authorization_url=MagicMock(return_value="https://provider.example/authorize")
    )
    config = SimpleNamespace()
    create_state = AsyncMock(return_value="state-1")

    monkeypatch.setattr(
        "src.services.auth.oauth.service.OAuthService._require_module_active",
        lambda _db: None,
    )
    monkeypatch.setattr(
        "src.services.auth.oauth.service.OAuthService._get_provider_impl",
        lambda _provider_type: provider,
    )
    monkeypatch.setattr(
        "src.services.auth.oauth.service.OAuthService._get_enabled_provider_config",
        lambda _db, _provider_type: config,
    )
    monkeypatch.setattr(
        "src.services.auth.oauth.service.get_redis_client",
        AsyncMock(return_value=object()),
    )
    monkeypatch.setattr(
        "src.services.auth.oauth.service.create_oauth_state",
        create_state,
    )

    url = await OAuthService.build_bind_authorize_url(
        db,
        user,
        "github",
        client_device_id="device-1",
    )

    assert url == "https://provider.example/authorize"
    assert create_state.await_args.kwargs["client_device_id"] == "device-1"


@pytest.mark.asyncio
async def test_handle_callback_allows_bind_state_without_device_id(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    provider = SimpleNamespace(
        exchange_code=AsyncMock(return_value=SimpleNamespace(access_token="provider-access")),
        get_user_info=AsyncMock(
            return_value=SimpleNamespace(
                id="oauth-user",
                username="tester",
                email="user@example.com",
                email_verified=True,
                raw={},
            )
        ),
    )
    config = SimpleNamespace(
        frontend_callback_url="https://app.example.com/auth/callback",
        display_name="GitHub",
        is_enabled=True,
    )

    monkeypatch.setattr(
        "src.services.auth.oauth.service.OAuthService._require_module_active",
        lambda _db: None,
    )
    monkeypatch.setattr(
        "src.services.auth.oauth.service.OAuthService._get_provider_impl",
        lambda _provider_type: provider,
    )
    monkeypatch.setattr(
        "src.services.auth.oauth.service.OAuthService._get_provider_config",
        lambda _db, _provider_type: config,
    )
    monkeypatch.setattr(
        "src.services.auth.oauth.service.get_redis_client",
        AsyncMock(return_value=object()),
    )
    monkeypatch.setattr(
        "src.services.auth.oauth.service.consume_oauth_state",
        AsyncMock(
            return_value=OAuthStateData(
                nonce="state-1",
                provider_type="github",
                action="bind",
                user_id="user-1",
                client_device_id=None,
                created_at=123,
            )
        ),
    )
    monkeypatch.setattr(
        "src.services.auth.oauth.service.OAuthService._handle_bind",
        AsyncMock(return_value=SimpleNamespace()),
    )

    result = await OAuthService.handle_callback(
        db=db,
        provider_type="github",
        state="state-1",
        code="code-1",
        error=None,
        error_description=None,
        client_ip=None,
        user_agent="pytest-agent",
        headers={},
    )

    parsed = urlparse(result.redirect_url)
    assert result.refresh_token is None
    assert parse_qs(parsed.query)["oauth_bound"] == ["GitHub"]


@pytest.mark.asyncio
async def test_handle_login_reloads_user_in_current_session(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = MagicMock()
    config = SimpleNamespace(provider_type="github")
    oauth_user = SimpleNamespace(id="oauth-user")
    user = SimpleNamespace(
        id="user-1",
        email="user@example.com",
        is_active=True,
        is_deleted=False,
    )
    invalidate_cache = AsyncMock()

    async def _fake_run_in_threadpool(func, provider_type, oauth_user_info):  # type: ignore[no-untyped-def]
        assert func is OAuthService._handle_login_sync
        assert provider_type == "github"
        assert oauth_user_info is oauth_user
        return "user-1"

    monkeypatch.setattr(
        "src.services.auth.oauth.service.run_in_threadpool",
        _fake_run_in_threadpool,
    )
    monkeypatch.setattr(
        "src.services.auth.oauth.service.UserCacheService.invalidate_user_cache",
        invalidate_cache,
    )

    db.query.return_value.filter.return_value.first.return_value = user

    result = await OAuthService._handle_login(db, config=config, oauth_user=oauth_user)

    assert result is user
    invalidate_cache.assert_awaited_once_with("user-1", "user@example.com")
