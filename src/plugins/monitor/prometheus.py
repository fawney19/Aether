"""
Prometheus监控插件
支持将指标导出到Prometheus
"""

from __future__ import annotations

import asyncio
from typing import Any

try:
    from prometheus_client import REGISTRY, Counter, Gauge, Histogram, Summary, generate_latest

    PROMETHEUS_AVAILABLE = True
except ImportError:
    # Prometheus client not installed, plugin will be disabled
    PROMETHEUS_AVAILABLE = False
    Counter = Gauge = Histogram = Summary = REGISTRY = generate_latest = None

from src.core.logger import logger

from .base import Metric, MetricType, MonitorPlugin


class PrometheusPlugin(MonitorPlugin):
    """
    Prometheus监控插件
    使用prometheus_client库导出指标
    """

    def __init__(self, name: str = "prometheus", config: dict[str, Any] | None = None):
        super().__init__(name, config)

        # Check if prometheus_client is available
        if not PROMETHEUS_AVAILABLE:
            self.enabled = False
            logger.warning("Prometheus client not installed, plugin disabled")
            return

        # 指标注册表
        self._metrics: dict[str, Any] = {}
        self._max_dynamic_metrics: int = 100  # 动态创建的指标数上限
        self._buffer: list[Metric] = []
        self._lock = asyncio.Lock()
        self._flush_task: asyncio.Task | None = None  # 跟踪后台任务

        # 预定义常用指标
        self._init_default_metrics()

        # 启动刷新任务
        self._start_flush_task()

    def _init_default_metrics(self) -> None:
        """初始化默认指标"""
        # HTTP请求指标
        http_label_names = ["method", "endpoint", "status", "status_class"]

        self._metrics["http_requests_total"] = Counter(
            "http_requests_total",
            "Total HTTP requests",
            http_label_names,
        )

        self._metrics["http_request_duration_seconds"] = Histogram(
            "http_request_duration_seconds",
            "HTTP request duration in seconds",
            http_label_names,
            buckets=(0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1, 2.5, 5, 10),
        )

        self._metrics["http_errors_total"] = Counter(
            "http_errors_total",
            "Total HTTP errors",
            http_label_names,
        )

        # Token使用指标（仅按 provider 维度聚合，避免 provider x model 笛卡尔积导致基数爆炸）
        self._metrics["tokens_input_total"] = Counter(
            "tokens_input_total", "Total input tokens", ["provider"]
        )

        self._metrics["tokens_output_total"] = Counter(
            "tokens_output_total", "Total output tokens", ["provider"]
        )

        self._metrics["tokens_total"] = Counter("tokens_total", "Total tokens", ["provider"])

        self._metrics["usage_cost_total"] = Counter(
            "usage_cost_total", "Total usage cost in USD", ["provider"]
        )

        # 系统指标
        self._metrics["active_connections"] = Gauge(
            "active_connections", "Number of active connections"
        )

        self._metrics["cache_hits_total"] = Counter(
            "cache_hits_total", "Total cache hits", ["cache_type"]
        )

        self._metrics["cache_misses_total"] = Counter(
            "cache_misses_total", "Total cache misses", ["cache_type"]
        )

        # 提供商健康指标
        self._metrics["provider_health"] = Gauge(
            "provider_health", "Provider health status (1=healthy, 0=unhealthy)", ["provider"]
        )

        self._metrics["provider_latency_seconds"] = Histogram(
            "provider_latency_seconds",
            "Provider response latency in seconds",
            ["provider"],
            buckets=(0.1, 0.25, 0.5, 1, 2.5, 5, 10, 30),
        )

    def _start_flush_task(self) -> None:
        """启动定期刷新任务"""

        async def flush_loop() -> Any:
            try:
                while self.enabled:
                    await asyncio.sleep(self.flush_interval)
                    if self.enabled:  # 再次检查，避免关闭时执行
                        await self.flush()
            except asyncio.CancelledError:
                # 任务被取消，正常关闭
                logger.debug("Prometheus flush task cancelled")
            except Exception as e:
                logger.error(f"Prometheus flush loop error: {e}")

        # 保存任务句柄以便后续取消
        try:
            loop = asyncio.get_running_loop()
            self._flush_task = loop.create_task(flush_loop())
        except RuntimeError:
            # 如果没有运行的事件循环，任务将在后续创建
            logger.warning("No event loop available for Prometheus flush task")

    def _get_or_create_metric(
        self, name: str, metric_type: MetricType, labels: list[str] | None = None
    ) -> Any | None:
        """获取或创建指标，超过上限后拒绝创建新指标"""
        if name not in self._metrics:
            if len(self._metrics) >= self._max_dynamic_metrics:
                logger.warning("Prometheus dynamic metrics limit reached, dropping: {}", name)
                return None
            labels = labels or []
            if metric_type == MetricType.COUNTER:
                self._metrics[name] = Counter(name, f"Auto-created counter {name}", labels)
            elif metric_type == MetricType.GAUGE:
                self._metrics[name] = Gauge(name, f"Auto-created gauge {name}", labels)
            elif metric_type == MetricType.HISTOGRAM:
                self._metrics[name] = Histogram(name, f"Auto-created histogram {name}", labels)
            elif metric_type == MetricType.SUMMARY:
                self._metrics[name] = Summary(name, f"Auto-created summary {name}", labels)

        return self._metrics.get(name)

    async def record_metric(self, metric: Metric) -> None:
        """记录单个指标"""
        async with self._lock:
            self._buffer.append(metric)

            # 如果缓冲区满，自动刷新
            if len(self._buffer) >= self.batch_size:
                await self.flush()

    async def record_batch(self, metrics: list[Metric]) -> None:
        """批量记录指标"""
        async with self._lock:
            self._buffer.extend(metrics)

            # 如果缓冲区满，自动刷新
            if len(self._buffer) >= self.batch_size:
                await self.flush()

    async def increment(
        self, name: str, value: float = 1, labels: dict[str, str] | None = None
    ) -> Any:
        """增加计数器"""
        try:
            if name in self._metrics:
                metric = self._metrics[name]
                if labels:
                    # 过滤掉不存在的标签
                    filtered_labels = {k: v for k, v in labels.items() if k in metric._labelnames}
                    metric.labels(**filtered_labels).inc(value)
                else:
                    metric.inc(value)
            else:
                # 创建新的计数器
                label_names = list(labels.keys()) if labels else []
                metric = self._get_or_create_metric(name, MetricType.COUNTER, label_names)
                if metric is None:
                    return
                if labels:
                    metric.labels(**labels).inc(value)
                else:
                    metric.inc(value)
        except Exception as e:
            # 记录错误但不中断
            logger.warning(f"Error recording metric {name}: {e}")

    async def gauge(self, name: str, value: float, labels: dict[str, str] | None = None) -> Any:
        """设置仪表值"""
        try:
            if name in self._metrics:
                metric = self._metrics[name]
                if labels:
                    filtered_labels = {k: v for k, v in labels.items() if k in metric._labelnames}
                    metric.labels(**filtered_labels).set(value)
                else:
                    metric.set(value)
            else:
                # 创建新的仪表
                label_names = list(labels.keys()) if labels else []
                metric = self._get_or_create_metric(name, MetricType.GAUGE, label_names)
                if metric is None:
                    return
                if labels:
                    metric.labels(**labels).set(value)
                else:
                    metric.set(value)
        except Exception as e:
            logger.warning(f"Error recording gauge {name}: {e}")

    async def histogram(
        self,
        name: str,
        value: float,
        labels: dict[str, str] | None = None,
        buckets: list[float] | None = None,
    ) -> Any:
        """记录直方图数据"""
        try:
            if name in self._metrics:
                metric = self._metrics[name]
                if labels:
                    filtered_labels = {k: v for k, v in labels.items() if k in metric._labelnames}
                    metric.labels(**filtered_labels).observe(value)
                else:
                    metric.observe(value)
            else:
                # 创建新的直方图
                label_names = list(labels.keys()) if labels else []
                if buckets:
                    if len(self._metrics) >= self._max_dynamic_metrics:
                        return
                    metric = Histogram(
                        name, f"Auto-created histogram {name}", label_names, buckets=buckets
                    )
                else:
                    metric = self._get_or_create_metric(name, MetricType.HISTOGRAM, label_names)
                if metric is None:
                    return
                self._metrics[name] = metric

                if labels:
                    metric.labels(**labels).observe(value)
                else:
                    metric.observe(value)
        except Exception as e:
            logger.warning(f"Error recording histogram {name}: {e}")

    async def timing(self, name: str, duration: float, labels: dict[str, str] | None = None) -> Any:
        """记录时间指标"""
        # 使用直方图记录时间
        await self.histogram(f"{name}_seconds", duration, labels)

    async def flush(self) -> Any:
        """刷新缓冲的指标到Prometheus"""
        async with self._lock:
            if not self._buffer:
                return

            # 处理缓冲区中的指标
            for metric in self._buffer:
                if metric.metric_type == MetricType.COUNTER:
                    await self.increment(metric.name, metric.value, metric.labels)
                elif metric.metric_type == MetricType.GAUGE:
                    await self.gauge(metric.name, metric.value, metric.labels)
                elif metric.metric_type == MetricType.HISTOGRAM:
                    await self.histogram(metric.name, metric.value, metric.labels)

            # 清空缓冲区
            self._buffer.clear()

    async def get_stats(self) -> dict[str, Any]:
        """获取插件统计信息"""
        return {
            "type": "prometheus",
            "metrics_count": len(self._metrics),
            "buffer_size": len(self._buffer),
            "flush_interval": self.flush_interval,
            "batch_size": self.batch_size,
        }

    def get_metrics(self) -> bytes:
        """
        获取Prometheus格式的指标数据

        Returns:
            Prometheus文本格式的指标
        """
        return generate_latest(REGISTRY)

    async def shutdown(self) -> Any:
        """
        关闭插件，取消后台任务

        这个方法应该在应用关闭时调用
        """
        # 禁用插件
        self.enabled = False

        # 取消并等待后台任务完成
        if self._flush_task and not self._flush_task.done():
            self._flush_task.cancel()
            try:
                await self._flush_task
            except asyncio.CancelledError:
                pass

        # 最后一次刷新缓冲区
        await self.flush()

        logger.info("Prometheus plugin shutdown complete")

    async def cleanup(self) -> Any:
        """
        清理资源（别名方法）
        """
        await self.shutdown()
