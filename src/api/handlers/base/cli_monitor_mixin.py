"""CLI Handler - 监控/统计 Mixin"""

from __future__ import annotations

import asyncio
import os
import time
from collections.abc import AsyncGenerator
from typing import TYPE_CHECKING, Any

import httpx
from fastapi import Request

from src.api.handlers.base.base_handler import MessageTelemetry
from src.api.handlers.base.stream_context import StreamContext
from src.api.handlers.base.utils import filter_proxy_response_headers
from src.core.error_utils import extract_client_error_message
from src.core.exceptions import (
    ProviderAuthException,
    ProviderRateLimitException,
    ProviderTimeoutException,
    ThinkingSignatureException,
)
from src.core.logger import logger
from src.database import get_db
from src.models.database import User
from src.services.provider.behavior import get_provider_behavior

if TYPE_CHECKING:
    from src.api.handlers.base.cli_protocol import CliHandlerProtocol


def _read_stream_idle_timeout_seconds() -> float:
    raw_value = os.getenv("STREAM_IDLE_TIMEOUT_SECONDS", "30")
    try:
        parsed = float(raw_value)
    except (TypeError, ValueError):
        return 30.0
    return parsed if parsed > 0 else 30.0


class CliMonitorMixin:
    """监控和统计相关方法的 Mixin"""

    # CancelledError 归因时，断连检查参数（秒）
    CANCEL_DISCONNECT_CHECK_TIMEOUT_SECONDS = 0.5
    CANCEL_DISCONNECT_RETRY_DELAYS_SECONDS = (0.1, 0.2)
    # 流式传输过程中若长时间没有任何 chunk，提前判定为 idle timeout，避免一直等到 worker 超时
    STREAM_IDLE_TIMEOUT_SECONDS = _read_stream_idle_timeout_seconds()

    async def _probe_client_disconnect(
        self,
        http_request: Request,
        *,
        request_id: str,
    ) -> tuple[bool, bool]:
        """单次探测客户端是否断连。

        Returns:
            (is_disconnected, is_indeterminate)
        """
        try:
            disconnected = await asyncio.wait_for(
                asyncio.shield(http_request.is_disconnected()),
                timeout=self.CANCEL_DISCONNECT_CHECK_TIMEOUT_SECONDS,
            )
            return bool(disconnected), False
        except (asyncio.CancelledError, asyncio.TimeoutError):
            return False, True
        except Exception as e:
            logger.debug("ID:{} | cancel 断连检测失败: {}", request_id, e)
            return False, True

    async def _confirm_client_disconnect(
        self,
        http_request: Request,
        *,
        request_id: str,
    ) -> tuple[bool, bool]:
        """CancelledError 场景下做多次断连确认，降低误判。

        Returns:
            (is_client_disconnected, check_indeterminate)
        """
        disconnected, uncertain = await self._probe_client_disconnect(
            http_request,
            request_id=request_id,
        )
        if disconnected:
            return True, uncertain

        for delay in self.CANCEL_DISCONNECT_RETRY_DELAYS_SECONDS:
            try:
                await asyncio.sleep(delay)
            except asyncio.CancelledError:
                # 重试期间协程再次被取消，无法继续探测，标记为不确定
                return False, True
            disconnected, step_uncertain = await self._probe_client_disconnect(
                http_request,
                request_id=request_id,
            )
            uncertain = uncertain or step_uncertain
            if disconnected:
                return True, uncertain

        return False, uncertain

    async def _create_monitored_stream(
        self,
        ctx: StreamContext,
        stream_generator: AsyncGenerator[bytes],
        http_request: Request | None = None,
    ) -> AsyncGenerator[bytes]:
        """
        创建带监控的流生成器

        支持两种断连检测方式：
        1. 如果提供了 http_request，使用后台任务主动检测客户端断连
        2. 如果未提供，仅依赖 asyncio.CancelledError 被动检测

        Args:
            ctx: 流上下文
            stream_generator: 底层流生成器
            http_request: FastAPI Request 对象，用于检测客户端断连
        """
        import time as time_module

        last_chunk_time = time_module.time()
        chunk_count = 0
        stream_started = False
        idle_timeout_triggered = False
        idle_timeout = self.STREAM_IDLE_TIMEOUT_SECONDS
        parent_task = asyncio.current_task()
        idle_watch_task: asyncio.Task[None] | None = None

        async def watch_stream_idle_timeout() -> None:
            nonlocal idle_timeout_triggered
            if parent_task is None:
                return
            poll_interval = min(1.0, max(0.1, idle_timeout / 5))
            while not ctx.has_completion:
                await asyncio.sleep(poll_interval)
                if not stream_started:
                    continue
                if (time_module.time() - last_chunk_time) <= idle_timeout:
                    continue
                idle_timeout_triggered = True
                parent_task.cancel()
                return

        try:
            idle_watch_task = asyncio.create_task(watch_stream_idle_timeout())
            if http_request is not None:
                # 使用后台任务检测断连，完全不阻塞流式传输
                disconnected = False

                async def check_disconnect_background() -> None:
                    nonlocal disconnected
                    while not disconnected and not ctx.has_completion:
                        await asyncio.sleep(0.5)
                        try:
                            if await http_request.is_disconnected():
                                disconnected = True
                                break
                        except Exception as e:
                            # 检测失败时不中断流，继续传输
                            logger.debug("ID:{} | 断连检测异常: {}", ctx.request_id, e)

                # 启动后台检查任务
                check_task = asyncio.create_task(check_disconnect_background())

                try:
                    async for chunk in stream_generator:
                        if disconnected:
                            # 如果响应已完成，客户端断开不算失败
                            if ctx.has_completion:
                                logger.info(
                                    f"ID:{ctx.request_id} | Client disconnected after completion"
                                )
                            else:
                                logger.warning("ID:{} | Client disconnected", ctx.request_id)
                                ctx.status_code = 499
                                ctx.error_message = "client_disconnected"
                            break
                        stream_started = True
                        last_chunk_time = time_module.time()
                        chunk_count += 1
                        yield chunk
                finally:
                    check_task.cancel()
                    try:
                        await check_task
                    except asyncio.CancelledError:
                        pass
                    if idle_watch_task is not None:
                        idle_watch_task.cancel()
                        try:
                            await idle_watch_task
                        except asyncio.CancelledError:
                            pass
            else:
                # 无 http_request，仅被动监控
                try:
                    async for chunk in stream_generator:
                        stream_started = True
                        last_chunk_time = time_module.time()
                        chunk_count += 1
                        yield chunk
                finally:
                    if idle_watch_task is not None:
                        idle_watch_task.cancel()
                        try:
                            await idle_watch_task
                        except asyncio.CancelledError:
                            pass

        except asyncio.CancelledError:
            # 防御性清理：正常路径中 idle_watch_task 已由内部 finally 取消，
            # 但若 CancelledError 在异常路径传播，确保不留孤儿 task。
            if idle_watch_task is not None and not idle_watch_task.done():
                idle_watch_task.cancel()
            # 注意：CancelledError 不等于"用户手动取消"，它既可能是客户端断连触发，
            # 也可能是服务端（重载/关停/内部取消）导致的协程取消。
            # 这里尽量做一次"断连归因"：仅当能确认客户端已断开时才记为 499 cancelled。
            time_since_last_chunk = time_module.time() - last_chunk_time
            if not ctx.has_completion:
                ctx.ensure_estimated_output_tokens()

            if not ctx.has_completion and idle_timeout_triggered:
                ctx.status_code = 504
                ctx.error_message = "stream_idle_timeout"
                cancel_origin = "stream_idle_timeout"
                logger.warning(
                    f"ID:{ctx.request_id} | Stream idle timeout: "
                    f"idle_timeout={idle_timeout:g}s, "
                    f"chunks={chunk_count}, "
                    f"has_completion={ctx.has_completion}, "
                    f"time_since_last_chunk={time_since_last_chunk:.2f}s, "
                    f"output_tokens={ctx.output_tokens}"
                )
                ctx.upstream_response = (
                    f"cancel_origin={cancel_origin}, "
                    f"chunks={chunk_count}, "
                    f"has_completion={ctx.has_completion}, "
                    f"time_since_last_chunk={time_since_last_chunk:.2f}s, "
                    f"output_tokens={ctx.output_tokens}, "
                    f"idle_timeout={idle_timeout:g}s"
                )
                raise

            is_client_disconnected = False
            disconnect_check_uncertain = False
            if http_request is not None:
                is_client_disconnected, disconnect_check_uncertain = (
                    await self._confirm_client_disconnect(
                        http_request,
                        request_id=ctx.request_id,
                    )
                )

            # 如果响应已完成，不标记为失败/取消
            if not ctx.has_completion:
                if is_client_disconnected:
                    ctx.status_code = 499
                    ctx.error_message = "client_disconnected"
                    cancel_origin = "client_disconnected"
                    logger.warning(
                        f"ID:{ctx.request_id} | Stream cancelled by client: "
                        f"chunks={chunk_count}, "
                        f"has_completion={ctx.has_completion}, "
                        f"time_since_last_chunk={time_since_last_chunk:.2f}s, "
                        f"output_tokens={ctx.output_tokens}"
                    )
                elif disconnect_check_uncertain:
                    # 断连检查本身不稳定（超时/取消/异常）时，避免直接定性为 server_cancelled。
                    ctx.status_code = 503
                    ctx.error_message = "cancelled_unknown"
                    cancel_origin = "cancelled_unknown"
                    logger.warning(
                        f"ID:{ctx.request_id} | Stream cancelled with unknown origin: "
                        f"chunks={chunk_count}, "
                        f"has_completion={ctx.has_completion}, "
                        f"time_since_last_chunk={time_since_last_chunk:.2f}s, "
                        f"output_tokens={ctx.output_tokens}"
                    )
                else:
                    # 服务端中断（例如重载/关停/内部取消） -- 不应伪装成客户端取消
                    ctx.status_code = 503
                    ctx.error_message = "server_cancelled"
                    cancel_origin = "server_cancelled"
                    logger.error(
                        f"ID:{ctx.request_id} | Stream interrupted by server: "
                        f"chunks={chunk_count}, "
                        f"has_completion={ctx.has_completion}, "
                        f"time_since_last_chunk={time_since_last_chunk:.2f}s, "
                        f"output_tokens={ctx.output_tokens}"
                    )
                ctx.upstream_response = (
                    f"cancel_origin={cancel_origin}, "
                    f"chunks={chunk_count}, "
                    f"has_completion={ctx.has_completion}, "
                    f"time_since_last_chunk={time_since_last_chunk:.2f}s, "
                    f"output_tokens={ctx.output_tokens}"
                )
            raise
        except httpx.TimeoutException as e:
            if idle_watch_task is not None and not idle_watch_task.done():
                idle_watch_task.cancel()
            ctx.status_code = 504
            ctx.error_message = str(e)
            raise
        except Exception as e:
            if idle_watch_task is not None and not idle_watch_task.done():
                idle_watch_task.cancel()
            ctx.status_code = 500
            ctx.error_message = str(e)
            raise

    async def _record_stream_stats(
        self: CliHandlerProtocol,
        ctx: StreamContext,
        original_headers: dict[str, str],
        original_request_body: dict[str, Any],
    ) -> None:
        """在流完成后记录统计信息"""
        try:
            # 使用 self.start_time 作为时间基准，与首字时间保持一致
            # 注意：不要把统计延迟算进响应时间里
            response_time_ms = int((time.time() - self.start_time) * 1000)

            await asyncio.sleep(0.1)

            if not ctx.provider_name:
                logger.warning("[{}] 流式请求失败，未选中提供商", ctx.request_id)
                return

            behavior = get_provider_behavior(
                provider_type=ctx.provider_type,
                endpoint_sig=ctx.provider_api_format,
            )
            envelope = behavior.envelope
            if envelope:
                envelope.on_http_status(
                    base_url=ctx.selected_base_url,
                    status_code=ctx.status_code,
                )

            # 获取新的 DB session
            db_gen = get_db()
            bg_db = next(db_gen)

            try:
                from src.models.database import ApiKey as ApiKeyModel

                user = bg_db.query(User).filter(User.id == ctx.user_id).first()
                api_key = bg_db.query(ApiKeyModel).filter(ApiKeyModel.id == ctx.api_key_id).first()

                if not user or not api_key:
                    logger.warning(
                        "[{}] 无法记录统计: user={} api_key={}",
                        ctx.request_id,
                        user is not None,
                        api_key is not None,
                    )
                    return

                bg_telemetry = MessageTelemetry(
                    bg_db, user, api_key, ctx.request_id, self.client_ip
                )

                if ctx.should_estimate_incomplete_tokens():
                    self._estimate_tokens_for_incomplete_stream(
                        ctx, ctx.provider_request_body or original_request_body
                    )

                with ctx.managed_recorded_bodies(response_time_ms) as recorded_bodies:
                    # 根据状态码决定记录成功还是失败
                    # 499 = 客户端取消（不算系统失败）；其他 4xx/5xx 视为失败
                    if ctx.status_code and ctx.status_code >= 400:
                        client_response_headers = ctx.client_response_headers or {
                            "content-type": "application/json"
                        }

                        if ctx.is_client_disconnected():
                            # 客户端取消：记录为 cancelled（不算系统失败）
                            request_metadata = self._merge_scheduling_metadata(
                                {"perf": ctx.perf_metrics} if ctx.perf_metrics else None,
                                selected_key_id=ctx.key_id,
                                candidate_keys=ctx.candidate_keys,
                                pool_summary=ctx.pool_summary,
                                fallback_from_request=True,
                            )
                            await bg_telemetry.record_cancelled(
                                provider=ctx.provider_name or "unknown",
                                model=ctx.model,
                                response_time_ms=response_time_ms,
                                first_byte_time_ms=ctx.first_byte_time_ms,
                                status_code=ctx.status_code,
                                request_headers=original_headers,
                                request_body=original_request_body,
                                is_stream=True,
                                api_format=ctx.api_format,
                                api_family=self.api_family,
                                endpoint_kind=self.endpoint_kind,
                                provider_request_headers=ctx.provider_request_headers,
                                provider_request_body=ctx.provider_request_body,
                                input_tokens=ctx.input_tokens,
                                output_tokens=ctx.output_tokens,
                                cache_creation_tokens=ctx.cache_creation_tokens,
                                cache_read_tokens=ctx.cached_tokens,
                                response_body=recorded_bodies.response_body,
                                client_response_body=recorded_bodies.client_response_body,
                                response_headers=ctx.response_headers,
                                client_response_headers=client_response_headers,
                                provider_id=ctx.provider_id,
                                provider_endpoint_id=ctx.endpoint_id,
                                provider_api_key_id=ctx.key_id,
                                model_group_id=ctx.model_group_id,
                                model_group_route_id=ctx.model_group_route_id,
                                user_billing_multiplier=ctx.user_billing_multiplier,
                                endpoint_api_format=ctx.provider_api_format or None,
                                has_format_conversion=ctx.has_format_conversion,
                                target_model=ctx.mapped_model,
                                request_metadata=request_metadata,
                            )
                            logger.debug("{} 流式响应被客户端取消", self.FORMAT_ID)
                            logger.info(
                                "[CANCEL] {} | {} | {} | {}ms | {} | in:{} out:{} cache:{}",
                                self.request_id[:8],
                                ctx.model,
                                ctx.provider_name,
                                response_time_ms,
                                ctx.status_code,
                                ctx.input_tokens,
                                ctx.output_tokens,
                                ctx.cached_tokens,
                            )
                        else:
                            # 服务端/上游异常：记录为失败
                            request_metadata = self._merge_scheduling_metadata(
                                {"perf": ctx.perf_metrics} if ctx.perf_metrics else None,
                                selected_key_id=ctx.key_id,
                                candidate_keys=ctx.candidate_keys,
                                pool_summary=ctx.pool_summary,
                                fallback_from_request=True,
                            )
                            await bg_telemetry.record_failure(
                                provider=ctx.provider_name or "unknown",
                                model=ctx.model,
                                response_time_ms=response_time_ms,
                                status_code=ctx.status_code,
                                error_message=ctx.error_message or f"HTTP {ctx.status_code}",
                                request_headers=original_headers,
                                request_body=original_request_body,
                                is_stream=True,
                                api_format=ctx.api_format,
                                api_family=self.api_family,
                                endpoint_kind=self.endpoint_kind,
                                provider_request_headers=ctx.provider_request_headers,
                                provider_request_body=ctx.provider_request_body,
                                # 预估 token 信息（来自 message_start 事件）
                                input_tokens=ctx.input_tokens,
                                output_tokens=ctx.output_tokens,
                                cache_creation_tokens=ctx.cache_creation_tokens,
                                cache_read_tokens=ctx.cached_tokens,
                                response_body=recorded_bodies.response_body,
                                client_response_body=recorded_bodies.client_response_body,
                                response_headers=ctx.response_headers,
                                client_response_headers=client_response_headers,
                                provider_id=ctx.provider_id,
                                provider_endpoint_id=ctx.endpoint_id,
                                provider_api_key_id=ctx.key_id,
                                model_group_id=ctx.model_group_id,
                                model_group_route_id=ctx.model_group_route_id,
                                user_billing_multiplier=ctx.user_billing_multiplier,
                                # 格式转换追踪
                                endpoint_api_format=ctx.provider_api_format or None,
                                has_format_conversion=ctx.has_format_conversion,
                                # 模型映射信息
                                target_model=ctx.mapped_model,
                                request_metadata=request_metadata,
                            )
                            logger.debug("{} 流式响应中断", self.FORMAT_ID)
                            logger.info(
                                "[FAIL] {} | {} | {} | {}ms | {} | in:{} out:{} cache:{}",
                                self.request_id[:8],
                                ctx.model,
                                ctx.provider_name,
                                response_time_ms,
                                ctx.status_code,
                                ctx.input_tokens,
                                ctx.output_tokens,
                                ctx.cached_tokens,
                            )
                    else:
                        # 在记录统计前，允许子类从 parsed_chunks 中提取额外的元数据
                        self._finalize_stream_metadata(ctx)

                        # 流式格式转换汇总日志
                        if ctx.stream_conversion_event_count > 0:
                            logger.debug(
                                "[{}] 流式转换完成: {}->{}, total_events={}",
                                self.request_id[:8],
                                ctx.provider_api_format,
                                ctx.client_api_format,
                                ctx.stream_conversion_event_count,
                            )

                        # 流未正常完成（如上游截断/连接中断）且无 token 数据时，
                        # 从已收集的文本和请求体估算 tokens，避免 usage 记录为 0
                        # 流式成功时，返回给客户端的是提供商响应头 + SSE 必需头
                        client_response_headers = filter_proxy_response_headers(
                            ctx.response_headers
                        )
                        client_response_headers.update(
                            {
                                "Cache-Control": "no-cache, no-transform",
                                "X-Accel-Buffering": "no",
                                "content-type": "text/event-stream",
                            }
                        )

                        logger.debug(
                            "[{}] 开始记录 Usage: provider={}, model={}, in={}, out={}",
                            ctx.request_id,
                            ctx.provider_name,
                            ctx.model,
                            ctx.input_tokens,
                            ctx.output_tokens,
                        )
                        request_metadata = self._merge_scheduling_metadata(
                            {"perf": ctx.perf_metrics} if ctx.perf_metrics else None,
                            selected_key_id=ctx.key_id,
                            candidate_keys=ctx.candidate_keys,
                            pool_summary=ctx.pool_summary,
                            fallback_from_request=True,
                        )
                        total_cost = await bg_telemetry.record_success(
                            provider=ctx.provider_name,
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
                            response_body=recorded_bodies.response_body,
                            client_response_body=recorded_bodies.client_response_body,
                            provider_request_body=ctx.provider_request_body,
                            cache_creation_tokens=ctx.cache_creation_tokens,
                            cache_read_tokens=ctx.cached_tokens,
                            is_stream=True,
                            provider_request_headers=ctx.provider_request_headers,
                            api_format=ctx.api_format,
                            api_family=self.api_family,
                            endpoint_kind=self.endpoint_kind,
                            # 格式转换追踪
                            endpoint_api_format=ctx.provider_api_format or None,
                            has_format_conversion=ctx.has_format_conversion,
                            # Provider 侧追踪信息（用于记录真实成本）
                            provider_id=ctx.provider_id,
                            provider_endpoint_id=ctx.endpoint_id,
                            provider_api_key_id=ctx.key_id,
                            model_group_id=ctx.model_group_id,
                            model_group_route_id=ctx.model_group_route_id,
                            user_billing_multiplier=ctx.user_billing_multiplier,
                            # 模型映射信息
                            target_model=ctx.mapped_model,
                            # Provider 响应元数据（如 Gemini 的 modelVersion）
                            response_metadata=(
                                ctx.response_metadata if ctx.response_metadata else None
                            ),
                            request_metadata=request_metadata,
                        )
                        logger.debug(
                            "[{}] Usage 记录完成: cost=${:.6f}", ctx.request_id, total_cost
                        )
                        # 简洁的请求完成摘要（两行格式）
                        ttfb_part = (
                            f" | TTFB: {ctx.first_byte_time_ms}ms" if ctx.first_byte_time_ms else ""
                        )
                        logger.info(
                            "[OK] {} | {} | {}{}\n      Total: {}ms | in:{} out:{}",
                            self.request_id[:8],
                            ctx.model,
                            ctx.provider_name,
                            ttfb_part,
                            response_time_ms,
                            ctx.input_tokens or 0,
                            ctx.output_tokens or 0,
                        )

                # 更新候选记录的最终状态和延迟时间
                # 注意：RequestExecutor 会在流开始时过早地标记成功（只记录了连接建立的时间）
                # 这里用流传输完成后的实际时间覆盖
                if ctx.attempt_id:
                    from src.services.request.candidate import RequestCandidateService

                    # 计算候选自身的 TTFB
                    candidate_first_byte_time_ms: int | None = None
                    if ctx.first_byte_time_ms is not None:
                        candidate_first_byte_time_ms = (
                            RequestCandidateService.calculate_candidate_ttfb(
                                db=bg_db,
                                candidate_id=ctx.attempt_id,
                                request_start_time=self.start_time,
                                global_first_byte_time_ms=ctx.first_byte_time_ms,
                            )
                        )

                    # 根据状态码决定是成功还是失败
                    # 499 = 客户端断开连接，应标记为失败
                    # 503 = 服务不可用（如流中断），应标记为失败
                    if ctx.status_code and ctx.status_code >= 400:
                        # 请求链路追踪使用 upstream_response（原始响应），回退到 error_message（友好消息）
                        trace_error_message = (
                            ctx.upstream_response or ctx.error_message or f"HTTP {ctx.status_code}"
                        )
                        extra_data = {
                            "stream_completed": False,
                            "chunk_count": ctx.chunk_count,
                            "data_count": ctx.data_count,
                        }
                        if ctx.proxy_info:
                            extra_data["proxy"] = ctx.proxy_info
                        if candidate_first_byte_time_ms is not None:
                            extra_data["first_byte_time_ms"] = candidate_first_byte_time_ms
                        if ctx.is_client_disconnected():
                            RequestCandidateService.mark_candidate_cancelled(
                                db=bg_db,
                                candidate_id=ctx.attempt_id,
                                status_code=ctx.status_code,
                                latency_ms=response_time_ms,
                                extra_data=extra_data,
                            )
                        else:
                            RequestCandidateService.mark_candidate_failed(
                                db=bg_db,
                                candidate_id=ctx.attempt_id,
                                error_type="stream_error",
                                error_message=trace_error_message,
                                status_code=ctx.status_code,
                                latency_ms=response_time_ms,
                                extra_data=extra_data,
                            )
                    else:
                        extra_data = {
                            "stream_completed": True,
                            "chunk_count": ctx.chunk_count,
                            "data_count": ctx.data_count,
                        }
                        if ctx.proxy_info:
                            extra_data["proxy"] = ctx.proxy_info
                        if ctx.rectified:
                            extra_data["rectified"] = True
                        if candidate_first_byte_time_ms is not None:
                            extra_data["first_byte_time_ms"] = candidate_first_byte_time_ms
                        RequestCandidateService.mark_candidate_success(
                            db=bg_db,
                            candidate_id=ctx.attempt_id,
                            status_code=ctx.status_code,
                            latency_ms=response_time_ms,
                            extra_data=extra_data,
                        )

            finally:
                bg_db.close()

        except Exception:
            logger.exception("记录流式统计信息时出错")
        finally:
            # 遥测写入完成后主动释放大对象列表，降低高并发长流的内存滞留。
            ctx.release_recorded_chunks()

    async def _record_stream_failure(
        self,
        ctx: StreamContext,
        error: Exception,
        original_headers: dict[str, str],
        original_request_body: dict[str, Any],
    ) -> None:
        """记录流式请求失败"""
        # 使用 self.start_time 作为时间基准，与首字时间保持一致
        response_time_ms = int((time.time() - self.start_time) * 1000)

        status_code = 503
        if isinstance(error, ThinkingSignatureException):
            status_code = 400
        elif isinstance(error, ProviderAuthException):
            status_code = 503
        elif isinstance(error, ProviderRateLimitException):
            status_code = 429
        elif isinstance(error, ProviderTimeoutException):
            status_code = 504

        ctx.status_code = status_code
        ctx.error_message = str(error)

        # 失败时返回给客户端的是 JSON 错误响应
        client_response_headers = {"content-type": "application/json"}

        request_metadata = self._merge_scheduling_metadata(
            {"perf": ctx.perf_metrics} if ctx.perf_metrics else None,
            selected_key_id=ctx.key_id,
            candidate_keys=ctx.candidate_keys,
            pool_summary=ctx.pool_summary,
            fallback_from_request=True,
        )
        try:
            await self.telemetry.record_failure(
                provider=ctx.provider_name or "unknown",
                model=ctx.model,
                response_time_ms=response_time_ms,
                status_code=status_code,
                error_message=extract_client_error_message(error),
                request_headers=original_headers,
                request_body=original_request_body,
                is_stream=True,
                api_format=ctx.api_format,
                api_family=self.api_family,
                endpoint_kind=self.endpoint_kind,
                provider_request_headers=ctx.provider_request_headers,
                provider_request_body=ctx.provider_request_body,
                response_headers=ctx.response_headers,
                client_response_headers=client_response_headers,
                provider_id=ctx.provider_id,
                provider_endpoint_id=ctx.endpoint_id,
                provider_api_key_id=ctx.key_id,
                model_group_id=ctx.model_group_id,
                model_group_route_id=ctx.model_group_route_id,
                user_billing_multiplier=ctx.user_billing_multiplier,
                # 格式转换追踪
                endpoint_api_format=ctx.provider_api_format or None,
                has_format_conversion=ctx.has_format_conversion,
                # 模型映射信息
                target_model=ctx.mapped_model,
                request_metadata=request_metadata,
            )
        finally:
            # 失败路径同样可能持有 chunk 审计数据，及时释放。
            ctx.release_recorded_chunks()
