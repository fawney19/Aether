"""
Gemini Video Handler - Veo 视频生成实现
"""

from __future__ import annotations

import time
from datetime import datetime, timedelta, timezone
from typing import Any, AsyncIterator
from uuid import uuid4

from fastapi import HTTPException, Request
from fastapi.responses import JSONResponse, Response, StreamingResponse
from sqlalchemy.exc import IntegrityError
from sqlalchemy.orm import Session

from src.api.handlers.base.request_builder import apply_body_rules, get_provider_auth
from src.api.handlers.base.video_handler_base import (
    VideoHandlerBase,
    normalize_gemini_operation_id,
    sanitize_error_message,
)
from src.clients.http_client import HTTPClientPool
from src.config.settings import config
from src.core.api_format import (
    ApiFamily,
    EndpointKind,
    build_upstream_headers_for_endpoint,
    get_extra_headers_from_endpoint,
    make_signature_key,
)
from src.core.api_format.conversion.internal_video import (
    InternalVideoRequest,
    InternalVideoTask,
    VideoStatus,
)
from src.core.api_format.conversion.normalizers.gemini import GeminiNormalizer
from src.core.api_format.conversion.registry import format_conversion_registry
from src.core.api_format.headers import HOP_BY_HOP_HEADERS
from src.core.crypto import crypto_service
from src.core.logger import logger
from src.models.database import ApiKey, ProviderAPIKey, ProviderEndpoint, User, VideoTask
from src.services.billing.rule_service import BillingRuleLookupResult, BillingRuleService
from src.services.scheduling.aware_scheduler import ProviderCandidate
from src.services.usage.service import UsageService


