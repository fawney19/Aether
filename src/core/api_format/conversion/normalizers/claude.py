"""
Claude Messages API Normalizer

负责：
- Claude Messages request/response <-> Internal 表示转换
- 可选：Claude streaming event <-> InternalStreamEvent
- 可选：Claude error <-> InternalError
"""

import json
from typing import Any

from src.core.api_format.conversion.field_mappings import (
    ERROR_TYPE_MAPPINGS,
    RETRYABLE_ERROR_TYPES,
    STOP_REASON_MAPPINGS,
    THINKING_BUDGET_TOKENS_MIN,
    THINKING_BUDGET_TOKENS_PERCENTAGE,
    USAGE_FIELD_MAPPINGS,
    WEB_SEARCH_CONTEXT_SIZE_TO_MAX_USES,
    get_claude_default_max_tokens,
)
from src.core.api_format.conversion.internal import (
    AudioBlock,
    ContentBlock,
    ContentType,
    ErrorType,
    FileBlock,
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
    ThinkingConfig,
    ToolChoice,
    ToolChoiceType,
    ToolDefinition,
    ToolResultBlock,
    ToolUseBlock,
    UnknownBlock,
    UsageInfo,
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


class ClaudeNormalizer(FormatNormalizer):
    FORMAT_ID = "claude:chat"
    capabilities = FormatCapabilities(
        supports_stream=True,
        supports_error_conversion=True,
        supports_tools=True,
        supports_images=True,
    )

    _CLAUDE_STOP_TO_INTERNAL: dict[str, StopReason] = {
        "end_turn": StopReason.END_TURN,
        "max_tokens": StopReason.MAX_TOKENS,
        "stop_sequence": StopReason.STOP_SEQUENCE,
        "tool_use": StopReason.TOOL_USE,
        "pause_turn": StopReason.PAUSE_TURN,
        "refusal": StopReason.REFUSAL,
        "content_filtered": StopReason.CONTENT_FILTERED,
    }

    _ERROR_TYPE_TO_CLAUDE: dict[ErrorType, str] = {
        ErrorType.INVALID_REQUEST: "invalid_request_error",
        ErrorType.AUTHENTICATION: "authentication_error",
        ErrorType.PERMISSION_DENIED: "permission_error",
        ErrorType.NOT_FOUND: "not_found_error",
        ErrorType.RATE_LIMIT: "rate_limit_error",
        ErrorType.OVERLOADED: "overloaded_error",
        ErrorType.SERVER_ERROR: "api_error",
        ErrorType.CONTENT_FILTERED: "invalid_request_error",
        ErrorType.CONTEXT_LENGTH_EXCEEDED: "invalid_request_error",
        ErrorType.UNKNOWN: "api_error",
    }

    # =========================
    # Requests
    # =========================

    def request_to_internal(self, request: dict[str, Any]) -> InternalRequest:
        model = str(request.get("model") or "")
        dropped: dict[str, int] = {}

        instructions: list[InstructionSegment] = []

        # 顶层 system 先进入 instructions（保持确定性优先级）
        sys_value = request.get("system")
        sys_text, sys_dropped, sys_segments = self._collapse_claude_system(sys_value)
        self._merge_dropped(dropped, sys_dropped)
        if sys_segments:
            # 数组格式含 cache_control：保留逐段 InstructionSegment
            instructions.extend(sys_segments)
        elif sys_text:
            instructions.append(InstructionSegment(role=Role.SYSTEM, text=sys_text))

        messages: list[InternalMessage] = []
        for msg in request.get("messages") or []:
            if not isinstance(msg, dict):
                dropped["claude_message_non_dict"] = dropped.get("claude_message_non_dict", 0) + 1
                continue

            role = str(msg.get("role") or "unknown")

            # 兼容：少数客户端可能把 system/developer 混进 messages[]
            if role in ("system", "developer"):
                text, md, _ = self._collapse_claude_system(msg.get("content"))
                self._merge_dropped(dropped, md)
                if text:
                    instructions.append(
                        InstructionSegment(
                            role=Role.SYSTEM if role == "system" else Role.DEVELOPER,
                            text=text,
                            extra=self._extract_extra(msg, {"role", "content"}),
                        )
                    )
                continue

            imsg, md = self._claude_message_to_internal(msg)
            self._merge_dropped(dropped, md)
            if imsg is not None:
                messages.append(imsg)

        system_text = self._join_instructions(instructions)

        tools = self._claude_tools_to_internal(request.get("tools"))
        tool_choice = self._claude_tool_choice_to_internal(request.get("tool_choice"))

        # 解析 Claude 原生 thinking 配置
        thinking: ThinkingConfig | None = None
        thinking_raw = request.get("thinking")
        if isinstance(thinking_raw, dict):
            thinking_type = str(thinking_raw.get("type") or "")
            if thinking_type in ("enabled", "adaptive"):
                budget = self._optional_int(thinking_raw.get("budget_tokens"))
                thinking = ThinkingConfig(
                    enabled=True,
                    budget_tokens=budget,
                    extra={"claude_thinking": thinking_raw},
                )

        internal = InternalRequest(
            model=model,
            messages=messages,
            instructions=instructions,
            system=system_text,
            max_tokens=self._optional_int(request.get("max_tokens")),
            temperature=self._optional_float(request.get("temperature")),
            top_p=self._optional_float(request.get("top_p")),
            top_k=self._optional_int(request.get("top_k")),
            stop_sequences=self._coerce_str_list(request.get("stop_sequences")),
            stream=bool(request.get("stream") or False),
            tools=tools,
            tool_choice=tool_choice,
            thinking=thinking,
            extra={
                "claude": self._extract_extra(
                    request,
                    {
                        "model",
                        "messages",
                        "system",
                        "max_tokens",
                        "temperature",
                        "top_p",
                        "top_k",
                        "stop_sequences",
                        "stream",
                        "tools",
                        "tool_choice",
                        "thinking",
                    },
                )
            },
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
        # system: 如果任一 InstructionSegment 有 cache_control，输出数组格式
        has_cache_control = any(seg.extra.get("cache_control") for seg in internal.instructions)
        if has_cache_control and internal.instructions:
            system_value: str | list[dict[str, Any]] | None = [
                {
                    "type": "text",
                    "text": seg.text,
                    **(
                        {"cache_control": seg.extra["cache_control"]}
                        if seg.extra.get("cache_control")
                        else {}
                    ),
                }
                for seg in internal.instructions
                if seg.text
            ]
            if not system_value:
                system_value = None
        else:
            system_value = internal.system or self._join_instructions(internal.instructions)

        # Claude Messages API: messages[] 仅允许 user/assistant，且需要交替；这里做最小修复
        fixed_messages = self._coerce_claude_message_sequence(internal.messages)

        out_messages: list[dict[str, Any]] = [
            self._internal_message_to_claude(m) for m in fixed_messages
        ]

        # max_tokens: 优先使用请求中的值, 其次 GlobalModel.output_limit, 最后硬编码默认值
        effective_max_tokens: int
        if internal.max_tokens is not None:
            effective_max_tokens = internal.max_tokens
        elif internal.output_limit is not None:
            effective_max_tokens = internal.output_limit
        else:
            effective_max_tokens = get_claude_default_max_tokens(internal.model)

        result: dict[str, Any] = {
            "model": internal.model,
            "messages": out_messages,
            "max_tokens": effective_max_tokens,
        }

        if system_value:
            result["system"] = system_value

        if internal.temperature is not None:
            result["temperature"] = internal.temperature
        if internal.top_p is not None:
            result["top_p"] = internal.top_p
        if internal.top_k is not None:
            result["top_k"] = internal.top_k
        if internal.stop_sequences:
            result["stop_sequences"] = list(internal.stop_sequences)
        if internal.stream:
            result["stream"] = True

        if internal.tools:
            claude_tools: list[dict[str, Any]] = []
            for t in internal.tools:
                # 跳过 Gemini 内置工具（在 Claude 中无对应物）
                if t.extra.get("gemini_builtin_tool"):
                    continue
                tool_def: dict[str, Any] = {
                    "name": t.name,
                    "input_schema": t.parameters or {},
                    **(t.extra.get("claude") or {}),
                }
                if t.description is not None:
                    tool_def["description"] = t.description
                claude_tools.append(tool_def)
            if claude_tools:
                result["tools"] = claude_tools

        if internal.tool_choice:
            tc = self._tool_choice_to_claude(internal.tool_choice)
            # parallel_tool_calls=False -> disable_parallel_tool_use=True
            if internal.parallel_tool_calls is False and tc.get("type") != "none":
                tc["disable_parallel_tool_use"] = True
            result["tool_choice"] = tc

        # thinking 配置
        if internal.thinking and internal.thinking.enabled:
            # 优先使用原始 Claude thinking 配置（round-trip 透传）
            claude_thinking = internal.thinking.extra.get("claude_thinking")
            if isinstance(claude_thinking, dict):
                result["thinking"] = claude_thinking
            else:
                thinking_out: dict[str, Any] = {"type": "enabled"}
                # Claude API 要求 budget_tokens 必须提供
                # 跨格式时 internal.model 可能是非 Claude 模型名，
                # 仅当模型名以 claude- 开头时用模型感知默认值，否则用固定安全值
                if internal.thinking.budget_tokens is not None:
                    thinking_out["budget_tokens"] = internal.thinking.budget_tokens
                else:
                    # 基于 effective_max_tokens 计算兜底 budget，避免覆盖用户显式指定的 max_tokens
                    thinking_out["budget_tokens"] = max(
                        int(effective_max_tokens * THINKING_BUDGET_TOKENS_PERCENTAGE),
                        THINKING_BUDGET_TOKENS_MIN,
                    )
                # 确保 budget_tokens >= 最小值（参考 new-api: 1280）
                bt = thinking_out["budget_tokens"]
                if bt < THINKING_BUDGET_TOKENS_MIN:
                    thinking_out["budget_tokens"] = THINKING_BUDGET_TOKENS_MIN
                    bt = THINKING_BUDGET_TOKENS_MIN
                # Claude API 要求 budget_tokens < max_tokens，确保不违反约束
                max_t = result.get("max_tokens")
                if max_t is not None and bt >= max_t:
                    result["max_tokens"] = bt + 1
                result["thinking"] = thinking_out

        # web_search_options -> Claude web_search tool
        web_search_opts = internal.extra.get("web_search_options") if internal.extra else None
        if isinstance(web_search_opts, dict):
            context_size = str(web_search_opts.get("search_context_size") or "medium")
            max_uses = WEB_SEARCH_CONTEXT_SIZE_TO_MAX_USES.get(context_size, 5)
            ws_tool: dict[str, Any] = {
                "type": "web_search_20250305",
                "name": "web_search",
                "max_uses": max_uses,
            }
            user_location = web_search_opts.get("user_location")
            if isinstance(user_location, dict):
                ws_tool["user_location"] = user_location
            if "tools" not in result:
                result["tools"] = []
            result["tools"].append(ws_tool)

        # 恢复 Claude 特有字段（如 metadata）
        claude_extra = internal.extra.get("claude") if isinstance(internal.extra, dict) else None
        if isinstance(claude_extra, dict):
            if "metadata" in claude_extra:
                result["metadata"] = claude_extra["metadata"]

        return result

    # =========================
    # Responses
    # =========================

    def response_to_internal(self, response: dict[str, Any]) -> InternalResponse:
        rid = str(response.get("id") or "")
        model = str(response.get("model") or "")

        blocks, dropped = self._claude_content_to_blocks(response.get("content"))

        raw_stop = response.get("stop_reason")
        stop_reason: StopReason | None = None
        if raw_stop is not None:
            stop_reason = self._CLAUDE_STOP_TO_INTERNAL.get(str(raw_stop), StopReason.UNKNOWN)

        usage_info = self._claude_usage_to_internal(response.get("usage"))

        extra: dict[str, Any] = {}
        if raw_stop is not None:
            extra.setdefault("raw", {})["stop_reason"] = raw_stop

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
        cid = internal.id or "unknown"
        if not cid.startswith("msg_"):
            cid = f"msg_{cid}"

        content: list[dict[str, Any]] = []
        for b in internal.content:
            if isinstance(b, ThinkingBlock):
                if b.thinking:
                    thinking_block: dict[str, Any] = {
                        "type": "thinking",
                        "thinking": b.thinking,
                    }
                    if b.signature:
                        thinking_block["signature"] = b.signature
                    content.append(thinking_block)
                continue
            if isinstance(b, TextBlock):
                if b.text:
                    content.append({"type": "text", "text": b.text})
                continue
            if isinstance(b, ToolUseBlock):
                content.append(
                    {
                        "type": "tool_use",
                        "id": b.tool_id,
                        "name": b.tool_name,
                        "input": b.tool_input or {},
                    }
                )
                continue
            if isinstance(b, ImageBlock):
                if b.data and b.media_type:
                    content.append(
                        {
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": b.media_type,
                                "data": b.data,
                            },
                        }
                    )
                elif b.url:
                    content.append(
                        {
                            "type": "image",
                            "source": {"type": "url", "url": b.url},
                        }
                    )
                continue
            if isinstance(b, FileBlock):
                if b.data and b.media_type:
                    content.append(
                        {
                            "type": "document",
                            "source": {
                                "type": "base64",
                                "media_type": b.media_type,
                                "data": b.data,
                            },
                        }
                    )
                elif b.file_url:
                    content.append(
                        {
                            "type": "document",
                            "source": {"type": "url", "url": b.file_url},
                        }
                    )
                continue
            if isinstance(b, AudioBlock):
                if b.data and b.media_type:
                    content.append(
                        {
                            "type": "document",
                            "source": {
                                "type": "base64",
                                "media_type": b.media_type,
                                "data": b.data,
                            },
                        }
                    )
                continue
            # Unknown/ToolResult 默认丢弃

        stop_reason = None
        if internal.stop_reason is not None:
            stop_reason = STOP_REASON_MAPPINGS.get("CLAUDE", {}).get(
                internal.stop_reason.value, "end_turn"
            )

        usage: dict[str, Any] = {"input_tokens": 0, "output_tokens": 0}
        if internal.usage:
            usage = {
                "input_tokens": int(internal.usage.input_tokens),
                "output_tokens": int(internal.usage.output_tokens),
            }
            if internal.usage.cache_read_tokens:
                usage["cache_read_input_tokens"] = int(internal.usage.cache_read_tokens)
            if internal.usage.cache_write_tokens:
                usage["cache_creation_input_tokens"] = int(internal.usage.cache_write_tokens)

        # 优先使用用户请求的原始模型名，回退到上游返回的模型名
        model_name = requested_model if requested_model else internal.model

        return {
            "id": cid,
            "type": "message",
            "role": "assistant",
            "model": model_name,
            "content": content,
            "stop_reason": stop_reason,
            "stop_sequence": None,
            "usage": usage,
        }

    # =========================
    # Streaming (Claude SSE events)
    # =========================

    def stream_chunk_to_internal(
        self,
        chunk: dict[str, Any],
        state: StreamState,
    ) -> list[InternalStreamEvent]:
        ss = state.substate(self.FORMAT_ID)
        events: list[InternalStreamEvent] = []

        event_type = chunk.get("type")
        if event_type is None:
            return events
        event_type = str(event_type)

        if event_type == "ping":
            return events

        if event_type == "message_start":
            message_raw = chunk.get("message")
            message: dict[str, Any] = message_raw if isinstance(message_raw, dict) else {}
            msg_id = str(message.get("id") or "")
            usage_info = self._claude_usage_to_internal(message.get("usage"))
            # 保留初始化时设置的 model（客户端请求的模型），仅在空时用上游值
            model = state.model or str(message.get("model") or "")
            state.message_id = msg_id or state.message_id
            if not state.model:
                state.model = model
            ss["message_started"] = True
            ss.setdefault("block_index_to_tool_id", {})
            if usage_info is not None:
                ss["usage"] = message.get("usage")
            events.append(MessageStartEvent(message_id=msg_id, model=model, usage=usage_info))
            return events

        if event_type == "content_block_start":
            index = int(chunk.get("index") or 0)
            block_raw = chunk.get("content_block")
            block: dict[str, Any] = block_raw if isinstance(block_raw, dict) else {}
            btype = str(block.get("type") or "unknown")

            if btype == "thinking":
                events.append(
                    ContentBlockStartEvent(block_index=index, block_type=ContentType.THINKING)
                )
                return events

            if btype == "text":
                events.append(
                    ContentBlockStartEvent(block_index=index, block_type=ContentType.TEXT)
                )
                return events

            if btype == "tool_use":
                tool_id = str(block.get("id") or "")
                tool_name = str(block.get("name") or "")
                mapping = ss.get("block_index_to_tool_id")
                if isinstance(mapping, dict):
                    mapping[index] = tool_id
                events.append(
                    ContentBlockStartEvent(
                        block_index=index,
                        block_type=ContentType.TOOL_USE,
                        tool_id=tool_id or None,
                        tool_name=tool_name or None,
                    )
                )
                return events

            events.append(
                ContentBlockStartEvent(
                    block_index=index,
                    block_type=ContentType.UNKNOWN,
                    extra={"raw": {"claude_block_type": btype, "content_block": block}},
                )
            )
            return events

        if event_type == "content_block_delta":
            index = int(chunk.get("index") or 0)
            delta_raw = chunk.get("delta")
            delta: dict[str, Any] = delta_raw if isinstance(delta_raw, dict) else {}
            dtype = str(delta.get("type") or "unknown")

            if dtype == "thinking_delta":
                thinking = delta.get("thinking")
                if thinking is None:
                    return events
                events.append(ContentDeltaEvent(block_index=index, text_delta=str(thinking)))
                return events

            if dtype == "signature_delta":
                sig = delta.get("signature")
                if isinstance(sig, str) and sig:
                    events.append(
                        ContentDeltaEvent(
                            block_index=index,
                            text_delta="",
                            extra={"thought_signature": sig},
                        )
                    )
                return events

            if dtype == "text_delta":
                text = delta.get("text")
                if text is None:
                    return events
                events.append(ContentDeltaEvent(block_index=index, text_delta=str(text)))
                return events

            if dtype == "input_json_delta":
                partial = delta.get("partial_json")
                if partial is None:
                    return events
                mapping = ss.get("block_index_to_tool_id")
                tool_id = ""
                if isinstance(mapping, dict):
                    tool_id = str(mapping.get(index) or "")
                events.append(
                    ToolCallDeltaEvent(block_index=index, tool_id=tool_id, input_delta=str(partial))
                )
                return events

            return events

        if event_type == "content_block_stop":
            index = int(chunk.get("index") or 0)
            events.append(ContentBlockStopEvent(block_index=index))
            return events

        if event_type == "message_delta":
            delta_raw2 = chunk.get("delta")
            delta2: dict[str, Any] = delta_raw2 if isinstance(delta_raw2, dict) else {}
            raw_stop = delta2.get("stop_reason")
            if raw_stop is not None:
                ss["stop_reason"] = str(raw_stop)
            usage = chunk.get("usage")
            if isinstance(usage, dict):
                ss["usage"] = usage
            return events

        if event_type == "message_stop":
            raw_stop = ss.get("stop_reason")
            stop_reason: StopReason | None = None
            if raw_stop is not None:
                stop_reason = self._CLAUDE_STOP_TO_INTERNAL.get(str(raw_stop), StopReason.UNKNOWN)
            usage_info = self._claude_usage_to_internal(ss.get("usage"))
            events.append(MessageStopEvent(stop_reason=stop_reason, usage=usage_info))
            return events

        if event_type == "error":
            internal_error = self.error_to_internal(chunk)
            events.append(ErrorEvent(error=internal_error))
            return events

        return events

    def stream_event_from_internal(
        self,
        event: InternalStreamEvent,
        state: StreamState,
    ) -> list[dict[str, Any]]:
        ss = state.substate(self.FORMAT_ID)
        out: list[dict[str, Any]] = []

        if isinstance(event, MessageStartEvent):
            state.message_id = event.message_id or state.message_id
            # 保留初始化时设置的 model（客户端请求的模型），仅在空时用事件值
            if not state.model:
                state.model = event.model or ""
            ss.setdefault("block_index_to_tool_id", {})
            message_obj: dict[str, Any] = {
                "id": state.message_id or "msg_stream",
                "type": "message",
                "role": "assistant",
                "model": state.model or "",
                "content": [],
                "stop_reason": None,
                "stop_sequence": None,
            }
            # Claude CLI 客户端对接口格式要求严格，必须包含 usage 字段
            # 即使没有 usage 信息也要提供默认值
            if event.usage:
                message_obj["usage"] = self._usage_to_claude(event.usage)
            else:
                message_obj["usage"] = {"input_tokens": 0, "output_tokens": 0}
            out.append({"type": "message_start", "message": message_obj})
            return out

        if isinstance(event, ContentBlockStartEvent):
            # 记录 block_index → block_type 映射（用于 ContentDeltaEvent 区分类型）
            ss[f"block_type_{event.block_index}"] = event.block_type.value

            if event.block_type == ContentType.THINKING:
                # 对齐 AM Claude streaming.rs：thinking content block
                out.append(
                    {
                        "type": "content_block_start",
                        "index": int(event.block_index),
                        "content_block": {"type": "thinking", "thinking": ""},
                    }
                )
                return out

            if event.block_type == ContentType.TEXT:
                out.append(
                    {
                        "type": "content_block_start",
                        "index": int(event.block_index),
                        "content_block": {"type": "text", "text": ""},
                    }
                )
                return out

            if event.block_type == ContentType.TOOL_USE:
                tool_id = event.tool_id or ""
                tool_name = event.tool_name or ""
                mapping = ss.get("block_index_to_tool_id")
                if isinstance(mapping, dict):
                    mapping[int(event.block_index)] = tool_id
                out.append(
                    {
                        "type": "content_block_start",
                        "index": int(event.block_index),
                        "content_block": {"type": "tool_use", "id": tool_id, "name": tool_name},
                    }
                )
                return out

            return out

        if isinstance(event, ContentDeltaEvent):
            if event.text_delta:
                block_type = ss.get(f"block_type_{event.block_index}")
                if block_type == ContentType.THINKING.value:
                    # 对齐 AM：thinking_delta
                    out.append(
                        {
                            "type": "content_block_delta",
                            "index": int(event.block_index),
                            "delta": {"type": "thinking_delta", "thinking": event.text_delta},
                        }
                    )
                else:
                    out.append(
                        {
                            "type": "content_block_delta",
                            "index": int(event.block_index),
                            "delta": {"type": "text_delta", "text": event.text_delta},
                        }
                    )
            # 对齐 AM：signature_delta（如果 extra 中有签名信息）
            sig = event.extra.get("thought_signature") if event.extra else None
            if isinstance(sig, str) and sig:
                out.append(
                    {
                        "type": "content_block_delta",
                        "index": int(event.block_index),
                        "delta": {"type": "signature_delta", "signature": sig},
                    }
                )
            return out

        if isinstance(event, ToolCallDeltaEvent):
            if event.input_delta:
                out.append(
                    {
                        "type": "content_block_delta",
                        "index": int(event.block_index),
                        "delta": {"type": "input_json_delta", "partial_json": event.input_delta},
                    }
                )
            return out

        if isinstance(event, ContentBlockStopEvent):
            out.append({"type": "content_block_stop", "index": int(event.block_index)})
            return out

        if isinstance(event, MessageStopEvent):
            stop_reason = None
            if event.stop_reason is not None:
                stop_reason = STOP_REASON_MAPPINGS.get("CLAUDE", {}).get(
                    event.stop_reason.value, "end_turn"
                )

            msg_delta: dict[str, Any] = {
                "type": "message_delta",
                "delta": {"stop_reason": stop_reason},
            }

            if event.usage:
                msg_delta["usage"] = self._usage_to_claude(event.usage)

            out.append(msg_delta)
            out.append({"type": "message_stop"})
            return out

        if isinstance(event, ErrorEvent):
            out.append(self.error_from_internal(event.error))
            return out

        return out

    # =========================
    # Error conversion
    # =========================

    def is_error_response(self, response: dict[str, Any]) -> bool:
        if not isinstance(response, dict):
            return False
        if response.get("type") == "error":
            return True
        return "error" in response

    def error_to_internal(self, error_response: dict[str, Any]) -> InternalError:
        err: dict[str, Any] = {}
        if isinstance(error_response, dict):
            err_raw = error_response.get("error")
            err = err_raw if isinstance(err_raw, dict) else {}

        raw_type = err.get("type")
        mapped = ERROR_TYPE_MAPPINGS.get("CLAUDE", {}).get(str(raw_type), ErrorType.UNKNOWN.value)
        internal_type = self._error_type_from_value(mapped)
        retryable = internal_type.value in RETRYABLE_ERROR_TYPES

        return InternalError(
            type=internal_type,
            message=str(err.get("message") or ""),
            code=err.get("code") if err.get("code") is None else str(err.get("code")),
            param=err.get("param") if err.get("param") is None else str(err.get("param")),
            retryable=retryable,
            extra={"claude": {"error": err}, "raw": {"type": raw_type}},
        )

    def error_from_internal(self, internal: InternalError) -> dict[str, Any]:
        type_str = self._ERROR_TYPE_TO_CLAUDE.get(internal.type, "api_error")
        payload: dict[str, Any] = {"type": type_str, "message": internal.message}
        if internal.param is not None:
            payload["param"] = internal.param
        if internal.code is not None:
            payload["code"] = internal.code
        return {"type": "error", "error": payload}

    # =========================
    # Helpers
    # =========================

    def _claude_message_to_internal(
        self, msg: dict[str, Any]
    ) -> tuple[InternalMessage | None, dict[str, int]]:
        dropped: dict[str, int] = {}
        role_raw = str(msg.get("role") or "unknown")

        if role_raw == "user":
            role = Role.USER
        elif role_raw == "assistant":
            role = Role.ASSISTANT
        else:
            role = Role.UNKNOWN

        blocks, bd = self._claude_content_to_blocks(msg.get("content"))
        self._merge_dropped(dropped, bd)

        return (
            InternalMessage(
                role=role,
                content=blocks,
                extra=self._extract_extra(msg, {"role", "content"}),
            ),
            dropped,
        )

    def _claude_content_to_blocks(self, content: Any) -> tuple[list[ContentBlock], dict[str, int]]:
        dropped: dict[str, int] = {}
        if content is None:
            return [], dropped
        if isinstance(content, str):
            return ([TextBlock(text=content)] if content else []), dropped
        if not isinstance(content, list):
            dropped["claude_content_non_list"] = dropped.get("claude_content_non_list", 0) + 1
            return [], dropped

        blocks: list[ContentBlock] = []
        for block in content:
            if not isinstance(block, dict):
                dropped["claude_block_non_dict"] = dropped.get("claude_block_non_dict", 0) + 1
                continue

            btype = str(block.get("type") or "unknown")
            if btype == "thinking":
                thinking_raw = block.get("thinking")
                signature = block.get("signature")

                if isinstance(thinking_raw, str) and thinking_raw:
                    blocks.append(
                        ThinkingBlock(
                            thinking=thinking_raw,
                            signature=str(signature) if signature else None,
                        )
                    )
                else:
                    dropped_key = f"claude_block:{btype}"
                    dropped[dropped_key] = dropped.get(dropped_key, 0) + 1
                    blocks.append(UnknownBlock(raw_type=btype, payload=block))

                continue

            if btype == "text":
                text = str(block.get("text") or "")
                if text:
                    block_extra = self._extract_extra(block, {"type", "text"})
                    # cache_control 透传
                    cc = block.get("cache_control")
                    if isinstance(cc, dict):
                        block_extra["cache_control"] = cc
                    blocks.append(TextBlock(text=text, extra=block_extra))
                continue

            if btype == "image":
                src_raw = block.get("source")
                src: dict[str, Any] = src_raw if isinstance(src_raw, dict) else {}
                stype = src.get("type")
                if stype == "base64":
                    data = src.get("data")
                    media_type = src.get("media_type")
                    if (
                        isinstance(data, str)
                        and data
                        and isinstance(media_type, str)
                        and media_type
                    ):
                        blocks.append(ImageBlock(data=data, media_type=media_type))
                        continue
                elif stype == "url":
                    url = src.get("url")
                    if isinstance(url, str) and url:
                        blocks.append(ImageBlock(url=url))
                        continue
                dropped["claude_image_unsupported"] = dropped.get("claude_image_unsupported", 0) + 1
                blocks.append(UnknownBlock(raw_type="image", payload=block))
                continue

            if btype == "document":
                doc_src = block.get("source") or {}
                if isinstance(doc_src, dict):
                    doc_stype = doc_src.get("type")
                    if doc_stype == "base64":
                        blocks.append(
                            FileBlock(
                                data=doc_src.get("data"),
                                media_type=doc_src.get("media_type"),
                                extra=self._extract_extra(block, {"type", "source"}),
                            )
                        )
                        continue
                    elif doc_stype == "url":
                        blocks.append(
                            FileBlock(
                                file_url=doc_src.get("url"),
                                media_type=doc_src.get("media_type"),
                                extra=self._extract_extra(block, {"type", "source"}),
                            )
                        )
                        continue
                dropped_key = f"claude_block:{btype}"
                dropped[dropped_key] = dropped.get(dropped_key, 0) + 1
                blocks.append(UnknownBlock(raw_type=btype, payload=block))
                continue

            if btype == "tool_use":
                tool_id = str(block.get("id") or "")
                tool_name = str(block.get("name") or "")
                tool_input = block.get("input")
                if not isinstance(tool_input, dict):
                    tool_input = {"raw": tool_input}
                blocks.append(
                    ToolUseBlock(
                        tool_id=tool_id,
                        tool_name=tool_name,
                        tool_input=tool_input,
                        extra={
                            "claude": self._extract_extra(block, {"type", "id", "name", "input"})
                        },
                    )
                )
                continue

            if btype == "tool_result":
                tool_use_id = str(block.get("tool_use_id") or "")
                is_error = bool(block.get("is_error") or False)
                raw_content = block.get("content")
                blocks.append(
                    self._tool_result_from_claude(tool_use_id, raw_content, is_error, block)
                )
                continue

            dropped_key = f"claude_block:{btype}"
            dropped[dropped_key] = dropped.get(dropped_key, 0) + 1
            blocks.append(UnknownBlock(raw_type=btype, payload=block))

        return blocks, dropped

    def _tool_result_from_claude(
        self,
        tool_use_id: str,
        raw_content: Any,
        is_error: bool,
        raw_block: dict[str, Any],
    ) -> ToolResultBlock:
        if raw_content is None:
            return ToolResultBlock(
                tool_use_id=tool_use_id,
                output=None,
                content_text=None,
                is_error=is_error,
                extra={"claude": raw_block},
            )

        if isinstance(raw_content, str):
            parsed: Any = None
            try:
                parsed = json.loads(raw_content)
            except json.JSONDecodeError:
                parsed = None

            if parsed is not None:
                return ToolResultBlock(
                    tool_use_id=tool_use_id,
                    output=parsed,
                    content_text=None,
                    is_error=is_error,
                    extra={"raw": {"content": raw_content}, "claude": raw_block},
                )

            return ToolResultBlock(
                tool_use_id=tool_use_id,
                output=None,
                content_text=raw_content,
                is_error=is_error,
                extra={"claude": raw_block},
            )

        if isinstance(raw_content, list):
            text_parts: list[str] = []
            for part in raw_content:
                if isinstance(part, dict) and part.get("type") == "text":
                    text = part.get("text")
                    if text:
                        text_parts.append(str(text))
            collapsed = "\n\n".join(text_parts) if text_parts else None

            return ToolResultBlock(
                tool_use_id=tool_use_id,
                output=None,
                content_text=collapsed,
                is_error=is_error,
                extra={"raw": {"content": raw_content}, "claude": raw_block},
            )

        return ToolResultBlock(
            tool_use_id=tool_use_id,
            output=raw_content,
            content_text=None,
            is_error=is_error,
            extra={"claude": raw_block},
        )

    def _collapse_claude_system(
        self, system_value: Any
    ) -> tuple[str | None, dict[str, int], list[InstructionSegment] | None]:
        """解析 Claude system 字段。返回 (text, dropped, segments)。
        segments 仅在 system 为数组且含 cache_control 时非空。"""
        dropped: dict[str, int] = {}
        if system_value is None:
            return None, dropped, None
        if isinstance(system_value, str):
            return (system_value or None), dropped, None

        if isinstance(system_value, list):
            texts: list[str] = []
            segments: list[InstructionSegment] = []
            has_cache_control = False
            for item in system_value:
                if not isinstance(item, dict):
                    dropped["claude_system_item_non_dict"] = (
                        dropped.get("claude_system_item_non_dict", 0) + 1
                    )
                    continue
                if item.get("type") == "text":
                    text = item.get("text")
                    if text:
                        texts.append(str(text))
                        seg_extra: dict[str, Any] = {}
                        cc = item.get("cache_control")
                        if isinstance(cc, dict):
                            seg_extra["cache_control"] = cc
                            has_cache_control = True
                        segments.append(
                            InstructionSegment(role=Role.SYSTEM, text=str(text), extra=seg_extra)
                        )
                else:
                    dropped_key = f"claude_system_item:{item.get('type')}"
                    dropped[dropped_key] = dropped.get(dropped_key, 0) + 1
            joined = "\n\n".join(texts)
            return (
                (joined or None),
                dropped,
                segments if has_cache_control else None,
            )

        dropped["claude_system_unsupported"] = dropped.get("claude_system_unsupported", 0) + 1
        return None, dropped, None

    def _join_instructions(self, instructions: list[InstructionSegment]) -> str | None:
        parts = [seg.text for seg in instructions if seg.text]
        joined = "\n\n".join(parts)
        return joined or None

    def _claude_tools_to_internal(self, tools: Any) -> list[ToolDefinition] | None:
        if not tools or not isinstance(tools, list):
            return None

        out: list[ToolDefinition] = []
        for tool in tools:
            if not isinstance(tool, dict):
                continue
            name = str(tool.get("name") or "")
            if not name:
                continue
            out.append(
                ToolDefinition(
                    name=name,
                    description=tool.get("description"),
                    parameters=(
                        tool.get("input_schema")
                        if isinstance(tool.get("input_schema"), dict)
                        else None
                    ),
                    extra={
                        "claude": self._extract_extra(tool, {"name", "description", "input_schema"})
                    },
                )
            )
        return out or None

    def _claude_tool_choice_to_internal(self, tool_choice: Any) -> ToolChoice | None:
        if tool_choice is None:
            return None
        if not isinstance(tool_choice, dict):
            return ToolChoice(type=ToolChoiceType.AUTO, extra={"raw": tool_choice})

        ctype = str(tool_choice.get("type") or "auto")
        if ctype == "none":
            return ToolChoice(type=ToolChoiceType.NONE, extra={"claude": tool_choice})
        if ctype == "auto":
            return ToolChoice(type=ToolChoiceType.AUTO, extra={"claude": tool_choice})
        if ctype in ("any", "required"):
            return ToolChoice(type=ToolChoiceType.REQUIRED, extra={"claude": tool_choice})
        if ctype in ("tool_use", "tool"):
            name = str(tool_choice.get("name") or "")
            return ToolChoice(
                type=ToolChoiceType.TOOL, tool_name=name, extra={"claude": tool_choice}
            )

        return ToolChoice(type=ToolChoiceType.AUTO, extra={"claude": tool_choice})

    def _tool_choice_to_claude(self, tool_choice: ToolChoice) -> dict[str, Any]:
        if tool_choice.type == ToolChoiceType.NONE:
            return {"type": "none"}
        if tool_choice.type == ToolChoiceType.AUTO:
            return {"type": "auto"}
        if tool_choice.type == ToolChoiceType.REQUIRED:
            return {"type": "any"}
        if tool_choice.type == ToolChoiceType.TOOL:
            return {"type": "tool", "name": tool_choice.tool_name or ""}
        return {"type": "auto"}

    def _internal_message_to_claude(self, msg: InternalMessage) -> dict[str, Any]:
        role = "user" if msg.role == Role.USER else "assistant"

        # 预扫描：如果任一 TextBlock 带 cache_control，所有 TextBlock 都走结构化路径以保持顺序
        force_structured_text = any(
            isinstance(b, TextBlock) and b.extra.get("cache_control") for b in msg.content
        )

        blocks: list[dict[str, Any]] = []
        text_parts: list[str] = []

        for b in msg.content:
            if isinstance(b, UnknownBlock):
                continue

            if isinstance(b, TextBlock):
                if b.text:
                    if force_structured_text:
                        text_block: dict[str, Any] = {"type": "text", "text": b.text}
                        cc = b.extra.get("cache_control") if b.extra else None
                        if isinstance(cc, dict):
                            text_block["cache_control"] = cc
                        blocks.append(text_block)
                    else:
                        text_parts.append(b.text)
                continue

            if isinstance(b, ImageBlock):
                if role != "user":
                    if b.url:
                        text_parts.append(f"[Image: {b.url}]")
                    elif b.media_type and b.data:
                        text_parts.append("[Image]")
                    continue

                if b.data and b.media_type:
                    blocks.append(
                        {
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": b.media_type,
                                "data": b.data,
                            },
                        }
                    )
                elif b.url:
                    blocks.append(
                        {
                            "type": "image",
                            "source": {"type": "url", "url": b.url},
                        }
                    )
                continue

            if isinstance(b, FileBlock):
                if b.data and b.media_type:
                    blocks.append(
                        {
                            "type": "document",
                            "source": {
                                "type": "base64",
                                "media_type": b.media_type,
                                "data": b.data,
                            },
                        }
                    )
                elif b.file_url:
                    blocks.append(
                        {
                            "type": "document",
                            "source": {
                                "type": "url",
                                "url": b.file_url,
                            },
                        }
                    )
                elif b.file_id:
                    text_parts.append(f"[File: {b.file_id}]")
                continue

            if isinstance(b, AudioBlock):
                if b.data and b.media_type:
                    blocks.append(
                        {
                            "type": "document",
                            "source": {
                                "type": "base64",
                                "media_type": b.media_type,
                                "data": b.data,
                            },
                        }
                    )
                continue

            if isinstance(b, ToolUseBlock) and role == "assistant":
                blocks.append(
                    {
                        "type": "tool_use",
                        "id": b.tool_id,
                        "name": b.tool_name,
                        "input": b.tool_input or {},
                    }
                )
                continue

            if isinstance(b, ToolResultBlock) and role == "user":
                if b.content_text is not None:
                    content: Any = b.content_text
                elif b.output is None:
                    content = ""
                elif isinstance(b.output, str):
                    content = b.output
                else:
                    content = b.output

                blocks.append(
                    {
                        "type": "tool_result",
                        "tool_use_id": b.tool_use_id,
                        "content": content,
                        "is_error": bool(b.is_error),
                    }
                )
                continue

        if text_parts:
            if blocks:
                blocks = [{"type": "text", "text": "\n".join(text_parts)}] + blocks
            else:
                return {"role": role, "content": "\n".join(text_parts)}

        return {"role": role, "content": blocks}

    def _coerce_claude_message_sequence(
        self, messages: list[InternalMessage]
    ) -> list[InternalMessage]:
        normalized: list[InternalMessage] = []
        for m in messages:
            role = m.role
            if role not in (Role.USER, Role.ASSISTANT):
                role = Role.USER
            normalized.append(InternalMessage(role=role, content=m.content, extra=m.extra))

        if not normalized:
            return []

        if normalized[0].role != Role.USER:
            normalized = [InternalMessage(role=Role.USER, content=[])] + normalized

        merged: list[InternalMessage] = []
        for m in normalized:
            if merged and merged[-1].role == m.role:
                merged[-1].content.extend(m.content)
                continue
            merged.append(m)

        return merged

    def _claude_usage_to_internal(self, usage: Any) -> UsageInfo | None:
        if not isinstance(usage, dict):
            return None

        mapping = USAGE_FIELD_MAPPINGS.get("CLAUDE", {})
        fields: dict[str, int] = {}
        extra = self._extract_extra(
            usage,
            set(mapping.keys())
            | {"cache_creation_input_tokens_5m", "cache_creation_input_tokens_1h"},
        )

        for provider_key, internal_key in mapping.items():
            if provider_key in usage and usage.get(provider_key) is not None:
                try:
                    fields[internal_key] = int(usage.get(provider_key) or 0)
                except (TypeError, ValueError):
                    continue

        if "total_tokens" not in fields:
            fields["total_tokens"] = int(
                fields.get("input_tokens", 0) + fields.get("output_tokens", 0)
            )

        # Claude cache_creation 5m/1h 细分（存入 extra）
        cache_details: dict[str, int] = {}
        for detail_key in (
            "cache_creation_input_tokens_5m",
            "cache_creation_input_tokens_1h",
        ):
            val = usage.get(detail_key)
            if val is not None:
                try:
                    cache_details[detail_key] = int(val)
                except (TypeError, ValueError):
                    pass
        if cache_details:
            extra["cache_creation_details"] = cache_details

        return UsageInfo(
            input_tokens=int(fields.get("input_tokens", 0)),
            output_tokens=int(fields.get("output_tokens", 0)),
            total_tokens=int(fields.get("total_tokens", 0)),
            cache_read_tokens=int(fields.get("cache_read_tokens", 0)),
            cache_write_tokens=int(fields.get("cache_write_tokens", 0)),
            extra={"claude": extra} if extra else {},
        )

    def _usage_to_claude(self, usage: UsageInfo) -> dict[str, Any]:
        result: dict[str, Any] = {
            "input_tokens": int(usage.input_tokens),
            "output_tokens": int(usage.output_tokens),
        }
        if usage.cache_read_tokens:
            result["cache_read_input_tokens"] = int(usage.cache_read_tokens)
        if usage.cache_write_tokens:
            result["cache_creation_input_tokens"] = int(usage.cache_write_tokens)
        # 回写 5m/1h 细分
        claude_extra = usage.extra.get("claude", {})
        if isinstance(claude_extra, dict):
            cache_details = claude_extra.get("cache_creation_details")
            if isinstance(cache_details, dict):
                for k, v in cache_details.items():
                    try:
                        result[k] = int(v)
                    except (TypeError, ValueError):
                        pass
        return result

    def _error_type_from_value(self, value: str) -> ErrorType:
        try:
            return ErrorType(value)
        except ValueError:
            return ErrorType.UNKNOWN


__all__ = ["ClaudeNormalizer"]
