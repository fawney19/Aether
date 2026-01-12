"""
统一的请求结果和元数据结构

设计原则：
1. RequestMetadata: 描述请求执行的上下文（Provider、Endpoint、Key、API格式等）
2. RequestResult: 封装请求的完整结果（成功/失败、响应、元数据、费用等）
3. 确保 api_format 在整个链路中始终可用

使用场景：
- ProviderService 创建 RequestMetadata
- FallbackOrchestrator 在异常时补充 RequestMetadata
- ChatHandlerBase 使用 RequestResult 记录 Usage
- ChatAdapterBase 使用 RequestResult 处理异常响应
"""

from dataclasses import dataclass, field
from enum import Enum
from typing import Any, AsyncIterator, Dict, Optional


class RequestStatus(Enum):
    """请求状态"""

    SUCCESS = "success"
    FAILED = "failed"
    PARTIAL = "partial"  # 流式请求部分成功


@dataclass
class RequestMetadata:
    """
    请求元数据 - 描述请求执行的上下文

    必填字段：
    - api_format: API 格式，必须在请求开始时就确定
    - provider: Provider 名称
    - model: 模型名称

    可选字段：
    - provider_id, provider_endpoint_id, provider_api_key_id: Provider 追踪信息
    - provider_request_headers, provider_response_headers: 请求/响应头
    - attempt_id: 请求尝试 ID
    - original_model: 用户请求的原始模型名（映射前）
    """

    # 必填字段 - 在请求开始时就应该确定
    api_format: str
    provider: str = "unknown"
    model: str = "unknown"

    # Provider 追踪信息
    provider_id: Optional[str] = None
    provider_endpoint_id: Optional[str] = None
    provider_api_key_id: Optional[str] = None

    # 请求/响应头
    provider_request_headers: Dict[str, str] = field(default_factory=dict)
    provider_response_headers: Dict[str, str] = field(default_factory=dict)

    # 其他元数据
    attempt_id: Optional[str] = None
    original_model: Optional[str] = None  # 用户请求的原始模型名（用于价格计算）

    # Provider 响应元数据（存储 provider 返回的额外信息，如 Gemini 的 modelVersion）
    response_metadata: Dict[str, Any] = field(default_factory=dict)

    def with_provider_info(
        self,
        provider: str,
        provider_id: str,
        provider_endpoint_id: str,
        provider_api_key_id: str,
    ) -> "RequestMetadata":
        """返回包含 Provider 信息的新 RequestMetadata"""
        return RequestMetadata(
            api_format=self.api_format,
            provider=provider,
            model=self.model,
            provider_id=provider_id,
            provider_endpoint_id=provider_endpoint_id,
            provider_api_key_id=provider_api_key_id,
            provider_request_headers=self.provider_request_headers,
            provider_response_headers=self.provider_response_headers,
            attempt_id=self.attempt_id,
            original_model=self.original_model,
            response_metadata=self.response_metadata,
        )

    def with_response_headers(self, headers: Dict[str, str]) -> "RequestMetadata":
        """返回包含响应头的新 RequestMetadata"""
        return RequestMetadata(
            api_format=self.api_format,
            provider=self.provider,
            model=self.model,
            provider_id=self.provider_id,
            provider_endpoint_id=self.provider_endpoint_id,
            provider_api_key_id=self.provider_api_key_id,
            provider_request_headers=self.provider_request_headers,
            provider_response_headers=headers,
            attempt_id=self.attempt_id,
            original_model=self.original_model,
            response_metadata=self.response_metadata,
        )


@dataclass
class UsageInfo:
    """Token 使用量信息"""

    input_tokens: int = 0
    output_tokens: int = 0
    cache_creation_input_tokens: int = 0
    cache_read_input_tokens: int = 0


@dataclass
class CostInfo:
    """费用信息"""

    input_cost_usd: float = 0.0
    output_cost_usd: float = 0.0
    cache_creation_cost_usd: float = 0.0
    cache_read_cost_usd: float = 0.0
    cache_cost_usd: float = 0.0
    total_cost_usd: float = 0.0

    # 实际费用（乘以 rate_multiplier 后）
    actual_input_cost_usd: float = 0.0
    actual_output_cost_usd: float = 0.0
    actual_cache_creation_cost_usd: float = 0.0
    actual_cache_read_cost_usd: float = 0.0
    actual_total_cost_usd: float = 0.0


