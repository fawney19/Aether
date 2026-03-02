from __future__ import annotations

from types import SimpleNamespace
from typing import Any, cast

import pytest

from src.core.exceptions import InvalidRequestException
from src.core.provider_types import ProviderType
from src.services.provider_keys import key_quota_service as quota_service_module
from src.services.provider_keys.key_quota_service import (
    _resolve_quota_refresh_handler,
    _select_refresh_endpoint,
    refresh_provider_quota_for_provider,
)
from src.services.provider_keys.quota_refresh.antigravity_refresher import (
    refresh_antigravity_key_quota,
)
from src.services.provider_keys.quota_refresh.codex_refresher import refresh_codex_key_quota
from src.services.provider_keys.quota_refresh.kiro_refresher import refresh_kiro_key_quota


def _provider_with_endpoints(*endpoints: tuple[str, bool]) -> SimpleNamespace:
    eps = [SimpleNamespace(api_format=fmt, is_active=active) for fmt, active in endpoints]
    return SimpleNamespace(endpoints=eps)


class _FakeQuery:
    def __init__(
        self,
        *,
        first_result: Any = None,
        all_result: list[Any] | None = None,
    ) -> None:
        self._first_result = first_result
        self._all_result = all_result or []

    def filter(self, *args: Any, **kwargs: Any) -> _FakeQuery:
        return self

    def first(self) -> Any:
        return self._first_result

    def all(self) -> list[Any]:
        return self._all_result


class _FakeDB:
    def __init__(self, *, provider: Any, keys: list[SimpleNamespace]) -> None:
        self._provider = provider
        self._keys = keys
        self.added: list[object] = []
        self.commit_count = 0

    def query(self, model: Any) -> _FakeQuery:
        model_name = getattr(model, "__name__", "")
        if model_name == "Provider":
            return _FakeQuery(first_result=self._provider)
        if model_name == "ProviderAPIKey":
            return _FakeQuery(all_result=self._keys)
        raise AssertionError(f"unexpected query model: {model}")

    def add(self, obj: object) -> None:
        self.added.append(obj)

    def commit(self) -> None:
        self.commit_count += 1


def test_select_refresh_endpoint_codex() -> None:
    provider = _provider_with_endpoints(("openai:chat", True), ("openai:cli", True))
    endpoint = _select_refresh_endpoint(cast(Any, provider), ProviderType.CODEX)
    assert endpoint is not None
    assert endpoint.api_format == "openai:cli"


def test_select_refresh_endpoint_codex_normalized_api_format() -> None:
    provider = _provider_with_endpoints(("openai:chat", True), (" OpenAI:CLI ", True))
    endpoint = _select_refresh_endpoint(cast(Any, provider), ProviderType.CODEX)
    assert endpoint is not None
    assert endpoint.api_format == " OpenAI:CLI "


def test_select_refresh_endpoint_antigravity_prefers_chat() -> None:
    provider = _provider_with_endpoints(("gemini:cli", True), ("gemini:chat", True))
    endpoint = _select_refresh_endpoint(cast(Any, provider), ProviderType.ANTIGRAVITY)
    assert endpoint is not None
    assert endpoint.api_format == "gemini:chat"


def test_select_refresh_endpoint_antigravity_fallback_cli() -> None:
    provider = _provider_with_endpoints(("gemini:chat", False), ("gemini:cli", True))
    endpoint = _select_refresh_endpoint(cast(Any, provider), ProviderType.ANTIGRAVITY)
    assert endpoint is not None
    assert endpoint.api_format == "gemini:cli"


def test_select_refresh_endpoint_kiro_returns_none() -> None:
    provider = _provider_with_endpoints(("openai:cli", True))
    endpoint = _select_refresh_endpoint(cast(Any, provider), ProviderType.KIRO)
    assert endpoint is None


def test_select_refresh_endpoint_codex_missing_raises() -> None:
    provider = _provider_with_endpoints(("openai:chat", True))
    with pytest.raises(InvalidRequestException, match="找不到有效的 openai:cli 端点"):
        _select_refresh_endpoint(cast(Any, provider), ProviderType.CODEX)


