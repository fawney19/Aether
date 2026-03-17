"""
流式处理器 - 从 ChatHandlerBase 提取的流式响应处理逻辑

职责：
1. SSE 事件解析和处理
2. 响应流生成
3. 预读和嵌套错误检测
4. 客户端断开检测
5. 流式平滑输出
"""

from __future__ import annotations

import asyncio
import codecs
import json
import time
from collections.abc import AsyncGenerator, Callable
from dataclasses import dataclass
from typing import Any

import httpx

from src.api.handlers.base.content_extractors import (
    ContentExtractor,
    get_extractor,
    get_extractor_formats,
)
from src.api.handlers.base.parsers import get_parser_for_format
from src.api.handlers.base.response_parser import ResponseParser
from src.api.handlers.base.stream_context import StreamContext
from src.api.handlers.base.utils import (
    check_html_response,
    check_prefetched_response_error,
    ensure_stream_buffer_limit,
    get_format_converter_registry,
)
from src.config.constants import StreamDefaults
from src.config.settings import config
from src.core.api_format.conversion.exceptions import FormatConversionError
from src.core.exceptions import (
    EmbeddedErrorException,
    ProviderNotAvailableException,
    ProviderTimeoutException,
)
from src.core.logger import logger
from src.core.usage_tokens import extract_cache_creation_tokens, extract_cache_read_tokens
from src.models.database import Provider, ProviderEndpoint
from src.services.provider.behavior import get_provider_behavior
from src.utils.perf import PerfRecorder
from src.utils.sse_parser import SSEEventParser
from src.utils.timeout import read_first_chunk_with_ttfb_timeout


@dataclass
class StreamSmoothingConfig:
    """流式平滑输出配置"""

    enabled: bool = False
    chunk_size: int = 20
    delay_ms: int = 8


