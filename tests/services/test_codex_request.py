from __future__ import annotations

from types import SimpleNamespace

import jwt
import pytest

from src.api.handlers.base.request_builder import PassthroughRequestBuilder
from src.services.provider.adapters.codex.context import set_codex_request_context
from src.services.provider.adapters.codex.request_patching import (
    build_stable_codex_prompt_cache_key,
    maybe_patch_request_for_codex,
    patch_openai_cli_request_for_codex,
)


def test_patch_openai_cli_request_for_codex_is_passthrough_except_internal_sentinel() -> None:
    req = {
        "model": "gpt-test",
        "input": [
            {
                "type": "message",
                "role": "system",
                "content": [{"type": "input_text", "text": "Hello"}],
            }
        ],
        "store": True,
        "stream": False,
        "instructions": "keep",
        "include": ["foo"],
        "parallel_tool_calls": False,
        "temperature": 0.7,
        "context_management": {"compaction": {"type": "summary"}},
        "user": "u_123",
        "_aether_compact": True,
    }
    out = patch_openai_cli_request_for_codex(req)

    assert out is not req
    assert "_aether_compact" not in out
    assert out["store"] is True
    assert out["stream"] is False
    assert out["instructions"] == "keep"
    assert out["include"] == ["foo"]
    assert out["parallel_tool_calls"] is False
    assert out["temperature"] == 0.7
    assert out["context_management"] == {"compaction": {"type": "summary"}}
    assert out["user"] == "u_123"
    assert out["input"][0]["role"] == "system"


def test_patch_openai_cli_request_for_codex_generates_stable_prompt_cache_key_from_user_api_key_id() -> (
    None
):
    req = {"model": "gpt-test", "input": []}

    out = patch_openai_cli_request_for_codex(req, user_api_key_id="user-key-123")

    assert out is not req
    assert out["prompt_cache_key"] == build_stable_codex_prompt_cache_key("user-key-123")


def test_patch_openai_cli_request_for_codex_preserves_existing_prompt_cache_key() -> None:
    req = {"model": "gpt-test", "input": [], "prompt_cache_key": "client-cache-key"}

    out = patch_openai_cli_request_for_codex(req, user_api_key_id="user-key-123")

    assert out["prompt_cache_key"] == "client-cache-key"


def test_patch_openai_cli_request_for_codex_ignores_provider_key_and_uses_only_user_api_key_id() -> (
    None
):
    req = {"model": "gpt-test", "input": []}

    out_a = patch_openai_cli_request_for_codex(req, user_api_key_id="user-key-123")
    out_b = patch_openai_cli_request_for_codex(req, user_api_key_id="user-key-123")

    assert out_a["prompt_cache_key"] == out_b["prompt_cache_key"]
    assert out_a["prompt_cache_key"] == build_stable_codex_prompt_cache_key("user-key-123")


def test_maybe_patch_request_for_codex_is_noop_for_non_codex() -> None:
    req = {"model": "gpt-test", "input": []}
    out = maybe_patch_request_for_codex(
        provider_type="custom",
        provider_api_format="openai:cli",
        request_body=req,
    )
    assert out is req


def test_maybe_patch_request_for_codex_is_noop_for_non_openai_cli() -> None:
    req = {"model": "gpt-test", "input": []}
    out = maybe_patch_request_for_codex(
        provider_type="codex",
        provider_api_format="openai:chat",
        request_body=req,
    )
    assert out is req


def test_maybe_patch_request_for_codex_patches_for_codex_openai_cli() -> None:
    req = {"model": "gpt-test", "input": [], "_aether_compact": True, "store": True}
    out = maybe_patch_request_for_codex(
        provider_type="codex",
        provider_api_format="openai:cli",
        request_body=req,
    )

    assert out is not req
    assert out["store"] is True
    assert "_aether_compact" not in out


def test_maybe_patch_request_for_codex_patches_for_codex_openai_compact() -> None:
    req = {"model": "gpt-test", "input": [], "_aether_compact": True, "store": True}
    out = maybe_patch_request_for_codex(
        provider_type="codex",
        provider_api_format="openai:compact",
        request_body=req,
    )

    assert out is not req
    assert out["store"] is True
    assert "_aether_compact" not in out


def test_openai_cli_normalizer_request_from_internal_codex_variant_preserves_store() -> None:
    from src.core.api_format.conversion.normalizers.openai_cli import OpenAICliNormalizer

    normalizer = OpenAICliNormalizer()
    internal = normalizer.request_to_internal({"model": "gpt-test", "input": [], "store": True})
    out = normalizer.request_from_internal(internal, target_variant="codex")

    assert out["store"] is True


def test_openai_cli_normalizer_request_from_internal_codex_variant_defaults_store_false() -> None:
    from src.core.api_format.conversion.normalizers.openai_cli import OpenAICliNormalizer

    normalizer = OpenAICliNormalizer()
    internal = normalizer.request_to_internal({"model": "gpt-test", "input": []})
    out = normalizer.request_from_internal(internal, target_variant="codex")

    assert out["store"] is False


