"""
流式遥测记录器 - 从 ChatHandlerBase 提取的统计记录逻辑

职责：
1. 记录流式请求的成功/失败统计
2. 更新 Usage 状态
3. 更新候选记录状态
"""

import asyncio
import json
import time
from typing import Any

from sqlalchemy.orm import Session

from src.api.handlers.base.base_handler import MessageTelemetry
from src.api.handlers.base.stream_context import StreamContext
from src.api.handlers.base.utils import filter_proxy_response_headers
from src.config.settings import config
from src.core.logger import logger
from src.database import get_db
from src.models.database import ApiKey, User
from src.services.system.config import SystemConfigService
from src.services.usage.telemetry_writer import (
    DbTelemetryWriter,
    QueueTelemetryWriter,
    TelemetryWriter,
)


class StreamTelemetryRecorder:
    """
    流式遥测记录器

    负责在流式请求完成后记录统计信息。
    从 ChatHandlerBase 中提取的 _record_stream_stats 逻辑。
    """

    def __init__(
        self,
        request_id: str,
        user_id: str,
        api_key_id: str,
        client_ip: str,
        format_id: str,
    ):
        """
        初始化遥测记录器

        Args:
            request_id: 请求 ID
            user_id: 用户 ID
            api_key_id: API Key ID
            client_ip: 客户端 IP
            format_id: API 格式标识
        """
        self.request_id = request_id
        self.user_id = user_id
        self.api_key_id = api_key_id
        self.client_ip = client_ip
        self.format_id = format_id

    async def record_stream_stats(
        self,
        ctx: StreamContext,
        original_headers: dict[str, str],
        original_request_body: dict[str, Any],
        start_time: float,
    ) -> None:
        """
        记录流式统计信息

        Args:
            ctx: 流式上下文
            original_headers: 原始请求头
            original_request_body: 原始请求体
            start_time: 请求开始时间 (time.time())
        """
        bg_db = None

        try:
            # 在流结束后计算响应时间，与首字时间使用相同的时间基准
            # 注意：不要把统计延迟（stream_stats_delay）算进响应时间里
            response_time_ms = int((time.time() - start_time) * 1000)

            await asyncio.sleep(config.stream_stats_delay)  # 等待流完全关闭

            if not ctx.provider_name:
                await self._update_usage_status_on_error(
                    response_time_ms=response_time_ms,
                    error_message="Provider name not available",
                )
                return

            db_gen = get_db()
            bg_db = next(db_gen)

            try:
                writer = await self._get_telemetry_writer(bg_db, ctx, response_time_ms)
                if writer is None:
                    return
                # 兜底估算：流未正常完成且 token 均为 0 时，从请求体粗略估算
                # 覆盖 Chat Handler 路径（CLI Handler 在更早的位置已做估算，
                # 若已估算过则 token > 0，此处条件不会触发）
                if (
                    ctx.is_success()
                    and not ctx.has_completion
                    and ctx.data_count > 0
                    and ctx.input_tokens == 0
                    and ctx.output_tokens == 0
                ):
                    # 用实际发给 Provider 的请求体估算 token（格式转换时与客户端请求体不同）
                    self._estimate_tokens_for_incomplete_stream(
                        ctx, ctx.provider_request_body or original_request_body
                    )

                should_log_body = SystemConfigService.should_log_body(bg_db)
                include_bodies = (
                    writer.include_bodies
                    if isinstance(writer, QueueTelemetryWriter)
                    else should_log_body
                )
                response_body = (
                    ctx.build_response_body(response_time_ms) if include_bodies else None
                )
                client_response_body = (
                    ctx.build_client_response_body(response_time_ms) if include_bodies else None
                )

                try:
                    await self._dispatch_record(
                        bg_db,
                        writer,
                        ctx,
                        original_headers,
                        original_request_body,
                        response_body,
                        response_time_ms,
                        client_response_body=client_response_body,
                    )
                except Exception as writer_error:
                    if not isinstance(writer, QueueTelemetryWriter):
                        raise
                    logger.warning(
                        f"[{self.request_id}] Queue writer failed, falling back to DB: {writer_error}"
                    )
                    db_writer = self._build_db_writer(bg_db)
                    if db_writer is None:
                        await self._update_usage_status_directly(
                            bg_db,
                            status=self._get_status_from_ctx(ctx),
                            response_time_ms=response_time_ms,
                            status_code=ctx.status_code,
                        )
                        return
                    if response_body is None and should_log_body:
                        response_body = ctx.build_response_body(response_time_ms)
                    if client_response_body is None and should_log_body:
                        client_response_body = ctx.build_client_response_body(response_time_ms)
                    await self._dispatch_record(
                        bg_db,
                        db_writer,
                        ctx,
                        original_headers,
                        original_request_body,
                        response_body,
                        response_time_ms,
                        client_response_body=client_response_body,
                    )

                # 更新候选记录状态
                await self._update_candidate_status(bg_db, ctx, response_time_ms, start_time)

            finally:
                if bg_db:
                    bg_db.close()

        except Exception as e:
            logger.exception("记录流式统计信息时出错")
            await self._update_usage_status_on_error(
                response_time_ms=response_time_ms,
                error_message=f"记录统计信息失败: {str(e)[:200]}",
            )

    async def _record_success(
        self,
        writer: TelemetryWriter,
        ctx: StreamContext,
        original_headers: dict[str, str],
        original_request_body: dict[str, Any],
        response_body: dict[str, Any] | None,
        response_time_ms: int,
        client_response_body: dict[str, Any] | None = None,
    ) -> None:
        """记录成功的请求"""
        # 流式成功时，返回给客户端的是提供商响应头 + SSE 必需头
        client_response_headers = filter_proxy_response_headers(ctx.response_headers)
        client_response_headers.update(
            {
                "Cache-Control": "no-cache, no-transform",
                "X-Accel-Buffering": "no",
                "content-type": "text/event-stream",
            }
        )
        metadata: dict[str, Any] = {"stream": True, "content_length": ctx.data_count}
        if ctx.perf_metrics:
            metadata["perf"] = ctx.perf_metrics
        if ctx.proxy_info:
            metadata["proxy"] = ctx.proxy_info
        if ctx.pool_summary:
            metadata["pool_summary"] = ctx.pool_summary
        if ctx.candidate_keys:
            metadata["candidate_keys"] = ctx.candidate_keys
        if ctx.scheduling_audit:
            metadata["scheduling_audit"] = ctx.scheduling_audit

        await writer.record_success(
            provider=ctx.provider_name or "unknown",
            model=ctx.model,
            input_tokens=ctx.input_tokens,
            output_tokens=ctx.output_tokens,
            response_time_ms=response_time_ms,
            first_byte_time_ms=ctx.first_byte_time_ms,  # 传递首字时间
            status_code=ctx.status_code,
            request_headers=original_headers,
            request_body=original_request_body,
            response_headers=ctx.response_headers,
            client_response_headers=client_response_headers,
            response_body=response_body,
            client_response_body=client_response_body,
            provider_request_body=ctx.provider_request_body,
            cache_creation_tokens=ctx.cache_creation_tokens,
            cache_read_tokens=ctx.cached_tokens,
            cache_creation_tokens_5m=ctx.cache_creation_tokens_5m,
            cache_creation_tokens_1h=ctx.cache_creation_tokens_1h,
            is_stream=True,
            provider_request_headers=ctx.provider_request_headers,
            api_format=ctx.api_format,
            api_family=ctx.api_family,
            endpoint_kind=ctx.endpoint_kind,
            provider_id=ctx.provider_id,
            provider_endpoint_id=ctx.endpoint_id,
            provider_api_key_id=ctx.key_id,
            target_model=ctx.mapped_model,
            request_type="chat",
            metadata=metadata,
            endpoint_api_format=ctx.provider_api_format,
            has_format_conversion=ctx.has_format_conversion,
        )

        logger.debug(f"{self.format_id} 流式响应完成")
        logger.info(ctx.get_log_summary(self.request_id, response_time_ms))

    async def _record_failure(
        self,
        writer: TelemetryWriter,
        ctx: StreamContext,
        original_headers: dict[str, str],
        original_request_body: dict[str, Any],
        response_body: dict[str, Any] | None,
        response_time_ms: int,
        client_response_body: dict[str, Any] | None = None,
    ) -> None:
        """记录失败的请求"""
        # 失败时返回给客户端的是 JSON 错误响应，如果没有设置则使用默认值
        client_response_headers = ctx.client_response_headers or {
            "content-type": "application/json"
        }
        metadata: dict[str, Any] = {"stream": True, "content_length": ctx.data_count}
        if ctx.perf_metrics:
            metadata["perf"] = ctx.perf_metrics
        if ctx.proxy_info:
            metadata["proxy"] = ctx.proxy_info
        if ctx.pool_summary:
            metadata["pool_summary"] = ctx.pool_summary
        if ctx.candidate_keys:
            metadata["candidate_keys"] = ctx.candidate_keys
        if ctx.scheduling_audit:
            metadata["scheduling_audit"] = ctx.scheduling_audit

        await writer.record_failure(
            provider=ctx.provider_name or "unknown",
            model=ctx.model,
            response_time_ms=response_time_ms,
            status_code=ctx.status_code,
            error_message=ctx.error_message or f"HTTP {ctx.status_code}",
            request_headers=original_headers,
            request_body=original_request_body,
            is_stream=True,
            api_format=ctx.api_format,
            api_family=ctx.api_family,
            endpoint_kind=ctx.endpoint_kind,
            provider_request_headers=ctx.provider_request_headers,
            provider_request_body=ctx.provider_request_body,
            input_tokens=ctx.input_tokens,
            output_tokens=ctx.output_tokens,
            cache_creation_tokens=ctx.cache_creation_tokens,
            cache_read_tokens=ctx.cached_tokens,
            cache_creation_tokens_5m=ctx.cache_creation_tokens_5m,
            cache_creation_tokens_1h=ctx.cache_creation_tokens_1h,
            response_body=response_body,
            client_response_body=client_response_body,
            response_headers=ctx.response_headers,
            client_response_headers=client_response_headers,
            provider_id=ctx.provider_id,
            provider_endpoint_id=ctx.endpoint_id,
            provider_api_key_id=ctx.key_id,
            target_model=ctx.mapped_model,
            request_type="chat",
            metadata=metadata,
            endpoint_api_format=ctx.provider_api_format,
            has_format_conversion=ctx.has_format_conversion,
        )

        logger.debug(f"{self.format_id} 流式响应中断")
        log_summary = ctx.get_log_summary(self.request_id, response_time_ms)
        # 对于失败日志，添加缓存信息
        logger.info(f"{log_summary} cache:{ctx.cached_tokens}")

    async def _record_cancelled(
        self,
        writer: TelemetryWriter,
        ctx: StreamContext,
        original_headers: dict[str, str],
        original_request_body: dict[str, Any],
        response_body: dict[str, Any] | None,
        response_time_ms: int,
        client_response_body: dict[str, Any] | None = None,
    ) -> None:
        """记录客户端取消的请求"""
        client_response_headers = ctx.client_response_headers or {
            "content-type": "application/json"
        }
        metadata: dict[str, Any] = {"stream": True, "content_length": ctx.data_count}
        if ctx.perf_metrics:
            metadata["perf"] = ctx.perf_metrics
        if ctx.proxy_info:
            metadata["proxy"] = ctx.proxy_info
        if ctx.pool_summary:
            metadata["pool_summary"] = ctx.pool_summary
        if ctx.candidate_keys:
            metadata["candidate_keys"] = ctx.candidate_keys
        if ctx.scheduling_audit:
            metadata["scheduling_audit"] = ctx.scheduling_audit

        await writer.record_cancelled(
            provider=ctx.provider_name or "unknown",
            model=ctx.model,
            response_time_ms=response_time_ms,
            first_byte_time_ms=ctx.first_byte_time_ms,
            status_code=ctx.status_code,
            request_headers=original_headers,
            request_body=original_request_body,
            is_stream=True,
            api_format=ctx.api_format,
            api_family=ctx.api_family,
            endpoint_kind=ctx.endpoint_kind,
            provider_request_headers=ctx.provider_request_headers,
            provider_request_body=ctx.provider_request_body,
            input_tokens=ctx.input_tokens,
            output_tokens=ctx.output_tokens,
            cache_creation_tokens=ctx.cache_creation_tokens,
            cache_read_tokens=ctx.cached_tokens,
            cache_creation_tokens_5m=ctx.cache_creation_tokens_5m,
            cache_creation_tokens_1h=ctx.cache_creation_tokens_1h,
            response_body=response_body,
            client_response_body=client_response_body,
            response_headers=ctx.response_headers,
            client_response_headers=client_response_headers,
            provider_id=ctx.provider_id,
            provider_endpoint_id=ctx.endpoint_id,
            provider_api_key_id=ctx.key_id,
            target_model=ctx.mapped_model,
            request_type="chat",
            metadata=metadata,
            endpoint_api_format=ctx.provider_api_format,
            has_format_conversion=ctx.has_format_conversion,
        )

        logger.debug(f"{self.format_id} 流式响应被客户端取消")
        logger.info(ctx.get_log_summary(self.request_id, response_time_ms))

    async def _update_candidate_status(
        self,
        db: Session,
        ctx: StreamContext,
        response_time_ms: int,
        request_start_time: float,
    ) -> None:
        """更新候选记录状态"""
        if not ctx.attempt_id:
            return

        # Capture all needed ctx attrs before handing off to thread
        attempt_id = ctx.attempt_id
        is_success = ctx.is_success()
        is_disconnected = ctx.is_client_disconnected()
        status_code = ctx.status_code
        data_count = ctx.data_count
        rectified = ctx.rectified
        proxy_info = ctx.proxy_info
        first_byte_time_ms_val = ctx.first_byte_time_ms
        upstream_response = ctx.upstream_response
        error_message = ctx.error_message

        def _sync() -> None:
            from src.services.request.candidate import RequestCandidateService

            extra_data: dict[str, Any] = {
                "stream_completed": is_success,
                "data_count": data_count,
            }
            if rectified:
                extra_data["rectified"] = True
            if proxy_info:
                extra_data["proxy"] = proxy_info
            if first_byte_time_ms_val is not None:
                candidate_ttfb = RequestCandidateService.calculate_candidate_ttfb(
                    db=db,
                    candidate_id=attempt_id,
                    request_start_time=request_start_time,
                    global_first_byte_time_ms=first_byte_time_ms_val,
                )
                extra_data["first_byte_time_ms"] = candidate_ttfb

            if is_success:
                RequestCandidateService.mark_candidate_success(
                    db=db,
                    candidate_id=attempt_id,
                    status_code=status_code,
                    latency_ms=response_time_ms,
                    extra_data=extra_data,
                )
            elif is_disconnected:
                RequestCandidateService.mark_candidate_cancelled(
                    db=db,
                    candidate_id=attempt_id,
                    status_code=status_code,
                    latency_ms=response_time_ms,
                    extra_data=extra_data,
                )
            else:
                trace_error_message = upstream_response or error_message or f"HTTP {status_code}"
                RequestCandidateService.mark_candidate_failed(
                    db=db,
                    candidate_id=attempt_id,
                    error_type="stream_error",
                    error_message=trace_error_message,
                    status_code=status_code,
                    latency_ms=response_time_ms,
                    extra_data=extra_data,
                )

        await asyncio.to_thread(_sync)

    async def _update_usage_status_on_error(
        self,
        response_time_ms: int,
        error_message: str,
    ) -> None:
        """在记录失败时更新 Usage 状态"""
        try:
            db_gen = get_db()
            error_db = next(db_gen)
            try:
                await self._update_usage_status_directly(
                    error_db,
                    status="failed",
                    response_time_ms=response_time_ms,
                    status_code=500,
                    error_message=error_message,
                )
            finally:
                error_db.close()
        except Exception as inner_e:
            logger.error(f"[{self.request_id}] 更新 Usage 状态失败: {inner_e}")

    async def _update_usage_status_directly(
        self,
        db: Session,
        status: str,
        response_time_ms: int,
        status_code: int = 200,
        error_message: str | None = None,
    ) -> None:
        """直接更新 Usage 表的状态字段"""
        request_id = self.request_id

        def _sync() -> None:
            from src.models.database import Usage

            usage = db.query(Usage).filter(Usage.request_id == request_id).first()
            if usage:
                setattr(usage, "status", status)
                setattr(usage, "status_code", status_code)
                setattr(usage, "response_time_ms", response_time_ms)
                if error_message:
                    setattr(usage, "error_message", error_message)
                db.commit()
                logger.debug(f"[{request_id}] Usage 状态已更新: {status}")

        try:
            await asyncio.to_thread(_sync)
        except Exception as e:
            logger.error(f"[{self.request_id}] 直接更新 Usage 状态失败: {e}")

    async def _get_telemetry_writer(
        self, bg_db: Session, ctx: StreamContext, response_time_ms: int
    ) -> TelemetryWriter | None:
        if config.usage_queue_enabled and self.user_id and self.api_key_id:
            # Queue payload detail follows system config request_record_level.
            log_level = SystemConfigService.get_request_record_level(bg_db).value
            sensitive_headers = SystemConfigService.get_sensitive_headers(bg_db) or []
            max_request_body_size = int(
                SystemConfigService.get_config(bg_db, "max_request_body_size", 5242880) or 0
            )
            max_response_body_size = int(
                SystemConfigService.get_config(bg_db, "max_response_body_size", 5242880) or 0
            )
            return QueueTelemetryWriter(
                request_id=self.request_id,
                user_id=self.user_id,
                api_key_id=self.api_key_id,
                log_level=log_level,
                sensitive_headers=sensitive_headers,
                max_request_body_size=max_request_body_size,
                max_response_body_size=max_response_body_size,
            )
        db_writer = self._build_db_writer(bg_db)
        if db_writer is None:
            await self._update_usage_status_directly(
                bg_db,
                status=self._get_status_from_ctx(ctx),
                response_time_ms=response_time_ms,
                status_code=ctx.status_code,
            )
            return None
        return db_writer

    async def _dispatch_record(
        self,
        db: Session,
        writer: TelemetryWriter,
        ctx: StreamContext,
        original_headers: dict[str, str],
        original_request_body: dict[str, Any],
        response_body: dict[str, Any] | None,
        response_time_ms: int,
        client_response_body: dict[str, Any] | None = None,
    ) -> None:
        """根据上下文状态分发到对应的记录方法"""
        if ctx.is_success():
            await self._record_success(
                writer,
                ctx,
                original_headers,
                original_request_body,
                response_body,
                response_time_ms,
                client_response_body=client_response_body,
            )
            # Queue writer 异步落库可能造成 UI 延迟，先直接更新 Usage 状态
            if isinstance(writer, QueueTelemetryWriter):
                await self._update_usage_status_directly(
                    db=db,
                    status=self._get_status_from_ctx(ctx),
                    response_time_ms=response_time_ms,
                    status_code=ctx.status_code,
                )
        elif ctx.is_client_disconnected():
            await self._record_cancelled(
                writer,
                ctx,
                original_headers,
                original_request_body,
                response_body,
                response_time_ms,
                client_response_body=client_response_body,
            )
            # Queue writer 异步落库可能造成 UI 延迟，先直接更新 Usage 状态
            if isinstance(writer, QueueTelemetryWriter):
                await self._update_usage_status_directly(
                    db=db,
                    status="cancelled",
                    response_time_ms=response_time_ms,
                    status_code=ctx.status_code,
                )
        else:
            await self._record_failure(
                writer,
                ctx,
                original_headers,
                original_request_body,
                response_body,
                response_time_ms,
                client_response_body=client_response_body,
            )
            # Queue writer 异步落库可能造成 UI 延迟，先直接更新 Usage 状态
            if isinstance(writer, QueueTelemetryWriter):
                await self._update_usage_status_directly(
                    db=db,
                    status=self._get_status_from_ctx(ctx),
                    response_time_ms=response_time_ms,
                    status_code=ctx.status_code,
                    error_message=ctx.error_message or f"HTTP {ctx.status_code}",
                )

    def _get_status_from_ctx(self, ctx: StreamContext) -> str:
        """根据上下文获取状态字符串"""
        if ctx.is_success():
            return "completed"
        if ctx.is_client_disconnected():
            return "cancelled"
        return "failed"

    @staticmethod
    def _estimate_tokens_for_incomplete_stream(
        ctx: StreamContext,
        request_body: dict[str, Any],
    ) -> None:
        """
        流未正常完成（无 response.completed）且 token 均为 0 时的兜底估算。

        从已收集的输出文本和请求体粗略估算 token 数，确保 usage 记录不为 0。
        估算采用 ~4 字符/token 的保守比例。
        """
        # 输出 tokens：从已收集的文本估算
        if ctx.collected_text_length > 0:
            ctx.output_tokens = max(1, ctx.collected_text_length // 4)

        # 输入 tokens：从请求体文本内容估算
        try:
            total_input_len = 0
            instructions = request_body.get("instructions")
            if isinstance(instructions, str):
                total_input_len += len(instructions)
            # OpenAI Responses API 使用 input 字段；Claude 使用 messages
            input_items = request_body.get("input") or request_body.get("messages") or []
            if isinstance(input_items, list):
                for item in input_items:
                    if isinstance(item, str):
                        total_input_len += len(item)
                    elif isinstance(item, dict):
                        content = item.get("content", "")
                        if isinstance(content, str):
                            total_input_len += len(content)
                        elif isinstance(content, list):
                            for block in content:
                                if isinstance(block, dict):
                                    text = block.get("text", "")
                                    if isinstance(text, str):
                                        total_input_len += len(text)
            if total_input_len > 0:
                ctx.input_tokens = max(1, total_input_len // 4)
            else:
                # fallback: 整个请求体 JSON 大小
                body_str = json.dumps(request_body, ensure_ascii=False)
                ctx.input_tokens = max(1, len(body_str) // 4)
        except Exception:
            pass

        if ctx.input_tokens > 0 or ctx.output_tokens > 0:
            logger.warning(
                f"[{ctx.request_id}] 流未正常完成 (has_completion=False, data_count={ctx.data_count}), "
                f"使用估算 tokens: in={ctx.input_tokens}, out={ctx.output_tokens}"
            )

    def _build_db_writer(self, bg_db: Session) -> DbTelemetryWriter | None:
        user = bg_db.query(User).filter(User.id == self.user_id).first()
        api_key_obj = bg_db.query(ApiKey).filter(ApiKey.id == self.api_key_id).first()

        if not user or not api_key_obj:
            logger.warning(
                f"[{self.request_id}] User or ApiKey not found, updating status directly"
            )
            return None

        bg_telemetry = MessageTelemetry(bg_db, user, api_key_obj, self.request_id, self.client_ip)
        return DbTelemetryWriter(bg_telemetry)
