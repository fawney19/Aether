"""Claude Code upstream envelope hooks."""

from __future__ import annotations

import copy
import platform
from dataclasses import replace
from typing import Any

from src.config.settings import config
from src.core.logger import logger
from src.services.provider.adapters.claude_code.context import (
    ClaudeCodeRequestContext,
    get_claude_code_request_context,
    set_claude_code_request_context,
)
from src.services.provider.adapters.claude_code.retry_policy import (
    is_retryable_status,
    should_retry_same_candidate,
)
from src.services.provider.adapters.claude_code.tool_prefix import (
    CLAUDE_TOOL_PREFIX,
    apply_claude_tool_prefix,
    apply_claude_tool_prefix_with_alias_map,
    strip_claude_tool_prefix_from_response,
)
from src.services.provider.request_context import get_selected_base_url

_PROMPT_CACHING_BETA = "prompt-caching-2024-07-31"
_CLAUDE_DEFAULT_BETAS: tuple[str, ...] = (
    "claude-code-20250219",
    "oauth-2025-04-20",
    "interleaved-thinking-2025-05-14",
    "fine-grained-tool-streaming-2025-05-14",
    _PROMPT_CACHING_BETA,
)

_OPENAI_ONLY_REQUEST_FIELDS: tuple[str, ...] = (
    "frequency_penalty",
    "presence_penalty",
    "logprobs",
    "top_logprobs",
    "n",
    "response_format",
)


def _parse_bool(value: Any) -> bool:
    if isinstance(value, bool):
        return value
    if isinstance(value, (int, float)):
        return bool(value)
    if isinstance(value, str):
        return value.strip().lower() in {"1", "true", "yes", "on", "enabled"}
    return False


def _normalize_auth_method(auth_config: dict[str, Any]) -> str:
    raw = str(auth_config.get("auth_method", "") or "").strip().lower()
    if raw in {"api_key", "apikey", "x-api-key"}:
        return "api_key"
    if raw in {"oauth"}:
        return "oauth"
    if raw in {"bearer"}:
        return "bearer"
    if auth_config.get("refresh_token"):
        return "oauth"
    return "bearer"


def _parse_beta_tokens(value: Any) -> list[str]:
    tokens: list[str] = []

    def _append_token(raw: Any) -> None:
        text = str(raw or "").strip()
        if not text:
            return
        for part in text.split(","):
            token = part.strip()
            if token:
                tokens.append(token)

    if isinstance(value, (list, tuple, set)):
        for item in value:
            _append_token(item)
    else:
        _append_token(value)

    deduped: list[str] = []
    seen: set[str] = set()
    for token in tokens:
        if token in seen:
            continue
        seen.add(token)
        deduped.append(token)
    return deduped


def merge_anthropic_beta_headers(existing_value: str | None, computed_value: str | None) -> str:
    merged = _parse_beta_tokens(existing_value)
    for token in _parse_beta_tokens(computed_value):
        if token not in merged:
            merged.append(token)
    return ",".join(merged)


def _extract_betas_from_body(request_body: dict[str, Any]) -> tuple[dict[str, Any], tuple[str, ...]]:
    copied = copy.deepcopy(request_body)
    combined_betas: list[str] = []
    for key in ("betas", "anthropic_beta", "anthropic-beta"):
        combined_betas.extend(_parse_beta_tokens(copied.pop(key, None)))
    deduped = _parse_beta_tokens(combined_betas)
    return copied, tuple(deduped)


def _coerce_positive_int(value: Any) -> int | None:
    try:
        parsed = int(value)
    except Exception:
        return None
    if parsed <= 0:
        return None
    return parsed


def _normalize_stop_sequences(request_body: dict[str, Any]) -> None:
    if "stop_sequences" in request_body:
        return
    stop = request_body.pop("stop", None)
    if stop is None:
        return
    if isinstance(stop, str):
        normalized = [stop] if stop != "" else []
    elif isinstance(stop, (list, tuple, set)):
        normalized = []
        for item in stop:
            if item is None:
                continue
            text = item if isinstance(item, str) else str(item)
            if text == "":
                continue
            normalized.append(text)
    else:
        normalized = []
    if normalized:
        request_body["stop_sequences"] = normalized


def _normalize_tool_choice(request_body: dict[str, Any]) -> None:
    raw = request_body.get("tool_choice")
    if not isinstance(raw, str):
        return
    normalized = raw.strip().lower()
    mapping = {
        "auto": "auto",
        "none": "none",
        "any": "any",
        "required": "any",
    }
    tool_type = mapping.get(normalized)
    if tool_type:
        request_body["tool_choice"] = {"type": tool_type}


