from __future__ import annotations

from types import SimpleNamespace

import jwt
import pytest

from src.api.handlers.base.request_builder import PassthroughRequestBuilder
from src.services.provider.behavior import get_provider_behavior


def test_codex_provider_behavior_has_no_runtime_envelope_or_variants() -> None:
    behavior = get_provider_behavior(provider_type="codex", endpoint_sig="openai:cli")

    assert behavior.envelope is None
    assert behavior.same_format_variant is None
    assert behavior.cross_format_variant is None


def test_openai_cli_normalizer_request_from_internal_codex_variant_preserves_store() -> None:
    from src.core.api_format.conversion.normalizers.openai_cli import OpenAICliNormalizer

    normalizer = OpenAICliNormalizer()
    internal = normalizer.request_to_internal({"model": "gpt-test", "input": [], "store": True})
    out = normalizer.request_from_internal(internal, target_variant="codex")

    assert out["store"] is True


def test_openai_cli_normalizer_request_from_internal_codex_variant_does_not_inject_store() -> None:
    from src.core.api_format.conversion.normalizers.openai_cli import OpenAICliNormalizer

    normalizer = OpenAICliNormalizer()
    internal = normalizer.request_to_internal({"model": "gpt-test", "input": []})
    out = normalizer.request_from_internal(internal, target_variant="codex")

    assert "store" not in out


def test_openai_cli_normalizer_codex_variant_keeps_instructions_missing_for_default_rule() -> None:
    from src.api.handlers.base.request_builder import apply_body_rules
    from src.core.api_format.conversion.normalizers.openai_cli import OpenAICliNormalizer
    from src.core.api_format.metadata import CODEX_DEFAULT_BODY_RULES

    normalizer = OpenAICliNormalizer()
    internal = normalizer.request_to_internal({"model": "gpt-test", "input": []})
    out = normalizer.request_from_internal(internal, target_variant="codex")

    assert "instructions" not in out

    patched = apply_body_rules(out, list(CODEX_DEFAULT_BODY_RULES))
    assert patched["instructions"] == "You are GPT-5."


def test_openai_cli_normalizer_patch_for_codex_is_noop() -> None:
    from src.core.api_format.conversion.normalizers.openai_cli import OpenAICliNormalizer

    normalizer = OpenAICliNormalizer()
    out = normalizer.patch_for_variant(
        {
            "temperature": 0.7,
            "input": [],
            "metadata": {"request_id": "abc"},
            "model": "gpt-test",
            "tools": [{"type": "function", "name": "demo"}],
            "instructions": "keep",
        },
        "codex",
    )

    assert out is None


def test_codex_passthrough_builder_preserves_real_codex_headers() -> None:
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


def test_codex_passthrough_builder_applies_prompt_body_rules() -> None:
    from src.core.api_format.metadata import CODEX_DEFAULT_BODY_RULES

    builder = PassthroughRequestBuilder()
    endpoint = SimpleNamespace(
        api_family="openai",
        endpoint_kind="cli",
        header_rules=None,
        body_rules=list(CODEX_DEFAULT_BODY_RULES),
    )
    key = SimpleNamespace(api_key="unused")

    payload, _headers = builder.build(
        {"model": "gpt-test", "input": []},
        {"content-type": "application/json"},
        endpoint,
        key,
        pre_computed_auth=("Authorization", "Bearer upstream-token"),
        provider_api_format="openai:cli",
    )

    assert payload["instructions"] == "You are GPT-5."
    assert payload["store"] is False
    assert "max_output_tokens" not in payload


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
