"""
响应解析器工厂

直接根据格式 ID 创建对应的 ResponseParser 实现，
不再经过 Protocol 抽象层。
"""

import re
from typing import Any

from src.api.handlers.base.response_parser import (
    ParsedChunk,
    ParsedResponse,
    ResponseParser,
    StreamStats,
)

# is_cli_format 权威定义在 core 层
from src.core.api_format import is_cli_format
from src.core.usage_tokens import extract_cache_creation_tokens, extract_cache_read_tokens


def _check_nested_error(response: dict[str, Any]) -> tuple[bool, dict[str, Any] | None]:
    """
    检查响应中是否存在嵌套错误（某些代理服务返回 HTTP 200 但在响应体中包含错误）

    检测格式:
    1. 顶层 error: {"error": {...}}
    2. 顶层 type=error: {"type": "error", ...}
    3. chunks 内嵌套 error: {"chunks": [{"error": {...}}]}

    Args:
        response: 响应字典

    Returns:
        (is_error, error_dict): 是否为错误，以及提取的错误信息
    """
    # 顶层 error
    if "error" in response:
        error = response["error"]
        if isinstance(error, dict):
            return True, error
        return True, {"message": str(error)}

    # 顶层 type=error
    if response.get("type") == "error":
        return True, response

    # chunks 内嵌套 error (某些代理返回这种格式)
    chunks = response.get("chunks", [])
    if chunks and isinstance(chunks, list):
        for chunk in chunks:
            if isinstance(chunk, dict):
                if "error" in chunk:
                    error = chunk["error"]
                    if isinstance(error, dict):
                        return True, error
                    return True, {"message": str(error)}
                if chunk.get("type") == "error":
                    return True, chunk

    return False, None


def _extract_embedded_status_code(error_info: dict[str, Any] | None) -> int | None:
    """
    从错误信息中提取嵌套的状态码

    支持多种格式:
    1. 直接的 code 字段: {"code": 400}
    2. status 字段: {"status": 400}
    3. 从 message 中正则提取: "Request failed with status code 400"
    4. 从 type 字段映射: "invalid_request_error" -> 400

    Args:
        error_info: 错误信息字典

    Returns:
        提取的状态码，如果无法提取则返回 None
    """
    if not error_info:
        return None

    # 1. 直接的 code 字段（Gemini 等）
    code = error_info.get("code")
    if isinstance(code, int) and 100 <= code < 600:
        return code
    if isinstance(code, str) and code.isdigit():
        code_int = int(code)
        if 100 <= code_int < 600:
            return code_int

    # 2. status 字段
    status = error_info.get("status")
    if isinstance(status, int) and 100 <= status < 600:
        return status
    if isinstance(status, str) and status.isdigit():
        status_int = int(status)
        if 100 <= status_int < 600:
            return status_int

    # 3. 从 message 中正则提取 (例如 "Request failed with status code 400")
    message = error_info.get("message", "")
    if message:
        # 匹配 "status code XXX" 或 "status XXX" 或 "HTTP XXX"
        match = re.search(r"(?:status\s*(?:code\s*)?|HTTP\s*)(\d{3})", message, re.IGNORECASE)
        if match:
            code_int = int(match.group(1))
            if 100 <= code_int < 600:
                return code_int

    # 4. 从 type 字段映射常见的错误类型
    error_type = error_info.get("type", "")
    type_to_status = {
        "invalid_request_error": 400,
        "authentication_error": 401,
        "permission_error": 403,
        "not_found_error": 404,
        "rate_limit_error": 429,
        "overloaded_error": 503,
        "api_error": 500,
        "internal_error": 500,
    }
    if error_type and error_type.lower() in type_to_status:
        return type_to_status[error_type.lower()]

    return None


