"""
通知插件基类
定义通知的接口和数据结构
"""

from __future__ import annotations

import asyncio
from abc import abstractmethod
from datetime import datetime, timezone
from enum import Enum
from typing import Any

from src.plugins.common import BasePlugin
from src.utils import json_helper as json


class NotificationLevel(Enum):
    """通知级别"""

    INFO = "info"
    WARNING = "warning"
    ERROR = "error"
    CRITICAL = "critical"


class Notification:
    """通知对象"""

    def __init__(
        self,
        title: str,
        message: str,
        level: NotificationLevel = NotificationLevel.INFO,
        notification_type: str | None = None,
        source: str | None = None,
        timestamp: datetime | None = None,
        metadata: dict[str, Any] | None = None,
        recipient: str | None = None,
        tags: list[str] | None = None,
    ):
        self.title = title
        self.message = message
        self.level = level
        self.notification_type = notification_type or "system"
        self.source = source or "aether"
        self.timestamp = timestamp or datetime.now(timezone.utc)
        self.metadata = metadata or {}
        self.recipient = recipient
        self.tags = tags or []

    def to_dict(self) -> dict[str, Any]:
        """转换为字典"""
        return {
            "title": self.title,
            "message": self.message,
            "level": self.level.value,
            "type": self.notification_type,
            "source": self.source,
            "timestamp": self.timestamp.isoformat(),
            "metadata": self.metadata,
            "recipient": self.recipient,
            "tags": self.tags,
        }

    def to_json(self) -> str:
        """转换为JSON"""
        return json.dumps(self.to_dict(), default=str)

    def format_message(self, template: str | None = None) -> str:
        """格式化消息"""
        if template:
            return template.format(
                title=self.title,
                message=self.message,
                level=self.level.value,
                type=self.notification_type,
                source=self.source,
                timestamp=self.timestamp.isoformat(),
                **self.metadata,
            )
        else:
            # 默认格式
            return f"[{self.level.value.upper()}] {self.title}\n{self.message}"


