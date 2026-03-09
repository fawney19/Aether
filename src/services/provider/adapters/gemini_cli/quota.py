"""Gemini CLI quota / RESOURCE_EXHAUSTED helpers."""

from __future__ import annotations

import json
import re
import time
from datetime import datetime, timezone
from typing import Any
from uuid import UUID

_DURATION_TOKEN_RE = re.compile(r"(\d+(?:\.\d+)?)([dhms])")
_RESET_AFTER_RE = re.compile(r"reset after\s+([^.,;]+)", re.IGNORECASE)


def _parse_json(error_text: str | None) -> dict[str, Any] | None:
    if not isinstance(error_text, str) or not error_text.strip():
        return None
    try:
        data = json.loads(error_text)
    except Exception:
        return None
    return data if isinstance(data, dict) else None


def _parse_duration_seconds(raw: Any) -> int | None:
    if isinstance(raw, (int, float)):
        return max(1, int(raw))
    if not isinstance(raw, str):
        return None
    text = raw.strip().lower()
    if not text:
        return None

    total_seconds = 0.0
    matched = False
    for amount_text, unit in _DURATION_TOKEN_RE.findall(text):
        matched = True
        amount = float(amount_text)
        if unit == "d":
            total_seconds += amount * 86400
        elif unit == "h":
            total_seconds += amount * 3600
        elif unit == "m":
            total_seconds += amount * 60
        elif unit == "s":
            total_seconds += amount
    if not matched:
        return None
    return max(1, int(total_seconds))


def _iter_error_details(payload: dict[str, Any]) -> list[dict[str, Any]]:
    error_obj = payload.get("error")
    if not isinstance(error_obj, dict):
        return []
    details = error_obj.get("details")
    if not isinstance(details, list):
        return []
    return [item for item in details if isinstance(item, dict)]


def _error_status(payload: dict[str, Any]) -> str:
    error_obj = payload.get("error")
    if not isinstance(error_obj, dict):
        return ""
    status = error_obj.get("status")
    return status.strip() if isinstance(status, str) else ""


def _error_message(payload: dict[str, Any]) -> str:
    error_obj = payload.get("error")
    if not isinstance(error_obj, dict):
        return ""
    message = error_obj.get("message")
    return message.strip() if isinstance(message, str) else ""


def _error_reason(payload: dict[str, Any]) -> str:
    for detail in _iter_error_details(payload):
        reason = detail.get("reason")
        if isinstance(reason, str) and reason.strip():
            return reason.strip()
    return ""


def _looks_like_uuid(value: str | None) -> bool:
    if not isinstance(value, str):
        return False
    text = value.strip()
    if not text:
        return False
    try:
        UUID(text)
    except Exception:
        return False
    return True


def is_resource_exhausted_error(error_text: str | None) -> bool:
    payload = _parse_json(error_text)
    if payload is None:
        return False

    status = _error_status(payload).upper()
    reason = _error_reason(payload).upper()
    if status == "RESOURCE_EXHAUSTED" or reason == "QUOTA_EXHAUSTED":
        return True

    message = _error_message(payload).lower()
    return "exhausted your capacity" in message or "quota" in message and "exhaust" in message


def parse_quota_reset_timestamp(error_text: str | None) -> int | None:
    payload = _parse_json(error_text)
    if payload is None:
        return None

    for detail in _iter_error_details(payload):
        metadata = detail.get("metadata")
        if not isinstance(metadata, dict):
            continue
        raw = metadata.get("quotaResetTimeStamp") or metadata.get("quotaResetTimestamp")
        if not isinstance(raw, str) or not raw.strip():
            continue
        text = raw.strip()
        try:
            if text.endswith("Z"):
                text = text[:-1] + "+00:00"
            parsed = datetime.fromisoformat(text)
            if parsed.tzinfo is None:
                parsed = parsed.replace(tzinfo=timezone.utc)
            return int(parsed.timestamp())
        except Exception:
            continue
    return None


