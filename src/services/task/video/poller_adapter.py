"""
Video task poller adapter.

Implements the video-specific poll/normalize/update logic used by TaskPollerService.

优化：HTTP 请求期间不持有数据库连接，避免阻塞其他请求。
采用三阶段处理：准备数据 -> HTTP 请求 -> 更新数据库。
"""

from __future__ import annotations

import json
from collections.abc import Awaitable, Callable
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from typing import Any

from sqlalchemy.orm import Session

from src.clients.http_client import HTTPClientPool
from src.config.settings import config
from src.core.api_format import (
    build_upstream_headers_for_endpoint,
    get_extra_headers_from_endpoint,
    make_signature_key,
)
from src.core.api_format.conversion.internal_video import InternalVideoPollResult, VideoStatus
from src.core.api_format.conversion.normalizers.gemini import GeminiNormalizer
from src.core.api_format.conversion.normalizers.openai import OpenAINormalizer
from src.core.crypto import crypto_service
from src.core.logger import logger
from src.core.provider_auth_types import ProviderAuthInfo
from src.core.video_utils import (
    normalize_gemini_operation_id,
    sanitize_error_message,
)
from src.database import create_session
from src.models.database import ProviderAPIKey, ProviderEndpoint, VideoTask
from src.services.provider.auth import get_provider_auth


@dataclass(slots=True)
class VideoPollContext:
    """视频轮询上下文，保存 HTTP 请求所需的数据（不依赖数据库会话）"""

    task_id: str
    external_task_id: str
    provider_api_format: str
    base_url: str
    upstream_key: str
    headers: dict[str, str]
    # 用于更新任务的原始数据
    poll_count: int
    retry_count: int
    poll_interval_seconds: int
    max_poll_count: int
    current_status: str


# 永久性错误指示词（用于降级判断，不应重试）
_PERMANENT_ERROR_INDICATORS = frozenset(
    {
        "not found",
        "404",
        "unauthorized",
        "401",
        "forbidden",
        "403",
        "invalid request",
        "invalid api key",
        "does not exist",
    }
)


class PollHTTPError(RuntimeError):
    """HTTP 轮询错误，携带状态码便于区分临时/永久错误"""

    def __init__(self, status_code: int, message: str):
        # 确保错误信息包含状态码
        full_message = f"HTTP {status_code}: {message}" if message else f"HTTP {status_code}"
        super().__init__(full_message)
        self.status_code = status_code
        self.original_message = message


VideoTaskFinalizeFn = Callable[[Session, VideoTask, Any | None], Awaitable[None]]


async def _default_finalize_video_task(
    db: Session,
    task: VideoTask,
    redis_client: Any | None,
) -> None:
    """默认终态结算逻辑（延迟导入，避免 task 模块循环依赖）。"""
    from src.services.task.video.operations import VideoTaskOperationsService

    await VideoTaskOperationsService(db, redis_client=redis_client).finalize_video_task(task)


