"""
OpenAI Chat Completions Normalizer

负责：
- OpenAI ChatCompletions request/response <-> Internal 表示转换
- 可选：OpenAI streaming chunk <-> InternalStreamEvent
- 可选：OpenAI error <-> InternalError
"""

import json
import time
from datetime import datetime, timezone
from typing import Any

from src.core.api_format.conversion.field_mappings import (
    ERROR_TYPE_MAPPINGS,
    RETRYABLE_ERROR_TYPES,
)
from src.core.api_format.conversion.internal import (
    ContentBlock,
    ContentType,
    ErrorType,
    FormatCapabilities,
    ImageBlock,
    InstructionSegment,
    InternalError,
    InternalMessage,
    InternalRequest,
    InternalResponse,
    Role,
    StopReason,
    TextBlock,
    ThinkingBlock,
    ToolChoice,
    ToolChoiceType,
    ToolDefinition,
    ToolResultBlock,
    ToolUseBlock,
    UnknownBlock,
    UsageInfo,
)
from src.core.api_format.conversion.internal_video import (
    InternalVideoPollResult,
    InternalVideoRequest,
    InternalVideoTask,
    VideoStatus,
)
from src.core.api_format.conversion.normalizer import FormatNormalizer
from src.core.api_format.conversion.stream_events import (
    ContentBlockStartEvent,
    ContentBlockStopEvent,
    ContentDeltaEvent,
    ErrorEvent,
    InternalStreamEvent,
    MessageStartEvent,
    MessageStopEvent,
    ToolCallDeltaEvent,
)
from src.core.api_format.conversion.stream_state import StreamState
from src.core.logger import logger


