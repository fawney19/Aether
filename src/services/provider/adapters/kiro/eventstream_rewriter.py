"""AWS Event Stream -> Claude SSE rewriter for Kiro.

Kiro streaming responses are returned as `application/vnd.amazon.eventstream`
(binary framed). This module decodes frames and emits Claude-style streaming
SSE events (as UTF-8 bytes).

The output format uses ``event: {type}\\ndata: {...}\\n\\n`` for typed events and
plain ``data: {...}\\n\\n`` for untyped events, matching how Aether parses Claude
streams.
"""

from __future__ import annotations

import json
import uuid
from collections.abc import AsyncGenerator
from dataclasses import dataclass, field
from typing import Any

from src.core.logger import logger
from src.services.provider.adapters.kiro.constants import CONTEXT_WINDOW_TOKENS
from src.services.provider.adapters.kiro.error_enhancer import build_kiro_network_diagnostic
from src.services.provider.adapters.kiro.parser.decoder import EventStreamDecoder

# Safety limit for thinking_buffer to prevent memory exhaustion from
# pathological upstream responses that never close the thinking tag.
_MAX_THINKING_BUFFER = 1024 * 1024  # 1 MiB

_QUOTE_CHARS: frozenset[str] = frozenset("`\"'\\#!@$%^&*()-_=+[]{};:<>,.?/")


def _is_quote_char(buffer: str, pos: int) -> bool:
    if pos < 0 or pos >= len(buffer):
        return False
    return buffer[pos] in _QUOTE_CHARS


def _find_real_thinking_start_tag(buffer: str) -> int | None:
    tag = "<thinking>"
    search = 0
    while True:
        pos = buffer.find(tag, search)
        if pos < 0:
            return None
        has_before = pos > 0 and _is_quote_char(buffer, pos - 1)
        after_pos = pos + len(tag)
        has_after = _is_quote_char(buffer, after_pos)
        if not has_before and not has_after:
            return pos
        search = pos + 1


def _find_real_thinking_end_tag(buffer: str) -> int | None:
    tag = "</thinking>"
    search = 0
    while True:
        pos = buffer.find(tag, search)
        if pos < 0:
            return None

        has_before = pos > 0 and _is_quote_char(buffer, pos - 1)
        after_pos = pos + len(tag)
        has_after = _is_quote_char(buffer, after_pos)
        if has_before or has_after:
            search = pos + 1
            continue

        after = buffer[after_pos:]
        if len(after) < 2:
            return None
        if after.startswith("\n\n"):
            return pos

        search = pos + 1


def _find_real_thinking_end_tag_at_buffer_end(buffer: str) -> int | None:
    tag = "</thinking>"
    search = 0
    while True:
        pos = buffer.find(tag, search)
        if pos < 0:
            return None

        has_before = pos > 0 and _is_quote_char(buffer, pos - 1)
        after_pos = pos + len(tag)
        has_after = _is_quote_char(buffer, after_pos)
        if has_before or has_after:
            search = pos + 1
            continue

        if buffer[after_pos:].strip() == "":
            return pos

        search = pos + 1


def _estimate_tokens(text: str) -> int:
    if not text:
        return 0
    chinese = 0
    other = 0
    for c in text:
        if "\u4e00" <= c <= "\u9fff":
            chinese += 1
        else:
            other += 1
    chinese_tokens = (chinese * 2 + 2) // 3
    other_tokens = (other + 3) // 4
    return max(chinese_tokens + other_tokens, 1)


def _sse_data_bytes(obj: dict[str, Any]) -> bytes:
    data = json.dumps(obj, ensure_ascii=False)
    event_type = obj.get("type", "")
    if event_type:
        return f"event: {event_type}\ndata: {data}\n\n".encode("utf-8")
    return f"data: {data}\n\n".encode("utf-8")


