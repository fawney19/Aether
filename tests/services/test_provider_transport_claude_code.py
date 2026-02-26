from __future__ import annotations

from dataclasses import dataclass
from types import SimpleNamespace

import pytest

from src.services.model.upstream_fetcher import UpstreamModelsFetcherRegistry
from src.services.provider.adapters.claude_code.plugin import register_all
from src.services.provider.transport import build_provider_url


@dataclass
class _DummyEndpoint:
    base_url: str
    api_format: str
    custom_path: str | None = None
    provider: object | None = None


def test_claude_code_claude_cli_uses_messages_path() -> None:
    endpoint = _DummyEndpoint(
        base_url="https://api.anthropic.com",
        api_format="claude:cli",
        provider=SimpleNamespace(provider_type="claude_code"),
    )

    url = build_provider_url(
        endpoint,  # type: ignore[arg-type]
        path_params={"model": "ignored"},
        is_stream=True,
    )

    assert url == "https://api.anthropic.com/v1/messages"


def test_claude_code_claude_cli_does_not_duplicate_messages_suffix() -> None:
    endpoint = _DummyEndpoint(
        base_url="https://api.anthropic.com/v1/messages",
        api_format="claude:cli",
        provider=SimpleNamespace(provider_type="claude_code"),
    )

    url = build_provider_url(
        endpoint,  # type: ignore[arg-type]
        path_params={"model": "ignored"},
        is_stream=False,
    )

    assert url == "https://api.anthropic.com/v1/messages"


def test_claude_code_claude_cli_appends_query_params() -> None:
    endpoint = _DummyEndpoint(
        base_url="https://api.anthropic.com/v1",
        api_format="claude:cli",
        provider=SimpleNamespace(provider_type="claude_code"),
    )

    url = build_provider_url(
        endpoint,  # type: ignore[arg-type]
        query_params={"beta": "true"},
        path_params={"model": "ignored"},
        is_stream=False,
    )

    assert url == "https://api.anthropic.com/v1/messages?beta=true"


@pytest.mark.asyncio
async def test_claude_code_registers_preset_model_fetcher() -> None:
    register_all()

    fetcher = UpstreamModelsFetcherRegistry.get("claude_code")
    assert fetcher is not None

    models, errors, has_success, upstream_metadata = await fetcher(SimpleNamespace(), 1.0)
    model_ids = {m["id"] for m in models}

    assert "claude-sonnet-4-5-20250929" in model_ids
    assert "claude-opus-4-6" in model_ids
    assert errors == []
    assert has_success is True
    assert upstream_metadata is None
