from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from sqlalchemy.orm import Session

from src.models.database import ApiKey, User


@dataclass
class UsageRecordParams:
    """用量记录参数数据类，用于在内部方法间传递数据"""

    db: Session
    user: User | None
    api_key: ApiKey | None
    provider: str
    model: str
    input_tokens: int
    output_tokens: int
    cache_creation_input_tokens: int
    cache_read_input_tokens: int
    request_type: str
    api_format: str | None
    api_family: str | None  # 协议族（从 Adapter 层透传）
    endpoint_kind: str | None  # 端点类型（从 Adapter 层透传）
    endpoint_api_format: str | None  # 端点原生 API 格式
    has_format_conversion: bool  # 是否发生了格式转换
    is_stream: bool
    response_time_ms: int | None
    first_byte_time_ms: int | None
    status_code: int
    error_message: str | None
    metadata: dict[str, Any] | None
    request_headers: dict[str, Any] | None
    request_body: Any | None
    provider_request_headers: dict[str, Any] | None
    provider_request_body: Any | None
    response_headers: dict[str, Any] | None
    client_response_headers: dict[str, Any] | None
    response_body: Any | None
    client_response_body: Any | None
    request_id: str
    provider_id: str | None
    provider_endpoint_id: str | None
    provider_api_key_id: str | None
    status: str
    cache_ttl_minutes: int | None
    use_tiered_pricing: bool
    target_model: str | None
    cache_creation_input_tokens_5m: int = 0
    cache_creation_input_tokens_1h: int = 0

    def __post_init__(self) -> None:
        """验证关键字段，确保数据完整性"""
        # Token 数量不能为负数
        if self.input_tokens < 0:
            raise ValueError(f"input_tokens 不能为负数: {self.input_tokens}")
        if self.output_tokens < 0:
            raise ValueError(f"output_tokens 不能为负数: {self.output_tokens}")
        if self.cache_creation_input_tokens < 0:
            raise ValueError(
                f"cache_creation_input_tokens 不能为负数: {self.cache_creation_input_tokens}"
            )
        if self.cache_read_input_tokens < 0:
            raise ValueError(f"cache_read_input_tokens 不能为负数: {self.cache_read_input_tokens}")

        # 响应时间不能为负数
        if self.response_time_ms is not None and self.response_time_ms < 0:
            raise ValueError(f"response_time_ms 不能为负数: {self.response_time_ms}")
        if self.first_byte_time_ms is not None and self.first_byte_time_ms < 0:
            raise ValueError(f"first_byte_time_ms 不能为负数: {self.first_byte_time_ms}")

        # HTTP 状态码范围校验
        if not (100 <= self.status_code <= 599):
            raise ValueError(f"无效的 HTTP 状态码: {self.status_code}")

        # 状态值校验
        # - pending: 请求已创建，等待处理
        # - streaming: 流式响应进行中
        # - completed: 请求成功完成
        # - failed: 请求失败（上游错误、超时等）
        # - cancelled: 客户端主动断开连接
        valid_statuses = {"pending", "streaming", "completed", "failed", "cancelled"}
        if self.status not in valid_statuses:
            raise ValueError(f"无效的状态值: {self.status}，有效值: {valid_statuses}")


@dataclass
class UsageCostInfo:
    """成本与价格信息，用于 _build_usage_params 参数封装"""

    # 成本计算结果
    input_cost: float = 0.0
    output_cost: float = 0.0
    cache_creation_cost: float = 0.0
    cache_read_cost: float = 0.0
    cache_cost: float = 0.0
    request_cost: float = 0.0
    total_cost: float = 0.0
    cache_creation_cost_5m: float = 0.0
    cache_creation_cost_1h: float = 0.0
    # 价格信息
    input_price: float | None = None
    output_price: float | None = None
    cache_creation_price: float | None = None
    cache_creation_price_5m: float | None = None
    cache_creation_price_1h: float | None = None
    cache_read_price: float | None = None
    request_price: float | None = None
    # 倍率
    actual_rate_multiplier: float = 1.0
    is_free_tier: bool = False