class VideoTaskPollerAdapter:
    task_type = "video"

    # scheduler
    job_id = "task_poller:video"
    job_name = "视频任务轮询"
    interval_seconds = config.video_poll_interval_seconds

    # distributed lock
    lock_key = "task_poller:video:lock"
    lock_ttl = 60

    # execution
    batch_size = config.video_poll_batch_size
    concurrency = config.video_poll_concurrency
    consecutive_failure_alert_threshold = 5
    max_backoff_seconds = 300

    def __init__(self, finalize_video_task_fn: VideoTaskFinalizeFn | None = None) -> None:
        self._openai_normalizer = OpenAINormalizer()
        self._gemini_normalizer = GeminiNormalizer()
        self._finalize_video_task = finalize_video_task_fn or _default_finalize_video_task

    def sanitize_error_message(self, message: str) -> str:
        return sanitize_error_message(message)

    def list_due_task_ids(self, db: Session, *, now: datetime, limit: int) -> list[str]:
        tasks = (
            db.query(VideoTask)
            .filter(
                VideoTask.status.in_(
                    [
                        VideoStatus.SUBMITTED.value,
                        VideoStatus.QUEUED.value,
                        VideoStatus.PROCESSING.value,
                    ]
                ),
                VideoTask.next_poll_at <= now,
                VideoTask.poll_count < VideoTask.max_poll_count,
            )
            .order_by(VideoTask.next_poll_at.asc())
            .limit(limit)
            .all()
        )
        return [t.id for t in tasks]

    def get_task(self, db: Session, task_id: str) -> VideoTask | None:
        # SQLAlchemy 1.4+ API
        return db.get(VideoTask, task_id)

    # ==================== 分阶段处理方法（优化数据库连接占用）====================

    async def prepare_poll_context(
        self, db: Session, task: VideoTask
    ) -> VideoPollContext | InternalVideoPollResult:
        """
        阶段 1：准备轮询上下文（短暂持有数据库连接）

        Returns:
            VideoPollContext: 成功时返回上下文
            InternalVideoPollResult: 失败时返回错误结果
        """
        if not task.endpoint_id or not task.key_id:
            return InternalVideoPollResult(
                status=VideoStatus.FAILED,
                error_code="missing_provider_info",
                error_message="Task missing endpoint_id or key_id",
            )

        endpoint = self._get_endpoint(db, task.endpoint_id)
        key = self._get_key(db, task.key_id)

        if not key.api_key:
            return InternalVideoPollResult(
                status=VideoStatus.FAILED,
                error_code="provider_config_error",
                error_message="Provider key not properly configured",
            )

        try:
            upstream_key = crypto_service.decrypt(key.api_key)
        except Exception:
            logger.warning("Failed to decrypt provider key for task {}", task.id)
            return InternalVideoPollResult(
                status=VideoStatus.FAILED,
                error_code="decryption_error",
                error_message="Failed to decrypt provider key",
            )

        provider_format = (task.provider_api_format or "").strip().lower()
        if not provider_format:
            provider_format = make_signature_key(
                str(getattr(endpoint, "api_family", "")).strip().lower(),
                str(getattr(endpoint, "endpoint_kind", "")).strip().lower(),
            )

        # 构建请求头
        if provider_format.startswith("gemini:"):
            auth_info = await get_provider_auth(endpoint, key)
        else:
            auth_info = None
        headers = self._build_headers(provider_format, upstream_key, endpoint, auth_info)

        return VideoPollContext(
            task_id=task.id,
            external_task_id=task.external_task_id or "",
            provider_api_format=provider_format,
            base_url=endpoint.base_url or "",
            upstream_key=upstream_key,
            headers=headers,
            poll_count=task.poll_count,
            retry_count=task.retry_count,
            poll_interval_seconds=task.poll_interval_seconds,
            max_poll_count=task.max_poll_count,
            current_status=task.status,
        )

    async def poll_task_http(self, ctx: VideoPollContext) -> InternalVideoPollResult:
        """
        阶段 2：执行 HTTP 请求（不持有数据库连接）

        Args:
            ctx: 轮询上下文

        Returns:
            InternalVideoPollResult: 轮询结果
        """
        if not ctx.external_task_id:
            return InternalVideoPollResult(
                status=VideoStatus.FAILED,
                error_code="missing_external_task_id",
                error_message="Task missing external_task_id",
            )

        if ctx.provider_api_format.startswith("gemini:"):
            return await self._poll_gemini_with_context(ctx)
        return await self._poll_openai_with_context(ctx)

    async def update_task_after_poll(
        self,
        task_id: str,
        result: InternalVideoPollResult,
        ctx: VideoPollContext | None,
        redis_client: Any | None,
        error_exception: Exception | None = None,
    ) -> None:
        """
        阶段 3：更新数据库（获取新的数据库连接）

        Args:
            task_id: 任务 ID
            result: 轮询结果
            ctx: 轮询上下文（准备阶段就失败时为 None）
            redis_client: Redis 客户端
            error_exception: 如果 HTTP 请求失败，传入异常对象
        """
        with create_session() as db:
            task = db.get(VideoTask, task_id)
            if not task:
                logger.warning("Task {} disappeared during poll update", task_id)
                return

            if error_exception is not None and ctx is not None:
                # HTTP 请求失败（需要 ctx 来计算 backoff）
                self._handle_poll_error(task, error_exception, ctx)
            elif result.status == VideoStatus.COMPLETED:
                task.status = VideoStatus.COMPLETED.value
                task.video_url = result.video_url
                task.video_expires_at = result.expires_at
                task.completed_at = datetime.now(timezone.utc)
                task.progress_percent = 100
                if result.video_urls:
                    task.video_urls = result.video_urls
                if result.video_duration_seconds is not None:
                    task.video_duration_seconds = result.video_duration_seconds
                self._attach_poll_raw_response(task, result)
            elif result.status == VideoStatus.FAILED:
                task.status = VideoStatus.FAILED.value
                task.error_code = result.error_code
                task.error_message = result.error_message
                task.completed_at = datetime.now(timezone.utc)
                self._attach_poll_raw_response(task, result)
            else:
                task.poll_count += 1
                task.progress_percent = result.progress_percent
                task.next_poll_at = datetime.now(timezone.utc) + timedelta(
                    seconds=task.poll_interval_seconds
                )

            # 超时检查
            task.updated_at = datetime.now(timezone.utc)
            if task.poll_count >= task.max_poll_count and task.status not in [
                VideoStatus.COMPLETED.value,
                VideoStatus.FAILED.value,
                VideoStatus.CANCELLED.value,
            ]:
                task.status = VideoStatus.FAILED.value
                task.error_code = "poll_timeout"
                task.error_message = f"Task timed out after {task.poll_count} polls"
                task.completed_at = datetime.now(timezone.utc)

            # 终态结算
            if task.status in (VideoStatus.COMPLETED.value, VideoStatus.FAILED.value):
                try:
                    await self._finalize_video_task(db, task, redis_client)
                except Exception as exc:
                    logger.exception(
                        "Failed to record video usage for task={}: {}",
                        task.id,
                        sanitize_error_message(str(exc)),
                    )

            db.commit()

    def _handle_poll_error(self, task: VideoTask, exc: Exception, ctx: VideoPollContext) -> None:
        """处理轮询错误"""
        task.poll_count += 1
        error_msg = sanitize_error_message(str(exc))
        logger.warning("Poll error for task {}: {}", task.id, error_msg)
        task.progress_message = f"Poll error: {error_msg}"

        status_code = exc.status_code if isinstance(exc, PollHTTPError) else None
        is_permanent = self._is_permanent_error(exc, status_code=status_code)
        if is_permanent:
            task.status = VideoStatus.FAILED.value
            task.error_code = "poll_permanent_error"
            task.error_message = error_msg
            task.completed_at = datetime.now(timezone.utc)
        else:
            backoff = min(
                ctx.poll_interval_seconds * (2 ** min(ctx.retry_count, 5)),
                self.max_backoff_seconds,
            )
            task.retry_count += 1
            task.next_poll_at = datetime.now(timezone.utc) + timedelta(seconds=backoff)

    async def _poll_openai_with_context(self, ctx: VideoPollContext) -> InternalVideoPollResult:
        """使用上下文进行 OpenAI 轮询（不需要数据库）"""
        url = self._build_openai_url(ctx.base_url, ctx.external_task_id)

        client = await HTTPClientPool.get_default_client_async()
        response = await client.get(url, headers=ctx.headers)
        if response.status_code >= 400:
            error_message = self._extract_error_message(response.text, response.status_code)
            raise PollHTTPError(response.status_code, error_message)

        payload = response.json()
        return self._openai_normalizer.video_poll_to_internal(payload)

    async def _poll_gemini_with_context(self, ctx: VideoPollContext) -> InternalVideoPollResult:
        """使用上下文进行 Gemini 轮询（不需要数据库）"""
        operation_name = normalize_gemini_operation_id(ctx.external_task_id)
        url = self._build_gemini_url(ctx.base_url, operation_name)

        logger.debug(
            "[VideoPoller] Gemini poll: task={} external_id={} url={}",
            ctx.task_id,
            ctx.external_task_id,
            url,
        )

        client = await HTTPClientPool.get_default_client_async()
        response = await client.get(url, headers=ctx.headers)
        if response.status_code >= 400:
            logger.warning(
                "[VideoPoller] Gemini poll failed: task={} status={} response={}",
                ctx.task_id,
                response.status_code,
                response.text[:500] if response.text else "(empty)",
            )
            error_message = self._extract_error_message(response.text, response.status_code)
            raise PollHTTPError(response.status_code, error_message)

        payload = response.json()
        return self._gemini_normalizer.video_poll_to_internal(payload)

    # ==================== 旧版方法（保留兼容性）====================

    async def poll_single_task(
        self, db: Session, task: VideoTask, *, redis_client: Any | None
    ) -> None:
        """
        兼容入口：复用三阶段轮询流程，避免维护重复逻辑。
        """
        ctx_or_result = await self.prepare_poll_context(db, task)

        if isinstance(ctx_or_result, InternalVideoPollResult):
            await self.update_task_after_poll(
                task_id=task.id,
                result=ctx_or_result,
                ctx=None,
                redis_client=redis_client,
            )
            return

        ctx = ctx_or_result
        error_exception: Exception | None = None
        try:
            result = await self.poll_task_http(ctx)
        except Exception as http_exc:
            error_exception = http_exc
            result = InternalVideoPollResult(
                status=None,  # type: ignore[arg-type]
                error_message=str(http_exc),
            )

        await self.update_task_after_poll(
            task_id=task.id,
            result=result,
            ctx=ctx,
            redis_client=redis_client,
            error_exception=error_exception,
        )

    def _attach_poll_raw_response(self, task: VideoTask, result: InternalVideoPollResult) -> None:
        if not result.raw_response:
            return
        # 重新赋值整个字典，确保 SQLAlchemy 检测到变更
        # （直接修改 JSON 字段内部不会自动标记为 dirty）
        metadata = dict(task.request_metadata) if task.request_metadata else {}
        metadata["poll_raw_response"] = result.raw_response
        task.request_metadata = metadata

    def _is_permanent_error(self, exc: Exception, status_code: int | None = None) -> bool:
        if status_code is not None:
            return 400 <= status_code < 500 and status_code != 429
        error_msg = str(exc).lower()
        return any(indicator in error_msg for indicator in _PERMANENT_ERROR_INDICATORS)

    async def _poll_task_status(self, db: Session, task: VideoTask) -> InternalVideoPollResult:
        if not task.endpoint_id or not task.key_id:
            return InternalVideoPollResult(
                status=VideoStatus.FAILED,
                error_code="missing_provider_info",
                error_message="Task missing endpoint_id or key_id",
            )
        endpoint = self._get_endpoint(db, task.endpoint_id)
        key = self._get_key(db, task.key_id)
        if not key.api_key:
            return InternalVideoPollResult(
                status=VideoStatus.FAILED,
                error_code="provider_config_error",
                error_message="Provider key not properly configured",
            )
        try:
            upstream_key = crypto_service.decrypt(key.api_key)
        except Exception:
            logger.warning("Failed to decrypt provider key for task {}", task.id)
            return InternalVideoPollResult(
                status=VideoStatus.FAILED,
                error_code="decryption_error",
                error_message="Failed to decrypt provider key",
            )

        provider_format = (task.provider_api_format or "").strip().lower()
        if not provider_format:
            provider_format = make_signature_key(
                str(getattr(endpoint, "api_family", "")).strip().lower(),
                str(getattr(endpoint, "endpoint_kind", "")).strip().lower(),
            )

        if provider_format.startswith("gemini:"):
            auth_info = await get_provider_auth(endpoint, key)
            return await self._poll_gemini(task, endpoint, upstream_key, auth_info)
        return await self._poll_openai(task, endpoint, upstream_key)

    async def _poll_openai(
        self,
        task: VideoTask,
        endpoint: ProviderEndpoint,
        upstream_key: str,
    ) -> InternalVideoPollResult:
        if not task.external_task_id:
            return InternalVideoPollResult(
                status=VideoStatus.FAILED,
                error_code="missing_external_task_id",
                error_message="Task missing external_task_id",
            )
        url = self._build_openai_url(endpoint.base_url, task.external_task_id)
        endpoint_sig = (task.provider_api_format or "").strip().lower() or make_signature_key(
            str(getattr(endpoint, "api_family", "")).strip().lower(),
            str(getattr(endpoint, "endpoint_kind", "")).strip().lower(),
        )
        headers = self._build_headers(endpoint_sig, upstream_key, endpoint)

        client = await HTTPClientPool.get_default_client_async()
        response = await client.get(url, headers=headers)
        if response.status_code >= 400:
            error_message = self._extract_error_message(response.text, response.status_code)
            raise PollHTTPError(response.status_code, error_message)

        payload = response.json()
        return self._openai_normalizer.video_poll_to_internal(payload)

    async def _poll_gemini(
        self,
        task: VideoTask,
        endpoint: ProviderEndpoint,
        upstream_key: str,
        auth_info: ProviderAuthInfo | None,
    ) -> InternalVideoPollResult:
        if not task.external_task_id:
            return InternalVideoPollResult(
                status=VideoStatus.FAILED,
                error_code="missing_external_task_id",
                error_message="Task missing external_task_id",
            )
        operation_name = normalize_gemini_operation_id(task.external_task_id)
        url = self._build_gemini_url(endpoint.base_url, operation_name)
        endpoint_sig = (task.provider_api_format or "").strip().lower() or make_signature_key(
            str(getattr(endpoint, "api_family", "")).strip().lower(),
            str(getattr(endpoint, "endpoint_kind", "")).strip().lower(),
        )
        headers = self._build_headers(endpoint_sig, upstream_key, endpoint, auth_info)

        logger.debug(
            "[VideoPoller] Gemini poll: task={} external_id={} url={}",
            task.id,
            task.external_task_id,
            url,
        )

        client = await HTTPClientPool.get_default_client_async()
        response = await client.get(url, headers=headers)
        if response.status_code >= 400:
            logger.warning(
                "[VideoPoller] Gemini poll failed: task={} status={} response={}",
                task.id,
                response.status_code,
                response.text[:500] if response.text else "(empty)",
            )
            error_message = self._extract_error_message(response.text, response.status_code)
            raise PollHTTPError(response.status_code, error_message)

        payload = response.json()
        return self._gemini_normalizer.video_poll_to_internal(payload)

    def _build_openai_url(self, base_url: str | None, task_id: str) -> str:
        base = (base_url or "https://api.openai.com").rstrip("/")
        if base.endswith("/v1"):
            return f"{base}/videos/{task_id}"
        return f"{base}/v1/videos/{task_id}"

    def _build_gemini_url(self, base_url: str | None, operation_name: str) -> str:
        base = (base_url or "https://generativelanguage.googleapis.com").rstrip("/")
        if base.endswith("/v1beta"):
            base = base[: -len("/v1beta")]
        return f"{base}/v1beta/{operation_name}"

    def _build_headers(
        self,
        endpoint_sig: str,
        upstream_key: str,
        endpoint: ProviderEndpoint,
        auth_info: ProviderAuthInfo | None = None,
    ) -> dict[str, str]:
        extra_headers = get_extra_headers_from_endpoint(endpoint)
        headers = build_upstream_headers_for_endpoint(
            {},
            endpoint_sig,
            upstream_key,
            endpoint_headers=extra_headers,
            header_rules=getattr(endpoint, "header_rules", None),
        )
        if auth_info:
            headers.pop("x-goog-api-key", None)
            headers[auth_info.auth_header] = auth_info.auth_value
        return headers

    def _get_endpoint(self, db: Session, endpoint_id: str) -> ProviderEndpoint:
        endpoint = db.query(ProviderEndpoint).filter(ProviderEndpoint.id == endpoint_id).first()
        if not endpoint:
            raise RuntimeError("Provider endpoint not found")
        return endpoint

    def _get_key(self, db: Session, key_id: str) -> ProviderAPIKey:
        key = db.query(ProviderAPIKey).filter(ProviderAPIKey.id == key_id).first()
        if not key:
            raise RuntimeError("Provider key not found")
        return key

    def _extract_error_message(self, response_text: str | None, status_code: int) -> str:
        """从响应中提取有意义的错误信息"""
        if not response_text:
            return f"Request failed with status {status_code}"

        # 尝试解析 JSON 格式的错误
        try:
            data = json.loads(response_text)
            # OpenAI 格式: {"error": {"message": "..."}}
            if isinstance(data.get("error"), dict):
                error_obj = data["error"]
                message = error_obj.get("message") or error_obj.get("detail") or str(error_obj)
                return sanitize_error_message(message)
            # Gemini 格式: {"error": {"message": "...", "code": 404}}
            if "message" in data:
                return sanitize_error_message(data["message"])
        except (json.JSONDecodeError, TypeError, KeyError):
            pass

        # 回退到原始文本（截断）
        return sanitize_error_message(response_text[:500])