@dataclass(slots=True)
class _KiroStreamState:
    model: str
    thinking_enabled: bool
    estimated_input_tokens: int = 0

    message_id: str = field(default_factory=lambda: f"msg_{uuid.uuid4().hex}")
    output_tokens: int = 0
    context_input_tokens: int | None = None

    next_block_index: int = 0
    open_blocks: dict[int, str] = field(default_factory=dict)

    text_block_index: int | None = None
    thinking_block_index: int | None = None
    tool_block_indices: dict[str, int] = field(default_factory=dict)

    thinking_buffer: str = ""
    in_thinking_block: bool = False
    thinking_extracted: bool = False
    strip_thinking_leading_newline: bool = False

    has_tool_use: bool = False
    stop_reason_override: str | None = None
    had_error: bool = False
    _last_content: str = ""

    def generate_initial_events(self) -> list[dict[str, Any]]:
        events: list[dict[str, Any]] = []

        # message_start
        events.append(
            {
                "type": "message_start",
                "message": {
                    "id": self.message_id,
                    "type": "message",
                    "role": "assistant",
                    "content": [],
                    "model": self.model,
                    "stop_reason": None,
                    "stop_sequence": None,
                    # Claude CLI clients expect usage to exist.
                    "usage": {
                        "input_tokens": int(self.estimated_input_tokens or 0),
                        "output_tokens": 1,
                    },
                },
            }
        )

        if not self.thinking_enabled:
            events.extend(self._ensure_text_block_open())

        return events

    def _ensure_text_block_open(self) -> list[dict[str, Any]]:
        if self.text_block_index is not None:
            if (
                self.text_block_index in self.open_blocks
                and self.open_blocks[self.text_block_index] == "text"
            ):
                return []
            self.text_block_index = None

        idx = self.next_block_index
        self.next_block_index += 1
        self.text_block_index = idx
        self.open_blocks[idx] = "text"

        return [
            {
                "type": "content_block_start",
                "index": idx,
                "content_block": {"type": "text", "text": ""},
            }
        ]

    def _close_block(self, idx: int) -> list[dict[str, Any]]:
        if idx not in self.open_blocks:
            return []
        self.open_blocks.pop(idx, None)
        return [{"type": "content_block_stop", "index": idx}]

    def _ensure_thinking_block_open(self) -> list[dict[str, Any]]:
        if self.thinking_block_index is not None:
            if (
                self.thinking_block_index in self.open_blocks
                and self.open_blocks[self.thinking_block_index] == "thinking"
            ):
                return []

        idx = self.next_block_index
        self.next_block_index += 1
        self.thinking_block_index = idx
        self.open_blocks[idx] = "thinking"

        return [
            {
                "type": "content_block_start",
                "index": idx,
                "content_block": {"type": "thinking", "thinking": ""},
            }
        ]

    def _emit_text_delta(self, text: str) -> list[dict[str, Any]]:
        if not text:
            return []
        events: list[dict[str, Any]] = []
        events.extend(self._ensure_text_block_open())
        idx = int(self.text_block_index or 0)
        events.append(
            {
                "type": "content_block_delta",
                "index": idx,
                "delta": {"type": "text_delta", "text": text},
            }
        )
        return events

    def _emit_thinking_delta(self, thinking: str) -> list[dict[str, Any]]:
        if not thinking:
            return []
        events: list[dict[str, Any]] = []
        events.extend(self._ensure_thinking_block_open())
        idx = int(self.thinking_block_index or 0)
        events.append(
            {
                "type": "content_block_delta",
                "index": idx,
                "delta": {"type": "thinking_delta", "thinking": thinking},
            }
        )
        return events

    def _close_thinking_block(self) -> list[dict[str, Any]]:
        """Send an empty thinking_delta sentinel and close the thinking block."""
        if self.thinking_block_index is None:
            return []
        idx = int(self.thinking_block_index)
        events: list[dict[str, Any]] = [
            {
                "type": "content_block_delta",
                "index": idx,
                "delta": {"type": "thinking_delta", "thinking": ""},
            }
        ]
        events.extend(self._close_block(idx))
        return events

    def process_context_usage(self, percentage: float) -> None:
        try:
            pct = float(percentage)
        except Exception:
            return
        # percentage * CONTEXT_WINDOW_TOKENS / 100
        self.context_input_tokens = int(pct * float(CONTEXT_WINDOW_TOKENS) / 100.0)

    def process_exception(self, exception_type: str) -> None:
        if exception_type == "ContentLengthExceededException":
            # ContentLengthExceededException is a normal completion signal (output
            # exceeded size limit), not a fatal error.  We record the stop_reason
            # but do NOT set had_error so that finalize() still emits message_delta
            # with stop_reason="max_tokens" and message_stop.
            self.stop_reason_override = "max_tokens"
            return

    def process_assistant_response(self, content: str) -> list[dict[str, Any]]:
        if not content:
            return []

        # Kiro may send duplicate content events; skip exact repeats.
        if content == self._last_content:
            return []
        self._last_content = content

        self.output_tokens += _estimate_tokens(content)

        if not self.thinking_enabled:
            return self._emit_text_delta(content)

        self.thinking_buffer += content

        # Safety: flush as text if thinking_buffer grows too large without closing tag
        if len(self.thinking_buffer) > _MAX_THINKING_BUFFER:
            logger.warning(
                "kiro thinking_buffer exceeded {} bytes, force-flushing as text",
                _MAX_THINKING_BUFFER,
            )
            overflow = self.thinking_buffer
            self.thinking_buffer = ""
            if self.in_thinking_block:
                result = self._emit_thinking_delta(overflow)
                result.extend(self._close_thinking_block())
                self.in_thinking_block = False
                self.thinking_extracted = True
                return result
            return self._emit_text_delta(overflow)

        events: list[dict[str, Any]] = []

        while True:
            if not self.in_thinking_block and not self.thinking_extracted:
                start_pos = _find_real_thinking_start_tag(self.thinking_buffer)
                if start_pos is not None:
                    before = self.thinking_buffer[:start_pos]
                    if before and before.strip():
                        events.extend(self._emit_text_delta(before))

                    self.in_thinking_block = True
                    self.strip_thinking_leading_newline = True
                    self.thinking_buffer = self.thinking_buffer[start_pos + len("<thinking>") :]
                    events.extend(self._ensure_thinking_block_open())
                    continue

                # Keep a short suffix in buffer for partial tag detection.
                keep = len("<thinking>")
                if len(self.thinking_buffer) > keep:
                    safe = self.thinking_buffer[:-keep]
                    if safe and safe.strip():
                        events.extend(self._emit_text_delta(safe))
                        self.thinking_buffer = self.thinking_buffer[-keep:]
                break

            if self.in_thinking_block:
                # Strip a single leading \n after <thinking> tag.
                # The model outputs `<thinking>\n` and the \n may arrive in the
                # same chunk or the next one; we drop it for cleaner output.
                if self.strip_thinking_leading_newline:
                    if self.thinking_buffer.startswith("\n"):
                        self.thinking_buffer = self.thinking_buffer[1:]
                        self.strip_thinking_leading_newline = False
                    elif self.thinking_buffer:
                        # Buffer is non-empty but doesn't start with \n; stop waiting.
                        self.strip_thinking_leading_newline = False
                    # else: buffer is empty, keep the flag for the next chunk.

                end_pos = _find_real_thinking_end_tag(self.thinking_buffer)
                if end_pos is not None:
                    thinking_text = self.thinking_buffer[:end_pos]
                    if thinking_text:
                        events.extend(self._emit_thinking_delta(thinking_text))

                    events.extend(self._close_thinking_block())

                    self.in_thinking_block = False
                    self.thinking_extracted = True
                    self.thinking_buffer = self.thinking_buffer[end_pos + len("</thinking>") :]
                    continue

                keep = len("</thinking>")
                if len(self.thinking_buffer) > keep:
                    safe = self.thinking_buffer[:-keep]
                    if safe:
                        events.extend(self._emit_thinking_delta(safe))
                        self.thinking_buffer = self.thinking_buffer[-keep:]
                break

            # thinking extracted: remaining buffer is text
            if self.thinking_buffer:
                remaining = self.thinking_buffer
                self.thinking_buffer = ""
                events.extend(self._emit_text_delta(remaining))
            break

        return events

    def process_tool_use(
        self,
        *,
        name: str,
        tool_use_id: str,
        input_json: str,
        stop: bool,
    ) -> list[dict[str, Any]]:
        if not tool_use_id:
            return []

        self.has_tool_use = True

        events: list[dict[str, Any]] = []

        # Boundary: close thinking block if needed, filtering a dangling </thinking>.
        if self.thinking_enabled and self.in_thinking_block and self.thinking_buffer:
            end_pos = _find_real_thinking_end_tag_at_buffer_end(self.thinking_buffer)
            if end_pos is not None:
                thinking_text = self.thinking_buffer[:end_pos]
                if thinking_text:
                    events.extend(self._emit_thinking_delta(thinking_text))

                events.extend(self._close_thinking_block())

                after_pos = end_pos + len("</thinking>")
                remaining = self.thinking_buffer[after_pos:]
                self.thinking_buffer = ""
                self.in_thinking_block = False
                self.thinking_extracted = True
                if remaining:
                    events.extend(self._emit_text_delta(remaining))
            else:
                # Best-effort flush all as thinking
                events.extend(self._emit_thinking_delta(self.thinking_buffer))
                events.extend(self._close_thinking_block())
                self.thinking_buffer = ""
                self.in_thinking_block = False
                self.thinking_extracted = True

        # Flush any buffered pre-thinking tail so tool_use doesn't swallow it.
        if (
            self.thinking_enabled
            and not self.in_thinking_block
            and not self.thinking_extracted
            and self.thinking_buffer
        ):
            buffered = self.thinking_buffer
            self.thinking_buffer = ""
            events.extend(self._emit_text_delta(buffered))

        # Close current text block before tool_use.
        if self.text_block_index is not None:
            idx = int(self.text_block_index)
            events.extend(self._close_block(idx))

        block_index = self.tool_block_indices.get(tool_use_id)
        if block_index is None:
            block_index = self.next_block_index
            self.next_block_index += 1
            self.tool_block_indices[tool_use_id] = block_index

        # Start tool block if not open.
        if block_index not in self.open_blocks:
            self.open_blocks[block_index] = "tool_use"
            events.append(
                {
                    "type": "content_block_start",
                    "index": block_index,
                    "content_block": {
                        "type": "tool_use",
                        "id": tool_use_id,
                        "name": name,
                        "input": {},
                    },
                }
            )

        if input_json:
            self.output_tokens += _estimate_tokens(input_json)
            events.append(
                {
                    "type": "content_block_delta",
                    "index": block_index,
                    "delta": {"type": "input_json_delta", "partial_json": input_json},
                }
            )

        if stop:
            events.extend(self._close_block(block_index))

        return events

    def finalize(self) -> list[dict[str, Any]]:
        events: list[dict[str, Any]] = []

        # Flush remaining thinking/text buffer.
        if self.thinking_enabled and self.thinking_buffer:
            if self.in_thinking_block:
                end_pos = _find_real_thinking_end_tag_at_buffer_end(self.thinking_buffer)
                if end_pos is not None:
                    thinking_text = self.thinking_buffer[:end_pos]
                    if thinking_text:
                        events.extend(self._emit_thinking_delta(thinking_text))

                    events.extend(self._close_thinking_block())

                    after_pos = end_pos + len("</thinking>")
                    remaining = self.thinking_buffer[after_pos:]
                    if remaining:
                        events.extend(self._emit_text_delta(remaining))
                else:
                    events.extend(self._emit_thinking_delta(self.thinking_buffer))
                    events.extend(self._close_thinking_block())

            else:
                events.extend(self._emit_text_delta(self.thinking_buffer))

        self.thinking_buffer = ""
        self.in_thinking_block = False
        self.thinking_extracted = True

        # Close any open blocks (best-effort).
        for idx in sorted(list(self.open_blocks.keys()), reverse=True):
            events.extend(self._close_block(idx))

        stop_reason = self.stop_reason_override
        if not stop_reason:
            stop_reason = "tool_use" if self.has_tool_use else "end_turn"

        input_tokens = (
            int(self.context_input_tokens)
            if self.context_input_tokens is not None
            else int(self.estimated_input_tokens or 0)
        )

        events.append(
            {
                "type": "message_delta",
                "delta": {"stop_reason": stop_reason, "stop_sequence": None},
                "usage": {"input_tokens": input_tokens, "output_tokens": int(self.output_tokens)},
            }
        )
        events.append({"type": "message_stop"})

        return events


