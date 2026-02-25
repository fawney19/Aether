"""Claude Code upstream envelope hooks."""

from __future__ import annotations

from typing import Any

from src.services.provider.adapters.claude_code.context import (
    ClaudeCodeRequestContext,
    set_claude_code_request_context,
)
from src.services.provider.request_context import get_selected_base_url


class ClaudeCodeEnvelope:
    """Provider envelope hooks for Claude Code upstream."""

    name = "claude-code:messages"

    def extra_headers(self) -> dict[str, str] | None:
        return {
            "anthropic-version": "2023-06-01",
            "Content-Type": "application/json",
        }

    def wrap_request(
        self,
        request_body: dict[str, Any],
        *,
        model: str,  # noqa: ARG002
        url_model: str | None,
        decrypted_auth_config: dict[str, Any] | None,
    ) -> tuple[dict[str, Any], str | None]:
        auth_config = decrypted_auth_config or {}
        auth_method = str(auth_config.get("auth_method", "bearer"))
        email = auth_config.get("email")

        set_claude_code_request_context(
            ClaudeCodeRequestContext(
                auth_method=auth_method,
                email=str(email) if email else None,
            )
        )
        return request_body, url_model

    def unwrap_response(self, data: Any) -> Any:
        return data

    def postprocess_unwrapped_response(self, *, model: str, data: Any) -> None:  # noqa: ARG002
        return

    def capture_selected_base_url(self) -> str | None:
        return get_selected_base_url()

    def on_http_status(self, *, base_url: str | None, status_code: int) -> None:  # noqa: ARG002
        return

    def on_connection_error(self, *, base_url: str | None, exc: Exception) -> None:  # noqa: ARG002
        return

    def force_stream_rewrite(self) -> bool:
        return False


claude_code_envelope = ClaudeCodeEnvelope()


__all__ = ["ClaudeCodeEnvelope", "claude_code_envelope"]
