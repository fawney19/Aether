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

from src.core.api_format.conversion.constants import (
    OPENAI_CHAT_PASSTHROUGH_KEYS as _CHAT_PASSTHROUGH_KEYS,
)
from src.core.api_format.conversion.constants import OPENAI_HANDLED_KEYS as _HANDLED_KEYS
from src.core.api_format.conversion.constants import (
    responses_tool_choice_to_chat as _responses_tool_choice_to_chat,
)
from src.core.api_format.conversion.constants import (
    responses_tool_to_chat_tool as _responses_tool_to_chat_tool,
)
from src.core.api_format.conversion.constants import (
    responses_web_search_tool_to_chat_options as _responses_web_search_tool_to_chat_options,
)
from src.core.api_format.conversion.constants import stable_json_dumps as _stable_json_dumps
from src.core.api_format.conversion.field_mappings import (
    ERROR_TYPE_MAPPINGS,
    REASONING_EFFORT_TO_THINKING_BUDGET,
    RETRYABLE_ERROR_TYPES,
    THINKING_BUDGET_TO_REASONING_EFFORT,
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


def _normalize_chat_completion_id(raw_id: str) -> str:
    """将非 chatcmpl- 前缀的 ID 规范化为 Chat Completions 格式。

    OpenAI Chat Completions 响应的 id 约定使用 chatcmpl- 前缀，
    当上游返回 Responses API 格式（resp_ 前缀）或其他格式时需要转换。
    """
    if not raw_id:
        return "chatcmpl-unknown"
    if raw_id.startswith("chatcmpl-"):
        return raw_id
    if raw_id.startswith("resp_"):
        return "chatcmpl-" + raw_id[5:]
    return raw_id


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
        StopReason.REFUSAL: "content_filter",
        StopReason.PAUSE_TURN: "stop",
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

        verbosity = request.get("verbosity")
        if isinstance(verbosity, str) and verbosity:
            extra["verbosity"] = verbosity

        # 处理 extra_body.google (用于 Gemini 特定功能透传，如 thinkingConfig, responseModalities)
        extra_body = request.get("extra_body")
        if isinstance(extra_body, dict):
            google_extra = extra_body.get("google")
            if isinstance(google_extra, dict) and google_extra:
                extra["google"] = google_extra

        # reasoning_effort -> ThinkingConfig
        thinking: ThinkingConfig | None = None
        reasoning_effort = request.get("reasoning_effort")
        if (
            isinstance(reasoning_effort, str)
            and reasoning_effort in REASONING_EFFORT_TO_THINKING_BUDGET
        ):
            thinking = ThinkingConfig(
                enabled=True,
                budget_tokens=REASONING_EFFORT_TO_THINKING_BUDGET[reasoning_effort],
                extra={"reasoning_effort": reasoning_effort},
            )
            extra["reasoning_effort"] = reasoning_effort

        # web_search_options 存入 extra 供目标 normalizer 使用
        web_search_options = request.get("web_search_options")
        if isinstance(web_search_options, dict):
            extra["web_search_options"] = web_search_options

        # parallel_tool_calls
        parallel_tool_calls: bool | None = None
        ptc = request.get("parallel_tool_calls")
        if ptc is not None:
            parallel_tool_calls = bool(ptc)

        # 采样参数
        n_value = self._optional_int(request.get("n"))
        presence_penalty = self._optional_float(request.get("presence_penalty"))
        frequency_penalty = self._optional_float(request.get("frequency_penalty"))
        seed = self._optional_int(request.get("seed"))
        logprobs = request.get("logprobs")
        logprobs_val: bool | None = bool(logprobs) if logprobs is not None else None
        top_logprobs = self._optional_int(request.get("top_logprobs"))

        # response_format
        response_format: ResponseFormatConfig | None = None
        rf = request.get("response_format")
        if isinstance(rf, dict):
            rf_type = str(rf.get("type") or "text")
            rf_schema = rf.get("json_schema") if isinstance(rf.get("json_schema"), dict) else None
            if rf_type != "text":
                response_format = ResponseFormatConfig(
                    type=rf_type,
                    json_schema=rf_schema,
                    extra=self._extract_extra(rf, {"type", "json_schema"}),
                )

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
            thinking=thinking,
            parallel_tool_calls=parallel_tool_calls,
            n=n_value,
            presence_penalty=presence_penalty,
            frequency_penalty=frequency_penalty,
            seed=seed,
            logprobs=logprobs_val,
            top_logprobs=top_logprobs,
            response_format=response_format,
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
            openai_tools: list[dict[str, Any]] = []
            for t in internal.tools:
                # 跳过 Gemini 内置工具（在 OpenAI 中无对应物）
                if t.extra.get("gemini_builtin_tool"):
                    continue
                raw_chat_tool = t.extra.get("openai_chat_raw_tool")
                if isinstance(raw_chat_tool, dict):
                    openai_tools.append(raw_chat_tool)
                    continue
                raw_responses_tool = t.extra.get("openai_cli_raw_tool")
                if isinstance(raw_responses_tool, dict):
                    if web_search_options := _responses_web_search_tool_to_chat_options(
                        raw_responses_tool
                    ):
                        result["web_search_options"] = web_search_options
                    if chat_tool := _responses_tool_to_chat_tool(raw_responses_tool):
                        openai_tools.append(chat_tool)
                    continue
                func: dict[str, Any] = {
                    **(t.extra.get("openai_function") or {}),
                    "name": t.name,
                }
                if t.description is not None:
                    func["description"] = t.description
                if t.parameters is not None:
                    func["parameters"] = t.parameters
                openai_tools.append(
                    {
                        "type": "function",
                        "function": func,
                        **(t.extra.get("openai_tool") or {}),
                    }
                )
            if openai_tools:
                result["tools"] = openai_tools

        if internal.tool_choice:
            result["tool_choice"] = self._tool_choice_to_openai(internal.tool_choice)

        # thinking -> reasoning_effort
        effort: str | None = None
        if internal.thinking and internal.thinking.enabled:
            effort = internal.thinking.extra.get("reasoning_effort")
        # 显式 reasoning_effort 应优先于 budget 反推，避免覆盖 Claude output_config.effort
        if not effort and internal.extra:
            effort = internal.extra.get("reasoning_effort")
        if (
            not effort
            and internal.thinking
            and internal.thinking.enabled
            and internal.thinking.budget_tokens is not None
        ):
            for threshold, level in THINKING_BUDGET_TO_REASONING_EFFORT:
                if internal.thinking.budget_tokens <= threshold:
                    effort = level
                    break
        if effort:
            # OpenAI Chat Completions 仅支持 low/medium/high，xhigh 降级为 high
            if effort == "xhigh":
                effort = "high"
            result["reasoning_effort"] = effort

        # parallel_tool_calls
        if internal.parallel_tool_calls is not None:
            result["parallel_tool_calls"] = internal.parallel_tool_calls

        # 采样参数
        if internal.n is not None and internal.n > 1:
            result["n"] = internal.n
        for attr, key in (
            ("presence_penalty", "presence_penalty"),
            ("frequency_penalty", "frequency_penalty"),
            ("seed", "seed"),
            ("logprobs", "logprobs"),
            ("top_logprobs", "top_logprobs"),
        ):
            val = getattr(internal, attr, None)
            if val is not None:
                result[key] = val

        # response_format
        if internal.response_format and internal.response_format.type != "text":
            rf: dict[str, Any] = {"type": internal.response_format.type}
            if internal.response_format.json_schema:
                rf["json_schema"] = internal.response_format.json_schema
            result["response_format"] = rf

        verbosity = internal.extra.get("verbosity") if internal.extra else None
        if isinstance(verbosity, str) and verbosity:
            result["verbosity"] = verbosity

        openai_extra = internal.extra.get("openai", {}) if internal.extra else {}
        openai_cli_extra = internal.extra.get("openai_cli", {}) if internal.extra else {}

        if isinstance(openai_cli_extra, dict):
            text_config = openai_cli_extra.get("text")
            if isinstance(text_config, dict):
                if "response_format" not in result:
                    text_format = text_config.get("format")
                    if isinstance(text_format, dict) and text_format.get("type"):
                        result["response_format"] = text_format
                if "verbosity" not in result:
                    text_verbosity = text_config.get("verbosity")
                    if isinstance(text_verbosity, str) and text_verbosity:
                        result["verbosity"] = text_verbosity

        # 还原 OpenAI Chat 特有的透传字段（先到先得，与 openai_cli 一致）
        if isinstance(openai_extra, dict):
            for key, value in openai_extra.items():
                if key in _CHAT_PASSTHROUGH_KEYS and key not in result and key not in _HANDLED_KEYS:
                    result[key] = value
        if isinstance(openai_cli_extra, dict):
            for key, value in openai_cli_extra.items():
                if key in _CHAT_PASSTHROUGH_KEYS and key not in result and key not in _HANDLED_KEYS:
                    result[key] = value

        return result

    # =========================
    # Responses
    # =========================

    def response_to_internal(self, response: dict[str, Any]) -> InternalResponse:
        rid = str(response.get("id") or "")
        model = str(response.get("model") or "")

        extra: dict[str, Any] = {}

        # 保留 system_fingerprint / service_tier 以便 roundtrip 还原
        sys_fp = response.get("system_fingerprint")
        if sys_fp is not None:
            extra.setdefault("openai", {})["system_fingerprint"] = sys_fp
        svc_tier = response.get("service_tier")
        if svc_tier is not None:
            extra.setdefault("openai", {})["service_tier"] = svc_tier

        choices = response.get("choices") or []
        if isinstance(choices, list) and len(choices) > 1:
            extra.setdefault("openai", {})["choices"] = choices

        choice0 = choices[0] if isinstance(choices, list) and choices else {}
        message = choice0.get("message") if isinstance(choice0, dict) else None
        message = message if isinstance(message, dict) else {}

        # reasoning_content -> ThinkingBlock
        reasoning_content = message.get("reasoning_content")
        reasoning_blocks: list[ContentBlock] = []
        if (
            isinstance(reasoning_content, str)
            and reasoning_content
            and reasoning_content != "[undefined]"
        ):
            reasoning_blocks.append(ThinkingBlock(thinking=reasoning_content))

        blocks, dropped = self._openai_content_to_blocks(message.get("content"))

        # thinking blocks first (align with Claude thinking-first convention)
        if reasoning_blocks:
            blocks = reasoning_blocks + blocks

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
            "id": _normalize_chat_completion_id(internal.id or ""),
            "object": "chat.completion",
            "created": int(time.time()),
            "model": model_name,
            "system_fingerprint": None,
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
        # assistant 有 tool_calls 时 content 允许为 null；否则回退为空字符串
        if content_value is not None:
            message["content"] = content_value
        elif tool_blocks:
            message["content"] = None
        else:
            message["content"] = ""

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
            usage_out: dict[str, Any] = {
                "prompt_tokens": int(internal.usage.input_tokens),
                "completion_tokens": int(internal.usage.output_tokens),
                "total_tokens": int(total),
            }
            # 回写 prompt_tokens_details（cached_tokens）
            if internal.usage.cache_read_tokens:
                usage_out["prompt_tokens_details"] = {
                    "cached_tokens": int(internal.usage.cache_read_tokens),
                }
            # 回写 completion_tokens_details
            openai_extra = internal.usage.extra.get("openai", {})
            if isinstance(openai_extra, dict):
                ctd = openai_extra.get("completion_tokens_details")
                if isinstance(ctd, dict):
                    usage_out["completion_tokens_details"] = ctd
            out["usage"] = usage_out

        # 还原 system_fingerprint / service_tier（roundtrip 保留）
        openai_resp_extra = internal.extra.get("openai", {})
        if isinstance(openai_resp_extra, dict):
            sfp = openai_resp_extra.get("system_fingerprint")
            if sfp is not None:
                out["system_fingerprint"] = sfp
            st = openai_resp_extra.get("service_tier")
            if st is not None:
                out["service_tier"] = st

        return out

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
            # index -> tool_id 映射（用于后续 delta 缺少 id 时查找）
            index_to_id: dict[str, str] = ss.setdefault("tool_index_to_id", {})

            for tool_call in tool_calls:
                if not isinstance(tool_call, dict):
                    continue

                tc_id = str(tool_call.get("id") or "")
                raw_index = tool_call.get("index")
                tc_index = str(raw_index) if raw_index is not None else ""
                fn = tool_call.get("function") or {}
                fn = fn if isinstance(fn, dict) else {}
                tc_name = str(fn.get("name") or "")
                tc_args = fn.get("arguments")

                # 首次出现的 delta 同时有 id 和 index，记录映射
                if tc_id and tc_index:
                    index_to_id[tc_index] = tc_id
                # 后续 delta 只有 index 没有 id，通过映射恢复 id
                elif not tc_id and tc_index and tc_index in index_to_id:
                    tc_id = index_to_id[tc_index]

                # 用 tool_id 作为优先 key（确保同一 tool call 始终同一 block_index）
                tool_key = tc_id or tc_index
                block_index = self._ensure_tool_block_index(ss, tool_key)

                # tool start（只在首次见到该 tool_key 时发）
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
            # 先补齐 content_block_stop（thinking + text + tool_calls），再发送 MessageStop
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
            # 补齐所有已开始但未结束的 tool_call block
            tool_id_map = ss.get("tool_id_to_block_index")
            if isinstance(tool_id_map, dict):
                for _tk, bi in tool_id_map.items():
                    stopped_key = f"tool_stopped:{bi}"
                    if ss.get(f"tool_started:{bi}") and not ss.get(stopped_key):
                        ss[stopped_key] = True
                        events.append(ContentBlockStopEvent(block_index=int(bi)))
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
                "id": _normalize_chat_completion_id(state.message_id or "chatcmpl-stream"),
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
                    out.append(base_chunk({"reasoning_content": event.text_delta}))
                else:
                    out.append(base_chunk({"content": event.text_delta}))
            return out

        if isinstance(event, ContentBlockStartEvent) and event.block_type == ContentType.TOOL_USE:
            tool_id = event.tool_id or ""
            tool_name = event.tool_name or ""
            tool_index = self._ensure_tool_call_index(ss, tool_id, event.block_index)
            if tool_id:
                self._ss_dict(ss, "block_to_tool_id")[int(event.block_index)] = str(tool_id)
            if tool_name:
                self._ss_dict(ss, "block_to_tool_name")[int(event.block_index)] = str(tool_name)
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
                image_blocks = self._ss_dict(ss, "image_blocks")
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
            tool_id = str(event.tool_id or "")
            block_to_tool_id = self._ss_dict(ss, "block_to_tool_id")
            if not tool_id:
                tool_id = str(block_to_tool_id.get(int(event.block_index)) or "")
            else:
                block_to_tool_id[int(event.block_index)] = tool_id

            tool_name = str(
                self._ss_dict(ss, "block_to_tool_name").get(int(event.block_index)) or ""
            )

            tool_index = self._ensure_tool_call_index(ss, tool_id, event.block_index)
            # 对严格客户端重复携带 id/type/name，避免它们把后续 delta 视为无效 tool_call。
            tc_delta: dict[str, Any] = {
                "index": tool_index,
                "type": "function",
                "function": {"arguments": event.input_delta},
            }
            if tool_id:
                tc_delta["id"] = tool_id
            if tool_name:
                tc_delta["function"]["name"] = tool_name
            out.append(base_chunk({"tool_calls": [tc_delta]}))
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
                    extra=self._extract_extra(msg, {"role", "content", "tool_call_id"}),
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

            if ptype == "file":
                # 标准 OpenAI 格式: {"type": "file", "file": {"file_id": ...}}
                # 或 {"type": "file", "file": {"file_data": "data:mime;base64,...", "filename": ...}}
                file_obj = part.get("file")
                if isinstance(file_obj, dict):
                    fid = file_obj.get("file_id")
                    fdata = file_obj.get("file_data")
                    fname = file_obj.get("filename")
                    if isinstance(fdata, str) and fdata:
                        # file_data 是 data URL 格式: "data:mime;base64,..."
                        mime_type = None
                        raw_data = fdata
                        if fdata.startswith("data:") and ";base64," in fdata:
                            header, raw_data = fdata.split(";base64,", 1)
                            mime_type = header[len("data:") :]
                        blocks.append(
                            FileBlock(
                                data=raw_data,
                                media_type=mime_type,
                                filename=fname,
                                extra=self._extract_extra(part, {"type", "file"}),
                            )
                        )
                    elif isinstance(fid, str) and fid:
                        blocks.append(
                            FileBlock(
                                file_id=fid,
                                filename=fname,
                                extra=self._extract_extra(part, {"type", "file"}),
                            )
                        )
                    else:
                        blocks.append(UnknownBlock(raw_type="file", payload=part))
                else:
                    blocks.append(UnknownBlock(raw_type="file", payload=part))
                continue

            if ptype == "input_audio":
                audio_data = part.get("input_audio") or {}
                if isinstance(audio_data, dict):
                    audio_fmt = str(audio_data.get("format") or "")
                    blocks.append(
                        AudioBlock(
                            data=audio_data.get("data"),
                            media_type=f"audio/{audio_fmt}" if audio_fmt else None,
                            format=audio_fmt or None,
                            extra=self._extract_extra(part, {"type", "input_audio"}),
                        )
                    )
                else:
                    blocks.append(UnknownBlock(raw_type="input_audio", payload=part))
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
                if tool.get("type") == "custom" and isinstance(tool.get("custom"), dict):
                    custom = tool["custom"]
                    name = str(custom.get("name") or "")
                    if not name:
                        continue
                    out.append(
                        ToolDefinition(
                            name=name,
                            description=custom.get("description"),
                            parameters=None,
                            extra={
                                "openai_chat_raw_tool": tool,
                                "openai_tool_kind": "custom",
                            },
                        )
                    )
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
                        "openai_chat_raw_tool": tool,
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
        if isinstance(tool_choice, dict) and tool_choice.get("type") == "custom":
            custom_raw = tool_choice.get("custom")
            custom: dict[str, Any] = custom_raw if isinstance(custom_raw, dict) else {}
            name = str(custom.get("name") or "")
            return ToolChoice(
                type=ToolChoiceType.TOOL, tool_name=name, extra={"openai": tool_choice}
            )

        return ToolChoice(type=ToolChoiceType.AUTO, extra={"openai": tool_choice})

    def _tool_choice_to_openai(self, tool_choice: ToolChoice) -> str | dict[str, Any]:
        raw_chat_choice = tool_choice.extra.get("openai")
        if isinstance(raw_chat_choice, dict):
            return raw_chat_choice
        raw_responses_choice = tool_choice.extra.get("openai_cli")
        if isinstance(raw_responses_choice, dict) and "type" in raw_responses_choice:
            if chat_choice := _responses_tool_choice_to_chat(raw_responses_choice):
                return chat_choice
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
        args_raw = fn.get("arguments") if isinstance(fn, dict) else None
        tool_id = str(tool_call.get("id") or "")

        tool_input: dict[str, Any]
        if isinstance(args_raw, str):
            if args_raw:
                try:
                    parsed = json.loads(args_raw)
                    tool_input = parsed if isinstance(parsed, dict) else {"raw": parsed}
                except json.JSONDecodeError:
                    tool_input = {"raw": args_raw}
            else:
                tool_input = {}
        else:
            tool_input = {}

        extra: dict[str, Any] = {"openai": tool_call}
        if isinstance(args_raw, str):
            extra["raw"] = {"arguments": args_raw}

        return (
            ToolUseBlock(
                tool_id=tool_id,
                tool_name=name,
                tool_input=tool_input,
                extra=extra,
            ),
            dropped,
        )

    def _legacy_function_call_to_block(
        self, func_call: dict[str, Any]
    ) -> tuple[ToolUseBlock | None, dict[str, int]]:
        dropped: dict[str, int] = {}
        name = str(func_call.get("name") or "")
        args_raw = func_call.get("arguments")
        if not name:
            dropped["openai_function_call_missing_name"] = (
                dropped.get("openai_function_call_missing_name", 0) + 1
            )
            return None, dropped

        tool_input: dict[str, Any]
        if isinstance(args_raw, str):
            if args_raw:
                try:
                    parsed = json.loads(args_raw)
                    tool_input = parsed if isinstance(parsed, dict) else {"raw": parsed}
                except json.JSONDecodeError:
                    tool_input = {"raw": args_raw}
            else:
                tool_input = {}
        else:
            tool_input = {}

        extra: dict[str, Any] = {"openai": {"function_call": func_call}}
        if isinstance(args_raw, str):
            extra["raw"] = {"arguments": args_raw}

        return (
            ToolUseBlock(
                tool_id="call_0",
                tool_name=name,
                tool_input=tool_input,
                extra=extra,
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

            raw_extra = {"raw": {"content": content}, "openai": msg}
            if parsed is not None:
                return (
                    ToolResultBlock(
                        tool_use_id=tool_call_id,
                        output=parsed,
                        content_text=None,
                        extra=raw_extra,
                    ),
                    dropped,
                )

            return (
                ToolResultBlock(
                    tool_use_id=tool_call_id,
                    output=None,
                    content_text=content,
                    extra=raw_extra,
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

        # prompt_tokens_details -> cache_read_tokens
        cache_read = 0
        ptd = usage.get("prompt_tokens_details")
        if isinstance(ptd, dict):
            cache_read = int(ptd.get("cached_tokens") or 0)

        # completion_tokens_details（reasoning_tokens 等存入 extra）
        ctd = usage.get("completion_tokens_details")

        extra = self._extract_extra(
            usage,
            {
                "prompt_tokens",
                "completion_tokens",
                "total_tokens",
                "prompt_tokens_details",
                "completion_tokens_details",
            },
        )

        # 保留 completion_tokens_details 细分到 extra
        if isinstance(ctd, dict):
            extra["completion_tokens_details"] = ctd

        return UsageInfo(
            input_tokens=input_tokens,
            output_tokens=output_tokens,
            total_tokens=total_tokens_int,
            cache_read_tokens=cache_read,
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
                if b.data and b.media_type:
                    # base64 data -> data URL in image_url format
                    data_url = f"data:{b.media_type};base64,{b.data}"
                    parts.append({"type": "image_url", "image_url": {"url": data_url}})
                elif b.url:
                    parts.append({"type": "image_url", "image_url": {"url": b.url}})
                continue

            if isinstance(b, FileBlock):
                if b.data and b.media_type:
                    # 标准 OpenAI 格式: {"type": "file", "file": {"file_data": "data:mime;base64,...", "filename": ...}}
                    data_url = f"data:{b.media_type};base64,{b.data}"
                    file_inner: dict[str, Any] = {"file_data": data_url}
                    if b.filename:
                        file_inner["filename"] = b.filename
                    parts.append({"type": "file", "file": file_inner})
                elif b.file_id:
                    file_inner_id: dict[str, Any] = {"file_id": b.file_id}
                    if b.filename:
                        file_inner_id["filename"] = b.filename
                    parts.append({"type": "file", "file": file_inner_id})
                elif b.file_url:
                    # 回退为文本描述
                    text_parts.append(f"[File: {b.file_url}]")
                continue

            if isinstance(b, AudioBlock):
                if b.data:
                    audio_fmt = b.format or (
                        b.media_type.split("/", 1)[1]
                        if b.media_type and "/" in b.media_type
                        else "mp3"
                    )
                    parts.append(
                        {
                            "type": "input_audio",
                            "input_audio": {
                                "data": b.data,
                                "format": audio_fmt,
                            },
                        }
                    )
                continue

            # Unknown / Tool blocks 不进入 OpenAI content

        # 如果有 OpenAI 原生格式的图片/文件/音频（multipart content parts），使用 multipart content
        if parts:
            if text_parts:
                parts = [{"type": "text", "text": "\n".join(text_parts)}] + parts
            return parts

        # 纯文本或包含 markdown 图片
        if text_parts:
            return "\n".join(text_parts)

        # 无任何内容：返回 None，让调用方决定是输出 null 还是空字符串
        # （assistant 有 tool_calls 时 content 应为 null；user 消息 content 可为空字符串）
        return None

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
                continue
            if isinstance(b, UnknownBlock):
                continue
            # TextBlock, ImageBlock, FileBlock, AudioBlock -> content
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
        # assistant 有 tool_calls 时 content 允许为 null；否则回退为空字符串
        if content_value is not None:
            out["content"] = content_value
        elif tool_blocks:
            out["content"] = None
        else:
            out["content"] = ""

        if tool_blocks:
            out["tool_calls"] = [
                self._tool_use_block_to_openai_call(b, idx) for idx, b in enumerate(tool_blocks)
            ]

        return out

    def _tool_result_block_to_openai_message(self, block: ToolResultBlock) -> dict[str, Any]:
        content: str
        raw = block.extra.get("raw") if isinstance(block.extra, dict) else None
        raw_content = raw.get("content") if isinstance(raw, dict) else None
        if isinstance(raw_content, str):
            content = raw_content
        elif block.content_text is not None:
            content = block.content_text
        elif block.output is None:
            content = ""
        elif isinstance(block.output, str):
            content = block.output
        else:
            content = _stable_json_dumps(block.output)

        return {
            "role": "tool",
            "tool_call_id": block.tool_use_id,
            "content": content,
        }

    def _tool_use_block_to_openai_call(self, block: ToolUseBlock, index: int) -> dict[str, Any]:
        # index 参数仅用于 fallback id 生成；非流式响应的 tool_calls 数组不应包含 index 字段
        raw = block.extra.get("raw") if isinstance(block.extra, dict) else None
        raw_arguments = raw.get("arguments") if isinstance(raw, dict) else None
        arguments = (
            raw_arguments
            if isinstance(raw_arguments, str)
            else _stable_json_dumps(block.tool_input or {})
        )
        return {
            "id": block.tool_id or f"call_{index}",
            "type": "function",
            "function": {
                "name": block.tool_name,
                "arguments": arguments,
            },
        }

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

    @staticmethod
    def _ss_dict(ss: dict[str, Any], key: str) -> dict:
        """Get or create a dict entry in stream state."""
        val = ss.get(key)
        if not isinstance(val, dict):
            val = {}
            ss[key] = val
        return val

    def _ensure_tool_block_index(self, ss: dict[str, Any], tool_key: str) -> int:
        mapping = self._ss_dict(ss, "tool_id_to_block_index")

        if tool_key in mapping:
            return int(mapping[tool_key])

        next_idx = int(ss.get("next_block_index") or 0)
        mapping[tool_key] = next_idx
        ss["next_block_index"] = next_idx + 1
        return next_idx

    def _ensure_tool_call_index(
        self, ss: dict[str, Any], tool_id: str, block_index: int | None = None
    ) -> int:
        mapping: dict[str, int] = ss.get("tool_id_to_index")  # type: ignore[assignment]
        if not isinstance(mapping, dict):
            mapping = {}
            ss["tool_id_to_index"] = mapping
            ss["next_tool_index"] = 0

        # block_index -> tool_index 的辅助映射，用于 tool_id 丢失时回落
        block_mapping: dict[int, int] = self._ss_dict(ss, "block_to_tool_index")  # type: ignore[assignment]

        # 优先用 tool_id 查找
        if tool_id and tool_id in mapping:
            return mapping[tool_id]

        # tool_id 为空时，用 block_index 回落查找
        if not tool_id and block_index is not None and block_index in block_mapping:
            return block_mapping[block_index]

        # 首次出现：分配新 index
        next_idx = int(ss.get("next_tool_index") or 0)
        if tool_id:
            mapping[tool_id] = next_idx
        if block_index is not None:
            block_mapping[block_index] = next_idx
        ss["next_tool_index"] = next_idx + 1
        return next_idx
