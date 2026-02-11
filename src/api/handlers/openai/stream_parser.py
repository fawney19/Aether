"""
OpenAI SSE 流解析器

解析 OpenAI Chat Completions API 的 Server-Sent Events 流。
"""

from typing import Any

from src.utils import json_helper as json


class OpenAIStreamParser:
    """
    OpenAI SSE 流解析器

    解析 OpenAI Chat Completions API 的 SSE 事件流。

    OpenAI 流格式：
    - 每个 chunk 是一个 JSON 对象，包含 choices 数组
    - choices[0].delta 包含增量内容
    - choices[0].finish_reason 表示结束原因
    - 流结束时发送 data: [DONE]
    """

    def parse_chunk(self, chunk: bytes | str) -> list[dict[str, Any]]:
        """
        解析 SSE 数据块

        Args:
            chunk: 原始 SSE 数据（bytes 或 str）

        Returns:
            解析后的 chunk 列表
        """
        if isinstance(chunk, bytes):
            text = chunk.decode("utf-8")
        else:
            text = chunk

        chunks: list[dict[str, Any]] = []
        lines = text.strip().split("\n")

        for line in lines:
            line = line.strip()
            if not line:
                continue

            # 解析数据行
            if line.startswith("data: "):
                data_str = line[6:]

                # 处理 [DONE] 标记
                if data_str == "[DONE]":
                    chunks.append({"__done__": True})
                    continue

                try:
                    data = json.loads(data_str)
                    chunks.append(data)
                except json.JSONDecodeError:
                    # 无法解析的数据，跳过
                    pass

        return chunks

    def parse_line(self, line: str) -> dict[str, Any] | None:
        """
        解析单行 SSE 数据

        Args:
            line: SSE 数据行（已去除 "data: " 前缀）

        Returns:
            解析后的 chunk 字典，如果无法解析返回 None
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

    def is_done_chunk(self, chunk: dict[str, Any]) -> bool:
        """
        判断是否为结束 chunk

        Args:
            chunk: chunk 字典

        Returns:
            True 如果是结束 chunk
        """
        # 内部标记
        if chunk.get("__done__"):
            return True

        # 检查 finish_reason
        choices = chunk.get("choices", [])
        if choices:
            finish_reason = choices[0].get("finish_reason")
            return finish_reason is not None

        return False

    def get_finish_reason(self, chunk: dict[str, Any]) -> str | None:
        """
        获取结束原因

        Args:
            chunk: chunk 字典

        Returns:
            结束原因字符串
        """
        choices = chunk.get("choices", [])
        if choices:
            reason = choices[0].get("finish_reason")
            return str(reason) if reason is not None else None
        return None

    def extract_text_delta(self, chunk: dict[str, Any]) -> str | None:
        """
        从 chunk 中提取文本增量

        Args:
            chunk: chunk 字典

        Returns:
            文本增量，如果没有返回 None
        """
        choices = chunk.get("choices", [])
        if not choices:
            return None

        delta = choices[0].get("delta", {})
        content = delta.get("content")

        if isinstance(content, str):
            return content

        return None

    def extract_tool_calls_delta(self, chunk: dict[str, Any]) -> list[dict[str, Any]] | None:
        """
        从 chunk 中提取工具调用增量

        Args:
            chunk: chunk 字典

        Returns:
            工具调用列表，如果没有返回 None
        """
        choices = chunk.get("choices", [])
        if not choices:
            return None

        delta = choices[0].get("delta", {})
        tool_calls = delta.get("tool_calls")
        if isinstance(tool_calls, list):
            return tool_calls
        return None

    def extract_role(self, chunk: dict[str, Any]) -> str | None:
        """
        从 chunk 中提取角色

        通常只在第一个 chunk 中出现。

        Args:
            chunk: chunk 字典

        Returns:
            角色字符串
        """
        choices = chunk.get("choices", [])
        if not choices:
            return None

        delta = choices[0].get("delta", {})
        role = delta.get("role")
        return str(role) if role is not None else None


__all__ = ["OpenAIStreamParser"]
