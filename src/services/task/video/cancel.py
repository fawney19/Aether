from __future__ import annotations

from typing import Any

from sqlalchemy.orm import Session

from src.core.logger import logger
from src.models.database import ProviderAPIKey, ProviderEndpoint
from src.services.usage.service import UsageService


class VideoTaskCancelService:
    """视频任务取消服务（上游取消 + 本地状态与计费回写）。"""

    def __init__(self, db: Session) -> None:
        self.db = db

    async def cancel_task(
        self,
        *,
        task: Any,
        task_id: str,
        original_headers: dict[str, str] | None = None,
    ) -> Any:
        """
        Cancel a video task (best-effort) and void its Usage (no charge).

        Returns:
        - None on success
        - upstream httpx.Response when upstream returns an error (status >= 400)
        """
        from datetime import datetime, timezone

        from fastapi import HTTPException

        from src.clients.http_client import HTTPClientPool
        from src.core.api_format import (
            build_upstream_headers_for_endpoint,
            get_extra_headers_from_endpoint,
            make_signature_key,
        )
        from src.core.api_format.conversion.internal_video import VideoStatus
        from src.core.crypto import crypto_service
        from src.services.provider.auth import get_provider_auth
        from src.services.provider.transport import build_provider_url

        external_task_id = getattr(task, "external_task_id", None)
        if not external_task_id:
            raise HTTPException(status_code=500, detail="Task missing external_task_id")

        endpoint = (
            self.db.query(ProviderEndpoint).filter(ProviderEndpoint.id == task.endpoint_id).first()
        )
        key = self.db.query(ProviderAPIKey).filter(ProviderAPIKey.id == task.key_id).first()
        if not endpoint or not key:
            raise HTTPException(status_code=500, detail="Provider endpoint or key not found")
        if not getattr(key, "api_key", None):
            raise HTTPException(status_code=500, detail="Provider key not configured")

        upstream_key = crypto_service.decrypt(key.api_key)
        extra_headers = get_extra_headers_from_endpoint(endpoint)

        raw_family = str(getattr(endpoint, "api_family", "") or "").strip().lower()
        raw_kind = str(getattr(endpoint, "endpoint_kind", "") or "").strip().lower()
        provider_format = (
            make_signature_key(raw_family, raw_kind)
            if raw_family and raw_kind
            else str(
                getattr(endpoint, "api_format", "")
                or getattr(task, "provider_api_format", "")
                or ""
            )
        )
        provider_format_norm = provider_format.strip().lower()

        headers = build_upstream_headers_for_endpoint(
            original_headers or {},
            provider_format,
            upstream_key,
            endpoint_headers=extra_headers,
            header_rules=getattr(endpoint, "header_rules", None),
        )

        client = await HTTPClientPool.get_default_client_async()

        if provider_format_norm.startswith("openai:"):
            upstream_url = build_provider_url(endpoint, is_stream=False, key=key)
            upstream_url = f"{upstream_url.rstrip('/')}/{str(external_task_id).lstrip('/')}"
            response = await client.delete(upstream_url, headers=headers)
            if response.status_code >= 400:
                return response

        elif provider_format_norm.startswith("gemini:"):
            # Gemini cancel endpoint supports both:
            # - operations/{id}:cancel
            # - models/{model}/operations/{id}:cancel
            operation_name = str(external_task_id)
            if not (
                operation_name.startswith("operations/") or operation_name.startswith("models/")
            ):
                operation_name = f"operations/{operation_name}"

            base = (
                getattr(endpoint, "base_url", None) or "https://generativelanguage.googleapis.com"
            ).rstrip("/")
            if base.endswith("/v1beta"):
                base = base[: -len("/v1beta")]
            upstream_url = f"{base}/v1beta/{operation_name}:cancel"

            auth_info = await get_provider_auth(endpoint, key)
            if auth_info:
                headers.pop("x-goog-api-key", None)
                headers[auth_info.auth_header] = auth_info.auth_value

            response = await client.post(upstream_url, headers=headers, json={})
            if response.status_code >= 400:
                return response

        else:
            raise HTTPException(
                status_code=400,
                detail=f"Cancel not supported for provider format: {provider_format}",
            )

        task.status = VideoStatus.CANCELLED.value
        task.updated_at = datetime.now(timezone.utc)

        # Void Usage (no charge)
        try:
            voided = UsageService.finalize_void(
                self.db,
                request_id=task.request_id,
                reason="cancelled_by_user",
            )
            if not voided:
                UsageService.void_settled(
                    self.db,
                    request_id=task.request_id,
                    reason="cancelled_by_user",
                )
        except Exception as exc:
            logger.warning(
                "Failed to void usage for cancelled task={}: {}",
                getattr(task, "id", task_id),
                str(exc),
            )

        self.db.commit()
        return None
