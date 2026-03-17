"""Helpers for stable outbound request cache fingerprints."""

from __future__ import annotations

import hashlib
import json
from collections.abc import Mapping
from enum import Enum
from typing import Any

# model 和 prompt_cache_key 统一包含在所有格式中，无需运行时动态添加
_CACHE_RELEVANT_FIELDS_BY_FORMAT: dict[str, frozenset[str]] = {
    "openai:chat": frozenset({"model", "messages", "tools", "tool_choice", "prompt_cache_key"}),
    "openai:cli": frozenset(
        {"model", "input", "instructions", "tools", "tool_choice", "prompt_cache_key"}
    ),
    "openai:compact": frozenset(
        {"model", "input", "instructions", "tools", "tool_choice", "prompt_cache_key"}
    ),
    "claude:chat": frozenset(
        {"model", "system", "messages", "tools", "tool_choice", "prompt_cache_key"}
    ),
    "gemini:chat": frozenset(
        {
            "model",
            "contents",
            "system_instruction",
            "systemInstruction",
            "tools",
            "tool_config",
            "toolConfig",
            "generation_config",
            "generationConfig",
            "prompt_cache_key",
        }
    ),
}


def _normalize_for_hash(value: Any) -> Any:
    if value is None or isinstance(value, (str, int, float, bool)):
        return value
    if isinstance(value, bytes):
        return value.decode("utf-8", errors="replace")
    if isinstance(value, Enum):
        return _normalize_for_hash(value.value)
    if isinstance(value, Mapping):
        return {str(key): _normalize_for_hash(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [_normalize_for_hash(item) for item in value]
    if isinstance(value, (set, frozenset)):
        normalized = [_normalize_for_hash(item) for item in value]
        return sorted(normalized, key=_stable_json_dumps)
    return str(value)


def _stable_json_dumps(value: Any) -> str:
    return json.dumps(
        _normalize_for_hash(value),
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
    )


def _hash_json_payload(value: Any) -> tuple[str, int]:
    """Return (sha256_hex, json_byte_length) for the canonicalized JSON."""
    payload = _stable_json_dumps(value).encode("utf-8")
    return hashlib.sha256(payload).hexdigest(), len(payload)


def _normalize_provider_api_format(provider_api_format: str | None) -> str | None:
    normalized = str(provider_api_format or "").strip().lower()
    return normalized or None


def _get_prompt_cache_key(payload: Any) -> str | None:
    if not isinstance(payload, Mapping):
        return None
    prompt_cache_key = str(payload.get("prompt_cache_key") or "").strip()
    return prompt_cache_key or None


def _extract_cache_relevant_payload(
    payload: Any, provider_api_format: str | None
) -> tuple[Any, list[str]]:
    if not isinstance(payload, Mapping):
        return payload, []

    fields = _CACHE_RELEVANT_FIELDS_BY_FORMAT.get(provider_api_format or "")
    if not fields:
        # 未知格式：整个 payload 参与哈希
        top_level_keys = sorted(str(key) for key in payload.keys())
        return dict(payload), top_level_keys

    subset = {field: payload[field] for field in fields if field in payload}
    if not subset:
        top_level_keys = sorted(str(key) for key in payload.keys())
        return dict(payload), top_level_keys

    return subset, sorted(subset.keys())


def _build_field_fingerprints(payload: Any, field_names: list[str]) -> dict[str, dict[str, Any]]:
    if not isinstance(payload, Mapping) or not field_names:
        return {}

    field_fingerprints: dict[str, dict[str, Any]] = {}
    for field_name in field_names:
        if field_name not in payload:
            continue
        field_sha256, field_bytes = _hash_json_payload(payload[field_name])
        field_fingerprints[field_name] = {
            "sha256": field_sha256,
            "bytes": field_bytes,
        }
    return field_fingerprints


def build_request_cache_fingerprint(
    provider_request_body: Any,
    *,
    provider_api_format: str | None = None,
) -> dict[str, Any] | None:
    """Build stable hashes for the final outbound payload and its cache-relevant subset."""
    if provider_request_body is None:
        return None

    normalized_format = _normalize_provider_api_format(provider_api_format)
    payload_sha256, payload_bytes = _hash_json_payload(provider_request_body)
    cache_relevant_payload, cache_relevant_keys = _extract_cache_relevant_payload(
        provider_request_body,
        normalized_format,
    )
    cache_relevant_sha256, cache_relevant_bytes = _hash_json_payload(cache_relevant_payload)
    field_fingerprints = _build_field_fingerprints(cache_relevant_payload, cache_relevant_keys)

    top_level_keys = []
    if isinstance(provider_request_body, Mapping):
        top_level_keys = sorted(str(key) for key in provider_request_body.keys())

    fingerprint: dict[str, Any] = {
        "version": 2,
        "provider_api_format": normalized_format,
        "payload_sha256": payload_sha256,
        "payload_bytes": payload_bytes,
        "cache_relevant_sha256": cache_relevant_sha256,
        "cache_relevant_bytes": cache_relevant_bytes,
        "top_level_keys": top_level_keys,
        "cache_relevant_keys": cache_relevant_keys,
        "field_fingerprints": field_fingerprints,
    }

    prompt_cache_key = _get_prompt_cache_key(provider_request_body)
    if prompt_cache_key:
        fingerprint["prompt_cache_key"] = prompt_cache_key

    return fingerprint


__all__ = ["build_request_cache_fingerprint"]
