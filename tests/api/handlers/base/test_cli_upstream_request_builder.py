from __future__ import annotations

from types import SimpleNamespace
from typing import Any

import pytest

import src.api.handlers.base.cli_request_mixin as mixmod
from src.api.handlers.base.cli_request_mixin import CliRequestMixin
from src.api.handlers.base.request_builder import PassthroughRequestBuilder
from src.core.api_format.metadata import CODEX_DEFAULT_BODY_RULES
from src.services.provider.adapters.codex.context import (
    CodexRequestContext,
    set_codex_request_context,
)
from src.services.provider.prompt_cache import build_stable_codex_prompt_cache_key


class _DummyAuthInfo:
    auth_header = "Authorization"
    auth_value = "Bearer upstream-token"
    decrypted_auth_config = None

    def as_tuple(self) -> tuple[str, str]:
        return self.auth_header, self.auth_value


class _DummyCliRequestHandler(CliRequestMixin):
    FORMAT_ID = "openai:cli"

    def __init__(self) -> None:
        self.api_key = SimpleNamespace(id="user-key-123")
        self._request_builder = PassthroughRequestBuilder()


def _build_codex_provider() -> Any:
    return SimpleNamespace(
        id="provider-1",
        provider_type="codex",
        config=None,
        proxy=None,
    )


def _build_codex_endpoint(*, api_format: str) -> Any:
    provider = _build_codex_provider()
    return SimpleNamespace(
        id="endpoint-1",
        api_family="openai",
        endpoint_kind="cli",
        api_format=api_format,
        base_url="https://chatgpt.com/backend-api/codex",
        custom_path=None,
        body_rules=list(CODEX_DEFAULT_BODY_RULES),
        header_rules=None,
        provider=provider,
    )


def _build_key() -> Any:
    return SimpleNamespace(id="key-1", api_key="unused", proxy=None)


def _build_headers() -> dict[str, str]:
    return {
        "accept": "application/json",
        "content-type": "application/json",
        "user-agent": "Codex Desktop/0.108.0-alpha.12",
        "originator": "Codex Desktop",
        "x-codex-turn-metadata": '{"turn_id":"abc"}',
        "host": "aether.hetunai.cn",
        "content-length": "123",
        "x-forwarded-scheme": "https",
    }


def _assert_common_codex_headers(headers: dict[str, str]) -> None:
    assert headers["accept"] == "application/json"
    assert headers["content-type"] == "application/json"
    assert headers["user-agent"] == "Codex Desktop/0.108.0-alpha.12"
    assert headers["originator"] == "Codex Desktop"
    assert headers["x-codex-turn-metadata"] == '{"turn_id":"abc"}'
    assert headers["Authorization"] == "Bearer upstream-token"
    assert "host" not in headers
    assert "content-length" not in headers
    assert "x-forwarded-scheme" not in headers


