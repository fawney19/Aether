from __future__ import annotations

from src.services.provider.adapters.codex.context import (
    CodexRequestContext,
    set_codex_request_context,
)
from src.services.provider.prompt_cache import (
    build_stable_codex_prompt_cache_key,
    build_stable_openai_prompt_cache_key,
    maybe_patch_request_with_prompt_cache_key,
    normalize_prompt_cache_client_family,
    resolve_prompt_cache_client_family,
)
from src.utils.url_utils import is_official_openai_api_url


def test_is_official_openai_api_url_matches_api_openai() -> None:
    assert is_official_openai_api_url("https://api.openai.com/v1")
    assert is_official_openai_api_url("api.openai.com")


def test_is_official_openai_api_url_rejects_compatible_hosts() -> None:
    assert not is_official_openai_api_url("https://api.deepseek.com/v1")
    assert not is_official_openai_api_url("https://example.com/openai")


def test_normalize_prompt_cache_client_family_is_version_stable() -> None:
    assert normalize_prompt_cache_client_family("AsyncOpenAI/Python 2.14.0") == "openai_python"
    assert normalize_prompt_cache_client_family("AsyncOpenAI/Python 2.15.1") == "openai_python"


def test_resolve_prompt_cache_client_family_defaults_to_generic() -> None:
    assert resolve_prompt_cache_client_family(None) == "generic"
    assert resolve_prompt_cache_client_family({"x-test": "1"}) == "generic"


def test_build_stable_openai_prompt_cache_key_ignores_client_family() -> None:
    python_key = build_stable_openai_prompt_cache_key(
        "user-key-123",
        client_family="openai_python",
    )
    node_key = build_stable_openai_prompt_cache_key(
        "user-key-123",
        client_family="openai_node",
    )

    assert python_key
    assert node_key
    assert python_key == node_key


def test_build_stable_prompt_cache_key_stays_scope_specific() -> None:
    openai_key = build_stable_openai_prompt_cache_key("user-key-123")
    codex_key = build_stable_codex_prompt_cache_key("user-key-123")

    assert openai_key
    assert codex_key
    assert openai_key != codex_key


def test_maybe_patch_request_with_prompt_cache_key_for_official_chat() -> None:
    req = {"model": "gpt-5", "messages": [{"role": "user", "content": "hi"}]}

    out = maybe_patch_request_with_prompt_cache_key(
        req,
        provider_api_format="openai:chat",
        provider_type="custom",
        base_url="https://api.openai.com/v1",
        user_api_key_id="user-key-123",
        request_headers={"user-agent": "AsyncOpenAI/Python 2.14.0"},
    )

    assert out is not req
    assert out["prompt_cache_key"] == build_stable_openai_prompt_cache_key("user-key-123")


def test_maybe_patch_request_with_prompt_cache_key_for_official_responses() -> None:
    req = {"model": "gpt-5", "input": []}

    out = maybe_patch_request_with_prompt_cache_key(
        req,
        provider_api_format="openai:cli",
        provider_type="custom",
        base_url="https://api.openai.com/v1",
        user_api_key_id="user-key-123",
        request_headers={"user-agent": "openai-node/4.0.0"},
    )

    assert out is not req
    assert out["prompt_cache_key"] == build_stable_openai_prompt_cache_key("user-key-123")


def test_maybe_patch_request_with_prompt_cache_key_for_codex_openai_cli() -> None:
    req = {"model": "gpt-5", "input": []}

    out = maybe_patch_request_with_prompt_cache_key(
        req,
        provider_api_format="openai:cli",
        provider_type="codex",
        base_url="https://chatgpt.com/backend-api/codex",
        user_api_key_id="user-key-123",
        request_headers={"user-agent": "Codex Desktop/0.108.0-alpha.12"},
    )

    assert out is not req
    assert out["prompt_cache_key"] == build_stable_codex_prompt_cache_key("user-key-123")


