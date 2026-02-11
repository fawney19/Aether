"""
流式内容提取器 - 策略模式实现

为不同 API 格式（OpenAI、Claude、Gemini）提供内容提取和 chunk 构造的抽象。
StreamSmoother 使用这些提取器来处理不同格式的 SSE 事件。
"""

import copy
from abc import ABC, abstractmethod

from src.utils import json_helper as json


class ContentExtractor(ABC):
    """
    流式内容提取器抽象基类

    定义从 SSE 事件中提取文本内容和构造新 chunk 的接口。
    每种 API 格式（OpenAI、Claude、Gemini）需要实现自己的提取器。
    """

    @abstractmethod
    def extract_content(self, data: dict) -> str | None:
        """
        从 SSE 数据中提取可拆分的文本内容

        Args:
            data: 解析后的 JSON 数据

        Returns:
            提取的文本内容，如果无法提取则返回 None
        """
        pass

    @abstractmethod
    def create_chunk(
        self,
        original_data: dict,
        new_content: str,
        event_type: str = "",
        is_first: bool = False,
    ) -> bytes:
        """
        使用新内容构造 SSE chunk

        Args:
            original_data: 原始 JSON 数据
            new_content: 新的文本内容
            event_type: SSE 事件类型（某些格式需要）
            is_first: 是否是第一个 chunk（用于保留 role 等字段）

        Returns:
            编码后的 SSE 字节数据
        """
        pass


class OpenAIContentExtractor(ContentExtractor):
    """
    OpenAI 格式内容提取器

    处理 OpenAI Chat Completions API 的流式响应格式：
    - 数据结构: choices[0].delta.content
    - 只在 delta 仅包含 role/content 时允许拆分，避免破坏 tool_calls 等结构
    """

    def extract_content(self, data: dict) -> str | None:
        if not isinstance(data, dict):
            return None

        choices = data.get("choices")
        if not isinstance(choices, list) or len(choices) != 1:
            return None

        first_choice = choices[0]
        if not isinstance(first_choice, dict):
            return None

        delta = first_choice.get("delta")
        if not isinstance(delta, dict):
            return None

        content = delta.get("content")
        if not isinstance(content, str):
            return None

        # 只有 delta 仅包含 role/content 时才允许拆分
        # 避免破坏 tool_calls、function_call 等复杂结构
        allowed_keys = {"role", "content"}
        if not all(key in allowed_keys for key in delta.keys()):
            return None

        return content

    def create_chunk(
        self,
        original_data: dict,
        new_content: str,
        event_type: str = "",
        is_first: bool = False,
    ) -> bytes:
        new_data = original_data.copy()

        if "choices" in new_data and new_data["choices"]:
            new_choices = []
            for choice in new_data["choices"]:
                new_choice = choice.copy()
                if "delta" in new_choice:
                    new_delta = {}
                    # 只有第一个 chunk 保留 role
                    if is_first and "role" in new_choice["delta"]:
                        new_delta["role"] = new_choice["delta"]["role"]
                    new_delta["content"] = new_content
                    new_choice["delta"] = new_delta
                new_choices.append(new_choice)
            new_data["choices"] = new_choices

        return f"data: {json.dumps(new_data, ensure_ascii=False)}\n\n".encode()


class ClaudeContentExtractor(ContentExtractor):
    """
    Claude 格式内容提取器

    处理 Claude Messages API 的流式响应格式：
    - 事件类型: content_block_delta
    - 数据结构: delta.type=text_delta, delta.text
    """

    def extract_content(self, data: dict) -> str | None:
        if not isinstance(data, dict):
            return None

        # 检查事件类型
        if data.get("type") != "content_block_delta":
            return None

        delta = data.get("delta", {})
        if not isinstance(delta, dict):
            return None

        # 检查 delta 类型
        if delta.get("type") != "text_delta":
            return None

        text = delta.get("text")
        if not isinstance(text, str):
            return None

        return text

    def create_chunk(
        self,
        original_data: dict,
        new_content: str,
        event_type: str = "",
        is_first: bool = False,
    ) -> bytes:
        new_data = original_data.copy()

        if "delta" in new_data:
            new_delta = new_data["delta"].copy()
            new_delta["text"] = new_content
            new_data["delta"] = new_delta

        # Claude 格式需要 event: 前缀
        event_name = event_type or "content_block_delta"
        return f"event: {event_name}\ndata: {json.dumps(new_data, ensure_ascii=False)}\n\n".encode()


class GeminiContentExtractor(ContentExtractor):
    """
    Gemini 格式内容提取器

    处理 Gemini API 的流式响应格式：
    - 数据结构: candidates[0].content.parts[0].text
    - 只有纯文本块才拆分
    """

    def extract_content(self, data: dict) -> str | None:
        if not isinstance(data, dict):
            return None

        candidates = data.get("candidates")
        if not isinstance(candidates, list) or len(candidates) != 1:
            return None

        first_candidate = candidates[0]
        if not isinstance(first_candidate, dict):
            return None

        content = first_candidate.get("content", {})
        if not isinstance(content, dict):
            return None

        parts = content.get("parts", [])
        if not isinstance(parts, list) or len(parts) != 1:
            return None

        first_part = parts[0]
        if not isinstance(first_part, dict):
            return None

        text = first_part.get("text")
        # 只有纯文本块（只有 text 字段）才拆分
        if not isinstance(text, str) or len(first_part) != 1:
            return None

        return text

    def create_chunk(
        self,
        original_data: dict,
        new_content: str,
        event_type: str = "",
        is_first: bool = False,
    ) -> bytes:
        new_data = copy.deepcopy(original_data)

        if "candidates" in new_data and new_data["candidates"]:
            first_candidate = new_data["candidates"][0]
            if "content" in first_candidate:
                content = first_candidate["content"]
                if "parts" in content and content["parts"]:
                    content["parts"][0]["text"] = new_content

        return f"data: {json.dumps(new_data, ensure_ascii=False)}\n\n".encode()


# 提取器注册表
_EXTRACTORS: dict[str, type[ContentExtractor]] = {
    "openai": OpenAIContentExtractor,
    "claude": ClaudeContentExtractor,
    "gemini": GeminiContentExtractor,
}


def get_extractor(format_name: str) -> ContentExtractor | None:
    """
    根据格式名获取对应的内容提取器实例

    Args:
        format_name: 格式名称（openai, claude, gemini）

    Returns:
        对应的提取器实例，如果格式不支持则返回 None
    """
    extractor_class = _EXTRACTORS.get(format_name.lower())
    if extractor_class:
        return extractor_class()
    return None


def register_extractor(format_name: str, extractor_class: type[ContentExtractor]) -> None:
    """
    注册新的内容提取器

    Args:
        format_name: 格式名称
        extractor_class: 提取器类
    """
    _EXTRACTORS[format_name.lower()] = extractor_class


def get_extractor_formats() -> list[str]:
    """
    获取所有已注册的格式名称列表

    Returns:
        格式名称列表
    """
    return list(_EXTRACTORS.keys())
