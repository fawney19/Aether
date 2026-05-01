"""Sync<->stream bridge helpers for the conversion layer.

We already have:
- streaming conversion: source stream chunk -> internal events -> target stream chunk
- sync conversion: source response -> internal response -> target response

This module fills the missing link:
- aggregate internal stream events into a single InternalResponse (stream -> sync)
- expand an InternalResponse into internal stream events (sync -> stream)

Used by handler-layer upstream policies that force upstream streaming mode.
"""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from typing import Any, Iterable, Iterator

from .internal import (
    ContentType,
    ImageBlock,
    InternalResponse,
    StopReason,
    TextBlock,
    ThinkingBlock,
    ToolUseBlock,
    UsageInfo,
)
from .stream_events import (
    ContentBlockStartEvent,
    ContentBlockStopEvent,
    ContentDeltaEvent,
    InternalStreamEvent,
    MessageStartEvent,
    MessageStopEvent,
    ToolCallDeltaEvent,
    UsageEvent,
)


@dataclass
class _BlockBuilder:
    block_type: ContentType
    text: str = ""
    tool_id: str | None = None
    tool_name: str | None = None
    tool_args_json: str = ""
    image_data: str | None = None
    image_media_type: str | None = None
    extra: dict[str, Any] = field(default_factory=dict)

    def finalize(self) -> Any:
        if self.block_type == ContentType.TEXT:
            return TextBlock(text=self.text, extra=self.extra)

        if self.block_type == ContentType.THINKING:
            signature = self.extra.get("signature") or self.extra.get("thought_signature")
            return ThinkingBlock(
                thinking=self.text,
                signature=str(signature) if signature is not None else None,
                extra=self.extra,
            )

        if self.block_type == ContentType.TOOL_USE:
            tool_input: dict[str, Any] = {}
            raw = self.tool_args_json.strip()
            if raw:
                try:
                    parsed = json.loads(raw)
                    if isinstance(parsed, dict):
                        tool_input = parsed
                except Exception:
                    tool_input = {}
            return ToolUseBlock(
                tool_id=str(self.tool_id or ""),
                tool_name=str(self.tool_name or ""),
                tool_input=tool_input,
                extra=self.extra,
            )

        if self.block_type == ContentType.IMAGE:
            return ImageBlock(
                data=self.image_data,
                media_type=self.image_media_type,
                url=None,
                extra=self.extra,
            )

        # Unknown block type: best-effort drop.
        return TextBlock(text=self.text, extra=self.extra)


class InternalStreamAggregator:
    """Aggregate internal stream events into a single InternalResponse (best-effort)."""

    def __init__(
        self,
        *,
        fallback_id: str = "resp",
        fallback_model: str = "",
    ) -> None:
        self._fallback_id = fallback_id
        self._fallback_model = fallback_model

        self._id: str | None = None
        self._model: str | None = None
        self._stop_reason: StopReason | None = None
        self._usage: UsageInfo | None = None

        self._open: dict[int, _BlockBuilder] = {}
        self._final: dict[int, Any] = {}

    def feed(self, events: Iterable[InternalStreamEvent]) -> None:
        for ev in events:
            if isinstance(ev, MessageStartEvent):
                if ev.message_id:
                    self._id = ev.message_id
                if ev.model:
                    self._model = ev.model
                if ev.usage:
                    self._usage = ev.usage
                continue

            if isinstance(ev, UsageEvent):
                if ev.usage:
                    self._usage = ev.usage
                continue

            if isinstance(ev, ContentBlockStartEvent):
                b = _BlockBuilder(block_type=ev.block_type, extra=dict(ev.extra or {}))
                if ev.block_type == ContentType.TOOL_USE:
                    b.tool_id = ev.tool_id
                    b.tool_name = ev.tool_name
                if ev.block_type == ContentType.IMAGE:
                    b.image_data = b.extra.get("image_data") or b.extra.get("data")
                    b.image_media_type = b.extra.get("image_media_type") or b.extra.get("mime_type")
                self._open[int(ev.block_index)] = b
                continue

            if isinstance(ev, ContentDeltaEvent):
                idx = int(ev.block_index)
                b = self._open.get(idx)
                if b is None:
                    b = _BlockBuilder(block_type=ContentType.TEXT)
                    self._open[idx] = b
                if ev.text_delta:
                    b.text += ev.text_delta
                if ev.extra:
                    b.extra.update(ev.extra)
                continue

            if isinstance(ev, ToolCallDeltaEvent):
                idx = int(ev.block_index)
                b = self._open.get(idx)
                if b is None:
                    b = _BlockBuilder(block_type=ContentType.TOOL_USE)
                    self._open[idx] = b
                if ev.input_delta:
                    b.tool_args_json += ev.input_delta
                continue

            if isinstance(ev, ContentBlockStopEvent):
                idx = int(ev.block_index)
                b = self._open.pop(idx, None)
                if b is not None:
                    self._final.setdefault(idx, b.finalize())
                continue

            if isinstance(ev, MessageStopEvent):
                self._stop_reason = ev.stop_reason
                if ev.usage:
                    self._usage = ev.usage
                # Flush remaining open blocks (best-effort).
                for idx, b in list(self._open.items()):
                    self._final.setdefault(idx, b.finalize())
                self._open.clear()
                continue

    @property
    def open_count(self) -> int:
        """当前未关闭的 block 数量。"""
        return len(self._open)

    @property
    def final_count(self) -> int:
        """已完成的 block 数量。"""
        return len(self._final)

    @property
    def usage(self) -> UsageInfo | None:
        return self._usage

    @property
    def stop_reason(self) -> StopReason | None:
        return self._stop_reason

    def build(self) -> InternalResponse:
        # Flush remaining open blocks (best-effort) in case MessageStopEvent was never received.
        for idx, b in list(self._open.items()):
            self._final.setdefault(idx, b.finalize())
        self._open.clear()

        rid = self._id or self._fallback_id
        model = self._model or self._fallback_model
        content = [self._final[k] for k in sorted(self._final.keys())]
        return InternalResponse(
            id=str(rid or "resp"),
            model=str(model or ""),
            content=content,
            stop_reason=self._stop_reason,
            usage=self._usage,
        )