@dataclass
class RequestResult:
    """
    请求结果 - 封装请求的完整结果

    用于：
    - 成功请求：包含响应数据、使用量、费用
    - 失败请求：包含错误信息、状态码
    - 流式请求：包含流生成器和元数据
    """

    # 状态
    status: RequestStatus

    # 元数据（必须存在）
    metadata: RequestMetadata

    # 响应相关
    response_data: Optional[Any] = None  # 成功时的响应数据
    stream: Optional[AsyncIterator[str]] = None  # 流式响应

    # 使用量和费用
    usage: UsageInfo = field(default_factory=UsageInfo)
    cost: CostInfo = field(default_factory=CostInfo)

    # 错误信息
    status_code: int = 200
    error_message: Optional[str] = None
    error_type: Optional[str] = None

    # 计时
    response_time_ms: int = 0

    # 请求信息（用于记录）
    is_stream: bool = False
    request_headers: Dict[str, str] = field(default_factory=dict)
    request_body: Dict[str, Any] = field(default_factory=dict)

    @property
    def is_success(self) -> bool:
        return self.status == RequestStatus.SUCCESS

    @property
    def is_failed(self) -> bool:
        return self.status == RequestStatus.FAILED

    @classmethod
    def success(
        cls,
        metadata: RequestMetadata,
        response_data: Any,
        usage: UsageInfo,
        response_time_ms: int,
        is_stream: bool = False,
    ) -> "RequestResult":
        """创建成功的请求结果"""
        return cls(
            status=RequestStatus.SUCCESS,
            metadata=metadata,
            response_data=response_data,
            usage=usage,
            status_code=200,
            response_time_ms=response_time_ms,
            is_stream=is_stream,
        )

    @classmethod
    def failed(
        cls,
        metadata: RequestMetadata,
        status_code: int,
        error_message: str,
        error_type: str,
        response_time_ms: int,
        is_stream: bool = False,
    ) -> "RequestResult":
        """创建失败的请求结果"""
        return cls(
            status=RequestStatus.FAILED,
            metadata=metadata,
            status_code=status_code,
            error_message=error_message,
            error_type=error_type,
            response_time_ms=response_time_ms,
            is_stream=is_stream,
        )

    @classmethod
    def from_exception(
        cls,
        exception: Exception,
        api_format: str,
        model: str,
        response_time_ms: int,
        is_stream: bool = False,
    ) -> "RequestResult":
        """从异常创建失败的请求结果"""
        # 尝试从异常中提取 metadata
        existing_metadata = getattr(exception, "request_metadata", None)

        def get_meta_value(meta, key, default=None):
            """从 metadata 中提取值，支持字典和对象两种形式"""
            if meta is None:
                return default
            if isinstance(meta, dict):
                return meta.get(key, default)
            return getattr(meta, key, default)

        if existing_metadata:
            # 如果异常已有 metadata，使用它但确保 api_format 存在
            metadata = RequestMetadata(
                api_format=get_meta_value(existing_metadata, "api_format") or api_format,
                provider=get_meta_value(existing_metadata, "provider", "unknown") or "unknown",
                model=get_meta_value(existing_metadata, "model", model) or model,
                provider_id=get_meta_value(existing_metadata, "provider_id"),
                provider_endpoint_id=get_meta_value(existing_metadata, "provider_endpoint_id"),
                provider_api_key_id=get_meta_value(existing_metadata, "provider_api_key_id"),
                provider_request_headers=get_meta_value(existing_metadata, "provider_request_headers", {}),
                provider_response_headers=get_meta_value(
                    existing_metadata, "provider_response_headers", {}
                ),
                attempt_id=get_meta_value(existing_metadata, "attempt_id"),
                original_model=get_meta_value(existing_metadata, "original_model"),
                response_metadata=get_meta_value(existing_metadata, "response_metadata", {}),
            )
        else:
            # 创建最小的 metadata
            metadata = RequestMetadata(
                api_format=api_format,
                provider="unknown",
                model=model,
            )

        # 确定状态码和错误类型
        from src.core.exceptions import (
            ProviderAuthException,
            ProviderNotAvailableException,
            ProviderRateLimitException,
            ProviderTimeoutException,
        )

        if isinstance(exception, ProviderAuthException):
            status_code = 503
            error_type = "provider_auth_error"
        elif isinstance(exception, ProviderRateLimitException):
            status_code = 429
            error_type = "rate_limit_exceeded"
        elif isinstance(exception, ProviderTimeoutException):
            status_code = 504
            error_type = "timeout_error"
        elif isinstance(exception, ProviderNotAvailableException):
            status_code = 503
            error_type = "provider_unavailable"
        else:
            status_code = 500
            error_type = "internal_error"

        # 构建错误消息：优先使用友好的 message 属性
        # upstream_response 仅用于调试/链路追踪，不作为客户端错误消息
        error_message = getattr(exception, "message", None)
        if not error_message or not isinstance(error_message, str):
            error_message = str(exception)

        return cls(
            status=RequestStatus.FAILED,
            metadata=metadata,
            status_code=status_code,
            error_message=error_message,
            error_type=error_type,
            response_time_ms=response_time_ms,
            is_stream=is_stream,
        )


class StreamWithMetadata:
    """带元数据的流式响应包装器"""

    def __init__(
        self,
        stream: AsyncIterator[str],
        metadata: RequestMetadata,
        response_headers_container: Optional[Dict[str, Any]] = None,
    ):
        self.stream = stream
        self.metadata = metadata
        self.response_headers_container = response_headers_container
        self._metadata_updated = False

    def update_metadata_with_response_headers(self):
        """使用实际的响应头更新元数据"""
        if self.response_headers_container and "headers" in self.response_headers_container:
            if not self._metadata_updated:
                self.metadata = self.metadata.with_response_headers(
                    self.response_headers_container["headers"]
                )
                self._metadata_updated = True

    def __aiter__(self):
        return self.stream

    async def __anext__(self):
        return await self.stream.__anext__()
