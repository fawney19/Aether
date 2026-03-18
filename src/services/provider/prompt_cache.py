"""Helpers for synthesizing stable prompt cache keys."""

from __future__ import annotations

import re
import uuid
from collections.abc import Mapping
from typing import Any

from src.core.provider_types import ProviderType, normalize_provider_type
from src.services.provider.adapters.codex.context import is_codex_compact_request
from src.utils.url_utils import is_official_openai_api_url

_OFFICIAL_OPENAI_PROMPT_CACHE_FORMATS: frozenset[str] = frozenset({"openai:chat", "openai:cli"})
_PROMPT_CACHE_NAMESPACE_VERSION = "v3"
_USER_AGENT_CLIENT_FAMILY_PATTERNS: tuple[tuple[tuple[str, ...], str], ...] = (
    (("codex desktop",), "codex_desktop"),
    (("asyncopenai/python", "openai/python"), "openai_python"),
    (("openai-node", "openai/javascript"), "openai_node"),
    (("openai-go",), "openai_go"),
    (("openai-java",), "openai_java"),
    (("openai-ruby",), "openai_ruby"),
    (("postmanruntime/",), "postman"),
    (("curl/",), "curl"),
    (("mozilla/",), "browser"),
)


def _get_header_value(headers: Mapping[str, Any] | None, header_name: str) -> str | None:
    if not isinstance(headers, Mapping):
        return None

    target = str(header_name or "").strip().lower()
    if not target:
        return None

    for name, value in headers.items():
        if str(name or "").strip().lower() != target:
            continue
        normalized = str(value or "").strip()
        return normalized or None
    return None


def normalize_prompt_cache_client_family(user_agent: str | None) -> str:
    """Reduce a raw User-Agent string to a stable client-family token."""
    raw = str(user_agent or "").strip().lower()
    if not raw:
        return "generic"

    for patterns, family in _USER_AGENT_CLIENT_FAMILY_PATTERNS:
        if any(pattern in raw for pattern in patterns):
            return family

    normalized = re.sub(r"[^a-z0-9]+", "_", raw.split()[0].split("/")[0]).strip("_")
    return normalized[:48] if normalized else "generic"


def resolve_prompt_cache_client_family(request_headers: Mapping[str, Any] | None) -> str:
    """Best-effort client-family extraction from request headers."""
    return normalize_prompt_cache_client_family(_get_header_value(request_headers, "user-agent"))


def _build_stable_prompt_cache_key(
    user_api_key_id: str | None,
    *,
    scope: str,
    client_family: str | None = None,
) -> str | None:
    normalized = str(user_api_key_id or "").strip()
    if not normalized:
        return None

    # Keep the optional argument for call-site compatibility, but ignore it.
    # Prompt cache reuse is more valuable than splitting namespaces by User-Agent.
    _ = client_family
    namespace = f"aether:{scope}:prompt-cache:{_PROMPT_CACHE_NAMESPACE_VERSION}:user:{normalized}"
    return str(uuid.uuid5(uuid.NAMESPACE_OID, namespace))


def build_stable_openai_prompt_cache_key(
    user_api_key_id: str | None,
    *,
    client_family: str | None = None,
) -> str | None:
    """Build a deterministic official OpenAI prompt cache key from the caller's user API key id."""
    return _build_stable_prompt_cache_key(
        user_api_key_id,
        scope="openai",
        client_family=client_family,
    )


def build_stable_codex_prompt_cache_key(
    user_api_key_id: str | None,
    *,
    client_family: str | None = None,
) -> str | None:
    """Build a deterministic Codex prompt cache key from the caller's user API key id."""
    return _build_stable_prompt_cache_key(
        user_api_key_id,
        scope="codex",
        client_family=client_family,
    )


def resolve_prompt_cache_key_scope(
    *,
    provider_api_format: str | None,
    provider_type: str | None = None,
    base_url: str | None = None,
) -> str | None:
    """Resolve which prompt cache strategy should be used for this request."""
    fmt = str(provider_api_format or "").strip().lower()
    if fmt == "openai:compact":
        return None

    pt = normalize_provider_type(provider_type)
    if pt == ProviderType.CODEX.value and fmt == "openai:cli":
        if is_codex_compact_request(endpoint_sig=fmt):
            return None
        return "codex"

    if fmt in _OFFICIAL_OPENAI_PROMPT_CACHE_FORMATS and is_official_openai_api_url(base_url):
        return "openai"

    return None


def maybe_patch_request_with_prompt_cache_key(
    request_body: Any,
    *,
    provider_api_format: str | None,
    provider_type: str | None = None,
    base_url: str | None = None,
    user_api_key_id: str | None = None,
    request_headers: Mapping[str, Any] | None = None,
) -> Any:
    """Inject a stable prompt_cache_key when the target upstream supports deterministic reuse."""
    if not isinstance(request_body, dict):
        return request_body

    scope = resolve_prompt_cache_key_scope(
        provider_api_format=provider_api_format,
        provider_type=provider_type,
        base_url=base_url,
    )
    if not scope:
        return request_body

    prompt_cache_key = str(request_body.get("prompt_cache_key") or "").strip()
    if prompt_cache_key:
        return request_body

    if scope == "codex":
        stable_key = build_stable_codex_prompt_cache_key(
            user_api_key_id,
        )
    else:
        stable_key = build_stable_openai_prompt_cache_key(
            user_api_key_id,
        )
    if not stable_key:
        return request_body

    out = dict(request_body)
    out["prompt_cache_key"] = stable_key
    return out


__all__ = [
    "build_stable_codex_prompt_cache_key",
    "build_stable_openai_prompt_cache_key",
    "maybe_patch_request_with_prompt_cache_key",
    "normalize_prompt_cache_client_family",
    "resolve_prompt_cache_client_family",
    "resolve_prompt_cache_key_scope",
]
