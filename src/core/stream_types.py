"""
响应解析器基类与流式统计类型。

从 api/handlers/base/response_parser.py 下沉到 core 层，
消除 services→api 的反向依赖。同时提供 parser 注册表，
允许 API 层注册具体实现，services 层通过 format_id 获取实例。
"""

from __future__ import annotations

from abc import ABC, abstractmethod
from dataclasses import dataclass, field
from typing import Any


@dataclass
class ParsedChunk:
    """解析后的流式数据块"""

    # 原始数据
    raw_line: str
    event_type: str | None = None
    data: dict[str, Any] | None = None

    # 提取的内容
    text_delta: str = ""
    is_done: bool = False
    is_error: bool = False
    error_message: str | None = None

    # 使用量信息（通常在最后一个 chunk 中）
    input_tokens: int = 0
    output_tokens: int = 0
    cache_creation_tokens: int = 0
    cache_read_tokens: int = 0
    cache_ttl_minutes: int | None = None

    # 响应 ID
    response_id: str | None = None


@dataclass
class StreamStats:
    """流式响应统计信息"""

    # 计数
    chunk_count: int = 0
    data_count: int = 0

    # Token 使用量
    input_tokens: int = 0
    output_tokens: int = 0
    cache_creation_tokens: int = 0
    cache_read_tokens: int = 0
    cache_ttl_minutes: int | None = None

    # 内容
    collected_text: str = ""
    response_id: str | None = None

    # 状态
    has_completion: bool = False
    status_code: int = 200
    error_message: str | None = None

    # Provider 信息
    provider_name: str | None = None
    endpoint_id: str | None = None
    key_id: str | None = None

    # 响应头和完整响应
    response_headers: dict[str, str] = field(default_factory=dict)
    final_response: dict[str, Any] | None = None


@dataclass
class ParsedResponse:
    """解析后的非流式响应"""

    # 原始响应
    raw_response: dict[str, Any]
    status_code: int

    # 提取的内容
    text_content: str = ""
    response_id: str | None = None

    # 使用量
    input_tokens: int = 0
    output_tokens: int = 0
    cache_creation_tokens: int = 0
    cache_read_tokens: int = 0
    cache_ttl_minutes: int | None = None

    # 错误信息
    is_error: bool = False
    error_type: str | None = None
    error_message: str | None = None
    # 从响应体解析出的嵌套状态码（当 HTTP 200 但响应体含错误时使用）
    embedded_status_code: int | None = None


class ResponseParser(ABC):
    """
    响应解析器基类

    定义统一的接口来解析不同 API 格式的响应。
    子类需要实现具体的解析逻辑。
    """

    # 解析器名称（用于日志）
    name: str = "base"

    # 支持的 API 格式
    api_format: str = "UNKNOWN"

    @abstractmethod
    def parse_sse_line(self, line: str, stats: StreamStats) -> ParsedChunk | None:
        """
        解析单行 SSE 数据

        Args:
            line: SSE 行数据
            stats: 流统计对象（会被更新）

        Returns:
            解析后的数据块，如果行不包含有效数据则返回 None
        """
        pass

    @abstractmethod
    def parse_response(self, response: dict[str, Any], status_code: int) -> ParsedResponse:
        """
        解析非流式响应

        Args:
            response: 响应 JSON
            status_code: HTTP 状态码

        Returns:
            解析后的响应对象
        """
        pass

    @abstractmethod
    def extract_usage_from_response(self, response: dict[str, Any]) -> dict[str, int]:
        """
        从响应中提取 token 使用量

        Args:
            response: 响应 JSON

        Returns:
            包含 input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens 的字典
        """
        pass

    @abstractmethod
    def extract_text_content(self, response: dict[str, Any]) -> str:
        """
        从响应中提取文本内容

        Args:
            response: 响应 JSON

        Returns:
            提取的文本内容
        """
        pass

    def is_error_response(self, response: dict[str, Any]) -> bool:
        """
        判断响应是否为错误响应

        Args:
            response: 响应 JSON

        Returns:
            是否为错误响应
        """
        return "error" in response

    def create_stats(self) -> StreamStats:
        """创建新的流统计对象"""
        return StreamStats()


# ---------------------------------------------------------------------------
# Parser 注册表 -- API 层注册具体实现，services 层通过 format_id 获取
# ---------------------------------------------------------------------------

_PARSER_REGISTRY: dict[str, type[ResponseParser]] = {}


def register_parser(format_id: str, parser_class: type[ResponseParser]) -> None:
    """注册一个格式对应的 ResponseParser 实现"""
    from src.core.api_format.signature import normalize_signature_key

    normalized = normalize_signature_key(format_id)
    _PARSER_REGISTRY[normalized] = parser_class


def get_parser_for_format(format_id: str) -> ResponseParser:
    """
    根据格式 ID 获取 ResponseParser 实例

    Args:
        format_id: endpoint signature，如 "claude:chat", "openai:cli"

    Returns:
        ResponseParser 实例

    Raises:
        KeyError: 格式不存在
    """
    from src.core.api_format.signature import normalize_signature_key

    if not _PARSER_REGISTRY:
        raise KeyError(
            f"Parser registry is empty when looking up '{format_id}'. "
            "Ensure parsers are registered at startup (import src.api.handlers.base.parsers)."
        )

    normalized = normalize_signature_key(format_id)
    # 1. 精确匹配
    if normalized in _PARSER_REGISTRY:
        return _PARSER_REGISTRY[normalized]()
    # 2. data_format_id 回退：如 "claude:cli" (dfid="claude") -> ClaudeResponseParser (dfid="claude")
    from src.core.api_format.metadata import get_data_format_id_for_endpoint

    target_dfid = get_data_format_id_for_endpoint(normalized)
    if target_dfid:
        for reg_key, parser_cls in _PARSER_REGISTRY.items():
            reg_dfid = get_data_format_id_for_endpoint(reg_key)
            if reg_dfid == target_dfid:
                return parser_cls()
    raise KeyError(f"Unknown format: {normalized}")
