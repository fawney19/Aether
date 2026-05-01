"""Provider-specific request body transformers.

Some providers require a full structural rewrite of the request body before
sending upstream (e.g. Codex reverse-proxy maps `openai:image` requests into
Responses API payloads with an `image_generation` tool).

This is heavier than `body_rules` (which only supports declarative set/drop
operations) and needs programmatic transformation with access to the mapped
model, path params (edit vs generate), and multipart-parsed data.

Usage mirrors `upstream_headers`:

    register_body_transformer("codex", "openai:image", transform_fn)

    new_body = transform_request_body(
        provider_type="codex",
        endpoint_sig="openai:image",
        request_body=original,
        context={"mapped_model": ..., "multipart": ..., "operation": ...},
    )

If no transformer is registered, returns the original body unchanged.
"""

from __future__ import annotations

from typing import Any, Callable

from src.core.provider_types import normalize_provider_type

BodyTransformerFn = Callable[..., dict[str, Any]]

_transformers: dict[tuple[str, str], BodyTransformerFn] = {}


def register_body_transformer(
    provider_type: str,
    endpoint_sig: str,
    transformer: BodyTransformerFn,
) -> None:
    """Register a provider-specific request body transformer."""
    pt = normalize_provider_type(provider_type)
    sig = str(endpoint_sig or "").strip().lower()
    if not pt or not sig:
        return
    _transformers[(pt, sig)] = transformer


def get_body_transformer(
    provider_type: str | None,
    endpoint_sig: str | None,
) -> BodyTransformerFn | None:
    pt = normalize_provider_type(provider_type)
    sig = str(endpoint_sig or "").strip().lower()
    if not pt or not sig:
        return None
    return _transformers.get((pt, sig))


def transform_request_body(
    *,
    provider_type: str | None,
    endpoint_sig: str | None,
    request_body: dict[str, Any],
    context: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """Apply registered transformer if any; otherwise return body unchanged."""
    transformer = get_body_transformer(provider_type, endpoint_sig)
    if transformer is None:
        return request_body
    return transformer(request_body, context or {})


__all__ = [
    "BodyTransformerFn",
    "register_body_transformer",
    "get_body_transformer",
    "transform_request_body",
]
