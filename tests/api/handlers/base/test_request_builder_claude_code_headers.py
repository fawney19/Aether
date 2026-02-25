from __future__ import annotations

from dataclasses import dataclass
from types import SimpleNamespace

from src.api.handlers.base.request_builder import PassthroughRequestBuilder


@dataclass
class _DummyEndpoint:
    api_family: str = "claude"
    endpoint_kind: str = "cli"
    api_format: str = "claude:cli"
    header_rules: list[dict] | None = None
    provider: object | None = None


@dataclass
class _DummyKey:
    api_key: str = ""
    provider: object | None = None


def test_passthrough_builder_merges_client_and_claude_code_beta_headers() -> None:
    endpoint = _DummyEndpoint(provider=SimpleNamespace(provider_type="claude_code"))
    key = _DummyKey(provider=endpoint.provider)
    builder = PassthroughRequestBuilder()

    headers = builder.build_headers(
        original_headers={
            "Anthropic-Beta": "context-1m-2025-08-07,oauth-2025-04-20",
            "X-From-Client": "1",
        },
        endpoint=endpoint,
        key=key,
        extra_headers={
            "Anthropic-Beta": "claude-code-20250219,oauth-2025-04-20,prompt-caching-2024-07-31",
            "X-App": "cli",
        },
        pre_computed_auth=("Authorization", "Bearer oauth-token"),
    )

    lower_headers = {k.lower(): v for k, v in headers.items()}
    assert lower_headers["x-from-client"] == "1"
    assert lower_headers["x-app"] == "cli"

    beta_tokens = [token.strip() for token in lower_headers["anthropic-beta"].split(",") if token.strip()]
    assert beta_tokens[0] == "context-1m-2025-08-07"
    assert "claude-code-20250219" in beta_tokens
    assert "prompt-caching-2024-07-31" in beta_tokens
    assert beta_tokens.count("oauth-2025-04-20") == 1