class NotificationPlugin(BasePlugin):
    """
    通知插件基类
    所有通知插件必须实现这个接口

    提供统一的重试机制，子类只需实现 _do_send 和 _do_send_batch 方法
    """

    def __init__(self, name: str = "notification", config: dict[str, Any] | None = None):
        # 调用父类初始化，设置metadata
        super().__init__(
            name=name, config=config, description="Notification Plugin", version="1.0.0"
        )

        self.min_level = NotificationLevel[self.config.get("min_level", "INFO").upper()]
        self.batch_size = self.config.get("batch_size", 10)
        self.flush_interval = self.config.get("flush_interval", 60)  # 秒
        self.retry_count = self.config.get("retry_count", 3)
        self.retry_delay = self.config.get("retry_delay", 5)  # 秒
        self.retry_backoff = self.config.get("retry_backoff", 2.0)  # 指数退避因子

        # 统计信息
        self._send_attempts = 0
        self._send_successes = 0
        self._send_failures = 0
        self._retry_total = 0

    async def send(self, notification: Notification) -> bool:
        """
        发送单个通知（带重试机制）

        Args:
            notification: 通知对象

        Returns:
            是否发送成功
        """
        if not self.should_send(notification):
            return False

        self._send_attempts += 1
        last_error = None

        for attempt in range(self.retry_count):
            try:
                result = await self._do_send(notification)
                if result:
                    self._send_successes += 1
                    return True
            except Exception as e:
                last_error = e

            # 如果不是最后一次尝试，等待后重试
            if attempt < self.retry_count - 1:
                self._retry_total += 1
                delay = self.retry_delay * (self.retry_backoff**attempt)
                await asyncio.sleep(delay)

        # 所有重试都失败
        self._send_failures += 1
        if last_error:
            # 可以在这里记录日志，但不抛出异常
            pass
        return False

    @abstractmethod
    async def _do_send(self, notification: Notification) -> bool:
        """
        实际发送单个通知（子类实现）

        Args:
            notification: 通知对象

        Returns:
            是否发送成功
        """
        pass

    async def send_batch(self, notifications: list[Notification]) -> dict[str, Any]:
        """
        批量发送通知（带重试机制）

        Args:
            notifications: 通知列表

        Returns:
            发送结果统计
        """
        # 过滤应该发送的通知
        to_send = [n for n in notifications if self.should_send(n)]

        if not to_send:
            return {"total": 0, "sent": 0, "failed": 0}

        self._send_attempts += len(to_send)
        last_error = None
        result = None

        for attempt in range(self.retry_count):
            try:
                result = await self._do_send_batch(to_send)
                if result and result.get("sent", 0) == len(to_send):
                    self._send_successes += result.get("sent", 0)
                    return result
                elif result:
                    # 部分成功
                    self._send_successes += result.get("sent", 0)
                    self._send_failures += result.get("failed", 0)
                    return result
            except Exception as e:
                last_error = e

            # 如果不是最后一次尝试，等待后重试
            if attempt < self.retry_count - 1:
                self._retry_total += 1
                delay = self.retry_delay * (self.retry_backoff**attempt)
                await asyncio.sleep(delay)

        # 所有重试都失败
        self._send_failures += len(to_send)
        return {
            "total": len(to_send),
            "sent": 0,
            "failed": len(to_send),
            "error": str(last_error) if last_error else "Unknown error",
        }

    @abstractmethod
    async def _do_send_batch(self, notifications: list[Notification]) -> dict[str, Any]:
        """
        实际批量发送通知（子类实现）

        Args:
            notifications: 通知列表

        Returns:
            发送结果统计 {"total": int, "sent": int, "failed": int}
        """
        pass

    def should_send(self, notification: Notification) -> bool:
        """判断是否应该发送通知"""
        if not self.enabled:
            return False

        # 级别过滤
        level_values = {level: i for i, level in enumerate(NotificationLevel)}
        if level_values[notification.level] < level_values[self.min_level]:
            return False

        return True

    async def send_error(
        self,
        error: Exception,
        context: dict[str, Any] | None = None,
        recipient: str | None = None,
    ) -> bool:
        """发送错误通知"""
        notification = Notification(
            title=f"Error: {type(error).__name__}",
            message=str(error),
            level=NotificationLevel.ERROR,
            notification_type="error",
            metadata=context or {},
            recipient=recipient,
            tags=["error", type(error).__name__],
        )
        return await self.send(notification)

    async def send_warning(
        self,
        title: str,
        message: str,
        context: dict[str, Any] | None = None,
        recipient: str | None = None,
    ) -> bool:
        """发送警告通知"""
        notification = Notification(
            title=title,
            message=message,
            level=NotificationLevel.WARNING,
            notification_type="warning",
            metadata=context or {},
            recipient=recipient,
            tags=["warning"],
        )
        return await self.send(notification)

    async def send_info(
        self,
        title: str,
        message: str,
        context: dict[str, Any] | None = None,
        recipient: str | None = None,
    ) -> bool:
        """发送信息通知"""
        notification = Notification(
            title=title,
            message=message,
            level=NotificationLevel.INFO,
            notification_type="info",
            metadata=context or {},
            recipient=recipient,
            tags=["info"],
        )
        return await self.send(notification)

    async def send_critical(
        self,
        title: str,
        message: str,
        context: dict[str, Any] | None = None,
        recipient: str | None = None,
    ) -> bool:
        """发送严重通知"""
        notification = Notification(
            title=title,
            message=message,
            level=NotificationLevel.CRITICAL,
            notification_type="critical",
            metadata=context or {},
            recipient=recipient,
            tags=["critical"],
        )
        return await self.send(notification)

    async def send_usage_alert(
        self,
        user_id: str,
        usage_percent: float,
        limit: int,
        current: int,
        resource_type: str = "tokens",
    ) -> bool:
        """发送使用量警告"""
        level = NotificationLevel.INFO
        if usage_percent >= 90:
            level = NotificationLevel.CRITICAL
        elif usage_percent >= 75:
            level = NotificationLevel.WARNING

        notification = Notification(
            title=f"Usage Alert: {resource_type.capitalize()}",
            message=f"User {user_id} has used {usage_percent:.1f}% of their {resource_type} quota ({current}/{limit})",
            level=level,
            notification_type="usage_alert",
            metadata={
                "user_id": user_id,
                "usage_percent": usage_percent,
                "limit": limit,
                "current": current,
                "resource_type": resource_type,
            },
            tags=["usage", resource_type],
        )
        return await self.send(notification)

    async def send_provider_status(
        self,
        provider: str,
        status: str,
        error: str | None = None,
        latency: float | None = None,
    ) -> bool:
        """发送提供商状态通知"""
        level = NotificationLevel.INFO
        if status == "down":
            level = NotificationLevel.CRITICAL
        elif status == "degraded":
            level = NotificationLevel.WARNING

        message = f"Provider {provider} is {status}"
        if error:
            message += f": {error}"
        if latency:
            message += f" (latency: {latency:.2f}s)"

        notification = Notification(
            title=f"Provider Status: {provider}",
            message=message,
            level=level,
            notification_type="provider_status",
            metadata={"provider": provider, "status": status, "error": error, "latency": latency},
            tags=["provider", status],
        )
        return await self.send(notification)

    async def get_stats(self) -> dict[str, Any]:
        """
        获取统计信息

        Returns:
            统计信息字典，包含基础重试统计和子类特定统计
        """
        base_stats = {
            "plugin_name": self.name,
            "enabled": self.enabled,
            "send_attempts": self._send_attempts,
            "send_successes": self._send_successes,
            "send_failures": self._send_failures,
            "retry_total": self._retry_total,
            "success_rate": (
                self._send_successes / self._send_attempts * 100 if self._send_attempts > 0 else 0
            ),
            "config": {
                "min_level": self.min_level.value,
                "retry_count": self.retry_count,
                "retry_delay": self.retry_delay,
                "retry_backoff": self.retry_backoff,
                "batch_size": self.batch_size,
                "flush_interval": self.flush_interval,
            },
        }

        # 获取子类特定的统计信息
        extra_stats = await self._get_extra_stats()
        if extra_stats:
            base_stats.update(extra_stats)

        return base_stats

    async def _get_extra_stats(self) -> dict[str, Any]:
        """
        获取子类特定的统计信息（子类可选重写）

        Returns:
            额外的统计信息
        """
        return {}