def test_select_refresh_endpoint_antigravity_missing_raises() -> None:
    provider = _provider_with_endpoints(("gemini:chat", False), ("gemini:cli", False))
    with pytest.raises(InvalidRequestException, match="找不到有效的 gemini:chat/gemini:cli 端点"):
        _select_refresh_endpoint(cast(Any, provider), ProviderType.ANTIGRAVITY)


def test_resolve_quota_refresh_handler() -> None:
    assert _resolve_quota_refresh_handler(ProviderType.CODEX) is refresh_codex_key_quota
    assert _resolve_quota_refresh_handler(ProviderType.ANTIGRAVITY) is refresh_antigravity_key_quota
    assert _resolve_quota_refresh_handler(ProviderType.KIRO) is refresh_kiro_key_quota


def test_resolve_quota_refresh_handler_unsupported_raises() -> None:
    with pytest.raises(
        InvalidRequestException,
        match="仅支持 Codex / Antigravity / Kiro 类型的 Provider 刷新限额",
    ):
        _resolve_quota_refresh_handler("unknown")


@pytest.mark.asyncio
async def test_refresh_provider_quota_no_active_keys_returns_empty() -> None:
    provider = SimpleNamespace(
        id="p1",
        provider_type=ProviderType.CODEX,
        endpoints=[SimpleNamespace(api_format="openai:cli", is_active=True)],
    )
    db = _FakeDB(provider=provider, keys=[])

    result = await refresh_provider_quota_for_provider(
        db=cast(Any, db),
        provider_id="p1",
        codex_wham_usage_url="https://example.test/wham/usage",
    )

    assert result == {
        "success": 0,
        "failed": 0,
        "total": 0,
        "results": [],
        "message": "没有活跃的 Key",
    }


