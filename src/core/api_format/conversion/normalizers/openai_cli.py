"""
OpenAI CLI / Responses Normalizer (OPENAI_CLI)

目标：
- 将 OpenAI Responses API（/v1/responses）映射到 InternalRequest / InternalResponse
- 支持流式事件：response.output_text.delta / response.completed 等

说明：
- 这里实现的是“最佳努力”的最小可用映射，重点覆盖文本与 usage。
- 未识别的字段会进入 extra/raw，未知内容块保留在 internal，但默认输出阶段会丢弃。
"""

import json
import time
from collections.abc import Callable
from typing import Any

from src.core.api_format.conversion.constants import OPENAI_HANDLED_KEYS as _HANDLED_KEYS
from src.core.api_format.conversion.constants import (
    OPENAI_RESPONSES_PASSTHROUGH_KEYS as _RESPONSES_PASSTHROUGH_KEYS,
)
from src.core.api_format.conversion.constants import (
    chat_tool_choice_to_responses as _chat_tool_choice_to_responses,
)
from src.core.api_format.conversion.constants import (
    chat_tool_to_responses_tool as _chat_tool_to_responses_tool,
)
from src.core.api_format.conversion.constants import (
    chat_web_search_options_to_responses_tools as _chat_web_search_options_to_responses_tools,
)
from src.core.api_format.conversion.constants import stable_json_dumps as _stable_json_dumps
from src.core.api_format.conversion.field_mappings import (
    ERROR_TYPE_MAPPINGS,
    REASONING_EFFORT_TO_THINKING_BUDGET,
    RETRYABLE_ERROR_TYPES,
    THINKING_BUDGET_TO_REASONING_EFFORT,
)
from src.core.api_format.conversion.internal import (
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
    ResponseFormatConfig,
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
    UnknownStreamEvent,
)
from src.core.api_format.conversion.stream_state import StreamState
from src.core.logger import logger


def _is_chat_completions_response(data: dict[str, Any]) -> bool:
    """检测数据是否为 OpenAI Chat Completions 格式（而非 Responses API 格式）。

    Chat Completions 的特征：
    - 非流式：有 choices 数组且 object == "chat.completion"
    - 流式：有 choices 数组且 object == "chat.completion.chunk"
    """
    if not isinstance(data, dict):
        return False
    obj = data.get("object", "")
    if isinstance(obj, str) and obj.startswith("chat.completion"):
        return True
    if isinstance(data.get("choices"), list) and "type" not in data:
        return True
    return False


