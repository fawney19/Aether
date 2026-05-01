from __future__ import annotations

import sys
import types
from dataclasses import dataclass
from types import SimpleNamespace

from src.services.provider.adapters.codex.context import (
    CodexRequestContext,
    set_codex_request_context,
)
from src.services.provider.transport import build_provider_url


@dataclass
class _DummyEndpoint:
    base_url: str
    api_format: str
    custom_path: str | None = None
    provider: object | None = None
    provider_id: str | None = None


class _FakeQuery:
    def __init__(self, row: object | None) -> None:
        self._row = row

    def filter(self, *_args: object, **_kwargs: object) -> "_FakeQuery":
        return self

    def first(self) -> object | None:
        return self._row


class _FakeSessionCtx:
    def __init__(self, row: object | None) -> None:
        self._row = row

    def __enter__(self) -> "_FakeSessionCtx":
        return self

    def __exit__(self, exc_type: object, exc: object, tb: object) -> bool:
        _ = exc_type, exc, tb
        return False

    def query(self, _model: object) -> _FakeQuery:
        return _FakeQuery(self._row)


def test_codex_openai_cli_uses_responses_path_without_v1_prefix() -> None:
    endpoint = _DummyEndpoint(
        base_url="https://chatgpt.com/backend-api/codex",
        api_format="openai:cli",
        provider=SimpleNamespace(provider_type="codex"),
    )

    url = build_provider_url(
        endpoint,  # type: ignore[arg-type]
        path_params={"model": "ignored"},
        is_stream=True,
    )

    assert url == "https://chatgpt.com/backend-api/codex/responses"


def test_codex_openai_cli_bad_input_is_not_deduped() -> None:
    """用户把 /responses 塞进 base_url 是坏输入，按新规则直接拼，不再去重。

    validator 会拦截这种保存；运行时如实拼接，让用户看到问题。
    """
    endpoint = _DummyEndpoint(
        base_url="https://chatgpt.com/backend-api/codex/responses",
        api_format="openai:cli",
        provider=SimpleNamespace(provider_type="codex"),
    )

    url = build_provider_url(
        endpoint,  # type: ignore[arg-type]
        path_params={"model": "ignored"},
        is_stream=False,
    )

    assert url == "https://chatgpt.com/backend-api/codex/responses/responses"


def test_codex_openai_cli_uses_compact_suffix_when_context_marked_compact() -> None:
    endpoint = _DummyEndpoint(
        base_url="https://chatgpt.com/backend-api/codex",
        api_format="openai:cli",
        provider=SimpleNamespace(provider_type="codex"),
    )
    try:
        set_codex_request_context(CodexRequestContext(is_compact=True))
        url = build_provider_url(
            endpoint,  # type: ignore[arg-type]
            path_params={"model": "ignored"},
            is_stream=False,
        )
        assert url == "https://chatgpt.com/backend-api/codex/responses/compact"
    finally:
        set_codex_request_context(None)


def test_codex_openai_compact_uses_compact_path_without_v1_prefix() -> None:
    endpoint = _DummyEndpoint(
        base_url="https://chatgpt.com/backend-api/codex",
        api_format="openai:compact",
        provider=SimpleNamespace(provider_type="codex"),
    )
    url = build_provider_url(
        endpoint,  # type: ignore[arg-type]
        path_params={"model": "ignored"},
        is_stream=False,
    )
    assert url == "https://chatgpt.com/backend-api/codex/responses/compact"


def test_codex_openai_cli_uses_provider_id_lookup_without_touching_endpoint_provider(
    monkeypatch,
) -> None:
    class _DetachedEndpoint:
        base_url = "https://chatgpt.com/backend-api/codex"
        api_format = "openai:cli"
        custom_path = None
        provider_id = "provider-1"

        @property
        def provider(self) -> object:
            raise RuntimeError("detached endpoint provider should not be lazy-loaded")

    fake_provider = SimpleNamespace(id="provider-1", provider_type="codex", proxy=None)
    fake_database = types.ModuleType("src.database")
    fake_database.create_session = lambda: _FakeSessionCtx(fake_provider)
    fake_models = types.ModuleType("src.models.database")
    fake_models.Provider = type("Provider", (), {"id": "id"})

    monkeypatch.setitem(sys.modules, "src.database", fake_database)
    monkeypatch.setitem(sys.modules, "src.models.database", fake_models)

    url = build_provider_url(
        _DetachedEndpoint(),  # type: ignore[arg-type]
        path_params={"model": "ignored"},
        is_stream=True,
    )

    assert url == "https://chatgpt.com/backend-api/codex/responses"
