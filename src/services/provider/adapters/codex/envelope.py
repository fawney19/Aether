"""Codex upstream envelope hooks.

Codex OAuth upstreams (e.g. `chatgpt.com/backend-api/codex`) behave like the OpenAI
Responses API (`openai:cli`) but may require additional transport-level headers
to avoid upstream blocks (Cloudflare, etc.).

Request/response shape quirks should live in the conversion layer as a same-format
variant (`target_variant="codex"` in the `openai:cli` normalizer). This envelope
only adds headers and keeps the rest as a no-op wrapper.

We use contextvars to pass request-scoped values (account_id) from wrap_request()
to extra_headers().
"""

from __future__ import annotations

import uuid
from typing import Any

from src.config.settings import config
from src.services.provider.adapters.codex.context import (
    CodexRequestContext,
    get_codex_request_context,
    set_codex_request_context,
)
from src.services.provider.request_context import get_selected_base_url


class CodexOAuthEnvelope:
    """Provider envelope hooks for Codex OAuth upstream."""

    name = "codex:oauth"

    def extra_headers(self) -> dict[str, str] | None:
        # These headers are best-effort: Codex upstream is stricter than public OpenAI API.
        # Keep them provider-scoped (via ProviderEnvelope) to avoid leaking to other upstreams.
        headers: dict[str, str] = {
            "x-oai-web-search-eligible": "true",
            "session_id": str(uuid.uuid4()),
            "originator": "codex_cli_rs",
            # Ensure SSE is returned when upstream is forced to streaming mode.
            "Accept": "text/event-stream",
        }

        ua = str(getattr(config, "internal_user_agent_openai_cli", "") or "").strip()
        if ua:
            headers["User-Agent"] = ua

        # Add chatgpt-account-id from context (set by wrap_request)
        ctx = get_codex_request_context()
        if ctx and ctx.account_id:
            headers["chatgpt-account-id"] = ctx.account_id

        return headers

    def wrap_request(
        self,
        request_body: dict[str, Any],
        *,
        model: str,  # noqa: ARG002
        url_model: str | None,
        decrypted_auth_config: dict[str, Any] | None,
    ) -> tuple[dict[str, Any], str | None]:
        # Extract account_id from auth_config and set context for extra_headers()
        account_id = (decrypted_auth_config or {}).get("account_id")
        user_id = (decrypted_auth_config or {}).get("user_id")
        set_codex_request_context(
            CodexRequestContext(
                account_id=str(account_id) if account_id else None,
                user_id=str(user_id) if user_id else None,
            )
        )
        # No wire envelope for Codex; keep request body as-is.
        return request_body, url_model

    def unwrap_response(self, data: Any) -> Any:
        # No response envelope for Codex.
        return data

    def postprocess_unwrapped_response(self, *, model: str, data: Any) -> None:  # noqa: ARG002
        return

    def capture_selected_base_url(self) -> str | None:
        # Keep interface consistent with Antigravity. Transport currently doesn't set this for Codex.
        return get_selected_base_url()

    def on_http_status(self, *, base_url: str | None, status_code: int) -> None:  # noqa: ARG002
        return

    def on_connection_error(self, *, base_url: str | None, exc: Exception) -> None:  # noqa: ARG002
        return

    def force_stream_rewrite(self) -> bool:
        return False


codex_oauth_envelope = CodexOAuthEnvelope()


__all__ = ["CodexOAuthEnvelope", "codex_oauth_envelope"]
