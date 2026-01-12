"""
流式处理上下文 - 类型安全的数据类替代 dict

提供流式请求处理过程中的状态跟踪，包括：
- Provider/Endpoint/Key 信息
- Token 统计
- 响应状态
- 请求/响应数据
"""

import time
from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional


@dataclass
class StreamContext:
    """
    流式处理上下文

    用于在流式请求处理过程中跟踪状态，替代原有的 ctx dict。
    所有字段都有类型注解，提供更好的 IDE 支持和运行时类型安全。
    """

    # 请求基本信息
    model: str
    api_format: str

    # 请求标识信息（CLI handler 需要）
    request_id: str = ""
    user_id: int = 0
    api_key_id: int = 0

    # Provider 信息（在请求执行时填充）
    provider_name: Optional[str] = None
    provider_id: Optional[str] = None
    endpoint_id: Optional[str] = None
    key_id: Optional[str] = None
    attempt_id: Optional[str] = None
    attempt_synced: bool = False
    provider_api_format: Optional[str] = None  # Provider 的响应格式

    # 模型映射
    mapped_model: Optional[str] = None

    # Token 统计
    input_tokens: int = 0
    output_tokens: int = 0
    cached_tokens: int = 0
    cache_creation_tokens: int = 0

    # 响应内容
    _collected_text_parts: List[str] = field(default_factory=list, repr=False)
    response_id: Optional[str] = None
    final_usage: Optional[Dict[str, Any]] = None
    final_response: Optional[Dict[str, Any]] = None

    # 时间指标
    first_byte_time_ms: Optional[int] = None  # 首字时间 (TTFB - Time To First Byte)
    start_time: float = field(default_factory=time.time)

    # 响应状态
    status_code: int = 200
    error_message: Optional[str] = None  # 客户端友好的错误消息
    upstream_response: Optional[str] = None  # 原始 Provider 响应（用于请求链路追踪）
    has_completion: bool = False

    # 请求/响应数据
    response_headers: Dict[str, str] = field(default_factory=dict)  # 提供商响应头
    client_response_headers: Dict[str, str] = field(default_factory=dict)  # 返回给客户端的响应头
    provider_request_headers: Dict[str, str] = field(default_factory=dict)
    provider_request_body: Optional[Dict[str, Any]] = None

    # 格式转换信息（CLI handler 需要）
    client_api_format: str = ""

    # Provider 响应元数据（CLI handler 需要）
    response_metadata: Dict[str, Any] = field(default_factory=dict)

    # 流式处理统计
    data_count: int = 0
    chunk_count: int = 0
    parsed_chunks: List[Dict[str, Any]] = field(default_factory=list)

    def reset_for_retry(self) -> None:
        """
        重试时重置状态

        在故障转移重试时调用，清除之前的数据避免累积。
        保留 model 和 api_format，重置其他所有状态。
        """
        self.parsed_chunks = []
        self.chunk_count = 0
        self.data_count = 0
        self.has_completion = False
        self._collected_text_parts = []
        self.input_tokens = 0
        self.output_tokens = 0
        self.cached_tokens = 0
        self.cache_creation_tokens = 0
        self.error_message = None
        self.upstream_response = None
        self.status_code = 200
        self.first_byte_time_ms = None
        self.response_headers = {}
        self.client_response_headers = {}
        self.provider_request_headers = {}
        self.provider_request_body = None
        self.response_id = None
        self.final_usage = None
        self.final_response = None

    @property
    def collected_text(self) -> str:
        """已收集的文本内容（按需拼接，避免在流式过程中频繁做字符串拷贝）"""
        return "".join(self._collected_text_parts)

    def append_text(self, text: str) -> None:
        """追加文本内容（仅在需要收集文本时调用）"""
        if text:
            self._collected_text_parts.append(text)

    def update_provider_info(
        self,
        provider_name: str,
        provider_id: str,
        endpoint_id: str,
        key_id: str,
        provider_api_format: Optional[str] = None,
    ) -> None:
        """更新 Provider 信息"""
        self.provider_name = provider_name
        self.provider_id = provider_id
        self.endpoint_id = endpoint_id
        self.key_id = key_id
        self.provider_api_format = provider_api_format

    def update_usage(
        self,
        input_tokens: Optional[int] = None,
        output_tokens: Optional[int] = None,
        cached_tokens: Optional[int] = None,
        cache_creation_tokens: Optional[int] = None,
    ) -> None:
        """
        更新 Token 使用统计

        采用防御性更新策略：只有当新值 > 0 或当前值为 0 时才更新，避免用 0 覆盖已有的正确值。

        设计原理：
        - 在流式响应中，某些事件可能不包含完整的 usage 信息（字段为 0 或不存在）
        - 后续事件可能会提供完整的统计数据
        - 通过这种策略，确保一旦获得非零值就保留它，不会被后续的 0 值覆盖

        示例场景：
        - message_start 事件：input_tokens=100, output_tokens=0
        - message_delta 事件：input_tokens=0, output_tokens=50
        - 最终结果：input_tokens=100, output_tokens=50

        注意事项：
        - 此策略假设初始值为 0 是正确的默认状态
        - 如果需要将已有值重置为 0，请直接修改实例属性（不使用此方法）

        Args:
            input_tokens: 输入 tokens 数量
            output_tokens: 输出 tokens 数量
            cached_tokens: 缓存命中 tokens 数量
            cache_creation_tokens: 缓存创建 tokens 数量
        """
        if input_tokens is not None and (input_tokens > 0 or self.input_tokens == 0):
            self.input_tokens = input_tokens
        if output_tokens is not None and (output_tokens > 0 or self.output_tokens == 0):
            self.output_tokens = output_tokens
        if cached_tokens is not None and (cached_tokens > 0 or self.cached_tokens == 0):
            self.cached_tokens = cached_tokens
        if cache_creation_tokens is not None and (
            cache_creation_tokens > 0 or self.cache_creation_tokens == 0
        ):
            self.cache_creation_tokens = cache_creation_tokens

    def mark_failed(
        self,
        status_code: int,
        error_message: str,
        upstream_response: Optional[str] = None,
    ) -> None:
        """
        标记请求失败

        Args:
            status_code: HTTP 状态码
            error_message: 客户端友好的错误消息
            upstream_response: 原始 Provider 响应（用于请求链路追踪）
        """
        self.status_code = status_code
        self.error_message = error_message
        if upstream_response:
            self.upstream_response = upstream_response

    def record_first_byte_time(self, start_time: float) -> None:
        """
        记录首字时间 (TTFB - Time To First Byte)

        应在第一次向客户端发送数据时调用。
        如果已记录过,则不会覆盖(避免重试时重复记录)。

        Args:
            start_time: 请求开始时间 (time.time())
        """
        if self.first_byte_time_ms is None:
            self.first_byte_time_ms = int((time.time() - start_time) * 1000)

    def is_success(self) -> bool:
        """检查请求是否成功"""
        return self.status_code < 400

    def build_response_body(self, response_time_ms: int) -> Dict[str, Any]:
        """
        构建响应体元数据

        用于记录到 Usage 表的 response_body 字段。
        """
        return {
            "chunks": self.parsed_chunks,
            "metadata": {
                "stream": True,
                "total_chunks": len(self.parsed_chunks),
                "data_count": self.data_count,
                "has_completion": self.has_completion,
                "response_time_ms": response_time_ms,
            },
        }

    def get_log_summary(self, request_id: str, response_time_ms: int) -> str:
        """
        获取日志摘要

        用于请求完成/失败时的日志输出。
        包含首字时间 (TTFB) 和总响应时间,分两行显示。
        """
        status = "OK" if self.is_success() else "FAIL"

        # 第一行:基本信息 + 首字时间
        line1 = (
            f"[{status}] {request_id[:8]} | {self.model} | "
            f"{self.provider_name or 'unknown'}"
        )
        if self.first_byte_time_ms is not None:
            line1 += f" | TTFB: {self.first_byte_time_ms}ms"

        # 第二行:总响应时间 + tokens
        line2 = (
            f"      Total: {response_time_ms}ms | "
            f"in:{self.input_tokens} out:{self.output_tokens}"
        )

        return f"{line1}\n{line2}"