def iter_internal_response_as_stream_events(
    internal: InternalResponse,
    *,
    chunk_text: bool = False,
    text_chunk_size: int = 200,
) -> Iterator[InternalStreamEvent]:
    """Expand an InternalResponse into internal stream events (best-effort).

    This is used to simulate SSE when the upstream is forced to sync mode.
    """

    msg_id = str(internal.id or "resp")
    model = str(internal.model or "")

    yield MessageStartEvent(message_id=msg_id, model=model)

    block_index = 0
    for block in internal.content or []:
        # Thinking / reasoning
        if isinstance(block, ThinkingBlock):
            block_extra = dict(block.extra or {})
            yield ContentBlockStartEvent(
                block_index=block_index,
                block_type=ContentType.THINKING,
                extra=block_extra,
            )
            thinking = str(block.thinking or "")
            if not chunk_text or text_chunk_size <= 0:
                if thinking:
                    yield ContentDeltaEvent(block_index=block_index, text_delta=thinking)
            else:
                for i in range(0, len(thinking), text_chunk_size):
                    part = thinking[i : i + text_chunk_size]
                    if part:
                        yield ContentDeltaEvent(block_index=block_index, text_delta=part)
            signature = block.signature or block_extra.get("thought_signature") or block_extra.get(
                "signature"
            )
            if signature:
                yield ContentDeltaEvent(
                    block_index=block_index,
                    extra={"thought_signature": str(signature)},
                )
            yield ContentBlockStopEvent(block_index=block_index)
            block_index += 1
            continue

        # Text
        if isinstance(block, TextBlock):
            yield ContentBlockStartEvent(block_index=block_index, block_type=ContentType.TEXT)
            text = str(block.text or "")
            if not chunk_text or text_chunk_size <= 0:
                if text:
                    yield ContentDeltaEvent(block_index=block_index, text_delta=text)
            else:
                for i in range(0, len(text), text_chunk_size):
                    part = text[i : i + text_chunk_size]
                    if part:
                        yield ContentDeltaEvent(block_index=block_index, text_delta=part)
            yield ContentBlockStopEvent(block_index=block_index)
            block_index += 1
            continue

        # Tool use
        if isinstance(block, ToolUseBlock):
            tool_id = block.tool_id or f"tool_{block_index}"
            yield ContentBlockStartEvent(
                block_index=block_index,
                block_type=ContentType.TOOL_USE,
                tool_id=tool_id,
                tool_name=block.tool_name or None,
            )
            payload = {}
            if isinstance(block.tool_input, dict):
                payload = block.tool_input
            yield ToolCallDeltaEvent(
                block_index=block_index,
                tool_id=str(tool_id),
                input_delta=json.dumps(payload, ensure_ascii=False),
            )
            yield ContentBlockStopEvent(block_index=block_index)
            block_index += 1
            continue

        # Image
        if isinstance(block, ImageBlock):
            yield ContentBlockStartEvent(
                block_index=block_index,
                block_type=ContentType.IMAGE,
                extra={
                    "image_data": block.data,
                    "image_media_type": block.media_type,
                },
            )
            yield ContentBlockStopEvent(block_index=block_index)
            block_index += 1
            continue

        # Unknown blocks: ignore.
        block_index += 1

    yield MessageStopEvent(
        stop_reason=internal.stop_reason or StopReason.END_TURN, usage=internal.usage
    )


__all__ = [
    "InternalStreamAggregator",
    "iter_internal_response_as_stream_events",
]
