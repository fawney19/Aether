"""
流式处理器 - 从 ChatHandlerBase 提取的流式响应处理逻辑

职责：
1. SSE 事件解析和处理
2. 响应流生成
3. 预读和嵌套错误检测
4. 客户端断开检测
"""

import asyncio
import codecs
import json
import time
from typing import Any, AsyncGenerator, Callable, Optional

import httpx

from src.api.handlers.base.parsers import get_parser_for_format
from src.api.handlers.base.response_parser import ResponseParser
from src.api.handlers.base.stream_context import StreamContext
from src.core.exceptions import EmbeddedErrorException
from src.core.logger import logger
from src.models.database import Provider, ProviderEndpoint
from src.utils.sse_parser import SSEEventParser


class StreamProcessor:
    """
    流式响应处理器

    负责处理 SSE 流的解析、错误检测和响应生成。
    从 ChatHandlerBase 中提取，使其职责更加单一。
    """

    def __init__(
        self,
        request_id: str,
        default_parser: ResponseParser,
        on_streaming_start: Optional[Callable[[], None]] = None,
        *,
        collect_text: bool = False,
    ):
        """
        初始化流处理器

        Args:
            request_id: 请求 ID（用于日志）
            default_parser: 默认响应解析器
            on_streaming_start: 流开始时的回调（用于更新状态）
        """
        self.request_id = request_id
        self.default_parser = default_parser
        self.on_streaming_start = on_streaming_start
        self.collect_text = collect_text

    def get_parser_for_provider(self, ctx: StreamContext) -> ResponseParser:
        """
        获取 Provider 格式的解析器

        根据 Provider 的 API 格式选择正确的解析器。
        """
        if ctx.provider_api_format:
            try:
                return get_parser_for_format(ctx.provider_api_format)
            except KeyError:
                pass
        return self.default_parser

    def handle_sse_event(
        self,
        ctx: StreamContext,
        event_name: Optional[str],
        data_str: str,
    ) -> None:
        """
        处理单个 SSE 事件

        解析事件数据，提取 usage 信息和文本内容。

        Args:
            ctx: 流式上下文
            event_name: 事件名称
            data_str: 事件数据字符串
        """
        if not data_str:
            return

        if data_str == "[DONE]":
            ctx.has_completion = True
            return

        try:
            data = json.loads(data_str)
        except json.JSONDecodeError:
            return

        ctx.data_count += 1

        if not isinstance(data, dict):
            return

        # 收集原始 chunk 数据
        ctx.parsed_chunks.append(data)

        # 根据 Provider 格式选择解析器
        parser = self.get_parser_for_provider(ctx)

        # 使用解析器提取 usage
        usage = parser.extract_usage_from_response(data)
        if usage:
            ctx.update_usage(
                input_tokens=usage.get("input_tokens"),
                output_tokens=usage.get("output_tokens"),
                cached_tokens=usage.get("cache_read_tokens"),
                cache_creation_tokens=usage.get("cache_creation_tokens"),
            )

        # 提取文本
        if self.collect_text:
            text = parser.extract_text_content(data)
            if text:
                ctx.append_text(text)

        # 检查完成
        event_type = event_name or data.get("type", "")
        if event_type in ("response.completed", "message_stop"):
            ctx.has_completion = True

        # 检查 OpenAI 格式的 finish_reason
        choices = data.get("choices", [])
        if choices and isinstance(choices, list) and len(choices) > 0:
            finish_reason = choices[0].get("finish_reason")
            if finish_reason is not None:
                ctx.has_completion = True

    async def prefetch_and_check_error(
        self,
        byte_iterator: Any,
        provider: Provider,
        endpoint: ProviderEndpoint,
        ctx: StreamContext,
        max_prefetch_lines: int = 5,
    ) -> list:
        """
        预读流的前几行，检测嵌套错误

        某些 Provider（如 Gemini）可能返回 HTTP 200，但在响应体中包含错误信息。
        这种情况需要在流开始输出之前检测，以便触发重试逻辑。

        Args:
            byte_iterator: 字节流迭代器
            provider: Provider 对象
            endpoint: Endpoint 对象
            ctx: 流式上下文
            max_prefetch_lines: 最多预读行数

        Returns:
            预读的字节块列表

        Raises:
            EmbeddedErrorException: 如果检测到嵌套错误
        """
        prefetched_chunks: list = []
        parser = self.get_parser_for_provider(ctx)
        buffer = b""
        line_count = 0
        should_stop = False
        # 使用增量解码器处理跨 chunk 的 UTF-8 字符
        decoder = codecs.getincrementaldecoder("utf-8")(errors="replace")

        try:
            async for chunk in byte_iterator:
                prefetched_chunks.append(chunk)
                buffer += chunk

                # 尝试按行解析缓冲区
                while b"\n" in buffer:
                    line_bytes, buffer = buffer.split(b"\n", 1)
                    try:
                        # 使用增量解码器，可以正确处理跨 chunk 的多字节字符
                        line = decoder.decode(line_bytes + b"\n", False).rstrip("\r\n")
                    except Exception as e:
                        logger.warning(
                            f"[{self.request_id}] 预读时 UTF-8 解码失败: {e}, "
                            f"bytes={line_bytes[:50]!r}"
                        )
                        continue

                    line_count += 1

                    # 跳过空行和注释行
                    if not line or line.startswith(":"):
                        if line_count >= max_prefetch_lines:
                            should_stop = True
                            break
                        continue

                    # 尝试解析 SSE 数据
                    data_str = line
                    if line.startswith("data: "):
                        data_str = line[6:]

                    if data_str == "[DONE]":
                        should_stop = True
                        break

                    try:
                        data = json.loads(data_str)
                    except json.JSONDecodeError:
                        if line_count >= max_prefetch_lines:
                            should_stop = True
                            break
                        continue

                    # 使用解析器检查是否为错误响应
                    if isinstance(data, dict) and parser.is_error_response(data):
                        parsed = parser.parse_response(data, 200)
                        logger.warning(
                            f"  [{self.request_id}] 检测到嵌套错误: "
                            f"Provider={provider.name}, "
                            f"error_type={parsed.error_type}, "
                            f"message={parsed.error_message}"
                        )
                        raise EmbeddedErrorException(
                            provider_name=str(provider.name),
                            error_code=(
                                int(parsed.error_type)
                                if parsed.error_type and parsed.error_type.isdigit()
                                else None
                            ),
                            error_message=parsed.error_message,
                            error_status=parsed.error_type,
                        )

                    # 预读到有效数据，没有错误，停止预读
                    should_stop = True
                    break

                if should_stop or line_count >= max_prefetch_lines:
                    break

        except EmbeddedErrorException:
            raise
        except Exception as e:
            logger.debug(f"  [{self.request_id}] 预读流时发生异常: {e}")

        return prefetched_chunks

    async def create_response_stream(
        self,
        ctx: StreamContext,
        byte_iterator: Any,
        response_ctx: Any,
        http_client: httpx.AsyncClient,
        prefetched_chunks: Optional[list] = None,
        *,
        start_time: Optional[float] = None,
    ) -> AsyncGenerator[bytes, None]:
        """
        创建响应流生成器

        从字节流中解析 SSE 数据并转发，支持预读数据。

        Args:
            ctx: 流式上下文
            byte_iterator: 字节流迭代器
            response_ctx: HTTP 响应上下文管理器
            http_client: HTTP 客户端
            prefetched_chunks: 预读的字节块列表（可选）
            start_time: 请求开始时间,用于计算 TTFB（可选）

        Yields:
            编码后的响应数据块
        """
        try:
            sse_parser = SSEEventParser()
            streaming_started = False
            buffer = b""
            # 使用增量解码器处理跨 chunk 的 UTF-8 字符
            decoder = codecs.getincrementaldecoder("utf-8")(errors="replace")

            # 处理预读数据
            if prefetched_chunks:
                if not streaming_started and self.on_streaming_start:
                    self.on_streaming_start()
                    streaming_started = True

                for chunk in prefetched_chunks:
                    # 记录首字时间 (TTFB) - 在 yield 之前记录
                    if start_time is not None:
                        ctx.record_first_byte_time(start_time)
                        start_time = None  # 只记录一次

                    # 把原始数据转发给客户端
                    yield chunk

                    buffer += chunk
                    # 处理缓冲区中的完整行
                    while b"\n" in buffer:
                        line_bytes, buffer = buffer.split(b"\n", 1)
                        try:
                            # 使用增量解码器，可以正确处理跨 chunk 的多字节字符
                            line = decoder.decode(line_bytes + b"\n", False)
                            self._process_line(ctx, sse_parser, line)
                        except Exception as e:
                            # 解码失败，记录警告但继续处理
                            logger.warning(
                                f"[{self.request_id}] UTF-8 解码失败: {e}, "
                                f"bytes={line_bytes[:50]!r}"
                            )
                            continue

            # 处理剩余的流数据
            async for chunk in byte_iterator:
                if not streaming_started and self.on_streaming_start:
                    self.on_streaming_start()
                    streaming_started = True

                # 记录首字时间 (TTFB) - 在 yield 之前记录（如果预读数据为空）
                if start_time is not None:
                    ctx.record_first_byte_time(start_time)
                    start_time = None  # 只记录一次

                # 原始数据透传
                yield chunk

                buffer += chunk
                # 处理缓冲区中的完整行
                while b"\n" in buffer:
                    line_bytes, buffer = buffer.split(b"\n", 1)
                    try:
                        # 使用增量解码器，可以正确处理跨 chunk 的多字节字符
                        line = decoder.decode(line_bytes + b"\n", False)
                        self._process_line(ctx, sse_parser, line)
                    except Exception as e:
                        # 解码失败，记录警告但继续处理
                        logger.warning(
                            f"[{self.request_id}] UTF-8 解码失败: {e}, "
                            f"bytes={line_bytes[:50]!r}"
                        )
                        continue

            # 处理剩余的缓冲区数据（如果有未完成的行）
            if buffer:
                try:
                    # 使用 final=True 处理最后的不完整字符
                    line = decoder.decode(buffer, True)
                    self._process_line(ctx, sse_parser, line)
                except Exception as e:
                    logger.warning(
                        f"[{self.request_id}] 处理剩余缓冲区失败: {e}, "
                        f"bytes={buffer[:50]!r}"
                    )

            # 处理剩余事件
            for event in sse_parser.flush():
                self.handle_sse_event(ctx, event.get("event"), event.get("data") or "")

        except GeneratorExit:
            raise
        finally:
            await self._cleanup(response_ctx, http_client)

    def _process_line(
        self,
        ctx: StreamContext,
        sse_parser: SSEEventParser,
        line: str,
    ) -> None:
        """
        处理单行数据

        Args:
            ctx: 流式上下文
            sse_parser: SSE 解析器
            line: 原始行数据
        """
        # SSEEventParser 以“去掉换行符”的单行文本作为输入；这里统一剔除 CR/LF，
        # 避免把空行误判成 "\n" 并导致事件边界解析错误。
        normalized_line = line.rstrip("\r\n")
        events = sse_parser.feed_line(normalized_line)

        if normalized_line != "":
            ctx.chunk_count += 1

        for event in events:
            self.handle_sse_event(ctx, event.get("event"), event.get("data") or "")

    async def create_monitored_stream(
        self,
        ctx: StreamContext,
        stream_generator: AsyncGenerator[bytes, None],
        is_disconnected: Callable[[], Any],
    ) -> AsyncGenerator[bytes, None]:
        """
        创建带监控的流生成器

        检测客户端断开连接并更新状态码。

        Args:
            ctx: 流式上下文
            stream_generator: 原始流生成器
            is_disconnected: 检查客户端是否断开的函数

        Yields:
            响应数据块
        """
        try:
            # 断连检查频率：每次 await 都会引入调度开销，过于频繁会让流式"发一段停一段"
            # 这里按时间间隔节流，兼顾及时停止上游读取与吞吐平滑性。
            next_disconnect_check_at = 0.0
            disconnect_check_interval_s = 0.25

            async for chunk in stream_generator:
                now = time.monotonic()
                if now >= next_disconnect_check_at:
                    next_disconnect_check_at = now + disconnect_check_interval_s
                    if await is_disconnected():
                        # 如果响应已完成（收到 finish_reason），客户端断开不算失败
                        if ctx.has_completion:
                            logger.info(
                                f"ID:{self.request_id} | Client disconnected after completion"
                            )
                        else:
                            logger.warning(f"ID:{self.request_id} | Client disconnected")
                            ctx.status_code = 499  # Client Closed Request
                            ctx.error_message = "client_disconnected"
                        break
                yield chunk
        except asyncio.CancelledError:
            # 如果响应已完成，不标记为失败
            if not ctx.has_completion:
                ctx.status_code = 499
                ctx.error_message = "client_disconnected"
            raise
        except Exception as e:
            ctx.status_code = 500
            ctx.error_message = str(e)
            raise

    async def _cleanup(
        self,
        response_ctx: Any,
        http_client: httpx.AsyncClient,
    ) -> None:
        """清理资源"""
        try:
            await response_ctx.__aexit__(None, None, None)
        except Exception:
            pass
        try:
            await http_client.aclose()
        except Exception:
            pass
