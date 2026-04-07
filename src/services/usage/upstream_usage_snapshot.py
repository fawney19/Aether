from __future__ import annotations

from copy import deepcopy
from typing import Any

from src.core.usage_tokens import extract_cache_creation_tokens_detail, extract_cache_read_tokens


def build_upstream_usage_snapshot(
    response_body: Any,
    *,
    api_family: str | None,
    is_stream: bool,
) -> dict[str, Any] | None:
    if not isinstance(response_body, dict):
        return None

    normalized_family = (api_family or "").strip().lower() or None

    if is_stream:
        chunks = response_body.get("chunks")
        if not isinstance(chunks, list) or not chunks:
            return None

        usage_events: list[dict[str, Any]] = []
        for chunk in chunks:
            if not isinstance(chunk, dict):
                continue
            event = _extract_usage_event(chunk)
            if event is not None:
                usage_events.append(event)

        if not usage_events:
            return None

        return {
            "snapshot_type": "parsed_stream_usage_events",
            "api_family": normalized_family,
            "events": usage_events,
        }

    raw_usage = _extract_raw_usage_object(response_body)
    if raw_usage is None:
        return None

    return {
        "snapshot_type": "response_usage",
        "api_family": normalized_family,
        "usage": raw_usage,
    }


def extract_usage_metrics_from_snapshot(snapshot: Any) -> dict[str, int | None] | None:
    if not isinstance(snapshot, dict):
        return None

    snapshot_type = snapshot.get("snapshot_type")
    api_family = str(snapshot.get("api_family") or "").strip().lower() or None

    if snapshot_type == "response_usage":
        usage = snapshot.get("usage")
        if not isinstance(usage, dict):
            return None
        return _extract_metrics_from_usage_dict(usage, api_family=api_family)

    if snapshot_type == "parsed_stream_usage_events":
        events = snapshot.get("events")
        if not isinstance(events, list):
            return None

        last_metrics: dict[str, int | None] | None = None
        for event in events:
            if not isinstance(event, dict):
                continue
            usage = _extract_raw_usage_object(event)
            if not isinstance(usage, dict):
                continue
            metrics = _extract_metrics_from_usage_dict(usage, api_family=api_family)
            if metrics is not None:
                last_metrics = metrics

        return last_metrics

    return None


def infer_cache_ttl_minutes(
    *,
    snapshot: Any,
    has_cache_tokens: bool,
    explicit_cache_ttl_minutes: int | None = None,
) -> int | None:
    if explicit_cache_ttl_minutes is not None:
        return int(explicit_cache_ttl_minutes)

    metrics = extract_usage_metrics_from_snapshot(snapshot)
    if metrics and metrics.get("cache_ttl_minutes") is not None:
        return int(metrics["cache_ttl_minutes"])

    if has_cache_tokens:
        return 5

    return None


def _extract_usage_event(chunk: dict[str, Any]) -> dict[str, Any] | None:
    if "usage" in chunk and isinstance(chunk.get("usage"), dict):
        return deepcopy(chunk)

    message = chunk.get("message")
    if isinstance(message, dict) and isinstance(message.get("usage"), dict):
        return deepcopy(chunk)

    if "usageMetadata" in chunk and isinstance(chunk.get("usageMetadata"), dict):
        return deepcopy(chunk)

    return None


def _extract_raw_usage_object(payload: dict[str, Any]) -> dict[str, Any] | None:
    if isinstance(payload.get("usage"), dict):
        return deepcopy(payload["usage"])

    message = payload.get("message")
    if isinstance(message, dict) and isinstance(message.get("usage"), dict):
        return deepcopy(message["usage"])

    if isinstance(payload.get("usageMetadata"), dict):
        return deepcopy(payload["usageMetadata"])

    return None


def _extract_metrics_from_usage_dict(
    usage: dict[str, Any],
    *,
    api_family: str | None,
) -> dict[str, int | None] | None:
    if api_family == "gemini" or "usageMetadata" in usage:
        input_tokens = int(
            usage.get("promptTokenCount")
            or usage.get("prompt_tokens")
            or usage.get("input_tokens")
            or 0
        )
        output_tokens = int(
            usage.get("candidatesTokenCount")
            or usage.get("completion_tokens")
            or usage.get("output_tokens")
            or 0
        )
        cache_read_tokens = int(usage.get("cachedContentTokenCount") or usage.get("cached_tokens") or 0)
        return {
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
            "cache_creation_input_tokens": 0,
            "cache_read_input_tokens": cache_read_tokens,
            "cache_ttl_minutes": None,
        }

    input_tokens = int(usage.get("input_tokens") or usage.get("prompt_tokens") or 0)
    output_tokens = int(usage.get("output_tokens") or usage.get("completion_tokens") or 0)
    cache_creation_total, cache_creation_5m, cache_creation_1h = extract_cache_creation_tokens_detail(
        usage
    )
    cache_read_tokens = extract_cache_read_tokens(usage)

    cache_ttl_minutes: int | None = None
    if cache_creation_1h > 0:
        cache_ttl_minutes = 60
    elif cache_creation_total > 0:
        cache_ttl_minutes = 5

    return {
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "cache_creation_input_tokens": cache_creation_total,
        "cache_read_input_tokens": cache_read_tokens,
        "cache_ttl_minutes": cache_ttl_minutes,
    }
