from types import SimpleNamespace

import pytest

from src.api.admin.endpoints import routes
from src.api.admin.endpoints.routes import AdminCreateProviderEndpointAdapter
from src.models.database import Provider, ProviderEndpoint
from src.models.endpoint_models import ProviderEndpointCreate


class _FakeQuery:
    def __init__(self, result: object | None) -> None:
        self._result = result

    def filter(self, *_args: object, **_kwargs: object) -> "_FakeQuery":
        return self

    def first(self) -> object | None:
        return self._result


class _FakeDB:
    def __init__(self, provider: object) -> None:
        self.provider = provider
        self.added: ProviderEndpoint | None = None

    def query(self, model: object) -> _FakeQuery:
        if model is Provider:
            return _FakeQuery(self.provider)
        if model is ProviderEndpoint:
            return _FakeQuery(None)
        raise AssertionError(f"unexpected model: {model}")

    def add(self, obj: ProviderEndpoint) -> None:
        self.added = obj

    def commit(self) -> None:
        return None

    def refresh(self, _obj: ProviderEndpoint) -> None:
        return None


async def _noop_invalidate_cache() -> None:
    return None


@pytest.mark.asyncio
async def test_create_endpoint_injects_default_body_rules_when_missing(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(routes, "invalidate_models_list_cache", _noop_invalidate_cache)
    monkeypatch.setattr(
        routes,
        "get_default_body_rules_for_endpoint",
        lambda _fmt, **_kw: [{"action": "drop", "path": "max_output_tokens"}],
    )

    db = _FakeDB(
        provider=SimpleNamespace(id="p1", name="P1", provider_type="custom"),
    )
    adapter = AdminCreateProviderEndpointAdapter(
        provider_id="p1",
        endpoint_data=ProviderEndpointCreate(
            provider_id="p1",
            api_format="openai:cli",
            base_url="https://api.example.com",
        ),
    )

    await adapter.handle(SimpleNamespace(db=db))  # type: ignore[arg-type]
    assert db.added is not None
    assert db.added.body_rules == [{"action": "drop", "path": "max_output_tokens"}]


@pytest.mark.asyncio
async def test_create_endpoint_keeps_user_body_rules_when_provided(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(routes, "invalidate_models_list_cache", _noop_invalidate_cache)
    monkeypatch.setattr(
        routes,
        "get_default_body_rules_for_endpoint",
        lambda _fmt, **_kw: [{"action": "drop", "path": "max_output_tokens"}],
    )

    user_rules = [{"action": "set", "path": "metadata.source", "value": "user"}]
    db = _FakeDB(
        provider=SimpleNamespace(id="p1", name="P1", provider_type="custom"),
    )
    adapter = AdminCreateProviderEndpointAdapter(
        provider_id="p1",
        endpoint_data=ProviderEndpointCreate(
            provider_id="p1",
            api_format="openai:cli",
            base_url="https://api.example.com",
            body_rules=user_rules,
        ),
    )

    await adapter.handle(SimpleNamespace(db=db))  # type: ignore[arg-type]
    assert db.added is not None
    assert db.added.body_rules == user_rules
