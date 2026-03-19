from __future__ import annotations

import json
import sys
import types
from types import SimpleNamespace
from typing import Any, cast

import pytest

from src.services.provider_keys.codex_usage_parser import (
    CodexUsageParseError,
    parse_codex_wham_usage_response,
)
from src.services.provider_keys.quota_refresh.antigravity_refresher import (
    refresh_antigravity_key_quota,
)
from src.services.provider_keys.quota_refresh.codex_refresher import refresh_codex_key_quota
from src.services.provider_keys.quota_refresh.kiro_refresher import refresh_kiro_key_quota


class _FakeDB:
    def __init__(self) -> None:
        self.commit_count = 0

    def commit(self) -> None:
        self.commit_count += 1


class _FakeResponse:
    def __init__(
        self,
        status_code: int,
        payload: Any = None,
        json_exc: Exception | None = None,
        *,
        headers: dict[str, str] | None = None,
        text: str | None = None,
    ) -> None:
        self.status_code = status_code
        self._payload = payload
        self._json_exc = json_exc
        self.headers = headers or {}
        if text is not None:
            self.text = text
        elif isinstance(payload, (dict, list)):
            self.text = json.dumps(payload, ensure_ascii=False)
        else:
            self.text = ""

    def json(self) -> Any:
        if self._json_exc:
            raise self._json_exc
        return self._payload


class _FakeAsyncClient:
    def __init__(self, response: _FakeResponse, **kwargs: Any) -> None:
        self._response = response
        self.kwargs = kwargs
        self.last_url: str | None = None
        self.last_headers: dict[str, str] | None = None

    async def __aenter__(self) -> "_FakeAsyncClient":
        return self

    async def __aexit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        tb: Any,
    ) -> bool:
        _ = exc_type, exc, tb
        return False

    async def get(self, url: str, headers: dict[str, str]) -> _FakeResponse:
        self.last_url = url
        self.last_headers = headers
        return self._response


def _install_module(monkeypatch: pytest.MonkeyPatch, name: str, attrs: dict[str, Any]) -> None:
    module = types.ModuleType(name)
    for key, value in attrs.items():
        setattr(module, key, value)
    monkeypatch.setitem(sys.modules, name, module)


@pytest.mark.asyncio
async def test_codex_refresher_endpoint_missing_returns_error() -> None:
    key = SimpleNamespace(id="k1", name="K1")
    provider = SimpleNamespace(proxy=None)

    result = await refresh_codex_key_quota(
        db=cast(Any, _FakeDB()),
        provider=cast(Any, provider),
        key=cast(Any, key),
        endpoint=None,
        codex_wham_usage_url="https://example.test",
        metadata_updates={},
        state_updates={},
    )

    assert result["status"] == "error"
    assert "openai:cli" in result["message"]


