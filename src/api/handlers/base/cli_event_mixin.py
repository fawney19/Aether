"""CLI Handler - SSE 事件处理 + 格式转换 Mixin"""

from __future__ import annotations

import codecs
import json
from collections.abc import Iterator
from typing import TYPE_CHECKING, Any

from src.api.handlers.base.parsers import get_parser_for_format
from src.api.handlers.base.stream_context import StreamContext
from src.api.handlers.base.utils import get_format_converter_registry
from src.core.logger import logger
from src.core.usage_tokens import extract_cache_creation_tokens, extract_cache_read_tokens
from src.services.provider.behavior import get_provider_behavior
from src.utils.sse_parser import SSEEventParser

from .cli_sse_helpers import (
    _format_converted_events_to_sse,
    _parse_gemini_json_array_line,
    _parse_sse_data_line,
    _parse_sse_event_data_line,
)

if TYPE_CHECKING:
    from src.api.handlers.base.cli_protocol import CliHandlerProtocol


class CliEventMixin:
    """SSE 事件处理和格式转换相关方法的 Mixin"""

    def _handle_sse_event(
        self: CliHandlerProtocol,
        ctx: StreamContext,
        event_name: str | None,
        data_str: str,
        record_chunk: bool = False,
    ) -> None:
        """
        处理 SSE 事件

        通用框架：解析 JSON、更新计数器
        子类可覆盖 _process_event_data() 实现格式特定逻辑

        Args:
            ctx: 流上下文
            event_name: 事件名称（如 message_start, content_block_delta 等）
            data_str: 事件数据字符串（JSON 格式）
            record_chunk: 是否记录到 parsed_chunks（不需要格式转换时应为 True）
                          当为 True 时，同时更新 data_count；
                          当为 False 时，data_count 由 _record_converted_chunks 更新
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

        behavior = get_provider_behavior(
            provider_type=ctx.provider_type,
            endpoint_sig=ctx.provider_api_format,
        )
        envelope = behavior.envelope
        if envelope:
            data = envelope.unwrap_response(data)
            if not isinstance(data, dict):
                return

        # 当不需要格式转换时，更新 data_count；需要记录时再写入 parsed_chunks。
        # 当需要格式转换时（record_chunk=False），data_count 由 _record_converted_chunks 更新
        if record_chunk:
            ctx.data_count += 1
            if ctx.record_parsed_chunks:
                ctx.parsed_chunks.append(data)
        else:
            # 格式转换场景：保留提供商原始数据
            if ctx.record_parsed_chunks:
                ctx.provider_parsed_chunks.append(data)

        event_type = event_name or data.get("type", "")

        if envelope:
            envelope.postprocess_unwrapped_response(model=ctx.model, data=data)

        # 调用格式特定的处理逻辑
        # 注意：跨格式转换时，_process_event_data 会自动选择正确的 Provider 解析器
        self._process_event_data(ctx, event_type, data)

    def _process_event_data(
        self,
        ctx: StreamContext,
        event_type: str,
        data: dict[str, Any],
    ) -> None:
        """
        处理解析后的事件数据 - 子类应覆盖此方法

        默认实现使用 ResponseParser 提取 usage
        """
        # 提取 response_id
        if not ctx.response_id:
            response_obj = data.get("response")
            if isinstance(response_obj, dict) and response_obj.get("id"):
                ctx.response_id = response_obj["id"]
            elif "id" in data:
                ctx.response_id = data["id"]

        # 使用解析器提取 usage
        # Claude/CLI 流式响应的 usage 可能在首个 chunk 或最后一个 chunk 中
        # 首个 chunk 可能部分为 0，最后一个 chunk 包含完整值，因此取最大值确保正确计费
        #
        # 重要：当跨格式转换时，收到的数据是 Provider 格式，需要使用 Provider 格式的解析器
        # 而不是客户端格式的解析器（self.parser）
        parser = self.parser
        if ctx.provider_api_format and ctx.provider_api_format != ctx.client_api_format:
            # 跨格式转换：使用 Provider 格式的解析器
            try:
                provider_parser = get_parser_for_format(ctx.provider_api_format)
                if provider_parser:
                    parser = provider_parser
            except KeyError:
                logger.debug(
                    f"[{getattr(ctx, 'request_id', 'unknown')}] 未找到 Provider 格式解析器: "
                    f"{ctx.provider_api_format}, 回退使用客户端格式解析器"
                )

        usage = parser.extract_usage_from_response(data)
        if usage:
            new_input = usage.get("input_tokens", 0)
            new_output = usage.get("output_tokens", 0)
            new_cached = usage.get("cache_read_tokens", 0)
            new_cache_creation = usage.get("cache_creation_tokens", 0)

            # 取最大值更新
            if new_input > ctx.input_tokens:
                ctx.input_tokens = new_input
            if new_output > ctx.output_tokens:
                ctx.output_tokens = new_output
            if new_cached > ctx.cached_tokens:
                ctx.cached_tokens = new_cached
            if new_cache_creation > ctx.cache_creation_tokens:
                ctx.cache_creation_tokens = new_cache_creation

            # 保存最后一个非空 usage 作为 final_usage
            if any([new_input, new_output, new_cached, new_cache_creation]):
                ctx.final_usage = usage

        # 提取文本内容（同样使用正确的解析器）
        text = parser.extract_text_content(data)
        if text:
            ctx.append_text(text)

        # 检查完成事件
        if event_type in ("response.completed", "message_stop"):
            ctx.has_completion = True
            response_obj = data.get("response")
            if isinstance(response_obj, dict):
                ctx.final_response = response_obj

    def _record_converted_chunks(
        self,
        ctx: StreamContext,
        converted_events: list[dict[str, Any]],
    ) -> None:
        """
        记录转换后的 chunk 数据到 parsed_chunks，并更新统计信息

        当需要格式转换时，记录的是转换后的数据（客户端实际收到的格式）；
        同时更新 data_count、has_completion 等统计信息。

        重要：此方法也从转换后的事件中提取 usage 信息，作为 _process_event_data
        从原始数据提取的补充。这确保即使原始 Provider 数据中没有 usage（如 OpenAI
        未设置 stream_options），也能从转换后的格式中获取。

        Args:
            ctx: 流上下文
            converted_events: 转换后的事件列表
        """
        for evt in converted_events:
            if isinstance(evt, dict):
                ctx.data_count += 1
                if ctx.record_parsed_chunks:
                    ctx.parsed_chunks.append(evt)

                # 检测完成事件（根据客户端格式判断）
                # OpenAI 格式: choices[].finish_reason
                # Claude 格式: type == "message_stop" 或 stop_reason
                event_type = evt.get("type", "")
                if event_type == "message_stop":
                    ctx.has_completion = True
                elif event_type == "response.completed":
                    ctx.has_completion = True
                elif "choices" in evt:
                    choices = evt.get("choices", [])
                    for choice in choices:
                        if isinstance(choice, dict) and choice.get("finish_reason"):
                            ctx.has_completion = True
                            break

                # 从转换后的事件中提取 usage（补充 _process_event_data 的提取）
                # Claude 格式: message_delta.usage 或 message_start.message.usage
                # OpenAI 格式: chunk.usage
                self._extract_usage_from_converted_event(ctx, evt, event_type)

    def _extract_usage_from_converted_event(
        self,
        ctx: StreamContext,
        evt: dict[str, Any],
        event_type: str,
    ) -> None:
        """
        从转换后的事件中提取 usage 信息

        支持多种格式：
        - Claude: message_delta.usage, message_start.message.usage
        - OpenAI: chunk.usage
        - Gemini: usageMetadata

        Args:
            ctx: 流上下文
            evt: 转换后的事件
            event_type: 事件类型
        """
        usage: dict[str, Any] | None = None

        # Claude 格式: message_delta 或 message_start
        if event_type == "message_delta":
            usage = evt.get("usage")
        elif event_type == "message_start":
            message = evt.get("message", {})
            if isinstance(message, dict):
                usage = message.get("usage")
        # OpenAI Responses API (openai:cli) 格式: response.completed 中 usage 嵌套在 response 对象内
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
                    "cache_creation_tokens": 0,  # Gemini 目前不支持缓存创建
                }

        if usage and isinstance(usage, dict):
            new_input = usage.get("input_tokens") or usage.get("prompt_tokens") or 0
            new_output = usage.get("output_tokens") or usage.get("completion_tokens") or 0
            new_cached = extract_cache_read_tokens(usage)
            new_cache_creation = extract_cache_creation_tokens(usage)

            # 取最大值更新（与 _process_event_data 相同的策略）
            if new_input > ctx.input_tokens:
                ctx.input_tokens = new_input
                logger.debug("[{}] 从转换后事件更新 input_tokens: {}", ctx.request_id, new_input)
            if new_output > ctx.output_tokens:
                ctx.output_tokens = new_output
                logger.debug("[{}] 从转换后事件更新 output_tokens: {}", ctx.request_id, new_output)
            if new_cached > ctx.cached_tokens:
                ctx.cached_tokens = new_cached
            if new_cache_creation > ctx.cache_creation_tokens:
                ctx.cache_creation_tokens = new_cache_creation

            # 保存最后一个非空 usage
            if any([new_input, new_output, new_cached, new_cache_creation]):
                ctx.final_usage = usage

    def _flush_buffer_with_conversion(
        self: CliHandlerProtocol,
        ctx: StreamContext,
        buffer: bytes,
        decoder: codecs.IncrementalDecoder,
        sse_parser: SSEEventParser,
        needs_conversion: bool,
    ) -> Iterator[bytes]:
        """flush 字节 buffer 残余数据 + SSE parser 内部缓冲区，并做格式转换。

        正常流结束时调用（区别于异常路径的 _flush_remaining_sse_data）。
        当 needs_conversion=True 时 yield 转换后的 SSE 行；
        当 needs_conversion=False 时 yield 原始行透传。
        """
        # 1) flush 字节 buffer 中的残余数据（最后一个 chunk 可能不以换行结尾）
        if buffer:
            try:
                remaining = decoder.decode(buffer, True)
            except Exception:
                remaining = ""
            for tail_line in remaining.split("\n"):
                stripped = tail_line.rstrip("\r")
                tail_events = sse_parser.feed_line(stripped)
                if stripped == "":
                    for event in tail_events:
                        self._handle_sse_event(
                            ctx,
                            event.get("event"),
                            event.get("data") or "",
                            record_chunk=not needs_conversion,
                        )
                    continue
                if needs_conversion and stripped:
                    converted_lines, converted_events = self._convert_sse_line(
                        ctx, stripped, tail_events
                    )
                    self._record_converted_chunks(ctx, converted_events)
                    for converted_line in converted_lines:
                        if converted_line:
                            yield (converted_line + "\n").encode("utf-8")
                elif stripped:
                    # 非 conversion 模式：透传原始行
                    yield (stripped + "\n").encode("utf-8")

        # 2) flush SSE parser 内部缓冲区中的残余事件
        for event in sse_parser.flush():
            self._handle_sse_event(
                ctx,
                event.get("event"),
                event.get("data") or "",
                record_chunk=not needs_conversion,
            )
            if needs_conversion:
                data_str = event.get("data") or ""
                if data_str and data_str != "[DONE]":
                    flush_line = f"data: {data_str}"
                    converted_lines, converted_events = self._convert_sse_line(ctx, flush_line, [])
                    self._record_converted_chunks(ctx, converted_events)
                    for converted_line in converted_lines:
                        if converted_line:
                            yield (converted_line + "\n").encode("utf-8")

    def _finalize_stream_metadata(self, ctx: StreamContext) -> None:
        """
        在记录统计前从 parsed_chunks 中提取额外的元数据 - 子类可覆盖

        这是一个后处理钩子，在流传输完成后、记录 Usage 之前调用。
        子类可以覆盖此方法从 ctx.parsed_chunks 中提取格式特定的元数据，
        如 Gemini 的 modelVersion、token 统计等。

        Args:
            ctx: 流上下文，包含 parsed_chunks 和 response_metadata
        """
        pass

    def _needs_format_conversion(self, ctx: StreamContext) -> bool:
        """
        [已废弃] 仅根据格式差异判断是否需要转换

        警告：此方法只检查格式是否不同，不检查端点的 format_acceptance_config 配置！
        正确的判断应使用候选筛选阶段的结果（ctx.needs_conversion），该结果由
        is_format_compatible() 函数根据全局开关和端点配置计算得出。

        此方法保留仅供调试和日志输出使用，流生成器中不应调用此方法。

        当 Provider 的 API 格式与客户端请求的 API 格式不同时，需要转换响应。
        例如：客户端请求 Claude 格式，但 Provider 返回 OpenAI 格式。

        注意：
        - CLAUDE 和 CLAUDE_CLI、GEMINI 和 GEMINI_CLI：格式相同，只是认证不同，可透传
        - OPENAI 和 OPENAI_CLI：格式不同（Chat Completions vs Responses API），需要转换
        """
        from src.core.api_format.metadata import can_passthrough_endpoint
        from src.core.api_format.signature import normalize_signature_key

        if not ctx.provider_api_format or not ctx.client_api_format:
            logger.debug(
                f"[{getattr(ctx, 'request_id', 'unknown')}] _needs_format_conversion: "
                f"provider_api_format={ctx.provider_api_format!r}, client_api_format={ctx.client_api_format!r} -> False (missing)"
            )
            return False

        provider_format = normalize_signature_key(str(ctx.provider_api_format))
        client_format = normalize_signature_key(str(ctx.client_api_format))

        # 1. 格式完全匹配 -> 不需要转换
        if provider_format == client_format:
            logger.debug(
                f"[{getattr(ctx, 'request_id', 'unknown')}] _needs_format_conversion: "
                f"provider={provider_format}, client={client_format} -> False (exact match)"
            )
            return False

        # 2. 根据 data_format_id 判断是否可透传（可透传则不需要转换）
        if can_passthrough_endpoint(client_format, provider_format):
            logger.debug(
                f"[{getattr(ctx, 'request_id', 'unknown')}] _needs_format_conversion: "
                f"provider={provider_format}, client={client_format} -> False (passthroughable)"
            )
            return False

        # 3. 其他情况 -> 需要转换
        logger.debug(
            f"[{getattr(ctx, 'request_id', 'unknown')}] _needs_format_conversion: "
            f"provider={provider_format}, client={client_format} -> True"
        )
        return True

    def _mark_first_output(self, ctx: StreamContext, state: dict[str, bool]) -> None:
        """
        标记首次输出：记录 TTFB 并更新 streaming 状态

        在第一次 yield 数据前调用，确保：
        1. 首字时间 (TTFB) 已记录到 ctx
        2. Usage 状态已更新为 streaming（包含 provider/key/TTFB 信息）

        Args:
            ctx: 流上下文
            state: 包含 first_yield 和 streaming_updated 的状态字典
        """
        if state["first_yield"]:
            ctx.record_first_byte_time(self.start_time)
            state["first_yield"] = False
            if not state["streaming_updated"]:
                # 优先使用当前请求的 DB 会话同步更新，避免状态延迟或丢失
                try:
                    from src.services.usage import UsageService

                    UsageService.update_usage_status(
                        db=self.db,
                        request_id=self.request_id,
                        status="streaming",
                        provider=ctx.provider_name,
                        target_model=ctx.mapped_model,
                        provider_id=ctx.provider_id,
                        provider_endpoint_id=ctx.endpoint_id,
                        provider_api_key_id=ctx.key_id,
                        first_byte_time_ms=ctx.first_byte_time_ms,
                        api_format=ctx.api_format,
                        endpoint_api_format=ctx.provider_api_format or None,
                        has_format_conversion=ctx.has_format_conversion,
                        provider_request_headers=ctx.provider_request_headers or None,
                        provider_request_body=ctx.provider_request_body,
                    )
                except Exception as e:
                    logger.warning("[{}] 同步更新 streaming 状态失败: {}", self.request_id, e)
                    # 回退到后台任务更新
                    self._update_usage_to_streaming_with_ctx(ctx)
                state["streaming_updated"] = True

    def _convert_sse_line(
        self,
        ctx: StreamContext,
        line: str,
        events: list,  # noqa: ARG002 - 预留给上下文感知转换
    ) -> tuple[list[str], list[dict[str, Any]]]:
        """
        将 SSE 行从 Provider 格式转换为客户端格式

        Args:
            ctx: 流上下文
            line: 原始 SSE 行
            events: 当前累积的事件列表（预留参数，用于未来上下文感知转换如合并相邻事件）

        Returns:
            (sse_lines, converted_events) 元组：
            - sse_lines: 转换后的 SSE 行列表（一入多出），空列表表示跳过该行
            - converted_events: 转换后的事件对象列表（用于记录到 parsed_chunks）
        """
        # 空行直接返回
        if not line or line.strip() == "":
            return ([line] if line else [], [])

        client_format = (ctx.client_api_format or "").strip().lower()

        # [DONE] 标记处理：只有 OpenAI 客户端需要，Claude 客户端不需要
        if line == "data: [DONE]":
            if client_format.startswith("openai"):
                return [line], []
            else:
                # Claude/Gemini 客户端不需要 [DONE] 标记
                return [], []

        provider_format = (ctx.provider_api_format or "").strip().lower()

        # 过滤上游控制行（id/retry），避免与目标格式混淆
        if line.startswith(("id:", "retry:")):
            return [], []

        # 解析 SSE 行为 JSON 对象
        data_obj, status = self._parse_sse_line_to_json(line, provider_format)

        # 根据解析状态决定行为
        if status == "empty" or status == "skip":
            return [], []
        if status == "invalid" or status == "passthrough":
            return [line], []

        behavior = get_provider_behavior(
            provider_type=ctx.provider_type,
            endpoint_sig=ctx.provider_api_format,
        )
        envelope = behavior.envelope
        if envelope:
            data_obj = envelope.unwrap_response(data_obj)
            envelope.postprocess_unwrapped_response(model=ctx.model, data=data_obj)

        # 初始化流式转换状态
        if ctx.stream_conversion_state is None:
            from src.core.api_format.conversion.stream_state import StreamState

            # 使用客户端请求的模型（ctx.model），而非映射后的上游模型（ctx.mapped_model）
            init_model = ctx.model or ""
            logger.debug(
                f"[{ctx.request_id}] StreamState init: ctx.model={ctx.model!r}, "
                f"mapped_model={ctx.mapped_model!r}, using={init_model!r}"
            )
            ctx.stream_conversion_state = StreamState(
                model=init_model,
                message_id=ctx.response_id or ctx.request_id or "",
            )

        # 执行格式转换
        try:
            registry = get_format_converter_registry()
            # status == "ok" 时 data_obj 必定是有效的 dict（防御性检查）
            if data_obj is None:
                return [], []
            converted_events = registry.convert_stream_chunk(
                data_obj,
                provider_format,
                client_format,
                state=ctx.stream_conversion_state,
            )
            result = _format_converted_events_to_sse(converted_events, client_format)
            if result:
                ctx.stream_conversion_event_count += len(converted_events)
            return result, converted_events

        except Exception as e:
            logger.warning("格式转换失败，透传原始数据: {}", e)
            return [line], []

    def _parse_sse_line_to_json(self, line: str, provider_format: str) -> tuple[Any | None, str]:
        """
        解析 SSE 行为 JSON 对象

        支持多种格式：
        - 标准 SSE: "data: {...}"
        - event+data 同行: "event: xxx data: {...}"
        - Gemini JSON-array: 裸 JSON 行

        Args:
            line: 原始 SSE 行
            provider_format: Provider API 格式

        Returns:
            (parsed_json, status) 元组：
            - (obj, "ok") - 解析成功
            - (None, "empty") - 内容为空，应跳过
            - (None, "invalid") - JSON 解析失败，应透传原始行
            - (None, "skip") - 应跳过（如纯 event 行）
            - (None, "passthrough") - 无法识别，应透传原始行
        """
        # 标准 SSE: data: {...}
        if line.startswith("data:"):
            return _parse_sse_data_line(line)

        # event + data 同行: event: xxx data: {...}
        if line.startswith("event:") and " data:" in line:
            return _parse_sse_event_data_line(line)

        # 纯 event 行不参与转换
        if line.startswith("event:"):
            return None, "skip"

        # Gemini JSON-array 格式
        if provider_format.startswith("gemini"):
            return _parse_gemini_json_array_line(line)

        # 其他格式：无法识别，透传
        return None, "passthrough"