class StreamProcessor:
    """
    流式响应处理器

    负责处理 SSE 流的解析、错误检测、响应生成和平滑输出。
    从 ChatHandlerBase 中提取，使其职责更加单一。
    """

    def __init__(
        self,
        request_id: str,
        default_parser: ResponseParser,
        on_streaming_start: Callable[[], None] | None = None,
        *,
        collect_text: bool = False,
        smoothing_config: StreamSmoothingConfig | None = None,
    ):
        """
        初始化流处理器

        Args:
            request_id: 请求 ID（用于日志）
            default_parser: 默认响应解析器
            on_streaming_start: 流开始时的回调（用于更新状态）
            collect_text: 是否收集文本内容
            smoothing_config: 流式平滑输出配置
        """
        self.request_id = request_id
        self.default_parser = default_parser
        self.on_streaming_start = on_streaming_start
        self.collect_text = collect_text
        self.smoothing_config = smoothing_config or StreamSmoothingConfig()

        # 内容提取器缓存
        self._extractors: dict[str, ContentExtractor] = {}

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

    @staticmethod
    def _maybe_mark_gemini_completion(ctx: StreamContext, data: dict[str, Any]) -> None:
        """Gemini: mark completion based on candidates[].finishReason."""
        candidates = data.get("candidates")
        if not isinstance(candidates, list) or not candidates:
            return
        for candidate in candidates:
            if not isinstance(candidate, dict):
                continue
            finish_reason = candidate.get("finishReason")
            if finish_reason is None:
                continue
            # UNSPECIFIED is the only clear "not done" sentinel across Gemini variants.
            if str(finish_reason) != "FINISH_REASON_UNSPECIFIED":
                ctx.has_completion = True
                return

    @staticmethod
    def _extract_antigravity_usage_from_gemini_event(data: dict[str, Any]) -> dict[str, int] | None:
        """Antigravity: lenient Gemini usage extraction (totalTokenCount may be missing)."""
        usage_metadata = data.get("usageMetadata", {})
        if not isinstance(usage_metadata, dict) or not usage_metadata:
            return None

        def _as_int(v: Any) -> int:
            try:
                return int(v or 0)
            except Exception:
                return 0

        prompt = _as_int(usage_metadata.get("promptTokenCount"))
        cached = _as_int(usage_metadata.get("cachedContentTokenCount"))
        candidates = _as_int(usage_metadata.get("candidatesTokenCount"))
        thoughts = _as_int(usage_metadata.get("thoughtsTokenCount"))

        # Align with Gemini billing convention: input_tokens includes cached content.
        return {
            "input_tokens": max(0, prompt),
            "output_tokens": max(0, candidates + thoughts),
            "cache_creation_tokens": 0,
            "cache_read_tokens": max(0, cached),
        }

    def _unwrap_provider_envelope(self, ctx: StreamContext, data: dict[str, Any]) -> dict[str, Any]:
        behavior = get_provider_behavior(
            provider_type=str(getattr(ctx, "provider_type", "") or ""),
            endpoint_sig=str(getattr(ctx, "provider_api_format", "") or ""),
        )
        envelope = behavior.envelope
        if not envelope:
            return data

        try:
            unwrapped = envelope.unwrap_response(data)
            envelope.postprocess_unwrapped_response(
                model=str(getattr(ctx, "model", "") or ""),
                data=unwrapped,
            )
            return unwrapped if isinstance(unwrapped, dict) else data
        except Exception:
            return data

    def _update_ctx_from_provider_event(
        self,
        ctx: StreamContext,
        data: dict[str, Any],
        *,
        already_unwrapped: bool = False,
    ) -> None:
        # Unwrap provider-specific envelopes (e.g. Antigravity v1internal wrapper)
        if not already_unwrapped:
            data = self._unwrap_provider_envelope(ctx, data)

        parser = self.get_parser_for_provider(ctx)

        # Provider usage extraction (best-effort)
        provider_type = str(getattr(ctx, "provider_type", "") or "").lower()
        provider_format = str(getattr(ctx, "provider_api_format", "") or "").strip().lower()

        usage: dict[str, int] | None = None
        if provider_type == "antigravity" and provider_format.startswith("gemini:"):
            usage = self._extract_antigravity_usage_from_gemini_event(data)
        if usage is None:
            try:
                usage = parser.extract_usage_from_response(data)
            except Exception:
                usage = None

        if usage:
            ctx.update_usage(
                input_tokens=usage.get("input_tokens"),
                output_tokens=usage.get("output_tokens"),
                cached_tokens=usage.get("cache_read_tokens"),
                cache_creation_tokens=usage.get("cache_creation_tokens"),
            )

        # Provider completion detection (Gemini doesn't emit response.completed).
        if provider_format.startswith("gemini:"):
            self._maybe_mark_gemini_completion(ctx, data)

        # Provider text extraction (optional)
        if self.collect_text:
            try:
                text = parser.extract_text_content(data)
            except Exception:
                text = ""
            if text:
                ctx.append_text(text)

    def handle_sse_event(
        self,
        ctx: StreamContext,
        event_name: str | None,
        data_str: str,
        *,
        skip_record: bool = False,
        skip_ctx_update: bool = False,
    ) -> None:
        """
        处理单个 SSE 事件

        解析事件数据，提取 usage 信息和文本内容。

        Args:
            ctx: 流式上下文
            event_name: 事件名称
            data_str: 事件数据字符串
            skip_record: 是否跳过记录到 parsed_chunks（当需要格式转换时应为 True）
            skip_ctx_update: 跳过 usage/completion/text 提取（由调用方统一处理时使用）
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

        if not isinstance(data, dict):
            return

        # Update usage/completion/text from provider event (envelope-aware).
        # 在 needs_conversion 正常流中由 _emit_converted_line 统一处理，此处跳过以避免重复。
        if not skip_ctx_update:
            self._update_ctx_from_provider_event(ctx, data)

        # 统计数据事件数量（当需要格式转换时跳过，由 _emit_converted_line 统计/记录转换后的数据）
        if not skip_record:
            ctx.data_count += 1
            if ctx.record_parsed_chunks:
                ctx.parsed_chunks.append(data)
        else:
            # 格式转换场景：保留提供商原始数据
            if ctx.record_parsed_chunks:
                ctx.provider_parsed_chunks.append(data)

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

    def _extract_usage_from_converted_event(
        self,
        ctx: StreamContext,
        evt: dict[str, Any],
        event_type: str,
    ) -> None:
        """
        从转换后的事件中提取 usage 信息（补充 Provider 事件解析）。

        支持多种格式：
        - Claude: message_delta.usage, message_start.message.usage
        - OpenAI: chunk.usage / response.completed.response.usage
        - Gemini: usageMetadata
        """
        usage: dict[str, Any] | None = None

        # Claude 格式: message_delta 或 message_start
        if event_type == "message_delta":
            usage = evt.get("usage")
        elif event_type == "message_start":
            message = evt.get("message", {})
            if isinstance(message, dict):
                usage = message.get("usage")
        # OpenAI Responses API 格式: response.completed 中 usage 嵌套在 response 对象内
        elif event_type == "response.completed":
            resp_obj = evt.get("response")
            if isinstance(resp_obj, dict):
                usage = resp_obj.get("usage")
            # 兼容: 部分实现可能在顶层也有 usage
            if not usage:
                usage = evt.get("usage")
        # OpenAI Chat 格式: 直接在 chunk 中
        elif "usage" in evt:
            usage = evt.get("usage")
        # Gemini 格式: usageMetadata
        elif "usageMetadata" in evt:
            meta = evt.get("usageMetadata", {})
            if isinstance(meta, dict):
                usage = {
                    "input_tokens": meta.get("promptTokenCount", 0),
                    "output_tokens": meta.get("candidatesTokenCount", 0),
                    "cache_read_tokens": meta.get("cachedContentTokenCount", 0),
                    "cache_creation_tokens": 0,
                }

        if usage and isinstance(usage, dict):
            new_input = usage.get("input_tokens") or usage.get("prompt_tokens") or 0
            new_output = usage.get("output_tokens") or usage.get("completion_tokens") or 0
            new_cached = extract_cache_read_tokens(usage)
            new_cache_creation = extract_cache_creation_tokens(usage)

            if new_input > ctx.input_tokens:
                ctx.input_tokens = new_input
                logger.debug("[{}] 从转换后事件更新 input_tokens: {}", self.request_id, new_input)
            if new_output > ctx.output_tokens:
                ctx.output_tokens = new_output
                logger.debug("[{}] 从转换后事件更新 output_tokens: {}", self.request_id, new_output)
            if new_cached > ctx.cached_tokens:
                ctx.cached_tokens = new_cached
            if new_cache_creation > ctx.cache_creation_tokens:
                ctx.cache_creation_tokens = new_cache_creation

            if any([new_input, new_output, new_cached, new_cache_creation]):
                ctx.final_usage = usage

    async def prefetch_and_check_error(
        self,
        byte_iterator: Any,
        provider: Provider,
        endpoint: ProviderEndpoint,
        ctx: StreamContext,
        max_prefetch_lines: int = 5,
        max_prefetch_bytes: int = StreamDefaults.MAX_PREFETCH_BYTES,
    ) -> list:
        """
        预读流的前几行，检测嵌套错误

        某些 Provider（如 Gemini）可能返回 HTTP 200，但在响应体中包含错误信息。
        这种情况需要在流开始输出之前检测，以便触发重试逻辑。

        首次读取时会应用 TTFB（首字节超时）检测，超时则触发故障转移。

        Args:
            byte_iterator: 字节流迭代器
            provider: Provider 对象
            endpoint: Endpoint 对象
            ctx: 流式上下文
            max_prefetch_lines: 最多预读行数
            max_prefetch_bytes: 最多预读字节数（避免无换行响应导致 buffer 增长）

        Returns:
            预读的字节块列表

        Raises:
            EmbeddedErrorException: 如果检测到嵌套错误
            ProviderNotAvailableException: 如果检测到 HTML 响应（配置错误）
            ProviderTimeoutException: 如果首字节超时（TTFB timeout）
        """
        prefetched_chunks: list = []
        parser = self.get_parser_for_provider(ctx)
        behavior = get_provider_behavior(
            provider_type=str(getattr(ctx, "provider_type", "") or ""),
            endpoint_sig=str(getattr(ctx, "provider_api_format", "") or ""),
        )
        envelope = behavior.envelope
        ctx_provider_type = str(getattr(ctx, "provider_type", "") or "").strip().lower()
        kiro_binary_stream = (
            ctx_provider_type == "kiro" and envelope and envelope.force_stream_rewrite()
        )
        buffer = b""
        line_count = 0
        should_stop = False
        total_prefetched_bytes = 0
        # 使用增量解码器处理跨 chunk 的 UTF-8 字符
        decoder = codecs.getincrementaldecoder("utf-8")(errors="replace")

        try:
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

            # Kiro upstream uses AWS Event Stream (binary). Do not attempt to split/decode lines here;
            # we only enforce TTFB and let StreamProcessor rewrite bytes later.
            if kiro_binary_stream:
                return prefetched_chunks
            buffer += first_chunk
            ensure_stream_buffer_limit(
                buffer,
                request_id=self.request_id,
                provider_name=str(provider.name),
            )

            # 继续读取剩余的预读数据
            async for chunk in aiter:
                prefetched_chunks.append(chunk)
                total_prefetched_bytes += len(chunk)
                buffer += chunk
                ensure_stream_buffer_limit(
                    buffer,
                    request_id=self.request_id,
                    provider_name=str(provider.name),
                )

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

                    # 检测 HTML 响应（base_url 配置错误的常见症状）
                    if check_html_response(line):
                        logger.error(
                            f"  [{self.request_id}] 检测到 HTML 响应，可能是 base_url 配置错误: "
                            f"Provider={provider.name}, Endpoint={endpoint.id[:8]}..., "
                            f"base_url={endpoint.base_url}"
                        )
                        raise ProviderNotAvailableException(
                            "上游服务返回了非预期的响应格式",
                            provider_name=str(provider.name),
                            upstream_status=200,
                            upstream_response=line[:500] if line else "(empty)",
                        )

                    # 跳过空行和注释行
                    if not line or line.startswith(":"):
                        if line_count >= max_prefetch_lines:
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
                            break
                        continue

                    # Provider envelope: unwrap SSE data chunk before error detection / trial conversion.
                    if envelope and isinstance(data, dict):
                        data = envelope.unwrap_response(data)
                        envelope.postprocess_unwrapped_response(
                            model=str(ctx.model or ""),
                            data=data,
                        )

                    # 使用解析器检查是否为错误响应
                    if isinstance(data, dict) and parser.is_error_response(data):
                        parsed = parser.parse_response(data, 200)
                        logger.warning(
                            f"  [{self.request_id}] 检测到嵌套错误: "
                            f"Provider={provider.name}, "
                            f"error_type={parsed.error_type}, "
                            f"embedded_status={parsed.embedded_status_code}, "
                            f"message={parsed.error_message}"
                        )
                        raise EmbeddedErrorException(
                            provider_name=str(provider.name),
                            error_code=parsed.embedded_status_code,
                            error_message=parsed.error_message,
                            error_status=parsed.error_type,
                        )

                    # 预读阶段格式转换试验：首字节前可 failover
                    # 如果需要跨格式转换，对首个有效数据块做试转换
                    if ctx.needs_conversion and isinstance(data, dict):
                        # 新模式：endpoint signature key（family:kind），这里仅用于转换器选择，不做 legacy 兼容
                        client_format = (ctx.client_api_format or "").strip().lower()
                        provider_format = (ctx.provider_api_format or "").strip().lower()
                        if client_format and provider_format:
                            try:
                                # 试转换：传 state=None，不保留状态
                                # 如果失败触发 failover，下一个候选会使用干净的 state
                                registry = get_format_converter_registry()
                                registry.convert_stream_chunk(
                                    data,
                                    provider_format,
                                    client_format,
                                    state=None,
                                )
                            except FormatConversionError as conv_err:
                                # 格式转换失败：抛出异常触发 failover
                                logger.debug(
                                    f"  [{self.request_id}] 预读阶段格式转换试验失败: "
                                    f"Provider={provider.name}, "
                                    f"{provider_format} -> {client_format}, "
                                    f"error={conv_err}"
                                )
                                raise

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
            if not should_stop and prefetched_chunks:
                check_prefetched_response_error(
                    prefetched_chunks=prefetched_chunks,
                    parser=parser,
                    request_id=self.request_id,
                    provider_name=str(provider.name),
                    endpoint_id=endpoint.id,
                    base_url=endpoint.base_url,
                )

        except (
            EmbeddedErrorException,
            ProviderNotAvailableException,
            ProviderTimeoutException,
            FormatConversionError,
        ):
            # 重新抛出可重试的 Provider 异常，触发故障转移
            raise
        except OSError as e:
            # 网络 I/O 异常：记录警告，可能需要重试
            logger.warning(f"  [{self.request_id}] 预读流时发生网络异常: {type(e).__name__}: {e}")
        except Exception as e:
            # 未预期的严重异常：记录错误并重新抛出，避免掩盖问题
            logger.error(
                f"  [{self.request_id}] 预读流时发生严重异常: {type(e).__name__}: {e}",
                exc_info=True,
            )
            raise

        return prefetched_chunks

    async def create_response_stream(
        self,
        ctx: StreamContext,
        byte_iterator: Any,
        response_ctx: Any,
        prefetched_chunks: list | None = None,
        *,
        start_time: float | None = None,
    ) -> AsyncGenerator[bytes]:
        """
        创建响应流生成器

        从字节流中解析 SSE 数据并转发，支持预读数据。

        Args:
            ctx: 流式上下文
            byte_iterator: 字节流迭代器
            response_ctx: HTTP 响应上下文管理器
            prefetched_chunks: 预读的字节块列表（可选）
            start_time: 请求开始时间,用于计算 TTFB（可选）

        Yields:
            编码后的响应数据块
        """
        try:
            sse_parser = SSEEventParser()
            streaming_started = False
            yielded_any = False
            buffer = b""
            # 使用增量解码器处理跨 chunk 的 UTF-8 字符
            decoder = codecs.getincrementaldecoder("utf-8")(errors="replace")
            metrics_enabled = PerfRecorder.enabled()
            perf_capture = metrics_enabled or ctx.perf_sampled
            parse_time = 0.0
            convert_time = 0.0

            _api_format_str = str(ctx.api_format or "")
            client_format = (ctx.client_api_format or _api_format_str).strip().lower()
            provider_format = (ctx.provider_api_format or _api_format_str).strip().lower()
            client_family = (
                client_format.split(":", 1)[0] if ":" in client_format else client_format
            ) or "unknown"
            provider_family = (
                provider_format.split(":", 1)[0] if ":" in provider_format else provider_format
            ) or "unknown"
            # 使用 handler 层预计算的 needs_conversion（由 candidate 决定）
            needs_conversion = ctx.needs_conversion
            behavior = get_provider_behavior(
                provider_type=str(getattr(ctx, "provider_type", "") or ""),
                endpoint_sig=str(getattr(ctx, "provider_api_format", "") or ""),
            )
            envelope = behavior.envelope
            if envelope and envelope.force_stream_rewrite():
                needs_conversion = True
                ctx.needs_conversion = True

            ctx_provider_type = str(getattr(ctx, "provider_type", "") or "").strip().lower()
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
                prefetched_chunks = None

                # Kiro 重写后输出的是 Claude SSE 格式（data: {...}\n\n）
                # 如果客户端也是 Claude 格式，则不需要再进行格式转换
                if client_family == "claude":
                    needs_conversion = False
                    ctx.needs_conversion = False

            # 安全检查：needs_conversion 为 True 时，provider_format 必须有值
            if needs_conversion and not provider_format:
                logger.warning(
                    f"[{self.request_id}] needs_conversion=True 但 provider_format 为空，回退到透传模式"
                )
                needs_conversion = False
                # 保持 ctx 与实际行为一致，避免 Usage 记录误标记为转换
                ctx.needs_conversion = False

            def _mark_stream_started() -> None:
                nonlocal start_time, streaming_started, yielded_any
                yielded_any = True
                # 记录首字时间 (TTFB) - 在 yield 之前记录
                if start_time is not None:
                    ctx.record_first_byte_time(start_time)
                    start_time = None  # 只记录一次
                # 首次输出前触发 streaming 回调（确保 TTFB 已写入 ctx）
                if not streaming_started and self.on_streaming_start:
                    self.on_streaming_start()
                    streaming_started = True

            def _process_line_with_perf(
                line: str,
                *,
                skip_record: bool = False,
                skip_ctx_update: bool = False,
            ) -> None:
                nonlocal parse_time
                if perf_capture:
                    t0 = time.perf_counter()
                    self._process_line(
                        ctx,
                        sse_parser,
                        line,
                        skip_record=skip_record,
                        skip_ctx_update=skip_ctx_update,
                    )
                    parse_time += time.perf_counter() - t0
                    return
                self._process_line(
                    ctx,
                    sse_parser,
                    line,
                    skip_record=skip_record,
                    skip_ctx_update=skip_ctx_update,
                )

            def _build_stream_error_payload(message: str) -> dict:
                if client_family == "openai":
                    return {
                        "error": {
                            "message": message,
                            "type": "format_conversion_error",
                        }
                    }
                # Claude 及其他格式使用统一的错误结构
                return {
                    "type": "error",
                    "error": {
                        "type": "format_conversion_error",
                        "message": message,
                    },
                }

            def _format_sse_event(evt: dict) -> bytes:
                """根据客户端格式生成 SSE 事件字节。

                Claude 格式需要 event: 行，OpenAI/Gemini 只需要 data: 行。
                """
                if client_family == "claude":
                    event_type_str = evt.get("type", "")
                    if event_type_str:
                        return f"event: {event_type_str}\ndata: {json.dumps(evt, ensure_ascii=False)}\n\n".encode()
                return f"data: {json.dumps(evt, ensure_ascii=False)}\n\n".encode()

            # 处理预读数据
            if needs_conversion:
                registry = get_format_converter_registry()

                # 初始化流式转换状态（Canonical）
                if ctx.stream_conversion_state is None:
                    from src.core.api_format.conversion.stream_state import StreamState

                    # 使用客户端请求的模型（ctx.model），而非映射后的上游模型（ctx.mapped_model）
                    ctx.stream_conversion_state = StreamState(
                        model=ctx.model or "",
                        message_id=ctx.response_id or ctx.request_id or "",
                    )

                # 转换状态变量（在 needs_conversion 块内统一初始化，确保作用域正确）
                skip_next_blank_line = False
                empty_yield_count = 0  # 空转计数（防护异常情况）
                openai_done_sent = (
                    False  # 统一为 OpenAI 客户端补齐 [DONE]（避免不同 Provider 行为差异）
                )

                def _emit_converted_line(normalized_line: str) -> list[bytes]:
                    nonlocal skip_next_blank_line, openai_done_sent, convert_time

                    # 空行：事件分隔符（避免重复输出）
                    if normalized_line == "":
                        if skip_next_blank_line:
                            skip_next_blank_line = False
                            return []
                        return [b"\n"]

                    # 丢弃 Provider 的 event 行，避免泄漏/污染目标格式
                    if normalized_line.startswith("event:"):
                        return []

                    # OpenAI done 信号（仅用于 OpenAI 客户端）
                    if (
                        normalized_line.startswith("data:")
                        and normalized_line[5:].strip() == "[DONE]"
                    ):
                        skip_next_blank_line = True
                        if client_family == "openai":
                            openai_done_sent = True
                            return [b"data: [DONE]\n\n"]
                        return []

                    # 默认只处理 SSE 的 data 行；但 Gemini 上游可能返回 JSON-array/chunks（无 data 前缀）
                    is_data_line = normalized_line.startswith("data:")
                    if not is_data_line:
                        if provider_family != "gemini":
                            return []
                        data_content = normalized_line.strip()
                    else:
                        data_content = normalized_line[5:].strip()

                    # Gemini 可能包含 JSON 数组包装符，直接忽略
                    if data_content in ("", "[", "]", ","):
                        return []
                    # JSON-array/chunks 可能带前后逗号（对象分隔符），做一次保守清理
                    data_content = data_content.lstrip(",").rstrip(",").strip()
                    if data_content in ("", "[", "]", ","):
                        return []

                    convert_start = time.perf_counter() if perf_capture else None
                    try:
                        data_obj = json.loads(data_content)
                    except json.JSONDecodeError:
                        if perf_capture and convert_start is not None:
                            convert_time += time.perf_counter() - convert_start
                        # 跨格式转换时，JSON 解析失败应跳过而不是透传（避免泄漏 Provider 格式）
                        logger.warning(
                            f"[{self.request_id}] JSON 解析失败，跳过该行: {data_content[:100]}"
                        )
                        return []

                    if not isinstance(data_obj, dict):
                        return []

                    # Provider envelope: unwrap v1internal wrapper before conversion
                    # (e.g. Antigravity {"response": {...}, "traceId": "..."} → inner response)
                    if envelope and isinstance(data_obj, dict):
                        data_obj = envelope.unwrap_response(data_obj)
                        envelope.postprocess_unwrapped_response(
                            model=str(ctx.model or ""),
                            data=data_obj,
                        )

                    # Update usage/completion/text based on the unwrapped provider event.
                    if isinstance(data_obj, dict):
                        self._update_ctx_from_provider_event(
                            ctx,
                            data_obj,
                            already_unwrapped=True,
                        )

                    try:
                        converted_events = registry.convert_stream_chunk(
                            data_obj,
                            provider_format,
                            client_format,
                            state=ctx.stream_conversion_state,
                        )
                    except Exception as conv_err:
                        # 首字节后无法 failover：输出目标格式错误事件并终止流
                        # 使用 502 表示上游返回了非预期格式（Bad Gateway）
                        ctx.status_code = 502
                        ctx.error_message = "format_conversion_failed"
                        # 日志记录完整错误（内部排查），客户端只返回脱敏消息
                        logger.warning(f"[{self.request_id}] 流式格式转换失败: {conv_err}")
                        payload = _build_stream_error_payload("响应格式转换失败，请稍后重试")
                        error_bytes = _format_sse_event(payload)
                        done_bytes = b"data: [DONE]\n\n" if client_family == "openai" else b""
                        if done_bytes:
                            openai_done_sent = True
                        if perf_capture and convert_start is not None:
                            convert_time += time.perf_counter() - convert_start
                        return [error_bytes, done_bytes]

                    if perf_capture and convert_start is not None:
                        convert_time += time.perf_counter() - convert_start

                    skip_next_blank_line = True
                    out: list[bytes] = []

                    for evt in converted_events:
                        # 记录转换后的数据到 parsed_chunks（这是客户端实际收到的格式）
                        if isinstance(evt, dict):
                            ctx.data_count += 1
                            if ctx.record_parsed_chunks:
                                ctx.parsed_chunks.append(evt)
                            event_type = evt.get("type", "")
                            if event_type in ("message_stop", "response.completed"):
                                ctx.has_completion = True
                            elif "choices" in evt:
                                choices = evt.get("choices", [])
                                for choice in choices:
                                    if isinstance(choice, dict) and choice.get("finish_reason"):
                                        ctx.has_completion = True
                                        break

                            # 从转换后的事件中补充 usage 信息
                            self._extract_usage_from_converted_event(ctx, evt, event_type)

                        # 根据客户端格式生成 SSE 事件
                        out.append(
                            _format_sse_event(evt)
                            if isinstance(evt, dict)
                            else f"data: {json.dumps(evt, ensure_ascii=False)}\n\n".encode()
                        )
                    return out

                # 统一处理 prefetched + iterator
                if prefetched_chunks:
                    for chunk in prefetched_chunks:
                        buffer += chunk
                        ensure_stream_buffer_limit(
                            buffer,
                            request_id=self.request_id,
                            provider_name=ctx.provider_name,
                        )
                        while b"\n" in buffer:
                            line_bytes, buffer = buffer.split(b"\n", 1)
                            try:
                                line = decoder.decode(line_bytes + b"\n", False)
                            except Exception as e:
                                logger.warning(
                                    f"[{self.request_id}] UTF-8 解码失败: {e}, bytes={line_bytes[:50]!r}"
                                )
                                line = ""

                            if line:
                                # 需要格式转换时，跳过记录原始数据（由 _emit_converted_line 记录转换后的数据）
                                _process_line_with_perf(
                                    line, skip_record=True, skip_ctx_update=True
                                )
                            normalized_line = line.rstrip("\r\n") if line else ""
                            out_chunks = _emit_converted_line(normalized_line)
                            if not out_chunks:
                                empty_yield_count += 1
                                if empty_yield_count == StreamDefaults.MAX_EMPTY_YIELDS_WARNING:
                                    logger.warning(
                                        f"[{self.request_id}] 流式转换连续 {empty_yield_count} 次空产出"
                                    )
                            else:
                                empty_yield_count = 0
                            for out in out_chunks:
                                if not out:
                                    continue
                                _mark_stream_started()
                                yield out
                                # 转换失败：已输出 error（可能还包含 done），直接终止
                                if ctx.error_message == "format_conversion_failed":
                                    return

                async for chunk in byte_iterator:
                    buffer += chunk
                    ensure_stream_buffer_limit(
                        buffer,
                        request_id=self.request_id,
                        provider_name=ctx.provider_name,
                    )
                    while b"\n" in buffer:
                        line_bytes, buffer = buffer.split(b"\n", 1)
                        try:
                            line = decoder.decode(line_bytes + b"\n", False)
                        except Exception as e:
                            logger.warning(
                                f"[{self.request_id}] UTF-8 解码失败: {e}, bytes={line_bytes[:50]!r}"
                            )
                            line = ""

                        if line:
                            # 需要格式转换时，跳过记录原始数据（由 _emit_converted_line 记录转换后的数据）
                            _process_line_with_perf(line, skip_record=True)
                        normalized_line = line.rstrip("\r\n") if line else ""
                        out_chunks = _emit_converted_line(normalized_line)
                        if not out_chunks:
                            empty_yield_count += 1
                            if empty_yield_count == StreamDefaults.MAX_EMPTY_YIELDS_WARNING:
                                logger.warning(
                                    f"[{self.request_id}] 流式转换连续 {empty_yield_count} 次空产出"
                                )
                        else:
                            empty_yield_count = 0
                        for out in out_chunks:
                            if not out:
                                continue
                            _mark_stream_started()
                            yield out
                            if ctx.error_message == "format_conversion_failed":
                                return

                # 处理剩余缓冲区（needs_conversion 分支内，可复用 _emit_converted_line）
                if buffer:
                    try:
                        line = decoder.decode(buffer, True)
                    except Exception as e:
                        logger.warning(
                            f"[{self.request_id}] 处理剩余缓冲区失败: {e}, bytes={buffer[:50]!r}"
                        )
                        line = ""
                    buffer = b""  # 标记已消费，避免 finally 中重复处理
                    if line:
                        # 需要格式转换时，跳过记录原始数据
                        _process_line_with_perf(line, skip_record=True)
                        normalized_line = line.rstrip("\r\n")
                        out_chunks = _emit_converted_line(normalized_line)
                        for out in out_chunks:
                            if out:
                                _mark_stream_started()
                                yield out
                                # 转换失败：已输出 error，直接终止
                                if ctx.error_message == "format_conversion_failed":
                                    return

                # Provider 流结束后，为 OpenAI 客户端补齐 [DONE]（许多上游不发送该哨兵）
                if client_family == "openai" and not openai_done_sent:
                    _mark_stream_started()
                    yield b"data: [DONE]\n\n"

            else:
                if prefetched_chunks:
                    for chunk in prefetched_chunks:
                        _mark_stream_started()
                        yield chunk

                        buffer += chunk
                        ensure_stream_buffer_limit(
                            buffer,
                            request_id=self.request_id,
                            provider_name=ctx.provider_name,
                        )
                        # 处理缓冲区中的完整行
                        while b"\n" in buffer:
                            line_bytes, buffer = buffer.split(b"\n", 1)
                            try:
                                # 使用增量解码器，可以正确处理跨 chunk 的多字节字符
                                line = decoder.decode(line_bytes + b"\n", False)
                                _process_line_with_perf(line)
                            except Exception as e:
                                # 解码失败，记录警告但继续处理
                                logger.warning(
                                    f"[{self.request_id}] UTF-8 解码失败: {e}, "
                                    f"bytes={line_bytes[:50]!r}"
                                )
                                continue

            # 处理剩余的流数据
            if not needs_conversion:
                async for chunk in byte_iterator:
                    _mark_stream_started()

                    # 原始数据透传
                    yield chunk

                    buffer += chunk
                    ensure_stream_buffer_limit(
                        buffer,
                        request_id=self.request_id,
                        provider_name=ctx.provider_name,
                    )
                    # 处理缓冲区中的完整行
                    while b"\n" in buffer:
                        line_bytes, buffer = buffer.split(b"\n", 1)
                        try:
                            # 使用增量解码器，可以正确处理跨 chunk 的多字节字符
                            line = decoder.decode(line_bytes + b"\n", False)
                            _process_line_with_perf(line)
                        except Exception as e:
                            # 解码失败，记录警告但继续处理
                            logger.warning(
                                f"[{self.request_id}] UTF-8 解码失败: {e}, "
                                f"bytes={line_bytes[:50]!r}"
                            )
                            continue

            # 处理剩余的缓冲区数据（仅非转换分支，转换分支已在内部处理）
            if not needs_conversion and buffer:
                try:
                    # 使用 final=True 处理最后的不完整字符
                    line = decoder.decode(buffer, True)
                    _process_line_with_perf(line)
                except Exception as e:
                    logger.warning(
                        f"[{self.request_id}] 处理剩余缓冲区失败: {e}, bytes={buffer[:50]!r}"
                    )
                buffer = b""  # 标记已消费，避免下方重复处理

            # flush 残留的字节 buffer（异常中断时 buffer 可能仍有未解析的数据，
            # 如包含 usage 的 message_delta/response.completed 事件）
            # 正常结束时 buffer 已在上方被消费为空，此处为 no-op
            if buffer:
                try:
                    remaining = decoder.decode(buffer, True)
                    for line in remaining.split("\n"):
                        stripped = line.rstrip("\r\n")
                        if stripped:
                            events = sse_parser.feed_line(stripped)
                            for event in events:
                                self.handle_sse_event(
                                    ctx, event.get("event"), event.get("data") or ""
                                )
                except Exception:
                    pass  # best-effort: 不应因 flush 失败影响后续流程

            # flush SSE parser 内部累积的未完成事件
            for event in sse_parser.flush():
                self.handle_sse_event(ctx, event.get("event"), event.get("data") or "")

        except GeneratorExit:
            raise
        except (httpx.StreamClosed, httpx.HTTPError) as exc:
            # 记录上游流中断的详细诊断信息
            elapsed = time.monotonic() - start_time if start_time else 0
            exc_chain = []
            seen: set[int] = set()
            cause: BaseException | None = exc
            while cause is not None and id(cause) not in seen:
                seen.add(id(cause))
                exc_chain.append(f"{type(cause).__name__}: {cause}")
                cause = cause.__cause__ or cause.__context__
            logger.warning(
                "[{}] upstream stream error: provider={}, model={}, "
                "yielded_any={}, has_completion={}, elapsed={:.1f}s, "
                "input_tokens={}, output_tokens={}, "
                "exception_chain=[{}]",
                self.request_id,
                ctx.provider_name,
                ctx.model,
                yielded_any,
                ctx.has_completion,
                elapsed,
                ctx.input_tokens,
                ctx.output_tokens,
                " -> ".join(exc_chain),
            )

            # 连接关闭/协议错误：best-effort flush 残留 SSE，避免丢失尾部 usage。
            try:
                if buffer:
                    remaining = decoder.decode(buffer, True)
                    buffer = b""
                    for line in remaining.split("\n"):
                        self._process_line(ctx, sse_parser, line, skip_record=needs_conversion)

                # flush SSE parser 内部累积的未完成事件
                for event in sse_parser.flush():
                    self.handle_sse_event(
                        ctx,
                        event.get("event"),
                        event.get("data") or "",
                        skip_record=needs_conversion,
                    )
            except Exception:
                # best-effort: 不应因 flush 失败影响后续流程
                pass

            # 若尚未向客户端输出任何数据，抛出异常以触发上层 failover。
            if not yielded_any:
                raise

            # 已输出过数据：不要继续抛异常（否则 StreamingResponse 背景任务不会执行，
            # usage/telemetry 可能无法落库）。标记为上游错误并结束流。
            if not ctx.has_completion:
                ctx.status_code = 502
                ctx.error_message = f"upstream_stream_error:{type(exc).__name__}"
            return
        finally:
            if metrics_enabled:
                labels = {
                    "format": client_family or "unknown",
                    "provider": str(ctx.provider_name or "unknown"),
                    "conversion": "true" if ctx.needs_conversion else "false",
                }
                if parse_time > 0:
                    PerfRecorder.record_timing("stream_parse", parse_time, labels=labels)
                if convert_time > 0:
                    PerfRecorder.record_timing("stream_conversion", convert_time, labels=labels)
                if ctx.chunk_count:
                    PerfRecorder.record_counter(
                        "stream_chunks_total", ctx.chunk_count, labels=labels
                    )
                if ctx.data_count:
                    PerfRecorder.record_counter(
                        "stream_data_events_total", ctx.data_count, labels=labels
                    )
            if ctx.perf_sampled:
                if parse_time > 0:
                    ctx.perf_metrics["stream_parse_ms"] = int(parse_time * 1000)
                if convert_time > 0:
                    ctx.perf_metrics["stream_conversion_ms"] = int(convert_time * 1000)
                if ctx.chunk_count:
                    ctx.perf_metrics["stream_chunks"] = int(ctx.chunk_count)
                if ctx.data_count:
                    ctx.perf_metrics["stream_data_events"] = int(ctx.data_count)
            await self._cleanup(response_ctx)

    def _process_line(
        self,
        ctx: StreamContext,
        sse_parser: SSEEventParser,
        line: str,
        *,
        skip_record: bool = False,
        skip_ctx_update: bool = False,
    ) -> None:
        """
        处理单行数据

        Args:
            ctx: 流式上下文
            sse_parser: SSE 解析器
            line: 原始行数据
            skip_record: 是否跳过记录到 parsed_chunks（当需要格式转换时应为 True）
            skip_ctx_update: 跳过 usage/completion/text 提取（由调用方统一处理时使用）
        """
        # SSEEventParser 以"去掉换行符"的单行文本作为输入；这里统一剔除 CR/LF，
        # 避免把空行误判成 "\n" 并导致事件边界解析错误。
        normalized_line = line.rstrip("\r\n")
        events = sse_parser.feed_line(normalized_line)

        if normalized_line != "":
            ctx.chunk_count += 1

        for event in events:
            self.handle_sse_event(
                ctx,
                event.get("event"),
                event.get("data") or "",
                skip_record=skip_record,
                skip_ctx_update=skip_ctx_update,
            )

    async def create_monitored_stream(
        self,
        ctx: StreamContext,
        stream_generator: AsyncGenerator[bytes],
        is_disconnected: Callable[[], Any],
    ) -> AsyncGenerator[bytes]:
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
            # 使用后台任务检查断连，完全不阻塞流式传输
            disconnected = False

            async def check_disconnect_background() -> None:
                nonlocal disconnected
                while not disconnected and not ctx.has_completion:
                    await asyncio.sleep(0.5)
                    if await is_disconnected():
                        disconnected = True
                        break

            # 启动后台检查任务
            check_task = asyncio.create_task(check_disconnect_background())

            try:
                async for chunk in stream_generator:
                    if disconnected:
                        # 如果响应已完成，客户端断开不算失败
                        if ctx.has_completion:
                            logger.info(
                                f"ID:{self.request_id} | Client disconnected after completion"
                            )
                        else:
                            logger.warning(f"ID:{self.request_id} | Client disconnected")
                            ctx.status_code = 499
                            ctx.error_message = "client_disconnected"
                        break
                    yield chunk
            finally:
                check_task.cancel()
                try:
                    await check_task
                except asyncio.CancelledError:
                    pass
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

    async def create_smoothed_stream(
        self,
        stream_generator: AsyncGenerator[bytes],
        *,
        provider_name: str | None = None,
    ) -> AsyncGenerator[bytes]:
        """
        创建平滑输出的流生成器

        如果启用了平滑输出，将大 chunk 拆分成小块并添加微小延迟。
        否则直接透传原始流。

        Args:
            stream_generator: 原始流生成器

        Yields:
            平滑处理后的响应数据块
        """
        if not self.smoothing_config.enabled:
            # 未启用平滑输出，直接透传
            async for chunk in stream_generator:
                yield chunk
            return

        # 启用平滑输出
        buffer = b""
        is_first_content = True

        async for chunk in stream_generator:
            buffer += chunk
            ensure_stream_buffer_limit(
                buffer,
                request_id=self.request_id,
                provider_name=provider_name,
            )

            # 按双换行分割 SSE 事件（标准 SSE 格式）
            while b"\n\n" in buffer:
                event_block, buffer = buffer.split(b"\n\n", 1)
                event_str = event_block.decode("utf-8", errors="replace")

                # 解析事件块
                lines = event_str.strip().split("\n")
                data_str = None
                event_type = ""

                for line in lines:
                    line = line.rstrip("\r")
                    if line.startswith("event: "):
                        event_type = line[7:].strip()
                    elif line.startswith("data: "):
                        data_str = line[6:]

                # 没有 data 行，直接透传
                if data_str is None:
                    yield event_block + b"\n\n"
                    continue

                # [DONE] 直接透传
                if data_str.strip() == "[DONE]":
                    yield event_block + b"\n\n"
                    continue

                # 尝试解析 JSON
                try:
                    data = json.loads(data_str)
                except json.JSONDecodeError:
                    yield event_block + b"\n\n"
                    continue

                # 检测格式并提取内容
                content, extractor = self._detect_format_and_extract(data)

                # 只有内容长度大于 1 才需要平滑处理
                if content and len(content) > 1 and extractor:
                    # 获取配置的延迟
                    delay_seconds = self._calculate_delay()

                    # 拆分内容
                    content_chunks = self._split_content(content)

                    for i, sub_content in enumerate(content_chunks):
                        is_first = is_first_content and i == 0

                        # 使用提取器创建新 chunk
                        sse_chunk = extractor.create_chunk(
                            data,
                            sub_content,
                            event_type=event_type,
                            is_first=is_first,
                        )

                        yield sse_chunk

                        # 除了最后一个块，其他块之间加延迟
                        if i < len(content_chunks) - 1:
                            await asyncio.sleep(delay_seconds)

                    is_first_content = False
                else:
                    # 不需要拆分，直接透传
                    yield event_block + b"\n\n"
                    if content:
                        is_first_content = False

        # 处理剩余数据
        if buffer:
            yield buffer

    def _get_extractor(self, format_name: str) -> ContentExtractor | None:
        """获取或创建格式对应的提取器（带缓存）"""
        if format_name not in self._extractors:
            extractor = get_extractor(format_name)
            if extractor:
                self._extractors[format_name] = extractor
        return self._extractors.get(format_name)

    def _detect_format_and_extract(self, data: dict) -> tuple[str | None, ContentExtractor | None]:
        """
        检测数据格式并提取内容

        依次尝试各格式的提取器，返回第一个成功提取内容的结果。

        Returns:
            (content, extractor): 提取的内容和对应的提取器
        """
        for format_name in get_extractor_formats():
            extractor = self._get_extractor(format_name)
            if extractor:
                content = extractor.extract_content(data)
                if content is not None:
                    return content, extractor

        return None, None

    def _calculate_delay(self) -> float:
        """获取配置的延迟（秒）"""
        return self.smoothing_config.delay_ms / 1000.0

    def _split_content(self, content: str) -> list[str]:
        """
        按块拆分文本
        """
        chunk_size = self.smoothing_config.chunk_size
        text_length = len(content)

        if text_length <= chunk_size:
            return [content]

        # 按块拆分
        chunks = []
        for i in range(0, text_length, chunk_size):
            chunks.append(content[i : i + chunk_size])
        return chunks

    async def _cleanup(
        self,
        response_ctx: Any,
    ) -> None:
        """清理响应上下文（不关闭池中复用的客户端）"""
        try:
            await response_ctx.__aexit__(None, None, None)
        except Exception:
            pass


async def create_smoothed_stream(
    stream_generator: AsyncGenerator[bytes],
    chunk_size: int = 20,
    delay_ms: int = 8,
    *,
    request_id: str | None = None,
    provider_name: str | None = None,
) -> AsyncGenerator[bytes]:
    """
    独立的平滑流生成函数

    供 CLI handler 等场景使用，无需创建完整的 StreamProcessor 实例。

    Args:
        stream_generator: 原始流生成器
        chunk_size: 每块字符数
        delay_ms: 每块之间的延迟毫秒数

    Yields:
        平滑处理后的响应数据块
    """
    processor = _LightweightSmoother(
        chunk_size=chunk_size,
        delay_ms=delay_ms,
        request_id=request_id,
        provider_name=provider_name,
    )
    async for chunk in processor.smooth(stream_generator):
        yield chunk


class _LightweightSmoother:
    """
    轻量级平滑处理器

    只包含平滑输出所需的最小逻辑，不依赖 StreamProcessor 的其他功能。
    """

    def __init__(
        self,
        chunk_size: int = 20,
        delay_ms: int = 8,
        request_id: str | None = None,
        provider_name: str | None = None,
    ) -> None:
        self.chunk_size = chunk_size
        self.delay_ms = delay_ms
        self.request_id = request_id
        self.provider_name = provider_name
        self._extractors: dict[str, ContentExtractor] = {}

    def _get_extractor(self, format_name: str) -> ContentExtractor | None:
        if format_name not in self._extractors:
            extractor = get_extractor(format_name)
            if extractor:
                self._extractors[format_name] = extractor
        return self._extractors.get(format_name)

    def _detect_format_and_extract(self, data: dict) -> tuple[str | None, ContentExtractor | None]:
        for format_name in get_extractor_formats():
            extractor = self._get_extractor(format_name)
            if extractor:
                content = extractor.extract_content(data)
                if content is not None:
                    return content, extractor
        return None, None

    def _calculate_delay(self) -> float:
        return self.delay_ms / 1000.0

    def _split_content(self, content: str) -> list[str]:
        text_length = len(content)
        if text_length <= self.chunk_size:
            return [content]
        return [content[i : i + self.chunk_size] for i in range(0, text_length, self.chunk_size)]

    async def smooth(self, stream_generator: AsyncGenerator[bytes]) -> AsyncGenerator[bytes]:
        buffer = b""
        is_first_content = True

        async for chunk in stream_generator:
            buffer += chunk
            ensure_stream_buffer_limit(
                buffer,
                request_id=self.request_id or "unknown",
                provider_name=self.provider_name,
            )

            while b"\n\n" in buffer:
                event_block, buffer = buffer.split(b"\n\n", 1)
                event_str = event_block.decode("utf-8", errors="replace")

                lines = event_str.strip().split("\n")
                data_str = None
                event_type = ""

                for line in lines:
                    line = line.rstrip("\r")
                    if line.startswith("event: "):
                        event_type = line[7:].strip()
                    elif line.startswith("data: "):
                        data_str = line[6:]

                if data_str is None:
                    yield event_block + b"\n\n"
                    continue

                if data_str.strip() == "[DONE]":
                    yield event_block + b"\n\n"
                    continue

                try:
                    data = json.loads(data_str)
                except json.JSONDecodeError:
                    yield event_block + b"\n\n"
                    continue

                content, extractor = self._detect_format_and_extract(data)

                if content and len(content) > 1 and extractor:
                    delay_seconds = self._calculate_delay()
                    content_chunks = self._split_content(content)

                    for i, sub_content in enumerate(content_chunks):
                        is_first = is_first_content and i == 0
                        sse_chunk = extractor.create_chunk(
                            data, sub_content, event_type=event_type, is_first=is_first
                        )
                        yield sse_chunk
                        if i < len(content_chunks) - 1:
                            await asyncio.sleep(delay_seconds)

                    is_first_content = False
                else:
                    yield event_block + b"\n\n"
                    if content:
                        is_first_content = False

        if buffer:
            yield buffer
