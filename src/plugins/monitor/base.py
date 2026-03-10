"""
监控插件基类
定义监控和指标收集的接口
"""

from __future__ import annotations

from abc import abstractmethod
from datetime import datetime, timezone
from enum import Enum
from typing import Any

from src.plugins.common import BasePlugin


class MetricType(Enum):
    """指标类型"""

    COUNTER = "counter"  # 计数器（只增不减）
    GAUGE = "gauge"  # 仪表（可增可减）
    HISTOGRAM = "histogram"  # 直方图（分布）
    SUMMARY = "summary"  # 摘要（分位数）


class Metric:
    """指标数据"""

    def __init__(
        self,
        name: str,
        value: float,
        metric_type: MetricType,
        labels: dict[str, str] | None = None,
        timestamp: datetime | None = None,
        description: str | None = None,
    ):
        self.name = name
        self.value = value
        self.metric_type = metric_type
        self.labels = labels or {}
        self.timestamp = timestamp or datetime.now(timezone.utc)
        self.description = description


class MonitorPlugin(BasePlugin):
    """
    监控插件基类
    所有监控插件必须继承此类并实现相关方法
    """

    def __init__(self, name: str, config: dict[str, Any] | None = None):
        """
        初始化监控插件

        Args:
            name: 插件名称
            config: 配置字典
        """
        # 调用父类初始化，设置metadata
        super().__init__(name=name, config=config, description="Monitor Plugin", version="1.0.0")

        self.flush_interval = self.config.get("flush_interval", 60)
        self.batch_size = self.config.get("batch_size", 100)

    @abstractmethod
    async def record_metric(self, metric: Metric) -> None:
        """
        记录单个指标

        Args:
            metric: 指标数据
        """
        pass

    @abstractmethod
    async def record_batch(self, metrics: list[Metric]) -> None:
        """
        批量记录指标

        Args:
            metrics: 指标列表
        """
        pass

    @abstractmethod
    async def increment(
        self, name: str, value: float = 1, labels: dict[str, str] | None = None
    ) -> Any:
        """
        增加计数器

        Args:
            name: 指标名称
            value: 增加的值
            labels: 标签字典
        """
        pass

    @abstractmethod
    async def gauge(self, name: str, value: float, labels: dict[str, str] | None = None) -> Any:
        """
        设置仪表值

        Args:
            name: 指标名称
            value: 仪表值
            labels: 标签字典
        """
        pass

    @abstractmethod
    async def histogram(
        self,
        name: str,
        value: float,
        labels: dict[str, str] | None = None,
        buckets: list[float] | None = None,
    ) -> Any:
        """
        记录直方图数据

        Args:
            name: 指标名称
            value: 观测值
            labels: 标签字典
            buckets: 桶边界
        """
        pass

    @abstractmethod
    async def timing(self, name: str, duration: float, labels: dict[str, str] | None = None) -> Any:
        """
        记录时间指标

        Args:
            name: 指标名称
            duration: 持续时间（秒）
            labels: 标签字典
        """
        pass

    @abstractmethod
    async def flush(self) -> Any:
        """
        刷新缓冲的指标到后端
        """
        pass

    @abstractmethod
    async def get_stats(self) -> dict[str, Any]:
        """
        获取插件统计信息

        Returns:
            统计信息字典
        """
        pass

    def record_request(
        self,
        method: str,
        endpoint: str,
        status_code: int,
        duration: float,
        provider: str | None = None,
        model: str | None = None,
    ) -> Any:
        """
        记录API请求指标（便捷方法）

        Args:
            method: HTTP方法
            endpoint: 端点路径
            status_code: 状态码
            duration: 请求时长
            provider: 提供商名称
            model: 模型名称
        """
        labels = {
            "method": method,
            "endpoint": endpoint,
            "status": str(status_code),
            "status_class": f"{status_code // 100}xx",
        }

        if provider:
            labels["provider"] = provider
        if model:
            labels["model"] = model

        # 异步记录指标
        from src.utils.async_utils import safe_create_task

        # 请求计数
        safe_create_task(self.increment("http_requests_total", labels=labels))

        # 请求延迟
        safe_create_task(self.histogram("http_request_duration_seconds", duration, labels=labels))

        # 错误计数
        if status_code >= 400:
            safe_create_task(self.increment("http_errors_total", labels=labels))

    def record_token_usage(
        self,
        provider: str,
        model: str,  # noqa: ARG002 - 保留签名兼容性，不再用于 Prometheus 标签
        input_tokens: int,
        output_tokens: int,
        cost: float | None = None,
    ) -> Any:
        """
        记录Token使用指标（便捷方法）

        Args:
            provider: 提供商名称
            model: 模型名称（保留签名兼容性，不再作为 Prometheus 标签）
            input_tokens: 输入token数
            output_tokens: 输出token数
            cost: 费用
        """
        labels = {"provider": provider}

        from src.utils.async_utils import safe_create_task

        # Token计数
        safe_create_task(self.increment("tokens_input_total", input_tokens, labels=labels))
        safe_create_task(self.increment("tokens_output_total", output_tokens, labels=labels))
        safe_create_task(
            self.increment("tokens_total", input_tokens + output_tokens, labels=labels)
        )

        # 费用
        if cost is not None:
            safe_create_task(self.increment("usage_cost_total", cost, labels=labels))

    def configure(self, config: dict[str, Any]) -> Any:
        """
        配置插件

        Args:
            config: 配置字典
        """
        self.config.update(config)
        self.enabled = config.get("enabled", True)
        self.flush_interval = config.get("flush_interval", self.flush_interval)
        self.batch_size = config.get("batch_size", self.batch_size)

    def __repr__(self) -> None:
        return f"<{self.__class__.__name__}(name={self.name}, enabled={self.enabled})>"
