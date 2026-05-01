"""
Claude SSE 流解析器

解析 Claude Messages API 的 Server-Sent Events 流。
"""

import json
from typing import Any

from src.core.usage_tokens import extract_cache_creation_tokens, extract_cache_ttl_minutes


class ClaudeStreamParser:
    """
    Claude SSE 流解析器

    解析 Claude Messages API 的 SSE 事件流。

    事件类型：
    - message_start: 消息开始，包含初始 message 对象
    - content_block_start: 内容块开始
    - content_block_delta: 内容块增量（文本、工具输入等）
    - content_block_stop: 内容块结束
    - message_delta: 消息增量，包含 stop_reason 和最终 usage
    - message_stop: 消息结束
    - ping: 心跳事件
    - error: 错误事件
    """

    # Claude SSE 事件类型
    EVENT_MESSAGE_START = "message_start"
    EVENT_MESSAGE_STOP = "message_stop"
    EVENT_MESSAGE_DELTA = "message_delta"
    EVENT_CONTENT_BLOCK_START = "content_block_start"
    EVENT_CONTENT_BLOCK_STOP = "content_block_stop"
    EVENT_CONTENT_BLOCK_DELTA = "content_block_delta"
    EVENT_PING = "ping"
    EVENT_ERROR = "error"

    # Delta 类型
    DELTA_TEXT = "text_delta"
    DELTA_INPUT_JSON = "input_json_delta"

    def parse_chunk(self, chunk: bytes | str) -> list[dict[str, Any]]:
        """
        解析 SSE 数据块

        Args:
            chunk: 原始 SSE 数据（bytes 或 str）

        Returns:
            解析后的事件列表
        """
        if isinstance(chunk, bytes):
            text = chunk.decode("utf-8")
        else:
            text = chunk

        events: list[dict[str, Any]] = []
        lines = text.strip().split("\n")

        current_event_type: str | None = None

        for line in lines:
            line = line.strip()
            if not line:
                continue

            # 解析事件类型行
            if line.startswith("event: "):
                current_event_type = line[7:]
                continue

            # 解析数据行
            if line.startswith("data: "):
                data_str = line[6:]

                # 处理 [DONE] 标记
                if data_str == "[DONE]":
                    events.append({"type": "__done__", "raw": "[DONE]"})
                    continue

                try:
                    data = json.loads(data_str)
                    # 如果数据中没有 type，使用事件行的类型
                    if "type" not in data and current_event_type:
                        data["type"] = current_event_type
                    events.append(data)
                except json.JSONDecodeError:
                    # 无法解析的数据，跳过
                    pass

                current_event_type = None

        return events

    def parse_line(self, line: str) -> dict[str, Any] | None:
        """
        解析单行 SSE 数据

        Args:
            line: SSE 数据行（已去除 "data: " 前缀）

        Returns:
            解析后的事件字典，如果无法解析返回 None
        """
        if not line or line == "[DONE]":
            return None

        try:
            result = json.loads(line)
            if isinstance(result, dict):
                return result
            return None
        except json.JSONDecodeError:
            return None

    def is_done_event(self, event: dict[str, Any]) -> bool:
        """
        判断是否为结束事件

        Args:
            event: 事件字典

        Returns:
            True 如果是结束事件
        """
        event_type = event.get("type")
        return event_type in (self.EVENT_MESSAGE_STOP, "__done__")

    def is_error_event(self, event: dict[str, Any]) -> bool:
        """
        判断是否为错误事件

        Args:
            event: 事件字典

        Returns:
            True 如果是错误事件
        """
        return event.get("type") == self.EVENT_ERROR

    def get_event_type(self, event: dict[str, Any]) -> str | None:
        """
        获取事件类型

        Args:
            event: 事件字典

        Returns:
            事件类型字符串
        """
        event_type = event.get("type")
        return str(event_type) if event_type is not None else None

    def extract_text_delta(self, event: dict[str, Any]) -> str | None:
        """
        从 content_block_delta 事件中提取文本增量

        Args:
            event: 事件字典

        Returns:
            文本增量，如果不是文本 delta 返回 None
        """
        if event.get("type") != self.EVENT_CONTENT_BLOCK_DELTA:
            return None

        delta = event.get("delta", {})
        if delta.get("type") == self.DELTA_TEXT:
            text = delta.get("text")
            return str(text) if text is not None else None

        return None

    def extract_usage(self, event: dict[str, Any]) -> dict[str, int] | None:
        """
        从事件中提取 token 使用量

        Args:
            event: 事件字典

        Returns:
            使用量字典，如果没有使用量信息返回 None
        """
        event_type = event.get("type")

        # message_start 事件包含初始 usage
        if event_type == self.EVENT_MESSAGE_START:
            message = event.get("message", {})
            usage = message.get("usage", {})
            if usage:
                return {
                    "input_tokens": usage.get("input_tokens", 0),
                    "output_tokens": usage.get("output_tokens", 0),
                    "cache_creation_tokens": extract_cache_creation_tokens(usage),
                    "cache_read_tokens": usage.get("cache_read_input_tokens", 0),
                    **(
                        {"cache_ttl_minutes": ttl}
                        if (ttl := extract_cache_ttl_minutes(usage)) is not None
                        else {}
                    ),
                }

        # message_delta 事件包含最终 usage
        if event_type == self.EVENT_MESSAGE_DELTA:
            usage = event.get("usage", {})
            if usage:
                return {
                    "input_tokens": usage.get("input_tokens", 0),
                    "output_tokens": usage.get("output_tokens", 0),
                    "cache_creation_tokens": extract_cache_creation_tokens(usage),
                    "cache_read_tokens": usage.get("cache_read_input_tokens", 0),
                    **(
                        {"cache_ttl_minutes": ttl}
                        if (ttl := extract_cache_ttl_minutes(usage)) is not None
                        else {}
                    ),
                }

        return None

    def extract_message_id(self, event: dict[str, Any]) -> str | None:
        """
        从 message_start 事件中提取消息 ID

        Args:
            event: 事件字典

        Returns:
            消息 ID，如果不是 message_start 返回 None
        """
        if event.get("type") != self.EVENT_MESSAGE_START:
            return None

        message = event.get("message", {})
        msg_id = message.get("id")
        return str(msg_id) if msg_id is not None else None

    def extract_stop_reason(self, event: dict[str, Any]) -> str | None:
        """
        从 message_delta 事件中提取停止原因

        Args:
            event: 事件字典

        Returns:
            停止原因，如果没有返回 None
        """
        if event.get("type") != self.EVENT_MESSAGE_DELTA:
            return None

        delta = event.get("delta", {})
        reason = delta.get("stop_reason")
        return str(reason) if reason is not None else None


__all__ = ["ClaudeStreamParser"]
