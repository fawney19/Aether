"""Codex Responses SSE 事件解析器 — 给 image 聚合器和 endpoint_checker 共用。

Codex 反代 `/responses` 同一个协议被两处消费：
- ``aggregate_codex_image_sse``（运行时图像生成）
- ``endpoint_checker._execute_stream_request``（管理面测试模型）

把事件识别逻辑放在本模块里，两处都走 ``parse_codex_sse_payload``，
避免同一个协议被解析两遍而 bug 需要修两次。

识别的事件类型：
- ``response.created``            — 拿 ``created_at``
- ``response.output_item.done``   — ``item.type == "image_generation_call"`` 时收 b64 图像
- ``response.completed``          — 终局 usage / 响应元信息
- ``response.failed``             — 上游致命错误（→ CodexStreamError）
- ``response.incomplete``         — 生成未完成（→ CodexStreamError）
- ``response.error``              — 通用错误事件（→ CodexStreamError）
- ``response.content_filter.*``   — 内容过滤阻断（→ CodexStreamError）
- 其它 ``error``/``type=="error"``    — 兜底错误事件

未识别事件记 DEBUG 日志，不静默丢弃。
"""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from typing import Any

from src.core.logger import logger


class CodexStreamError(Exception):
    """Codex SSE 流中携带的上游错误事件。

    由 ``parse_codex_sse_payload`` 抛出；handler 侧应捕获并转为 ``UpstreamClientException``
    进入 ErrorClassifier 分类；endpoint_checker 侧转为测试失败。

    ``upstream_request_id`` 来自 Codex 自己的 ``request_id`` 字段，用户可以拿这个 ID
    去 OpenAI help center 直接查具体原因，别被我们这边的抽象盖掉。
    """

    def __init__(
        self,
        message: str,
        *,
        event_type: str,
        code: str | None = None,
        upstream_request_id: str | None = None,
        raw: dict[str, Any] | None = None,
    ) -> None:
        super().__init__(message)
        self.event_type = event_type
        self.code = code
        self.upstream_request_id = upstream_request_id
        self.raw = raw or {}


@dataclass
class CodexImageStreamState:
    """单次流式聚合状态，由调用方持有，事件解析回填。"""

    created_at: int | None = None
    images: list[dict[str, Any]] = field(default_factory=list)
    completed_response: dict[str, Any] = field(default_factory=dict)
    # 统计：观察性用途
    event_count: int = 0
    unknown_event_types: list[str] = field(default_factory=list)


# 被视为 *致命* 的事件类型 —— 命中任一个都应当中止聚合并上报错误。
# 公开给 image_transform 复用（避免下划线跨模块引用的反模式）。
FATAL_EVENT_TYPES: frozenset[str] = frozenset(
    {
        "response.failed",
        "response.incomplete",
        "response.error",
        "error",
    }
)
# 保留原名供内部引用
_FATAL_EVENT_TYPES = FATAL_EVENT_TYPES

# 已知但"无需动作"的事件，避免被当作 unknown 漏报
_KNOWN_NOOP_EVENT_TYPES = frozenset(
    {
        "response.in_progress",
        "response.output_item.added",
        "response.output_text.delta",
        "response.output_text.done",
        "response.output_item.delta",
        "response.image_generation_call.in_progress",
        "response.image_generation_call.partial_image",
        "response.image_generation_call.generating",
        "response.content_part.added",
        "response.content_part.done",
    }
)


def iter_sse_data_lines(event_bytes: bytes) -> list[str]:
    """从一段 SSE event bytes 提取所有 ``data:`` 行的 payload 字符串。

    - 支持多行 ``data:`` 合并（按 SSE 规范）
    - 过滤空串与 ``[DONE]``
    """
    try:
        text = event_bytes.decode("utf-8", errors="ignore")
    except Exception:
        return []

    out: list[str] = []
    for line in text.splitlines():
        stripped = line.strip()
        if not stripped.startswith("data:"):
            continue
        payload = stripped[5:].strip()
        if not payload or payload == "[DONE]":
            continue
        out.append(payload)
    return out