class OpenAIResponseParser(ResponseParser):
    """OpenAI 格式响应解析器"""

    API_FORMAT = "openai:chat"

    def __init__(self) -> None:
        from src.api.handlers.openai.stream_parser import OpenAIStreamParser

        self._parser = OpenAIStreamParser()
        self.name = self.API_FORMAT
        self.api_format = self.API_FORMAT

    def parse_sse_line(self, line: str, stats: StreamStats) -> ParsedChunk | None:
        if not line or not line.strip():
            return None

        if line.startswith("data: "):
            data_str = line[6:]
        else:
            data_str = line

        parsed = self._parser.parse_line(data_str)
        if parsed is None:
            return None

        chunk = ParsedChunk(
            raw_line=line,
            event_type=None,
            data=parsed,
        )

        # 提取文本增量
        text_delta = self._parser.extract_text_delta(parsed)
        if text_delta:
            chunk.text_delta = text_delta
            stats.collected_text += text_delta

        # 检查是否结束
        if self._parser.is_done_chunk(parsed):
            chunk.is_done = True
            stats.has_completion = True

        # 提取 usage 信息（某些 OpenAI 兼容 API 如豆包会在最后一个 chunk 中发送 usage）
        # 这个 chunk 通常 choices 为空数组，但包含完整的 usage 信息
        # 使用取最大值策略确保正确统计
        usage = parsed.get("usage")
        if usage and isinstance(usage, dict):
            chunk.input_tokens = usage.get("prompt_tokens", 0)
            chunk.output_tokens = usage.get("completion_tokens", 0)

            # 取最大值更新 stats
            if chunk.input_tokens > stats.input_tokens:
                stats.input_tokens = chunk.input_tokens
            if chunk.output_tokens > stats.output_tokens:
                stats.output_tokens = chunk.output_tokens

        stats.chunk_count += 1
        stats.data_count += 1

        return chunk

    def parse_response(self, response: dict[str, Any], status_code: int) -> ParsedResponse:
        result = ParsedResponse(
            raw_response=response,
            status_code=status_code,
        )

        # 提取文本内容
        choices = response.get("choices", [])
        if choices:
            message = choices[0].get("message", {})
            content = message.get("content")
            if content:
                result.text_content = content

        result.response_id = response.get("id")

        # 提取 usage
        usage = response.get("usage") or {}
        result.input_tokens = usage.get("prompt_tokens", 0)
        result.output_tokens = usage.get("completion_tokens", 0)
        result.cache_read_tokens = extract_cache_read_tokens(usage)

        # 检查错误（支持嵌套错误格式）
        is_error, error_info = _check_nested_error(response)
        if is_error and error_info:
            result.is_error = True
            result.error_type = error_info.get("type")
            result.error_message = error_info.get("message")
            result.embedded_status_code = _extract_embedded_status_code(error_info)

        return result

    def extract_usage_from_response(self, response: dict[str, Any]) -> dict[str, int]:
        usage = response.get("usage") or {}
        return {
            "input_tokens": usage.get("prompt_tokens", 0),
            "output_tokens": usage.get("completion_tokens", 0),
            "cache_creation_tokens": 0,
            "cache_read_tokens": extract_cache_read_tokens(usage),
        }

    def extract_text_content(self, response: dict[str, Any]) -> str:
        choices = response.get("choices", [])
        if choices:
            message = choices[0].get("message", {})
            content = message.get("content")
            if isinstance(content, str):
                return content
        return ""

    def is_error_response(self, response: dict[str, Any]) -> bool:
        is_error, _ = _check_nested_error(response)
        return is_error


