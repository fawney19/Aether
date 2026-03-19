"""Kiro provider envelope.

Kiro upstream is not Claude wire-compatible:
- Request: wrap Claude Messages body into Kiro `conversationState` request.
- Stream response: handled by StreamProcessor via binary EventStream rewrite.

We use contextvars to pass request-scoped values (region, machine_id, thinking)
from wrap_request() to extra_headers() and transport hook.
"""

from __future__ import annotations

from typing import Any

from src.core.logger import logger
from src.services.provider.adapters.kiro.context import KiroRequestContext, set_kiro_request_context
from src.services.provider.adapters.kiro.error_enhancer import (
    classify_kiro_connection_error,
    classify_kiro_http_status,
    summarize_kiro_connection_error,
)
from src.services.provider.adapters.kiro.headers import build_generate_assistant_headers
from src.services.provider.adapters.kiro.models.credentials import KiroAuthConfig
from src.services.provider.adapters.kiro.request import (
    build_kiro_request_context,
    build_kiro_request_payload,
)
from src.services.provider.request_context import get_selected_base_url


class KiroEnvelope:
    name = "kiro:generateAssistantResponse"

    def extra_headers(self) -> dict[str, str] | None:
        # Called after wrap_request(); relies on KiroRequestContext.
        from src.services.provider.adapters.kiro.context import get_kiro_request_context

        ctx = get_kiro_request_context()
        if ctx is None:
            return None

        host = f"q.{ctx.region}.amazonaws.com"
        return build_generate_assistant_headers(
            host=host,
            machine_id=ctx.machine_id,
            kiro_version=ctx.kiro_version,
            system_version=ctx.system_version,
            node_version=ctx.node_version,
        )

    def wrap_request(
        self,
        request_body: dict[str, Any],
        *,
        model: str,
        url_model: str | None,
        decrypted_auth_config: dict[str, Any] | None,
    ) -> tuple[dict[str, Any], str | None]:
        cfg = KiroAuthConfig.from_dict(decrypted_auth_config or {})
        set_kiro_request_context(build_kiro_request_context(request_body, cfg=cfg))
        wrapped = build_kiro_request_payload(
            request_body,
            model=model,
            cfg=cfg,
        )

        return wrapped, url_model

    def unwrap_response(self, data: Any) -> Any:
        return data

    def postprocess_unwrapped_response(self, *, model: str, data: Any) -> None:  # noqa: ARG002
        return

    def capture_selected_base_url(self) -> str | None:
        return get_selected_base_url()

    def on_http_status(self, *, base_url: str | None, status_code: int) -> None:
        from src.services.provider.adapters.kiro.context import update_kiro_http_status

        category = classify_kiro_http_status(status_code)
        update_kiro_http_status(status_code=status_code, category=category)
        if status_code >= 400:
            logger.warning(
                "kiro upstream http status: status={}, category={}, base_url={}",
                status_code,
                category,
                base_url or "-",
            )

    def on_connection_error(self, *, base_url: str | None, exc: Exception) -> None:
        from src.services.provider.adapters.kiro.context import update_kiro_connection_error

        category = classify_kiro_connection_error(exc)
        summary = summarize_kiro_connection_error(exc)
        update_kiro_connection_error(category=category, summary=summary)
        logger.warning(
            "kiro upstream connection error: category={}, base_url={}, error={}",
            category,
            base_url or "-",
            summary,
        )

    def force_stream_rewrite(self) -> bool:
        # Kiro streaming is binary AWS Event Stream and must be rewritten.
        return True


kiro_envelope = KiroEnvelope()


__all__ = ["KiroEnvelope", "kiro_envelope"]
