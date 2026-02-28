"""
OpenAI Video Handler - Sora 视频生成实现
"""

from __future__ import annotations

import json
import time
from datetime import datetime, timedelta, timezone
from typing import Any, AsyncIterator
from uuid import uuid4

import httpx
from fastapi import HTTPException, Request
from fastapi.responses import JSONResponse, Response, StreamingResponse
from sqlalchemy.exc import IntegrityError
from sqlalchemy.orm import Session

from src.api.handlers.base.request_builder import apply_body_rules, get_provider_auth
from src.api.handlers.base.video_handler_base import VideoHandlerBase, sanitize_error_message
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
from src.core.api_format.conversion.normalizers.openai import OpenAINormalizer
from src.core.api_format.conversion.registry import format_conversion_registry
from src.core.api_format.headers import HOP_BY_HOP_HEADERS
from src.core.crypto import crypto_service
from src.core.logger import logger
from src.models.database import ApiKey, ProviderAPIKey, ProviderEndpoint, User, VideoTask
from src.services.billing.rule_service import BillingRuleLookupResult, BillingRuleService
from src.services.scheduling.aware_scheduler import ProviderCandidate
from src.services.usage.service import UsageService


class OpenAIVideoHandler(VideoHandlerBase):
    FORMAT_ID = "openai:video"
    API_FAMILY = ApiFamily.OPENAI
    ENDPOINT_KIND = EndpointKind.VIDEO

    DEFAULT_BASE_URL = "https://api.openai.com"

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
        self._normalizer = OpenAINormalizer()

    async def handle_create_task(
        self,
        *,
        http_request: Request,
        original_headers: dict[str, str],
        original_request_body: dict[str, Any],
        query_params: dict[str, str] | None = None,
        path_params: dict[str, Any] | None = None,
    ) -> JSONResponse:
        try:
            internal_request = self._normalizer.video_request_to_internal(original_request_body)
        except ValueError as e:
            # 请求解析失败，记录失败的使用记录
            await self._record_failed_usage(
                model="unknown",
                error_message=str(e),
                status_code=400,
                original_request_body=original_request_body,
                original_headers=original_headers,
            )
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
            upstream_key, endpoint, _provider_key = await self._resolve_upstream_key(candidate)

            # 检测目标格式
            provider_format = make_signature_key(
                str(getattr(endpoint, "api_family", "")).strip().lower(),
                str(getattr(endpoint, "endpoint_kind", "")).strip().lower(),
            )
            needs_conversion = provider_format.upper() != self.FORMAT_ID.upper()
            format_conversion_info["provider_format"] = provider_format
            format_conversion_info["converted"] = needs_conversion

            # 确保 seconds 字段为字符串类型（上游 Go 服务要求 string）
            request_body = original_request_body.copy()
            if "seconds" in request_body and request_body["seconds"] is not None:
                request_body["seconds"] = str(request_body["seconds"])

            # 应用端点的请求体规则
            endpoint_body_rules = getattr(endpoint, "body_rules", None)

            if needs_conversion and provider_format.upper().startswith("GEMINI:"):
                # OpenAI -> Gemini 格式转换
                converted_body = format_conversion_registry.convert_video_request(
                    request_body,
                    self.FORMAT_ID,
                    provider_format,
                )
                # 如果 model 不在请求体中，从路径或内部请求中获取
                if "model" not in converted_body:
                    converted_body["model"] = internal_request.model

                if endpoint_body_rules:
                    converted_body = apply_body_rules(converted_body, endpoint_body_rules)

                # 构建 Gemini 风格的 URL
                upstream_url = self._build_gemini_upstream_url(
                    endpoint.base_url, internal_request.model
                )

                # 构建 Gemini 风格的请求头
                auth_info = await get_provider_auth(endpoint, _provider_key)
                headers = self._build_gemini_upstream_headers(
                    original_headers, upstream_key, endpoint, auth_info
                )

                client = await HTTPClientPool.get_default_client_async()
                return await client.post(upstream_url, headers=headers, json=converted_body)
            else:
                # 原始 OpenAI 格式
                if endpoint_body_rules:
                    request_body = apply_body_rules(request_body, endpoint_body_rules)

                upstream_url = self._build_upstream_url(endpoint.base_url)
                headers = self._build_upstream_headers(original_headers, upstream_key, endpoint)
                client = await HTTPClientPool.get_default_client_async()
                return await client.post(upstream_url, headers=headers, json=request_body)

        def _extract_task_id(payload: dict[str, Any]) -> str | None:
            # 根据响应格式提取 task ID
            # OpenAI: {"id": "..."}
            # Gemini: {"name": "operations/..."}
            if "id" in payload:
                return str(payload["id"])
            if "name" in payload:
                # Gemini 格式
                return str(payload["name"])
            return None

        # 捕获提交阶段的所有错误，记录失败任务
        try:
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
        except HTTPException as exc:
            # 提交失败（如无可用 provider），创建失败任务记录
            # 尝试获取 candidate_keys（如果是 AllCandidatesFailedError 转换来的）
            candidate_keys = getattr(exc, "candidate_keys", None)
            await self._create_failed_task_and_usage(
                internal_request=internal_request,
                original_request_body=original_request_body,
                original_headers=original_headers,
                error_code="provider_unavailable",
                error_message=exc.detail if isinstance(exc.detail, str) else str(exc.detail),
                status_code=exc.status_code,
                candidate_keys=candidate_keys,
            )
            raise

        if isinstance(outcome_or_response, JSONResponse):
            # 上游返回客户端错误，也要记录
            await self._create_failed_task_and_usage(
                internal_request=internal_request,
                original_request_body=original_request_body,
                original_headers=original_headers,
                error_code="upstream_client_error",
                error_message="Upstream rejected the request",
                status_code=outcome_or_response.status_code,
            )
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
                    "[OpenAIVideoHandler] Failed to record converted request: {}",
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
        except IntegrityError:
            self.db.rollback()
            raise HTTPException(status_code=409, detail="Task already exists")

        # 先构建返回给客户端的响应（OpenAI Sora 使用 UUID）
        internal_task = InternalVideoTask(
            id=task.id,
            external_id=external_task_id,
            status=VideoStatus.SUBMITTED,
            created_at=task.created_at,
            original_request=internal_request,
        )
        response_body = self._normalizer.video_task_from_internal(internal_task)

        # 提交成功后立即结算 Usage（费用暂时为 0，轮询完成后更新）
        response_time_ms = int((time.time() - self.start_time) * 1000)
        try:
            # 构建发送给上游的请求头（脱敏）
            upstream_request_headers = self._build_upstream_headers(
                original_headers,
                "",  # key 不重要，只是用于记录
                outcome.candidate.endpoint,
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
        task = self._get_task(task_id)
        internal_task = self._task_to_internal(task)
        response_body = self._normalizer.video_task_from_internal(internal_task)
        return JSONResponse(response_body)

    async def handle_list_tasks(
        self,
        *,
        http_request: Request,
        original_headers: dict[str, str],
        query_params: dict[str, str] | None = None,
        path_params: dict[str, Any] | None = None,
    ) -> JSONResponse:
        params = query_params or {}

        # 解析分页参数
        after = params.get("after")
        try:
            limit = min(int(params.get("limit") or 20), 100)  # 默认 20，最大 100
        except (ValueError, TypeError):
            limit = 20
        order = params.get("order", "desc").lower()
        if order not in ("asc", "desc"):
            order = "desc"

        # 构建查询
        query = self.db.query(VideoTask).filter(VideoTask.user_id == self.user.id)

        # 处理游标分页（after 参数使用 UUID）
        if after:
            after_task = (
                self.db.query(VideoTask)
                .filter(VideoTask.id == after, VideoTask.user_id == self.user.id)
                .first()
            )
            if after_task and after_task.created_at:
                if order == "desc":
                    query = query.filter(VideoTask.created_at < after_task.created_at)
                else:
                    query = query.filter(VideoTask.created_at > after_task.created_at)

        # 排序
        if order == "asc":
            query = query.order_by(VideoTask.created_at.asc())
        else:
            query = query.order_by(VideoTask.created_at.desc())

        # 获取 limit + 1 条记录以判断是否有更多数据
        tasks = query.limit(limit + 1).all()
        has_more = len(tasks) > limit
        tasks = tasks[:limit]

        items = [
            self._normalizer.video_task_from_internal(self._task_to_internal(t)) for t in tasks
        ]

        response_data: dict[str, Any] = {
            "object": "list",
            "data": items,
            "has_more": has_more,
        }
        # 如果有更多数据，返回最后一条的 ID 作为下一页游标
        if has_more and tasks:
            response_data["last_id"] = tasks[-1].id

        return JSONResponse(response_data)

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

    async def handle_delete_task(
        self,
        *,
        task_id: str,
        http_request: Request,
        original_headers: dict[str, str],
        query_params: dict[str, str] | None = None,
        path_params: dict[str, Any] | None = None,
    ) -> JSONResponse:
        """删除已完成或失败的视频及其存储资源"""
        task = self._get_task(task_id)

        # 只能删除已完成或失败的视频
        if task.status not in (VideoStatus.COMPLETED.value, VideoStatus.FAILED.value):
            raise HTTPException(
                status_code=400,
                detail=f"Can only delete completed or failed videos (current status: {task.status})",
            )

        # 如果有 external_task_id，向上游发送删除请求
        if task.external_task_id:
            try:
                endpoint, key = self._get_endpoint_and_key(task)
                if key.api_key:
                    upstream_key = crypto_service.decrypt(key.api_key)
                    upstream_url = self._build_upstream_url(
                        endpoint.base_url, task.external_task_id
                    )
                    headers = self._build_upstream_headers(original_headers, upstream_key, endpoint)

                    client = await HTTPClientPool.get_default_client_async()
                    response = await client.delete(upstream_url, headers=headers)
                    if response.status_code >= 400 and response.status_code != 404:
                        # 404 表示上游已删除，不算错误
                        return self._build_error_response(response)
            except Exception as exc:
                logger.warning(
                    "Failed to delete video from upstream task={}: {}",
                    task.id,
                    sanitize_error_message(str(exc)),
                )
                # 继续删除本地记录

        # 删除本地任务记录
        self.db.delete(task)
        self.db.commit()

        return JSONResponse({"id": task_id, "object": "video", "deleted": True})

    async def handle_remix_task(
        self,
        *,
        task_id: str,
        http_request: Request,
        original_headers: dict[str, str],
        original_request_body: dict[str, Any],
        query_params: dict[str, str] | None = None,
        path_params: dict[str, Any] | None = None,
    ) -> JSONResponse:
        # 获取原始任务以验证所有权和状态
        original_task = self._get_task(task_id)
        if original_task.status != VideoStatus.COMPLETED.value:
            raise HTTPException(
                status_code=400,
                detail=f"Can only remix completed videos (current status: {original_task.status})",
            )
        if not original_task.external_task_id:
            raise HTTPException(status_code=500, detail="Original task missing external_task_id")

        # 获取原始任务的 endpoint 和 key
        endpoint, key = self._get_endpoint_and_key(original_task)
        if not key.api_key:
            raise HTTPException(status_code=500, detail="Provider key not configured")
        upstream_key = crypto_service.decrypt(key.api_key)

        # 构建 remix 请求的上游 URL
        upstream_url = self._build_upstream_url(
            endpoint.base_url, f"{original_task.external_task_id}/remix"
        )
        headers = self._build_upstream_headers(original_headers, upstream_key, endpoint)

        # 确保 seconds 字段为字符串类型（上游 Go 服务要求 string）
        request_body = original_request_body.copy()
        if "seconds" in request_body and request_body["seconds"] is not None:
            request_body["seconds"] = str(request_body["seconds"])

        # 应用端点的请求体规则
        endpoint_body_rules = getattr(endpoint, "body_rules", None)
        if endpoint_body_rules:
            request_body = apply_body_rules(request_body, endpoint_body_rules)

        client = await HTTPClientPool.get_default_client_async()
        response = await client.post(upstream_url, headers=headers, json=request_body)

        if response.status_code >= 400:
            return self._build_error_response(response)

        # 解析上游响应
        try:
            response_data = response.json()
        except (ValueError, TypeError):
            raise HTTPException(status_code=502, detail="Invalid response from upstream")

        external_task_id = response_data.get("id")
        if not external_task_id:
            raise HTTPException(status_code=502, detail="Upstream did not return task ID")

        # 解析 remix 请求
        try:
            internal_request = self._normalizer.video_request_to_internal(
                {
                    "prompt": original_request_body.get("prompt", ""),
                    "model": original_task.model,
                    "size": original_task.size,
                    "seconds": original_task.duration_seconds,
                }
            )
        except ValueError as e:
            raise HTTPException(status_code=400, detail=str(e))

        # 复用原始任务的 billing rule snapshot
        billing_rule_snapshot = None
        if original_task.request_metadata:
            billing_rule_snapshot = original_task.request_metadata.get("billing_rule_snapshot")

        # 构建 ProviderCandidate（复用原始任务的 provider 配置）
        from src.models.database import Provider

        provider = self.db.query(Provider).filter(Provider.id == original_task.provider_id).first()
        if not provider:
            raise HTTPException(status_code=500, detail="Provider not found")

        candidate = ProviderCandidate(
            provider=provider,
            endpoint=endpoint,
            key=key,
        )

        # 创建新任务记录
        task = self._create_task_record(
            external_task_id=external_task_id,
            candidate=candidate,
            original_request_body={
                **original_request_body,
                "remix_video_id": task_id,
            },
            internal_request=internal_request,
            original_headers=original_headers,
            billing_rule_snapshot=billing_rule_snapshot,
        )

        try:
            self.db.add(task)
            self.db.flush()
            self.db.commit()
            self.db.refresh(task)
        except IntegrityError:
            self.db.rollback()
            raise HTTPException(status_code=409, detail="Task already exists")

        internal_task = InternalVideoTask(
            id=task.id,  # OpenAI Sora 使用 UUID
            external_id=external_task_id,
            status=VideoStatus.SUBMITTED,
            created_at=task.created_at,
            original_request=internal_request,
        )
        response_body = self._normalizer.video_task_from_internal(internal_task)
        return JSONResponse(response_body)

    async def handle_download_content(
        self,
        *,
        task_id: str,
        http_request: Request,
        original_headers: dict[str, str],
        query_params: dict[str, str] | None = None,
        path_params: dict[str, Any] | None = None,
    ) -> Response | StreamingResponse:
        task = self._get_task(task_id)

        # 根据本地任务状态提前返回适当错误（避免不必要的上游请求）
        if task.status in (
            VideoStatus.PENDING.value,
            VideoStatus.SUBMITTED.value,
            VideoStatus.QUEUED.value,
            VideoStatus.PROCESSING.value,
        ):
            raise HTTPException(
                status_code=202,
                detail=f"Video is still processing (status: {task.status})",
            )
        if task.status == VideoStatus.FAILED.value:
            raise HTTPException(
                status_code=422,
                detail=f"Video generation failed: {task.error_message or 'Unknown error'}",
            )
        if task.status == VideoStatus.CANCELLED.value:
            raise HTTPException(status_code=404, detail="Video task was cancelled")

        # 支持 variant 查询参数: video (默认), thumbnail, spritesheet
        variant = (query_params or {}).get("variant", "video")

        # 如果 video_url 是完整的 HTTP URL，直接代理该 URL（适用于不支持 /content 端点的上游如 API易）
        # 保持流式代理而非重定向，确保客户端行为与官方 OpenAI 一致
        if variant == "video" and task.video_url and task.video_url.startswith("http"):
            logger.debug(
                "[VideoDownload] Proxying direct URL task={} url={}",
                task_id,
                task.video_url,
            )
            return await self._proxy_direct_url(task.video_url, task_id)

        if not task.external_task_id:
            raise HTTPException(status_code=500, detail="Task missing external_task_id")
        endpoint, key = self._get_endpoint_and_key(task)
        if not key.api_key:
            raise HTTPException(status_code=500, detail="Provider key not configured")
        upstream_key = crypto_service.decrypt(key.api_key)
        if variant not in {"video", "thumbnail", "spritesheet"}:
            raise HTTPException(
                status_code=400,
                detail=f"Invalid variant '{variant}'. Must be one of: video, thumbnail, spritesheet",
            )

        # 构建上游 URL，透传 variant 参数
        content_path = f"{task.external_task_id}/content"
        if variant != "video":
            content_path = f"{content_path}?variant={variant}"
        upstream_url = self._build_upstream_url(endpoint.base_url, content_path)
        headers = self._build_upstream_headers(original_headers, upstream_key, endpoint)

        client = await HTTPClientPool.get_default_client_async()
        logger.debug(
            "[VideoDownload] Requesting upstream url={} task={} external_task_id={}",
            upstream_url,
            task_id,
            task.external_task_id,
        )
        try:
            # 使用 httpx 的 stream 方法并正确管理上下文
            # 视频下载可能较大，设置 5 分钟超时
            request = client.build_request(
                "GET", upstream_url, headers=headers, timeout=httpx.Timeout(300.0)
            )
            response = await client.send(request, stream=True)
        except Exception as exc:
            logger.warning(
                "[VideoDownload] Upstream connection failed task={} url={}: {}",
                task_id,
                upstream_url,
                sanitize_error_message(str(exc)),
            )
            raise HTTPException(status_code=502, detail="Upstream connection failed") from exc

        if response.status_code >= 400:
            error_body = await response.aread()
            await response.aclose()
            content_type = response.headers.get("content-type", "")
            if "application/json" in content_type:
                try:
                    data = json.loads(error_body)
                    # 脱敏：移除可能的敏感信息
                    if isinstance(data, dict) and isinstance(data.get("error"), dict):
                        if "message" in data["error"]:
                            data["error"]["message"] = sanitize_error_message(
                                str(data["error"]["message"])
                            )
                    return JSONResponse(status_code=response.status_code, content=data)
                except json.JSONDecodeError:
                    pass
            message = sanitize_error_message(error_body.decode(errors="ignore"))
            return JSONResponse(
                status_code=response.status_code,
                content={"error": {"type": "upstream_error", "message": message}},
            )

        async def _iter_bytes() -> AsyncIterator[bytes]:
            try:
                async for chunk in response.aiter_bytes():
                    yield chunk
            finally:
                await response.aclose()

        # 过滤 hop-by-hop 和系统管理头部
        safe_headers = {
            k: v for k, v in response.headers.items() if k.lower() not in HOP_BY_HOP_HEADERS
        }
        return StreamingResponse(
            _iter_bytes(),
            status_code=response.status_code,
            headers=safe_headers,
            media_type=response.headers.get("content-type", "application/octet-stream"),
        )

    # ------------------------------------------------------------------
    # Helpers
    # ------------------------------------------------------------------

    async def _resolve_upstream_key(
        self, candidate: ProviderCandidate
    ) -> tuple[str, ProviderEndpoint, ProviderAPIKey]:
        try:
            upstream_key = crypto_service.decrypt(candidate.key.api_key)
        except Exception as exc:
            logger.error(
                "Failed to decrypt provider key id={}: {}",
                candidate.key.id,
                sanitize_error_message(str(exc)),
            )
            raise HTTPException(status_code=500, detail="Failed to decrypt provider key")
        return upstream_key, candidate.endpoint, candidate.key

    async def _proxy_direct_url(self, url: str, task_id: str) -> Response | StreamingResponse:
        """代理直接的视频 URL（如 CDN URL），保持与官方 API 一致的流式返回行为"""
        client = await HTTPClientPool.get_default_client_async()
        try:
            request = client.build_request("GET", url, timeout=httpx.Timeout(300.0))
            response = await client.send(request, stream=True)
        except Exception as exc:
            logger.warning(
                "[VideoDownload] Direct URL connection failed task={} url={}: {}",
                task_id,
                url,
                sanitize_error_message(str(exc)),
            )
            raise HTTPException(status_code=502, detail="Video download failed") from exc

        if response.status_code >= 400:
            await response.aread()  # consume body before closing
            await response.aclose()
            return JSONResponse(
                status_code=response.status_code,
                content={"error": {"type": "upstream_error", "message": "Video not available"}},
            )

        async def _iter_bytes() -> AsyncIterator[bytes]:
            try:
                async for chunk in response.aiter_bytes():
                    yield chunk
            finally:
                await response.aclose()

        safe_headers = {
            k: v for k, v in response.headers.items() if k.lower() not in HOP_BY_HOP_HEADERS
        }
        return StreamingResponse(
            _iter_bytes(),
            status_code=response.status_code,
            headers=safe_headers,
            media_type=response.headers.get("content-type", "video/mp4"),
        )

    def _build_upstream_url(self, base_url: str | None, suffix: str | None = None) -> str:
        base = (base_url or self.DEFAULT_BASE_URL).rstrip("/")
        if base.endswith("/v1"):
            url = f"{base}/videos"
        else:
            url = f"{base}/v1/videos"
        if suffix:
            return f"{url}/{suffix}"
        return url

    def _build_upstream_headers(
        self, original_headers: dict[str, str], upstream_key: str, endpoint: ProviderEndpoint
    ) -> dict[str, str]:
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

    # ------------------------------------------------------------------
    # Gemini format conversion helpers
    # ------------------------------------------------------------------

    def _build_gemini_upstream_url(self, base_url: str | None, model: str) -> str:
        """构建 Gemini Veo API 的上游 URL"""
        base = (base_url or "https://generativelanguage.googleapis.com").rstrip("/")
        if base.endswith("/v1beta"):
            base = base[: -len("/v1beta")]
        return f"{base}/v1beta/models/{model}:predictLongRunning"

    def _build_gemini_upstream_headers(
        self,
        original_headers: dict[str, str],
        upstream_key: str,
        endpoint: ProviderEndpoint,
        auth_info: Any | None,
    ) -> dict[str, str]:
        """构建 Gemini 格式的请求头"""
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

    # _build_error_response 继承自基类 VideoHandlerBase

    def _create_task_record(
        self,
        *,
        external_task_id: str,
        candidate: ProviderCandidate,
        original_request_body: dict[str, Any],
        internal_request: InternalVideoRequest,
        candidate_keys: list[dict[str, Any]] | None = None,
        original_headers: dict[str, str] | None = None,
        billing_rule_snapshot: dict[str, Any] | None = None,
        converted_request_body: dict[str, Any] | None = None,
        format_converted: bool = False,
    ) -> VideoTask:
        now = datetime.now(timezone.utc)
        size = internal_request.extra.get("original_size")

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
                if k.lower() not in {"authorization", "x-api-key", "cookie"}
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
            size=size,
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
        try:
            status = VideoStatus(task.status)
        except ValueError:
            status = VideoStatus.PENDING

        # 构建 extra 字段
        extra: dict[str, Any] = {
            "model": task.model,
            "size": task.size,
            "seconds": str(task.duration_seconds) if task.duration_seconds else None,
            "prompt": task.prompt,
        }

        # 检查是否是 remix 视频
        if task.original_request_body and isinstance(task.original_request_body, dict):
            remixed_from = task.original_request_body.get("remix_video_id")
            if remixed_from:
                extra["remixed_from_video_id"] = remixed_from

        return InternalVideoTask(
            id=task.id,  # OpenAI Sora 使用 UUID
            external_id=task.external_task_id,
            status=status,
            progress_percent=task.progress_percent or 0,
            progress_message=task.progress_message,
            video_url=task.video_url,
            video_urls=task.video_urls or [],
            thumbnail_url=task.thumbnail_url,
            video_duration_seconds=task.duration_seconds,
            video_size_bytes=task.video_size_bytes,
            created_at=task.created_at,
            completed_at=task.completed_at,
            expires_at=task.video_expires_at,
            error_code=task.error_code,
            error_message=task.error_message,
            extra=extra,
        )

    async def _record_failed_usage(
        self,
        *,
        model: str,
        error_message: str,
        status_code: int,
        original_request_body: dict[str, Any],
        original_headers: dict[str, str],
    ) -> None:
        """记录失败请求的使用记录（无任务记录）"""
        response_time_ms = int((time.time() - self.start_time) * 1000)
        safe_headers = {
            k: v
            for k, v in original_headers.items()
            if k.lower() not in {"authorization", "x-api-key", "cookie"}
        }

        try:
            await UsageService.record_usage_with_custom_cost(
                db=self.db,
                user=self.user,
                api_key=self.api_key,
                provider="unknown",
                model=model,
                request_type="video",
                total_cost_usd=0.0,
                request_cost_usd=0.0,
                input_tokens=0,
                output_tokens=0,
                cache_creation_input_tokens=0,
                cache_read_input_tokens=0,
                api_format=self.FORMAT_ID,
                api_family=self.API_FAMILY.value if self.API_FAMILY else None,
                endpoint_kind=self.ENDPOINT_KIND.value if self.ENDPOINT_KIND else None,
                endpoint_api_format=None,
                has_format_conversion=False,
                is_stream=False,
                response_time_ms=response_time_ms,
                first_byte_time_ms=None,
                status_code=status_code,
                error_message=error_message,
                metadata={"failure_stage": "request_parsing"},
                request_headers=safe_headers,
                request_body=original_request_body,
                provider_request_headers=None,
                response_headers=None,
                client_response_headers=None,
                response_body=None,
                request_id=self.request_id,
                provider_id=None,
                provider_endpoint_id=None,
                provider_api_key_id=None,
                status="failed",
                target_model=None,
            )
        except Exception as exc:
            logger.warning("Failed to record failed usage: {}", sanitize_error_message(str(exc)))

    async def _create_failed_task_and_usage(
        self,
        *,
        internal_request: InternalVideoRequest,
        original_request_body: dict[str, Any],
        original_headers: dict[str, str],
        error_code: str,
        error_message: str,
        status_code: int,
        candidate_keys: list[dict[str, Any]] | None = None,
    ) -> None:
        """创建失败的任务记录和使用记录"""
        now = datetime.now(timezone.utc)
        response_time_ms = int((time.time() - self.start_time) * 1000)

        # 构建请求元数据
        safe_headers = {
            k: v
            for k, v in original_headers.items()
            if k.lower() not in {"authorization", "x-api-key", "cookie"}
        }
        request_metadata: dict[str, Any] = {
            "client_ip": self.client_ip,
            "user_agent": self.user_agent,
            "request_id": self.request_id,
            "request_headers": safe_headers,
            "failure_stage": "submit",
        }
        # 添加候选链路追踪信息
        if candidate_keys:
            request_metadata["candidate_keys"] = candidate_keys

        size = internal_request.extra.get("original_size")

        # 创建失败的任务记录
        task = VideoTask(
            id=str(uuid4()),
            request_id=self.request_id,
            external_task_id=None,
            user_id=self.user.id,
            api_key_id=self.api_key.id,
            provider_id=None,
            endpoint_id=None,
            key_id=None,
            client_api_format=self.FORMAT_ID,
            provider_api_format=self.FORMAT_ID,
            format_converted=False,
            model=internal_request.model,
            prompt=internal_request.prompt,
            original_request_body=original_request_body,
            converted_request_body=original_request_body,
            duration_seconds=internal_request.duration_seconds,
            resolution=internal_request.resolution,
            aspect_ratio=internal_request.aspect_ratio,
            size=size,
            status=VideoStatus.FAILED.value,
            progress_percent=0,
            error_code=error_code,
            error_message=error_message,
            submitted_at=now,
            completed_at=now,
            request_metadata=request_metadata,
        )

        try:
            self.db.add(task)
            self.db.commit()
            self.db.refresh(task)
        except Exception as exc:
            self.db.rollback()
            logger.warning(
                "Failed to create failed task record: {}", sanitize_error_message(str(exc))
            )
            # 即使任务记录失败，仍然尝试记录使用记录
            task = None

        # 记录使用记录
        try:
            await UsageService.record_usage_with_custom_cost(
                db=self.db,
                user=self.user,
                api_key=self.api_key,
                provider="unknown",
                model=internal_request.model,
                request_type="video",
                total_cost_usd=0.0,
                request_cost_usd=0.0,
                input_tokens=0,
                output_tokens=0,
                cache_creation_input_tokens=0,
                cache_read_input_tokens=0,
                api_format=self.FORMAT_ID,
                api_family=self.API_FAMILY.value if self.API_FAMILY else None,
                endpoint_kind=self.ENDPOINT_KIND.value if self.ENDPOINT_KIND else None,
                endpoint_api_format=None,
                has_format_conversion=False,
                is_stream=False,
                response_time_ms=response_time_ms,
                first_byte_time_ms=None,
                status_code=status_code,
                error_message=error_message,
                metadata={
                    "failure_stage": "submit",
                    "error_code": error_code,
                    "video_task_id": task.id if task else None,
                },
                request_headers=safe_headers,
                request_body=original_request_body,
                provider_request_headers=None,
                response_headers=None,
                client_response_headers=None,
                response_body=None,
                request_id=self.request_id,
                provider_id=None,
                provider_endpoint_id=None,
                provider_api_key_id=None,
                status="failed",
                target_model=None,
            )
        except Exception as exc:
            logger.warning("Failed to record failed usage: {}", sanitize_error_message(str(exc)))


__all__ = ["OpenAIVideoHandler"]