class OpenAICliResponseParser(OpenAIResponseParser):
    """OpenAI CLI / Responses API 格式响应解析器

    OpenAI Responses API 与 Chat Completions API 的关键差异：
    - Usage 字段: input_tokens/output_tokens（而非 prompt_tokens/completion_tokens）
    - 响应结构: output[].content[].text（而非 choices[].message.content）
    - 流式事件: response.completed 事件中 usage 嵌套在 response 对象内
    """

    API_FORMAT = "openai:cli"

    def __init__(self) -> None:
        super().__init__()
        self.name = self.API_FORMAT
        self.api_format = self.API_FORMAT

    def parse_response(self, response: dict[str, Any], status_code: int) -> ParsedResponse:
        result = ParsedResponse(
            raw_response=response,
            status_code=status_code,
        )

        # Responses API: 文本在 output[].content[].text 中
        result.text_content = self._extract_responses_api_text(response)
        result.response_id = response.get("id")

        # Responses API usage: input_tokens / output_tokens
        usage = self._extract_responses_api_usage(response)
        result.input_tokens = usage.get("input_tokens", 0)
        result.output_tokens = usage.get("output_tokens", 0)
        result.cache_creation_tokens = usage.get("cache_creation_tokens", 0)
        result.cache_read_tokens = usage.get("cache_read_tokens", 0)

        # 检查错误（支持嵌套错误格式）
        is_error, error_info = _check_nested_error(response)
        if is_error and error_info:
            result.is_error = True
            result.error_type = error_info.get("type")
            result.error_message = error_info.get("message")
            result.embedded_status_code = _extract_embedded_status_code(error_info)

        return result

    def extract_usage_from_response(self, response: dict[str, Any]) -> dict[str, int]:
        usage = self._extract_responses_api_usage(response)
        return usage

    def extract_text_content(self, response: dict[str, Any]) -> str:
        return self._extract_responses_api_text(response)

    @staticmethod
    def _extract_responses_api_usage(response: dict[str, Any]) -> dict[str, int]:
        """从 Responses API 响应或流式事件中提取 usage

        支持多种结构：
        1. 顶层 usage（非流式响应 / 部分转换后的响应）
        2. response.usage（流式 response.completed 事件）
        3. 兼容 Chat Completions 字段名（prompt_tokens/completion_tokens）
        """
        usage: dict[str, Any] = {}

        # 优先从顶层 usage 提取
        top_usage = response.get("usage")
        if isinstance(top_usage, dict):
            usage = top_usage
        else:
            # 流式事件: response.completed 中 usage 嵌套在 response 对象内
            resp_obj = response.get("response")
            if isinstance(resp_obj, dict):
                nested_usage = resp_obj.get("usage")
                if isinstance(nested_usage, dict):
                    usage = nested_usage

        if not usage:
            return {
                "input_tokens": 0,
                "output_tokens": 0,
                "cache_creation_tokens": 0,
                "cache_read_tokens": 0,
            }

        # Responses API 使用 input_tokens/output_tokens
        # 兼容 Chat Completions 的 prompt_tokens/completion_tokens（以防转换后的响应）
        input_tokens = usage.get("input_tokens") or usage.get("prompt_tokens") or 0
        output_tokens = usage.get("output_tokens") or usage.get("completion_tokens") or 0

        return {
            "input_tokens": int(input_tokens),
            "output_tokens": int(output_tokens),
            "cache_creation_tokens": extract_cache_creation_tokens(usage),
            "cache_read_tokens": extract_cache_read_tokens(usage),
        }

    @staticmethod
    def _extract_responses_api_text(response: dict[str, Any]) -> str:
        """从 Responses API 响应中提取文本内容

        支持结构: output[].content[].text 或 output[].text
        """
        text_parts: list[str] = []

        output = response.get("output")
        if isinstance(output, list):
            for item in output:
                if not isinstance(item, dict):
                    continue
                # message 类型: output[].content[].text
                if item.get("type") == "message":
                    content = item.get("content")
                    if isinstance(content, list):
                        for part in content:
                            if isinstance(part, dict):
                                ptype = str(part.get("type") or "")
                                if ptype in ("output_text", "text") and isinstance(
                                    part.get("text"), str
                                ):
                                    text_parts.append(part["text"])
                # 直接文本类型: output[].text
                elif item.get("type") in ("output_text", "text") and isinstance(
                    item.get("text"), str
                ):
                    text_parts.append(item["text"])

        # 兼容: 部分实现可能直接给 output_text
        if not text_parts and isinstance(response.get("output_text"), str):
            text_parts.append(response["output_text"])

        # 兼容: 如果是 Chat Completions 格式（可能来自转换后的响应），回退到 choices 结构
        if not text_parts:
            choices = response.get("choices", [])
            if isinstance(choices, list) and choices:
                message = choices[0].get("message", {}) if isinstance(choices[0], dict) else {}
                content = message.get("content") if isinstance(message, dict) else None
                if isinstance(content, str):
                    text_parts.append(content)

        return "".join(text_parts)


