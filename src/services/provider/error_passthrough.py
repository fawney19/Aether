from __future__ import annotations

import json
import re
from dataclasses import dataclass
from typing import Any

_COMPILED_CACHE_KEY = "_compiled_error_passthrough_patterns"
_MAX_CLIENT_ERROR_MESSAGE_LENGTH = 800
_SENSITIVE_INLINE_PATTERN = re.compile(
    r"(?i)\b(api[_-]?key|token)\b([\"']?\s*[:=]\s*[\"']?)([^\s,\"'}]+)"
)
_SENSITIVE_AUTH_PATTERN = re.compile(
    r"(?i)\bauthorization\b([\"']?\s*[:=]\s*[\"']?)(Bearer\s+[A-Za-z0-9._~+/=-]+|[^\s,\"'}]+)"
)
_SENSITIVE_BEARER_PATTERN = re.compile(r"(?i)\bBearer\s+[A-Za-z0-9._~+/=-]+\b")


@dataclass(slots=True, frozen=True)
class ErrorPassthroughMatch:
    pattern: str
    message: str


def _compile_patterns(
    rules: dict[str, Any],
) -> list[tuple[re.Pattern[str], dict[str, Any]]]:
    cached = rules.get(_COMPILED_CACHE_KEY)
    if cached is not None:
        return cached

    compiled: list[tuple[re.Pattern[str], dict[str, Any]]] = []
    for rule in rules.get("patterns", []):
        if not isinstance(rule, dict):
            continue
        pattern = str(rule.get("pattern", "") or "").strip()
        if not pattern:
            continue
        try:
            compiled.append((re.compile(pattern), rule))
        except re.error:
            continue

    rules[_COMPILED_CACHE_KEY] = compiled
    return compiled


def _sanitize_message(message: str) -> str:
    sanitized = _SENSITIVE_AUTH_PATTERN.sub(r"authorization\1[REDACTED]", message)
    sanitized = _SENSITIVE_INLINE_PATTERN.sub(r"\1\2[REDACTED]", sanitized)
    sanitized = _SENSITIVE_BEARER_PATTERN.sub("Bearer [REDACTED]", sanitized)
    sanitized = re.sub(r"\s+", " ", sanitized).strip()
    return sanitized[:_MAX_CLIENT_ERROR_MESSAGE_LENGTH]


def _stringify_json(value: Any) -> str | None:
    try:
        rendered = json.dumps(value, ensure_ascii=False, separators=(",", ":"))
    except Exception:
        rendered = str(value)
    rendered = rendered.strip()
    return rendered or None


def _extract_labelled_message(payload: dict[str, Any]) -> str | None:
    error_obj = payload.get("error")
    if isinstance(error_obj, dict):
        label = (
            str(error_obj.get("code") or "").strip()
            or str(error_obj.get("reason") or "").strip()
            or str(error_obj.get("status") or "").strip()
            or str(error_obj.get("type") or "").strip()
            or str(error_obj.get("__type") or "").strip()
        )
        message = (
            str(error_obj.get("message") or "").strip()
            or str(error_obj.get("detail") or "").strip()
            or str(error_obj.get("error") or "").strip()
        )
        if message and label and label.lower() not in message.lower():
            return f"{label}: {message}"
        return message or label or None

    if isinstance(error_obj, str):
        return error_obj.strip() or None

    for key in ("message", "detail", "errorMessage", "msg", "reason", "description"):
        value = payload.get(key)
        if isinstance(value, str) and value.strip():
            return value.strip()

    errors_value = payload.get("errors")
    if isinstance(errors_value, list) and errors_value:
        first_item = errors_value[0]
        if isinstance(first_item, str) and first_item.strip():
            return first_item.strip()
        if isinstance(first_item, dict):
            nested = _extract_labelled_message(first_item)
            if nested:
                return nested
            return _stringify_json(first_item)

    return _stringify_json(payload)


def extract_upstream_error_message(response_text: str | None) -> str | None:
    """从上游错误响应中提取适合返回给客户端的错误消息。"""
    if not response_text or not isinstance(response_text, str):
        return None

    raw_text = response_text.strip()
    if not raw_text:
        return None

    message: str | None = None
    try:
        parsed = json.loads(raw_text)
    except Exception:
        message = raw_text
    else:
        if isinstance(parsed, dict):
            message = _extract_labelled_message(parsed)
        elif isinstance(parsed, list):
            message = _stringify_json(parsed)
        elif parsed is not None:
            message = str(parsed).strip()

    if not message:
        return None
    return _sanitize_message(message)


def match_error_passthrough_rule(
    provider_config: dict[str, Any] | None,
    *,
    response_text: str | None,
    status_code: int | None,
) -> ErrorPassthroughMatch | None:
    """按 provider 规则匹配是否允许将上游错误信息透传给客户端。"""
    if not isinstance(provider_config, dict):
        return None

    raw_rules = provider_config.get("error_passthrough_rules")
    if not isinstance(raw_rules, dict):
        return None

    compiled_rules = _compile_patterns(raw_rules)
    if not compiled_rules:
        return None

    response_body = str(response_text or "").strip()
    if not response_body:
        return None

    for regex, rule in compiled_rules:
        rule_status_codes = rule.get("status_codes")
        if isinstance(rule_status_codes, list) and rule_status_codes:
            if status_code is None or status_code not in rule_status_codes:
                continue
        if not regex.search(response_body):
            continue

        message = extract_upstream_error_message(response_body)
        if not message:
            continue
        return ErrorPassthroughMatch(
            pattern=str(rule.get("pattern", "") or ""),
            message=message,
        )

    return None