def _normalize_max_tokens(request_body: dict[str, Any]) -> None:
    if _coerce_positive_int(request_body.get("max_tokens")):
        request_body["max_tokens"] = int(request_body["max_tokens"])
        return
    for alias in ("max_completion_tokens", "max_tokens_to_sample"):
        value = _coerce_positive_int(request_body.get(alias))
        if value:
            request_body["max_tokens"] = value
            break
    request_body.pop("max_completion_tokens", None)
    request_body.pop("max_tokens_to_sample", None)


def _normalize_tool_names_in_place(request_body: dict[str, Any]) -> None:
    tools = request_body.get("tools")
    if isinstance(tools, list):
        for tool in tools:
            if isinstance(tool, dict) and isinstance(tool.get("name"), str):
                tool["name"] = tool["name"].strip()

    tool_choice = request_body.get("tool_choice")
    if isinstance(tool_choice, dict) and isinstance(tool_choice.get("name"), str):
        tool_choice["name"] = tool_choice["name"].strip()

    messages = request_body.get("messages")
    if not isinstance(messages, list):
        return
    for message in messages:
        if not isinstance(message, dict):
            continue
        content = message.get("content")
        if not isinstance(content, list):
            continue
        for block in content:
            if not isinstance(block, dict):
                continue
            if isinstance(block.get("name"), str):
                block["name"] = block["name"].strip()
            if isinstance(block.get("tool_name"), str):
                block["tool_name"] = block["tool_name"].strip()
            nested = block.get("content")
            if not isinstance(nested, list):
                continue
            for nested_block in nested:
                if not isinstance(nested_block, dict):
                    continue
                if isinstance(nested_block.get("tool_name"), str):
                    nested_block["tool_name"] = nested_block["tool_name"].strip()


def _normalize_claude_code_request_shape(request_body: dict[str, Any]) -> dict[str, Any]:
    normalized = copy.deepcopy(request_body)

    _normalize_max_tokens(normalized)
    _normalize_stop_sequences(normalized)
    _normalize_tool_choice(normalized)
    _normalize_tool_names_in_place(normalized)

    # 兼容 tuple 输入（某些上游转换链会产生不可变序列）
    if isinstance(normalized.get("messages"), tuple):
        normalized["messages"] = list(normalized["messages"])

    normalized["stream"] = bool(normalized.get("stream"))

    # OpenAI-only 字段在 Claude Code upstream 会触发 400，提前移除。
    for field_name in _OPENAI_ONLY_REQUEST_FIELDS:
        normalized.pop(field_name, None)

    return normalized


def _map_stainless_os() -> str:
    name = platform.system().strip().lower()
    if name == "darwin":
        return "MacOS"
    if name == "windows":
        return "Windows"
    if name == "linux":
        return "Linux"
    if name == "freebsd":
        return "FreeBSD"
    return f"Other::{name or 'unknown'}"


def _map_stainless_arch() -> str:
    machine = platform.machine().strip().lower()
    if machine in {"x86_64", "amd64"}:
        return "x64"
    if machine in {"arm64", "aarch64"}:
        return "arm64"
    if machine in {"i386", "i686", "x86"}:
        return "x86"
    return f"other::{machine or 'unknown'}"