def parse_quota_reset_delay_seconds(error_text: str | None) -> int | None:
    payload = _parse_json(error_text)
    if payload is None:
        return None

    for detail in _iter_error_details(payload):
        metadata = detail.get("metadata")
        if not isinstance(metadata, dict):
            continue
        delay = metadata.get("quotaResetDelay")
        parsed = _parse_duration_seconds(delay)
        if parsed is not None:
            return parsed
    return None


def parse_quota_reset_message_seconds(error_text: str | None) -> int | None:
    payload = _parse_json(error_text)
    if payload is None:
        return None

    message = _error_message(payload)
    if not message:
        return None

    matched = _RESET_AFTER_RE.search(message)
    if not matched:
        return None

    return _parse_duration_seconds(matched.group(1))


def extract_error_model_name(error_text: str | None, *, fallback: str | None = None) -> str | None:
    payload = _parse_json(error_text)
    if payload is not None:
        for detail in _iter_error_details(payload):
            metadata = detail.get("metadata")
            if not isinstance(metadata, dict):
                continue
            model = metadata.get("model")
            if isinstance(model, str) and model.strip():
                return model.strip()

    fallback_text = str(fallback or "").strip()
    if fallback_text and not _looks_like_uuid(fallback_text):
        return fallback_text
    return None


def extract_quota_cooldown_seconds(error_text: str | None, *, now_ts: int | None = None) -> int | None:
    now = int(now_ts or time.time())

    reset_at = parse_quota_reset_timestamp(error_text)
    if reset_at is not None:
        return max(1, reset_at - now)

    delay = parse_quota_reset_delay_seconds(error_text)
    if delay is not None:
        return max(1, delay)

    message_delay = parse_quota_reset_message_seconds(error_text)
    if message_delay is not None:
        return max(1, message_delay)

    return None


def build_quota_exhausted_metadata(
    *,
    model_name: str,
    error_text: str | None,
    current_namespace: dict[str, Any] | None = None,
    now_ts: int | None = None,
) -> dict[str, Any] | None:
    normalized_model = str(model_name or "").strip()
    if not normalized_model:
        return None
    if not is_resource_exhausted_error(error_text):
        return None

    now = int(now_ts or time.time())
    reset_at = parse_quota_reset_timestamp(error_text)
    if reset_at is None:
        delay = parse_quota_reset_delay_seconds(error_text)
        if delay is not None:
            reset_at = now + delay
    if reset_at is None:
        message_delay = parse_quota_reset_message_seconds(error_text)
        if message_delay is not None:
            reset_at = now + message_delay
    if reset_at is None:
        return None

    payload = _parse_json(error_text) or {}
    namespace = dict(current_namespace) if isinstance(current_namespace, dict) else {}
    quota_by_model_raw = namespace.get("quota_by_model")
    quota_by_model = dict(quota_by_model_raw) if isinstance(quota_by_model_raw, dict) else {}
    model_entry_raw = quota_by_model.get(normalized_model)
    model_entry = dict(model_entry_raw) if isinstance(model_entry_raw, dict) else {}

    model_entry["is_exhausted"] = True
    model_entry["remaining_fraction"] = 0.0
    model_entry["used_percent"] = 100.0
    model_entry["updated_at"] = now

    if reset_at is not None:
        model_entry["reset_at"] = reset_at
        model_entry["reset_time"] = datetime.fromtimestamp(reset_at, timezone.utc).isoformat()
        model_entry["reset_seconds"] = max(0, reset_at - now)

    reason = _error_reason(payload) or _error_status(payload) or _error_message(payload)
    if reason:
        model_entry["reason"] = reason

    quota_by_model[normalized_model] = model_entry
    namespace["quota_by_model"] = quota_by_model
    namespace["updated_at"] = now

    status = _error_status(payload)
    if status:
        namespace["last_error_status"] = status
    if reason:
        namespace["last_error_reason"] = reason

    return {"gemini_cli": namespace}


__all__ = [
    "extract_error_model_name",
    "build_quota_exhausted_metadata",
    "extract_quota_cooldown_seconds",
    "is_resource_exhausted_error",
    "parse_quota_reset_message_seconds",
    "parse_quota_reset_delay_seconds",
    "parse_quota_reset_timestamp",
]
