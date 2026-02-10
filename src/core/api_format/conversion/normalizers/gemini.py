"""
Gemini (GenerateContent / streamGenerateContent) Normalizer

负责：
- Gemini request/response <-> Internal 表示转换
- 可选：Gemini streaming chunk <-> InternalStreamEvent
- 可选：Gemini error <-> InternalError

说明：
- 请求体字段在本项目中同时兼容 snake_case（历史转换器产物）与 camelCase（官方/客户端输入）。
- 响应/流式通常为 camelCase（candidates/finishReason/usageMetadata/modelVersion）。
"""

import json
from typing import Any

from src.core.api_format.conversion.field_mappings import (
    ERROR_TYPE_MAPPINGS,
    RETRYABLE_ERROR_TYPES,
    STOP_REASON_MAPPINGS,
    USAGE_FIELD_MAPPINGS,
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
from src.core.api_format.schema_utils import clean_gemini_schema as _clean_gemini_schema

# Valid Gemini Part data-oneof field names (camelCase + snake_case).
_VALID_PART_DATA_FIELDS = frozenset(
    {
        "text",
        "inlineData",
        "inline_data",
        "functionCall",
        "function_call",
        "functionResponse",
        "function_response",
        "fileData",
        "file_data",
        "executableCode",
        "executable_code",
        "codeExecutionResult",
        "code_execution_result",
        "videoMetadata",
        "video_metadata",
    }
)


def _is_valid_gemini_part(part: Any) -> bool:
    """Return True if *part* has at least one recognised Gemini data field."""
    if not isinstance(part, dict) or not part:
        return False
    return bool(part.keys() & _VALID_PART_DATA_FIELDS)


def compact_gemini_contents(contents: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """Strip invalid parts, drop empty contents, merge consecutive same-role.

    Gemini requires every content to have at least one valid part (with a
    recognised data-oneof field), and strictly alternating user/model roles.
    Cross-format conversions (e.g. Responses API reasoning items) may produce
    contents with no Gemini-compatible parts, or consecutive same-role entries
    after filtering.
    """
    # 1. Strip invalid parts, then drop contents with no valid parts remaining.
    #    Use shallow copy to avoid mutating the caller's original dicts.
    non_empty: list[dict[str, Any]] = []
    for c in contents:
        parts = c.get("parts")
        if not isinstance(parts, list):
            continue
        valid_parts = [p for p in parts if _is_valid_gemini_part(p)]
        if valid_parts:
            c = {**c, "parts": valid_parts}
            non_empty.append(c)

    # 2. Merge consecutive same-role entries.
    if not non_empty:
        return non_empty

    merged: list[dict[str, Any]] = [non_empty[0]]
    for c in non_empty[1:]:
        if c.get("role") == merged[-1].get("role"):
            prev_parts = merged[-1].get("parts")
            if isinstance(prev_parts, list):
                prev_parts.extend(c.get("parts") or [])
            else:
                merged[-1]["parts"] = list(c.get("parts") or [])
        else:
            merged.append(c)

    return merged


class GeminiNormalizer(FormatNormalizer):
    FORMAT_ID = "gemini:chat"
    capabilities = FormatCapabilities(
        supports_stream=True,
        supports_error_conversion=True,
        supports_tools=True,
        supports_images=True,
    )

    _FINISH_REASON_TO_STOP: dict[str, StopReason] = {
        "STOP": StopReason.END_TURN,
        "MAX_TOKENS": StopReason.MAX_TOKENS,
        "SAFETY": StopReason.CONTENT_FILTERED,
        "RECITATION": StopReason.CONTENT_FILTERED,
        "MALFORMED_FUNCTION_CALL": StopReason.TOOL_USE,
        "OTHER": StopReason.UNKNOWN,
    }

    _ERROR_TYPE_TO_GEMINI_STATUS: dict[ErrorType, str] = {
        ErrorType.INVALID_REQUEST: "INVALID_ARGUMENT",
        ErrorType.AUTHENTICATION: "UNAUTHENTICATED",
        ErrorType.PERMISSION_DENIED: "PERMISSION_DENIED",
        ErrorType.NOT_FOUND: "NOT_FOUND",
        ErrorType.RATE_LIMIT: "RESOURCE_EXHAUSTED",
        ErrorType.OVERLOADED: "UNAVAILABLE",
        ErrorType.SERVER_ERROR: "INTERNAL",
        ErrorType.CONTENT_FILTERED: "FAILED_PRECONDITION",
        ErrorType.CONTEXT_LENGTH_EXCEEDED: "INVALID_ARGUMENT",
        ErrorType.UNKNOWN: "INTERNAL",
    }

    # =========================
    # Requests
    # =========================

    def request_to_internal(self, request: dict[str, Any]) -> InternalRequest:
        model = str(request.get("model") or "")
        dropped: dict[str, int] = {}

        instructions: list[InstructionSegment] = []
        system_text, sys_dropped = self._collapse_system_instruction(
            request.get("system_instruction")
            if "system_instruction" in request
            else request.get("systemInstruction")
        )
        self._merge_dropped(dropped, sys_dropped)
        if system_text:
            instructions.append(InstructionSegment(role=Role.SYSTEM, text=system_text))

        messages: list[InternalMessage] = []
        contents = request.get("contents") or []
        if isinstance(contents, list):
            for content in contents:
                if not isinstance(content, dict):
                    dropped["gemini_content_non_dict"] = (
                        dropped.get("gemini_content_non_dict", 0) + 1
                    )
                    continue
                imsg, md = self._content_to_internal_message(content)
                self._merge_dropped(dropped, md)
                if imsg is not None:
                    messages.append(imsg)
        else:
            dropped["gemini_contents_non_list"] = dropped.get("gemini_contents_non_list", 0) + 1

        generation_config = self._get_generation_config(request)

        max_tokens = self._optional_int(
            generation_config.get("max_output_tokens")
            if isinstance(generation_config, dict)
            else None
        )
        temperature = self._optional_float(
            generation_config.get("temperature") if isinstance(generation_config, dict) else None
        )
        top_p = self._optional_float(
            generation_config.get("top_p") if isinstance(generation_config, dict) else None
        )
        top_k = self._optional_int(
            generation_config.get("top_k") if isinstance(generation_config, dict) else None
        )
        stop_sequences = None
        if isinstance(generation_config, dict):
            stop_sequences = self._coerce_str_list(generation_config.get("stop_sequences"))

        tools = self._gemini_tools_to_internal(request.get("tools"))
        tool_choice = self._gemini_tool_config_to_tool_choice(
            request.get("tool_config") if "tool_config" in request else request.get("toolConfig")
        )

        # 构建 extra，保留原始 gemini 字段
        extra: dict[str, Any] = {"gemini": self._extract_extra(request, {"contents"})}

        # 保留 generationConfig 中的特殊字段（responseModalities, thinkingConfig 等）
        # 这些字段在 _get_generation_config 中已提取，需要单独存储以便转换时使用
        if isinstance(generation_config, dict):
            response_modalities = generation_config.get("response_modalities")
            thinking_config = generation_config.get("thinking_config")
            if response_modalities or thinking_config:
                google_extra: dict[str, Any] = {}
                if response_modalities:
                    google_extra["response_modalities"] = response_modalities
                if thinking_config:
                    google_extra["thinking_config"] = thinking_config
                extra["google"] = google_extra

        internal = InternalRequest(
            model=model,
            messages=messages,
            instructions=instructions,
            system=self._join_instructions(instructions),
            max_tokens=max_tokens,
            temperature=temperature,
            top_p=top_p,
            top_k=top_k,
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
        system_text = internal.system or self._join_instructions(internal.instructions)

        target_variant_norm = str(target_variant or "").strip().lower()
        is_antigravity = target_variant_norm == "antigravity"
        allow_dummy_thought = bool(
            is_antigravity and str(internal.model or "").startswith("gemini-")
        )
        thinking_enabled = (
            self._is_antigravity_thinking_enabled(internal) if is_antigravity else False
        )

        # tools/tool_choice — clean unsupported JSON Schema fields from parameters
        tools = None
        if internal.tools:
            func_decls: list[dict[str, Any]] = []
            for t in internal.tools:
                params = dict(t.parameters) if t.parameters else {}
                if params:
                    _clean_gemini_schema(params)
                decl: dict[str, Any] = {
                    "name": t.name,
                    "description": t.description,
                    "parameters": params,
                    **(t.extra.get("gemini_function_declaration") or {}),
                }
                func_decls.append(decl)
            tools = [{"function_declarations": func_decls}]

        tool_config = None
        if internal.tool_choice:
            tool_config = self._tool_choice_to_gemini_tool_config(internal.tool_choice)

        generation_config: dict[str, Any] = {}
        if internal.max_tokens is not None:
            generation_config["max_output_tokens"] = internal.max_tokens
        if internal.temperature is not None:
            generation_config["temperature"] = internal.temperature
        if internal.top_p is not None:
            generation_config["top_p"] = internal.top_p
        if internal.top_k is not None:
            generation_config["top_k"] = internal.top_k
        if internal.stop_sequences:
            generation_config["stop_sequences"] = list(internal.stop_sequences)

        # 从 internal.extra["google"] 读取 OpenAI extra_body.google 透传的配置
        google_extra = internal.extra.get("google", {})
        if isinstance(google_extra, dict):
            # 处理 thinking_config -> thinkingConfig
            thinking_config = google_extra.get("thinking_config")
            if isinstance(thinking_config, dict):
                # snake_case -> camelCase 转换
                gemini_thinking: dict[str, Any] = {}
                if "thinking_budget" in thinking_config:
                    gemini_thinking["thinkingBudget"] = thinking_config["thinking_budget"]
                if "include_thoughts" in thinking_config:
                    gemini_thinking["includeThoughts"] = thinking_config["include_thoughts"]
                # 保留其他可能的字段
                for k, v in thinking_config.items():
                    if k not in ("thinking_budget", "include_thoughts"):
                        gemini_thinking[k] = v
                if gemini_thinking:
                    generation_config["thinkingConfig"] = gemini_thinking

            # 处理 response_modalities -> responseModalities
            response_modalities = google_extra.get("response_modalities")
            if response_modalities:
                generation_config["responseModalities"] = response_modalities

        # 从 internal.extra["gemini"] 读取原生 Gemini 配置（Gemini -> Gemini 场景）
        gemini_extra = internal.extra.get("gemini", {})
        if isinstance(gemini_extra, dict):
            # 保留原生 Gemini generationConfig 中的额外字段
            orig_gc = gemini_extra.get("generation_config") or gemini_extra.get("generationConfig")
            if isinstance(orig_gc, dict):
                # responseModalities
                if (
                    "responseModalities" in orig_gc
                    and "responseModalities" not in generation_config
                ):
                    generation_config["responseModalities"] = orig_gc["responseModalities"]
                if (
                    "response_modalities" in orig_gc
                    and "responseModalities" not in generation_config
                ):
                    generation_config["responseModalities"] = orig_gc["response_modalities"]
                # thinkingConfig
                if "thinkingConfig" in orig_gc and "thinkingConfig" not in generation_config:
                    generation_config["thinkingConfig"] = orig_gc["thinkingConfig"]
                if "thinking_config" in orig_gc and "thinkingConfig" not in generation_config:
                    generation_config["thinkingConfig"] = orig_gc["thinking_config"]

        raw_contents: list[dict[str, Any]] = []
        last_idx = len(internal.messages) - 1
        for idx, msg in enumerate(internal.messages):
            content = self._internal_message_to_content(
                msg,
                target_variant=target_variant_norm,
                model=internal.model,
            )

            # Antigravity: Gemini models allow a dummy thought signature as a workaround
            # for strict thought signature validation. Only apply to the last assistant
            # turn (prefill scenario) when thinking is enabled and no thought part exists.
            if (
                allow_dummy_thought
                and thinking_enabled
                and idx == last_idx
                and content.get("role") == "model"
            ):
                parts = content.get("parts")
                if isinstance(parts, list) and parts:
                    has_thought = any(
                        isinstance(p, dict) and p.get("thought") is True for p in parts
                    )
                    if not has_thought:
                        try:
                            from src.services.provider.adapters.antigravity.constants import (
                                DUMMY_THOUGHT_SIGNATURE,
                            )

                            dummy_sig = DUMMY_THOUGHT_SIGNATURE
                        except Exception:
                            dummy_sig = "skip_thought_signature_validator"

                        dummy_part: dict[str, Any] = {
                            "text": "Thinking...",
                            "thought": True,
                            "thoughtSignature": dummy_sig,
                        }
                        content = dict(content)
                        content["parts"] = [dummy_part, *parts]

            raw_contents.append(content)

        # Drop contents with empty parts (e.g. reasoning-only messages that have
        # no Gemini-compatible representation) and merge consecutive same-role
        # contents — Gemini requires strictly alternating user/model turns.
        contents = compact_gemini_contents(raw_contents)

        result: dict[str, Any] = {
            "contents": contents,
        }

        # Gemini Chat 模式 model 可能在 URL 路径中；这里仅在 internal.model 存在时回写
        if internal.model:
            result["model"] = internal.model

        if system_text:
            result["system_instruction"] = {"parts": [{"text": system_text}]}

        if generation_config:
            result["generation_config"] = generation_config

        if tools:
            result["tools"] = tools

        if tool_config:
            result["tool_config"] = tool_config

        return result

    # =========================
    # Responses
    # =========================

    def response_to_internal(self, response: dict[str, Any]) -> InternalResponse:
        rid = str(response.get("id") or "")
        model = str(response.get("modelVersion") or response.get("model") or "")

        candidates = response.get("candidates") or []
        candidate0 = candidates[0] if isinstance(candidates, list) and candidates else {}
        candidate0 = candidate0 if isinstance(candidate0, dict) else {}

        content = candidate0.get("content") if isinstance(candidate0, dict) else None
        content = content if isinstance(content, dict) else {}

        blocks, dropped = self._parts_to_blocks(content.get("parts"))

        finish_reason = candidate0.get("finishReason")
        stop_reason = None
        if finish_reason is not None:
            stop_reason = self._FINISH_REASON_TO_STOP.get(str(finish_reason), StopReason.UNKNOWN)

        usage_info = self._usage_metadata_to_internal(response.get("usageMetadata"))

        extra: dict[str, Any] = {}
        if finish_reason is not None:
            extra.setdefault("raw", {})["finishReason"] = finish_reason

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
        parts: list[dict[str, Any]] = []
        for b in internal.content:
            if isinstance(b, TextBlock):
                if b.text:
                    parts.append({"text": b.text})
                continue
            if isinstance(b, ToolUseBlock):
                fc: dict[str, Any] = {
                    "name": b.tool_name,
                    "args": b.tool_input or {},
                }
                # 保留 tool_id 用于 Claude/Antigravity 兼容
                if b.tool_id:
                    fc["id"] = b.tool_id
                parts.append({"functionCall": fc})
                continue
            if isinstance(b, ImageBlock):
                if b.data and b.media_type:
                    parts.append(
                        {
                            "inlineData": {
                                "mimeType": b.media_type,
                                "data": b.data,
                            }
                        }
                    )
                elif b.url:
                    parts.append({"text": f"[Image: {b.url}]"})
                continue
            # Unknown/ToolResult 默认丢弃

        finish_reason = None
        if internal.stop_reason is not None:
            finish_reason = STOP_REASON_MAPPINGS.get("GEMINI", {}).get(
                internal.stop_reason.value, "OTHER"
            )

        usage_metadata: dict[str, Any] = {}
        if internal.usage:
            usage_metadata = {
                "promptTokenCount": int(internal.usage.input_tokens),
                "candidatesTokenCount": int(internal.usage.output_tokens),
                "totalTokenCount": int(
                    internal.usage.total_tokens
                    or (internal.usage.input_tokens + internal.usage.output_tokens)
                ),
            }
            if internal.usage.cache_read_tokens:
                usage_metadata["cachedContentTokenCount"] = int(internal.usage.cache_read_tokens)

        candidate: dict[str, Any] = {
            "content": {"parts": parts, "role": "model"},
            "index": 0,
        }
        if finish_reason is not None:
            candidate["finishReason"] = finish_reason

        # 优先使用用户请求的原始模型名，回退到上游返回的模型名
        model_name = requested_model if requested_model else (internal.model or "gemini")

        out: dict[str, Any] = {
            "candidates": [candidate],
            "modelVersion": model_name,
        }

        if usage_metadata:
            out["usageMetadata"] = usage_metadata

        # id 不是 Gemini 标准字段，但内部可能携带；保守保留
        if internal.id:
            out["id"] = internal.id

        return out

    # =========================
    # Streaming
    # =========================

    def stream_chunk_to_internal(
        self, chunk: dict[str, Any], state: StreamState
    ) -> list[InternalStreamEvent]:
        ss = state.substate(self.FORMAT_ID)
        events: list[InternalStreamEvent] = []

        if not ss.get("message_started"):
            # 保留初始化时设置的 model（客户端请求的模型），仅在空时用上游值
            model = state.model or str(chunk.get("modelVersion") or "")
            if not state.model:
                state.model = model
            state.message_id = state.message_id or "gemini"
            ss["message_started"] = True
            ss.setdefault("text_block_started", False)
            ss.setdefault("text_block_stopped", False)
            ss.setdefault("thinking_block_started", False)
            ss.setdefault("thinking_block_stopped", False)
            # 根据首先出现的内容类型延迟分配块索引。
            # 当不存在思考内容时，golden tests 期望文本位于索引0。
            ss.setdefault("text_block_index", None)
            ss.setdefault("thinking_block_index", None)
            ss.setdefault("accumulated_text", "")
            ss.setdefault("accumulated_thinking", "")
            ss.setdefault("next_block_index", 0)
            events.append(MessageStartEvent(message_id=state.message_id, model=model))

        def _reserve_block_index(slot_key: str) -> int:
            idx = ss.get(slot_key)
            if isinstance(idx, int) and idx >= 0:
                return idx
            next_idx = int(ss.get("next_block_index") or 0)
            ss[slot_key] = next_idx
            ss["next_block_index"] = next_idx + 1
            return next_idx

        candidates = chunk.get("candidates") or []
        if not isinstance(candidates, list) or not candidates:
            return events

        candidate0 = candidates[0] if candidates else {}
        candidate0 = candidate0 if isinstance(candidate0, dict) else {}

        content = candidate0.get("content") if isinstance(candidate0, dict) else None
        content = content if isinstance(content, dict) else {}
        parts = content.get("parts") or []

        # parts -> events
        if isinstance(parts, list):
            for part in parts:
                if not isinstance(part, dict):
                    continue

                # text（兼容：delta 或累积）
                # 对齐 AM：检查 thought 标记，分流 thinking 和 regular text
                text = part.get("text")
                if isinstance(text, str) and text:
                    is_thought = part.get("thought") is True

                    if is_thought:
                        # Thinking content
                        prev = str(ss.get("accumulated_thinking") or "")
                        if text.startswith(prev):
                            delta = text[len(prev) :]
                            ss["accumulated_thinking"] = text
                        else:
                            delta = text
                            ss["accumulated_thinking"] = prev + delta

                        if delta:
                            if not ss.get("thinking_block_started"):
                                ss["thinking_block_started"] = True
                                events.append(
                                    ContentBlockStartEvent(
                                        block_index=_reserve_block_index("thinking_block_index"),
                                        block_type=ContentType.THINKING,
                                    )
                                )
                            events.append(
                                ContentDeltaEvent(
                                    block_index=_reserve_block_index("thinking_block_index"),
                                    text_delta=delta,
                                )
                            )
                    else:
                        # Regular text
                        prev = str(ss.get("accumulated_text") or "")
                        if text.startswith(prev):
                            delta = text[len(prev) :]
                            ss["accumulated_text"] = text
                        else:
                            delta = text
                            ss["accumulated_text"] = prev + delta

                        if delta:
                            # Transition: stop thinking block before text (Claude requires stop/start ordering).
                            if ss.get("thinking_block_started") and not ss.get(
                                "thinking_block_stopped"
                            ):
                                ss["thinking_block_stopped"] = True
                                events.append(
                                    ContentBlockStopEvent(
                                        block_index=_reserve_block_index("thinking_block_index")
                                    )
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
                                    text_delta=delta,
                                )
                            )

                    sig = part.get("thoughtSignature") or part.get("thought_signature")
                    if isinstance(sig, str) and sig and ss.get("thinking_block_started"):
                        # 仅在 thinking block 已开启时发射 signature delta
                        events.append(
                            ContentDeltaEvent(
                                block_index=_reserve_block_index("thinking_block_index"),
                                text_delta="",
                                extra={"thought_signature": sig},
                            )
                        )
                    continue

                # functionCall（stream response 常见 camelCase）
                func_call = part.get("functionCall")
                if func_call is None:
                    func_call = part.get("function_call")

                if isinstance(func_call, dict):
                    # 关闭前面的 thinking/text block（如果还开着）
                    if ss.get("thinking_block_started") and not ss.get("thinking_block_stopped"):
                        ss["thinking_block_stopped"] = True
                        events.append(ContentBlockStopEvent(block_index=_reserve_block_index("thinking_block_index")))
                    if ss.get("text_block_started") and not ss.get("text_block_stopped"):
                        ss["text_block_stopped"] = True
                        events.append(ContentBlockStopEvent(block_index=_reserve_block_index("text_block_index")))

                    name = str(func_call.get("name") or "")
                    args = func_call.get("args")
                    if not isinstance(args, dict):
                        args = {}

                    # 优先使用 Antigravity/Claude 注入的 id
                    fc_id = func_call.get("id")
                    tool_id = fc_id if isinstance(fc_id, str) and fc_id else None

                    block_index = int(ss.get("next_block_index") or 0)
                    ss["next_block_index"] = block_index + 1

                    events.append(
                        ContentBlockStartEvent(
                            block_index=block_index,
                            block_type=ContentType.TOOL_USE,
                            tool_id=tool_id,
                            tool_name=name or None,
                        )
                    )
                    if args:
                        events.append(
                            ToolCallDeltaEvent(
                                block_index=block_index,
                                tool_id=tool_id or "",
                                input_delta=json.dumps(args, ensure_ascii=False),
                            )
                        )
                    events.append(ContentBlockStopEvent(block_index=block_index))
                    continue

                # inlineData（图片生成等多模态输出）
                inline_data = part.get("inlineData")
                if inline_data is None:
                    inline_data = part.get("inline_data")

                if isinstance(inline_data, dict):
                    mime_type = str(
                        inline_data.get("mimeType") or inline_data.get("mime_type") or ""
                    ).strip()
                    data = str(inline_data.get("data") or "").strip()

                    # 确保 mime_type 和 data 都非空
                    if mime_type and data and len(data) > 10:  # base64 图片数据至少几十个字符
                        block_index = int(ss.get("next_block_index") or 0)
                        ss["next_block_index"] = block_index + 1

                        # 使用 ContentBlockStartEvent 传递图片数据
                        # 图片数据存储在 extra 中，供 target normalizer 处理
                        events.append(
                            ContentBlockStartEvent(
                                block_index=block_index,
                                block_type=ContentType.IMAGE,
                                extra={
                                    "image_data": data,
                                    "image_media_type": mime_type,
                                },
                            )
                        )
                        events.append(ContentBlockStopEvent(block_index=block_index))
                    continue

        finish_reason = candidate0.get("finishReason")
        if finish_reason is not None:
            stop_reason = self._FINISH_REASON_TO_STOP.get(str(finish_reason), StopReason.UNKNOWN)
            usage_info = self._usage_metadata_to_internal(chunk.get("usageMetadata"))
            # 先补齐 content_block_stop（所有已开启的 block），再发送 MessageStop
            if ss.get("thinking_block_started") and not ss.get("thinking_block_stopped"):
                ss["thinking_block_stopped"] = True
                events.append(ContentBlockStopEvent(block_index=_reserve_block_index("thinking_block_index")))
            if ss.get("text_block_started") and not ss.get("text_block_stopped"):
                ss["text_block_stopped"] = True
                events.append(ContentBlockStopEvent(block_index=_reserve_block_index("text_block_index")))
            events.append(MessageStopEvent(stop_reason=stop_reason, usage=usage_info))

        if "error" in chunk:
            events.append(ErrorEvent(error=self.error_to_internal(chunk)))

        return events

    def stream_event_from_internal(
        self,
        event: InternalStreamEvent,
        state: StreamState,
    ) -> list[dict[str, Any]]:
        ss = state.substate(self.FORMAT_ID)
        out: list[dict[str, Any]] = []

        def base_chunk(parts: list[dict[str, Any]]) -> dict[str, Any]:
            return {
                "candidates": [
                    {
                        "content": {"parts": parts, "role": "model"},
                        "index": 0,
                    }
                ],
                "modelVersion": state.model or "",
            }

        if isinstance(event, MessageStartEvent):
            state.message_id = event.message_id or state.message_id
            # 保留初始化时设置的 model（客户端请求的模型），仅在空时用事件值
            if not state.model:
                state.model = event.model or ""
            ss.setdefault("tool_blocks", {})
            return out

        if isinstance(event, ContentBlockStartEvent):
            # 记录 block_index → block_type 映射
            ss[f"block_type_{event.block_index}"] = event.block_type.value
            if event.block_type == ContentType.THINKING:
                ss["thinking_output_started"] = True
                return out

        if isinstance(event, ContentDeltaEvent):
            if event.text_delta:
                # 检查 block_index → block_type 映射，区分 thinking 和 text
                block_type = ss.get(f"block_type_{event.block_index}")
                if block_type == ContentType.THINKING.value:
                    part: dict[str, Any] = {"text": event.text_delta, "thought": True}
                    out.append(base_chunk([part]))
                else:
                    out.append(base_chunk([{"text": event.text_delta}]))
            # thoughtSignature（来自 Gemini upstream 或 其他格式转换）
            sig = (event.extra or {}).get("thought_signature")
            if isinstance(sig, str) and sig:
                out.append(base_chunk([{"text": "", "thought": True, "thoughtSignature": sig}]))
            return out

        if isinstance(event, ContentBlockStartEvent) and event.block_type == ContentType.TOOL_USE:
            tool_blocks = ss.get("tool_blocks")
            if not isinstance(tool_blocks, dict):
                tool_blocks = {}
                ss["tool_blocks"] = tool_blocks

            tool_blocks[int(event.block_index)] = {
                "name": event.tool_name or "",
                "json": "",
                "id": event.tool_id or "",  # 保留 tool_id 用于 Claude/Antigravity 兼容
            }
            return out

        # 图片内容块（来自其他格式的图像生成输出）
        if isinstance(event, ContentBlockStartEvent) and event.block_type == ContentType.IMAGE:
            image_data = event.extra.get("image_data")
            image_media_type = event.extra.get("image_media_type")
            if image_data and image_media_type:
                out.append(
                    base_chunk(
                        [
                            {
                                "inlineData": {
                                    "mimeType": image_media_type,
                                    "data": image_data,
                                }
                            }
                        ]
                    )
                )
            return out

        if isinstance(event, ToolCallDeltaEvent):
            tool_blocks = ss.get("tool_blocks")
            if isinstance(tool_blocks, dict):
                entry = tool_blocks.get(int(event.block_index))
                if isinstance(entry, dict):
                    entry["json"] = str(entry.get("json") or "") + (event.input_delta or "")
            return out

        if isinstance(event, ContentBlockStopEvent):
            tool_blocks = ss.get("tool_blocks")
            if not isinstance(tool_blocks, dict):
                return out

            entry = tool_blocks.get(int(event.block_index))
            if not isinstance(entry, dict):
                return out

            name = str(entry.get("name") or "")
            raw_json = str(entry.get("json") or "")
            tool_id = str(entry.get("id") or "")
            args: dict[str, Any] = {}
            if raw_json:
                try:
                    parsed = json.loads(raw_json)
                    if isinstance(parsed, dict):
                        args = parsed
                except json.JSONDecodeError:
                    args = {}

            fc: dict[str, Any] = {"name": name, "args": args}
            # 保留 tool_id 用于 Claude/Antigravity 兼容
            if tool_id:
                fc["id"] = tool_id
            out.append(base_chunk([{"functionCall": fc}]))
            return out

        if isinstance(event, MessageStopEvent):
            finish_reason = None
            if event.stop_reason is not None:
                finish_reason = STOP_REASON_MAPPINGS.get("GEMINI", {}).get(
                    event.stop_reason.value, "OTHER"
                )

            chunk: dict[str, Any] = base_chunk([])
            if finish_reason is not None:
                chunk["candidates"][0]["finishReason"] = finish_reason

            if event.usage:
                chunk["usageMetadata"] = {
                    "promptTokenCount": int(event.usage.input_tokens),
                    "candidatesTokenCount": int(event.usage.output_tokens),
                    "totalTokenCount": int(
                        event.usage.total_tokens
                        or (event.usage.input_tokens + event.usage.output_tokens)
                    ),
                }
                if event.usage.cache_read_tokens:
                    chunk["usageMetadata"]["cachedContentTokenCount"] = int(
                        event.usage.cache_read_tokens
                    )

            out.append(chunk)
            return out

        if isinstance(event, ErrorEvent):
            out.append(self.error_from_internal(event.error))
            return out

        return out

    # =========================
    # Error conversion
    # =========================

    def is_error_response(self, response: dict[str, Any]) -> bool:
        return isinstance(response, dict) and "error" in response

    def error_to_internal(self, error_response: dict[str, Any]) -> InternalError:
        err = error_response.get("error") if isinstance(error_response, dict) else None
        err = err if isinstance(err, dict) else {}

        raw_status = err.get("status")
        mapped = ERROR_TYPE_MAPPINGS.get("GEMINI", {}).get(str(raw_status), ErrorType.UNKNOWN.value)
        internal_type = self._error_type_from_value(mapped)
        retryable = internal_type.value in RETRYABLE_ERROR_TYPES

        code_value = err.get("code")
        code_str = None
        if code_value is not None:
            code_str = str(code_value)

        return InternalError(
            type=internal_type,
            message=str(err.get("message") or ""),
            code=code_str,
            param=None,
            retryable=retryable,
            extra={"gemini": {"error": err}, "raw": {"status": raw_status}},
        )

    def error_from_internal(self, internal: InternalError) -> dict[str, Any]:
        status = self._ERROR_TYPE_TO_GEMINI_STATUS.get(internal.type, "INTERNAL")
        payload: dict[str, Any] = {
            "code": 400 if internal.type == ErrorType.INVALID_REQUEST else 500,
            "message": internal.message,
            "status": status,
        }
        return {"error": payload}

    # =========================
    # Video conversion
    # =========================

    def video_request_to_internal(self, request: dict[str, Any]) -> InternalVideoRequest:
        instances = request.get("instances")
        if not instances or not isinstance(instances, list) or len(instances) == 0:
            raise ValueError("Video request requires at least one instance")
        instance = instances[0] if isinstance(instances[0], dict) else {}
        params = request.get("parameters") or {}

        # 解析 image（用于 image-to-video 或第一帧）
        # 官方格式: {"image": {"inlineData": {"mimeType": "image/png", "data": "base64..."}}}
        image = instance.get("image") if isinstance(instance, dict) else None
        image_ref = None
        if isinstance(image, dict):
            inline_data = image.get("inlineData", {})
            if isinstance(inline_data, dict):
                image_ref = inline_data.get("data")
            # 兼容旧格式
            if not image_ref:
                image_ref = image.get("bytesBase64Encoded")

        prompt = instance.get("prompt") if isinstance(instance, dict) else None
        prompt_str = str(prompt).strip() if prompt else ""
        if not prompt_str:
            raise ValueError("Video prompt is required")

        duration_raw = params.get("durationSeconds")
        sample_count_raw = params.get("sampleCount") or params.get("numberOfVideos")

        try:
            duration_seconds = int(duration_raw) if duration_raw else 8
        except (ValueError, TypeError):
            duration_seconds = 8

        # 安全解析 sampleCount
        try:
            sample_count = int(sample_count_raw) if sample_count_raw else 1
        except (ValueError, TypeError):
            sample_count = 1

        # 构建 extra 字段，保留所有 Veo 特有的参数
        extra: dict[str, Any] = {
            "personGeneration": params.get("personGeneration"),
            "sampleCount": sample_count,
        }

        # negativePrompt - 负面提示词
        if params.get("negativePrompt"):
            extra["negativePrompt"] = params["negativePrompt"]

        # lastFrame - 最后一帧（用于插值）
        last_frame = params.get("lastFrame")
        if isinstance(last_frame, dict):
            extra["lastFrame"] = last_frame

        # referenceImages - 参考图像（最多3张，仅 Veo 3.1）
        ref_images = params.get("referenceImages")
        if isinstance(ref_images, list) and ref_images:
            extra["referenceImages"] = ref_images

        # video - 视频扩展输入（用于视频续写）
        video_input = instance.get("video") if isinstance(instance, dict) else None
        if isinstance(video_input, dict):
            extra["video"] = video_input

        # seed - 种子值（Veo 3）
        if params.get("seed") is not None:
            extra["seed"] = params["seed"]

        return InternalVideoRequest(
            prompt=prompt_str,
            model=str(request.get("model") or "veo-3.1-generate-preview"),
            duration_seconds=duration_seconds,
            aspect_ratio=str(params.get("aspectRatio") or "16:9"),
            resolution=str(params.get("resolution") or "720p"),
            reference_image_url=image_ref,
            extra=extra,
        )

    def video_request_from_internal(self, internal: InternalVideoRequest) -> dict[str, Any]:
        instance: dict[str, Any] = {"prompt": internal.prompt}
        if internal.reference_image_url:
            instance["image"] = {"bytesBase64Encoded": internal.reference_image_url}

        parameters: dict[str, Any] = {
            "aspectRatio": internal.aspect_ratio,
            "resolution": internal.resolution,
            "durationSeconds": internal.duration_seconds,
        }
        for key in ["personGeneration", "sampleCount", "negativePrompt", "seed"]:
            if key in internal.extra:
                parameters[key] = internal.extra[key]

        # lastFrame 和 referenceImages 需要特殊处理
        if internal.extra.get("lastFrame"):
            parameters["lastFrame"] = internal.extra["lastFrame"]
        if internal.extra.get("referenceImages"):
            parameters["referenceImages"] = internal.extra["referenceImages"]

        # video 输入（视频续写）
        if internal.extra.get("video"):
            instance["video"] = internal.extra["video"]

        result: dict[str, Any] = {
            "instances": [instance],
            "parameters": parameters,
        }

        # 模型信息（用于 URL 构建）
        if internal.model:
            result["model"] = internal.model

        return result

    def video_task_to_internal(self, response: dict[str, Any]) -> InternalVideoTask:
        operation_name = str(response.get("name") or "")
        done = bool(response.get("done"))

        if done:
            video_response = response.get("response", {}).get("generateVideoResponse", {})
            samples = video_response.get("generatedSamples", [])
            video_urls = [
                s.get("video", {}).get("uri")
                for s in samples
                if isinstance(s, dict) and s.get("video", {}).get("uri")
            ]
            return InternalVideoTask(
                id=operation_name.replace("operations/", ""),
                external_id=operation_name,
                status=VideoStatus.COMPLETED,
                progress_percent=100,
                video_url=video_urls[0] if video_urls else None,
                video_urls=video_urls,
                extra={"raw_response": video_response},
            )

        metadata = response.get("metadata", {})
        return InternalVideoTask(
            id=operation_name.replace("operations/", ""),
            external_id=operation_name,
            status=VideoStatus.PROCESSING,
            progress_percent=50,
            extra={"metadata": metadata},
        )

    def video_task_from_internal(
        self, internal: InternalVideoTask, *, base_url: str | None = None
    ) -> dict[str, Any]:
        # 从 external_id 中提取 model 名称，用于构建 operation name
        # external_id 格式: models/{model}/operations/{gemini_id}
        model_name = "unknown"
        if internal.external_id:
            parts = internal.external_id.split("/")
            if len(parts) >= 2 and parts[0] == "models":
                model_name = parts[1]

        # 使用我们的内部 task_id 构建 operation name，不暴露 Gemini 的 operation ID
        # 格式: models/{model}/operations/{our_task_id}
        operation_name = f"models/{model_name}/operations/{internal.id}"

        if internal.status == VideoStatus.COMPLETED:
            # 使用我们的内部 task_id 构建下载 URL，不暴露真实的 Gemini file_id
            # 使用 aev_ 前缀标识这是视频任务的下载链接
            # 格式：/v1beta/files/aev_{task_id}:download?alt=media
            download_path = f"/v1beta/files/aev_{internal.id}:download?alt=media"
            # 如果提供了 base_url，返回完整 URL；否则返回相对路径
            proxy_download_url = f"{base_url}{download_path}" if base_url else download_path
            return {
                "name": operation_name,
                "done": True,
                "response": {
                    "generateVideoResponse": {
                        "generatedSamples": [
                            {"video": {"uri": proxy_download_url, "mimeType": "video/mp4"}}
                        ]
                    }
                },
            }

        if internal.status == VideoStatus.FAILED:
            return {
                "name": operation_name,
                "done": True,
                "error": {
                    "code": internal.error_code or "UNKNOWN",
                    "message": internal.error_message or "Video generation failed",
                },
            }

        return {
            "name": operation_name,
            "done": False,
            "metadata": internal.extra.get("metadata", {}),
        }

    def video_poll_to_internal(self, response: dict[str, Any]) -> InternalVideoPollResult:
        done = bool(response.get("done"))
        if done:
            error = response.get("error")
            if error:
                return InternalVideoPollResult(
                    status=VideoStatus.FAILED,
                    error_code=str(error.get("code", "unknown")),
                    error_message=error.get("message", "Unknown error"),
                    raw_response=response,
                )

            video_response = response.get("response", {}).get("generateVideoResponse", {})
            samples = video_response.get("generatedSamples", [])
            video_urls = [
                s.get("video", {}).get("uri")
                for s in samples
                if isinstance(s, dict) and s.get("video", {}).get("uri")
            ]
            video_url = video_urls[0] if video_urls else None
            # 提取实际视频时长
            video_duration = self._extract_gemini_video_duration(response, samples)
            return InternalVideoPollResult(
                status=VideoStatus.COMPLETED,
                progress_percent=100,
                video_url=video_url,
                video_urls=video_urls,
                raw_response=response,
                video_duration_seconds=video_duration,
            )

        return InternalVideoPollResult(
            status=VideoStatus.PROCESSING,
            progress_percent=50,
            raw_response=response,
        )

    def _extract_gemini_video_duration(
        self, response: dict[str, Any], samples: list[dict[str, Any]]
    ) -> float | None:
        """从 Gemini 响应中提取实际视频时长"""
        # 尝试从 samples 中获取时长
        for sample in samples:
            if not isinstance(sample, dict):
                continue
            video = sample.get("video", {})
            if isinstance(video, dict):
                # 尝试多种字段名
                for field in ["durationSeconds", "duration_seconds", "duration"]:
                    val = video.get(field)
                    if val is not None:
                        try:
                            # duration 可能是 "5s" 格式
                            if isinstance(val, str) and val.endswith("s"):
                                return float(val[:-1])
                            return float(val)
                        except (ValueError, TypeError):
                            continue
        # 尝试从 response.metadata 获取
        metadata = response.get("metadata", {})
        if isinstance(metadata, dict):
            for field in ["durationSeconds", "duration_seconds", "duration"]:
                val = metadata.get(field)
                if val is not None:
                    try:
                        if isinstance(val, str) and val.endswith("s"):
                            return float(val[:-1])
                        return float(val)
                    except (ValueError, TypeError):
                        continue
        return None

    # =========================
    # Helpers
    # =========================

    def _content_to_internal_message(
        self, content: dict[str, Any]
    ) -> tuple[InternalMessage | None, dict[str, int]]:
        dropped: dict[str, int] = {}

        role_raw = str(content.get("role") or "user")
        if role_raw == "model":
            role = Role.ASSISTANT
        elif role_raw == "user":
            role = Role.USER
        else:
            role = Role.UNKNOWN

        blocks, bd = self._parts_to_blocks(content.get("parts"))
        self._merge_dropped(dropped, bd)

        return (
            InternalMessage(
                role=role,
                content=blocks,
                extra=self._extract_extra(content, {"role", "parts"}),
            ),
            dropped,
        )

    def _parts_to_blocks(self, parts: Any) -> tuple[list[ContentBlock], dict[str, int]]:
        dropped: dict[str, int] = {}
        if parts is None:
            return [], dropped
        if not isinstance(parts, list):
            dropped["gemini_parts_non_list"] = dropped.get("gemini_parts_non_list", 0) + 1
            return [], dropped

        blocks: list[ContentBlock] = []
        for part in parts:
            if not isinstance(part, dict):
                dropped["gemini_part_non_dict"] = dropped.get("gemini_part_non_dict", 0) + 1
                continue

            if "text" in part:
                text = part.get("text")
                if isinstance(text, str) and text:
                    is_thought = part.get("thought") is True
                    if is_thought:
                        sig = part.get("thoughtSignature") or part.get("thought_signature")
                        blocks.append(
                            ThinkingBlock(
                                thinking=text,
                                signature=sig if isinstance(sig, str) else None,
                                extra=self._extract_extra(
                                    part,
                                    {"text", "thought", "thoughtSignature", "thought_signature"},
                                ),
                            )
                        )
                    else:
                        blocks.append(
                            TextBlock(text=text, extra=self._extract_extra(part, {"text"}))
                        )
                continue

            inline = part.get("inline_data")
            if inline is None:
                inline = part.get("inlineData")
            if isinstance(inline, dict):
                mime_type = (
                    inline.get("mime_type") if "mime_type" in inline else inline.get("mimeType")
                )
                data = inline.get("data")
                if isinstance(mime_type, str) and mime_type and isinstance(data, str) and data:
                    blocks.append(ImageBlock(data=data, media_type=mime_type))
                else:
                    dropped["gemini_inline_data_invalid"] = (
                        dropped.get("gemini_inline_data_invalid", 0) + 1
                    )
                    blocks.append(UnknownBlock(raw_type="inline_data", payload=part))
                continue

            func_call = part.get("function_call")
            if func_call is None:
                func_call = part.get("functionCall")
            if isinstance(func_call, dict):
                name = str(func_call.get("name") or "")
                args = func_call.get("args")
                if not isinstance(args, dict):
                    args = {}
                # 优先使用 Antigravity/Claude 注入的 id，回退到生成的 id
                fc_id = func_call.get("id")
                if isinstance(fc_id, str) and fc_id:
                    tool_id = fc_id
                else:
                    tool_id = f"toolu_{name}" if name else "toolu_0"
                blocks.append(
                    ToolUseBlock(
                        tool_id=tool_id,
                        tool_name=name,
                        tool_input=args,
                        extra={"gemini": part},
                    )
                )
                continue

            func_resp = part.get("function_response")
            if func_resp is None:
                func_resp = part.get("functionResponse")
            if isinstance(func_resp, dict):
                name = str(func_resp.get("name") or "")
                response = func_resp.get("response")
                output: Any = None
                content_text: str | None = None

                # 兼容历史：response 常见结构为 {"result": ...}
                if isinstance(response, dict) and "result" in response:
                    output = response.get("result")
                    if isinstance(output, str):
                        content_text = output
                        output = None
                else:
                    output = response

                # 优先使用 Antigravity/Claude 注入的 id，回退到 name
                fr_id = func_resp.get("id")
                if isinstance(fr_id, str) and fr_id:
                    tool_use_id = fr_id
                else:
                    tool_use_id = name

                blocks.append(
                    ToolResultBlock(
                        tool_use_id=tool_use_id,
                        tool_name=name or None,  # 保留工具名称用于输出
                        output=output,
                        content_text=content_text,
                        is_error=False,
                        extra={"gemini": part},
                    )
                )
                continue

            # 其它：Unknown
            raw_type = next(iter(part.keys()), "unknown")
            dropped_key = f"gemini_part:{raw_type}"
            dropped[dropped_key] = dropped.get(dropped_key, 0) + 1
            blocks.append(UnknownBlock(raw_type=str(raw_type), payload=part))

        return blocks, dropped

    def _internal_message_to_content(
        self,
        msg: InternalMessage,
        *,
        target_variant: str | None = None,
        model: str | None = None,
    ) -> dict[str, Any]:
        role = "model" if msg.role == Role.ASSISTANT else "user"

        parts: list[dict[str, Any]] = []
        for b in msg.content:
            if isinstance(b, UnknownBlock):
                if str(target_variant or "").strip().lower() == "antigravity":
                    part = self._unknown_block_to_antigravity_part(b, model=str(model or ""))
                    if part is not None:
                        parts.append(part)
                continue

            if isinstance(b, ThinkingBlock):
                if b.thinking:
                    signature: str | None = b.signature or None

                    if signature is None and target_variant == "antigravity":
                        model_str = str(model or "")
                        try:
                            from src.services.provider.adapters.antigravity.constants import (
                                DUMMY_THOUGHT_SIGNATURE,
                            )
                            from src.services.provider.adapters.antigravity.signature_cache import (
                                signature_cache,
                            )

                            cached_or_dummy = signature_cache.get_or_dummy(model_str, b.thinking)

                            # Prefer cached real signature > dummy signature.
                            if (
                                isinstance(cached_or_dummy, str)
                                and cached_or_dummy
                                and cached_or_dummy != DUMMY_THOUGHT_SIGNATURE
                            ):
                                signature = cached_or_dummy
                            elif isinstance(cached_or_dummy, str) and cached_or_dummy:
                                signature = cached_or_dummy
                        except Exception:
                            # Best-effort fallback: Gemini models can accept a dummy signature.
                            if model_str.startswith("gemini-"):
                                signature = "skip_thought_signature_validator"

                    # For Antigravity, missing signature is likely to fail upstream validation.
                    if target_variant == "antigravity" and not signature:
                        continue

                    thought_part: dict[str, Any] = {"text": b.thinking, "thought": True}
                    if signature:
                        thought_part["thoughtSignature"] = signature
                    parts.append(thought_part)
                continue

            if isinstance(b, TextBlock):
                if b.text:
                    parts.append({"text": b.text})
                continue

            if isinstance(b, ImageBlock):
                if b.data and b.media_type:
                    parts.append({"inline_data": {"mime_type": b.media_type, "data": b.data}})
                elif b.url:
                    parts.append({"text": f"[Image: {b.url}]"})
                continue

            if isinstance(b, ToolUseBlock) and role == "model":
                fc: dict[str, Any] = {"name": b.tool_name, "args": b.tool_input or {}}
                # 保留 tool_id 用于 Claude/Antigravity 兼容
                if b.tool_id:
                    fc["id"] = b.tool_id
                parts.append({"function_call": fc})
                continue

            if isinstance(b, ToolResultBlock) and role == "user":
                # name 使用 tool_name（工具名称），id 使用 tool_use_id（call id）
                # 如果没有 tool_name，回退到 tool_use_id 以保持向后兼容
                value: Any
                if b.content_text is not None:
                    value = b.content_text
                elif b.output is None:
                    value = ""
                else:
                    value = b.output

                # 优先使用 tool_name 作为 name，回退到 tool_use_id
                name = b.tool_name if b.tool_name else b.tool_use_id
                fr: dict[str, Any] = {
                    "name": name,
                    "response": {"result": value},
                }
                # 保留 tool_use_id 作为 id 用于 Claude/Antigravity 兼容
                if b.tool_use_id:
                    fr["id"] = b.tool_use_id
                parts.append({"function_response": fr})
                continue

        return {"role": role, "parts": parts}

    def _is_antigravity_thinking_enabled(self, internal: InternalRequest) -> bool:
        """Best-effort detection of Claude-style `thinking` flag for Antigravity conversions."""
        try:
            extra = internal.extra if isinstance(internal.extra, dict) else {}
            claude_extra = extra.get("claude")
            if not isinstance(claude_extra, dict):
                return False

            thinking = claude_extra.get("thinking")
            if thinking is True:
                return True

            if isinstance(thinking, dict):
                ttype = thinking.get("type")
                if isinstance(ttype, str) and ttype.strip().lower() == "enabled":
                    return True
                enabled = thinking.get("enabled")
                if enabled is True:
                    return True

            return False
        except Exception:
            return False

    def _unknown_block_to_antigravity_part(
        self,
        block: UnknownBlock,
        *,
        model: str,
    ) -> dict[str, Any] | None:
        """Translate Claude thinking blocks into Gemini thought parts for Antigravity.

        The internal representation stores Claude `thinking`/`redacted_thinking` as UnknownBlock.
        Antigravity expects them as Gemini parts with `thought=true` and `thoughtSignature`.
        """
        raw_type = str(getattr(block, "raw_type", "") or "").strip().lower()
        if raw_type not in {"thinking", "redacted_thinking"}:
            return None

        payload = block.payload if isinstance(block.payload, dict) else {}

        # Claude: thinking -> {type:"thinking", thinking:"...", signature?: "..."}
        # Claude: redacted_thinking -> {type:"redacted_thinking", data:"..."}
        if raw_type == "thinking":
            text_val = payload.get("thinking")
        else:
            text_val = payload.get("data")
        if text_val is None:
            text_val = payload.get("text")

        if not isinstance(text_val, str) or not text_val:
            return None

        payload_sig = (
            payload.get("signature")
            or payload.get("thoughtSignature")
            or payload.get("thought_signature")
        )
        if not isinstance(payload_sig, str) or not payload_sig:
            payload_sig = None

        signature: str | None = None
        try:
            from src.services.provider.adapters.antigravity.constants import DUMMY_THOUGHT_SIGNATURE
            from src.services.provider.adapters.antigravity.signature_cache import signature_cache

            cached_or_dummy = signature_cache.get_or_dummy(model, text_val)

            # Prefer cached real signature > client-provided signature > dummy signature.
            if (
                isinstance(cached_or_dummy, str)
                and cached_or_dummy
                and cached_or_dummy != DUMMY_THOUGHT_SIGNATURE
            ):
                signature = cached_or_dummy
            elif payload_sig:
                signature = payload_sig
            elif isinstance(cached_or_dummy, str) and cached_or_dummy:
                signature = cached_or_dummy
        except Exception:
            signature = payload_sig

        # For non-gemini models, missing signature is likely to fail upstream validation.
        if not signature:
            return None

        return {"text": text_val, "thought": True, "thoughtSignature": signature}

    def _collapse_system_instruction(
        self, system_instruction: Any
    ) -> tuple[str | None, dict[str, int]]:
        dropped: dict[str, int] = {}
        if system_instruction is None:
            return None, dropped

        # 支持 {"parts": [{"text": ...}, ...]}
        if isinstance(system_instruction, dict):
            parts = system_instruction.get("parts")
            if isinstance(parts, list):
                texts: list[str] = []
                for part in parts:
                    if isinstance(part, dict) and "text" in part and part.get("text"):
                        texts.append(str(part.get("text")))
                joined = "".join(texts)
                return (joined or None), dropped

        dropped["gemini_system_instruction_unsupported"] = (
            dropped.get("gemini_system_instruction_unsupported", 0) + 1
        )
        return None, dropped

    def _get_generation_config(self, request: dict[str, Any]) -> dict[str, Any]:
        # 兼容 snake_case 与 camelCase
        gc = (
            request.get("generation_config")
            if "generation_config" in request
            else request.get("generationConfig")
        )
        if not isinstance(gc, dict):
            return {}

        # 统一内部使用 snake_case key
        def pick(*keys: str) -> Any:
            for k in keys:
                if k in gc:
                    return gc.get(k)
            return None

        normalized: dict[str, Any] = {}
        normalized["max_output_tokens"] = pick("max_output_tokens", "maxOutputTokens")
        normalized["temperature"] = pick("temperature")
        normalized["top_p"] = pick("top_p", "topP")
        normalized["top_k"] = pick("top_k", "topK")
        normalized["stop_sequences"] = pick("stop_sequences", "stopSequences")

        # 保留 responseModalities（图像生成等多模态输出必需）
        response_modalities = pick("response_modalities", "responseModalities")
        if response_modalities:
            normalized["response_modalities"] = response_modalities

        # 保留 thinkingConfig（思考模式配置）
        thinking_config = pick("thinking_config", "thinkingConfig")
        if thinking_config:
            normalized["thinking_config"] = thinking_config

        return {k: v for k, v in normalized.items() if v is not None}

    def _gemini_tools_to_internal(self, tools: Any) -> list[ToolDefinition] | None:
        if not tools or not isinstance(tools, list):
            return None

        out: list[ToolDefinition] = []
        for tool in tools:
            if not isinstance(tool, dict):
                continue

            decls = tool.get("function_declarations")
            if decls is None:
                decls = tool.get("functionDeclarations")

            if not isinstance(decls, list):
                continue

            for decl in decls:
                if not isinstance(decl, dict):
                    continue
                name = str(decl.get("name") or "")
                if not name:
                    continue
                out.append(
                    ToolDefinition(
                        name=name,
                        description=decl.get("description"),
                        parameters=(
                            decl.get("parameters")
                            if isinstance(decl.get("parameters"), dict)
                            else None
                        ),
                        extra={
                            "gemini_function_declaration": self._extract_extra(
                                decl, {"name", "description", "parameters"}
                            )
                        },
                    )
                )

        return out or None

    def _gemini_tool_config_to_tool_choice(self, tool_config: Any) -> ToolChoice | None:
        if tool_config is None:
            return None
        if not isinstance(tool_config, dict):
            return ToolChoice(type=ToolChoiceType.AUTO, extra={"raw": tool_config})

        cfg = tool_config.get("function_calling_config")
        if cfg is None:
            cfg = tool_config.get("functionCallingConfig")

        if not isinstance(cfg, dict):
            return ToolChoice(type=ToolChoiceType.AUTO, extra={"gemini": tool_config})

        mode = str(cfg.get("mode") or "AUTO").upper()
        allowed = cfg.get("allowed_function_names")
        if allowed is None:
            allowed = cfg.get("allowedFunctionNames")

        if mode == "NONE":
            return ToolChoice(type=ToolChoiceType.NONE, extra={"gemini": tool_config})
        if mode in ("ANY", "REQUIRED"):
            return ToolChoice(type=ToolChoiceType.REQUIRED, extra={"gemini": tool_config})
        if isinstance(allowed, list) and len(allowed) == 1:
            return ToolChoice(
                type=ToolChoiceType.TOOL,
                tool_name=str(allowed[0] or ""),
                extra={"gemini": tool_config},
            )

        return ToolChoice(type=ToolChoiceType.AUTO, extra={"gemini": tool_config})

    def _tool_choice_to_gemini_tool_config(self, tool_choice: ToolChoice) -> dict[str, Any]:
        mode = "AUTO"
        cfg: dict[str, Any] = {}

        if tool_choice.type == ToolChoiceType.NONE:
            mode = "NONE"
        elif tool_choice.type == ToolChoiceType.REQUIRED:
            mode = "ANY"
        elif tool_choice.type == ToolChoiceType.TOOL:
            mode = "ANY"
            cfg["allowed_function_names"] = [tool_choice.tool_name or ""]

        cfg["mode"] = mode
        return {"function_calling_config": cfg}

    def _usage_metadata_to_internal(self, usage_metadata: Any) -> UsageInfo | None:
        if not isinstance(usage_metadata, dict):
            return None

        mapping = USAGE_FIELD_MAPPINGS.get("GEMINI", {})
        fields: dict[str, int] = {}
        extra = self._extract_extra(usage_metadata, set(mapping.keys()))

        # promptTokenCount/candidatesTokenCount/totalTokenCount/cachedContentTokenCount
        for provider_key, internal_key in mapping.items():
            if provider_key in usage_metadata and usage_metadata.get(provider_key) is not None:
                try:
                    fields[internal_key] = int(usage_metadata.get(provider_key) or 0)
                except (TypeError, ValueError):
                    continue

        # thoughtsTokenCount（如果存在，按 handler 的口径并入 output_tokens）
        thoughts = usage_metadata.get("thoughtsTokenCount")
        if thoughts is not None:
            try:
                fields["output_tokens"] = int(fields.get("output_tokens", 0) + int(thoughts or 0))
            except (TypeError, ValueError):
                pass

        if "total_tokens" not in fields:
            fields["total_tokens"] = int(
                fields.get("input_tokens", 0) + fields.get("output_tokens", 0)
            )

        return UsageInfo(
            input_tokens=int(fields.get("input_tokens", 0)),
            output_tokens=int(fields.get("output_tokens", 0)),
            total_tokens=int(fields.get("total_tokens", 0)),
            cache_read_tokens=int(fields.get("cache_read_tokens", 0)),
            cache_write_tokens=0,
            extra={"gemini": extra} if extra else {},
        )

    def _join_instructions(self, instructions: list[InstructionSegment]) -> str | None:
        parts = [seg.text for seg in instructions if seg.text]
        joined = "\n\n".join(parts)
        return joined or None

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


__all__ = ["GeminiNormalizer"]