def _get_openai_chat_normalizer() -> "FormatNormalizer | None":
    """获取已注册的 openai:chat normalizer 实例（延迟获取避免循环导入）。"""
    from src.core.api_format.conversion.registry import format_conversion_registry

    return format_conversion_registry.get_normalizer("openai:chat")


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

    def patch_for_variant(
        self,
        request: dict[str, Any],
        variant: str,
    ) -> dict[str, Any] | None:
        """Codex 同格式透传：直接在原始请求体上做最小补丁，跳过 internal 转换。"""
        if variant.lower() != "codex":
            return None
        out: dict[str, Any] = dict(request)
        # 内部路由标记：绝不能透传到上游。
        out.pop("_aether_compact", None)
        return out

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
        top_logprobs = self._optional_int(request.get("top_logprobs"))

        # parallel_tool_calls
        parallel_tool_calls: bool | None = None
        ptc = request.get("parallel_tool_calls")
        if ptc is not None:
            parallel_tool_calls = bool(ptc)

        # reasoning -> ThinkingConfig (Responses API uses reasoning.effort)
        thinking: ThinkingConfig | None = None
        reasoning = request.get("reasoning")
        if isinstance(reasoning, dict):
            effort = reasoning.get("effort")
            if isinstance(effort, str) and effort in REASONING_EFFORT_TO_THINKING_BUDGET:
                thinking = ThinkingConfig(
                    enabled=True,
                    budget_tokens=REASONING_EFFORT_TO_THINKING_BUDGET[effort],
                    extra={"reasoning_effort": effort, "reasoning": reasoning},
                )

        text_config = request.get("text")
        response_format: ResponseFormatConfig | None = None
        extra: dict[str, Any] = {
            "openai_cli": self._extract_extra(
                request,
                {
                    "model",
                    "input",
                    "instructions",
                    "max_output_tokens",
                    "max_tokens",
                    "temperature",
                    "top_p",
                    "top_logprobs",
                    "stop",
                    "stream",
                    "tools",
                    "tool_choice",
                    "parallel_tool_calls",
                    "reasoning",
                },
            ),
            "openai_cli_request_flags": {
                "has_instructions": "instructions" in request,
                "has_stream": "stream" in request,
            },
        }
        if isinstance(text_config, dict):
            fmt = text_config.get("format")
            if isinstance(fmt, dict):
                fmt_type = str(fmt.get("type") or "text")
                fmt_schema = (
                    fmt.get("json_schema") if isinstance(fmt.get("json_schema"), dict) else None
                )
                if fmt_type != "text":
                    response_format = ResponseFormatConfig(
                        type=fmt_type,
                        json_schema=fmt_schema,
                        extra=self._extract_extra(fmt, {"type", "json_schema"}),
                    )
            verbosity = text_config.get("verbosity")
            if isinstance(verbosity, str) and verbosity:
                extra["verbosity"] = verbosity

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
            thinking=thinking,
            parallel_tool_calls=parallel_tool_calls,
            top_logprobs=top_logprobs,
            response_format=response_format,
            extra=extra,
        )

        # reasoning_effort 同步存入 extra (支持独立于 thinking 的跨格式转换)
        if thinking and thinking.extra.get("reasoning_effort"):
            internal.extra["reasoning_effort"] = thinking.extra["reasoning_effort"]

        return internal

    def request_from_internal(
        self,
        internal: InternalRequest,
        *,
        target_variant: str | None = None,
    ) -> dict[str, Any]:
        openai_extra = internal.extra.get("openai", {})
        openai_cli_extra = internal.extra.get("openai_cli", {})
        request_flags = internal.extra.get("openai_cli_request_flags", {}) if internal.extra else {}
        is_codex_variant = (target_variant or "").lower() == "codex"
        has_explicit_instructions = bool(
            isinstance(request_flags, dict) and request_flags.get("has_instructions")
        )
        has_explicit_stream = bool(
            isinstance(request_flags, dict) and request_flags.get("has_stream")
        )

        # 合并 instructions，如果没有则使用 system。
        # 仅在 internal 中确有内容或原请求显式提供过时输出，
        # 让 Codex 默认 body_rules 仍可在字段缺失时注入默认 instructions。
        instructions_text = (
            self._join_instructions(internal.instructions)
            if internal.instructions
            else internal.system
        )
        result: dict[str, Any] = {"model": internal.model}
        if instructions_text or has_explicit_instructions:
            result["instructions"] = instructions_text or ""
        # Keep the stable system prefix ahead of the typically dynamic input payload.
        result["input"] = self._internal_messages_to_input(
            internal.messages,
            system_to_developer=False,
        )

        if internal.max_tokens is not None:
            # Responses API 使用 max_output_tokens
            result["max_output_tokens"] = internal.max_tokens
        if internal.temperature is not None:
            result["temperature"] = internal.temperature
        if internal.top_p is not None:
            result["top_p"] = internal.top_p

        if internal.stop_sequences:
            result["stop"] = list(internal.stop_sequences)
        if internal.stream or has_explicit_stream:
            result["stream"] = bool(internal.stream)

        if internal.tools:
            # Responses API 使用扁平结构: {type, name, description, parameters}
            # 而非 Chat Completions 的嵌套结构: {type, function: {name, ...}}
            rebuilt_tools: list[dict[str, Any]] = []
            for t in internal.tools:
                # 非 function 类型（如 custom/web_search）：直接还原原始 dict
                raw_tool = t.extra.get("openai_cli_raw_tool")
                if isinstance(raw_tool, dict):
                    rebuilt_tools.append(raw_tool)
                elif isinstance(t.extra.get("openai_chat_raw_tool"), dict):
                    chat_tool = t.extra["openai_chat_raw_tool"]
                    if translated_tool := _chat_tool_to_responses_tool(chat_tool):
                        rebuilt_tools.append(translated_tool)
                else:
                    rebuilt_tool: dict[str, Any] = {
                        "type": "function",
                        **(t.extra.get("openai_cli") or {}),
                        **(t.extra.get("openai_tool") or {}),
                        **(t.extra.get("openai_function") or {}),
                        "name": t.name,
                    }
                    if t.description is not None:
                        rebuilt_tool["description"] = t.description
                    if t.parameters is not None:
                        rebuilt_tool["parameters"] = t.parameters
                    rebuilt_tools.append(rebuilt_tool)
            if rebuilt_tools:
                result["tools"] = rebuilt_tools

        if internal.tool_choice:
            result["tool_choice"] = self._tool_choice_to_openai(internal.tool_choice)

        # thinking -> reasoning (Responses API)
        effort: str | None = None
        if internal.thinking and internal.thinking.enabled:
            # 优先还原原始 reasoning 对象
            original_reasoning = internal.thinking.extra.get("reasoning")
            if isinstance(original_reasoning, dict):
                result["reasoning"] = original_reasoning
                effort = None  # 已还原，不需要再构造
            else:
                effort = internal.thinking.extra.get("reasoning_effort")
        # 显式 reasoning_effort 应优先于 budget 反推，避免覆盖 Claude output_config.effort
        if not effort and internal.extra and "reasoning" not in result:
            effort = internal.extra.get("reasoning_effort")
        if (
            not effort
            and "reasoning" not in result
            and internal.thinking
            and internal.thinking.enabled
            and internal.thinking.budget_tokens is not None
        ):
            for threshold, level in THINKING_BUDGET_TO_REASONING_EFFORT:
                if internal.thinking.budget_tokens <= threshold:
                    effort = level
                    break
        if effort:
            # xhigh 降级为 high (Responses API 也仅支持 low/medium/high)
            if effort == "xhigh":
                effort = "high"
            result["reasoning"] = {"effort": effort}

        # parallel_tool_calls
        if internal.parallel_tool_calls is not None:
            result["parallel_tool_calls"] = internal.parallel_tool_calls

        text_config: dict[str, Any] = {}
        if internal.response_format:
            text_config["format"] = {"type": internal.response_format.type}
            if internal.response_format.json_schema:
                text_config["format"]["json_schema"] = internal.response_format.json_schema
        verbosity = internal.extra.get("verbosity") if internal.extra else None
        if isinstance(verbosity, str) and verbosity:
            text_config["verbosity"] = verbosity
        if text_config:
            result["text"] = text_config

        if internal.top_logprobs is not None:
            result["top_logprobs"] = internal.top_logprobs

        if isinstance(openai_extra, dict):
            mapped_tools = _chat_web_search_options_to_responses_tools(
                openai_extra.get("web_search_options")
            )
            if mapped_tools:
                existing_tools = result.setdefault("tools", [])
                if isinstance(existing_tools, list) and not any(
                    isinstance(tool, dict) and str(tool.get("type") or "").startswith("web_search")
                    for tool in existing_tools
                ):
                    existing_tools.extend(mapped_tools)

            for key, value in openai_extra.items():
                if (
                    key in _RESPONSES_PASSTHROUGH_KEYS
                    and key not in result
                    and key not in _HANDLED_KEYS
                ):
                    result[key] = value
        if isinstance(openai_cli_extra, dict):
            for key, value in openai_cli_extra.items():
                if (
                    key in _RESPONSES_PASSTHROUGH_KEYS
                    and key not in result
                    and key not in _HANDLED_KEYS
                ):
                    result[key] = value

        if is_codex_variant and "store" not in result:
            result["store"] = False

        return self._reorder_request_prefix_keys(result)

    # =========================
    # Responses
    # =========================

    def response_to_internal(self, response: dict[str, Any]) -> InternalResponse:
        payload = self._unwrap_response_object(response)

        # 检测 Chat Completions 格式回退
        if _is_chat_completions_response(payload):
            chat_norm = _get_openai_chat_normalizer()
            if chat_norm is not None:
                logger.debug(
                    "[OpenAICliNormalizer] 检测到 Chat Completions 响应格式，委托给 openai:chat normalizer"
                )
                return chat_norm.response_to_internal(payload)

        rid = str(payload.get("id") or "")
        model = str(payload.get("model") or "")

        blocks, extra, has_tool_use = self._extract_output_blocks(payload)
        usage = self._usage_to_internal(payload.get("usage"))

        stop_reason = StopReason.UNKNOWN
        status = payload.get("status")
        if isinstance(status, str) and status == "completed":
            stop_reason = StopReason.TOOL_USE if has_tool_use else StopReason.END_TURN

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
        output_items: list[dict[str, Any]] = []

        # 构建 output items：reasoning（思考）、message（文本）、function_call（工具调用）
        # 按 Responses API 顺序：reasoning -> message -> function_call
        rs_idx = 0
        for block in internal.content:
            if isinstance(block, ThinkingBlock) and block.thinking:
                rs_id = (
                    f"rs_{internal.id or 'resp'}"
                    if rs_idx == 0
                    else f"rs_{internal.id or 'resp'}_{rs_idx}"
                )
                output_items.append(
                    {
                        "type": "reasoning",
                        "id": rs_id,
                        "summary": [{"type": "summary_text", "text": block.thinking}],
                    }
                )
                rs_idx += 1

        text = self._collapse_internal_text(internal.content)
        if text:
            output_items.append(
                {
                    "type": "message",
                    "id": f"msg_{internal.id or 'stream'}",
                    "role": "assistant",
                    "status": "completed",
                    "content": [{"type": "output_text", "text": text, "annotations": []}],
                }
            )

        for block in internal.content:
            if isinstance(block, ToolUseBlock):
                raw = block.extra.get("raw") if isinstance(block.extra, dict) else None
                raw_arguments = raw.get("arguments") if isinstance(raw, dict) else None
                arguments = (
                    raw_arguments
                    if isinstance(raw_arguments, str)
                    else _stable_json_dumps(block.tool_input or {})
                )
                output_items.append(
                    {
                        "type": "function_call",
                        "call_id": block.tool_id,
                        "id": block.tool_id,
                        "name": block.tool_name,
                        "arguments": arguments,
                        "status": "completed",
                    }
                )

        # 如果没有任何 output item，添加空 message（保持结构完整）
        if not output_items:
            output_items.append(
                {
                    "type": "message",
                    "id": f"msg_{internal.id or 'stream'}",
                    "role": "assistant",
                    "status": "completed",
                    "content": [{"type": "output_text", "text": "", "annotations": []}],
                }
            )

        usage = internal.usage or UsageInfo()
        usage_obj: dict[str, Any] = {
            "input_tokens": usage.input_tokens,
            "output_tokens": usage.output_tokens,
            "total_tokens": usage.total_tokens or (usage.input_tokens + usage.output_tokens),
        }
        # input_tokens_details（cached_tokens）
        if usage.cache_read_tokens:
            usage_obj["input_tokens_details"] = {"cached_tokens": usage.cache_read_tokens}
        # output_tokens_details（reasoning_tokens）— 从原始 usage 中恢复
        raw_usage = (usage.extra.get("openai_cli") or {}).get("usage") or {}
        if isinstance(raw_usage, dict):
            output_details = raw_usage.get("output_tokens_details")
            if isinstance(output_details, dict) and output_details.get("reasoning_tokens"):
                usage_obj["output_tokens_details"] = {
                    "reasoning_tokens": int(output_details["reasoning_tokens"]),
                }

        # 优先使用用户请求的原始模型名，回退到上游返回的模型名
        model_name = requested_model if requested_model else (internal.model or "")

        result: dict[str, Any] = {
            "id": internal.id or "resp",
            "object": "response",
            "created_at": int(time.time()),
            "status": "completed",
            "model": model_name,
            "output": output_items,
            "usage": usage_obj,
            "background": False,
            "error": None,
            "incomplete_details": None,
        }

        # 回显原始请求字段（CLI 客户端可能依赖这些字段做状态管理）
        openai_cli_extra = internal.extra.get("openai_cli", {})
        for key in (
            "instructions",
            "max_output_tokens",
            "temperature",
            "top_p",
            "tool_choice",
            "tools",
            "store",
            "metadata",
            "previous_response_id",
            "reasoning",
            "truncation",
            "service_tier",
            "parallel_tool_calls",
        ):
            if key in openai_cli_extra and key not in result:
                result[key] = openai_cli_extra[key]

        return result

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

        # 检测 Chat Completions 流式格式回退
        # 某些 Provider 即使配置为 openai:cli 也可能返回 Chat Completions 格式
        if _is_chat_completions_response(chunk):
            chat_norm = _get_openai_chat_normalizer()
            if chat_norm is not None:
                if not ss.get("_chat_fallback_logged"):
                    ss["_chat_fallback_logged"] = True
                    logger.debug(
                        "[OpenAICliNormalizer] 检测到 Chat Completions 流式格式，委托给 openai:chat"
                    )
                return chat_norm.stream_chunk_to_internal(chunk, state)

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
                ss.setdefault("text_block_index", None)
                ss.setdefault("next_block_index", 0)
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
            text_block_index = self._ensure_text_block_index(ss)
            if not ss.get("text_block_started"):
                ss["text_block_started"] = True
                events.append(
                    ContentBlockStartEvent(
                        block_index=text_block_index,
                        block_type=ContentType.TEXT,
                    )
                )
            events.append(ContentDeltaEvent(block_index=text_block_index, text_delta=delta_text))
        return events

    def _handle_output_text_done(
        self, chunk: dict[str, Any], state: StreamState, ss: dict[str, Any]
    ) -> list[InternalStreamEvent]:
        events: list[InternalStreamEvent] = []
        if ss.get("text_block_started") and not ss.get("text_block_stopped"):
            ss["text_block_stopped"] = True
            events.append(ContentBlockStopEvent(block_index=self._ensure_text_block_index(ss)))
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
            events.append(ContentBlockStopEvent(block_index=self._ensure_text_block_index(ss)))

        # 补齐所有已开始但未结束的 tool_call block
        active_tools = ss.get("active_tool_blocks")
        if isinstance(active_tools, dict):
            for tool_id, bi in list(active_tools.items()):
                events.append(ContentBlockStopEvent(block_index=bi))
            active_tools.clear()

        # 根据流中是否出现过工具调用来判断 stop_reason
        has_tool_calls = bool(ss.get("tool_calls"))
        stop_reason = StopReason.TOOL_USE if has_tool_calls else StopReason.END_TURN
        events.append(MessageStopEvent(stop_reason=stop_reason, usage=usage))
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
                item_id = str(item.get("id") or "")
                tool_id = str(item.get("call_id") or item.get("id") or "")
                tool_name = str(item.get("name") or "")
                block_index = self._allocate_block_index(ss)

                # 先注册别名，再 resolve，确保后续 delta 能正确映射
                self._register_tool_alias(
                    item_id=item_id,
                    call_id=tool_id,
                    ss=ss,
                )

                # 记录当前活跃的工具调用（支持并行）
                active_tools = ss.setdefault("active_tool_blocks", {})
                active_tools[tool_id] = block_index
                ss["current_tool_id"] = tool_id
                ss["current_tool_name"] = tool_name

                # 初始化工具调用收集
                tool_calls = ss.setdefault("tool_calls", {})
                tool_calls.setdefault(tool_id, {"name": tool_name, "args": ""})

                events.append(
                    ContentBlockStartEvent(
                        block_index=block_index,
                        block_type=ContentType.TOOL_USE,
                        tool_id=tool_id,
                        tool_name=tool_name,
                    )
                )
                events.extend(
                    self._sync_tool_arguments_snapshot(
                        tool_id=tool_id,
                        block_index=block_index,
                        args_snapshot=item.get("arguments"),
                        ss=ss,
                    )
                )
        return events

    def _handle_output_item_done(
        self, chunk: dict[str, Any], state: StreamState, ss: dict[str, Any]
    ) -> list[InternalStreamEvent]:
        events: list[InternalStreamEvent] = []
        item = chunk.get("item")
        if isinstance(item, dict):
            item_type = item.get("type")
            if item_type == "function_call":
                tool_id = self._resolve_tool_call_id(item.get("call_id") or item.get("id"), ss)
                self._register_tool_alias(
                    item_id=str(item.get("id") or ""),
                    call_id=tool_id,
                    ss=ss,
                )
                active_tools = ss.get("active_tool_blocks", {})
                block_index = active_tools.pop(tool_id, self._last_block_index(ss))
                events.extend(
                    self._sync_tool_arguments_snapshot(
                        tool_id=tool_id,
                        block_index=int(block_index),
                        args_snapshot=item.get("arguments"),
                        ss=ss,
                    )
                )
                events.append(ContentBlockStopEvent(block_index=block_index))
        return events

    def _handle_function_call_delta(
        self, chunk: dict[str, Any], state: StreamState, ss: dict[str, Any]
    ) -> list[InternalStreamEvent]:
        events: list[InternalStreamEvent] = []
        delta = chunk.get("delta") or ""
        if delta:
            # 确定当前工具调用的 block_index 和 tool_id
            tool_id = self._resolve_tool_call_id(
                chunk.get("item_id") or ss.get("current_tool_id", ""),
                ss,
            )
            active_tools = ss.get("active_tool_blocks", {})
            block_index = active_tools.get(tool_id, self._last_block_index(ss))

            # 累积参数
            tool_calls = ss.setdefault("tool_calls", {})
            entry = tool_calls.setdefault(tool_id, {"name": "", "args": ""})
            entry["args"] = str(entry.get("args") or "") + delta

            events.append(
                ToolCallDeltaEvent(
                    block_index=block_index,
                    tool_id=tool_id,
                    input_delta=delta,
                )
            )
        return events

    def _handle_function_call_done(
        self, chunk: dict[str, Any], state: StreamState, ss: dict[str, Any]
    ) -> list[InternalStreamEvent]:
        tool_id = self._resolve_tool_call_id(
            chunk.get("item_id") or ss.get("current_tool_id", ""),
            ss,
        )
        active_tools = ss.get("active_tool_blocks", {})
        block_index = int(active_tools.get(tool_id, self._last_block_index(ss)))
        return self._sync_tool_arguments_snapshot(
            tool_id=tool_id,
            block_index=block_index,
            args_snapshot=chunk.get("arguments"),
            ss=ss,
        )

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
        "response.function_call_arguments.done": _handle_function_call_done,
        "response.content_part.added": _handle_noop,
        "response.content_part.done": _handle_noop,
        "response.reasoning_summary_text.delta": _handle_as_unknown,
        "response.reasoning_summary_text.done": _handle_as_unknown,
    }

    def _sync_tool_arguments_snapshot(
        self,
        *,
        tool_id: str,
        block_index: int,
        args_snapshot: Any,
        ss: dict[str, Any],
    ) -> list[InternalStreamEvent]:
        if not tool_id:
            return []
        if not isinstance(args_snapshot, str) or not args_snapshot:
            return []

        tool_calls = ss.setdefault("tool_calls", {})
        entry = tool_calls.setdefault(tool_id, {"name": "", "args": ""})
        current_args = str(entry.get("args") or "")

        if args_snapshot == current_args:
            return []

        if current_args and not args_snapshot.startswith(current_args):
            logger.debug(
                "[OpenAICliNormalizer] function_call args snapshot is not a prefix extension: "
                "tool_id={}, current_len={}, snapshot_len={}",
                tool_id,
                len(current_args),
                len(args_snapshot),
            )
            delta = args_snapshot
        else:
            delta = args_snapshot[len(current_args) :] if current_args else args_snapshot
        entry["args"] = args_snapshot
        if not delta:
            return []
        return [
            ToolCallDeltaEvent(
                block_index=block_index,
                tool_id=tool_id,
                input_delta=delta,
            )
        ]

    @staticmethod
    def _register_tool_alias(
        *,
        item_id: str,
        call_id: str,
        ss: dict[str, Any],
    ) -> None:
        if not item_id or not call_id or item_id == call_id:
            return
        alias_map = ss.get("tool_item_to_call_id")
        if not isinstance(alias_map, dict):
            alias_map = {}
            ss["tool_item_to_call_id"] = alias_map
        alias_map[item_id] = call_id

    @staticmethod
    def _resolve_tool_call_id(tool_ref: Any, ss: dict[str, Any]) -> str:
        raw = str(tool_ref or "")
        if not raw:
            return ""
        alias_map = ss.get("tool_item_to_call_id")
        if isinstance(alias_map, dict):
            return str(alias_map.get(raw) or raw)
        return raw

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
        ss.setdefault("seq", 0)
        response_obj = {
            "id": state.message_id,
            "object": "response",
            "created_at": int(time.time()),
            "model": state.model,
            "status": "in_progress",
            "background": False,
            "error": None,
            "output": [],
        }
        out.append(
            {
                "type": "response.created",
                "sequence_number": self._next_seq(ss),
                "response": response_obj,
            }
        )
        # OpenAI Responses API 常见的 in_progress 事件（可选，最佳努力）
        if not ss.get("sent_in_progress"):
            ss["sent_in_progress"] = True
            out.append(
                {
                    "type": "response.in_progress",
                    "sequence_number": self._next_seq(ss),
                    "response": response_obj,
                }
            )
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
                    "sequence_number": self._next_seq(ss),
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
                    "sequence_number": self._next_seq(ss),
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
            # function_call_arguments.done
            out.append(
                {
                    "type": "response.function_call_arguments.done",
                    "sequence_number": self._next_seq(ss),
                    "item_id": tool_id,
                    "output_index": output_index,
                    "arguments": entry.get("args") or "",
                }
            )
            # output_item.done
            out.append(
                {
                    "type": "response.output_item.done",
                    "sequence_number": self._next_seq(ss),
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
                            "sequence_number": self._next_seq(ss),
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
                            "sequence_number": self._next_seq(ss),
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
                        "sequence_number": self._next_seq(ss),
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
                        "sequence_number": self._next_seq(ss),
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
                # content_part.added
                out.append(
                    {
                        "type": "response.content_part.added",
                        "sequence_number": self._next_seq(ss),
                        "item_id": message_id,
                        "output_index": output_index,
                        "content_index": 0,
                        "part": {"type": "output_text", "text": "", "annotations": []},
                    }
                )
            ss["text_started"] = True
            ss["collected_text"] = str(ss.get("collected_text") or "") + event.text_delta
            message_id = f"msg_{state.message_id or 'stream'}"
            output_index = ss.get("message_output_index") or 0
            out.append(
                {
                    "type": "response.output_text.delta",
                    "sequence_number": self._next_seq(ss),
                    "item_id": message_id,
                    "output_index": output_index,
                    "content_index": 0,
                    "delta": event.text_delta,
                }
            )
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
                    "sequence_number": self._next_seq(ss),
                    "item_id": reasoning_id,
                    "output_index": reasoning_output_index,
                    "summary_index": 0,
                    "text": final_thinking,
                }
            )
            out.append(
                {
                    "type": "response.reasoning_summary_part.done",
                    "sequence_number": self._next_seq(ss),
                    "item_id": reasoning_id,
                    "output_index": reasoning_output_index,
                    "summary_index": 0,
                    "part": {"type": "summary_text", "text": final_thinking},
                }
            )
            out.append(
                {
                    "type": "response.output_item.done",
                    "sequence_number": self._next_seq(ss),
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
            "content": (
                [{"type": "output_text", "text": final_text, "annotations": []}]
                if final_text
                else []
            ),
        }
        if ss.get("text_started"):
            msg_output_index = ss.get("message_output_index") or 0
            out.append(
                {
                    "type": "response.output_text.done",
                    "sequence_number": self._next_seq(ss),
                    "item_id": message_id,
                    "output_index": msg_output_index,
                    "content_index": 0,
                    "text": final_text,
                }
            )
            # content_part.done
            out.append(
                {
                    "type": "response.content_part.done",
                    "sequence_number": self._next_seq(ss),
                    "item_id": message_id,
                    "output_index": msg_output_index,
                    "content_index": 0,
                    "part": {"type": "output_text", "text": final_text, "annotations": []},
                }
            )
        if ss.get("message_output_started"):
            output_index = ss.get("message_output_index") or 0
            out.append(
                {
                    "type": "response.output_item.done",
                    "sequence_number": self._next_seq(ss),
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

        # 传递 extra 以便 response_from_internal 能回显请求字段
        resp_extra = state.extra.get("openai_cli_request_extra") or {}
        response_obj = self.response_from_internal(
            InternalResponse(
                id=state.message_id or "resp",
                model=state.model or "",
                content=final_content,
                stop_reason=event.stop_reason or StopReason.END_TURN,
                usage=event.usage or UsageInfo(),
                extra={"openai_cli": resp_extra} if resp_extra else {},
            )
        )
        # 将工具调用添加到 output（最佳努力）
        output_items = self._build_final_output_items(message_item, ss)
        if output_items:
            response_obj["output"] = output_items
        out.append(
            {
                "type": "response.completed",
                "sequence_number": self._next_seq(ss),
                "response": response_obj,
            }
        )
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
        err_payload["sequence_number"] = self._next_seq(ss)
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

    @staticmethod
    def _next_seq(ss: dict[str, Any]) -> int:
        """递增并返回下一个 sequence_number"""
        seq = int(ss.get("seq") or 0) + 1
        ss["seq"] = seq
        return seq

    @staticmethod
    def _allocate_block_index(ss: dict[str, Any]) -> int:
        idx = ss.get("next_block_index", 0)
        ss["next_block_index"] = idx + 1
        return idx

    @classmethod
    def _ensure_text_block_index(cls, ss: dict[str, Any]) -> int:
        idx = ss.get("text_block_index")
        if idx is not None:
            return idx
        idx = cls._allocate_block_index(ss)
        ss["text_block_index"] = idx
        return idx

    @staticmethod
    def _last_block_index(ss: dict[str, Any]) -> int:
        return max(ss.get("next_block_index", 1) - 1, 0)

    def _unwrap_response_object(self, response: dict[str, Any]) -> dict[str, Any]:
        if not isinstance(response, dict):
            return {}
        resp_inner = response.get("response")
        if isinstance(resp_inner, dict) and isinstance(response.get("type"), str):
            # 例如：{"type": "response.completed", "response": {...}}
            return resp_inner
        return response

    def _extract_output_blocks(
        self, payload: dict[str, Any]
    ) -> tuple[list[ContentBlock], dict[str, Any], bool]:
        """从 Responses API 的 output 提取所有内容块。

        Returns:
            (blocks, extra, has_tool_use): 内容块列表、extra 信息、是否包含工具调用
        """
        text_parts: list[str] = []
        thinking_blocks: list[ContentBlock] = []
        blocks: list[ContentBlock] = []
        has_tool_use = False

        output = payload.get("output")
        if isinstance(output, list):
            for item in output:
                if not isinstance(item, dict):
                    continue

                item_type = item.get("type")

                if item_type == "message":
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

                if item_type == "function_call":
                    has_tool_use = True
                    tool_id = str(item.get("call_id") or item.get("id") or "")
                    tool_name = str(item.get("name") or "")
                    args_raw = item.get("arguments")
                    if args_raw is None:
                        args_raw = "{}"
                    try:
                        tool_input = (
                            json.loads(args_raw)
                            if isinstance(args_raw, str)
                            else (args_raw if isinstance(args_raw, dict) else {})
                        )
                    except (json.JSONDecodeError, TypeError):
                        tool_input = {"_raw": args_raw}
                    extra: dict[str, Any] = {
                        "openai_cli": self._extract_extra(
                            item, {"type", "call_id", "id", "name", "arguments", "status"}
                        )
                    }
                    if isinstance(args_raw, str):
                        extra["raw"] = {"arguments": args_raw}
                    blocks.append(
                        ToolUseBlock(
                            tool_id=tool_id,
                            tool_name=tool_name,
                            tool_input=tool_input,
                            extra=extra,
                        )
                    )
                    continue

                if item_type in ("output_text", "text") and isinstance(item.get("text"), str):
                    text_parts.append(item.get("text") or "")
                    continue

                if item_type == "reasoning":
                    # reasoning output item -> ThinkingBlock
                    summary = item.get("summary")
                    summary_parts: list[str] = []
                    if isinstance(summary, list):
                        for s in summary:
                            if isinstance(s, dict) and s.get("type") == "summary_text":
                                t = s.get("text")
                                if isinstance(t, str) and t:
                                    summary_parts.append(t)
                    thinking_text = "\n".join(summary_parts)
                    if thinking_text:
                        thinking_blocks.append(ThinkingBlock(thinking=thinking_text))
                    continue

        # 兼容：部分实现可能直接给 output_text
        if not text_parts and isinstance(payload.get("output_text"), str):
            text_parts.append(payload.get("output_text") or "")

        # thinking 在前，文本在中，工具调用在后（与 Claude 的 content 顺序一致）
        result_blocks: list[ContentBlock] = []
        result_blocks.extend(thinking_blocks)
        text = "".join(text_parts)
        if text:
            result_blocks.append(TextBlock(text=text))
        result_blocks.extend(blocks)

        extra: dict[str, Any] = {"raw": {"openai_cli_output": output}} if output is not None else {}
        return result_blocks, extra, has_tool_use

    def _usage_to_internal(self, usage: Any) -> UsageInfo:
        if not isinstance(usage, dict):
            return UsageInfo()
        input_tokens = int(usage.get("input_tokens") or 0)
        output_tokens = int(usage.get("output_tokens") or 0)
        total_tokens = int(usage.get("total_tokens") or (input_tokens + output_tokens))

        # input_tokens_details.cached_tokens
        cache_read_tokens = 0
        input_details = usage.get("input_tokens_details")
        if isinstance(input_details, dict):
            cache_read_tokens = int(input_details.get("cached_tokens") or 0)

        return UsageInfo(
            input_tokens=input_tokens,
            output_tokens=output_tokens,
            total_tokens=total_tokens,
            cache_read_tokens=cache_read_tokens,
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
        args_raw = item.get("arguments")
        if args_raw is None:
            args_raw = "{}"

        if isinstance(args_raw, str):
            if args_raw:
                try:
                    parsed = json.loads(args_raw)
                    tool_input = parsed if isinstance(parsed, dict) else {"_raw": parsed}
                except (json.JSONDecodeError, TypeError):
                    tool_input = {"_raw": args_raw}
            else:
                tool_input = {}
        elif isinstance(args_raw, dict):
            tool_input = args_raw
        else:
            tool_input = {}

        extra: dict[str, Any] = {
            "openai_cli": self._extract_extra(item, {"type", "call_id", "id", "name", "arguments"})
        }
        if isinstance(args_raw, str):
            extra["raw"] = {"arguments": args_raw}

        tool_block = ToolUseBlock(
            tool_id=tool_id,
            tool_name=tool_name,
            tool_input=tool_input,
            extra=extra,
        )
        return InternalMessage(role=Role.ASSISTANT, content=[tool_block])

    def _parse_function_call_output_item(self, item: dict[str, Any]) -> InternalMessage:
        tool_use_id = str(item.get("call_id") or item.get("id") or "")
        output = item.get("output")

        extra: dict[str, Any] = {
            "openai_cli": self._extract_extra(item, {"type", "call_id", "id", "output"})
        }
        if isinstance(output, str):
            extra["raw"] = {"content": output, "output": output}
            try:
                parsed = json.loads(output)
            except (json.JSONDecodeError, TypeError):
                parsed = None

            result_block = ToolResultBlock(
                tool_use_id=tool_use_id,
                output=parsed,
                content_text=None if parsed is not None else output,
                extra=extra,
            )
            return InternalMessage(role=Role.USER, content=[result_block])

        result_block = ToolResultBlock(
            tool_use_id=tool_use_id,
            output=output,
            content_text=None,
            extra=extra,
        )
        return InternalMessage(role=Role.USER, content=[result_block])

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
            if ptype in ("input_image", "output_image"):
                image_url = part.get("image_url") or part.get("url") or ""
                if isinstance(image_url, str) and image_url:
                    img = self._image_url_to_block(image_url)
                    img.extra.update(self._extract_extra(part, {"type", "image_url", "url"}))
                    blocks.append(img)
                else:
                    blocks.append(UnknownBlock(raw_type=ptype, payload=part))
                continue
            if ptype == "input_file":
                file_data = part.get("file_data")
                file_id = part.get("file_id")
                filename = part.get("filename")
                fb = FileBlock(filename=filename)
                if isinstance(file_data, str) and file_data:
                    # file_data 是 data URL: "data:mime;base64,..."
                    if file_data.startswith("data:") and ";base64," in file_data:
                        header, _, data = file_data.partition(",")
                        fb.media_type = header.split(";")[0].split(":", 1)[-1]
                        fb.data = data
                    else:
                        fb.data = file_data
                elif isinstance(file_id, str) and file_id:
                    fb.file_id = file_id
                fb.extra = self._extract_extra(part, {"type", "file_data", "file_id", "filename"})
                blocks.append(fb)
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
                    raw = block.extra.get("raw") if isinstance(block.extra, dict) else None
                    raw_arguments = raw.get("arguments") if isinstance(raw, dict) else None
                    arguments = (
                        raw_arguments
                        if isinstance(raw_arguments, str)
                        else _stable_json_dumps(block.tool_input or {})
                    )
                    out.append(
                        {
                            "type": "function_call",
                            "call_id": block.tool_id,
                            "name": block.tool_name,
                            "arguments": arguments,
                        }
                    )
                    continue

                if isinstance(block, ToolResultBlock):
                    # Responses API function_call_output.output 必须是字符串
                    raw = block.extra.get("raw") if isinstance(block.extra, dict) else None
                    raw_output = raw.get("output") if isinstance(raw, dict) else None
                    raw_content = raw.get("content") if isinstance(raw, dict) else None
                    if isinstance(raw_output, str):
                        output_str = raw_output
                    elif isinstance(raw_content, str):
                        output_str = raw_content
                    elif block.content_text is not None:
                        output_str = block.content_text
                    elif isinstance(block.output, str):
                        output_str = block.output
                    elif block.output is not None:
                        output_str = _stable_json_dumps(block.output)
                    else:
                        output_str = ""
                    out.append(
                        {
                            "type": "function_call_output",
                            "call_id": block.tool_use_id,
                            "output": output_str,
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
            has_content = False

            for block in msg.content:
                if isinstance(block, (ToolUseBlock, ToolResultBlock)):
                    continue  # 已在上面处理
                if isinstance(block, UnknownBlock) and block.raw_type == "reasoning":
                    continue  # 已在上面处理
                if isinstance(block, UnknownBlock):
                    continue  # 跳过其他未知块
                if isinstance(block, ImageBlock):
                    if block.data and block.media_type:
                        image_url = f"data:{block.media_type};base64,{block.data}"
                    elif block.url:
                        image_url = block.url
                    else:
                        continue
                    image_type = "output_image" if role == "assistant" else "input_image"
                    item: dict[str, Any] = {
                        "type": image_type,
                        "image_url": image_url,
                    }
                    if block.extra.get("detail"):
                        item["detail"] = block.extra["detail"]
                    content_items.append(item)
                    has_content = True
                    continue
                if isinstance(block, TextBlock) and block.text:
                    # assistant 角色使用 output_text，其他角色使用 input_text
                    text_type = "output_text" if role == "assistant" else "input_text"
                    content_items.append({"type": text_type, "text": block.text})
                    has_content = True

            if has_content:
                out.append({"type": "message", "role": role, "content": content_items})

        return out

    def _tools_to_internal(self, tools: Any) -> list[ToolDefinition] | None:
        if not isinstance(tools, list):
            return None
        out: list[ToolDefinition] = []
        for tool in tools:
            if not isinstance(tool, dict):
                continue

            tool_type = tool.get("type")

            # 标准嵌套结构: {type: "function", function: {name, description, parameters}}
            if tool_type == "function" and isinstance(tool.get("function"), dict):
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
                            "openai_cli_raw_tool": tool,
                            "openai_tool": self._extract_extra(tool, {"type", "function"}),
                            "openai_function": self._extract_extra(
                                fn, {"name", "description", "parameters"}
                            ),
                        },
                    )
                )
                continue

            # 非 function 类型（如 type: "custom", "web_search" 等）：保留原始 dict 以便透传还原
            if tool_type and tool_type != "function":
                name = str(tool.get("name") or "")
                if tool_type == "custom" and not name:
                    custom_raw = tool.get("custom")
                    if isinstance(custom_raw, dict):
                        name = str(custom_raw.get("name") or "")
                if not name and isinstance(tool_type, str) and tool_type:
                    name = tool_type
                if not name:
                    logger.debug(
                        "[OpenAICliNormalizer] 跳过无 name 的非 function tool: type={}",
                        tool_type,
                    )
                    continue
                out.append(
                    ToolDefinition(
                        name=name,
                        description=tool.get("description"),
                        parameters=(
                            tool.get("parameters")
                            if isinstance(tool.get("parameters"), dict)
                            else None
                        ),
                        extra={"openai_cli_raw_tool": tool},
                    )
                )
                continue

            # 兼容：扁平 function 结构 {name, description, parameters}（无 type 或 type=function）
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
                            "openai_cli_raw_tool": tool,
                            "openai_cli": self._extract_extra(
                                tool, {"name", "description", "parameters"}
                            ),
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
            if tool_choice == "required":
                return ToolChoice(
                    type=ToolChoiceType.REQUIRED,
                    extra={"openai_cli": {"tool_choice": tool_choice}},
                )
            return ToolChoice(type=ToolChoiceType.AUTO, extra={"raw": tool_choice})

        if isinstance(tool_choice, dict):
            # OpenAI 兼容结构：{"type":"function","function":{"name":"..."}}
            if tool_choice.get("type") == "function":
                if isinstance(tool_choice.get("function"), dict):
                    name = str(tool_choice["function"].get("name") or "")
                else:
                    name = str(tool_choice.get("name") or "")
                return ToolChoice(
                    type=ToolChoiceType.TOOL, tool_name=name, extra={"openai_cli": tool_choice}
                )
            if tool_choice.get("type") == "custom":
                name = str(
                    tool_choice.get("name")
                    or (
                        (tool_choice.get("custom") or {}).get("name")
                        if isinstance(tool_choice.get("custom"), dict)
                        else ""
                    )
                    or ""
                )
                return ToolChoice(
                    type=ToolChoiceType.TOOL, tool_name=name, extra={"openai_cli": tool_choice}
                )
            return ToolChoice(type=ToolChoiceType.AUTO, extra={"openai_cli": tool_choice})

        return ToolChoice(type=ToolChoiceType.AUTO, extra={"raw": tool_choice})

    def _tool_choice_to_openai(self, tool_choice: ToolChoice) -> str | dict[str, Any]:
        raw_responses_choice = tool_choice.extra.get("openai_cli")
        if isinstance(raw_responses_choice, dict) and "type" in raw_responses_choice:
            return raw_responses_choice
        raw_chat_choice = tool_choice.extra.get("openai")
        if isinstance(raw_chat_choice, dict):
            converted = _chat_tool_choice_to_responses(raw_chat_choice)
            if converted is not None:
                return converted
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

    _REQUEST_PREFIX_KEYS = ("model", "instructions", "tools", "input")

    def _reorder_request_prefix_keys(self, payload: dict[str, Any]) -> dict[str, Any]:
        return self._reorder_request_keys(payload, self._REQUEST_PREFIX_KEYS)

    def _error_type_from_value(self, value: str) -> ErrorType:
        for t in ErrorType:
            if t.value == value:
                return t
        return ErrorType.UNKNOWN


__all__ = ["OpenAICliNormalizer"]
