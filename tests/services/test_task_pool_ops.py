from __future__ import annotations

from types import SimpleNamespace
from typing import Any, cast

import pytest

from src.models.database import Provider, ProviderAPIKey, ProviderEndpoint
from src.services.scheduling.schemas import PoolCandidate, ProviderCandidate
from src.services.task.execute.pool import TaskPoolOperationsService


def _provider(provider_id: str) -> Provider:
    return cast(Provider, SimpleNamespace(id=provider_id))


def _endpoint(endpoint_id: str) -> ProviderEndpoint:
    return cast(ProviderEndpoint, SimpleNamespace(id=endpoint_id))


def _key(key_id: str) -> ProviderAPIKey:
    return cast(ProviderAPIKey, SimpleNamespace(id=key_id))


def test_extract_session_uuid_returns_none_for_non_dict_request_body() -> None:
    svc = TaskPoolOperationsService()
    assert svc.extract_session_uuid("openai", None) is None
    assert svc.extract_session_uuid("openai", cast(Any, "not-dict")) is None


def test_extract_session_uuid_uses_pool_hook(monkeypatch: pytest.MonkeyPatch) -> None:
    from src.services.provider.pool import hooks

    hook = SimpleNamespace(extract_session_uuid=lambda body: f"sid:{body.get('session')}")

    def _get_pool_hook(_provider_type: str) -> Any:
        return hook

    monkeypatch.setattr(hooks, "get_pool_hook", _get_pool_hook)

    svc = TaskPoolOperationsService()
    session_id = svc.extract_session_uuid("claude_code", {"session": "abc"})
    assert session_id == "sid:abc"


def test_expand_pool_candidates_for_async_submit_keeps_non_pool_candidate() -> None:
    provider = _provider("p1")
    endpoint = _endpoint("e1")
    key = _key("k1")
    candidate = ProviderCandidate(provider=provider, endpoint=endpoint, key=key)

    svc = TaskPoolOperationsService()
    expanded = svc.expand_pool_candidates_for_async_submit([candidate])

    assert len(expanded) == 1
    assert expanded[0] is candidate


def test_expand_pool_candidates_for_async_submit_expands_pool_keys() -> None:
    provider = _provider("p1")
    endpoint = _endpoint("e1")
    key = _key("k0")
    pool_key_1 = cast(
        ProviderAPIKey,
        SimpleNamespace(
            id="k1",
            _pool_skipped=False,
            _pool_mapping_matched_model="mapped-model",
            _pool_extra_data={"source": "warm"},
        ),
    )
    pool_key_2 = cast(
        ProviderAPIKey,
        SimpleNamespace(
            id="k2",
            _pool_skipped=True,
            _pool_skip_reason="cooldown",
            _pool_extra_data={"reason_code": "429"},
        ),
    )

    pool_candidate = PoolCandidate(
        provider=provider,
        endpoint=endpoint,
        key=key,
        is_cached=True,
        is_skipped=False,
        skip_reason=None,
        mapping_matched_model="fallback-model",
        needs_conversion=True,
        provider_api_format="openai:chat",
        output_limit=1024,
        capability_miss_count=1,
        pool_keys=[pool_key_1, pool_key_2],
    )

    svc = TaskPoolOperationsService()
    expanded = svc.expand_pool_candidates_for_async_submit([pool_candidate])

    assert len(expanded) == 2
    first, second = expanded

    assert first.key is pool_key_1
    assert first.is_skipped is False
    assert first.mapping_matched_model == "mapped-model"
    first_extra = getattr(first, "_pool_extra_data")
    assert first_extra["pool_group_id"] == "p1"
    assert first_extra["pool_key_index"] == 0
    assert first_extra["source"] == "warm"

    assert second.key is pool_key_2
    assert second.is_skipped is True
    assert second.skip_reason == "cooldown"
    assert second.mapping_matched_model == "fallback-model"
    second_extra = getattr(second, "_pool_extra_data")
    assert second_extra["pool_group_id"] == "p1"
    assert second_extra["pool_key_index"] == 1
    assert second_extra["reason_code"] == "429"
