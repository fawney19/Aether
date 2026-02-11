"""Upstream stream bridging helpers (handler layer).

This module provides small utilities used when handler-layer policies force an
upstream request to be streaming (SSE) even when the client asked for sync.

It intentionally stays lightweight and works with:
- standard SSE `data: {...}` lines (OpenAI/Claude/etc.)
- Gemini CLI JSON-array lines (best-effort)
"""

from __future__ import annotations

import codecs
from collections.abc import AsyncIterator
from typing import Any

from src.api.handlers.base.response_parser import ResponseParser
from src.api.handlers.base.utils import get_format_converter_registry
from src.core.api_format.conversion.internal import InternalResponse
from src.core.api_format.conversion.stream_bridge import InternalStreamAggregator
from src.core.api_format.conversion.stream_state import StreamState
from src.core.exceptions import EmbeddedErrorException
from src.core.logger import logger
from src.services.provider.envelope import ProviderEnvelope
from src.utils import json_helper as json


def _parse_sse_data_line(line: str) -> tuple[Any | None, str]:
    """Parse `data: {...}` as JSON."""
    payload = line[5:].strip()
    if not payload:
        return None, "empty"
    try:
        return json.loads(payload), "ok"
    except json.JSONDecodeError:
        return None, "invalid"


def _parse_sse_event_data_line(line: str) -> tuple[Any | None, str]:
    """Parse `event: xxx data: {...}` as JSON (best-effort)."""
    # Split only on the first " data:" occurrence.
    try:
        _, data_part = line.split(" data:", 1)
    except ValueError:
        return None, "invalid"
    payload = data_part.strip()
    if not payload:
        return None, "empty"
    try:
        return json.loads(payload), "ok"
    except json.JSONDecodeError:
        return None, "invalid"


def _parse_gemini_json_array_line(line: str) -> tuple[Any | None, str]:
    """Parse Gemini CLI JSON-array streaming line (best-effort).

    Gemini CLI may stream objects in a JSON array form like:
    - "[{...},"
    - " {...},"
    - " {...}]"
    """
    stripped = (line or "").strip()
    if not stripped:
        return None, "empty"

    # Quick filter: must contain a JSON object boundary.
    if "{" not in stripped:
        return None, "skip"

    candidate = stripped.lstrip(",").rstrip(",").strip()
    # Drop array brackets on edges.
    if candidate.startswith("["):
        candidate = candidate[1:].strip()
    if candidate.endswith("]"):
        candidate = candidate[:-1].strip()
    candidate = candidate.lstrip(",").rstrip(",").strip()

    if not candidate:
        return None, "empty"
    try:
        return json.loads(candidate), "ok"
    except json.JSONDecodeError:
        logger.debug(f"Gemini JSON-array line skip: {stripped[:50]}")
        return None, "invalid"


def parse_provider_stream_line_to_json(
    line: str,
    provider_format: str,
) -> tuple[Any | None, str]:
    """Best-effort parse for upstream streaming lines (SSE or Gemini JSON-array)."""

    if not line:
        return None, "skip"

    normalized = line.rstrip("\r").strip("\n")
    if not normalized or normalized.strip() == "":
        return None, "skip"

    # Standard SSE data line.
    if normalized.startswith("data:"):
        # `data: [DONE]` is a sentinel.
        if normalized[5:].strip() == "[DONE]":
            return None, "skip"
        return _parse_sse_data_line(normalized)

    # event + data on same line.
    if normalized.startswith("event:") and " data:" in normalized:
        return _parse_sse_event_data_line(normalized)

    # Other control lines.
    if normalized.startswith(("event:", "id:", "retry:")):
        return None, "skip"

    # Gemini JSON-array/chunked streaming (no SSE prefix).
    if str(provider_format or "").strip().lower().startswith("gemini"):
        return _parse_gemini_json_array_line(normalized)

    return None, "skip"


async def aggregate_upstream_stream_to_internal_response(
    byte_iter: AsyncIterator[bytes],
    *,
    provider_api_format: str,
    provider_name: str,
    model: str,
    request_id: str,
    envelope: ProviderEnvelope | None = None,
    provider_parser: ResponseParser | None = None,
) -> InternalResponse:
    """Aggregate upstream SSE/streaming bytes into an InternalResponse (best-effort)."""

    registry = get_format_converter_registry()
    src_norm = registry.get_normalizer(provider_api_format) if provider_api_format else None
    if src_norm is None:
        raise RuntimeError(f"未注册 Normalizer: {provider_api_format}")
    if not getattr(src_norm, "capabilities", None) or not src_norm.capabilities.supports_stream:
        raise RuntimeError(f"上游格式不支持流式: {provider_api_format}")

    state = StreamState(model=str(model or ""), message_id=str(request_id or ""))
    aggregator = InternalStreamAggregator(
        fallback_id=str(request_id or "resp"),
        fallback_model=str(model or ""),
    )

    buffer = b""
    decoder = codecs.getincrementaldecoder("utf-8")(errors="replace")

    def _feed_line(normalized_line: str) -> None:
        data_obj, st = parse_provider_stream_line_to_json(normalized_line, provider_api_format)
        if st != "ok" or data_obj is None:
            return
        if not isinstance(data_obj, dict):
            return

        if envelope:
            unwrapped = envelope.unwrap_response(data_obj)
            if not isinstance(unwrapped, dict):
                return
            data_obj = unwrapped
            envelope.postprocess_unwrapped_response(model=model, data=data_obj)

        if provider_parser and provider_parser.is_error_response(data_obj):
            parsed = provider_parser.parse_response(data_obj, 200)
            raise EmbeddedErrorException(
                provider_name=str(provider_name),
                error_code=parsed.embedded_status_code,
                error_message=parsed.error_message,
                error_status=parsed.error_type,
            )

        internal_events = src_norm.stream_chunk_to_internal(data_obj, state)
        aggregator.feed(internal_events)

    async for chunk in byte_iter:
        buffer += chunk
        while b"\n" in buffer:
            line_bytes, buffer = buffer.split(b"\n", 1)
            line = decoder.decode(line_bytes + b"\n", False).rstrip("\n")
            normalized_line = line.rstrip("\r")

            _feed_line(normalized_line)

    # Flush remaining buffered bytes (in case upstream doesn't end with newline).
    if buffer:
        try:
            tail = decoder.decode(buffer, True)
        except Exception:
            tail = ""
        normalized_tail = (tail or "").rstrip("\r\n")
        if normalized_tail:
            _feed_line(normalized_tail)

    return aggregator.build()


__all__ = [
    "aggregate_upstream_stream_to_internal_response",
    "parse_provider_stream_line_to_json",
]