def test_codex_envelope_extra_headers_does_not_inject_synthetic_headers() -> None:
    from src.services.provider.adapters.codex.envelope import codex_oauth_envelope

    assert codex_oauth_envelope.extra_headers() is None


def test_codex_envelope_wrap_request_injects_stable_prompt_cache_key_from_user_api_key() -> None:
    from src.services.provider.adapters.codex.envelope import codex_oauth_envelope

    try:
        codex_oauth_envelope.prepare_context(
            provider_config=None,
            key_id="provider-key-123",
            user_api_key_id="user-key-123",
            is_stream=True,
        )
        out, url_model = codex_oauth_envelope.wrap_request(
            {"model": "gpt-test", "input": []},
            model="gpt-test",
            url_model=None,
            decrypted_auth_config=None,
        )
    finally:
        set_codex_request_context(None)

    assert url_model is None
    assert out["prompt_cache_key"] == build_stable_codex_prompt_cache_key("user-key-123")


def test_codex_envelope_wrap_request_same_user_different_provider_keys_share_prompt_cache_key() -> (
    None
):
    from src.services.provider.adapters.codex.envelope import codex_oauth_envelope

    try:
        codex_oauth_envelope.prepare_context(
            provider_config=None,
            key_id="provider-key-123",
            user_api_key_id="user-key-123",
            is_stream=True,
        )
        out_a, _ = codex_oauth_envelope.wrap_request(
            {"model": "gpt-test", "input": []},
            model="gpt-test",
            url_model=None,
            decrypted_auth_config=None,
        )
        codex_oauth_envelope.prepare_context(
            provider_config=None,
            key_id="provider-key-456",
            user_api_key_id="user-key-123",
            is_stream=True,
        )
        out_b, _ = codex_oauth_envelope.wrap_request(
            {"model": "gpt-test", "input": []},
            model="gpt-test",
            url_model=None,
            decrypted_auth_config=None,
        )
    finally:
        set_codex_request_context(None)

    assert out_a["prompt_cache_key"] == out_b["prompt_cache_key"]
    assert out_a["prompt_cache_key"] == build_stable_codex_prompt_cache_key("user-key-123")


def test_codex_passthrough_builder_preserves_real_codex_headers() -> None:
    from src.services.provider.adapters.codex.envelope import codex_oauth_envelope

    builder = PassthroughRequestBuilder()
    endpoint = SimpleNamespace(api_family="openai", endpoint_kind="cli", header_rules=None)
    key = SimpleNamespace(api_key="unused")

    headers = builder.build_headers(
        original_headers={
            "accept": "text/event-stream",
            "content-type": "application/json",
            "user-agent": "Codex Desktop/0.108.0-alpha.12",
            "originator": "Codex Desktop",
            "x-codex-turn-metadata": '{"turn_id":"abc"}',
            "x-forwarded-scheme": "https",
            "host": "aether.hetunai.cn",
            "content-length": "123",
        },
        endpoint=endpoint,
        key=key,
        pre_computed_auth=("Authorization", "Bearer upstream-token"),
        envelope=codex_oauth_envelope,
    )

    assert headers["accept"] == "text/event-stream"
    assert headers["content-type"] == "application/json"
    assert headers["user-agent"] == "Codex Desktop/0.108.0-alpha.12"
    assert headers["originator"] == "Codex Desktop"
    assert headers["x-codex-turn-metadata"] == '{"turn_id":"abc"}'
    assert headers["Authorization"] == "Bearer upstream-token"
    assert "Version" not in headers
    assert "Session_id" not in headers
    assert "Connection" not in headers
    assert "Chatgpt-Account-Id" not in headers
    assert "host" not in headers
    assert "content-length" not in headers
    assert "x-forwarded-scheme" not in headers


def _encode_unsigned_jwt(payload: dict[str, object]) -> str:
    token = jwt.encode(payload, key="", algorithm="none")
    return token.decode("utf-8") if isinstance(token, bytes) else token


@pytest.mark.asyncio
async def test_enrich_codex_uses_access_token_when_id_token_missing() -> None:
    from src.services.provider.adapters.codex.plugin import enrich_codex

    access_token = _encode_unsigned_jwt(
        {
            "email": "u@example.com",
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "acc-access",
                "chatgpt_plan_type": "team",
                "chatgpt_user_id": "user-access",
            },
        }
    )

    auth_config: dict[str, object] = {}
    out = await enrich_codex(
        auth_config=auth_config,
        token_response={"access_token": access_token},
        access_token=access_token,
        proxy_config=None,
    )

    assert out["email"] == "u@example.com"
    assert out["account_id"] == "acc-access"
    assert out["plan_type"] == "team"
    assert out["user_id"] == "user-access"