class ClaudeCodeEnvelope:
    """Provider envelope hooks for Claude Code upstream."""

    name = "claude-code:messages"

    def extra_headers(self) -> dict[str, str] | None:
        ctx = get_claude_code_request_context()
        is_stream = bool(ctx.is_stream) if ctx else False
        body_betas = tuple(ctx.body_betas) if ctx else tuple()
        all_betas = merge_anthropic_beta_headers(
            ",".join(_CLAUDE_DEFAULT_BETAS),
            ",".join(body_betas),
        )

        user_agent = str(
            getattr(config, "internal_user_agent_claude_cli", "") or "claude-code/1.0.1"
        ).strip()
        if not user_agent:
            user_agent = "claude-code/1.0.1"

        headers: dict[str, str] = {
            "Anthropic-Version": "2023-06-01",
            "Anthropic-Beta": all_betas,
            "Anthropic-Dangerous-Direct-Browser-Access": "true",
            "Content-Type": "application/json",
            "X-App": "cli",
            "X-Stainless-Helper-Method": "stream",
            "X-Stainless-Retry-Count": "0",
            "X-Stainless-Runtime-Version": "v24.3.0",
            "X-Stainless-Package-Version": "0.74.0",
            "X-Stainless-Runtime": "node",
            "X-Stainless-Lang": "js",
            "X-Stainless-Arch": _map_stainless_arch(),
            "X-Stainless-Os": _map_stainless_os(),
            "X-Stainless-Timeout": "600",
            "Connection": "keep-alive",
            "Accept-Encoding": "gzip, deflate, br, zstd",
            "Accept": "text/event-stream" if is_stream else "application/json",
            "User-Agent": user_agent,
        }
        return headers

    def wrap_request(
        self,
        request_body: dict[str, Any],
        *,
        model: str,  # noqa: ARG002
        url_model: str | None,
        decrypted_auth_config: dict[str, Any] | None,
    ) -> tuple[dict[str, Any], str | None]:
        auth_config = decrypted_auth_config or {}
        auth_method = _normalize_auth_method(auth_config)
        email = auth_config.get("email")

        normalized_request = _normalize_claude_code_request_shape(request_body)
        cleaned_request, body_betas = _extract_betas_from_body(normalized_request)
        tool_prefix_enabled = auth_method == "oauth" and not _parse_bool(
            auth_config.get("tool_prefix_disabled")
        )
        tool_randomized = tool_prefix_enabled and not (
            _parse_bool(auth_config.get("tool_randomization_disabled"))
            or _parse_bool(auth_config.get("tool_prefix_randomization_disabled"))
        )
        tool_aliases: tuple[tuple[str, str], ...] = tuple()

        if tool_prefix_enabled:
            forced_suffix_raw = str(auth_config.get("tool_prefix_random_suffix") or "").strip()
            forced_suffix = forced_suffix_raw or None
            cleaned_request, alias_reverse = apply_claude_tool_prefix_with_alias_map(
                cleaned_request,
                prefix=CLAUDE_TOOL_PREFIX,
                randomize_suffix=tool_randomized,
                random_suffix=forced_suffix,
            )
            tool_aliases = tuple(alias_reverse.items())

        set_claude_code_request_context(
            ClaudeCodeRequestContext(
                auth_method=auth_method,
                email=str(email) if email else None,
                is_stream=bool(cleaned_request.get("stream")),
                tool_prefix_enabled=tool_prefix_enabled,
                tool_randomized=tool_randomized,
                tool_aliases=tool_aliases,
                body_betas=body_betas,
            )
        )
        return cleaned_request, url_model

    def unwrap_response(self, data: Any) -> Any:
        ctx = get_claude_code_request_context()
        if not ctx or not ctx.tool_prefix_enabled:
            return data
        alias_map = dict(ctx.tool_aliases)
        return strip_claude_tool_prefix_from_response(
            data,
            prefix=CLAUDE_TOOL_PREFIX,
            alias_to_original=alias_map,
        )

    def postprocess_unwrapped_response(self, *, model: str, data: Any) -> None:  # noqa: ARG002
        return

    def capture_selected_base_url(self) -> str | None:
        return get_selected_base_url()

    def on_http_status(self, *, base_url: str | None, status_code: int) -> None:
        retryable = should_retry_same_candidate(status_code, None) or is_retryable_status(status_code)
        ctx = get_claude_code_request_context()
        if ctx:
            set_claude_code_request_context(
                replace(
                    ctx,
                    last_upstream_status=status_code,
                    last_upstream_retryable=retryable,
                )
            )
        logger.debug(
            "[claude_code] upstream status={} retryable={} base_url={}",
            status_code,
            retryable,
            base_url or "<unknown>",
        )

    def on_connection_error(self, *, base_url: str | None, exc: Exception) -> None:
        ctx = get_claude_code_request_context()
        if ctx:
            set_claude_code_request_context(
                replace(
                    ctx,
                    last_upstream_status=None,
                    last_upstream_retryable=True,
                )
            )
        logger.warning(
            "[claude_code] upstream connection error base_url={} error={}",
            base_url or "<unknown>",
            type(exc).__name__,
        )

    def force_stream_rewrite(self) -> bool:
        ctx = get_claude_code_request_context()
        return bool(ctx and ctx.tool_prefix_enabled)


claude_code_envelope = ClaudeCodeEnvelope()


__all__ = [
    "CLAUDE_TOOL_PREFIX",
    "ClaudeCodeEnvelope",
    "apply_claude_tool_prefix",
    "claude_code_envelope",
    "merge_anthropic_beta_headers",
    "strip_claude_tool_prefix_from_response",
]