class ClaudeResponseParser(ResponseParser):
    """Claude 格式响应解析器"""

    API_FORMAT = "claude:chat"

    def __init__(self) -> None:
        from src.api.handlers.claude.stream_parser import ClaudeStreamParser

        self._parser = ClaudeStreamParser()
        self.name = self.API_FORMAT
        self.api_format = self.API_FORMAT

    def parse_sse_line(self, line: str, stats: StreamStats) -> ParsedChunk | None:
        if not line or not line.strip():
            return None

        if line.startswith("data: "):
            data_str = line[6:]
        else:
            data_str = line

        parsed = self._parser.parse_line(data_str)
        if parsed is None:
            return None

        chunk = ParsedChunk(
            raw_line=line,
            event_type=self._parser.get_event_type(parsed),
            data=parsed,
        )

        # 提取文本增量
        text_delta = self._parser.extract_text_delta(parsed)
        if text_delta:
            chunk.text_delta = text_delta
            stats.collected_text += text_delta

        # 检查是否结束
        if self._parser.is_done_event(parsed):
            chunk.is_done = True
            stats.has_completion = True

        # 提取 usage
        # Claude 流式响应的 usage 可能在首个 chunk（message_start）或最后一个 chunk（message_delta）中
        # 首个 chunk 通常包含 input_tokens，最后一个 chunk 包含 output_tokens
        # 使用取最大值策略确保正确统计
        usage = self._parser.extract_usage(parsed)
        if usage:
            chunk.input_tokens = usage.get("input_tokens", 0)
            chunk.output_tokens = usage.get("output_tokens", 0)
            chunk.cache_creation_tokens = usage.get("cache_creation_tokens", 0)
            chunk.cache_read_tokens = usage.get("cache_read_tokens", 0)

            # 取最大值更新 stats
            if chunk.input_tokens > stats.input_tokens:
                stats.input_tokens = chunk.input_tokens
            if chunk.output_tokens > stats.output_tokens:
                stats.output_tokens = chunk.output_tokens
            if chunk.cache_creation_tokens > stats.cache_creation_tokens:
                stats.cache_creation_tokens = chunk.cache_creation_tokens
            if chunk.cache_read_tokens > stats.cache_read_tokens:
                stats.cache_read_tokens = chunk.cache_read_tokens

        # 检查错误
        if self._parser.is_error_event(parsed):
            chunk.is_error = True
            error = parsed.get("error", {})
            if isinstance(error, dict):
                chunk.error_message = error.get("message", str(error))
            else:
                chunk.error_message = str(error)

        stats.chunk_count += 1
        stats.data_count += 1

        return chunk

    def parse_response(self, response: dict[str, Any], status_code: int) -> ParsedResponse:
        result = ParsedResponse(
            raw_response=response,
            status_code=status_code,
        )

        # 提取文本内容
        content = response.get("content", [])
        if isinstance(content, list):
            text_parts = []
            for block in content:
                if isinstance(block, dict) and block.get("type") == "text":
                    text_parts.append(block.get("text", ""))
            result.text_content = "".join(text_parts)

        result.response_id = response.get("id")

        # 提取 usage
        usage = response.get("usage") or {}
        result.input_tokens = usage.get("input_tokens", 0)
        result.output_tokens = usage.get("output_tokens", 0)
        result.cache_creation_tokens = extract_cache_creation_tokens(usage)
        result.cache_read_tokens = usage.get("cache_read_input_tokens", 0)

        # 检查错误（支持嵌套错误格式）
        is_error, error_info = _check_nested_error(response)
        if is_error and error_info:
            result.is_error = True
            result.error_type = error_info.get("type")
            result.error_message = error_info.get("message")
            result.embedded_status_code = _extract_embedded_status_code(error_info)

        return result

    def extract_usage_from_response(self, response: dict[str, Any]) -> dict[str, int]:
        # 对于 message_start 事件，usage 在 message.usage 路径下
        # 对于其他响应，usage 在顶层
        usage = response.get("usage") or {}
        if not usage and "message" in response:
            usage = (response.get("message") or {}).get("usage") or {}

        return {
            "input_tokens": usage.get("input_tokens", 0),
            "output_tokens": usage.get("output_tokens", 0),
            "cache_creation_tokens": extract_cache_creation_tokens(usage),
            "cache_read_tokens": usage.get("cache_read_input_tokens", 0),
        }

    def extract_text_content(self, response: dict[str, Any]) -> str:
        content = response.get("content", [])
        if isinstance(content, list):
            text_parts = []
            for block in content:
                if isinstance(block, dict) and block.get("type") == "text":
                    text_parts.append(block.get("text", ""))
            return "".join(text_parts)
        return ""

    def is_error_response(self, response: dict[str, Any]) -> bool:
        is_error, _ = _check_nested_error(response)
        return is_error