def test_maybe_patch_request_with_prompt_cache_key_skips_official_compact() -> None:
    req = {"model": "gpt-5", "input": []}

    out = maybe_patch_request_with_prompt_cache_key(
        req,
        provider_api_format="openai:compact",
        provider_type="custom",
        base_url="https://api.openai.com/v1",
        user_api_key_id="user-key-123",
    )

    assert out is req
    assert "prompt_cache_key" not in out


def test_maybe_patch_request_with_prompt_cache_key_skips_legacy_codex_compact_context() -> None:
    req = {"model": "gpt-5", "input": []}

    try:
        set_codex_request_context(CodexRequestContext(is_compact=True))
        out = maybe_patch_request_with_prompt_cache_key(
            req,
            provider_api_format="openai:cli",
            provider_type="codex",
            base_url="https://chatgpt.com/backend-api/codex",
            user_api_key_id="user-key-123",
        )
    finally:
        set_codex_request_context(None)

    assert out is req
    assert "prompt_cache_key" not in out


def test_maybe_patch_request_with_prompt_cache_key_preserves_existing_key() -> None:
    req = {"model": "gpt-5", "input": [], "prompt_cache_key": "client-cache-key"}

    out = maybe_patch_request_with_prompt_cache_key(
        req,
        provider_api_format="openai:cli",
        provider_type="custom",
        base_url="https://api.openai.com/v1",
        user_api_key_id="user-key-123",
        request_headers={"user-agent": "openai-node/4.0.0"},
    )

    assert out is req
    assert out["prompt_cache_key"] == "client-cache-key"


def test_maybe_patch_request_with_prompt_cache_key_skips_compatible_openai_like_hosts() -> None:
    req = {"model": "gpt-5", "messages": [{"role": "user", "content": "hi"}]}

    out = maybe_patch_request_with_prompt_cache_key(
        req,
        provider_api_format="openai:chat",
        provider_type="custom",
        base_url="https://api.deepseek.com/v1",
        user_api_key_id="user-key-123",
        request_headers={"user-agent": "AsyncOpenAI/Python 2.14.0"},
    )

    assert out is req
    assert "prompt_cache_key" not in out


def test_maybe_patch_request_with_prompt_cache_key_skips_unmatched_provider() -> None:
    req = {"model": "gpt-5", "input": []}

    out = maybe_patch_request_with_prompt_cache_key(
        req,
        provider_api_format="openai:video",
        provider_type="custom",
        base_url="https://api.openai.com/v1",
        user_api_key_id="user-key-123",
        request_headers={"user-agent": "AsyncOpenAI/Python 2.14.0"},
    )

    assert out is req
    assert "prompt_cache_key" not in out


def test_maybe_patch_request_with_prompt_cache_key_is_stable_without_user_agent() -> None:
    req = {"model": "gpt-5", "messages": [{"role": "user", "content": "hi"}]}

    out = maybe_patch_request_with_prompt_cache_key(
        req,
        provider_api_format="openai:chat",
        provider_type="custom",
        base_url="https://api.openai.com/v1",
        user_api_key_id="user-key-123",
    )

    assert out is not req
    assert out["prompt_cache_key"] == build_stable_openai_prompt_cache_key("user-key-123")


def test_maybe_patch_request_with_prompt_cache_key_ignores_user_agent_variants() -> None:
    req_python = {"model": "gpt-5", "input": []}
    req_codex = {"model": "gpt-5", "input": []}

    out_python = maybe_patch_request_with_prompt_cache_key(
        req_python,
        provider_api_format="openai:cli",
        provider_type="custom",
        base_url="https://api.openai.com/v1",
        user_api_key_id="user-key-123",
        request_headers={"user-agent": "AsyncOpenAI/Python 2.14.0"},
    )
    out_codex = maybe_patch_request_with_prompt_cache_key(
        req_codex,
        provider_api_format="openai:cli",
        provider_type="custom",
        base_url="https://api.openai.com/v1",
        user_api_key_id="user-key-123",
        request_headers={"user-agent": "Codex Desktop/0.108.0-alpha.12"},
    )

    assert out_python["prompt_cache_key"] == out_codex["prompt_cache_key"]
