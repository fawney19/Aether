from __future__ import annotations

from dataclasses import dataclass
from types import SimpleNamespace

from src.services.provider.request_context import get_selected_base_url
from src.services.provider.transport import build_provider_url


@dataclass
class _DummyEndpoint:
    base_url: str
    api_format: str
    custom_path: str | None = None
    provider: object | None = None


def test_claude_code_url_normalizes_base_v1_suffix_and_sets_contextvar() -> None:
    endpoint = _DummyEndpoint(
        base_url="https://api.anthropic.com/v1",
        api_format="claude:cli",
        provider=SimpleNamespace(provider_type="claude_code"),
    )

    url = build_provider_url(
        endpoint,  # type: ignore[arg-type]
        path_params={"model": "ignored"},
        is_stream=False,
    )

    assert url == "https://api.anthropic.com/v1/messages"
    assert get_selected_base_url() == "https://api.anthropic.com"


def test_claude_code_url_prefers_custom_path() -> None:
    endpoint = _DummyEndpoint(
        base_url="https://api.anthropic.com",
        api_format="claude:cli",
        custom_path="/v1/messages/count_tokens",
        provider=SimpleNamespace(provider_type="claude_code"),
    )

    url = build_provider_url(
        endpoint,  # type: ignore[arg-type]
        path_params={"model": "ignored"},
        query_params={"beta": "1"},
        is_stream=False,
    )

    assert url == "https://api.anthropic.com/v1/messages/count_tokens?beta=1"