@pytest.mark.asyncio
async def test_codex_refresher_http_non_200_returns_error(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from src.services.provider_keys.quota_refresh import codex_refresher as module

    key = SimpleNamespace(
        id="k1", name="K1", api_key="enc", auth_type="api_key", auth_config=None, proxy=None
    )
    provider = SimpleNamespace(proxy=None)
    endpoint = SimpleNamespace()

    async def _fake_auth_info(_endpoint: Any, _key: Any) -> Any:
        return None

    _install_module(
        monkeypatch,
        "src.services.proxy_node.resolver",
        {
            "resolve_effective_proxy": lambda provider_proxy, key_proxy: None,
            "build_proxy_client_kwargs": lambda proxy, timeout: {"timeout": timeout},
        },
    )
    monkeypatch.setattr(module, "get_provider_auth", _fake_auth_info)
    monkeypatch.setattr(module.crypto_service, "decrypt", lambda _v: "sk-test")
    response = _FakeResponse(status_code=503, payload={"x": 1})
    monkeypatch.setattr(
        module.httpx, "AsyncClient", lambda **kwargs: _FakeAsyncClient(response, **kwargs)
    )

    result = await refresh_codex_key_quota(
        db=cast(Any, _FakeDB()),
        provider=cast(Any, provider),
        key=cast(Any, key),
        endpoint=cast(Any, endpoint),
        codex_wham_usage_url="https://example.test",
        metadata_updates={},
        state_updates={},
    )

    assert result["status"] == "error"
    assert result["status_code"] == 503


@pytest.mark.asyncio
async def test_codex_refresher_http_401_marks_auth_invalid_without_disabling_key(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from src.services.provider_keys.quota_refresh import codex_refresher as module

    key = SimpleNamespace(
        id="k1", name="K1", api_key="enc", auth_type="api_key", auth_config=None, proxy=None
    )
    provider = SimpleNamespace(proxy=None)
    endpoint = SimpleNamespace()
    metadata_updates: dict[str, dict[str, Any]] = {}
    state_updates: dict[str, dict[str, Any]] = {}

    async def _fake_auth_info(_endpoint: Any, _key: Any) -> Any:
        return None

    _install_module(
        monkeypatch,
        "src.services.proxy_node.resolver",
        {
            "resolve_effective_proxy": lambda provider_proxy, key_proxy: None,
            "build_proxy_client_kwargs": lambda proxy, timeout: {"timeout": timeout},
        },
    )
    monkeypatch.setattr(module, "get_provider_auth", _fake_auth_info)
    monkeypatch.setattr(module.crypto_service, "decrypt", lambda _v: "sk-test")
    response = _FakeResponse(status_code=401, payload={"error": {"message": "token expired"}})
    monkeypatch.setattr(
        module.httpx, "AsyncClient", lambda **kwargs: _FakeAsyncClient(response, **kwargs)
    )

    result = await refresh_codex_key_quota(
        db=cast(Any, _FakeDB()),
        provider=cast(Any, provider),
        key=cast(Any, key),
        endpoint=cast(Any, endpoint),
        codex_wham_usage_url="https://example.test",
        metadata_updates=metadata_updates,
        state_updates=state_updates,
    )

    assert result["status"] == "auth_invalid"
    assert result["status_code"] == 401
    assert result["auto_disabled"] is False
    assert metadata_updates == {}
    assert str(state_updates["k1"]["oauth_invalid_reason"]).startswith("[OAUTH_EXPIRED]")


@pytest.mark.asyncio
async def test_codex_refresher_http_402_sets_quota_exhausted_metadata(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from src.services.provider_keys.quota_refresh import codex_refresher as module

    key = SimpleNamespace(
        id="k1",
        name="K1",
        api_key="enc-key",
        auth_type="oauth",
        auth_config="enc-config",
        proxy=None,
    )
    provider = SimpleNamespace(proxy=None)
    endpoint = SimpleNamespace()
    metadata_updates: dict[str, dict[str, Any]] = {}
    state_updates: dict[str, dict[str, Any]] = {}

    async def _fake_auth_info(_endpoint: Any, _key: Any) -> Any:
        return None

    _install_module(
        monkeypatch,
        "src.services.proxy_node.resolver",
        {
            "resolve_effective_proxy": lambda provider_proxy, key_proxy: None,
            "build_proxy_client_kwargs": lambda proxy, timeout: {"timeout": timeout},
        },
    )
    monkeypatch.setattr(module, "get_provider_auth", _fake_auth_info)
    monkeypatch.setattr(
        module.crypto_service,
        "decrypt",
        lambda value: (
            "sk-test"
            if value == "enc-key"
            else json.dumps({"plan_type": "team", "account_id": "acc-1"})
        ),
    )
    response = _FakeResponse(status_code=402, payload={"error": {"message": "payment required"}})
    monkeypatch.setattr(
        module.httpx, "AsyncClient", lambda **kwargs: _FakeAsyncClient(response, **kwargs)
    )

    result = await refresh_codex_key_quota(
        db=cast(Any, _FakeDB()),
        provider=cast(Any, provider),
        key=cast(Any, key),
        endpoint=cast(Any, endpoint),
        codex_wham_usage_url="https://example.test",
        metadata_updates=metadata_updates,
        state_updates=state_updates,
    )

    assert result["status"] == "quota_exhausted"
    assert result["status_code"] == 402
    codex_meta = metadata_updates["k1"]["codex"]
    assert codex_meta["plan_type"] == "team"
    assert codex_meta["primary_used_percent"] == 100.0
    assert codex_meta["secondary_used_percent"] == 100.0
    assert state_updates["k1"]["oauth_invalid_at"] is None
    assert state_updates["k1"]["oauth_invalid_reason"] is None


@pytest.mark.asyncio
async def test_codex_refresher_http_403_token_invalidated_marks_oauth_expired(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from src.services.provider_keys.quota_refresh import codex_refresher as module

    key = SimpleNamespace(
        id="k1", name="K1", api_key="enc", auth_type="api_key", auth_config=None, proxy=None
    )
    provider = SimpleNamespace(proxy=None)
    endpoint = SimpleNamespace()
    metadata_updates: dict[str, dict[str, Any]] = {}
    state_updates: dict[str, dict[str, Any]] = {}

    async def _fake_auth_info(_endpoint: Any, _key: Any) -> Any:
        return None

    _install_module(
        monkeypatch,
        "src.services.proxy_node.resolver",
        {
            "resolve_effective_proxy": lambda provider_proxy, key_proxy: None,
            "build_proxy_client_kwargs": lambda proxy, timeout: {"timeout": timeout},
        },
    )
    monkeypatch.setattr(module, "get_provider_auth", _fake_auth_info)
    monkeypatch.setattr(module.crypto_service, "decrypt", lambda _v: "sk-test")
    response = _FakeResponse(
        status_code=403,
        payload={"error": {"message": "Authentication token has been invalidated."}},
    )
    monkeypatch.setattr(
        module.httpx, "AsyncClient", lambda **kwargs: _FakeAsyncClient(response, **kwargs)
    )

    result = await refresh_codex_key_quota(
        db=cast(Any, _FakeDB()),
        provider=cast(Any, provider),
        key=cast(Any, key),
        endpoint=cast(Any, endpoint),
        codex_wham_usage_url="https://example.test",
        metadata_updates=metadata_updates,
        state_updates=state_updates,
    )

    assert result["status"] == "forbidden"
    assert result["status_code"] == 403
    assert result["auto_disabled"] is False
    assert str(state_updates["k1"]["oauth_invalid_reason"]).startswith("[OAUTH_EXPIRED]")


@pytest.mark.asyncio
async def test_codex_refresher_http_403_generic_marks_soft_request_failed(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from src.services.provider_keys.quota_refresh import codex_refresher as module

    key = SimpleNamespace(
        id="k1", name="K1", api_key="enc", auth_type="api_key", auth_config=None, proxy=None
    )
    provider = SimpleNamespace(proxy=None)
    endpoint = SimpleNamespace()
    metadata_updates: dict[str, dict[str, Any]] = {}
    state_updates: dict[str, dict[str, Any]] = {}

    async def _fake_auth_info(_endpoint: Any, _key: Any) -> Any:
        return None

    _install_module(
        monkeypatch,
        "src.services.proxy_node.resolver",
        {
            "resolve_effective_proxy": lambda provider_proxy, key_proxy: None,
            "build_proxy_client_kwargs": lambda proxy, timeout: {"timeout": timeout},
        },
    )
    monkeypatch.setattr(module, "get_provider_auth", _fake_auth_info)
    monkeypatch.setattr(module.crypto_service, "decrypt", lambda _v: "sk-test")
    response = _FakeResponse(
        status_code=403,
        payload={"error": {"message": "Access forbidden for this account."}},
    )
    monkeypatch.setattr(
        module.httpx, "AsyncClient", lambda **kwargs: _FakeAsyncClient(response, **kwargs)
    )

    result = await refresh_codex_key_quota(
        db=cast(Any, _FakeDB()),
        provider=cast(Any, provider),
        key=cast(Any, key),
        endpoint=cast(Any, endpoint),
        codex_wham_usage_url="https://example.test",
        metadata_updates=metadata_updates,
        state_updates=state_updates,
    )

    assert result["status"] == "forbidden"
    assert result["status_code"] == 403
    assert result["auto_disabled"] is False
    assert str(state_updates["k1"]["oauth_invalid_reason"]).startswith("[REQUEST_FAILED]")


@pytest.mark.asyncio
async def test_codex_refresher_success_updates_metadata(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from src.services.provider_keys.quota_refresh import codex_refresher as module

    key = SimpleNamespace(
        id="k1",
        name="K1",
        api_key="enc",
        auth_type="api_key",
        auth_config=None,
        proxy=None,
        oauth_invalid_at="old",
        oauth_invalid_reason="old",
    )
    provider = SimpleNamespace(proxy=None)
    endpoint = SimpleNamespace()
    metadata_updates: dict[str, dict[str, Any]] = {}
    state_updates: dict[str, dict[str, Any]] = {}

    async def _fake_auth_info(_endpoint: Any, _key: Any) -> Any:
        return None

    _install_module(
        monkeypatch,
        "src.services.proxy_node.resolver",
        {
            "resolve_effective_proxy": lambda provider_proxy, key_proxy: None,
            "build_proxy_client_kwargs": lambda proxy, timeout: {"timeout": timeout},
        },
    )
    monkeypatch.setattr(module, "get_provider_auth", _fake_auth_info)
    monkeypatch.setattr(module.crypto_service, "decrypt", lambda _v: "sk-test")
    monkeypatch.setattr(
        module, "parse_codex_wham_usage_response", lambda _data: {"used_percent": 10.0}
    )
    response = _FakeResponse(status_code=200, payload={"ok": True})
    monkeypatch.setattr(
        module.httpx, "AsyncClient", lambda **kwargs: _FakeAsyncClient(response, **kwargs)
    )

    result = await refresh_codex_key_quota(
        db=cast(Any, _FakeDB()),
        provider=cast(Any, provider),
        key=cast(Any, key),
        endpoint=cast(Any, endpoint),
        codex_wham_usage_url="https://example.test",
        metadata_updates=metadata_updates,
        state_updates=state_updates,
    )

    assert result["status"] == "success"
    assert metadata_updates == {"k1": {"codex": {"used_percent": 10.0}}}
    assert state_updates == {"k1": {"oauth_invalid_at": None, "oauth_invalid_reason": None}}


@pytest.mark.asyncio
async def test_codex_refresher_parse_error_is_diagnostic(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from src.services.provider_keys.quota_refresh import codex_refresher as module

    key = SimpleNamespace(
        id="k1", name="K1", api_key="enc", auth_type="api_key", auth_config=None, proxy=None
    )
    provider = SimpleNamespace(proxy=None)
    endpoint = SimpleNamespace()

    async def _fake_auth_info(_endpoint: Any, _key: Any) -> Any:
        return None

    _install_module(
        monkeypatch,
        "src.services.proxy_node.resolver",
        {
            "resolve_effective_proxy": lambda provider_proxy, key_proxy: None,
            "build_proxy_client_kwargs": lambda proxy, timeout: {"timeout": timeout},
        },
    )
    monkeypatch.setattr(module, "get_provider_auth", _fake_auth_info)
    monkeypatch.setattr(module.crypto_service, "decrypt", lambda _v: "sk-test")
    monkeypatch.setattr(
        module,
        "parse_codex_wham_usage_response",
        lambda _data: (_ for _ in ()).throw(
            CodexUsageParseError("rate_limit.primary_window 类型错误")
        ),
    )
    response = _FakeResponse(status_code=200, payload={"ok": True})
    monkeypatch.setattr(
        module.httpx, "AsyncClient", lambda **kwargs: _FakeAsyncClient(response, **kwargs)
    )

    result = await refresh_codex_key_quota(
        db=cast(Any, _FakeDB()),
        provider=cast(Any, provider),
        key=cast(Any, key),
        endpoint=cast(Any, endpoint),
        codex_wham_usage_url="https://example.test",
        metadata_updates={},
        state_updates={},
    )

    assert result["status"] == "error"
    assert "响应结构异常" in result["message"]
    assert "rate_limit.primary_window 类型错误" in result["message"]


@pytest.mark.asyncio
async def test_codex_refresher_oauth_missing_plan_type_adds_account_header(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from src.services.provider_keys.quota_refresh import codex_refresher as module

    key = SimpleNamespace(
        id="k1",
        name="K1",
        api_key="enc",
        auth_type="oauth",
        auth_config="enc-config",
        proxy=None,
    )
    provider = SimpleNamespace(proxy=None)
    endpoint = SimpleNamespace()
    response = _FakeResponse(status_code=200, payload={"ok": True})
    client_ref: dict[str, _FakeAsyncClient] = {}

    async def _fake_auth_info(_endpoint: Any, _key: Any) -> Any:
        return SimpleNamespace(auth_header="Authorization", auth_value="Bearer oauth-token")

    _install_module(
        monkeypatch,
        "src.services.proxy_node.resolver",
        {
            "resolve_effective_proxy": lambda provider_proxy, key_proxy: None,
            "build_proxy_client_kwargs": lambda proxy, timeout: {"timeout": timeout},
        },
    )
    monkeypatch.setattr(module, "get_provider_auth", _fake_auth_info)
    monkeypatch.setattr(
        module, "parse_codex_wham_usage_response", lambda _data: {"used_percent": 1.0}
    )
    monkeypatch.setattr(
        module.crypto_service, "decrypt", lambda _v: json.dumps({"account_id": "acc-1"})
    )

    def _client_factory(**kwargs: Any) -> _FakeAsyncClient:
        client = _FakeAsyncClient(response, **kwargs)
        client_ref["client"] = client
        return client

    monkeypatch.setattr(module.httpx, "AsyncClient", _client_factory)

    result = await refresh_codex_key_quota(
        db=cast(Any, _FakeDB()),
        provider=cast(Any, provider),
        key=cast(Any, key),
        endpoint=cast(Any, endpoint),
        codex_wham_usage_url="https://example.test",
        metadata_updates={},
        state_updates={},
    )

    assert result["status"] == "success"
    assert client_ref["client"].last_headers is not None
    assert client_ref["client"].last_headers.get("chatgpt-account-id") == "acc-1"


@pytest.mark.asyncio
async def test_codex_refresher_oauth_uppercase_free_does_not_add_account_header(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from src.services.provider_keys.quota_refresh import codex_refresher as module

    key = SimpleNamespace(
        id="k1",
        name="K1",
        api_key="enc",
        auth_type="oauth",
        auth_config="enc-config",
        proxy=None,
    )
    provider = SimpleNamespace(proxy=None)
    endpoint = SimpleNamespace()
    response = _FakeResponse(status_code=200, payload={"ok": True})
    client_ref: dict[str, _FakeAsyncClient] = {}

    async def _fake_auth_info(_endpoint: Any, _key: Any) -> Any:
        return SimpleNamespace(auth_header="Authorization", auth_value="Bearer oauth-token")

    _install_module(
        monkeypatch,
        "src.services.proxy_node.resolver",
        {
            "resolve_effective_proxy": lambda provider_proxy, key_proxy: None,
            "build_proxy_client_kwargs": lambda proxy, timeout: {"timeout": timeout},
        },
    )
    monkeypatch.setattr(module, "get_provider_auth", _fake_auth_info)
    monkeypatch.setattr(
        module, "parse_codex_wham_usage_response", lambda _data: {"used_percent": 1.0}
    )
    monkeypatch.setattr(
        module.crypto_service,
        "decrypt",
        lambda _v: json.dumps({"account_id": "acc-1", "plan_type": "FREE"}),
    )

    def _client_factory(**kwargs: Any) -> _FakeAsyncClient:
        client = _FakeAsyncClient(response, **kwargs)
        client_ref["client"] = client
        return client

    monkeypatch.setattr(module.httpx, "AsyncClient", _client_factory)

    result = await refresh_codex_key_quota(
        db=cast(Any, _FakeDB()),
        provider=cast(Any, provider),
        key=cast(Any, key),
        endpoint=cast(Any, endpoint),
        codex_wham_usage_url="https://example.test",
        metadata_updates={},
        state_updates={},
    )

    assert result["status"] == "success"
    assert client_ref["client"].last_headers is not None
    assert "chatgpt-account-id" not in client_ref["client"].last_headers


def test_parse_codex_usage_plan_type_case_insensitive_free_window_semantics() -> None:
    parsed = parse_codex_wham_usage_response(
        {
            "plan_type": "FREE",
            "rate_limit": {
                "primary_window": {
                    "used_percent": "12.5",
                    "reset_after_seconds": "120",
                    "reset_at": "1700000000",
                    "limit_window_seconds": "604800",
                }
            },
        }
    )
    assert parsed is not None
    assert parsed["plan_type"] == "free"
    assert parsed["primary_used_percent"] == 12.5
    assert parsed["primary_window_minutes"] == 10080
    assert "secondary_used_percent" not in parsed


def test_parse_codex_usage_missing_plan_type_infers_paid_windows() -> None:
    parsed = parse_codex_wham_usage_response(
        {
            "rate_limit": {
                "primary_window": {
                    "used_percent": 25,
                    "reset_after_seconds": 600,
                    "reset_at": 1700000000,
                    "limit_window_seconds": 18000,
                },
                "secondary_window": {
                    "used_percent": 80,
                    "reset_after_seconds": 3600,
                    "reset_at": 1700003600,
                    "limit_window_seconds": 604800,
                },
            }
        }
    )
    assert parsed is not None
    assert parsed["primary_used_percent"] == 80.0
    assert parsed["secondary_used_percent"] == 25.0
    assert parsed["primary_window_minutes"] == 10080
    assert parsed["secondary_window_minutes"] == 300


@pytest.mark.parametrize("plan_type", ["plus", "enterprise"])
def test_parse_codex_usage_paid_plan_windows_mapping(plan_type: str) -> None:
    parsed = parse_codex_wham_usage_response(
        {
            "plan_type": plan_type,
            "rate_limit": {
                "primary_window": {
                    "used_percent": 25,
                    "reset_after_seconds": 600,
                    "reset_at": 1700000000,
                    "limit_window_seconds": 18000,
                },
                "secondary_window": {
                    "used_percent": 80,
                    "reset_after_seconds": 3600,
                    "reset_at": 1700003600,
                    "limit_window_seconds": 604800,
                },
            },
        }
    )
    assert parsed is not None
    assert parsed["plan_type"] == plan_type
    assert parsed["primary_used_percent"] == 80.0
    assert parsed["secondary_used_percent"] == 25.0
    assert parsed["primary_window_minutes"] == 10080
    assert parsed["secondary_window_minutes"] == 300


def test_parse_codex_usage_blank_credits_balance_is_ignored() -> None:
    parsed = parse_codex_wham_usage_response(
        {
            "plan_type": "team",
            "rate_limit": {
                "primary_window": {
                    "used_percent": 10,
                    "reset_after_seconds": 100,
                    "reset_at": 1700000000,
                    "limit_window_seconds": 18000,
                }
            },
            "credits": {
                "has_credits": False,
                "balance": "",
                "unlimited": "false",
            },
        }
    )
    assert parsed is not None
    assert parsed["has_credits"] is False
    assert parsed["credits_unlimited"] is False
    assert "credits_balance" not in parsed


def test_parse_codex_usage_invalid_type_raises_diagnostic_error() -> None:
    with pytest.raises(CodexUsageParseError, match="rate_limit.primary_window 类型错误"):
        parse_codex_wham_usage_response({"rate_limit": {"primary_window": []}})


@pytest.mark.asyncio
async def test_antigravity_refresher_forbidden_collects_updates_without_commit(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from src.services.provider_keys.quota_refresh import antigravity_refresher as module

    class _Forbidden(Exception):
        def __init__(self, reason: str) -> None:
            super().__init__(reason)
            self.reason = reason
            self.message = reason

    async def _fake_auth_info(_endpoint: Any, _key: Any) -> Any:
        return SimpleNamespace(auth_value="Bearer tk", decrypted_auth_config={"pid": "p1"})

    async def _fetch_models_for_key(_ctx: Any, timeout_seconds: float) -> Any:
        _ = timeout_seconds
        raise _Forbidden("forbidden-by-test")

    class _UpstreamModelsFetchContext:  # noqa: D101
        def __init__(self, **kwargs: Any) -> None:
            self.kwargs = kwargs

    _install_module(
        monkeypatch,
        "src.services.model.upstream_fetcher",
        {
            "UpstreamModelsFetchContext": _UpstreamModelsFetchContext,
            "fetch_models_for_key": _fetch_models_for_key,
        },
    )
    _install_module(
        monkeypatch,
        "src.services.provider.adapters.antigravity.client",
        {"AntigravityAccountForbiddenException": _Forbidden},
    )
    _install_module(
        monkeypatch,
        "src.services.proxy_node.resolver",
        {"resolve_effective_proxy": lambda provider_proxy, key_proxy: None},
    )
    monkeypatch.setattr(module, "get_provider_auth", _fake_auth_info)

    db = _FakeDB()
    provider = SimpleNamespace(proxy=None)
    key = SimpleNamespace(
        id="k1",
        name="K1",
        proxy=None,
        is_active=True,
        oauth_invalid_at=None,
        oauth_invalid_reason=None,
        upstream_metadata={},
    )
    endpoint = SimpleNamespace()
    metadata_updates: dict[str, dict[str, Any]] = {}
    state_updates: dict[str, dict[str, Any]] = {}

    result = await refresh_antigravity_key_quota(
        db=cast(Any, db),
        provider=cast(Any, provider),
        key=cast(Any, key),
        endpoint=cast(Any, endpoint),
        codex_wham_usage_url="https://example.test",
        metadata_updates=metadata_updates,
        state_updates=state_updates,
    )

    assert result["status"] == "forbidden"
    assert result["auto_disabled"] is True
    assert key.is_active is True
    assert key.oauth_invalid_reason is None
    assert state_updates["k1"]["is_active"] is False
    assert state_updates["k1"]["oauth_invalid_reason"].startswith("账户访问被禁止")
    assert metadata_updates["k1"]["antigravity"]["is_forbidden"] is True
    assert db.commit_count == 0


@pytest.mark.asyncio
async def test_antigravity_refresher_success_resets_forbidden_flag(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from src.services.provider_keys.quota_refresh import antigravity_refresher as module

    async def _fake_auth_info(_endpoint: Any, _key: Any) -> Any:
        return SimpleNamespace(auth_value="Bearer tk", decrypted_auth_config={})

    async def _fetch_models_for_key(_ctx: Any, timeout_seconds: float) -> Any:
        _ = timeout_seconds
        return (
            [],
            [],
            True,
            {"antigravity": {"is_forbidden": True, "forbidden_reason": "x", "forbidden_at": 1}},
        )

    class _UpstreamModelsFetchContext:  # noqa: D101
        def __init__(self, **kwargs: Any) -> None:
            self.kwargs = kwargs

    _install_module(
        monkeypatch,
        "src.services.model.upstream_fetcher",
        {
            "UpstreamModelsFetchContext": _UpstreamModelsFetchContext,
            "fetch_models_for_key": _fetch_models_for_key,
        },
    )
    _install_module(
        monkeypatch,
        "src.services.provider.adapters.antigravity.client",
        {"AntigravityAccountForbiddenException": RuntimeError},
    )
    _install_module(
        monkeypatch,
        "src.services.proxy_node.resolver",
        {"resolve_effective_proxy": lambda provider_proxy, key_proxy: None},
    )
    monkeypatch.setattr(module, "get_provider_auth", _fake_auth_info)

    metadata_updates: dict[str, dict[str, Any]] = {}
    state_updates: dict[str, dict[str, Any]] = {}
    result = await refresh_antigravity_key_quota(
        db=cast(Any, _FakeDB()),
        provider=cast(Any, SimpleNamespace(proxy=None)),
        key=cast(
            Any,
            SimpleNamespace(
                id="k1",
                name="K1",
                proxy=None,
                oauth_invalid_at="old",
                oauth_invalid_reason="old",
            ),
        ),
        endpoint=cast(Any, SimpleNamespace()),
        codex_wham_usage_url="https://example.test",
        metadata_updates=metadata_updates,
        state_updates=state_updates,
    )

    assert result["status"] == "success"
    assert metadata_updates["k1"]["antigravity"]["is_forbidden"] is False
    assert metadata_updates["k1"]["antigravity"]["forbidden_reason"] is None
    assert metadata_updates["k1"]["antigravity"]["forbidden_at"] is None
    assert state_updates["k1"]["oauth_invalid_at"] is None
    assert state_updates["k1"]["oauth_invalid_reason"] is None


@pytest.mark.asyncio
async def test_kiro_refresher_runtime_401_marks_key_invalid(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    class _Banned(Exception):
        pass

    async def _fetch_limits(auth_config: dict[str, Any], proxy_config: object) -> Any:
        _ = auth_config, proxy_config
        raise RuntimeError("401 token expired")

    def _parse_usage(_usage: Any) -> dict[str, Any]:
        return {"quota": 1}

    _install_module(
        monkeypatch,
        "src.services.provider.adapters.kiro.usage",
        {
            "KiroAccountBannedException": _Banned,
            "fetch_kiro_usage_limits": _fetch_limits,
            "parse_kiro_usage_response": _parse_usage,
        },
    )
    _install_module(
        monkeypatch,
        "src.services.proxy_node.resolver",
        {"resolve_effective_proxy": lambda provider_proxy, key_proxy: None},
    )
    monkeypatch.setattr(
        "src.services.provider_keys.quota_refresh.kiro_refresher.crypto_service.decrypt",
        lambda _v: "{}",
    )

    db = _FakeDB()
    key = SimpleNamespace(
        id="k1",
        name="K1",
        auth_config="enc",
        proxy=None,
        is_active=True,
        oauth_invalid_at=None,
        oauth_invalid_reason=None,
        upstream_metadata={},
    )
    state_updates: dict[str, dict[str, Any]] = {}

    result = await refresh_kiro_key_quota(
        db=cast(Any, db),
        provider=cast(Any, SimpleNamespace(proxy=None)),
        key=cast(Any, key),
        endpoint=None,
        codex_wham_usage_url="https://example.test",
        metadata_updates={},
        state_updates=state_updates,
    )

    assert result["status"] == "error"
    assert "401" in result["message"]
    assert key.is_active is True
    assert key.oauth_invalid_reason is None
    assert state_updates["k1"]["is_active"] is False
    assert state_updates["k1"]["oauth_invalid_reason"] == "Kiro Token 无效或已过期"
    assert db.commit_count == 0


@pytest.mark.asyncio
async def test_kiro_refresher_success_updates_metadata_and_auth_config(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    class _Banned(Exception):
        pass

    async def _fetch_limits(auth_config: dict[str, Any], proxy_config: object) -> Any:
        _ = auth_config, proxy_config
        return {"usage_data": {"x": 1}, "updated_auth_config": {"token": "new"}}

    def _parse_usage(_usage: Any) -> dict[str, Any]:
        return {"quota": 1}

    _install_module(
        monkeypatch,
        "src.services.provider.adapters.kiro.usage",
        {
            "KiroAccountBannedException": _Banned,
            "fetch_kiro_usage_limits": _fetch_limits,
            "parse_kiro_usage_response": _parse_usage,
        },
    )
    _install_module(
        monkeypatch,
        "src.services.proxy_node.resolver",
        {"resolve_effective_proxy": lambda provider_proxy, key_proxy: None},
    )
    monkeypatch.setattr(
        "src.services.provider_keys.quota_refresh.kiro_refresher.crypto_service.decrypt",
        lambda _v: json.dumps({"seed": 1}),
    )
    monkeypatch.setattr(
        "src.services.provider_keys.quota_refresh.kiro_refresher.crypto_service.encrypt",
        lambda raw: f"ENC:{raw}",
    )

    key = SimpleNamespace(
        id="k1",
        name="K1",
        auth_config="enc",
        proxy=None,
        oauth_invalid_at="old",
        oauth_invalid_reason="old",
        upstream_metadata={},
    )
    metadata_updates: dict[str, dict[str, Any]] = {}
    state_updates: dict[str, dict[str, Any]] = {}

    result = await refresh_kiro_key_quota(
        db=cast(Any, _FakeDB()),
        provider=cast(Any, SimpleNamespace(proxy=None)),
        key=cast(Any, key),
        endpoint=None,
        codex_wham_usage_url="https://example.test",
        metadata_updates=metadata_updates,
        state_updates=state_updates,
    )

    assert result["status"] == "success"
    assert metadata_updates["k1"]["kiro"]["is_banned"] is False
    assert metadata_updates["k1"]["kiro"]["quota"] == 1
    assert key.auth_config == "enc"
    assert state_updates["k1"]["oauth_invalid_at"] is None
    assert state_updates["k1"]["oauth_invalid_reason"] is None
    assert state_updates["k1"]["auth_config"].startswith("ENC:")


@pytest.mark.asyncio
async def test_codex_refresher_http_402_workspace_deactivated_marks_account_block(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from src.services.provider_keys.quota_refresh import codex_refresher as module

    key = SimpleNamespace(
        id="k1",
        name="K1",
        api_key="enc-key",
        auth_type="oauth",
        auth_config="enc-config",
        proxy=None,
    )
    provider = SimpleNamespace(proxy=None)
    endpoint = SimpleNamespace()
    metadata_updates: dict[str, dict[str, Any]] = {}
    state_updates: dict[str, dict[str, Any]] = {}

    async def _fake_auth_info(_endpoint: Any, _key: Any) -> Any:
        return None

    _install_module(
        monkeypatch,
        "src.services.proxy_node.resolver",
        {
            "resolve_effective_proxy": lambda provider_proxy, key_proxy: None,
            "build_proxy_client_kwargs": lambda proxy, timeout: {"timeout": timeout},
        },
    )
    monkeypatch.setattr(module, "get_provider_auth", _fake_auth_info)
    monkeypatch.setattr(
        module.crypto_service,
        "decrypt",
        lambda value: (
            "sk-test"
            if value == "enc-key"
            else json.dumps({"plan_type": "team", "account_id": "acc-1"})
        ),
    )
    response = _FakeResponse(status_code=402, payload={"detail": {"code": "deactivated_workspace"}})
    monkeypatch.setattr(
        module.httpx, "AsyncClient", lambda **kwargs: _FakeAsyncClient(response, **kwargs)
    )

    result = await refresh_codex_key_quota(
        db=cast(Any, _FakeDB()),
        provider=cast(Any, provider),
        key=cast(Any, key),
        endpoint=cast(Any, endpoint),
        codex_wham_usage_url="https://example.test",
        metadata_updates=metadata_updates,
        state_updates=state_updates,
    )

    assert result["status"] == "workspace_deactivated"
    assert result["status_code"] == 402
    assert metadata_updates["k1"]["codex"]["account_disabled"] is True
    assert metadata_updates["k1"]["codex"]["reason"] == "deactivated_workspace"
    assert state_updates["k1"]["oauth_invalid_at"] is not None
    assert str(state_updates["k1"]["oauth_invalid_reason"]).startswith("[ACCOUNT_BLOCK]")