class GeminiResponseParser(ResponseParser):
    """Gemini 格式响应解析器"""

    API_FORMAT = "gemini:chat"

    def __init__(self) -> None:
        from src.api.handlers.gemini.stream_parser import GeminiStreamParser

        self._parser = GeminiStreamParser()
        self.name = self.API_FORMAT
        self.api_format = self.API_FORMAT

    def parse_sse_line(self, line: str, stats: StreamStats) -> ParsedChunk | None:
        """
        解析 Gemini SSE 行

        Gemini 的流式响应使用 SSE 格式 (data: {...})
        """
        if not line or not line.strip():
            return None

        # Gemini SSE 格式: data: {...}
        if line.startswith("data: "):
            data_str = line[6:]
        else:
            data_str = line

        parsed = self._parser.parse_line(data_str)
        if parsed is None:
            return None

        chunk = ParsedChunk(
            raw_line=line,
            event_type="content",
            data=parsed,
        )

        # 提取文本增量
        text_delta = self._parser.extract_text_delta(parsed)
        if text_delta:
            chunk.text_delta = text_delta
            stats.collected_text += text_delta

        # 检查是否结束
        if self._parser.is_done_event(parsed):
            chunk.is_done = True
            stats.has_completion = True

        # 提取 usage
        # Gemini 流式响应的 usage 可能出现在多个 chunk 中
        # 使用取最大值策略确保正确统计
        usage = self._parser.extract_usage(parsed)
        if usage:
            chunk.input_tokens = usage.get("input_tokens", 0)
            chunk.output_tokens = usage.get("output_tokens", 0)
            chunk.cache_read_tokens = usage.get("cached_tokens", 0)

            # 取最大值更新 stats
            if chunk.input_tokens > stats.input_tokens:
                stats.input_tokens = chunk.input_tokens
            if chunk.output_tokens > stats.output_tokens:
                stats.output_tokens = chunk.output_tokens
            if chunk.cache_read_tokens > stats.cache_read_tokens:
                stats.cache_read_tokens = chunk.cache_read_tokens

        # 检查错误
        if self._parser.is_error_event(parsed):
            chunk.is_error = True
            error = parsed.get("error", {})
            if isinstance(error, dict):
                chunk.error_message = error.get("message", str(error))
            else:
                chunk.error_message = str(error)

        stats.chunk_count += 1
        stats.data_count += 1

        return chunk

    def parse_response(self, response: dict[str, Any], status_code: int) -> ParsedResponse:
        result = ParsedResponse(
            raw_response=response,
            status_code=status_code,
        )

        # 提取文本内容
        candidates = response.get("candidates", [])
        if candidates:
            content = candidates[0].get("content", {})
            parts = content.get("parts", [])
            text_parts = []
            for part in parts:
                if "text" in part:
                    text_parts.append(part["text"])
            result.text_content = "".join(text_parts)

        result.response_id = response.get("modelVersion")

        # 提取 usage（调用 GeminiStreamParser.extract_usage 作为单一实现源）
        usage = self._parser.extract_usage(response)
        if usage:
            result.input_tokens = usage.get("input_tokens", 0)
            result.output_tokens = usage.get("output_tokens", 0)
            result.cache_read_tokens = usage.get("cached_tokens", 0)

        # 检查错误（使用增强的错误检测）
        error_info = self._parser.extract_error_info(response)
        if error_info:
            result.is_error = True
            result.error_type = error_info.get("status")
            result.error_message = error_info.get("message")
            result.embedded_status_code = _extract_embedded_status_code(error_info)

        return result

    def extract_usage_from_response(self, response: dict[str, Any]) -> dict[str, int]:
        """
        从 Gemini 响应中提取 token 使用量

        调用 GeminiStreamParser.extract_usage 作为单一实现源
        """
        usage = self._parser.extract_usage(response)
        if not usage:
            return {
                "input_tokens": 0,
                "output_tokens": 0,
                "cache_creation_tokens": 0,
                "cache_read_tokens": 0,
            }

        return {
            "input_tokens": usage.get("input_tokens", 0),
            "output_tokens": usage.get("output_tokens", 0),
            "cache_creation_tokens": 0,
            "cache_read_tokens": usage.get("cached_tokens", 0),
        }

    def extract_text_content(self, response: dict[str, Any]) -> str:
        candidates = response.get("candidates", [])
        if candidates:
            content = candidates[0].get("content", {})
            parts = content.get("parts", [])
            text_parts = []
            for part in parts:
                if "text" in part:
                    text_parts.append(part["text"])
            return "".join(text_parts)
        return ""

    def is_error_response(self, response: dict[str, Any]) -> bool:
        """
        判断响应是否为错误响应

        使用增强的错误检测逻辑，支持嵌套在 chunks 中的错误
        """
        return bool(self._parser.is_error_event(response))


# 注册解析器到 core 层注册表（供 services 层通过 format_id 获取）
from src.core.stream_types import get_parser_for_format, register_parser


def register_default_parsers() -> None:
    """自动发现所有 ResponseParser 子类并注册

    通过 __subclasses__() 递归收集所有 ResponseParser 子类，
    使用类级别 API_FORMAT 属性获取格式 ID，无需实例化。
    """

    def _collect_subclasses(base: type) -> list[type]:
        subs = base.__subclasses__()
        return subs + [s for c in subs for s in _collect_subclasses(c)]

    for cls in _collect_subclasses(ResponseParser):
        api_format = getattr(cls, "API_FORMAT", None)
        if api_format:
            register_parser(api_format, cls)


# 模块加载时自动注册（保证 import parsers 即可用，测试也不需要手动初始化）
# main.py lifespan 中的显式调用是冗余但无害的安全保障（dict 覆盖幂等）
register_default_parsers()


__all__ = [
    "OpenAIResponseParser",
    "OpenAICliResponseParser",
    "ClaudeResponseParser",
    "GeminiResponseParser",
    "register_default_parsers",
    "get_parser_for_format",
    "is_cli_format",
]