async def rewrite_eventstream_to_sse(
    byte_iterator: Any,
    *,
    model: str,
    thinking_enabled: bool,
    estimated_input_tokens: int = 0,
) -> AsyncGenerator[bytes]:
    """Rewrite Kiro AWS Event Stream bytes to Claude SSE bytes."""
    decoder = EventStreamDecoder()
    state = _KiroStreamState(
        model=str(model or ""),
        thinking_enabled=bool(thinking_enabled),
        estimated_input_tokens=int(estimated_input_tokens or 0),
    )

    # 收集原始字节用于错误诊断
    raw_bytes_buffer = b""

    # Initial events
    for evt in state.generate_initial_events():
        yield _sse_data_bytes(evt)

    async for chunk in byte_iterator:
        if not chunk:
            continue

        # 保留原始字节用于错误诊断（限制大小）
        if len(raw_bytes_buffer) < 4096:
            raw_bytes_buffer += chunk

        try:
            decoder.feed(chunk)
            frames = decoder.decode_available()
        except Exception as e:
            logger.warning("kiro eventstream decode error: {}", e)
            # 尝试解析原始响应为 JSON 错误
            error_message = f"kiro eventstream decode failed: {type(e).__name__}"
            try:
                raw_text = raw_bytes_buffer.decode("utf-8", errors="replace")
                # 尝试解析为 JSON
                error_json = json.loads(raw_text)
                if isinstance(error_json, dict):
                    # 提取上游错误信息
                    upstream_msg = error_json.get("message") or error_json.get("error", {}).get(
                        "message"
                    )
                    if upstream_msg:
                        error_message = f"Kiro API error: {upstream_msg}"
            except Exception:
                pass
            try:
                from src.services.provider.adapters.kiro.context import get_kiro_request_context

                kiro_ctx = get_kiro_request_context()
                diag = build_kiro_network_diagnostic(
                    http_status=getattr(kiro_ctx, "last_http_status", None) if kiro_ctx else None,
                    http_category=(
                        getattr(kiro_ctx, "last_http_error_category", None) if kiro_ctx else None
                    ),
                    connection_summary=(
                        getattr(kiro_ctx, "last_connection_error_summary", None)
                        if kiro_ctx
                        else None
                    ),
                )
                if diag:
                    error_message = f"{error_message} | {diag}"
            except Exception:
                pass
            yield _sse_data_bytes(
                {
                    "type": "error",
                    "error": {
                        "type": "upstream_stream_error",
                        "message": error_message,
                    },
                }
            )
            break

        for frame in frames:
            mtype = (frame.message_type() or "event").strip().lower()
            etype = (frame.event_type() or "").strip()
            payload_text = frame.payload_as_text()

            if mtype == "event":
                try:
                    payload = json.loads(payload_text) if payload_text else {}
                except Exception:
                    payload = {}

                if etype == "assistantResponseEvent":
                    content = payload.get("content") if isinstance(payload, dict) else None
                    if isinstance(content, str) and content:
                        for evt in state.process_assistant_response(content):
                            yield _sse_data_bytes(evt)
                    continue

                if etype == "toolUseEvent":
                    if isinstance(payload, dict):
                        name = str(payload.get("name") or "")
                        tool_use_id = payload.get("toolUseId") or payload.get("tool_use_id")
                        tool_use_id = str(tool_use_id or "")
                        raw_input = payload.get("input")
                        if raw_input is None:
                            input_json = ""
                        elif isinstance(raw_input, str):
                            input_json = raw_input
                        else:
                            try:
                                input_json = json.dumps(raw_input, ensure_ascii=False)
                            except Exception:
                                input_json = str(raw_input)
                        stop = bool(payload.get("stop", False))
                        for evt in state.process_tool_use(
                            name=name,
                            tool_use_id=tool_use_id,
                            input_json=input_json,
                            stop=stop,
                        ):
                            yield _sse_data_bytes(evt)
                    continue

                if etype == "contextUsageEvent":
                    if isinstance(payload, dict):
                        pct = payload.get("contextUsagePercentage")
                        if pct is not None:
                            try:
                                state.process_context_usage(float(pct))
                            except (ValueError, TypeError):
                                logger.debug(
                                    "kiro: failed to parse contextUsagePercentage: {!r}", pct
                                )
                    continue

                # meteringEvent / unknown: ignore
                continue

            if mtype == "exception":
                ex_type = frame.headers.exception_type() or "UnknownException"
                state.process_exception(ex_type)
                # ContentLengthExceededException is handled by process_exception
                # (sets stop_reason_override) and should NOT prevent finalize().
                if not state.stop_reason_override:
                    state.had_error = True
                logger.debug("kiro upstream exception: {} | {}", ex_type, payload_text[:200])
                if state.had_error:
                    yield _sse_data_bytes(
                        {
                            "type": "error",
                            "error": {
                                "type": "upstream_exception",
                                "message": ex_type,
                            },
                        }
                    )
                continue

            if mtype == "error":
                err_code = frame.headers.error_code() or "UnknownError"
                state.had_error = True
                logger.debug("kiro upstream error: {} | {}", err_code, payload_text[:200])
                yield _sse_data_bytes(
                    {
                        "type": "error",
                        "error": {
                            "type": "upstream_error",
                            "message": err_code,
                        },
                    }
                )
                continue

    if not state.had_error:
        for evt in state.finalize():
            yield _sse_data_bytes(evt)


