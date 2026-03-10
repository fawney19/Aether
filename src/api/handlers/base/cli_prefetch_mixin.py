"""CLI Handler - Prefetch 和错误检测 Mixin"""

from __future__ import annotations

import codecs
import json
import time
from collections.abc import AsyncGenerator
from typing import TYPE_CHECKING, Any

import httpx

from src.api.handlers.base.parsers import get_parser_for_format
from src.api.handlers.base.stream_context import StreamContext
from src.api.handlers.base.utils import (
    check_html_response,
    check_prefetched_response_error,
)
from src.config.constants import StreamDefaults
from src.config.settings import config
from src.core.exceptions import (
    EmbeddedErrorException,
    ProviderNotAvailableException,
    ProviderTimeoutException,
)
from src.core.logger import logger
from src.services.provider.behavior import get_provider_behavior
from src.utils.sse_parser import SSEEventParser
from src.utils.timeout import read_first_chunk_with_ttfb_timeout

if TYPE_CHECKING:
    from src.api.handlers.base.cli_protocol import CliHandlerProtocol
    from src.models.database import Provider, ProviderEndpoint


class CliPrefetchMixin:
    """Prefetch 和错误检测相关方法的 Mixin"""

    def _flush_remaining_sse_data(
        self,
        ctx: StreamContext,
        buffer: bytes,
        decoder: codecs.IncrementalDecoder,
        sse_parser: SSEEventParser,
        *,
        record_chunk: bool = True,
    ) -> None:
        """
        异常发生时 flush 残留的字节 buffer 和 SSE parser 内部缓冲区。

        用于 StreamClosed / RemoteProtocolError 等场景：
        连接断开可能恰好发生在最后一个 SSE 事件（如 response.completed）
        的 data 行已收到、但终止空行尚未到达之时。此方法确保这些事件仍能被处理，
        从而正确捕获 usage 等关键信息。
        """
        try:
            # 1) flush 字节 buffer 中的残余行
            if buffer:
                remaining = decoder.decode(buffer, True)
                for line in remaining.split("\n"):
                    stripped = line.rstrip("\r")
                    events = sse_parser.feed_line(stripped)
                    for event in events:
                        self._handle_sse_event(
                            ctx,
                            event.get("event"),
                            event.get("data") or "",
                            record_chunk=record_chunk,
                        )
            # 2) flush SSE parser 内部累积的未完成事件
            for event in sse_parser.flush():
                self._handle_sse_event(
                    ctx,
                    event.get("event"),
                    event.get("data") or "",
                    record_chunk=record_chunk,
                )
        except Exception:
            # best-effort: 不应因 flush 失败影响后续流程
            pass

    def _estimate_tokens_for_incomplete_stream(
        self,
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
                "[{}] 流未正常完成 (has_completion=False, data_count={}), "
                "使用估算 tokens: in={}, out={}",
                ctx.request_id,
                ctx.data_count,
                ctx.input_tokens,
                ctx.output_tokens,
            )

    async def _prefetch_and_check_embedded_error(
        self: CliHandlerProtocol,
        byte_iterator: Any,
        provider: "Provider",
        endpoint: "ProviderEndpoint",
        ctx: StreamContext,
    ) -> list:
        """
        预读流的前几行，检测嵌套错误

        某些 Provider（如 Gemini）可能返回 HTTP 200，但在响应体中包含错误信息。
        这种情况需要在流开始输出之前检测，以便触发重试逻辑。

        同时检测 HTML 响应（通常是 base_url 配置错误导致返回网页）。

        首次读取时会应用 TTFB（首字节超时）检测，超时则触发故障转移。

        Args:
            byte_iterator: 字节流迭代器
            provider: Provider 对象
            endpoint: Endpoint 对象
            ctx: 流上下文

        Returns:
            预读的字节块列表（需要在后续流中先输出）

        Raises:
            EmbeddedErrorException: 如果检测到嵌套错误
            ProviderNotAvailableException: 如果检测到 HTML 响应（配置错误）
            ProviderTimeoutException: 如果首字节超时（TTFB timeout）
        """
        prefetched_chunks: list = []
        max_prefetch_lines = config.stream_prefetch_lines  # 最多预读行数来检测错误
        max_prefetch_bytes = StreamDefaults.MAX_PREFETCH_BYTES  # 避免无换行响应导致 buffer 增长
        total_prefetched_bytes = 0
        buffer = b""
        line_count = 0
        should_stop = False
        # 使用增量解码器处理跨 chunk 的 UTF-8 字符
        decoder = codecs.getincrementaldecoder("utf-8")(errors="replace")

        try:
            # 获取对应格式的解析器
            provider_format = ctx.provider_api_format
            if provider_format:
                try:
                    provider_parser = get_parser_for_format(provider_format)
                except KeyError:
                    provider_parser = self.parser
            else:
                provider_parser = self.parser

            # 使用共享的 TTFB 超时函数读取首字节
            # 优先使用 Provider 配置，否则使用全局配置
            ttfb_timeout = provider.stream_first_byte_timeout or config.stream_first_byte_timeout
            first_chunk, aiter = await read_first_chunk_with_ttfb_timeout(
                byte_iterator,
                timeout=ttfb_timeout,
                request_id=self.request_id,
                provider_name=str(provider.name),
            )
            prefetched_chunks.append(first_chunk)
            total_prefetched_bytes += len(first_chunk)
            buffer += first_chunk

            # 继续读取剩余的预读数据
            async for chunk in aiter:
                prefetched_chunks.append(chunk)
                total_prefetched_bytes += len(chunk)
                buffer += chunk

                # 尝试按行解析缓冲区（SSE 格式）
                while b"\n" in buffer:
                    line_bytes, buffer = buffer.split(b"\n", 1)
                    try:
                        # 使用增量解码器，可以正确处理跨 chunk 的多字节字符
                        line = decoder.decode(line_bytes + b"\n", False).rstrip("\n")
                    except Exception as e:
                        logger.warning(
                            f"[{self.request_id}] 预读时 UTF-8 解码失败: {e}, "
                            f"bytes={line_bytes[:50]!r}"
                        )
                        continue

                    line_count += 1
                    normalized_line = line.rstrip("\r")

                    # 检测 HTML 响应（base_url 配置错误的常见症状）
                    if check_html_response(normalized_line):
                        logger.error(
                            f"  [{self.request_id}] 检测到 HTML 响应，可能是 base_url 配置错误: "
                            f"Provider={provider.name}, Endpoint={endpoint.id[:8]}..., "
                            f"base_url={endpoint.base_url}"
                        )
                        raise ProviderNotAvailableException(
                            "上游服务返回了非预期的响应格式",
                            provider_name=str(provider.name),
                            upstream_status=200,
                            upstream_response=(
                                normalized_line[:500] if normalized_line else "(empty)"
                            ),
                        )

                    if not normalized_line or normalized_line.startswith(":"):
                        # 空行或注释行，继续预读
                        if line_count >= max_prefetch_lines:
                            break
                        continue

                    # 尝试解析 SSE 数据
                    data_str = normalized_line
                    if normalized_line.startswith("data: "):
                        data_str = normalized_line[6:]

                    if data_str == "[DONE]":
                        should_stop = True
                        break

                    try:
                        data = json.loads(data_str)
                    except json.JSONDecodeError:
                        # 不是有效 JSON，可能是部分数据，继续
                        if line_count >= max_prefetch_lines:
                            break
                        continue

                    # 使用解析器检查是否为错误响应
                    if isinstance(data, dict) and provider_parser.is_error_response(data):
                        # 提取错误信息
                        parsed = provider_parser.parse_response(data, 200)
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

                # 达到预读字节上限，停止继续预读（避免无换行响应导致内存增长）
                if not should_stop and total_prefetched_bytes >= max_prefetch_bytes:
                    logger.debug(
                        f"  [{self.request_id}] 预读达到字节上限，停止继续预读: "
                        f"Provider={provider.name}, bytes={total_prefetched_bytes}, "
                        f"max_bytes={max_prefetch_bytes}"
                    )
                    break

                if should_stop or line_count >= max_prefetch_lines:
                    break

            # 预读结束后，检查是否为非 SSE 格式的 HTML/JSON 响应
            # 处理某些代理返回的纯 JSON 错误（可能无换行/多行 JSON）以及 HTML 页面（base_url 配置错误）
            if not should_stop and prefetched_chunks:
                check_prefetched_response_error(
                    prefetched_chunks=prefetched_chunks,
                    parser=provider_parser,
                    request_id=self.request_id,
                    provider_name=str(provider.name),
                    endpoint_id=endpoint.id,
                    base_url=endpoint.base_url,
                )

        except (EmbeddedErrorException, ProviderTimeoutException, ProviderNotAvailableException):
            # 重新抛出可重试的 Provider 异常，触发故障转移
            raise
        except OSError as e:
            # 网络 I/O 异常：记录警告，可能需要重试
            logger.warning(
                "  [{}] 预读流时发生网络异常: {}: {}", self.request_id, type(e).__name__, e
            )
        except Exception as e:
            # 未预期的严重异常：记录错误并重新抛出，避免掩盖问题
            logger.error(
                f"  [{self.request_id}] 预读流时发生严重异常: {type(e).__name__}: {e}",
                exc_info=True,
            )
            raise

        return prefetched_chunks

    async def _create_response_stream_with_prefetch(
        self,
        ctx: StreamContext,
        byte_iterator: Any,
        response_ctx: Any,
        prefetched_chunks: list,
    ) -> AsyncGenerator[bytes]:
        """创建响应流生成器（带预读数据，使用字节流）"""
        try:
            sse_parser = SSEEventParser()
            last_data_time = time.time()
            buffer = b""
            output_state = {"first_yield": True, "streaming_updated": False}
            _sample_lines: list[str] = []  # 采集前几行原始内容，用于空流诊断
            _MAX_SAMPLE_LINES = 5
            # 使用增量解码器处理跨 chunk 的 UTF-8 字符
            decoder = codecs.getincrementaldecoder("utf-8")(errors="replace")

            # 使用已设置的 ctx.needs_conversion（由候选筛选阶段根据端点配置判断）
            # 不再调用 _needs_format_conversion，它只检查格式差异，不检查端点配置
            needs_conversion = ctx.needs_conversion
            behavior = get_provider_behavior(
                provider_type=ctx.provider_type,
                endpoint_sig=ctx.provider_api_format,
            )
            envelope = behavior.envelope
            if envelope and envelope.force_stream_rewrite():
                needs_conversion = True
                ctx.needs_conversion = True

            # Kiro 特殊处理：AWS Event Stream 二进制流需要重写为 SSE
            ctx_provider_type = str(ctx.provider_type or "").strip().lower()
            if ctx_provider_type == "kiro" and envelope and envelope.force_stream_rewrite():
                from src.services.provider.adapters.kiro.eventstream_rewriter import (
                    apply_kiro_stream_rewrite,
                )

                byte_iterator = apply_kiro_stream_rewrite(
                    byte_iterator,
                    model=str(ctx.model or ""),
                    input_tokens=int(ctx.input_tokens or 0),
                    prefetched_chunks=list(prefetched_chunks) if prefetched_chunks else None,
                )
                prefetched_chunks = []

                # Kiro 重写后输出的是 Claude SSE 格式
                # 客户端也是 Claude CLI，不需要再进行格式转换
                needs_conversion = False
                ctx.needs_conversion = False

            # 先处理预读的字节块
            for chunk in prefetched_chunks:
                buffer += chunk
                # 处理缓冲区中的完整行
                while b"\n" in buffer:
                    line_bytes, buffer = buffer.split(b"\n", 1)
                    try:
                        # 使用增量解码器，可以正确处理跨 chunk 的多字节字符
                        line = decoder.decode(line_bytes + b"\n", False).rstrip("\n")
                    except Exception as e:
                        logger.warning(
                            f"[{self.request_id}] UTF-8 解码失败: {e}, "
                            f"bytes={line_bytes[:50]!r}"
                        )
                        continue

                    normalized_line = line.rstrip("\r")
                    events = sse_parser.feed_line(normalized_line)

                    if normalized_line == "":
                        for event in events:
                            self._handle_sse_event(
                                ctx,
                                event.get("event"),
                                event.get("data") or "",
                                record_chunk=not needs_conversion,
                            )
                        self._mark_first_output(ctx, output_state)
                        yield b"\n"
                        continue

                    ctx.chunk_count += 1
                    if len(_sample_lines) < _MAX_SAMPLE_LINES:
                        _sample_lines.append(normalized_line[:200])

                    # 格式转换或直接透传
                    if needs_conversion:
                        converted_lines, converted_events = self._convert_sse_line(
                            ctx, line, events
                        )
                        # 记录转换后的数据到 parsed_chunks
                        self._record_converted_chunks(ctx, converted_events)
                        for converted_line in converted_lines:
                            if converted_line:
                                self._mark_first_output(ctx, output_state)
                                yield (converted_line + "\n").encode("utf-8")
                    else:
                        self._mark_first_output(ctx, output_state)
                        yield (line + "\n").encode("utf-8")

                    for event in events:
                        self._handle_sse_event(
                            ctx,
                            event.get("event"),
                            event.get("data") or "",
                            record_chunk=not needs_conversion,
                        )

                    if ctx.data_count > 0:
                        last_data_time = time.time()

            # 继续处理剩余的流数据（使用同一个迭代器）
            async for chunk in byte_iterator:
                buffer += chunk
                # 处理缓冲区中的完整行
                while b"\n" in buffer:
                    line_bytes, buffer = buffer.split(b"\n", 1)
                    try:
                        # 使用增量解码器，可以正确处理跨 chunk 的多字节字符
                        line = decoder.decode(line_bytes + b"\n", False).rstrip("\n")
                    except Exception as e:
                        logger.warning(
                            f"[{self.request_id}] UTF-8 解码失败: {e}, "
                            f"bytes={line_bytes[:50]!r}"
                        )
                        continue

                    normalized_line = line.rstrip("\r")
                    events = sse_parser.feed_line(normalized_line)

                    if normalized_line == "":
                        for event in events:
                            self._handle_sse_event(
                                ctx,
                                event.get("event"),
                                event.get("data") or "",
                                record_chunk=not needs_conversion,
                            )
                        self._mark_first_output(ctx, output_state)
                        yield b"\n"
                        continue

                    ctx.chunk_count += 1
                    if len(_sample_lines) < _MAX_SAMPLE_LINES:
                        _sample_lines.append(normalized_line[:200])

                    # 空流检测：超过阈值且无数据，发送错误事件并结束
                    if ctx.chunk_count > self.EMPTY_CHUNK_THRESHOLD and ctx.data_count == 0:
                        elapsed = time.time() - last_data_time
                        if elapsed > self.DATA_TIMEOUT:
                            logger.warning("Provider '{}' 流超时且无数据", ctx.provider_name)
                            # 设置错误状态用于后续记录
                            ctx.status_code = 504
                            ctx.error_message = "流式响应超时，未收到有效数据"
                            ctx.upstream_response = f"流超时: Provider={ctx.provider_name}, elapsed={elapsed:.1f}s, chunk_count={ctx.chunk_count}, data_count=0"
                            error_event = {
                                "type": "error",
                                "error": {
                                    "type": "empty_stream_timeout",
                                    "message": ctx.error_message,
                                },
                            }
                            self._mark_first_output(ctx, output_state)
                            yield f"event: error\ndata: {json.dumps(error_event)}\n\n".encode()
                            return

                    # 格式转换或直接透传
                    if needs_conversion:
                        converted_lines, converted_events = self._convert_sse_line(
                            ctx, line, events
                        )
                        # 记录转换后的数据到 parsed_chunks
                        self._record_converted_chunks(ctx, converted_events)
                        for converted_line in converted_lines:
                            if converted_line:
                                self._mark_first_output(ctx, output_state)
                                yield (converted_line + "\n").encode("utf-8")
                    else:
                        self._mark_first_output(ctx, output_state)
                        yield (line + "\n").encode("utf-8")

                    for event in events:
                        self._handle_sse_event(
                            ctx,
                            event.get("event"),
                            event.get("data") or "",
                            record_chunk=not needs_conversion,
                        )

                    if ctx.data_count > 0:
                        last_data_time = time.time()

            # flush 字节 buffer 残余数据 + SSE parser 内部缓冲区
            for chunk in self._flush_buffer_with_conversion(
                ctx, buffer, decoder, sse_parser, needs_conversion
            ):
                yield chunk

            # 检查是否收到数据
            if ctx.data_count == 0:
                # 空流通常意味着配置错误（如 base_url 指向了网页而非 API）
                sample_info = f", 前几行内容: {_sample_lines!r}" if _sample_lines else ""
                logger.error(
                    f"Provider '{ctx.provider_name}' 返回空流式响应 (收到 {ctx.chunk_count} 个非数据行), "
                    f"可能是 endpoint base_url 配置错误{sample_info}"
                )
                # 设置错误状态用于后续记录
                ctx.status_code = 503
                ctx.error_message = "上游服务返回了空的流式响应"
                ctx.upstream_response = f"空流式响应: Provider={ctx.provider_name}, chunk_count={ctx.chunk_count}, data_count=0, 可能是 base_url 配置错误"
                error_event = {
                    "type": "error",
                    "error": {
                        "type": "empty_response",
                        "message": ctx.error_message,
                    },
                }
                yield f"event: error\ndata: {json.dumps(error_event)}\n\n".encode()
            else:
                logger.debug("流式数据转发完成")
                # 为 OpenAI 客户端补齐 [DONE] 标记（非 CLI 格式）
                client_fmt = (ctx.client_api_format or "").strip().lower()
                if needs_conversion and client_fmt == "openai:chat":
                    yield b"data: [DONE]\n\n"

        except GeneratorExit:
            raise
        except httpx.StreamClosed:
            # 连接关闭前 flush 残余数据，尝试捕获尾部事件（如 response.completed 中的 usage）
            self._flush_remaining_sse_data(
                ctx, buffer, decoder, sse_parser, record_chunk=not needs_conversion
            )
            if ctx.data_count == 0:
                logger.warning("Provider '{}' 流连接关闭且无数据", ctx.provider_name)
                # 设置错误状态用于后续记录
                ctx.status_code = 503
                ctx.error_message = "上游服务连接关闭且未返回数据"
                ctx.upstream_response = f"流连接关闭: Provider={ctx.provider_name}, chunk_count={ctx.chunk_count}, data_count=0"
                error_event = {
                    "type": "error",
                    "error": {
                        "type": "stream_closed",
                        "message": ctx.error_message,
                    },
                }
                yield f"event: error\ndata: {json.dumps(error_event)}\n\n".encode()
        except httpx.RemoteProtocolError:
            # 连接异常关闭前 flush 残余数据，尝试捕获尾部事件（如 response.completed 中的 usage）
            self._flush_remaining_sse_data(
                ctx, buffer, decoder, sse_parser, record_chunk=not needs_conversion
            )
            if ctx.data_count > 0:
                error_event = {
                    "type": "error",
                    "error": {
                        "type": "connection_error",
                        "message": "上游连接意外关闭，部分响应已成功传输",
                    },
                }
                yield f"event: error\ndata: {json.dumps(error_event)}\n\n".encode()
            else:
                raise
        except httpx.ReadError:
            # 代理/上游连接读取失败（如 aether-proxy 中断），与 RemoteProtocolError 处理逻辑一致
            self._flush_remaining_sse_data(
                ctx, buffer, decoder, sse_parser, record_chunk=not needs_conversion
            )
            if ctx.data_count > 0:
                error_event = {
                    "type": "error",
                    "error": {
                        "type": "connection_error",
                        "message": "代理或上游连接读取失败，部分响应已成功传输",
                    },
                }
                yield f"event: error\ndata: {json.dumps(error_event)}\n\n".encode()
            else:
                raise
        finally:
            try:
                await response_ctx.__aexit__(None, None, None)
            except Exception:
                pass