def parse_codex_sse_payload(
    payload: str | dict[str, Any],
    state: CodexImageStreamState,
) -> None:
    """解析单条 Codex SSE JSON payload 并回填 ``state``。

    命中致命事件时抛 :class:`CodexStreamError`。
    未识别事件记 DEBUG 日志、累计到 ``state.unknown_event_types``，不抛。
    """
    obj: Any
    if isinstance(payload, dict):
        obj = payload
    else:
        try:
            obj = json.loads(payload)
        except Exception:
            return
    if not isinstance(obj, dict):
        return

    state.event_count += 1
    evt_type = str(obj.get("type") or "")

    # 致命错误事件 —— 立即抛
    if evt_type in _FATAL_EVENT_TYPES:
        raise _build_stream_error(evt_type, obj)

    # content filter 类事件可能是 response.content_filter.violation 等
    if evt_type.startswith("response.content_filter"):
        raise _build_stream_error(evt_type, obj)

    # 有些上游把错误塞到 response.completed.response.error 里
    if evt_type == "response.completed":
        response = obj.get("response")
        if isinstance(response, dict):
            err = response.get("error") or response.get("incomplete_details")
            if isinstance(err, dict) and (err.get("message") or err.get("code") or err.get("reason")):
                raise _build_stream_error("response.completed", obj)
            state.completed_response = response
        return

    if evt_type == "response.created":
        response = obj.get("response")
        if isinstance(response, dict):
            value = response.get("created_at")
            if isinstance(value, int):
                state.created_at = value
        return

    if evt_type == "response.output_item.done":
        item = obj.get("item")
        if not isinstance(item, dict):
            return
        if item.get("type") != "image_generation_call":
            return
        result = item.get("result")
        if not isinstance(result, str) or not result:
            return
        state.images.append(
            {
                "b64_json": result,
                "revised_prompt": item.get("revised_prompt"),
            }
        )
        return

    # 已知 no-op：不报
    if evt_type in _KNOWN_NOOP_EVENT_TYPES:
        return

    # 未识别：留痕（便于未来 Codex 协议演进时发现）
    if evt_type and evt_type not in state.unknown_event_types:
        state.unknown_event_types.append(evt_type)
        logger.debug("[CodexSSE] unknown event type: {}", evt_type)


def build_stream_error(evt_type: str, obj: dict[str, Any]) -> CodexStreamError:
    """从上游错误事件 payload 构造 :class:`CodexStreamError`。公开给 image_transform
    等同包内模块复用。内部原名 ``_build_stream_error`` 保留向后兼容。"""
    return _build_stream_error(evt_type, obj)


def _build_stream_error(evt_type: str, obj: dict[str, Any]) -> CodexStreamError:
    import re

    response = obj.get("response") if isinstance(obj.get("response"), dict) else {}

    # 常见的错误结构：{"error": {"code": "...", "message": "..."}}
    err_source: dict[str, Any] = {}
    for candidate in (
        obj.get("error"),
        response.get("error"),
        response.get("incomplete_details"),
        obj.get("incomplete_details"),
    ):
        if isinstance(candidate, dict) and candidate:
            err_source = candidate
            break

    code = str(err_source.get("code") or err_source.get("reason") or "")
    message = str(
        err_source.get("message")
        or err_source.get("detail")
        or response.get("status")
        or evt_type
    )[:500]

    # Codex 会在几个位置挂 request_id；依次查，任何一个命中就用。
    # 最后还能从 message 里正则捞 —— Codex 习惯把 "Please include the request ID xxx"
    # 写在消息文本里。
    upstream_request_id: str | None = None
    for rid_candidate in (
        obj.get("request_id"),
        err_source.get("request_id"),
        response.get("id"),
        response.get("request_id"),
    ):
        if isinstance(rid_candidate, str) and rid_candidate.strip():
            upstream_request_id = rid_candidate.strip()
            break
    if not upstream_request_id and message:
        match = re.search(
            r"request\s*ID\s+([0-9a-f-]{16,})", message, re.IGNORECASE
        )
        if match:
            upstream_request_id = match.group(1)

    suffix = f" | upstream_request_id={upstream_request_id}" if upstream_request_id else ""
    return CodexStreamError(
        f"Codex stream error: {evt_type} — {message}{suffix}",
        event_type=evt_type,
        code=code or None,
        upstream_request_id=upstream_request_id,
        raw=obj,
    )


def extract_codex_image_usage(
    completed_response: dict[str, Any],
) -> tuple[dict[str, Any] | None, dict[str, Any] | None]:
    """从 ``response.completed`` 里拿 (billing_usage, tool_usage_metadata)。

    **优先级**（与 Rust 分支 ``sync_finalize.rs`` 对齐）：
      1. ``response.tool_usage.image_gen`` —— **Codex 把标准 usage 字段真正放在这里**；
         它的字段名就是 openai 标准（``input_tokens`` / ``output_tokens``），不是
         `"images"` / `"images_generated"` —— 这是之前审查时猜错的一点。
      2. ``response.usage`` —— 某些 Codex 版本会同时填顶层 usage；作为 fallback。

    返回 ``(usage_for_billing, tool_usage_metadata)``；
    billing_usage 直接给 billing collector 按 ``usage.input_tokens`` 路径取。
    """
    if not isinstance(completed_response, dict):
        return None, None

    billing_usage: dict[str, Any] | None = None
    tool_usage_metadata: dict[str, Any] | None = None

    tool_usage = completed_response.get("tool_usage")
    if isinstance(tool_usage, dict):
        image_gen = tool_usage.get("image_gen")
        if isinstance(image_gen, dict) and image_gen:
            billing_usage = image_gen
            tool_usage_metadata = image_gen

    if billing_usage is None:
        usage = completed_response.get("usage")
        if isinstance(usage, dict) and usage:
            billing_usage = usage

    return billing_usage, tool_usage_metadata


__all__ = [
    "CodexStreamError",
    "CodexImageStreamState",
    "iter_sse_data_lines",
    "parse_codex_sse_payload",
    "extract_codex_image_usage",
]
