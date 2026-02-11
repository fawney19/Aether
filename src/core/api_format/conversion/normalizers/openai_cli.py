"""
OpenAI CLI / Responses Normalizer (OPENAI_CLI)

目标：
- 将 OpenAI Responses API（/v1/responses）映射到 InternalRequest / InternalResponse
- 支持流式事件：response.output_text.delta / response.completed 等

说明：
- 这里实现的是“最佳努力”的最小可用映射，重点覆盖文本与 usage。
- 未识别的字段会进入 extra/raw，未知内容块保留在 internal，但默认输出阶段会丢弃。
"""

import time
from collections.abc import Callable
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
    UnknownStreamEvent,
)
from src.core.api_format.conversion.stream_state import StreamState
from src.utils import json_helper as json


class OpenAICliNormalizer(FormatNormalizer):
    FORMAT_ID = "openai:cli"
    capabilities = FormatCapabilities(
        supports_stream=True,
        supports_error_conversion=True,
        supports_tools=True,
        supports_images=True,
    )

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

        instructions_text = request.get("instructions")
        instructions: list[InstructionSegment] = []
        system_text: str | None = None
        if isinstance(instructions_text, str) and instructions_text.strip():
            system_text = instructions_text
            instructions.append(InstructionSegment(role=Role.SYSTEM, text=instructions_text))

        messages = self._input_to_internal_messages(request.get("input"))

        tools = self._tools_to_internal(request.get("tools"))
        tool_choice = self._tool_choice_to_internal(request.get("tool_choice"))

        max_tokens = self._optional_int(request.get("max_output_tokens", request.get("max_tokens")))

        internal = InternalRequest(
            model=model,
            messages=messages,
            instructions=instructions,
            system=system_text,
            max_tokens=max_tokens,
            temperature=self._optional_float(request.get("temperature")),
            top_p=self._optional_float(request.get("top_p")),
            stop_sequences=self._coerce_str_list(request.get("stop")),
            stream=bool(request.get("stream") or False),
            tools=tools,
            tool_choice=tool_choice,
            extra={"openai_cli": self._extract_extra(request, {"input"})},
        )

        return internal

    # Codex 需要的 include 项
    _CODEX_REQUIRED_INCLUDE = "reasoning.encrypted_content"

    def request_from_internal(
        self,
        internal: InternalRequest,
        *,
        target_variant: str | None = None,
    ) -> dict[str, Any]:
        is_codex = str(target_variant or "").lower() == "codex"

        result: dict[str, Any] = {
            "model": internal.model,
            "input": self._internal_messages_to_input(
                internal.messages, system_to_developer=is_codex
            ),
        }

        # 合并 instructions，如果没有则使用 system
        instructions_text = (
            self._join_instructions(internal.instructions)
            if internal.instructions
            else internal.system
        )
        # Responses API 兼容 instructions 字段，Codex 强制要求
        # 统一添加该字段以确保兼容性
        result["instructions"] = instructions_text or ""

        # max_output_tokens/temperature/top_p: Codex 不支持，标准 API 可选
        if not is_codex:
            if internal.max_tokens is not None:
                # Responses API 使用 max_output_tokens
                result["max_output_tokens"] = internal.max_tokens
            if internal.temperature is not None:
                result["temperature"] = internal.temperature
            if internal.top_p is not None:
                result["top_p"] = internal.top_p

        if internal.stop_sequences:
            result["stop"] = list(internal.stop_sequences)
        # Codex 强制要求 stream=true；其他情况尊重客户端请求
        result["stream"] = True if is_codex else bool(internal.stream)

        if internal.tools:
            # Responses API 使用扁平结构: {type, name, description, parameters}
            # 而非 Chat Completions 的嵌套结构: {type, function: {name, ...}}
            result["tools"] = [
                {
                    "type": "function",
                    "name": t.name,
                    "description": t.description or "",
                    "parameters": t.parameters or {},
                    **(t.extra.get("openai_tool") or {}),
                }
                for t in internal.tools
            ]

        if internal.tool_choice:
            result["tool_choice"] = self._tool_choice_to_openai(internal.tool_choice)

        # 还原 OpenAI Responses API 的其他字段（黑名单：已单独处理的字段不还原）
        openai_cli_extra = internal.extra.get("openai_cli", {})
        handled_keys = {
            "model",
            "input",
            "instructions",
            "max_output_tokens",
            "max_tokens",
            "temperature",
            "top_p",
            "stop",
            "stream",
            "tools",
            "tool_choice",
        }
        for key, value in openai_cli_extra.items():
            if key not in handled_keys and key not in result:
                result[key] = value

        # 统一设置 store=false（Codex 强制要求，标准 API 兼容）
        if "store" not in result:
            result["store"] = False

        # Codex 特定设置（覆盖/删除不支持的字段）
        if is_codex:
            result["parallel_tool_calls"] = True
            # 添加 reasoning.encrypted_content 到 include
            include = result.get("include", [])
            if not isinstance(include, list):
                include = []
            if self._CODEX_REQUIRED_INCLUDE not in include:
                include.append(self._CODEX_REQUIRED_INCLUDE)
            result["include"] = include
            # 删除 Codex 不支持的字段
            for key in (
                "previous_response_id",
                "prompt_cache_key",
                "service_tier",
                "max_completion_tokens",
            ):
                result.pop(key, None)

        return result

    # =========================
    # Responses
    # =========================

    def response_to_internal(self, response: dict[str, Any]) -> InternalResponse:
        payload = self._unwrap_response_object(response)

        rid = str(payload.get("id") or "")
        model = str(payload.get("model") or "")

        blocks, extra = self._extract_output_text_blocks(payload)
        usage = self._usage_to_internal(payload.get("usage"))

        stop_reason = StopReason.UNKNOWN
        status = payload.get("status")
        if isinstance(status, str) and status == "completed":
            stop_reason = StopReason.END_TURN

        return InternalResponse(
            id=rid,
            model=model,
            content=blocks,
            stop_reason=stop_reason,
            usage=usage,
            extra=extra,
        )

    def response_from_internal(
        self,
        internal: InternalResponse,
        *,
        requested_model: str | None = None,
    ) -> dict[str, Any]:
        text = self._collapse_internal_text(internal.content)

        output_message = {
            "type": "message",
            "id": f"msg_{internal.id or 'stream'}",
            "role": "assistant",
            "content": [{"type": "output_text", "text": text}],
        }

        usage = internal.usage or UsageInfo()
        usage_obj: dict[str, Any] = {
            "input_tokens": usage.input_tokens,
            "output_tokens": usage.output_tokens,
            "total_tokens": usage.total_tokens or (usage.input_tokens + usage.output_tokens),
        }

        # 优先使用用户请求的原始模型名，回退到上游返回的模型名
        model_name = requested_model if requested_model else (internal.model or "")

        return {
            "id": internal.id or "resp",
            "object": "response",
            "created": int(time.time()),
            "model": model_name,
            "status": "completed",
            "output": [output_message],
            "usage": usage_obj,
        }

    # =========================
    # Stream conversion
    # =========================

    def stream_chunk_to_internal(
        self,
        chunk: dict[str, Any],
        state: StreamState,
    ) -> list[InternalStreamEvent]:
        ss = state.substate(self.FORMAT_ID)
        events: list[InternalStreamEvent] = []

        # 统一错误结构（最佳努力）
        if isinstance(chunk, dict) and "error" in chunk:
            try:
                events.append(ErrorEvent(error=self.error_to_internal(chunk)))
            except Exception:
                pass
            return events

        etype = str(chunk.get("type") or "")

        # 尽量在首次事件补齐 message_start
        if not ss.get("message_started"):
            resp_obj = chunk.get("response")
            resp_obj = resp_obj if isinstance(resp_obj, dict) else {}
            msg_id = str(resp_obj.get("id") or chunk.get("id") or state.message_id or "")
            # 保留初始化时设置的 model（客户端请求的模型），仅在空时用上游值
            model = state.model or str(resp_obj.get("model") or chunk.get("model") or "")
            if msg_id or model or etype:
                state.message_id = msg_id
                if not state.model:
                    state.model = model
                ss["message_started"] = True
                ss.setdefault("text_block_started", False)
                ss.setdefault("text_block_stopped", False)
                events.append(MessageStartEvent(message_id=msg_id, model=model))

        handler = self._CHUNK_HANDLERS.get(etype)
        if handler is not None:
            events.extend(handler(self, chunk, state, ss))
            return events

        # 未匹配的事件类型
        if etype:
            return events + [UnknownStreamEvent(raw_type=etype, payload=chunk)]
        return events + [UnknownStreamEvent(raw_type="unknown", payload=chunk)]

    def _handle_response_created(
        self, chunk: dict[str, Any], state: StreamState, ss: dict[str, Any]
    ) -> list[InternalStreamEvent]:
        # message_start 已在主方法中处理
        return []

    def _handle_output_text_delta(
        self, chunk: dict[str, Any], state: StreamState, ss: dict[str, Any]
    ) -> list[InternalStreamEvent]:
        events: list[InternalStreamEvent] = []
        delta = chunk.get("delta")
        delta_text = ""
        if isinstance(delta, str):
            delta_text = delta
        elif isinstance(delta, dict) and isinstance(delta.get("text"), str):
            delta_text = str(delta.get("text") or "")

        if delta_text:
            if not ss.get("text_block_started"):
                ss["text_block_started"] = True
                events.append(ContentBlockStartEvent(block_index=0, block_type=ContentType.TEXT))
            events.append(ContentDeltaEvent(block_index=0, text_delta=delta_text))
        return events

    def _handle_output_text_done(
        self, chunk: dict[str, Any], state: StreamState, ss: dict[str, Any]
    ) -> list[InternalStreamEvent]:
        events: list[InternalStreamEvent] = []
        if ss.get("text_block_started") and not ss.get("text_block_stopped"):
            ss["text_block_stopped"] = True
            events.append(ContentBlockStopEvent(block_index=0))
        return events

    def _handle_response_completed(
        self, chunk: dict[str, Any], state: StreamState, ss: dict[str, Any]
    ) -> list[InternalStreamEvent]:
        events: list[InternalStreamEvent] = []
        resp_obj = chunk.get("response")
        resp_obj = resp_obj if isinstance(resp_obj, dict) else {}
        usage = self._usage_to_internal(resp_obj.get("usage") or chunk.get("usage"))

        if ss.get("text_block_started") and not ss.get("text_block_stopped"):
            ss["text_block_stopped"] = True
            events.append(ContentBlockStopEvent(block_index=0))

        events.append(MessageStopEvent(stop_reason=StopReason.END_TURN, usage=usage))
        return events

    def _handle_response_failed(
        self, chunk: dict[str, Any], state: StreamState, ss: dict[str, Any]
    ) -> list[InternalStreamEvent]:
        events: list[InternalStreamEvent] = []
        try:
            events.append(ErrorEvent(error=self.error_to_internal(chunk)))
        except Exception:
            pass
        return events

    def _handle_response_in_progress(
        self, chunk: dict[str, Any], state: StreamState, ss: dict[str, Any]
    ) -> list[InternalStreamEvent]:
        # 更新 state 中的元数据（如果有）
        # 注意：model 保持初始值（客户端请求的模型），不被上游覆盖
        resp_obj = chunk.get("response")
        if isinstance(resp_obj, dict):
            if resp_obj.get("id"):
                state.message_id = str(resp_obj.get("id"))
            # 仅在 model 为空时才用上游值
            if not state.model and resp_obj.get("model"):
                state.model = str(resp_obj.get("model"))
        return []

    def _handle_output_item_added(
        self, chunk: dict[str, Any], state: StreamState, ss: dict[str, Any]
    ) -> list[InternalStreamEvent]:
        events: list[InternalStreamEvent] = []
        item = chunk.get("item")
        if isinstance(item, dict):
            item_type = item.get("type")
            if item_type == "function_call":
                if not ss.get("tool_block_started"):
                    ss["tool_block_started"] = True
                    ss["current_tool_id"] = item.get("call_id") or item.get("id") or ""
                    ss["current_tool_name"] = item.get("name") or ""
                    events.append(
                        ContentBlockStartEvent(
                            block_index=ss.get("block_index", 0),
                            block_type=ContentType.TOOL_USE,
                            extra={
                                "tool_id": ss["current_tool_id"],
                                "tool_name": ss["current_tool_name"],
                            },
                        )
                    )
                    ss["block_index"] = ss.get("block_index", 0) + 1
        return events

    def _handle_output_item_done(
        self, chunk: dict[str, Any], state: StreamState, ss: dict[str, Any]
    ) -> list[InternalStreamEvent]:
        events: list[InternalStreamEvent] = []
        item = chunk.get("item")
        if isinstance(item, dict):
            item_type = item.get("type")
            if item_type == "function_call" and ss.get("tool_block_started"):
                ss["tool_block_started"] = False
                events.append(ContentBlockStopEvent(block_index=ss.get("block_index", 1) - 1))
        return events

    def _handle_function_call_delta(
        self, chunk: dict[str, Any], state: StreamState, ss: dict[str, Any]
    ) -> list[InternalStreamEvent]:
        events: list[InternalStreamEvent] = []
        delta = chunk.get("delta") or ""
        if delta:
            events.append(
                ToolCallDeltaEvent(
                    block_index=ss.get("block_index", 1) - 1,
                    tool_id=ss.get("current_tool_id", ""),
                    input_delta=delta,
                )
            )
        return events

    def _handle_noop(
        self, chunk: dict[str, Any], state: StreamState, ss: dict[str, Any]
    ) -> list[InternalStreamEvent]:
        return []

    def _handle_as_unknown(
        self, chunk: dict[str, Any], state: StreamState, ss: dict[str, Any]
    ) -> list[InternalStreamEvent]:
        etype = str(chunk.get("type") or "unknown")
        return [UnknownStreamEvent(raw_type=etype, payload=chunk)]

    # 事件类型 -> 处理器映射表
    _CHUNK_HANDLERS: dict[str, Callable[..., list[InternalStreamEvent]]] = {
        "response.created": _handle_response_created,
        "response.output_text.delta": _handle_output_text_delta,
        "response.outtext.delta": _handle_output_text_delta,
        "response.output_text.done": _handle_output_text_done,
        "response.completed": _handle_response_completed,
        "response.failed": _handle_response_failed,
        "response.in_progress": _handle_response_in_progress,
        "response.output_item.added": _handle_output_item_added,
        "response.output_item.done": _handle_output_item_done,
        "response.function_call_arguments.delta": _handle_function_call_delta,
        "response.function_call_arguments.done": _handle_noop,
        "response.content_part.added": _handle_noop,
        "response.content_part.done": _handle_noop,
        "response.reasoning_summary_text.delta": _handle_as_unknown,
        "response.reasoning_summary_text.done": _handle_as_unknown,
    }

    def stream_event_from_internal(
        self,
        event: InternalStreamEvent,
        state: StreamState,
    ) -> list[dict[str, Any]]:
        ss = state.substate(self.FORMAT_ID)

        if isinstance(event, MessageStartEvent):
            return self._emit_message_start(event, state, ss)
        if isinstance(event, ContentBlockStartEvent):
            return self._emit_content_block_start(event, state, ss)
        if isinstance(event, ToolCallDeltaEvent):
            return self._emit_tool_call_delta(event, state, ss)
        if isinstance(event, ContentBlockStopEvent):
            return self._emit_content_block_stop(event, state, ss)
        if isinstance(event, ContentDeltaEvent):
            return self._emit_content_delta(event, state, ss)
        if isinstance(event, MessageStopEvent):
            return self._emit_message_stop(event, state, ss)
        if isinstance(event, ErrorEvent):
            return self._emit_error(event, state, ss)
        # 其他事件：Responses SSE 无直接对应，跳过
        return []

    def _emit_message_start(
        self, event: MessageStartEvent, state: StreamState, ss: dict[str, Any]
    ) -> list[dict[str, Any]]:
        out: list[dict[str, Any]] = []
        state.message_id = event.message_id or state.message_id or "resp_stream"
        # 保留初始化时设置的 model（客户端请求的模型），仅在空时用事件值
        if not state.model:
            state.model = event.model or ""
        ss.setdefault("collected_text", "")
        ss.setdefault("tool_calls", {})
        ss.setdefault("tool_blocks", {})
        ss.setdefault("tool_output_index", {})
        ss.setdefault("output_order", [])
        ss.setdefault("message_output_index", None)
        ss.setdefault("message_output_started", False)
        ss.setdefault("text_started", False)
        ss.setdefault("next_output_index", 0)
        ss.setdefault("sent_in_progress", False)
        response_obj = {
            "id": state.message_id,
            "object": "response",
            "created": int(time.time()),
            "model": state.model,
            "status": "in_progress",
            "output": [],
        }
        out.append({"type": "response.created", "response": response_obj})
        # OpenAI Responses API 常见的 in_progress 事件（可选，最佳努力）
        if not ss.get("sent_in_progress"):
            ss["sent_in_progress"] = True
            out.append({"type": "response.in_progress", "response": response_obj})
        return out

    def _emit_content_block_start(
        self, event: ContentBlockStartEvent, state: StreamState, ss: dict[str, Any]
    ) -> list[dict[str, Any]]:
        out: list[dict[str, Any]] = []

        # 记录 block_index → block_type 映射
        ss[f"block_type_{event.block_index}"] = event.block_type.value

        # Thinking block：Responses API 目前无 reasoning stream 事件
        if event.block_type == ContentType.THINKING:
            # thinking 内容静默收集，不输出（Responses API 无标准 reasoning 字段）
            ss.setdefault("thinking_text", "")
            return out

        # 工具调用块：输出 function_call 添加事件
        if event.block_type == ContentType.TOOL_USE:
            tool_id = event.tool_id or ""
            tool_name = event.tool_name or ""
            output_index = int(ss.get("next_output_index") or 0)
            ss["next_output_index"] = output_index + 1
            if tool_id:
                tool_calls = ss.setdefault("tool_calls", {})
                tool_calls.setdefault(tool_id, {"name": tool_name, "args": ""})
                output_order = ss.setdefault("output_order", [])
                output_order.append({"kind": "tool", "id": tool_id, "output_index": output_index})
                ss.setdefault("tool_blocks", {})[event.block_index] = tool_id
                ss.setdefault("tool_output_index", {})[tool_id] = output_index
            out.append(
                {
                    "type": "response.output_item.added",
                    "output_index": output_index,
                    "item": {
                        "type": "function_call",
                        "call_id": tool_id,
                        "id": tool_id or f"call_{output_index}",
                        "name": tool_name,
                        "status": "in_progress",
                        "arguments": "",
                    },
                }
            )
        return out

    def _emit_tool_call_delta(
        self, event: ToolCallDeltaEvent, state: StreamState, ss: dict[str, Any]
    ) -> list[dict[str, Any]]:
        out: list[dict[str, Any]] = []
        tool_id = event.tool_id or ss.get("tool_blocks", {}).get(event.block_index, "")
        if tool_id:
            tool_calls = ss.setdefault("tool_calls", {})
            entry = tool_calls.setdefault(tool_id, {"name": "", "args": ""})
            entry["args"] = str(entry.get("args") or "") + (event.input_delta or "")
            output_index = ss.get("tool_output_index", {}).get(tool_id, event.block_index)
            out.append(
                {
                    "type": "response.function_call_arguments.delta",
                    "delta": event.input_delta,
                    "item_id": tool_id,
                    "output_index": output_index,
                }
            )
        return out

    def _emit_content_block_stop(
        self, event: ContentBlockStopEvent, state: StreamState, ss: dict[str, Any]
    ) -> list[dict[str, Any]]:
        out: list[dict[str, Any]] = []
        tool_blocks = ss.get("tool_blocks", {})
        tool_id = (
            tool_blocks.pop(event.block_index, None) if isinstance(tool_blocks, dict) else None
        )
        if tool_id:
            tool_calls = ss.get("tool_calls", {})
            entry = tool_calls.get(tool_id, {})
            output_index = ss.get("tool_output_index", {}).get(tool_id, event.block_index)
            out.append(
                {
                    "type": "response.output_item.done",
                    "output_index": output_index,
                    "item": {
                        "type": "function_call",
                        "call_id": tool_id,
                        "id": tool_id,
                        "name": entry.get("name") or "",
                        "arguments": entry.get("args") or "",
                        "status": "completed",
                    },
                }
            )
        return out

    def _emit_content_delta(
        self, event: ContentDeltaEvent, state: StreamState, ss: dict[str, Any]
    ) -> list[dict[str, Any]]:
        out: list[dict[str, Any]] = []
        if event.text_delta:
            # Thinking delta：Responses API reasoning summary 事件
            block_type = ss.get(f"block_type_{event.block_index}")
            if block_type == ContentType.THINKING.value:
                # 首次 thinking delta -> 添加 reasoning output item
                if not ss.get("reasoning_output_started"):
                    reasoning_output_index = int(ss.get("next_output_index") or 0)
                    ss["next_output_index"] = reasoning_output_index + 1
                    ss["reasoning_output_index"] = reasoning_output_index
                    ss["reasoning_output_started"] = True
                    reasoning_id = f"rs_{state.message_id or 'stream'}"
                    ss["reasoning_id"] = reasoning_id
                    ss.setdefault("output_order", []).append(
                        {
                            "kind": "reasoning",
                            "id": reasoning_id,
                            "output_index": reasoning_output_index,
                        }
                    )
                    out.append(
                        {
                            "type": "response.output_item.added",
                            "output_index": reasoning_output_index,
                            "item": {
                                "type": "reasoning",
                                "id": reasoning_id,
                                "summary": [],
                            },
                        }
                    )
                    # summary part 开始
                    out.append(
                        {
                            "type": "response.reasoning_summary_part.added",
                            "item_id": reasoning_id,
                            "output_index": reasoning_output_index,
                            "summary_index": 0,
                            "part": {"type": "summary_text", "text": ""},
                        }
                    )
                ss["thinking_text"] = str(ss.get("thinking_text") or "") + event.text_delta
                out.append(
                    {
                        "type": "response.reasoning_summary_text.delta",
                        "item_id": ss.get("reasoning_id", ""),
                        "output_index": ss.get("reasoning_output_index", 0),
                        "summary_index": 0,
                        "delta": event.text_delta,
                    }
                )
                return out

            if not ss.get("message_output_started"):
                output_index = int(ss.get("next_output_index") or 0)
                ss["next_output_index"] = output_index + 1
                ss["message_output_index"] = output_index
                ss["message_output_started"] = True
                message_id = f"msg_{state.message_id or 'stream'}"
                ss.setdefault("output_order", []).append(
                    {"kind": "message", "id": message_id, "output_index": output_index}
                )
                out.append(
                    {
                        "type": "response.output_item.added",
                        "output_index": output_index,
                        "item": {
                            "type": "message",
                            "id": message_id,
                            "role": "assistant",
                            "status": "in_progress",
                            "content": [],
                        },
                    }
                )
            ss["text_started"] = True
            ss["collected_text"] = str(ss.get("collected_text") or "") + event.text_delta
            out.append({"type": "response.output_text.delta", "delta": event.text_delta})
        return out

    def _emit_message_stop(
        self, event: MessageStopEvent, state: StreamState, ss: dict[str, Any]
    ) -> list[dict[str, Any]]:
        out: list[dict[str, Any]] = []
        final_text = str(ss.get("collected_text") or "")
        final_thinking = str(ss.get("thinking_text") or "")

        # 先关闭 reasoning summary（如有）
        if ss.get("reasoning_output_started"):
            reasoning_id = ss.get("reasoning_id", "")
            reasoning_output_index = ss.get("reasoning_output_index", 0)
            out.append(
                {
                    "type": "response.reasoning_summary_text.done",
                    "item_id": reasoning_id,
                    "output_index": reasoning_output_index,
                    "summary_index": 0,
                    "text": final_thinking,
                }
            )
            out.append(
                {
                    "type": "response.reasoning_summary_part.done",
                    "item_id": reasoning_id,
                    "output_index": reasoning_output_index,
                    "summary_index": 0,
                    "part": {"type": "summary_text", "text": final_thinking},
                }
            )
            out.append(
                {
                    "type": "response.output_item.done",
                    "output_index": reasoning_output_index,
                    "item": {
                        "type": "reasoning",
                        "id": reasoning_id,
                        "summary": [{"type": "summary_text", "text": final_thinking}],
                    },
                }
            )

        message_id = f"msg_{state.message_id or 'stream'}"
        message_item = {
            "type": "message",
            "id": message_id,
            "role": "assistant",
            "status": "completed",
            "content": ([{"type": "output_text", "text": final_text}] if final_text else []),
        }
        if ss.get("text_started"):
            out.append({"type": "response.output_text.done", "text": final_text})
        if ss.get("message_output_started"):
            output_index = ss.get("message_output_index") or 0
            out.append(
                {
                    "type": "response.output_item.done",
                    "output_index": output_index,
                    "item": message_item,
                }
            )

        # 构建最终 content（包含 thinking）
        final_content: list[ContentBlock] = []
        if final_thinking:
            final_content.append(ThinkingBlock(thinking=final_thinking))
        if final_text:
            final_content.append(TextBlock(text=final_text))

        response_obj = self.response_from_internal(
            InternalResponse(
                id=state.message_id or "resp",
                model=state.model or "",
                content=final_content,
                stop_reason=event.stop_reason or StopReason.END_TURN,
                usage=event.usage or UsageInfo(),
            )
        )
        # 将工具调用添加到 output（最佳努力）
        output_items = self._build_final_output_items(message_item, ss)
        if output_items:
            response_obj["output"] = output_items
        out.append({"type": "response.completed", "response": response_obj})
        return out

    def _build_final_output_items(
        self, message_item: dict[str, Any], ss: dict[str, Any]
    ) -> list[dict[str, Any]]:
        tool_calls = ss.get("tool_calls", {})
        output_order = ss.get("output_order", [])
        output_items: list[dict[str, Any]] = []
        used_tool_ids: set[str] = set()
        if isinstance(output_order, list) and output_order:
            for entry in output_order:
                if not isinstance(entry, dict):
                    continue
                if entry.get("kind") == "reasoning":
                    thinking_text = str(ss.get("thinking_text") or "")
                    if thinking_text:
                        output_items.append(
                            {
                                "type": "reasoning",
                                "id": entry.get("id", ""),
                                "summary": [{"type": "summary_text", "text": thinking_text}],
                            }
                        )
                elif entry.get("kind") == "message":
                    if message_item.get("content"):
                        output_items.append(message_item)
                elif entry.get("kind") == "tool":
                    tool_id = entry.get("id")
                    if not isinstance(tool_id, str) or not tool_id:
                        continue
                    used_tool_ids.add(tool_id)
                    tool_entry = tool_calls.get(tool_id) if isinstance(tool_calls, dict) else None
                    if isinstance(tool_entry, dict):
                        output_items.append(self._tool_call_item(tool_id, tool_entry))
        if isinstance(tool_calls, dict):
            for tool_id, tool_entry in tool_calls.items():
                if tool_id in used_tool_ids or not isinstance(tool_entry, dict):
                    continue
                output_items.append(self._tool_call_item(tool_id, tool_entry))
        return output_items

    @staticmethod
    def _tool_call_item(tool_id: str, entry: dict[str, Any]) -> dict[str, Any]:
        return {
            "type": "function_call",
            "call_id": tool_id,
            "id": tool_id,
            "name": entry.get("name") or "",
            "arguments": entry.get("args") or "",
            "status": "completed",
        }

    def _emit_error(
        self, event: ErrorEvent, state: StreamState, ss: dict[str, Any]
    ) -> list[dict[str, Any]]:
        err_payload = self.error_from_internal(event.error)
        err_payload["type"] = "response.failed"
        return [err_payload]

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
            extra={"openai_cli": {"error": err}, "raw": {"type": raw_type}},
        )

    def error_from_internal(self, internal: InternalError) -> dict[str, Any]:
        type_str = self._ERROR_TYPE_TO_OPENAI.get(internal.type, "server_error")
        payload: dict[str, Any] = {"type": type_str, "message": internal.message}
        if internal.code is not None:
            payload["code"] = internal.code
        if internal.param is not None:
            payload["param"] = internal.param
        return {"error": payload}

    # =========================
    # Helpers
    # =========================

    def _unwrap_response_object(self, response: dict[str, Any]) -> dict[str, Any]:
        if not isinstance(response, dict):
            return {}
        resp_inner = response.get("response")
        if isinstance(resp_inner, dict) and isinstance(response.get("type"), str):
            # 例如：{"type": "response.completed", "response": {...}}
            return resp_inner
        return response

    def _extract_output_text_blocks(
        self, payload: dict[str, Any]
    ) -> tuple[list[ContentBlock], dict[str, Any]]:
        text_parts: list[str] = []

        output = payload.get("output")
        if isinstance(output, list):
            for item in output:
                if not isinstance(item, dict):
                    continue
                if item.get("type") == "message":
                    content = item.get("content")
                    if isinstance(content, list):
                        for part in content:
                            if not isinstance(part, dict):
                                continue
                            ptype = str(part.get("type") or "")
                            if ptype in ("output_text", "text") and isinstance(
                                part.get("text"), str
                            ):
                                text_parts.append(part.get("text") or "")
                    continue

                if item.get("type") in ("output_text", "text") and isinstance(
                    item.get("text"), str
                ):
                    text_parts.append(item.get("text") or "")

        # 兼容：部分实现可能直接给 output_text
        if not text_parts and isinstance(payload.get("output_text"), str):
            text_parts.append(payload.get("output_text") or "")

        blocks: list[ContentBlock] = []
        text = "".join(text_parts)
        if text:
            blocks.append(TextBlock(text=text))

        extra: dict[str, Any] = {"raw": {"openai_cli_output": output}} if output is not None else {}
        return blocks, extra

    def _usage_to_internal(self, usage: Any) -> UsageInfo:
        if not isinstance(usage, dict):
            return UsageInfo()
        input_tokens = int(usage.get("input_tokens") or 0)
        output_tokens = int(usage.get("output_tokens") or 0)
        total_tokens = int(usage.get("total_tokens") or (input_tokens + output_tokens))
        return UsageInfo(
            input_tokens=input_tokens,
            output_tokens=output_tokens,
            total_tokens=total_tokens,
            extra={"openai_cli": {"usage": usage}},
        )

    def _collapse_internal_text(self, blocks: list[ContentBlock]) -> str:
        parts: list[str] = []
        for block in blocks:
            if isinstance(block, TextBlock) and block.text:
                parts.append(block.text)
        return "".join(parts)

    def _input_to_internal_messages(self, input_data: Any) -> list[InternalMessage]:
        if input_data is None:
            return []

        # input: "text"
        if isinstance(input_data, str):
            return [InternalMessage(role=Role.USER, content=[TextBlock(text=input_data)])]

        # input: {"messages": [...]}
        if isinstance(input_data, dict) and isinstance(input_data.get("messages"), list):
            input_data = input_data.get("messages")

        if not isinstance(input_data, list):
            return [
                InternalMessage(
                    role=Role.USER,
                    content=[UnknownBlock(raw_type="input", payload={"input": input_data})],
                )
            ]

        messages: list[InternalMessage] = []
        for item in input_data:
            if not isinstance(item, dict):
                continue
            msg = self._parse_input_item(item)
            if msg is not None:
                messages.append(msg)
        return messages

    def _parse_input_item(self, item: dict[str, Any]) -> InternalMessage | None:
        item_type = str(item.get("type") or "")

        # 标准 message（有 role 字段）
        if item_type == "message" or item.get("role"):
            return self._parse_message_item(item)
        if item_type == "function_call":
            return self._parse_function_call_item(item)
        if item_type == "function_call_output":
            return self._parse_function_call_output_item(item)
        if item_type == "reasoning":
            return self._parse_reasoning_item(item)

        # 其他未知类型 -> 保留为 UnknownBlock
        return InternalMessage(
            role=Role.UNKNOWN,
            content=[UnknownBlock(raw_type=item_type or "unknown", payload=item)],
        )

    def _parse_message_item(self, item: dict[str, Any]) -> InternalMessage:
        role = self._role_from_value(item.get("role"))
        blocks = self._responses_content_to_blocks(item.get("content"))
        return InternalMessage(
            role=role,
            content=blocks,
            extra=self._extract_extra(item, {"type", "role", "content"}),
        )

    def _parse_function_call_item(self, item: dict[str, Any]) -> InternalMessage:
        tool_id = str(item.get("call_id") or item.get("id") or "")
        tool_name = str(item.get("name") or "")
        args_raw = item.get("arguments") or "{}"
        try:
            tool_input = (
                json.loads(args_raw)
                if isinstance(args_raw, str)
                else (args_raw if isinstance(args_raw, dict) else {})
            )
        except (json.JSONDecodeError, TypeError):
            tool_input = {"_raw": args_raw}
        tool_block = ToolUseBlock(
            tool_id=tool_id,
            tool_name=tool_name,
            tool_input=tool_input,
            extra={
                "openai_cli": self._extract_extra(
                    item, {"type", "call_id", "id", "name", "arguments"}
                )
            },
        )
        return InternalMessage(role=Role.ASSISTANT, content=[tool_block])

    def _parse_function_call_output_item(self, item: dict[str, Any]) -> InternalMessage:
        tool_use_id = str(item.get("call_id") or item.get("id") or "")
        output = item.get("output")
        content_text = output if isinstance(output, str) else None
        result_block = ToolResultBlock(
            tool_use_id=tool_use_id,
            output=output,
            content_text=content_text,
            extra={"openai_cli": self._extract_extra(item, {"type", "call_id", "id", "output"})},
        )
        return InternalMessage(role=Role.TOOL, content=[result_block])

    def _parse_reasoning_item(self, item: dict[str, Any]) -> InternalMessage:
        summary_parts: list[str] = []
        summary = item.get("summary")
        if isinstance(summary, list):
            for s in summary:
                if isinstance(s, dict) and s.get("type") == "summary_text":
                    text = s.get("text")
                    if isinstance(text, str) and text:
                        summary_parts.append(text)
                elif isinstance(s, str) and s:
                    summary_parts.append(s)
        elif isinstance(summary, str) and summary:
            summary_parts.append(summary)

        reasoning_blocks: list[ContentBlock] = []
        if summary_parts:
            reasoning_blocks.append(
                UnknownBlock(
                    raw_type="reasoning",
                    payload={"summary_text": "\n".join(summary_parts), "original": item},
                )
            )
        else:
            reasoning_blocks.append(UnknownBlock(raw_type="reasoning", payload=item))
        return InternalMessage(
            role=Role.ASSISTANT,
            content=reasoning_blocks,
            extra={"openai_cli": {"type": "reasoning"}},
        )

    def _responses_content_to_blocks(self, content: Any) -> list[ContentBlock]:
        if content is None:
            return []
        if isinstance(content, str):
            return [TextBlock(text=content)]
        if isinstance(content, dict) and isinstance(content.get("text"), str):
            return [TextBlock(text=str(content.get("text") or ""))]

        if not isinstance(content, list):
            return [UnknownBlock(raw_type="content", payload={"content": content})]

        blocks: list[ContentBlock] = []
        for part in content:
            if isinstance(part, str):
                if part:
                    blocks.append(TextBlock(text=part))
                continue
            if not isinstance(part, dict):
                continue
            ptype = str(part.get("type") or "")
            if ptype in ("input_text", "output_text", "text") and isinstance(part.get("text"), str):
                text = part.get("text") or ""
                if text:
                    blocks.append(TextBlock(text=text))
                continue
            blocks.append(UnknownBlock(raw_type=ptype or "unknown", payload=part))
        return blocks

    def _internal_messages_to_input(
        self,
        messages: list[InternalMessage],
        *,
        system_to_developer: bool = False,
    ) -> list[dict[str, Any]]:
        out: list[dict[str, Any]] = []
        for msg in messages:
            # ToolUseBlock -> function_call
            for block in msg.content:
                if isinstance(block, ToolUseBlock):
                    out.append(
                        {
                            "type": "function_call",
                            "call_id": block.tool_id,
                            "name": block.tool_name,
                            "arguments": (
                                json.dumps(block.tool_input, ensure_ascii=False)
                                if block.tool_input
                                else "{}"
                            ),
                        }
                    )
                    continue

                if isinstance(block, ToolResultBlock):
                    out.append(
                        {
                            "type": "function_call_output",
                            "call_id": block.tool_use_id,
                            "output": (
                                block.content_text
                                if block.content_text is not None
                                else block.output
                            ),
                        }
                    )
                    continue

                # reasoning（UnknownBlock with raw_type="reasoning"）
                if isinstance(block, UnknownBlock) and block.raw_type == "reasoning":
                    payload = block.payload or {}
                    original = payload.get("original")
                    if isinstance(original, dict):
                        # 尽量还原原始结构
                        out.append(original)
                    else:
                        summary_text = payload.get("summary_text", "")
                        out.append(
                            {
                                "type": "reasoning",
                                "summary": (
                                    [{"type": "summary_text", "text": summary_text}]
                                    if summary_text
                                    else []
                                ),
                            }
                        )
                    continue

            # 普通 message（TextBlock）
            role = self._role_to_openai(msg.role)
            # Codex 不接受 system 角色，需要转换为 developer
            if system_to_developer and role == "system":
                role = "developer"
            content_items: list[dict[str, Any]] = []
            has_text = False

            for block in msg.content:
                if isinstance(block, (ToolUseBlock, ToolResultBlock)):
                    continue  # 已在上面处理
                if isinstance(block, UnknownBlock) and block.raw_type == "reasoning":
                    continue  # 已在上面处理
                if isinstance(block, UnknownBlock):
                    continue  # 跳过其他未知块
                if isinstance(block, TextBlock) and block.text:
                    # assistant 角色使用 output_text，其他角色使用 input_text
                    text_type = "output_text" if role == "assistant" else "input_text"
                    content_items.append({"type": text_type, "text": block.text})
                    has_text = True

            if has_text:
                out.append({"type": "message", "role": role, "content": content_items})

        return out

    def _tools_to_internal(self, tools: Any) -> list[ToolDefinition] | None:
        if not isinstance(tools, list):
            return None
        out: list[ToolDefinition] = []
        for tool in tools:
            if not isinstance(tool, dict):
                continue
            if tool.get("type") == "function" and isinstance(tool.get("function"), dict):
                fn = tool["function"]
                name = str(fn.get("name") or "")
                if not name:
                    continue
                out.append(
                    ToolDefinition(
                        name=name,
                        description=fn.get("description"),
                        parameters=(
                            fn.get("parameters") if isinstance(fn.get("parameters"), dict) else None
                        ),
                        extra={
                            "openai_tool": self._extract_extra(tool, {"type", "function"}),
                            "openai_function": self._extract_extra(
                                fn, {"name", "description", "parameters"}
                            ),
                        },
                    )
                )
                continue

            # 兼容：部分实现可能直接给 {name, description, parameters}
            name = str(tool.get("name") or "")
            if name:
                out.append(
                    ToolDefinition(
                        name=name,
                        description=tool.get("description"),
                        parameters=(
                            tool.get("parameters")
                            if isinstance(tool.get("parameters"), dict)
                            else None
                        ),
                        extra={
                            "openai_cli": self._extract_extra(
                                tool, {"name", "description", "parameters"}
                            )
                        },
                    )
                )
        return out or None

    def _tool_choice_to_internal(self, tool_choice: Any) -> ToolChoice | None:
        if tool_choice is None:
            return None
        if isinstance(tool_choice, str):
            if tool_choice == "none":
                return ToolChoice(
                    type=ToolChoiceType.NONE, extra={"openai_cli": {"tool_choice": tool_choice}}
                )
            if tool_choice == "auto":
                return ToolChoice(
                    type=ToolChoiceType.AUTO, extra={"openai_cli": {"tool_choice": tool_choice}}
                )
            return ToolChoice(type=ToolChoiceType.AUTO, extra={"raw": tool_choice})

        if isinstance(tool_choice, dict):
            # OpenAI 兼容结构：{"type":"function","function":{"name":"..."}}
            if tool_choice.get("type") == "function" and isinstance(
                tool_choice.get("function"), dict
            ):
                name = str(tool_choice["function"].get("name") or "")
                return ToolChoice(
                    type=ToolChoiceType.TOOL, tool_name=name, extra={"openai_cli": tool_choice}
                )
            return ToolChoice(type=ToolChoiceType.AUTO, extra={"openai_cli": tool_choice})

        return ToolChoice(type=ToolChoiceType.AUTO, extra={"raw": tool_choice})

    def _tool_choice_to_openai(self, tool_choice: ToolChoice) -> str | dict[str, Any]:
        if tool_choice.type == ToolChoiceType.NONE:
            return "none"
        if tool_choice.type == ToolChoiceType.AUTO:
            return "auto"
        if tool_choice.type == ToolChoiceType.REQUIRED:
            return "required"
        if tool_choice.type == ToolChoiceType.TOOL:
            # Responses API 使用扁平结构: {type, name}
            return {"type": "function", "name": tool_choice.tool_name or ""}
        return "auto"

    def _role_from_value(self, role: Any) -> Role:
        value = str(role or "").lower()
        if value == "user":
            return Role.USER
        if value == "assistant":
            return Role.ASSISTANT
        if value == "system":
            return Role.SYSTEM
        if value == "developer":
            return Role.DEVELOPER
        if value == "tool":
            return Role.TOOL
        return Role.UNKNOWN

    def _role_to_openai(self, role: Role) -> str:
        if role == Role.USER:
            return "user"
        if role == Role.ASSISTANT:
            return "assistant"
        if role == Role.SYSTEM:
            return "system"
        if role == Role.DEVELOPER:
            return "developer"
        if role == Role.TOOL:
            return "tool"
        return "user"

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
            out: list[str] = []
            for item in value:
                if item is None:
                    continue
                out.append(str(item))
            return out
        return [str(value)]

    def _extract_extra(self, payload: dict[str, Any], keep_keys: set[str]) -> dict[str, Any]:
        if not isinstance(payload, dict):
            return {}
        return {k: v for k, v in payload.items() if k not in keep_keys}

    def _join_instructions(self, instructions: list[InstructionSegment]) -> str | None:
        """合并 instructions 为单一字符串，与其他 normalizer 保持一致"""
        parts = [seg.text for seg in instructions if seg.text]
        joined = "\n\n".join(parts)
        return joined or None

    def _error_type_from_value(self, value: str) -> ErrorType:
        for t in ErrorType:
            if t.value == value:
                return t
        return ErrorType.UNKNOWN


__all__ = ["OpenAICliNormalizer"]