@pytest.mark.asyncio
async def test_build_upstream_request_codex_cli_injects_prompt_cache_and_forces_stream(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    async def _fake_get_provider_auth(endpoint: Any, key: Any) -> _DummyAuthInfo:
        return _DummyAuthInfo()

    monkeypatch.setattr(mixmod, "get_provider_auth", _fake_get_provider_auth)

    handler = _DummyCliRequestHandler()
    endpoint = _build_codex_endpoint(api_format="openai:cli")
    provider = endpoint.provider
    key = _build_key()

    result = await handler._build_upstream_request(
        provider=provider,
        endpoint=endpoint,
        key=key,
        request_body={
            "model": "gpt-5",
            "input": [],
            "stream": False,
            "max_output_tokens": 4096,
            "temperature": 0.7,
            "top_p": 0.8,
        },
        original_headers=_build_headers(),
        query_params=None,
        client_api_format="openai:cli",
        provider_api_format="openai:cli",
        fallback_model="gpt-5",
        mapped_model=None,
        client_is_stream=False,
    )

    assert result.url == "https://chatgpt.com/backend-api/codex/responses"
    assert result.upstream_is_stream is True
    assert result.payload["stream"] is True
    assert result.payload["instructions"] == "You are GPT-5."
    assert result.payload["store"] is False
    assert result.payload["prompt_cache_key"] == build_stable_codex_prompt_cache_key("user-key-123")
    assert "max_output_tokens" not in result.payload
    assert "temperature" not in result.payload
    assert "top_p" not in result.payload
    _assert_common_codex_headers(result.headers)


@pytest.mark.asyncio
async def test_build_upstream_request_codex_compact_drops_stream_and_skips_prompt_cache(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    async def _fake_get_provider_auth(endpoint: Any, key: Any) -> _DummyAuthInfo:
        return _DummyAuthInfo()

    monkeypatch.setattr(mixmod, "get_provider_auth", _fake_get_provider_auth)

    handler = _DummyCliRequestHandler()
    endpoint = _build_codex_endpoint(api_format="openai:compact")
    provider = endpoint.provider
    key = _build_key()

    result = await handler._build_upstream_request(
        provider=provider,
        endpoint=endpoint,
        key=key,
        request_body={"model": "gpt-5", "input": [], "stream": True},
        original_headers=_build_headers(),
        query_params=None,
        client_api_format="openai:cli",
        provider_api_format="openai:compact",
        fallback_model="gpt-5",
        mapped_model=None,
        client_is_stream=False,
    )

    assert result.url == "https://chatgpt.com/backend-api/codex/responses/compact"
    assert result.upstream_is_stream is False
    assert "stream" not in result.payload
    assert "prompt_cache_key" not in result.payload
    assert result.payload["instructions"] == "You are GPT-5."
    assert result.payload["store"] is False
    _assert_common_codex_headers(result.headers)


@pytest.mark.asyncio
async def test_build_upstream_request_legacy_codex_compact_context_uses_compact_url(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    async def _fake_get_provider_auth(endpoint: Any, key: Any) -> _DummyAuthInfo:
        return _DummyAuthInfo()

    monkeypatch.setattr(mixmod, "get_provider_auth", _fake_get_provider_auth)

    handler = _DummyCliRequestHandler()
    endpoint = _build_codex_endpoint(api_format="openai:cli")
    provider = endpoint.provider
    key = _build_key()

    try:
        set_codex_request_context(CodexRequestContext(is_compact=True))
        result = await handler._build_upstream_request(
            provider=provider,
            endpoint=endpoint,
            key=key,
            request_body={"model": "gpt-5", "input": [], "stream": True},
            original_headers=_build_headers(),
            query_params=None,
            client_api_format="openai:cli",
            provider_api_format="openai:cli",
            fallback_model="gpt-5",
            mapped_model=None,
            client_is_stream=False,
        )
    finally:
        set_codex_request_context(None)

    assert result.url == "https://chatgpt.com/backend-api/codex/responses/compact"
    assert result.upstream_is_stream is False
    assert "stream" not in result.payload
    assert "prompt_cache_key" not in result.payload
    assert result.payload["instructions"] == "You are GPT-5."
    _assert_common_codex_headers(result.headers)


@pytest.mark.asyncio
async def test_build_upstream_request_codex_cli_preserves_explicit_prompt_cache_key(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    async def _fake_get_provider_auth(endpoint: Any, key: Any) -> _DummyAuthInfo:
        return _DummyAuthInfo()

    monkeypatch.setattr(mixmod, "get_provider_auth", _fake_get_provider_auth)

    handler = _DummyCliRequestHandler()
    endpoint = _build_codex_endpoint(api_format="openai:cli")
    provider = endpoint.provider
    key = _build_key()

    result = await handler._build_upstream_request(
        provider=provider,
        endpoint=endpoint,
        key=key,
        request_body={
            "model": "gpt-5",
            "input": [],
            "prompt_cache_key": "client-cache-key",
        },
        original_headers=_build_headers(),
        query_params=None,
        client_api_format="openai:cli",
        provider_api_format="openai:cli",
        fallback_model="gpt-5",
        mapped_model=None,
        client_is_stream=False,
    )

    assert result.url == "https://chatgpt.com/backend-api/codex/responses"
    assert result.payload["prompt_cache_key"] == "client-cache-key"
    assert result.payload["stream"] is True
    _assert_common_codex_headers(result.headers)
