"""Claude Code upstream envelope hooks."""

from __future__ import annotations

import copy
import platform
from typing import Any

from src.config.settings import config
from src.services.provider.adapters.claude_code.context import (
    ClaudeCodeRequestContext,
    get_claude_code_request_context,
    set_claude_code_request_context,
)
from src.services.provider.request_context import get_selected_base_url

CLAUDE_TOOL_PREFIX = "proxy_"
_PROMPT_CACHING_BETA = "prompt-caching-2024-07-31"
_CLAUDE_DEFAULT_BETAS: tuple[str, ...] = (
    "claude-code-20250219",
    "oauth-2025-04-20",
    "interleaved-thinking-2025-05-14",
    "fine-grained-tool-streaming-2025-05-14",
    _PROMPT_CACHING_BETA,
)
_BUILTIN_TOOL_NAMES: frozenset[str] = frozenset(
    {
        "web_search",
        "code_execution",
        "text_editor",
        "computer",
    }
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
    betas = _parse_beta_tokens(copied.pop("betas", None))
    return copied, tuple(betas)


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


def _collect_builtin_tool_names(payload: dict[str, Any]) -> set[str]:
    names = set(_BUILTIN_TOOL_NAMES)
    tools = payload.get("tools")
    if not isinstance(tools, list):
        return names
    for tool in tools:
        if not isinstance(tool, dict):
            continue
        tool_name = tool.get("name")
        tool_type = str(tool.get("type") or "").strip()
        if isinstance(tool_name, str) and tool_name.strip() and tool_type:
            names.add(tool_name.strip())
    return names


def _prefix_tool_field(
    container: dict[str, Any],
    field: str,
    *,
    prefix: str,
    builtin_names: set[str],
) -> None:
    value = container.get(field)
    if not isinstance(value, str):
        return
    if not value or value.startswith(prefix) or value in builtin_names:
        return
    container[field] = f"{prefix}{value}"


def apply_claude_tool_prefix(
    request_body: dict[str, Any],
    *,
    prefix: str = CLAUDE_TOOL_PREFIX,
) -> dict[str, Any]:
    if not prefix:
        return request_body

    copied = copy.deepcopy(request_body)
    builtin_names = _collect_builtin_tool_names(copied)

    tools = copied.get("tools")
    if isinstance(tools, list):
        for tool in tools:
            if not isinstance(tool, dict):
                continue
            if str(tool.get("type") or "").strip():
                continue
            _prefix_tool_field(tool, "name", prefix=prefix, builtin_names=builtin_names)

    tool_choice = copied.get("tool_choice")
    if isinstance(tool_choice, dict) and str(tool_choice.get("type") or "").strip().lower() == "tool":
        _prefix_tool_field(tool_choice, "name", prefix=prefix, builtin_names=builtin_names)

    messages = copied.get("messages")
    if isinstance(messages, list):
        for message in messages:
            if not isinstance(message, dict):
                continue
            content = message.get("content")
            if not isinstance(content, list):
                continue
            for block in content:
                if not isinstance(block, dict):
                    continue
                block_type = str(block.get("type") or "").strip().lower()
                if block_type == "tool_use":
                    _prefix_tool_field(block, "name", prefix=prefix, builtin_names=builtin_names)
                elif block_type == "tool_reference":
                    _prefix_tool_field(
                        block,
                        "tool_name",
                        prefix=prefix,
                        builtin_names=builtin_names,
                    )
                elif block_type == "tool_result":
                    nested_content = block.get("content")
                    if not isinstance(nested_content, list):
                        continue
                    for nested_block in nested_content:
                        if not isinstance(nested_block, dict):
                            continue
                        if str(nested_block.get("type") or "").strip().lower() != "tool_reference":
                            continue
                        _prefix_tool_field(
                            nested_block,
                            "tool_name",
                            prefix=prefix,
                            builtin_names=builtin_names,
                        )

    return copied


def _strip_tool_field(container: dict[str, Any], field: str, *, prefix: str) -> None:
    value = container.get(field)
    if not isinstance(value, str) or not value.startswith(prefix):
        return
    container[field] = value[len(prefix) :]


def _strip_tool_prefix_from_content(content: list[Any], *, prefix: str) -> None:
    for block in content:
        if not isinstance(block, dict):
            continue
        block_type = str(block.get("type") or "").strip().lower()
        if block_type == "tool_use":
            _strip_tool_field(block, "name", prefix=prefix)
        elif block_type == "tool_reference":
            _strip_tool_field(block, "tool_name", prefix=prefix)
        elif block_type == "tool_result":
            nested = block.get("content")
            if isinstance(nested, list):
                _strip_tool_prefix_from_content(nested, prefix=prefix)


def strip_claude_tool_prefix_from_response(
    data: Any,
    *,
    prefix: str = CLAUDE_TOOL_PREFIX,
) -> Any:
    if not prefix or not isinstance(data, dict):
        return data

    content = data.get("content")
    if isinstance(content, list):
        _strip_tool_prefix_from_content(content, prefix=prefix)

    content_block = data.get("content_block")
    if isinstance(content_block, dict):
        _strip_tool_prefix_from_content([content_block], prefix=prefix)

    return data


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

        cleaned_request, body_betas = _extract_betas_from_body(request_body)
        tool_prefix_enabled = auth_method == "oauth" and not _parse_bool(
            auth_config.get("tool_prefix_disabled")
        )

        if tool_prefix_enabled:
            cleaned_request = apply_claude_tool_prefix(cleaned_request, prefix=CLAUDE_TOOL_PREFIX)

        set_claude_code_request_context(
            ClaudeCodeRequestContext(
                auth_method=auth_method,
                email=str(email) if email else None,
                is_stream=bool(cleaned_request.get("stream")),
                tool_prefix_enabled=tool_prefix_enabled,
                body_betas=body_betas,
            )
        )
        return cleaned_request, url_model

    def unwrap_response(self, data: Any) -> Any:
        ctx = get_claude_code_request_context()
        if not ctx or not ctx.tool_prefix_enabled:
            return data
        return strip_claude_tool_prefix_from_response(data, prefix=CLAUDE_TOOL_PREFIX)

    def postprocess_unwrapped_response(self, *, model: str, data: Any) -> None:  # noqa: ARG002
        return

    def capture_selected_base_url(self) -> str | None:
        return get_selected_base_url()

    def on_http_status(self, *, base_url: str | None, status_code: int) -> None:  # noqa: ARG002
        return

    def on_connection_error(self, *, base_url: str | None, exc: Exception) -> None:  # noqa: ARG002
        return

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
