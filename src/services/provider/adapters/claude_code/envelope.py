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
from src.services.provider.adapters.claude_code.tool_prefix import (
    CLAUDE_TOOL_PREFIX,
    apply_claude_tool_prefix,
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