def apply_kiro_stream_rewrite(
    byte_iter: Any,
    *,
    model: str = "",
    input_tokens: int = 0,
    prefetched_chunks: list[bytes] | None = None,
) -> AsyncGenerator[bytes]:
    """Apply Kiro EventStream->SSE rewrite if context is available.

    Consolidates the repeated import-context-rewrite pattern used across
    ``chat_handler_base``, ``cli_handler_base``, and ``stream_processor``.

    Args:
        byte_iter: Upstream byte iterator (raw AWS Event Stream).
        model: Model name for SSE events.
        input_tokens: Estimated input token count.
        prefetched_chunks: Optional pre-fetched bytes to prepend.

    Returns:
        An async generator of Claude-compatible SSE bytes.
    """
    from src.services.provider.adapters.kiro.context import get_kiro_request_context

    kiro_ctx = get_kiro_request_context()
    thinking_enabled = bool(getattr(kiro_ctx, "thinking_enabled", False)) if kiro_ctx else False

    if prefetched_chunks:
        upstream = byte_iter
        prefix = list(prefetched_chunks)

        async def _combined() -> AsyncGenerator[bytes, None]:
            for c in prefix:
                if c:
                    yield c
            async for c in upstream:
                if c:
                    yield c

        source: Any = _combined()
    else:
        source = byte_iter

    return rewrite_eventstream_to_sse(
        source,
        model=str(model or ""),
        thinking_enabled=thinking_enabled,
        estimated_input_tokens=int(input_tokens or 0),
    )


__all__ = [
    "apply_kiro_stream_rewrite",
    "rewrite_eventstream_to_sse",
]
