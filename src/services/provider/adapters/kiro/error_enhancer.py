"""Kiro HTTP/network error classification helpers."""

from __future__ import annotations

import json

import httpx

_KNOWN_REASON_MESSAGES: dict[str, str] = {
    "CONTENT_LENGTH_EXCEEDS_THRESHOLD": "输入超过模型上下文限制",
    "MONTHLY_REQUEST_COUNT": "账户已达到月度请求配额",
}


def classify_kiro_http_status(status_code: int) -> str:
    """Classify upstream HTTP status into stable buckets."""
    if 200 <= status_code < 300:
        return "ok"
    if status_code in {401, 403}:
        return "auth_error"
    if status_code == 429:
        return "rate_limited"
    if status_code in {408, 504}:
        return "timeout"
    if 500 <= status_code < 600:
        return "upstream_server_error"
    if 400 <= status_code < 500:
        return "upstream_client_error"
    return "unexpected_status"


def classify_kiro_connection_error(exc: Exception) -> str:
    """Classify transport exceptions raised by httpx."""
    if isinstance(exc, httpx.ConnectTimeout):
        return "connect_timeout"
    if isinstance(exc, httpx.ReadTimeout):
        return "read_timeout"
    if isinstance(exc, httpx.WriteTimeout):
        return "write_timeout"
    if isinstance(exc, httpx.PoolTimeout):
        return "pool_timeout"
    if isinstance(exc, httpx.TimeoutException):
        return "timeout"
    if isinstance(exc, httpx.ConnectError):
        return "connect_error"
    return "network_error"


def summarize_kiro_connection_error(exc: Exception) -> str:
    """Build a compact diagnostic string safe for logs/errors."""
    category = classify_kiro_connection_error(exc)
    detail = str(exc).strip()
    if len(detail) > 200:
        detail = detail[:200]
    if detail:
        return f"{category}: {type(exc).__name__}: {detail}"
    return f"{category}: {type(exc).__name__}"


def build_kiro_network_diagnostic(
    *,
    http_status: int | None,
    http_category: str | None,
    connection_summary: str | None,
) -> str | None:
    """Build short supplemental diagnostic text for user-facing error paths."""
    if connection_summary:
        return f"network={connection_summary}"
    if http_status is None:
        return None
    category = str(http_category or "unknown").strip() or "unknown"
    return f"http_status={http_status} ({category})"


def parse_kiro_error_text(raw_text: str | None) -> dict[str, str]:
    result = {
        "type": "",
        "reason": "",
        "message": "",
        "raw": str(raw_text or "").strip(),
    }
    if not result["raw"]:
        return result

    try:
        data = json.loads(result["raw"])
    except Exception:
        result["message"] = result["raw"]
        return result

    error_obj = data.get("error")
    if isinstance(error_obj, dict):
        result["type"] = str(error_obj.get("type") or error_obj.get("__type") or "").strip()
        result["reason"] = str(error_obj.get("reason") or error_obj.get("code") or "").strip()
        message = error_obj.get("message")
        if isinstance(message, str) and message.strip():
            result["message"] = message.strip()

    if not result["message"]:
        message = data.get("message")
        if isinstance(message, str) and message.strip():
            result["message"] = message.strip()

    if not result["reason"]:
        reason = data.get("reason") or data.get("code")
        if isinstance(reason, str) and reason.strip():
            result["reason"] = reason.strip()

    if not result["message"]:
        result["message"] = result["raw"]

    return result


def enhance_kiro_http_error_text(
    raw_text: str | None,
    *,
    status_code: int | None = None,
) -> str:
    parsed = parse_kiro_error_text(raw_text)
    reason = parsed["reason"].upper()
    type_name = parsed["type"]
    message = parsed["message"]

    friendly_message = _KNOWN_REASON_MESSAGES.get(reason)
    if friendly_message:
        message = friendly_message
    elif status_code == 403 and "access denied" in message.lower():
        message = "Kiro 账户权限被拒绝"
    elif status_code == 429 and not reason:
        message = "Kiro 请求过于频繁，请稍后重试"

    parts: list[str] = []
    if type_name:
        parts.append(type_name)
    if reason:
        parts.append(f"[{reason}]")
    if message:
        parts.append(message)

    return ": ".join(parts) if parts else parsed["raw"]


async def extract_kiro_http_error_text(
    source: httpx.Response | httpx.HTTPStatusError,
    *,
    limit: int = 4000,
) -> str:
    response = source.response if isinstance(source, httpx.HTTPStatusError) else source

    raw_text = ""
    try:
        if hasattr(response, "is_stream_consumed") and not response.is_stream_consumed:
            error_bytes = await response.aread()
            for encoding in ("utf-8", "gbk", "latin1"):
                try:
                    raw_text = error_bytes.decode(encoding)
                    break
                except (UnicodeDecodeError, LookupError):
                    continue
            if not raw_text:
                raw_text = error_bytes.decode("utf-8", errors="replace")
        else:
            raw_text = response.text if hasattr(response, "_content") else ""
    except Exception as exc:
        return f"Unable to read Kiro error response: {exc}"

    raw_text = (raw_text or "")[:limit]
    if not raw_text:
        return ""
    return enhance_kiro_http_error_text(raw_text, status_code=response.status_code)


__all__ = [
    "build_kiro_network_diagnostic",
    "classify_kiro_connection_error",
    "classify_kiro_http_status",
    "enhance_kiro_http_error_text",
    "extract_kiro_http_error_text",
    "parse_kiro_error_text",
    "summarize_kiro_connection_error",
]