class GeminiVeoHandler(VideoHandlerBase):
    FORMAT_ID = "gemini:video"
    API_FAMILY = ApiFamily.GEMINI
    ENDPOINT_KIND = EndpointKind.VIDEO

    DEFAULT_BASE_URL = "https://generativelanguage.googleapis.com"

    def __init__(
        self,
        db: Session,
        user: User,
        api_key: ApiKey,
        request_id: str,
        client_ip: str,
        user_agent: str,
        start_time: float,
        allowed_api_formats: list[str] | None = None,
    ):
        super().__init__(
            db=db,
            user=user,
            api_key=api_key,
            request_id=request_id,
            client_ip=client_ip,
            user_agent=user_agent,
            start_time=start_time,
            allowed_api_formats=allowed_api_formats,
        )
        self._normalizer = GeminiNormalizer()

    @staticmethod
    def _get_request_base_url(http_request: Request) -> str:
        """从 HTTP 请求中获取基础 URL（协议 + 主机）"""
        # 优先使用 X-Forwarded-Proto 和 X-Forwarded-Host（代理场景）
        proto = http_request.headers.get("x-forwarded-proto") or http_request.url.scheme
        host = http_request.headers.get("x-forwarded-host") or http_request.headers.get("host")
        if host:
            return f"{proto}://{host}"
        # 回退到 request.url
        return f"{http_request.url.scheme}://{http_request.url.netloc}"

    async def handle_create_task(
        self,
        *,
        http_request: Request,
        original_headers: dict[str, str],
        original_request_body: dict[str, Any],
        query_params: dict[str, str] | None = None,
        path_params: dict[str, Any] | None = None,
    ) -> JSONResponse:
        # 将路径中的 model 合并到请求体再解析
        model = path_params.get("model") if path_params else None
        request_with_model = {**original_request_body}
        if model:
            request_with_model["model"] = str(model)

        try:
            internal_request = self._normalizer.video_request_to_internal(request_with_model)
        except ValueError as e:
            raise HTTPException(status_code=400, detail=str(e))

        # 异步任务：提前创建 pending usage，便于前端看到“处理中”
        try:
            UsageService.create_pending_usage(
                db=self.db,
                request_id=self.request_id,
                user=self.user,
                api_key=self.api_key,
                model=internal_request.model,
                is_stream=False,
                request_type="video",
                api_format=self.FORMAT_ID,
                request_headers=original_headers,
                request_body=original_request_body,
            )
        except Exception as exc:
            logger.warning(
                "Failed to create pending usage for video request_id={}: {}",
                self.request_id,
                sanitize_error_message(str(exc)),
            )

        # 用于跟踪是否发生了格式转换
        format_conversion_info: dict[str, Any] = {
            "converted": False,
            "provider_format": None,
        }

        async def _submit(candidate: ProviderCandidate) -> Any:
            upstream_key, endpoint, _key, auth_info = await self._resolve_upstream_key(candidate)

            # 检测目标格式
            provider_format = make_signature_key(
                str(getattr(endpoint, "api_family", "")).strip().lower(),
                str(getattr(endpoint, "endpoint_kind", "")).strip().lower(),
            )
            needs_conversion = provider_format.upper() != self.FORMAT_ID.upper()
            format_conversion_info["provider_format"] = provider_format
            format_conversion_info["converted"] = needs_conversion

            # 应用端点的请求体规则
            endpoint_body_rules = getattr(endpoint, "body_rules", None)

            if needs_conversion and provider_format.upper().startswith("OPENAI:"):
                # Gemini -> OpenAI 格式转换
                converted_body = format_conversion_registry.convert_video_request(
                    original_request_body,
                    self.FORMAT_ID,
                    provider_format,
                )
                # 确保 seconds 字段为字符串类型（上游 Go 服务要求 string）
                if "seconds" in converted_body and converted_body["seconds"] is not None:
                    converted_body["seconds"] = str(converted_body["seconds"])

                if endpoint_body_rules:
                    converted_body = apply_body_rules(converted_body, endpoint_body_rules)

                # 构建 OpenAI 风格的 URL
                upstream_url = self._build_openai_upstream_url(endpoint.base_url)

                # 构建 OpenAI 风格的请求头
                headers = self._build_openai_upstream_headers(
                    original_headers, upstream_key, endpoint
                )

                client = await HTTPClientPool.get_default_client_async()
                return await client.post(upstream_url, headers=headers, json=converted_body)
            else:
                # 原始 Gemini 格式
                request_body = (
                    original_request_body.copy() if endpoint_body_rules else original_request_body
                )
                if endpoint_body_rules:
                    request_body = apply_body_rules(request_body, endpoint_body_rules)

                upstream_url = self._build_upstream_url(endpoint.base_url, internal_request.model)
                headers = self._build_upstream_headers(
                    original_headers, upstream_key, endpoint, auth_info
                )
                client = await HTTPClientPool.get_default_client_async()
                return await client.post(upstream_url, headers=headers, json=request_body)

        def _extract_task_id(payload: dict[str, Any]) -> str | None:
            # 根据响应格式提取 task ID
            # Gemini: {"name": "operations/..."}
            # OpenAI: {"id": "..."}
            if "name" in payload:
                value = payload.get("name")
                logger.debug(
                    "[GeminiVeoHandler] Upstream response name={}, keys={}",
                    value,
                    list(payload.keys()) if isinstance(payload, dict) else type(payload),
                )
                if not value:
                    return None
                return normalize_gemini_operation_id(str(value))
            if "id" in payload:
                # OpenAI 格式
                return str(payload["id"])
            return None

        outcome_or_response = await self._submit_with_failover(
            api_format=self.FORMAT_ID,
            model_name=internal_request.model,
            task_type="video",
            submit_func=_submit,
            extract_external_task_id=_extract_task_id,
            supported_auth_types={"api_key", "service_account", "vertex_ai"},
            allow_format_conversion=True,
            max_candidates=10,
        )
        if isinstance(outcome_or_response, JSONResponse):
            return outcome_or_response
        outcome = outcome_or_response

        # 冻结 billing_rule 配置（用于异步任务的成本一致性）
        # 复用 _select_candidate 中已查询的结果；billing_require_rule=false 时需补查
        rule_lookup = outcome.rule_lookup
        if rule_lookup is None:
            rule_lookup = BillingRuleService.find_rule(
                self.db,
                provider_id=outcome.candidate.provider.id,
                model_name=internal_request.model,
                task_type="video",
            )
        billing_rule_snapshot = self._build_billing_rule_snapshot(rule_lookup)

        external_task_id = outcome.external_task_id

        # 如果发生了格式转换，记录转换后的请求体
        converted_request_body = original_request_body
        if format_conversion_info["converted"]:
            try:
                converted_request_body = format_conversion_registry.convert_video_request(
                    original_request_body,
                    self.FORMAT_ID,
                    format_conversion_info["provider_format"],
                )
            except Exception as e:
                logger.warning(
                    "[GeminiVeoHandler] Failed to record converted request: {}",
                    sanitize_error_message(str(e)),
                )

        task = self._create_task_record(
            external_task_id=external_task_id,
            candidate=outcome.candidate,
            original_request_body=original_request_body,
            converted_request_body=converted_request_body,
            internal_request=internal_request,
            candidate_keys=outcome.candidate_keys,
            original_headers=original_headers,
            billing_rule_snapshot=billing_rule_snapshot,
            format_converted=format_conversion_info["converted"],
        )
        try:
            self.db.add(task)
            self.db.flush()  # 先 flush 检测冲突
            self.db.commit()
            self.db.refresh(task)
            logger.debug(
                "[GeminiVeoHandler] Task created: id={}, external_task_id={}",
                task.id,
                task.external_task_id,
            )
        except IntegrityError:
            self.db.rollback()
            raise HTTPException(status_code=409, detail="Task already exists")

        # 先构建返回给客户端的响应（使用短 ID 对外暴露）
        internal_task = InternalVideoTask(
            id=task.short_id,
            external_id=external_task_id,
            status=VideoStatus.SUBMITTED,
            created_at=task.created_at,
            original_request=internal_request,
        )
        base_url = self._get_request_base_url(http_request)
        response_body = self._normalizer.video_task_from_internal(internal_task, base_url=base_url)

        # 提交成功后立即结算 Usage（费用暂时为 0，轮询完成后更新）
        response_time_ms = int((time.time() - self.start_time) * 1000)
        try:
            # 构建发送给上游的请求头（脱敏）
            upstream_request_headers = self._build_upstream_headers(
                original_headers,
                "",  # key 不重要，只是用于记录
                outcome.candidate.endpoint,
                None,  # auth_info
            )

            UsageService.finalize_submitted(
                self.db,
                request_id=self.request_id,
                provider_name=outcome.candidate.provider.name,
                provider_id=outcome.candidate.provider.id,
                provider_endpoint_id=outcome.candidate.endpoint.id,
                provider_api_key_id=outcome.candidate.key.id,
                response_time_ms=response_time_ms,
                status_code=outcome.upstream_status_code or 200,
                endpoint_api_format=make_signature_key(
                    str(getattr(outcome.candidate.endpoint, "api_family", "")).strip().lower(),
                    str(getattr(outcome.candidate.endpoint, "endpoint_kind", "")).strip().lower(),
                ),
                provider_request_headers=upstream_request_headers,
                response_headers=outcome.upstream_headers,
                response_body=response_body,  # 使用我们转换后的响应（包含我们的 ID）
            )
            self.db.commit()
        except Exception as exc:
            logger.warning(
                "Failed to finalize submitted usage for video request_id={}: {}",
                self.request_id,
                sanitize_error_message(str(exc)),
            )

        return JSONResponse(response_body)

    async def handle_get_task(
        self,
        *,
        task_id: str,
        http_request: Request,
        original_headers: dict[str, str],
        query_params: dict[str, str] | None = None,
        path_params: dict[str, Any] | None = None,
    ) -> JSONResponse:
        # Gemini 使用 operations/{id} 格式，需要按 external_task_id 查找
        task = self._get_task_by_external_id(task_id)

        # 直接从数据库返回任务状态（后台轮询服务会持续更新状态）
        internal_task = self._task_to_internal(task)
        base_url = self._get_request_base_url(http_request)
        response_body = self._normalizer.video_task_from_internal(internal_task, base_url=base_url)
        return JSONResponse(response_body)

    async def handle_list_tasks(
        self,
        *,
        http_request: Request,
        original_headers: dict[str, str],
        query_params: dict[str, str] | None = None,
        path_params: dict[str, Any] | None = None,
    ) -> JSONResponse:
        tasks = (
            self.db.query(VideoTask)
            .filter(VideoTask.user_id == self.user.id)
            .order_by(VideoTask.created_at.desc())
            .limit(100)
            .all()
        )
        base_url = self._get_request_base_url(http_request)
        items = [
            self._normalizer.video_task_from_internal(self._task_to_internal(t), base_url=base_url)
            for t in tasks
        ]
        return JSONResponse({"operations": items})

    async def handle_cancel_task(
        self,
        *,
        task_id: str,
        http_request: Request,
        original_headers: dict[str, str],
        query_params: dict[str, str] | None = None,
        path_params: dict[str, Any] | None = None,
    ) -> JSONResponse:
        from src.services.task.service import TaskService

        _ = (http_request, query_params, path_params)  # reserved for future extensions
        err_resp = await TaskService(self.db).cancel(
            task_id,
            user_id=str(self.user.id),
            original_headers=original_headers,
        )
        if err_resp is not None:
            return self._build_error_response(err_resp)
        return JSONResponse({})

    async def handle_download_content(
        self,
        *,
        task_id: str,
        http_request: Request,
        original_headers: dict[str, str],
        query_params: dict[str, str] | None = None,
        path_params: dict[str, Any] | None = None,
    ) -> Response | StreamingResponse:
        task = self._get_task_by_external_id(task_id)

        # 根据任务状态返回不同的错误码
        if not task.video_url:
            if task.status in (
                VideoStatus.PENDING.value,
                VideoStatus.SUBMITTED.value,
                VideoStatus.QUEUED.value,
                VideoStatus.PROCESSING.value,
            ):
                # 任务仍在处理中，返回 202 Accepted
                raise HTTPException(
                    status_code=202,
                    detail=f"Video is still processing (status: {task.status})",
                )
            if task.status == VideoStatus.FAILED.value:
                raise HTTPException(
                    status_code=422,
                    detail=f"Video generation failed: {task.error_message or 'Unknown error'}",
                )
            # 其他状态（如 CANCELLED）
            raise HTTPException(status_code=404, detail="Video not available")

        # 检查视频是否已过期
        if task.video_expires_at:
            now = datetime.now(timezone.utc)
            if task.video_expires_at < now:
                raise HTTPException(status_code=410, detail="Video URL has expired")

        # 获取 provider 的认证信息（Gemini 下载视频需要带 API Key）
        endpoint, key = self._get_endpoint_and_key(task)
        download_headers: dict[str, str] = {}
        if key.api_key:
            try:
                upstream_key = crypto_service.decrypt(key.api_key)
                # Gemini API 使用 x-goog-api-key 头进行认证
                download_headers["x-goog-api-key"] = upstream_key

                # 如果是 Vertex AI，需要使用 OAuth Bearer token
                auth_info = await get_provider_auth(endpoint, key)
                if auth_info:
                    download_headers.pop("x-goog-api-key", None)
                    download_headers[auth_info.auth_header] = auth_info.auth_value
            except Exception as exc:
                logger.warning(
                    "[VideoDownload] Failed to get auth for download task={}: {}",
                    task.id,
                    sanitize_error_message(str(exc)),
                )
                # 继续尝试无认证下载（某些 URL 可能是预签名的）

        # 代理下载而非直接重定向，避免暴露上游存储 URL
        # 使用 httpx 支持重定向（Gemini 视频 URL 会重定向到实际存储位置）
        import httpx

        try:
            # 解析代理配置（key > provider > 系统默认）
            from src.services.proxy_node.resolver import (
                build_proxy_client_kwargs,
                resolve_effective_proxy,
            )

            provider = getattr(endpoint, "provider", None) if endpoint else None
            eff_proxy = resolve_effective_proxy(
                getattr(provider, "proxy", None) if provider else None,
                getattr(key, "proxy", None),
            )

            # 使用 follow_redirects=True 跟随重定向
            async with httpx.AsyncClient(
                **build_proxy_client_kwargs(
                    eff_proxy,
                    timeout=httpx.Timeout(300.0),
                    follow_redirects=True,
                )
            ) as client:
                response = await client.get(task.video_url, headers=download_headers)
        except Exception as exc:
            logger.error(
                "[VideoDownload] Upstream fetch failed user={} task={}: {}",
                self.user.id,
                task.id,
                sanitize_error_message(str(exc)),
            )
            raise HTTPException(status_code=502, detail="Failed to fetch video")

        if response.status_code >= 400:
            raise HTTPException(status_code=response.status_code, detail="Upstream error")

        # 返回完整的视频内容（非 streaming，因为需要跟随重定向）
        safe_headers = {
            k: v for k, v in response.headers.items() if k.lower() not in HOP_BY_HOP_HEADERS
        }
        return Response(
            content=response.content,
            status_code=response.status_code,
            headers=safe_headers,
            media_type=response.headers.get("content-type", "video/mp4"),
        )

    # ------------------------------------------------------------------
    # Helpers
    # ------------------------------------------------------------------

    async def _resolve_upstream_key(
        self, candidate: ProviderCandidate
    ) -> tuple[str, ProviderEndpoint, ProviderAPIKey, Any | None]:
        try:
            upstream_key = crypto_service.decrypt(candidate.key.api_key)
        except Exception as exc:
            logger.error(
                "Failed to decrypt provider key id={}: {}",
                candidate.key.id,
                sanitize_error_message(str(exc)),
            )
            raise HTTPException(status_code=500, detail="Failed to decrypt provider key")

        auth_info = await get_provider_auth(candidate.endpoint, candidate.key)
        return upstream_key, candidate.endpoint, candidate.key, auth_info

    def _build_upstream_url(self, base_url: str | None, model: str) -> str:
        base = (base_url or self.DEFAULT_BASE_URL).rstrip("/")
        if base.endswith("/v1beta"):
            base = base[: -len("/v1beta")]
        return f"{base}/v1beta/models/{model}:predictLongRunning"

    def _build_cancel_url(self, base_url: str | None, operation_name: str) -> str:
        base = (base_url or self.DEFAULT_BASE_URL).rstrip("/")
        if base.endswith("/v1beta"):
            base = base[: -len("/v1beta")]
        return f"{base}/v1beta/{operation_name}:cancel"

    def _build_upstream_headers(
        self,
        original_headers: dict[str, str],
        upstream_key: str,
        endpoint: ProviderEndpoint,
        auth_info: Any | None,
    ) -> dict[str, str]:
        extra_headers = get_extra_headers_from_endpoint(endpoint)
        endpoint_sig = make_signature_key(
            str(getattr(endpoint, "api_family", "")).strip().lower(),
            str(getattr(endpoint, "endpoint_kind", "")).strip().lower(),
        )
        headers = build_upstream_headers_for_endpoint(
            original_headers,
            endpoint_sig,
            upstream_key,
            endpoint_headers=extra_headers,
            header_rules=getattr(endpoint, "header_rules", None),
        )
        if auth_info:
            # 覆盖为 OAuth2 Bearer（Vertex AI）
            headers.pop("x-goog-api-key", None)
            headers[auth_info.auth_header] = auth_info.auth_value
        return headers

    def _format_error_payload(self, error: dict[str, Any], status_code: int) -> dict[str, Any]:
        """Gemini 风格错误格式"""
        return {
            "code": error.get("code", status_code),
            "message": sanitize_error_message(error.get("message", "Request failed")),
            "status": error.get("status", "BAD_GATEWAY"),
        }

    # ------------------------------------------------------------------
    # OpenAI format conversion helpers
    # ------------------------------------------------------------------

    def _build_openai_upstream_url(self, base_url: str | None) -> str:
        """构建 OpenAI Sora API 的上游 URL"""
        base = (base_url or "https://api.openai.com").rstrip("/")
        if base.endswith("/v1"):
            return f"{base}/videos"
        return f"{base}/v1/videos"

    def _build_openai_upstream_headers(
        self,
        original_headers: dict[str, str],
        upstream_key: str,
        endpoint: ProviderEndpoint,
    ) -> dict[str, str]:
        """构建 OpenAI 格式的请求头"""
        extra_headers = get_extra_headers_from_endpoint(endpoint)
        endpoint_sig = make_signature_key(
            str(getattr(endpoint, "api_family", "")).strip().lower(),
            str(getattr(endpoint, "endpoint_kind", "")).strip().lower(),
        )
        return build_upstream_headers_for_endpoint(
            original_headers,
            endpoint_sig,
            upstream_key,
            endpoint_headers=extra_headers,
            header_rules=getattr(endpoint, "header_rules", None),
        )

    def _create_task_record(
        self,
        *,
        external_task_id: str,
        candidate: ProviderCandidate,
        original_request_body: dict[str, Any],
        internal_request: Any,
        candidate_keys: list[dict[str, Any]] | None = None,
        original_headers: dict[str, str] | None = None,
        billing_rule_snapshot: dict[str, Any] | None = None,
        converted_request_body: dict[str, Any] | None = None,
        format_converted: bool = False,
    ) -> VideoTask:
        now = datetime.now(timezone.utc)

        # 构建请求元数据（使用追踪信息）
        request_metadata = {
            "candidate_keys": candidate_keys or [],
            "selected_key_id": candidate.key.id,
            "selected_endpoint_id": candidate.endpoint.id,
            "client_ip": self.client_ip,
            "user_agent": self.user_agent,
            "request_id": self.request_id,
            "billing_rule_snapshot": billing_rule_snapshot,
        }
        # 记录请求头（脱敏处理）
        if original_headers:
            safe_headers = {
                k: v
                for k, v in original_headers.items()
                if k.lower() not in {"authorization", "x-api-key", "x-goog-api-key", "cookie"}
            }
            request_metadata["request_headers"] = safe_headers

        provider_api_format = make_signature_key(
            str(getattr(candidate.endpoint, "api_family", "")).strip().lower(),
            str(getattr(candidate.endpoint, "endpoint_kind", "")).strip().lower(),
        )

        return VideoTask(
            id=str(uuid4()),
            request_id=self.request_id,
            external_task_id=external_task_id,
            user_id=self.user.id,
            api_key_id=self.api_key.id,
            provider_id=candidate.provider.id,
            endpoint_id=candidate.endpoint.id,
            key_id=candidate.key.id,
            client_api_format=self.FORMAT_ID,
            provider_api_format=provider_api_format,
            format_converted=format_converted,
            model=internal_request.model,
            prompt=internal_request.prompt,
            original_request_body=original_request_body,
            converted_request_body=converted_request_body or original_request_body,
            duration_seconds=internal_request.duration_seconds,
            resolution=internal_request.resolution,
            aspect_ratio=internal_request.aspect_ratio,
            status=VideoStatus.SUBMITTED.value,
            progress_percent=0,
            poll_interval_seconds=config.video_poll_interval_seconds,
            next_poll_at=now + timedelta(seconds=config.video_poll_interval_seconds),
            poll_count=0,
            max_poll_count=config.video_max_poll_count,
            submitted_at=now,
            request_metadata=request_metadata,
        )

    def _task_to_internal(self, task: VideoTask) -> InternalVideoTask:
        """覆盖父类方法，Gemini 使用 short_id 作为对外暴露的 ID"""
        try:
            status = VideoStatus(task.status)
        except ValueError:
            status = VideoStatus.PENDING
        return InternalVideoTask(
            id=task.short_id,  # Gemini 使用短 ID
            external_id=task.external_task_id,
            status=status,
            progress_percent=task.progress_percent or 0,
            progress_message=task.progress_message,
            video_url=task.video_url,
            video_urls=task.video_urls or [],
            created_at=task.created_at,
            completed_at=task.completed_at,
            error_code=task.error_code,
            error_message=task.error_message,
            extra={"model": task.model},
        )

    def _get_task_by_external_id(self, external_id: str) -> VideoTask:
        """按 short_id 查找任务（我们对外暴露的 operation 格式是 models/{model}/operations/{short_id}）"""
        from src.api.handlers.base.video_handler_base import extract_short_id_from_operation

        short_id = extract_short_id_from_operation(external_id)

        # 通过 short_id 查找任务
        task = (
            self.db.query(VideoTask)
            .filter(
                VideoTask.short_id == short_id,
                VideoTask.user_id == self.user.id,
            )
            .first()
        )
        if not task:
            logger.debug("[GeminiVeoHandler] Task not found: short_id={}", short_id)
            raise HTTPException(status_code=404, detail="Video task not found")
        return task


__all__ = ["GeminiVeoHandler"]
