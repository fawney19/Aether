"""Upstream streaming execution policy (per endpoint).

This is about how we talk to the upstream provider, not what the client asked for.

Motivation:
- Some upstreams require streaming only (e.g. Codex Responses OAuth endpoint).
- Some upstreams do not support streaming (or are flaky with SSE).

We allow forcing upstream request mode per ProviderEndpoint, while the gateway still
returns what the client requested by doing internal sync<->stream bridging.
"""

from __future__ import annotations

from enum import Enum
from typing import Any

from src.core.api_format.metadata import resolve_endpoint_definition
from src.core.provider_types import ProviderType
from src.services.provider.adapters.codex.context import is_codex_compact_request


class UpstreamStreamPolicy(str, Enum):
    AUTO = "auto"  # follow client request
    FORCE_STREAM = "force_stream"
    FORCE_NON_STREAM = "force_non_stream"


def parse_upstream_stream_policy(value: Any) -> UpstreamStreamPolicy:
    if value is None:
        return UpstreamStreamPolicy.AUTO

    if isinstance(value, bool):
        return UpstreamStreamPolicy.FORCE_STREAM if value else UpstreamStreamPolicy.FORCE_NON_STREAM

    raw = str(value).strip().lower()
    if raw in {"", "auto", "follow", "client", "default"}:
        return UpstreamStreamPolicy.AUTO
    if raw in {"force_stream", "stream", "sse", "true", "1", "yes"}:
        return UpstreamStreamPolicy.FORCE_STREAM
    if raw in {"force_non_stream", "force_sync", "non_stream", "sync", "false", "0", "no"}:
        return UpstreamStreamPolicy.FORCE_NON_STREAM

    return UpstreamStreamPolicy.AUTO


def get_upstream_stream_policy(
    endpoint: Any,
    *,
    provider_type: str | None = None,
    endpoint_sig: str | None = None,
) -> UpstreamStreamPolicy:
    """Resolve policy for an endpoint.

    Config source: endpoint.config["upstream_stream_policy"] (preferred).

    Defaults:
    - Codex + openai:cli: FORCE_STREAM (Codex upstream requires stream=true).
    - Codex + openai:compact: follow endpoint/client policy (no hard force).
    """

    provider_obj = getattr(endpoint, "provider", None)
    pt = str(provider_type or getattr(provider_obj, "provider_type", "") or "").strip().lower()
    sig = str(endpoint_sig or getattr(endpoint, "api_format", "") or "").strip().lower()
    is_codex_cli = pt == ProviderType.CODEX and sig == "openai:cli"
    is_codex_compact = pt == ProviderType.CODEX and sig == "openai:compact"
    if is_codex_cli:
        is_codex_compact = is_codex_compact_request(endpoint_sig=sig)

    # Explicit config wins (unless upstream has a hard constraint).
    cfg = getattr(endpoint, "config", None)
    if isinstance(cfg, dict):
        val = (
            cfg.get("upstream_stream_policy")
            or cfg.get("upstreamStreamPolicy")
            or cfg.get("upstream_stream")
        )
        parsed = parse_upstream_stream_policy(val)
        if parsed != UpstreamStreamPolicy.AUTO:
            # Codex upstream requires streaming; do not allow forcing non-stream.
            if (
                is_codex_cli
                and parsed == UpstreamStreamPolicy.FORCE_NON_STREAM
                and not is_codex_compact
            ):
                return UpstreamStreamPolicy.FORCE_STREAM
            if pt == ProviderType.KIRO and parsed == UpstreamStreamPolicy.FORCE_NON_STREAM:
                return UpstreamStreamPolicy.FORCE_STREAM
            return parsed

    # Safe-by-default: Codex Responses OAuth behaves like SSE-only.
    if is_codex_cli:
        return (
            UpstreamStreamPolicy.FORCE_NON_STREAM
            if is_codex_compact
            else UpstreamStreamPolicy.FORCE_STREAM
        )

    # Kiro upstream streams binary AWS Event Stream; treat as stream-only.
    if pt == ProviderType.KIRO:
        return UpstreamStreamPolicy.FORCE_STREAM

    return UpstreamStreamPolicy.AUTO


def resolve_upstream_is_stream(
    *,
    client_is_stream: bool,
    policy: UpstreamStreamPolicy,
) -> bool:
    if policy == UpstreamStreamPolicy.FORCE_STREAM:
        return True
    if policy == UpstreamStreamPolicy.FORCE_NON_STREAM:
        return False
    return bool(client_is_stream)


def enforce_stream_mode_for_upstream(
    request_body: dict[str, Any],
    *,
    provider_api_format: str,
    upstream_is_stream: bool,
) -> dict[str, Any]:
    """Force upstream stream/sync mode in request body (best-effort).

    Note: Some formats (Gemini) do not use a `stream` field in body; for those we
    remove it to avoid leaking client intent.
    """

    meta = resolve_endpoint_definition(provider_api_format)
    provider_uses_stream = meta.stream_in_body if meta is not None else True

    provider_fmt = str(provider_api_format or "").strip().lower()
    # OpenAI compact endpoint: keep request body stream field absent.
    if provider_fmt == "openai:compact":
        request_body.pop("stream", None)
        return request_body

    # Backward compatibility: Codex compact routed through openai:cli + context marker.
    if provider_fmt == "openai:cli" and is_codex_compact_request(endpoint_sig=provider_fmt):
        request_body.pop("stream", None)
        return request_body

    if provider_uses_stream:
        request_body["stream"] = bool(upstream_is_stream)
    else:
        request_body.pop("stream", None)

    # OpenAI Chat Completions: request usage in streaming mode.
    if upstream_is_stream and provider_fmt == "openai:chat":
        stream_options = request_body.get("stream_options")
        if not isinstance(stream_options, dict):
            stream_options = {}
        stream_options["include_usage"] = True
        request_body["stream_options"] = stream_options

    return request_body


__all__ = [
    "UpstreamStreamPolicy",
    "enforce_stream_mode_for_upstream",
    "get_upstream_stream_policy",
    "parse_upstream_stream_policy",
    "resolve_upstream_is_stream",
]
