"""GeminiCLI v1internal request envelope."""

from __future__ import annotations

from typing import Any

from src.services.provider.adapters.gemini_cli.constants import get_v1internal_extra_headers
from src.services.provider.request_context import get_selected_base_url


def wrap_v1internal_request(
    gemini_request: dict[str, Any],
    *,
    project_id: str,
    model: str,
) -> dict[str, Any]:
    """Wrap a Gemini request into GeminiCLI v1internal format."""
    inner_request = dict(gemini_request)
    inner_request.pop("model", None)
    inner_request.pop("stream", None)
    return {
        "model": model,
        "project": project_id,
        "request": inner_request,
    }


class GeminiCliV1InternalEnvelope:
    name = "gemini_cli:v1internal"

    def extra_headers(self) -> dict[str, str] | None:
        return get_v1internal_extra_headers()

    def wrap_request(
        self,
        request_body: dict[str, Any],
        *,
        model: str,
        url_model: str | None,
        decrypted_auth_config: dict[str, Any] | None,
    ) -> tuple[dict[str, Any], str | None]:
        _ = url_model
        project_id = (decrypted_auth_config or {}).get("project_id")
        if not isinstance(project_id, str) or not project_id:
            from src.core.exceptions import ProviderNotAvailableException

            raise ProviderNotAvailableException(
                "GeminiCLI OAuth 配置缺少 project_id，请重新授权",
                provider_name="gemini_cli",
                upstream_response="missing auth_config.project_id",
            )

        wrapped = wrap_v1internal_request(
            request_body,
            project_id=project_id,
            model=model,
        )
        return wrapped, None

    def unwrap_response(self, data: Any) -> Any:
        if not isinstance(data, dict):
            return data

        response_obj = data.get("response")
        if "candidates" not in data and isinstance(response_obj, dict):
            return response_obj

        return data

    def postprocess_unwrapped_response(self, *, model: str, data: Any) -> None:
        _ = model, data
        return

    def capture_selected_base_url(self) -> str | None:
        return get_selected_base_url()

    def on_http_status(self, *, base_url: str | None, status_code: int) -> None:
        _ = base_url, status_code
        return

    def on_connection_error(self, *, base_url: str | None, exc: Exception) -> None:
        _ = base_url, exc
        return

    def force_stream_rewrite(self) -> bool:
        return False


gemini_cli_v1internal_envelope = GeminiCliV1InternalEnvelope()


__all__ = [
    "GeminiCliV1InternalEnvelope",
    "gemini_cli_v1internal_envelope",
    "wrap_v1internal_request",
]