class OpenAINormalizer(FormatNormalizer):
    # 新模式：ApiFamily + EndpointKind 的 signature key
    FORMAT_ID = "openai:chat"
    capabilities = FormatCapabilities(
        supports_stream=True,
        supports_error_conversion=True,
        supports_tools=True,
        supports_images=True,
    )

    # finish_reason -> StopReason
    _FINISH_REASON_TO_STOP: dict[str, StopReason] = {
        "stop": StopReason.END_TURN,
        "length": StopReason.MAX_TOKENS,
        "tool_calls": StopReason.TOOL_USE,
        "function_call": StopReason.TOOL_USE,
        "content_filter": StopReason.CONTENT_FILTERED,
    }

    # StopReason -> finish_reason
    _STOP_TO_FINISH_REASON: dict[StopReason, str] = {
        StopReason.END_TURN: "stop",
        StopReason.MAX_TOKENS: "length",
        StopReason.STOP_SEQUENCE: "stop",
        StopReason.TOOL_USE: "tool_calls",
        StopReason.CONTENT_FILTERED: "content_filter",
        StopReason.UNKNOWN: "stop",
    }

    # 视频尺寸映射: (resolution, aspect_ratio) -> size
    _VIDEO_SIZE_MAP: dict[tuple[str, str], str] = {
        ("480p", "16:9"): "854x480",
        ("480p", "9:16"): "480x854",
        ("480p", "1:1"): "480x480",
        ("720p", "16:9"): "1280x720",
        ("720p", "9:16"): "720x1280",
        ("720p", "1:1"): "720x720",
        ("1080p", "16:9"): "1920x1080",
        ("1080p", "9:16"): "1080x1920",
        ("1080p", "1:1"): "1080x1080",
    }
    # OpenAI Sora 特定尺寸（非标准分辨率，需单独处理）
    _SORA_SIZE_REVERSE: dict[str, tuple[str, str]] = {
        "1792x1024": ("1080p", "16:9"),
        "1024x1792": ("1080p", "9:16"),
    }
    _VIDEO_SIZE_REVERSE: dict[str, tuple[str, str]] = {
        **{value: key for key, value in _VIDEO_SIZE_MAP.items()},
        **_SORA_SIZE_REVERSE,
    }

    # InternalError.type -> OpenAI error.type（最佳努力）
    _ERROR_TYPE_TO_OPENAI: dict[ErrorType, str] = {
        ErrorType.INVALID_REQUEST: "invalid_request_error",
        ErrorType.AUTHENTICATION: "invalid_api_key",
        ErrorType.PERMISSION_DENIED: "invalid_request_error",
        ErrorType.NOT_FOUND: "not_found",
        ErrorType.RATE_LIMIT: "rate_limit_exceeded",
        ErrorType.OVERLOADED: "server_error",
        ErrorType.SERVER_ERROR: "server_error",
        ErrorType.CONTENT_FILTERED: "content_policy_violation",
        ErrorType.CONTEXT_LENGTH_EXCEEDED: "context_length_exceeded",
        ErrorType.UNKNOWN: "server_error",
    }

    # =========================
    # Requests
    # =========================

    def request_to_internal(self, request: dict[str, Any]) -> InternalRequest:
        model = str(request.get("model") or "")

        dropped: dict[str, int] = {}

        instructions: list[InstructionSegment] = []
        messages: list[InternalMessage] = []

        for msg in request.get("messages") or []:
            if not isinstance(msg, dict):
                dropped["openai_message_non_dict"] = dropped.get("openai_message_non_dict", 0) + 1
                continue

            role = str(msg.get("role") or "unknown")
            if role in ("system", "developer"):
                text, msg_dropped = self._collapse_openai_text(msg.get("content"))
                self._merge_dropped(dropped, msg_dropped)
                instructions.append(
                    InstructionSegment(
                        role=Role.SYSTEM if role == "system" else Role.DEVELOPER,
                        text=text,
                        extra=self._extract_extra(msg, {"role", "content"}),
                    )
                )
                continue

            internal_msg, msg_dropped = self._openai_message_to_internal(msg)
            self._merge_dropped(dropped, msg_dropped)
            if internal_msg is not None:
                messages.append(internal_msg)

        system_text = self._join_instructions(instructions)

        tools = self._openai_tools_to_internal(request.get("tools"))
        tool_choice = self._openai_tool_choice_to_internal(request.get("tool_choice"))

        stop_sequences = self._coerce_str_list(request.get("stop"))

        # 兼容新旧参数名：优先使用 max_completion_tokens，回退到 max_tokens
        mct = request.get("max_completion_tokens")
        max_tokens_value = self._optional_int(mct if mct is not None else request.get("max_tokens"))

        # 构建 extra，保留未识别字段
        extra: dict[str, Any] = {"openai": self._extract_extra(request, {"messages"})}

        # 处理 extra_body.google (用于 Gemini 特定功能透传，如 thinkingConfig, responseModalities)
        extra_body = request.get("extra_body")
        if isinstance(extra_body, dict):
            google_extra = extra_body.get("google")
            if isinstance(google_extra, dict) and google_extra:
                extra["google"] = google_extra

        internal = InternalRequest(
            model=model,
            messages=messages,
            instructions=instructions,
            system=system_text,
            max_tokens=max_tokens_value,
            temperature=self._optional_float(request.get("temperature")),
            top_p=self._optional_float(request.get("top_p")),
            stop_sequences=stop_sequences,
            stream=bool(request.get("stream") or False),
            tools=tools,
            tool_choice=tool_choice,
            extra=extra,
        )

        if dropped:
            internal.extra.setdefault("raw", {})["dropped_blocks"] = dropped

        return internal

    def request_from_internal(
        self,
        internal: InternalRequest,
        *,
        target_variant: str | None = None,
    ) -> dict[str, Any]:
        out_messages: list[dict[str, Any]] = []

        if internal.instructions:
            for seg in internal.instructions:
                role = "system" if seg.role == Role.SYSTEM else "developer"
                out_messages.append({"role": role, "content": seg.text})
        elif internal.system:
            # 兜底：没有 instructions 但有 system 字符串
            out_messages.append({"role": "system", "content": internal.system})

        for msg in internal.messages:
            out_messages.extend(self._internal_message_to_openai_messages(msg))

        result: dict[str, Any] = {
            "model": internal.model,
            "messages": out_messages,
        }

        if internal.max_tokens is not None:
            result["max_tokens"] = internal.max_tokens
        if internal.temperature is not None:
            result["temperature"] = internal.temperature
        if internal.top_p is not None:
            result["top_p"] = internal.top_p
        if internal.stop_sequences:
            # OpenAI stop 接受 str 或 list；为减少分支，统一输出 list
            result["stop"] = list(internal.stop_sequences)
        if internal.stream:
            result["stream"] = True
            # 启用流式响应的 usage 统计（OpenAI 默认不返回流式 usage）
            result["stream_options"] = {"include_usage": True}

        if internal.tools:
            result["tools"] = [
                {
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters or {},
                        **(t.extra.get("openai_function") or {}),
                    },
                    **(t.extra.get("openai_tool") or {}),
                }
                for t in internal.tools
            ]

        if internal.tool_choice:
            result["tool_choice"] = self._tool_choice_to_openai(internal.tool_choice)

        # 其余字段：保留在 internal.extra 中，但不默认回写到 OpenAI body（兼容优先）
        return result

    # =========================
    # Responses
    # =========================

    def response_to_internal(self, response: dict[str, Any]) -> InternalResponse:
        rid = str(response.get("id") or "")
        model = str(response.get("model") or "")

        extra: dict[str, Any] = {}

        choices = response.get("choices") or []
        if isinstance(choices, list) and len(choices) > 1:
            extra.setdefault("openai", {})["choices"] = choices

        choice0 = choices[0] if isinstance(choices, list) and choices else {}
        message = choice0.get("message") if isinstance(choice0, dict) else None
        message = message if isinstance(message, dict) else {}

        blocks, dropped = self._openai_content_to_blocks(message.get("content"))

        # tool_calls -> ToolUseBlock
        tool_calls = message.get("tool_calls") or []
        if isinstance(tool_calls, list):
            for tool_call in tool_calls:
                tool_block, tc_dropped = self._openai_tool_call_to_block(tool_call)
                self._merge_dropped(dropped, tc_dropped)
                if tool_block is not None:
                    blocks.append(tool_block)

        finish_reason = choice0.get("finish_reason") if isinstance(choice0, dict) else None
        stop_reason = self._FINISH_REASON_TO_STOP.get(str(finish_reason), StopReason.UNKNOWN)

        if finish_reason is not None:
            extra.setdefault("raw", {})["finish_reason"] = finish_reason

        usage = response.get("usage") if isinstance(response, dict) else None
        usage_info = self._openai_usage_to_internal(usage)

        internal = InternalResponse(
            id=rid,
            model=model,
            content=blocks,
            stop_reason=stop_reason,
            usage=usage_info,
            extra=extra,
        )

        if dropped:
            internal.extra.setdefault("raw", {})["dropped_blocks"] = dropped

        return internal

    def response_from_internal(
        self,
        internal: InternalResponse,
        *,
        requested_model: str | None = None,
    ) -> dict[str, Any]:
        # OpenAI Chat Completions response envelope
        # 优先使用用户请求的原始模型名，回退到上游返回的模型名
        model_name = requested_model if requested_model else internal.model
        out: dict[str, Any] = {
            "id": internal.id or "chatcmpl-unknown",
            "object": "chat.completion",
            "created": int(time.time()),
            "model": model_name,
            "choices": [],
        }

        message: dict[str, Any] = {"role": "assistant"}

        thinking_blocks, content_blocks, tool_blocks = self._split_blocks(internal.content)

        # 对齐 AM：ThinkingBlock → reasoning_content
        if thinking_blocks:
            reasoning_text = "".join(
                b.thinking for b in thinking_blocks if isinstance(b, ThinkingBlock) and b.thinking
            )
            if reasoning_text:
                message["reasoning_content"] = reasoning_text

        content_value = self._blocks_to_openai_content(content_blocks)
        if content_value is not None:
            message["content"] = content_value

        if tool_blocks:
            message["tool_calls"] = [
                self._tool_use_block_to_openai_call(b, idx) for idx, b in enumerate(tool_blocks)
            ]

        finish_reason = None
        if internal.stop_reason is not None:
            finish_reason = self._STOP_TO_FINISH_REASON.get(internal.stop_reason, "stop")

        out["choices"].append(
            {
                "index": 0,
                "message": message,
                "finish_reason": finish_reason,
            }
        )

        if internal.usage:
            total = internal.usage.total_tokens or (
                internal.usage.input_tokens + internal.usage.output_tokens
            )
            out["usage"] = {
                "prompt_tokens": int(internal.usage.input_tokens),
                "completion_tokens": int(internal.usage.output_tokens),
                "total_tokens": int(total),
            }

        return out

    # =========================
    # Streaming
    # =========================

    def stream_chunk_to_internal(
        self, chunk: dict[str, Any], state: StreamState
    ) -> list[InternalStreamEvent]:
        ss = state.substate(self.FORMAT_ID)
        events: list[InternalStreamEvent] = []

        # OpenAI streaming error（通常是单个 {"error": {...}}）
        if isinstance(chunk, dict) and "error" in chunk:
            try:
                events.append(ErrorEvent(error=self.error_to_internal(chunk)))
            except Exception:
                pass
            return events

        # 初始化 message_start
        if not ss.get("message_started"):
            msg_id = str(chunk.get("id") or state.message_id or "")
            # 保留初始化时设置的 model（客户端请求的模型），仅在空时用上游值
            model = state.model or str(chunk.get("model") or "")
            state.message_id = msg_id
            if not state.model:
                state.model = model
            ss["message_started"] = True
            ss.setdefault("thinking_block_started", False)
            ss.setdefault("thinking_block_stopped", False)
            ss.setdefault("text_block_started", False)
            ss.setdefault("text_block_stopped", False)
            # 根据首先出现的内容类型延迟分配块索引。
            # 这样可以避免预留空白：当不存在思考内容时，测试期望文本位于索引0。
            ss.setdefault("thinking_block_index", None)
            ss.setdefault("text_block_index", None)
            ss.setdefault("tool_id_to_block_index", {})
            ss.setdefault("next_block_index", 0)
            events.append(MessageStartEvent(message_id=msg_id, model=model))

        def _reserve_block_index(slot_key: str) -> int:
            idx = ss.get(slot_key)
            if isinstance(idx, int) and idx >= 0:
                return idx
            next_idx = int(ss.get("next_block_index") or 0)
            ss[slot_key] = next_idx
            ss["next_block_index"] = next_idx + 1
            return next_idx

        choices = chunk.get("choices") or []
        if not choices or not isinstance(choices, list):
            # OpenAI streaming may send a final "usage-only" chunk when
            # stream_options.include_usage=true, where `choices` is empty but `usage` exists.
            usage_info = self._openai_usage_to_internal(chunk.get("usage"))
            if usage_info is not None and (
                usage_info.total_tokens or usage_info.input_tokens or usage_info.output_tokens
            ):
                # For cross-format targets (e.g. Gemini), emitting usage as a late MessageStopEvent
                # allows the target normalizer to surface usage metadata even if the stop chunk
                # didn't carry it.
                events.append(MessageStopEvent(stop_reason=None, usage=usage_info))
            return events

        c0 = choices[0] if choices else {}
        if not isinstance(c0, dict):
            return events

        delta = c0.get("delta") or {}
        if not isinstance(delta, dict):
            delta = {}

        # reasoning_content delta (thinking)
        reasoning_delta = delta.get("reasoning_content")
        if isinstance(reasoning_delta, str) and reasoning_delta:
            thinking_index = _reserve_block_index("thinking_block_index")
            if not ss.get("thinking_block_started"):
                ss["thinking_block_started"] = True
                events.append(
                    ContentBlockStartEvent(
                        block_index=thinking_index,
                        block_type=ContentType.THINKING,
                    )
                )
            events.append(ContentDeltaEvent(block_index=thinking_index, text_delta=reasoning_delta))

        # content delta
        content_delta = delta.get("content")
        if isinstance(content_delta, str) and content_delta:
            # From thinking -> text, close thinking block first (only when text hasn't started yet).
            if (
                ss.get("thinking_block_started")
                and not ss.get("thinking_block_stopped")
                and not ss.get("text_block_started")
            ):
                ss["thinking_block_stopped"] = True
                events.append(
                    ContentBlockStopEvent(block_index=_reserve_block_index("thinking_block_index"))
                )
            if not ss.get("text_block_started"):
                ss["text_block_started"] = True
                events.append(
                    ContentBlockStartEvent(
                        block_index=_reserve_block_index("text_block_index"),
                        block_type=ContentType.TEXT,
                    )
                )
            events.append(
                ContentDeltaEvent(
                    block_index=_reserve_block_index("text_block_index"),
                    text_delta=content_delta,
                )
            )

        # tool_calls delta
        tool_calls = delta.get("tool_calls")
        if isinstance(tool_calls, list):
            for tool_call in tool_calls:
                if not isinstance(tool_call, dict):
                    continue

                tc_id = str(tool_call.get("id") or "")
                fn = tool_call.get("function") or {}
                fn = fn if isinstance(fn, dict) else {}
                tc_name = str(fn.get("name") or "")
                tc_args = fn.get("arguments")

                block_index = self._ensure_tool_block_index(
                    ss, tc_id or str(tool_call.get("index") or "")
                )

                # tool start（只在首次见到该 tool_id 时发）
                started_key = f"tool_started:{block_index}"
                if not ss.get(started_key):
                    ss[started_key] = True
                    events.append(
                        ContentBlockStartEvent(
                            block_index=block_index,
                            block_type=ContentType.TOOL_USE,
                            tool_id=tc_id or None,
                            tool_name=tc_name or None,
                        )
                    )

                if isinstance(tc_args, str) and tc_args:
                    events.append(
                        ToolCallDeltaEvent(
                            block_index=block_index,
                            tool_id=tc_id,
                            input_delta=tc_args,
                        )
                    )

        # finish_reason
        finish_reason = c0.get("finish_reason")
        if finish_reason is not None:
            stop_reason = self._FINISH_REASON_TO_STOP.get(str(finish_reason), StopReason.UNKNOWN)
            # 先补齐 content_block_stop（thinking + text），再发送 MessageStop
            if ss.get("thinking_block_started") and not ss.get("thinking_block_stopped"):
                ss["thinking_block_stopped"] = True
                events.append(
                    ContentBlockStopEvent(block_index=_reserve_block_index("thinking_block_index"))
                )
            if ss.get("text_block_started") and not ss.get("text_block_stopped"):
                ss["text_block_stopped"] = True
                events.append(
                    ContentBlockStopEvent(block_index=_reserve_block_index("text_block_index"))
                )
            # 解析 usage（需要请求时设置 stream_options.include_usage: true）
            usage_info = self._openai_usage_to_internal(chunk.get("usage"))
            events.append(MessageStopEvent(stop_reason=stop_reason, usage=usage_info))

        return events

    def stream_event_from_internal(
        self,
        event: InternalStreamEvent,
        state: StreamState,
    ) -> list[dict[str, Any]]:
        ss = state.substate(self.FORMAT_ID)
        out: list[dict[str, Any]] = []

        def base_chunk(delta: dict[str, Any], finish_reason: str | None = None) -> dict[str, Any]:
            return {
                "id": state.message_id or "chatcmpl-stream",
                "object": "chat.completion.chunk",
                "created": int(time.time()),
                "model": state.model or "",
                "system_fingerprint": None,
                "choices": [
                    {
                        "index": 0,
                        "delta": delta,
                        "finish_reason": finish_reason,
                    }
                ],
            }

        if isinstance(event, MessageStartEvent):
            state.message_id = event.message_id or state.message_id
            # 保留初始化时设置的 model（客户端请求的模型），仅在空时用事件值
            if not state.model:
                state.model = event.model or ""
            out.append(base_chunk({"role": "assistant"}))
            return out

        # 记录 block_index → block_type 的映射（用于 ContentDeltaEvent 区分 thinking vs text）
        if isinstance(event, ContentBlockStartEvent):
            ss[f"block_type_{event.block_index}"] = event.block_type.value

        if isinstance(event, ContentBlockStartEvent) and event.block_type == ContentType.THINKING:
            # Thinking block 开始：OpenAI 格式无需显式开始事件
            return out

        if isinstance(event, ContentDeltaEvent):
            if event.text_delta:
                # 检查 block_index 对应的 block_type（thinking vs text）
                block_type = ss.get(f"block_type_{event.block_index}")
                if block_type == ContentType.THINKING.value:
                    # 对齐 AM：thinking 内容输出为 reasoning_content
                    out.append(base_chunk({"reasoning_content": event.text_delta, "content": None}))
                else:
                    out.append(base_chunk({"content": event.text_delta}))
            return out

        if isinstance(event, ContentBlockStartEvent) and event.block_type == ContentType.TOOL_USE:
            tool_id = event.tool_id or ""
            tool_name = event.tool_name or ""
            tool_index = self._ensure_tool_call_index(ss, tool_id)
            out.append(
                base_chunk(
                    {
                        "tool_calls": [
                            {
                                "index": tool_index,
                                "id": tool_id,
                                "type": "function",
                                "function": {"name": tool_name, "arguments": ""},
                            }
                        ]
                    }
                )
            )
            return out

        # 图片内容块（来自 Gemini 图像生成等多模态输出）
        if isinstance(event, ContentBlockStartEvent) and event.block_type == ContentType.IMAGE:
            image_data = event.extra.get("image_data")
            image_media_type = event.extra.get("image_media_type")
            # 确保图片数据有效（base64 数据至少有一定长度）
            if (
                image_data
                and image_media_type
                and isinstance(image_data, str)
                and len(image_data) > 10
            ):
                # 构造 data URL 格式的图片
                data_url = f"data:{image_media_type};base64,{image_data}"
                # 存储图片数据，在 ContentBlockStopEvent 时输出
                image_blocks = ss.get("image_blocks")
                if not isinstance(image_blocks, dict):
                    image_blocks = {}
                    ss["image_blocks"] = image_blocks
                image_blocks[int(event.block_index)] = {
                    "url": data_url,
                    "media_type": image_media_type,
                }
            return out

        # 图片内容块结束时输出
        if isinstance(event, ContentBlockStopEvent):
            image_blocks = ss.get("image_blocks")
            if isinstance(image_blocks, dict):
                entry = image_blocks.get(int(event.block_index))
                if isinstance(entry, dict):
                    url = entry.get("url")
                    # 确保 URL 有效（data URL 至少包含 "data:" 前缀 + 一些数据）
                    if url and isinstance(url, str) and len(url) > 20:
                        # OpenAI 流式响应中 delta.content 必须是字符串
                        # 使用 markdown 图片格式，兼容各种客户端渲染
                        # 注意：base64 data URL 可能很长（几百KB~几MB），客户端需支持长内容
                        # 典型图片大小：100KB 原图 ≈ 130KB base64，1MB 原图 ≈ 1.3MB base64
                        if len(url) > 1_000_000:  # > 1MB
                            logger.warning(
                                f"Large image in stream response: {len(url)} bytes, "
                                "client may have rendering issues"
                            )
                        out.append(base_chunk({"content": f"![image]({url})"}))
                    # 清理已处理的图片
                    del image_blocks[int(event.block_index)]
                    return out
            # 其他 ContentBlockStopEvent 不处理
            return out

        if isinstance(event, ToolCallDeltaEvent):
            tool_index = self._ensure_tool_call_index(ss, event.tool_id)
            out.append(
                base_chunk(
                    {
                        "tool_calls": [
                            {
                                "index": tool_index,
                                "id": event.tool_id,
                                "type": "function",
                                "function": {"arguments": event.input_delta},
                            }
                        ]
                    }
                )
            )
            return out

        if isinstance(event, MessageStopEvent):
            finish_reason = None
            if event.stop_reason is not None:
                finish_reason = self._STOP_TO_FINISH_REASON.get(event.stop_reason, "stop")
            out.append(base_chunk({}, finish_reason=finish_reason))
            return out

        if isinstance(event, ErrorEvent):
            out.append(self.error_from_internal(event.error))
            return out

        # 其他事件类型：OpenAI chunk 无直接对应，跳过
        return out

    # =========================
    # Error conversion
    # =========================

    def is_error_response(self, response: dict[str, Any]) -> bool:
        return isinstance(response, dict) and "error" in response

    def error_to_internal(self, error_response: dict[str, Any]) -> InternalError:
        err = error_response.get("error") if isinstance(error_response, dict) else None
        err = err if isinstance(err, dict) else {}

        raw_type = err.get("type")
        mapped = ERROR_TYPE_MAPPINGS.get("OPENAI", {}).get(str(raw_type), ErrorType.UNKNOWN.value)
        internal_type = self._error_type_from_value(mapped)

        retryable = internal_type.value in RETRYABLE_ERROR_TYPES

        return InternalError(
            type=internal_type,
            message=str(err.get("message") or ""),
            code=err.get("code") if err.get("code") is None else str(err.get("code")),
            param=err.get("param") if err.get("param") is None else str(err.get("param")),
            retryable=retryable,
            extra={"openai": {"error": err}, "raw": {"type": raw_type}},
        )

    def error_from_internal(self, internal: InternalError) -> dict[str, Any]:
        type_str = self._ERROR_TYPE_TO_OPENAI.get(internal.type, "server_error")
        payload: dict[str, Any] = {
            "message": internal.message,
            "type": type_str,
        }
        if internal.code is not None:
            payload["code"] = internal.code
        if internal.param is not None:
            payload["param"] = internal.param
        return {"error": payload}

    # =========================
    # Video conversion
    # =========================

    def video_request_to_internal(self, request: dict[str, Any]) -> InternalVideoRequest:
        prompt = str(request.get("prompt") or "").strip()
        if not prompt:
            raise ValueError("Video prompt is required")

        size = str(request.get("size") or "720x1280")
        resolution, aspect_ratio = self._VIDEO_SIZE_REVERSE.get(size, ("720p", "9:16"))

        input_reference = request.get("input_reference")
        reference_url = str(input_reference) if input_reference else None

        # 安全解析 seconds 字段
        seconds_raw = request.get("seconds")
        try:
            duration_seconds = int(seconds_raw) if seconds_raw else 4
        except (ValueError, TypeError):
            duration_seconds = 4

        return InternalVideoRequest(
            prompt=prompt,
            model=str(request.get("model") or "sora-2"),
            duration_seconds=duration_seconds,
            resolution=resolution,
            aspect_ratio=aspect_ratio,
            character_ids=request.get("character_ids") or [],
            reference_image_url=reference_url,
            extra={"original_size": size},
        )

    def video_request_from_internal(self, internal: InternalVideoRequest) -> dict[str, Any]:
        size = self._VIDEO_SIZE_MAP.get((internal.resolution, internal.aspect_ratio), "720x1280")
        payload: dict[str, Any] = {
            "prompt": internal.prompt,
            "model": internal.model,
            "seconds": internal.duration_seconds,
            "size": size,
            "character_ids": internal.character_ids,
        }
        if internal.reference_image_url:
            payload["input_reference"] = internal.reference_image_url
        return payload

    def video_task_to_internal(self, response: dict[str, Any]) -> InternalVideoTask:
        status_map = {
            "queued": VideoStatus.QUEUED,
            "processing": VideoStatus.PROCESSING,
            "completed": VideoStatus.COMPLETED,
            "failed": VideoStatus.FAILED,
        }
        status = status_map.get(str(response.get("status") or ""), VideoStatus.PENDING)

        error = response.get("error") or {}
        error_code = error.get("code") if isinstance(error, dict) else None
        error_message = error.get("message") if isinstance(error, dict) else None

        created_at = response.get("created_at")
        completed_at = response.get("completed_at")
        expires_at = response.get("expires_at")

        return InternalVideoTask(
            id=str(response.get("id") or ""),
            status=status,
            progress_percent=int(response.get("progress") or 0),
            created_at=datetime.fromtimestamp(created_at, tz=timezone.utc) if created_at else None,
            completed_at=(
                datetime.fromtimestamp(completed_at, tz=timezone.utc) if completed_at else None
            ),
            expires_at=datetime.fromtimestamp(expires_at, tz=timezone.utc) if expires_at else None,
            error_code=error_code,
            error_message=error_message,
            extra={
                "object": response.get("object"),
                "model": response.get("model"),
                "size": response.get("size"),
                "seconds": response.get("seconds"),
            },
        )

    def video_task_from_internal(
        self, internal: InternalVideoTask, *, base_url: str | None = None
    ) -> dict[str, Any]:
        status_map = {
            VideoStatus.PENDING: "queued",
            VideoStatus.SUBMITTED: "queued",
            VideoStatus.QUEUED: "queued",
            VideoStatus.PROCESSING: "processing",
            VideoStatus.COMPLETED: "completed",
            VideoStatus.FAILED: "failed",
            VideoStatus.CANCELLED: "failed",
            VideoStatus.EXPIRED: "failed",
        }
        payload: dict[str, Any] = {
            "id": internal.id,
            "object": "video",
            "status": status_map.get(internal.status, "queued"),
            "progress": internal.progress_percent,
        }

        if internal.created_at:
            payload["created_at"] = int(internal.created_at.timestamp())
        if internal.completed_at:
            payload["completed_at"] = int(internal.completed_at.timestamp())
        if internal.expires_at:
            payload["expires_at"] = int(internal.expires_at.timestamp())
        if internal.error_code:
            payload["error"] = {
                "code": internal.error_code,
                "message": internal.error_message,
            }

        # 基本字段
        for key in ["model", "size", "prompt"]:
            if internal.extra.get(key):
                payload[key] = internal.extra[key]

        # seconds 必须是字符串类型
        seconds = internal.extra.get("seconds")
        if seconds is not None:
            payload["seconds"] = str(seconds)

        # remix 相关字段
        remixed_from = internal.extra.get("remixed_from_video_id")
        if remixed_from:
            payload["remixed_from_video_id"] = remixed_from

        return payload

    def video_poll_to_internal(self, response: dict[str, Any]) -> InternalVideoPollResult:
        status = str(response.get("status") or "")
        task_id = response.get("id")

        if status == "completed":
            expires_at = response.get("expires_at")
            # 优先使用上游返回的直接 URL（某些代理如 API易 会返回 CDN URL）
            # 回退到构建相对路径（标准 OpenAI API 通过 /content 端点下载）
            direct_url = (
                response.get("video_url") or response.get("url") or response.get("result_url")
            )
            # 使用直接 URL 或回退到相对路径
            video_url = direct_url or (f"videos/{task_id}/content" if task_id else None)
            if not video_url:
                return InternalVideoPollResult(
                    status=VideoStatus.FAILED,
                    error_code="missing_video_url",
                    error_message="Upstream response missing video url",
                    raw_response=response,
                )
            # 提取实际视频时长（尝试多种字段名）
            video_duration = self._extract_video_duration(response)
            return InternalVideoPollResult(
                status=VideoStatus.COMPLETED,
                progress_percent=100,
                video_url=video_url,
                expires_at=(
                    datetime.fromtimestamp(expires_at, tz=timezone.utc) if expires_at else None
                ),
                raw_response=response,
                video_duration_seconds=video_duration,
            )
        if status == "failed":
            error = response.get("error") or {}
            return InternalVideoPollResult(
                status=VideoStatus.FAILED,
                error_code=error.get("code") if isinstance(error, dict) else None,
                error_message=error.get("message") if isinstance(error, dict) else None,
                raw_response=response,
            )

        return InternalVideoPollResult(
            status=VideoStatus.PROCESSING,
            progress_percent=int(response.get("progress") or 0),
            raw_response=response,
        )

    def _extract_video_duration(self, response: dict[str, Any]) -> float | None:
        """从响应中提取实际视频时长"""
        # 尝试多种可能的字段名
        duration_fields = [
            "duration_seconds",
            "duration",
            "video_duration",
            "video_duration_seconds",
            "length",
            "length_seconds",
        ]
        for field in duration_fields:
            val = response.get(field)
            if val is not None:
                try:
                    return float(val)
                except (ValueError, TypeError):
                    continue
        # 尝试从嵌套的 metadata 中获取
        metadata = response.get("metadata") or response.get("video_metadata") or {}
        if isinstance(metadata, dict):
            for field in duration_fields:
                val = metadata.get(field)
                if val is not None:
                    try:
                        return float(val)
                    except (ValueError, TypeError):
                        continue
        return None

    # =========================
    # Helpers
    # =========================

    def _openai_message_to_internal(
        self, msg: dict[str, Any]
    ) -> tuple[InternalMessage | None, dict[str, int]]:
        dropped: dict[str, int] = {}

        role_raw = str(msg.get("role") or "unknown")
        role = self._role_from_openai(role_raw)

        # tool role -> internal user message with ToolResultBlock
        if role_raw == "tool":
            tool_call_id = str(msg.get("tool_call_id") or "")
            tr_block, tr_dropped = self._openai_tool_result_message_to_block(msg, tool_call_id)
            self._merge_dropped(dropped, tr_dropped)
            if tr_block is None:
                return None, dropped
            return (
                InternalMessage(
                    role=Role.USER,
                    content=[tr_block],
                    extra=self._extract_extra(msg, {"role", "content"}),
                ),
                dropped,
            )

        # 对齐 AM：解析 reasoning_content → ThinkingBlock（放在 content blocks 前面）
        reasoning_content = msg.get("reasoning_content")
        reasoning_blocks: list[ContentBlock] = []
        if (
            isinstance(reasoning_content, str)
            and reasoning_content
            and reasoning_content != "[undefined]"
        ):
            reasoning_blocks.append(ThinkingBlock(thinking=reasoning_content))

        blocks, content_dropped = self._openai_content_to_blocks(msg.get("content"))
        self._merge_dropped(dropped, content_dropped)

        # 合并：thinking blocks 放在前面（对齐 AM/Claude 的 thinking-first 约定）
        if reasoning_blocks:
            blocks = reasoning_blocks + blocks

        # assistant tool_calls
        if role_raw == "assistant":
            tool_calls = msg.get("tool_calls") or []
            if isinstance(tool_calls, list):
                for tool_call in tool_calls:
                    tool_block, tc_dropped = self._openai_tool_call_to_block(tool_call)
                    self._merge_dropped(dropped, tc_dropped)
                    if tool_block is not None:
                        blocks.append(tool_block)

            # legacy function_call
            func_call = msg.get("function_call")
            if isinstance(func_call, dict):
                tool_block, tc_dropped = self._legacy_function_call_to_block(func_call)
                self._merge_dropped(dropped, tc_dropped)
                if tool_block is not None:
                    blocks.append(tool_block)

        return (
            InternalMessage(
                role=role,
                content=blocks,
                extra=self._extract_extra(msg, {"role", "content", "tool_calls", "function_call"}),
            ),
            dropped,
        )

    def _openai_content_to_blocks(self, content: Any) -> tuple[list[ContentBlock], dict[str, int]]:
        dropped: dict[str, int] = {}

        if content is None:
            return [], dropped
        if isinstance(content, str):
            return ([TextBlock(text=content)] if content else []), dropped
        if not isinstance(content, list):
            dropped["openai_content_non_list"] = dropped.get("openai_content_non_list", 0) + 1
            return [], dropped

        blocks: list[ContentBlock] = []
        for part in content:
            if not isinstance(part, dict):
                dropped["openai_content_part_non_dict"] = (
                    dropped.get("openai_content_part_non_dict", 0) + 1
                )
                continue

            ptype = str(part.get("type") or "unknown")
            if ptype == "text":
                text = str(part.get("text") or "")
                if text:
                    blocks.append(
                        TextBlock(text=text, extra=self._extract_extra(part, {"type", "text"}))
                    )
                continue

            if ptype == "image_url":
                url = (
                    (part.get("image_url") or {}).get("url")
                    if isinstance(part.get("image_url"), dict)
                    else None
                )
                if isinstance(url, str) and url:
                    img = self._image_url_to_block(url)
                    img.extra.update(self._extract_extra(part, {"type", "image_url"}))
                    blocks.append(img)
                else:
                    dropped["openai_image_url_missing"] = (
                        dropped.get("openai_image_url_missing", 0) + 1
                    )
                    blocks.append(UnknownBlock(raw_type="image_url", payload=part))
                continue

            # 其他类型：UnknownBlock
            dropped_key = f"openai_part:{ptype}"
            dropped[dropped_key] = dropped.get(dropped_key, 0) + 1
            blocks.append(UnknownBlock(raw_type=ptype, payload=part))

        return blocks, dropped

    def _collapse_openai_text(self, content: Any) -> tuple[str, dict[str, int]]:
        blocks, dropped = self._openai_content_to_blocks(content)
        text_parts = [b.text for b in blocks if isinstance(b, TextBlock) and b.text]
        return ("\n\n".join(text_parts), dropped)

    def _join_instructions(self, instructions: list[InstructionSegment]) -> str | None:
        parts = [seg.text for seg in instructions if seg.text]
        joined = "\n\n".join(parts)
        return joined or None

    def _openai_tools_to_internal(self, tools: Any) -> list[ToolDefinition] | None:
        if not tools or not isinstance(tools, list):
            return None

        out: list[ToolDefinition] = []
        for tool in tools:
            if not isinstance(tool, dict):
                continue
            if tool.get("type") != "function":
                continue

            function_raw = tool.get("function")
            function: dict[str, Any] = function_raw if isinstance(function_raw, dict) else {}
            name = str(function.get("name") or "")
            if not name:
                continue

            params_raw = function.get("parameters")
            out.append(
                ToolDefinition(
                    name=name,
                    description=function.get("description"),
                    parameters=params_raw if isinstance(params_raw, dict) else None,
                    extra={
                        "openai_tool": self._extract_extra(tool, {"type", "function"}),
                        "openai_function": self._extract_extra(
                            function, {"name", "description", "parameters"}
                        ),
                    },
                )
            )

        return out or None

    def _openai_tool_choice_to_internal(self, tool_choice: Any) -> ToolChoice | None:
        if tool_choice is None:
            return None

        if isinstance(tool_choice, str):
            if tool_choice == "none":
                return ToolChoice(type=ToolChoiceType.NONE)
            if tool_choice == "auto":
                return ToolChoice(type=ToolChoiceType.AUTO)
            if tool_choice == "required":
                return ToolChoice(type=ToolChoiceType.REQUIRED)
            # 其他字符串：保守降级为 auto
            return ToolChoice(type=ToolChoiceType.AUTO, extra={"raw": tool_choice})

        if isinstance(tool_choice, dict) and tool_choice.get("type") == "function":
            fn_raw = tool_choice.get("function")
            fn: dict[str, Any] = fn_raw if isinstance(fn_raw, dict) else {}
            name = str(fn.get("name") or "")
            return ToolChoice(
                type=ToolChoiceType.TOOL, tool_name=name, extra={"openai": tool_choice}
            )

        return ToolChoice(type=ToolChoiceType.AUTO, extra={"openai": tool_choice})

    def _tool_choice_to_openai(self, tool_choice: ToolChoice) -> str | dict[str, Any]:
        if tool_choice.type == ToolChoiceType.NONE:
            return "none"
        if tool_choice.type == ToolChoiceType.AUTO:
            return "auto"
        if tool_choice.type == ToolChoiceType.REQUIRED:
            return "required"
        if tool_choice.type == ToolChoiceType.TOOL:
            return {"type": "function", "function": {"name": tool_choice.tool_name or ""}}
        return "auto"

    def _openai_tool_call_to_block(
        self, tool_call: Any
    ) -> tuple[ToolUseBlock | None, dict[str, int]]:
        dropped: dict[str, int] = {}
        if not isinstance(tool_call, dict):
            dropped["openai_tool_call_non_dict"] = dropped.get("openai_tool_call_non_dict", 0) + 1
            return None, dropped

        if tool_call.get("type") != "function":
            dropped_key = f"openai_tool_call_type:{tool_call.get('type')}"
            dropped[dropped_key] = dropped.get(dropped_key, 0) + 1
            return None, dropped

        fn_raw = tool_call.get("function")
        fn: dict[str, Any] = fn_raw if isinstance(fn_raw, dict) else {}
        name = str(fn.get("name") or "")
        args_str = str(fn.get("arguments") or "")
        tool_id = str(tool_call.get("id") or "")

        tool_input: dict[str, Any]
        if args_str:
            try:
                parsed = json.loads(args_str)
                tool_input = parsed if isinstance(parsed, dict) else {"raw": parsed}
            except json.JSONDecodeError:
                tool_input = {"raw": args_str}
        else:
            tool_input = {}

        return (
            ToolUseBlock(
                tool_id=tool_id,
                tool_name=name,
                tool_input=tool_input,
                extra={"openai": tool_call},
            ),
            dropped,
        )

    def _legacy_function_call_to_block(
        self, func_call: dict[str, Any]
    ) -> tuple[ToolUseBlock | None, dict[str, int]]:
        dropped: dict[str, int] = {}
        name = str(func_call.get("name") or "")
        args_str = str(func_call.get("arguments") or "")
        if not name:
            dropped["openai_function_call_missing_name"] = (
                dropped.get("openai_function_call_missing_name", 0) + 1
            )
            return None, dropped

        tool_input: dict[str, Any]
        if args_str:
            try:
                parsed = json.loads(args_str)
                tool_input = parsed if isinstance(parsed, dict) else {"raw": parsed}
            except json.JSONDecodeError:
                tool_input = {"raw": args_str}
        else:
            tool_input = {}

        return (
            ToolUseBlock(
                tool_id="call_0",
                tool_name=name,
                tool_input=tool_input,
                extra={"openai": {"function_call": func_call}},
            ),
            dropped,
        )

    def _openai_tool_result_message_to_block(
        self,
        msg: dict[str, Any],
        tool_call_id: str,
    ) -> tuple[ToolResultBlock | None, dict[str, int]]:
        dropped: dict[str, int] = {}
        content = msg.get("content")
        if content is None:
            return (
                ToolResultBlock(
                    tool_use_id=tool_call_id, output=None, content_text=None, extra={"openai": msg}
                ),
                dropped,
            )

        if isinstance(content, str):
            parsed: Any = None
            try:
                parsed = json.loads(content)
            except json.JSONDecodeError:
                parsed = None

            if parsed is not None:
                return (
                    ToolResultBlock(
                        tool_use_id=tool_call_id,
                        output=parsed,
                        content_text=None,
                        extra={"raw": {"content": content}, "openai": msg},
                    ),
                    dropped,
                )

            return (
                ToolResultBlock(
                    tool_use_id=tool_call_id,
                    output=None,
                    content_text=content,
                    extra={"openai": msg},
                ),
                dropped,
            )

        # 非字符串：尽量保留为 output
        return (
            ToolResultBlock(
                tool_use_id=tool_call_id,
                output=content,
                content_text=None,
                extra={"openai": msg},
            ),
            dropped,
        )

    def _openai_usage_to_internal(self, usage: Any) -> UsageInfo | None:
        if not isinstance(usage, dict):
            return None

        input_tokens = int(usage.get("prompt_tokens") or 0)
        output_tokens = int(usage.get("completion_tokens") or 0)
        total_tokens = usage.get("total_tokens")
        if total_tokens is None:
            total_tokens = input_tokens + output_tokens
        total_tokens_int = int(total_tokens)

        # Some OpenAI-compatible providers may include a placeholder `usage` object with 0s
        # (or omit fields); treat it as "missing usage" to avoid emitting misleading usageMetadata.
        if input_tokens == 0 and output_tokens == 0 and total_tokens_int == 0:
            return None

        extra = self._extract_extra(
            usage,
            {"prompt_tokens", "completion_tokens", "total_tokens"},
        )
        return UsageInfo(
            input_tokens=input_tokens,
            output_tokens=output_tokens,
            total_tokens=total_tokens_int,
            extra={"openai": extra} if extra else {},
        )

    def _blocks_to_openai_content(
        self, blocks: list[ContentBlock]
    ) -> str | list[dict[str, Any]] | None:
        parts: list[dict[str, Any]] = []
        text_parts: list[str] = []

        for b in blocks:
            if isinstance(b, TextBlock):
                if b.text:
                    text_parts.append(b.text)
                continue
            if isinstance(b, ImageBlock):
                # 区分两种图片来源：
                # 1. URL 引用（OpenAI 原生格式）-> multipart content
                # 2. base64 内嵌数据（格式转换来的）-> markdown 格式
                if b.url and not b.data:
                    # OpenAI 原生格式：URL 引用的图片，使用 multipart content
                    parts.append({"type": "image_url", "image_url": {"url": b.url}})
                elif b.data and b.media_type:
                    # 格式转换来的图片（base64 内嵌），使用 markdown 格式
                    data_url = f"data:{b.media_type};base64,{b.data}"
                    text_parts.append(f"![image]({data_url})")
                elif b.url:
                    # 有 URL 也有 data，优先使用 URL
                    parts.append({"type": "image_url", "image_url": {"url": b.url}})
                continue

            # Unknown / Tool blocks 不进入 OpenAI content

        # 如果有 OpenAI 原生格式的图片（URL 引用），使用 multipart content
        if parts:
            if text_parts:
                parts = [{"type": "text", "text": "\n".join(text_parts)}] + parts
            return parts

        # 纯文本或包含 markdown 图片
        if text_parts:
            return "\n".join(text_parts)

        # OpenAI content 可以是空字符串；但作为响应 message.content 通常允许为 ""/None。
        return ""

    def _split_blocks(
        self, blocks: list[ContentBlock]
    ) -> tuple[list[ThinkingBlock], list[ContentBlock], list[ToolUseBlock]]:
        thinking_blocks: list[ThinkingBlock] = []
        content_blocks: list[ContentBlock] = []
        tool_blocks: list[ToolUseBlock] = []
        for b in blocks:
            if isinstance(b, ThinkingBlock):
                thinking_blocks.append(b)
                continue
            if isinstance(b, ToolUseBlock):
                tool_blocks.append(b)
                continue
            if isinstance(b, ToolResultBlock):
                # InternalResponse.content 不应该包含 tool_result；忽略
                continue
            if isinstance(b, UnknownBlock):
                continue
            content_blocks.append(b)
        return thinking_blocks, content_blocks, tool_blocks

    def _internal_message_to_openai_messages(self, msg: InternalMessage) -> list[dict[str, Any]]:
        if msg.role == Role.USER:
            return self._user_message_to_openai(msg)
        if msg.role == Role.ASSISTANT:
            return [self._assistant_message_to_openai(msg)]
        if msg.role == Role.TOOL:
            # 兜底：尽量按 tool message 输出（但内部应避免 Role.TOOL）
            content_value = self._blocks_to_openai_content(msg.content)
            return [{"role": "tool", "content": content_value or ""}]
        return [{"role": "user", "content": ""}]

    def _user_message_to_openai(self, msg: InternalMessage) -> list[dict[str, Any]]:
        out: list[dict[str, Any]] = []
        pending: list[ContentBlock] = []

        def flush_user() -> None:
            nonlocal pending
            # 丢弃 Unknown/Tool blocks（tool_result 在 flush 时不会出现）
            content = self._blocks_to_openai_content(
                [
                    b
                    for b in pending
                    if not isinstance(b, (UnknownBlock, ToolUseBlock, ToolResultBlock))
                ]
            )
            if content is None:
                content = ""
            out.append({"role": "user", "content": content})
            pending = []

        for b in msg.content:
            if isinstance(b, ToolResultBlock):
                if pending:
                    flush_user()
                out.append(self._tool_result_block_to_openai_message(b))
                continue
            if isinstance(b, ToolUseBlock):
                # user role 下不应出现 tool_use，直接忽略
                continue
            if isinstance(b, UnknownBlock):
                continue
            pending.append(b)

        if pending:
            flush_user()

        if not out:
            out.append({"role": "user", "content": ""})

        return out

    def _assistant_message_to_openai(self, msg: InternalMessage) -> dict[str, Any]:
        thinking_texts: list[str] = []
        content_blocks: list[ContentBlock] = []
        tool_blocks: list[ToolUseBlock] = []

        for b in msg.content:
            if isinstance(b, ThinkingBlock):
                if b.thinking:
                    thinking_texts.append(b.thinking)
                continue
            if isinstance(b, ToolUseBlock):
                tool_blocks.append(b)
                continue
            if isinstance(b, ToolResultBlock):
                continue
            if isinstance(b, UnknownBlock):
                continue
            content_blocks.append(b)

        out: dict[str, Any] = {"role": "assistant"}

        # 对齐 AM：ThinkingBlock → reasoning_content
        if thinking_texts:
            out["reasoning_content"] = "".join(thinking_texts)

        content_value = self._blocks_to_openai_content(content_blocks)
        out["content"] = content_value if content_value is not None else ""

        if tool_blocks:
            out["tool_calls"] = [
                self._tool_use_block_to_openai_call(b, idx) for idx, b in enumerate(tool_blocks)
            ]

        return out

    def _tool_result_block_to_openai_message(self, block: ToolResultBlock) -> dict[str, Any]:
        content: str
        if block.content_text is not None:
            content = block.content_text
        elif block.output is None:
            content = ""
        elif isinstance(block.output, str):
            content = block.output
        else:
            content = json.dumps(block.output, ensure_ascii=False)

        return {
            "role": "tool",
            "tool_call_id": block.tool_use_id,
            "content": content,
        }

    def _tool_use_block_to_openai_call(self, block: ToolUseBlock, index: int) -> dict[str, Any]:
        return {
            "index": index,
            "id": block.tool_id or f"call_{index}",
            "type": "function",
            "function": {
                "name": block.tool_name,
                "arguments": json.dumps(block.tool_input or {}, ensure_ascii=False),
            },
        }

    def _image_url_to_block(self, url: str) -> ImageBlock:
        if url.startswith("data:") and ";base64," in url:
            header, _, data = url.partition(",")
            media_type = header.split(";")[0].split(":", 1)[-1]
            return ImageBlock(data=data, media_type=media_type)
        return ImageBlock(url=url)

    def _role_from_openai(self, role: str) -> Role:
        if role == "user":
            return Role.USER
        if role == "assistant":
            return Role.ASSISTANT
        if role == "system":
            return Role.SYSTEM
        if role == "developer":
            return Role.DEVELOPER
        if role == "tool":
            return Role.TOOL
        return Role.UNKNOWN

    def _error_type_from_value(self, value: str) -> ErrorType:
        try:
            return ErrorType(value)
        except ValueError:
            return ErrorType.UNKNOWN

    def _optional_int(self, value: Any) -> int | None:
        if value is None:
            return None
        try:
            return int(value)
        except (TypeError, ValueError):
            return None

    def _optional_float(self, value: Any) -> float | None:
        if value is None:
            return None
        try:
            return float(value)
        except (TypeError, ValueError):
            return None

    def _coerce_str_list(self, value: Any) -> list[str] | None:
        if value is None:
            return None
        if isinstance(value, str):
            return [value]
        if isinstance(value, list):
            return [str(x) for x in value if x is not None]
        return None

    def _extract_extra(self, payload: dict[str, Any], known_keys: set[str]) -> dict[str, Any]:
        return {k: v for k, v in payload.items() if k not in known_keys}

    def _merge_dropped(self, target: dict[str, int], source: dict[str, int]) -> None:
        for k, v in source.items():
            target[k] = target.get(k, 0) + int(v)

    def _ensure_tool_block_index(self, ss: dict[str, Any], tool_key: str) -> int:
        mapping = ss.get("tool_id_to_block_index")
        if not isinstance(mapping, dict):
            mapping = {}
            ss["tool_id_to_block_index"] = mapping

        if tool_key in mapping:
            return int(mapping[tool_key])

        next_idx = int(ss.get("next_block_index") or 0)
        mapping[tool_key] = next_idx
        ss["next_block_index"] = next_idx + 1
        return next_idx

    def _ensure_tool_call_index(self, ss: dict[str, Any], tool_id: str) -> int:
        mapping = ss.get("tool_id_to_index")
        if not isinstance(mapping, dict):
            mapping = {}
            ss["tool_id_to_index"] = mapping
            ss["next_tool_index"] = 0

        if tool_id in mapping:
            return int(mapping[tool_id])

        next_idx = int(ss.get("next_tool_index") or 0)
        mapping[tool_id] = next_idx
        ss["next_tool_index"] = next_idx + 1
        return next_idx
