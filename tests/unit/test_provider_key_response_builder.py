from __future__ import annotations

from datetime import datetime, timezone

import pytest

from src.models.database import ProviderAPIKey
from src.services.provider_keys import response_builder as module
from src.services.provider_keys.response_builder import build_key_response


def test_build_key_response_includes_codex_identity_metadata(
    monkeypatch: "pytest.MonkeyPatch",
) -> None:
    key = ProviderAPIKey(
        id="key-1",
        provider_id="provider-1",
        api_formats=["openai:chat"],
        auth_type="oauth",
        api_key="enc-access-token",
        auth_config='{"email":"u@example.com","plan_type":"team","account_id":"acc-1","account_name":"Workspace Alpha","account_user_id":"user-1__acc-1","organizations":[{"id":"org-1","title":"Personal","is_default":true,"role":"owner"}],"expires_at":2100000000}',
        name="codex-user",
    )
    now = datetime.now(timezone.utc)
    key.success_count = 0
    key.request_count = 0
    key.error_count = 0
    key.total_response_time_ms = 0
    key.rpm_limit = None
    key.global_priority_by_format = None
    key.allowed_models = None
    key.capabilities = None
    key.is_active = True
    key.created_at = now
    key.updated_at = now
    key.cache_ttl_minutes = 5
    key.max_probe_interval_minutes = 32
    key.health_by_format = None
    key.circuit_breaker_by_format = None
    key.oauth_invalid_at = None
    key.oauth_invalid_reason = None
    key.note = None
    key.last_used_at = None

    monkeypatch.setattr(module.crypto_service, "decrypt", lambda value: value)

    result = build_key_response(key)

    assert result.oauth_email == "u@example.com"
    assert result.oauth_plan_type == "team"
    assert result.oauth_account_id == "acc-1"
    assert result.oauth_account_name == "Workspace Alpha"
    assert result.oauth_account_user_id == "user-1__acc-1"
    assert len(result.oauth_organizations) == 1
    assert result.oauth_organizations[0].title == "Personal"
    assert result.oauth_organizations[0].is_default is True
    assert result.status_snapshot.oauth.code == "valid"
    assert result.status_snapshot.oauth.expires_at == 2100000000
    assert result.status_snapshot.account.code == "ok"


def test_build_key_response_prefers_persisted_status_snapshot_layers(
    monkeypatch: "pytest.MonkeyPatch",
) -> None:
    key = ProviderAPIKey(
        id="key-2",
        provider_id="provider-1",
        api_formats=["openai:chat"],
        auth_type="oauth",
        api_key="enc-access-token",
        auth_config='{"expires_at":100}',
        name="codex-user",
        status_snapshot={
            "oauth": {"code": "valid", "label": "有效", "expires_at": 100},
            "account": {
                "code": "workspace_deactivated",
                "label": "工作区停用",
                "reason": "persisted",
                "blocked": True,
            },
            "quota": {
                "code": "exhausted",
                "label": "额度耗尽",
                "reason": "persisted quota",
                "exhausted": True,
            },
        },
    )
    now = datetime.now(timezone.utc)
    key.success_count = 0
    key.request_count = 0
    key.error_count = 0
    key.total_response_time_ms = 0
    key.rpm_limit = None
    key.global_priority_by_format = None
    key.allowed_models = None
    key.capabilities = None
    key.is_active = True
    key.created_at = now
    key.updated_at = now
    key.cache_ttl_minutes = 5
    key.max_probe_interval_minutes = 32
    key.health_by_format = None
    key.circuit_breaker_by_format = None
    key.oauth_invalid_at = None
    key.oauth_invalid_reason = None
    key.note = None
    key.last_used_at = None

    monkeypatch.setattr(module.crypto_service, "decrypt", lambda value: value)

    result = build_key_response(key)

    assert result.status_snapshot.oauth.code == "expired"
    assert result.status_snapshot.account.code == "workspace_deactivated"
    assert result.status_snapshot.quota.code == "exhausted"


def test_build_key_response_prefers_realtime_plan_type_over_stale_auth_config(
    monkeypatch: "pytest.MonkeyPatch",
) -> None:
    key = ProviderAPIKey(
        id="key-3",
        provider_id="provider-1",
        api_formats=["openai:cli"],
        auth_type="oauth",
        api_key="enc-access-token",
        auth_config='{"email":"u@example.com","plan_type":"free","expires_at":2100000000}',
        upstream_metadata={"codex": {"plan_type": "plus"}},
        name="codex-user",
    )
    key.oauth_plan_type = "free"
    now = datetime.now(timezone.utc)
    key.success_count = 0
    key.request_count = 0
    key.error_count = 0
    key.total_response_time_ms = 0
    key.rpm_limit = None
    key.global_priority_by_format = None
    key.allowed_models = None
    key.capabilities = None
    key.is_active = True
    key.created_at = now
    key.updated_at = now
    key.cache_ttl_minutes = 5
    key.max_probe_interval_minutes = 32
    key.health_by_format = None
    key.circuit_breaker_by_format = None
    key.oauth_invalid_at = None
    key.oauth_invalid_reason = None
    key.note = None
    key.last_used_at = None

    monkeypatch.setattr(module.crypto_service, "decrypt", lambda value: value)

    result = build_key_response(key, provider_type="codex")

    assert result.oauth_plan_type == "plus"
