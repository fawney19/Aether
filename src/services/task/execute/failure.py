from __future__ import annotations

from typing import Any

import httpx

from src.core.exceptions import ProviderNotAvailableException
from src.core.logger import logger
from src.services.request.result import RequestMetadata


class TaskFailureOperationsService:
    """任务失败收敛相关操作（异常元数据与统一抛错）。"""

    @staticmethod
    def attach_metadata_to_error(
        error: Exception | None,
        candidate: Any | None,
        model_name: str,
        api_format: str,
    ) -> None:
        """Attach candidate metadata onto exception for usage recording."""
        if not error or not candidate:
            return

        existing_metadata = getattr(error, "request_metadata", None)
        if existing_metadata and getattr(existing_metadata, "api_format", None):
            return

        metadata = RequestMetadata(
            provider_request_headers=(
                getattr(existing_metadata, "provider_request_headers", {})
                if existing_metadata
                else {}
            ),
            provider=getattr(existing_metadata, "provider", None) or str(candidate.provider.name),
            model=getattr(existing_metadata, "model", None) or model_name,
            provider_id=getattr(existing_metadata, "provider_id", None)
            or str(candidate.provider.id),
            provider_endpoint_id=(
                getattr(existing_metadata, "provider_endpoint_id", None)
                or str(candidate.endpoint.id)
            ),
            provider_api_key_id=(
                getattr(existing_metadata, "provider_api_key_id", None) or str(candidate.key.id)
            ),
            api_format=api_format,
        )
        setattr(error, "request_metadata", metadata)

    @staticmethod
    def raise_all_failed_exception(
        request_id: str | None,
        max_attempts: int,
        last_candidate: Any | None,
        model_name: str,
        api_format: str,
        last_error: Exception | None = None,
    ) -> None:
        """Raise a unified 'all candidates failed' exception."""
        logger.error("  [{}] 所有 {} 个组合均失败", request_id, max_attempts)

        request_metadata = None
        if last_candidate:
            request_metadata = {
                "provider": last_candidate.provider.name,
                "model": model_name,
                "provider_id": str(last_candidate.provider.id),
                "provider_endpoint_id": str(last_candidate.endpoint.id),
                "provider_api_key_id": str(last_candidate.key.id),
                "api_format": api_format,
            }

        upstream_status: int | None = None
        upstream_response: str | None = None
        if last_error:
            if isinstance(last_error, httpx.HTTPStatusError):
                upstream_status = last_error.response.status_code
                upstream_response = getattr(last_error, "upstream_response", None)
                if not upstream_response:
                    try:
                        upstream_response = last_error.response.text
                    except Exception:
                        pass
            else:
                upstream_status = getattr(last_error, "upstream_status", None)
                upstream_response = getattr(last_error, "upstream_response", None)

            if (
                not upstream_response
                or not upstream_response.strip()
                or upstream_response.startswith("Unable to read")
            ):
                upstream_response = str(last_error)

        friendly_message = "服务暂时不可用，请稍后重试"
        if last_error:
            last_error_message = getattr(last_error, "message", None)
            if last_error_message and isinstance(last_error_message, str):
                friendly_message = last_error_message

        raise ProviderNotAvailableException(
            friendly_message,
            request_metadata=request_metadata,
            upstream_status=upstream_status,
            upstream_response=upstream_response,
        )
