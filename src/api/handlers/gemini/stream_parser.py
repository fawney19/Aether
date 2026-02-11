"""
Gemini 流解析器（SSE + JSON-array 兼容）

Gemini streamGenerateContent 常见两种返回（与上游/代理实现有关）：
1) `?alt=sse`：SSE（`data: {GenerateContentResponse}`）
2) 默认：JSON-array / JSON-chunks（`[{...},{...},...]`，可能跨 chunk/跨行）

本解析器提供：
- parse_line(): 适用于 SSE data 行或逐行 JSON 对象
- parse_chunk(): 适用于 JSON-array/chunks（可跨 chunk 拼接）

参考:
- https://ai.google.dev/gemini-api/docs/text-generation?lang=python#generate-a-text-stream
- https://generativelanguage.googleapis.com/$discovery/rest?version=v1beta
"""

from typing import Any

from src.utils import json_helper as json


class GeminiStreamParser:
    """
    Gemini 流解析器

    解析 Gemini streamGenerateContent API 的响应流。

    Gemini 流式响应特点:
    - 每个事件块本质上都是一个 GenerateContentResponse JSON 对象（包含 candidates、usageMetadata 等）
    - 结束判定以 `candidates[].finishReason` 为准（存在且不为 FINISH_REASON_UNSPECIFIED）
    """

    # finishReason（官方枚举值很多，见 discovery；这里仅保留一个明确的“未结束”哨兵）
    FINISH_REASON_UNSPECIFIED = "FINISH_REASON_UNSPECIFIED"

    def __init__(self) -> None:
        self._buffer = ""
        self._in_array = False
        self._brace_depth = 0

    def reset(self) -> None:
        """重置解析器状态"""
        self._buffer = ""
        self._in_array = False
        self._brace_depth = 0

    def parse_chunk(self, chunk: bytes | str) -> list[dict[str, Any]]:
        """
        解析流式数据块

        Args:
            chunk: 原始数据（bytes 或 str）

        Returns:
            解析后的事件列表
        """
        if isinstance(chunk, bytes):
            text = chunk.decode("utf-8")
        else:
            text = chunk

        events: list[dict[str, Any]] = []

        for char in text:
            if char == "[" and not self._in_array:
                self._in_array = True
                continue

            if char == "]" and self._in_array and self._brace_depth == 0:
                # 数组结束
                self._in_array = False
                if self._buffer.strip():
                    try:
                        obj = json.loads(self._buffer.strip().rstrip(","))
                        events.append(obj)
                    except json.JSONDecodeError:
                        pass
                self._buffer = ""
                continue

            if self._in_array:
                if char == "{":
                    self._brace_depth += 1
                elif char == "}":
                    self._brace_depth -= 1

                self._buffer += char

                # 当 brace_depth 回到 0 时，说明一个完整的 JSON 对象结束
                if self._brace_depth == 0 and self._buffer.strip():
                    try:
                        obj = json.loads(self._buffer.strip().rstrip(","))
                        events.append(obj)
                        self._buffer = ""
                    except json.JSONDecodeError:
                        # 可能还不完整，继续累积
                        pass

        return events

    def parse_line(self, line: str) -> dict[str, Any] | None:
        """
        解析单行 JSON 数据

        Args:
            line: JSON 数据行

        Returns:
            解析后的事件字典，如果无法解析返回 None
        """
        if not line or line.strip() in ["[", "]", ","]:
            return None

        try:
            result = json.loads(line.strip().rstrip(","))
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
        candidates = event.get("candidates", [])
        if not candidates:
            return False

        for candidate in candidates:
            finish_reason = candidate.get("finishReason")
            if not finish_reason:
                continue
            # 只要出现非 UNSPECIFIED 的 finishReason，通常表示该 candidate 已结束。
            # 例如：STOP/MAX_TOKENS/SAFETY/RECITATION/MALFORMED_FUNCTION_CALL/...（枚举持续演进）
            if str(finish_reason) != self.FINISH_REASON_UNSPECIFIED:
                return True

        return False

    def is_error_event(self, event: dict[str, Any]) -> bool:
        """
        判断是否为错误事件

        检测多种 Gemini 错误格式:
        1. 顶层 error: {"error": {...}}
        2. chunks 内嵌套 error: {"chunks": [{"error": {...}}]}
        3. candidates 内的错误状态

        Args:
            event: 事件字典

        Returns:
            True 如果是错误事件
        """
        # 顶层 error
        if "error" in event:
            return True

        # chunks 内嵌套 error (某些 Gemini 响应格式)
        chunks = event.get("chunks", [])
        if chunks and isinstance(chunks, list):
            for chunk in chunks:
                if isinstance(chunk, dict) and "error" in chunk:
                    return True

        return False

    def extract_error_info(self, event: dict[str, Any]) -> dict[str, Any] | None:
        """
        从事件中提取错误信息

        Args:
            event: 事件字典

        Returns:
            错误信息字典 {"code": int, "message": str, "status": str}，无错误返回 None
        """
        # 顶层 error
        if "error" in event:
            error = event["error"]
            if isinstance(error, dict):
                return {
                    "code": error.get("code"),
                    "message": error.get("message", str(error)),
                    "status": error.get("status"),
                }
            return {"code": None, "message": str(error), "status": None}

        # chunks 内嵌套 error
        chunks = event.get("chunks", [])
        if chunks and isinstance(chunks, list):
            for chunk in chunks:
                if isinstance(chunk, dict) and "error" in chunk:
                    error = chunk["error"]
                    if isinstance(error, dict):
                        return {
                            "code": error.get("code"),
                            "message": error.get("message", str(error)),
                            "status": error.get("status"),
                        }
                    return {"code": None, "message": str(error), "status": None}

        return None

    def get_finish_reason(self, event: dict[str, Any]) -> str | None:
        """
        获取结束原因

        Args:
            event: 事件字典

        Returns:
            结束原因字符串
        """
        candidates = event.get("candidates", [])
        if candidates:
            reason = candidates[0].get("finishReason")
            return str(reason) if reason is not None else None
        return None

    def extract_text_delta(self, event: dict[str, Any]) -> str | None:
        """
        从响应中提取文本内容

        Args:
            event: 事件字典

        Returns:
            文本内容，如果没有文本返回 None
        """
        candidates = event.get("candidates", [])
        if not candidates:
            return None

        content = candidates[0].get("content", {})
        parts = content.get("parts", [])

        text_parts = []
        for part in parts:
            if "text" in part:
                text_parts.append(part["text"])

        return "".join(text_parts) if text_parts else None

    def extract_usage(self, event: dict[str, Any]) -> dict[str, int] | None:
        """
        从事件中提取 token 使用量

        这是 Gemini token 提取的单一实现源，其他地方都应该调用此方法。

        Args:
            event: 事件字典（包含 usageMetadata）

        Returns:
            使用量字典，如果没有完整的使用量信息返回 None

        注意:
            - 只有当 totalTokenCount 存在时才提取（确保是完整的 usage 数据）
            - 输出 token = thoughtsTokenCount + candidatesTokenCount
        """
        usage_metadata = event.get("usageMetadata", {})
        if not usage_metadata or "totalTokenCount" not in usage_metadata:
            return None

        # 输出 token = thoughtsTokenCount + candidatesTokenCount
        thoughts_tokens = usage_metadata.get("thoughtsTokenCount", 0)
        candidates_tokens = usage_metadata.get("candidatesTokenCount", 0)
        output_tokens = thoughts_tokens + candidates_tokens

        return {
            "input_tokens": usage_metadata.get("promptTokenCount", 0),
            "output_tokens": output_tokens,
            "total_tokens": usage_metadata.get("totalTokenCount", 0),
            "cached_tokens": usage_metadata.get("cachedContentTokenCount", 0),
        }

    def extract_model_version(self, event: dict[str, Any]) -> str | None:
        """
        从响应中提取模型版本

        Args:
            event: 事件字典

        Returns:
            模型版本，如果没有返回 None
        """
        version = event.get("modelVersion")
        return str(version) if version is not None else None

    def extract_safety_ratings(self, event: dict[str, Any]) -> list[dict[str, Any]] | None:
        """
        从响应中提取安全评级

        Args:
            event: 事件字典

        Returns:
            安全评级列表，如果没有返回 None
        """
        candidates = event.get("candidates", [])
        if not candidates:
            return None

        ratings = candidates[0].get("safetyRatings")
        if isinstance(ratings, list):
            return ratings
        return None


__all__ = ["GeminiStreamParser"]
