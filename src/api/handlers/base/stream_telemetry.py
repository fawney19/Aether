"""
流式遥测记录器 - 从 ChatHandlerBase 提取的统计记录逻辑

职责：
1. 记录流式请求的成功/失败统计
2. 更新 Usage 状态
3. 更新候选记录状态
"""

import asyncio
import time
from typing import Any, Dict, Optional

from sqlalchemy.orm import Session

from src.api.handlers.base.base_handler import MessageTelemetry
from src.api.handlers.base.stream_context import StreamContext
from src.config.settings import config
from src.core.logger import logger
from src.database import get_db
from src.models.database import ApiKey, User


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
        original_headers: Dict[str, str],
        original_request_body: Dict[str, Any],
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
                user = bg_db.query(User).filter(User.id == self.user_id).first()
                api_key_obj = bg_db.query(ApiKey).filter(ApiKey.id == self.api_key_id).first()

                if not user or not api_key_obj:
                    logger.warning(
                        f"[{self.request_id}] User or ApiKey not found, updating status directly"
                    )
                    await self._update_usage_status_directly(
                        bg_db,
                        status="completed" if ctx.is_success() else "failed",
                        response_time_ms=response_time_ms,
                        status_code=ctx.status_code,
                    )
                    return

                bg_telemetry = MessageTelemetry(
                    bg_db, user, api_key_obj, self.request_id, self.client_ip
                )

                actual_request_body = ctx.provider_request_body or original_request_body
                response_body = ctx.build_response_body(response_time_ms)

                if ctx.is_success():
                    await self._record_success(
                        bg_telemetry,
                        ctx,
                        original_headers,
                        actual_request_body,
                        response_body,
                        response_time_ms,
                    )
                else:
                    await self._record_failure(
                        bg_telemetry,
                        ctx,
                        original_headers,
                        actual_request_body,
                        response_body,
                        response_time_ms,
                    )

                # 更新候选记录状态
                await self._update_candidate_status(bg_db, ctx, response_time_ms)

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
        telemetry: MessageTelemetry,
        ctx: StreamContext,
        original_headers: Dict[str, str],
        actual_request_body: Dict[str, Any],
        response_body: Dict[str, Any],
        response_time_ms: int,
    ) -> None:
        """记录成功的请求"""
        # 流式成功时，返回给客户端的是提供商响应头 + SSE 必需头
        client_response_headers = dict(ctx.response_headers) if ctx.response_headers else {}
        client_response_headers.update({
            "Cache-Control": "no-cache, no-transform",
            "X-Accel-Buffering": "no",
            "content-type": "text/event-stream",
        })

        await telemetry.record_success(
            provider=ctx.provider_name or "unknown",
            model=ctx.model,
            input_tokens=ctx.input_tokens,
            output_tokens=ctx.output_tokens,
            response_time_ms=response_time_ms,
            first_byte_time_ms=ctx.first_byte_time_ms,  # 传递首字时间
            status_code=ctx.status_code,
            request_headers=original_headers,
            request_body=actual_request_body,
            response_headers=ctx.response_headers,
            client_response_headers=client_response_headers,
            response_body=response_body,
            cache_creation_tokens=ctx.cache_creation_tokens,
            cache_read_tokens=ctx.cached_tokens,
            is_stream=True,
            provider_request_headers=ctx.provider_request_headers,
            api_format=ctx.api_format,
            provider_id=ctx.provider_id,
            provider_endpoint_id=ctx.endpoint_id,
            provider_api_key_id=ctx.key_id,
            target_model=ctx.mapped_model,
        )

        logger.debug(f"{self.format_id} 流式响应完成")
        logger.info(ctx.get_log_summary(self.request_id, response_time_ms))

    async def _record_failure(
        self,
        telemetry: MessageTelemetry,
        ctx: StreamContext,
        original_headers: Dict[str, str],
        actual_request_body: Dict[str, Any],
        response_body: Dict[str, Any],
        response_time_ms: int,
    ) -> None:
        """记录失败的请求"""
        # 失败时返回给客户端的是 JSON 错误响应，如果没有设置则使用默认值
        client_response_headers = ctx.client_response_headers or {"content-type": "application/json"}

        await telemetry.record_failure(
            provider=ctx.provider_name or "unknown",
            model=ctx.model,
            response_time_ms=response_time_ms,
            status_code=ctx.status_code,
            error_message=ctx.error_message or f"HTTP {ctx.status_code}",
            request_headers=original_headers,
            request_body=actual_request_body,
            is_stream=True,
            api_format=ctx.api_format,
            provider_request_headers=ctx.provider_request_headers,
            input_tokens=ctx.input_tokens,
            output_tokens=ctx.output_tokens,
            cache_creation_tokens=ctx.cache_creation_tokens,
            cache_read_tokens=ctx.cached_tokens,
            response_body=response_body,
            response_headers=ctx.response_headers,
            client_response_headers=client_response_headers,
            target_model=ctx.mapped_model,
        )

        logger.debug(f"{self.format_id} 流式响应中断")
        log_summary = ctx.get_log_summary(self.request_id, response_time_ms)
        # 对于失败日志，添加缓存信息
        logger.info(f"{log_summary} cache:{ctx.cached_tokens}")

    async def _update_candidate_status(
        self,
        db: Session,
        ctx: StreamContext,
        response_time_ms: int,
    ) -> None:
        """更新候选记录状态"""
        if not ctx.attempt_id:
            return

        from src.services.request.candidate import RequestCandidateService

        if ctx.is_success():
            RequestCandidateService.mark_candidate_success(
                db=db,
                candidate_id=ctx.attempt_id,
                status_code=ctx.status_code,
                latency_ms=response_time_ms,
                extra_data={
                    "stream_completed": True,
                    "data_count": ctx.data_count,
                },
            )
        else:
            error_type = "client_disconnected" if ctx.status_code == 499 else "stream_error"
            # 请求链路追踪使用 upstream_response（原始响应），回退到 error_message（友好消息）
            trace_error_message = ctx.upstream_response or ctx.error_message or f"HTTP {ctx.status_code}"
            RequestCandidateService.mark_candidate_failed(
                db=db,
                candidate_id=ctx.attempt_id,
                error_type=error_type,
                error_message=trace_error_message,
                status_code=ctx.status_code,
                latency_ms=response_time_ms,
                extra_data={
                    "stream_completed": False,
                    "data_count": ctx.data_count,
                },
            )

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
        error_message: Optional[str] = None,
    ) -> None:
        """直接更新 Usage 表的状态字段"""
        try:
            from src.models.database import Usage

            usage = db.query(Usage).filter(Usage.request_id == self.request_id).first()
            if usage:
                setattr(usage, "status", status)
                setattr(usage, "status_code", status_code)
                setattr(usage, "response_time_ms", response_time_ms)
                if error_message:
                    setattr(usage, "error_message", error_message)
                db.commit()
                logger.debug(f"[{self.request_id}] Usage 状态已更新: {status}")
        except Exception as e:
            logger.error(f"[{self.request_id}] 直接更新 Usage 状态失败: {e}")
