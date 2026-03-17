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

from typing import Any

from src.services.provider.adapters.codex.context import (
    CodexRequestContext,
    get_codex_request_context,
    set_codex_request_context,
)
from src.services.provider.adapters.codex.request_patching import patch_openai_cli_request_for_codex
from src.services.provider.request_context import get_selected_base_url


class CodexOAuthEnvelope:
    """Provider envelope hooks for Codex OAuth upstream."""

    name = "codex:oauth"

    def extra_headers(self) -> dict[str, str] | None:
        # Codex desktop clients already send the protocol-specific headers they need.
        # Preserve the original request headers as much as possible and avoid injecting
        # synthetic CLI identity headers here.
        return None

    def prepare_context(
        self,
        *,
        provider_config: Any,  # noqa: ARG002
        key_id: str,  # noqa: ARG002
        user_api_key_id: str | None = None,
        is_stream: bool,  # noqa: ARG002
        provider_id: str | None = None,  # noqa: ARG002
        key: Any = None,  # noqa: ARG002
    ) -> str | None:
        existing_ctx = get_codex_request_context()
        set_codex_request_context(
            CodexRequestContext(
                account_id=existing_ctx.account_id if existing_ctx else None,
                user_api_key_id=str(user_api_key_id or "").strip() or None,
                is_compact=existing_ctx.is_compact if existing_ctx else False,
            )
        )
        return None

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
        # Compact sentinel may have been popped earlier by finalize_provider_request;
        # prefer the pre-set context var (set by adapter), fall back to request body.
        existing_ctx = get_codex_request_context()
        is_compact = (existing_ctx.is_compact if existing_ctx else False) or bool(
            request_body.get("_aether_compact", False)
        )
        patched_request_body = patch_openai_cli_request_for_codex(request_body)
        set_codex_request_context(
            CodexRequestContext(
                account_id=str(account_id) if account_id else None,
                user_api_key_id=existing_ctx.user_api_key_id if existing_ctx else None,
                is_compact=is_compact,
            )
        )
        # Context 不需要手动清理: FastAPI 每个请求运行在独立的 asyncio Task 中,
        # contextvars 天然隔离, Task 结束后自动回收。
        # No wire envelope for Codex; keep request body as-is.
        return patched_request_body, url_model

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