@pytest.mark.asyncio
async def test_refresh_provider_quota_aggregates_and_merges_metadata(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    provider = SimpleNamespace(
        id="p1",
        provider_type=ProviderType.CODEX,
        endpoints=[SimpleNamespace(api_format="openai:cli", is_active=True)],
    )
    key1 = SimpleNamespace(id="k1", name="K1", upstream_metadata={"old": True})
    key2 = SimpleNamespace(id="k2", name="K2", upstream_metadata={})
    db = _FakeDB(provider=provider, keys=[key1, key2])

    async def _fake_handler(**kwargs: Any) -> dict[str, Any]:
        key = kwargs["key"]
        metadata_updates = kwargs["metadata_updates"]
        if key.id == "k1":
            metadata_updates[key.id] = {"codex": {"used": 10}}
            return {"key_id": key.id, "key_name": key.name, "status": "success"}
        return {"key_id": key.id, "key_name": key.name, "status": "error", "message": "boom"}

    monkeypatch.setattr(
        quota_service_module,
        "_select_refresh_endpoint",
        lambda provider, provider_type: provider.endpoints[0],
    )
    monkeypatch.setattr(
        quota_service_module, "_resolve_quota_refresh_handler", lambda _: _fake_handler
    )
    monkeypatch.setattr(
        quota_service_module,
        "merge_upstream_metadata",
        lambda current, updates: {**(current or {}), **updates},
    )

    result = await refresh_provider_quota_for_provider(
        db=cast(Any, db),
        provider_id="p1",
        codex_wham_usage_url="https://example.test/wham/usage",
    )

    assert result["success"] == 1
    assert result["failed"] == 1
    assert result["total"] == 2
    assert len(result["results"]) == 2
    assert key1.upstream_metadata == {"old": True, "codex": {"used": 10}}
    assert db.commit_count == 1
    assert db.added == [key1]


@pytest.mark.asyncio
async def test_refresh_provider_quota_applies_state_updates_and_single_commit(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    provider = SimpleNamespace(
        id="p1",
        provider_type=ProviderType.CODEX,
        endpoints=[SimpleNamespace(api_format="openai:cli", is_active=True)],
    )
    key1 = SimpleNamespace(
        id="k1",
        name="K1",
        upstream_metadata={},
        is_active=True,
        oauth_invalid_at="old",
        oauth_invalid_reason="old-reason",
    )
    key2 = SimpleNamespace(
        id="k2",
        name="K2",
        upstream_metadata={},
        is_active=True,
        oauth_invalid_at=None,
        oauth_invalid_reason=None,
    )
    db = _FakeDB(provider=provider, keys=[key1, key2])

    async def _fake_handler(**kwargs: Any) -> dict[str, Any]:
        key = kwargs["key"]
        state_updates = kwargs["state_updates"]
        if key.id == "k1":
            state_updates[key.id] = {"oauth_invalid_at": None, "oauth_invalid_reason": None}
            return {"key_id": key.id, "key_name": key.name, "status": "success"}
        state_updates[key.id] = {"is_active": False, "oauth_invalid_reason": "401"}
        return {"key_id": key.id, "key_name": key.name, "status": "error", "message": "401"}

    monkeypatch.setattr(
        quota_service_module,
        "_select_refresh_endpoint",
        lambda provider, provider_type: provider.endpoints[0],
    )
    monkeypatch.setattr(
        quota_service_module, "_resolve_quota_refresh_handler", lambda _: _fake_handler
    )

    result = await refresh_provider_quota_for_provider(
        db=cast(Any, db),
        provider_id="p1",
        codex_wham_usage_url="https://example.test/wham/usage",
    )

    assert result["success"] == 1
    assert result["failed"] == 1
    assert key1.oauth_invalid_at is None
    assert key1.oauth_invalid_reason is None
    assert key2.is_active is False
    assert key2.oauth_invalid_reason == "401"
    assert db.commit_count == 1
    assert db.added == [key1, key2]


@pytest.mark.asyncio
async def test_refresh_provider_quota_handler_exception_returns_error(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    provider = SimpleNamespace(
        id="p1",
        provider_type=ProviderType.CODEX,
        endpoints=[SimpleNamespace(api_format="openai:cli", is_active=True)],
    )
    key = SimpleNamespace(id="k1", name="K1", upstream_metadata={})
    db = _FakeDB(provider=provider, keys=[key])

    async def _boom_handler(**kwargs: Any) -> dict[str, Any]:
        _ = kwargs
        raise RuntimeError("unit-test boom")

    monkeypatch.setattr(
        quota_service_module,
        "_select_refresh_endpoint",
        lambda provider, provider_type: provider.endpoints[0],
    )
    monkeypatch.setattr(
        quota_service_module, "_resolve_quota_refresh_handler", lambda _: _boom_handler
    )

    result = await refresh_provider_quota_for_provider(
        db=cast(Any, db),
        provider_id="p1",
        codex_wham_usage_url="https://example.test/wham/usage",
    )

    assert result["success"] == 0
    assert result["failed"] == 1
    assert result["total"] == 1
    assert result["results"][0]["status"] == "error"
    assert "unit-test boom" in result["results"][0]["message"]


@pytest.mark.asyncio
async def test_refresh_provider_quota_ignores_quota_exhausted_flag_for_redis_cooldown(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    provider = SimpleNamespace(
        id="p1",
        provider_type=ProviderType.CODEX,
        endpoints=[SimpleNamespace(api_format="openai:cli", is_active=True)],
    )
    key = SimpleNamespace(
        id="k1",
        name="K1",
        is_active=True,
        upstream_metadata={"codex": {"primary_used_percent": 100.0}},
    )
    db = _FakeDB(provider=provider, keys=[key])

    async def _fake_handler(**kwargs: Any) -> dict[str, Any]:
        _ = kwargs
        return {
            "key_id": "k1",
            "key_name": "K1",
            "status": "success",
            "quota_exhausted": True,
        }

    monkeypatch.setattr(
        quota_service_module,
        "_select_refresh_endpoint",
        lambda provider, provider_type: provider.endpoints[0],
    )
    monkeypatch.setattr(
        quota_service_module, "_resolve_quota_refresh_handler", lambda _: _fake_handler
    )

    result = await refresh_provider_quota_for_provider(
        db=cast(Any, db),
        provider_id="p1",
        codex_wham_usage_url="https://example.test/wham/usage",
    )

    assert result["success"] == 1
    assert result["failed"] == 0
    assert result["total"] == 1
