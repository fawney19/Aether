"""
Webhook通知插件
通过HTTP Webhook发送通知
"""

from __future__ import annotations

import asyncio
import hashlib
import hmac
import json
import time
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    import aiohttp

try:
    import aiohttp

    AIOHTTP_AVAILABLE = True
except ImportError:
    AIOHTTP_AVAILABLE = False

from src.core.logger import logger

from .base import Notification, NotificationLevel, NotificationPlugin


class WebhookNotificationPlugin(NotificationPlugin):
    """
    Webhook通知插件
    支持多种Webhook格式（Slack, Discord, 通用）
    """

    def __init__(self, name: str = "webhook", config: dict[str, Any] | None = None):
        super().__init__(name, config)

        if not AIOHTTP_AVAILABLE:
            self.enabled = False
            logger.warning("aiohttp not installed, webhook plugin disabled")
            return

        # Webhook配置
        self.webhook_url = config.get("webhook_url") if config else None
        self.webhook_type = (
            config.get("webhook_type", "generic") if config else "generic"
        )  # generic, slack, discord, teams
        self.secret = config.get("secret") if config else None  # 用于签名
        self.timeout = config.get("timeout", 30) if config else 30
        self.headers = config.get("headers", {}) if config else {}

        # 缓冲配置
        self._buffer: list[Notification] = []
        self._buffer_max_size = config.get("buffer_max_size", 500) if config else 500
        self._lock = asyncio.Lock()
        self._session: aiohttp.ClientSession | None = None
        self._flush_task = None

        if not self.webhook_url:
            self.enabled = False
            logger.warning("No webhook URL configured")
            return

        # 启动刷新任务
        self._start_flush_task()

    def _start_flush_task(self) -> None:
        """启动定时刷新任务"""

        async def flush_loop() -> Any:
            while self.enabled:
                await asyncio.sleep(self.flush_interval)
                await self.flush()

        self._flush_task = asyncio.create_task(flush_loop())

    async def _get_session(self) -> aiohttp.ClientSession:
        """获取HTTP会话"""
        if not self._session:
            self._session = aiohttp.ClientSession(timeout=aiohttp.ClientTimeout(total=self.timeout))
        return self._session

    def _generate_signature(self, payload: str) -> str:
        """生成请求签名"""
        if not self.secret:
            return ""

        # 使用HMAC-SHA256生成签名
        signature = hmac.new(
            self.secret.encode("utf-8"), payload.encode("utf-8"), hashlib.sha256
        ).hexdigest()

        return signature

    def _format_for_slack(self, notification: Notification) -> dict[str, Any]:
        """格式化为Slack消息"""
        # Slack颜色映射
        color_map = {
            NotificationLevel.INFO: "#36a64f",
            NotificationLevel.WARNING: "warning",
            NotificationLevel.ERROR: "danger",
            NotificationLevel.CRITICAL: "#ff0000",
        }

        return {
            "text": notification.title,
            "attachments": [
                {
                    "color": color_map.get(notification.level, "#808080"),
                    "title": notification.title,
                    "text": notification.message,
                    "fields": (
                        [
                            {"title": k, "value": str(v), "short": True}
                            for k, v in notification.metadata.items()
                        ]
                        if notification.metadata
                        else []
                    ),
                    "footer": notification.source,
                    "ts": int(notification.timestamp.timestamp()),
                }
            ],
        }

    def _format_for_discord(self, notification: Notification) -> dict[str, Any]:
        """格式化为Discord消息"""
        # Discord颜色映射
        color_map = {
            NotificationLevel.INFO: 0x00FF00,
            NotificationLevel.WARNING: 0xFFA500,
            NotificationLevel.ERROR: 0xFF0000,
            NotificationLevel.CRITICAL: 0x8B0000,
        }

        embeds = [
            {
                "title": notification.title,
                "description": notification.message,
                "color": color_map.get(notification.level, 0x808080),
                "fields": (
                    [
                        {"name": k, "value": str(v), "inline": True}
                        for k, v in notification.metadata.items()
                    ]
                    if notification.metadata
                    else []
                ),
                "footer": {"text": notification.source},
                "timestamp": notification.timestamp.isoformat(),
            }
        ]

        return {"embeds": embeds}

    def _format_for_teams(self, notification: Notification) -> dict[str, Any]:
        """格式化为Microsoft Teams消息"""
        # Teams颜色映射
        color_map = {
            NotificationLevel.INFO: "00ff00",
            NotificationLevel.WARNING: "ffa500",
            NotificationLevel.ERROR: "ff0000",
            NotificationLevel.CRITICAL: "8b0000",
        }

        facts = (
            [{"name": k, "value": str(v)} for k, v in notification.metadata.items()]
            if notification.metadata
            else []
        )

        return {
            "@type": "MessageCard",
            "@context": "https://schema.org/extensions",
            "themeColor": color_map.get(notification.level, "808080"),
            "title": notification.title,
            "text": notification.message,
            "sections": [{"facts": facts}] if facts else [],
            "summary": notification.title,
        }

    def _format_payload(self, notification: Notification) -> dict[str, Any]:
        """根据Webhook类型格式化负载"""
        if self.webhook_type == "slack":
            return self._format_for_slack(notification)
        elif self.webhook_type == "discord":
            return self._format_for_discord(notification)
        elif self.webhook_type == "teams":
            return self._format_for_teams(notification)
        else:
            # 通用格式
            return notification.to_dict()

    async def _do_send(self, notification: Notification) -> bool:
        """
        实际发送单个通知

        Note: 对于 CRITICAL 级别通知，直接发送；其他级别加入缓冲区
        """
        # 添加到缓冲区
        async with self._lock:
            # 缓冲区溢出保护：丢弃最旧的通知
            if len(self._buffer) >= self._buffer_max_size:
                drop_count = len(self._buffer) - self._buffer_max_size + 1
                del self._buffer[:drop_count]
                logger.warning("Webhook 通知缓冲区溢出，丢弃 {} 条旧通知", drop_count)

            self._buffer.append(notification)

            # 如果是严重通知，立即发送
            if notification.level == NotificationLevel.CRITICAL:
                return await self._flush_buffer()

            # 如果缓冲区满，自动刷新
            if len(self._buffer) >= self.batch_size:
                return await self._flush_buffer()

        return True

    async def _do_send_batch(self, notifications: list[Notification]) -> dict[str, Any]:
        """实际批量发送通知"""
        success_count = 0
        failed_count = 0
        errors = []

        if not notifications:
            return {"total": 0, "sent": 0, "failed": 0}

        # 批量发送
        for notification in notifications:
            try:
                payload = self._format_payload(notification)
                payload_str = json.dumps(payload)

                headers = dict(self.headers)
                headers["Content-Type"] = "application/json"

                # 添加签名
                if self.secret:
                    signature = self._generate_signature(payload_str)
                    headers["X-Signature"] = signature
                    headers["X-Timestamp"] = str(int(time.time()))

                # 发送请求
                session = await self._get_session()
                async with session.post(
                    self.webhook_url, data=payload_str, headers=headers
                ) as response:
                    if response.status < 300:
                        success_count += 1
                    else:
                        failed_count += 1
                        error_text = await response.text()
                        errors.append(f"HTTP {response.status}: {error_text}")

            except Exception as e:
                failed_count += 1
                errors.append(str(e))

        return {
            "total": len(notifications),
            "sent": success_count,
            "failed": failed_count,
            "errors": errors,
        }

    async def _flush_buffer(self) -> bool:
        """刷新缓冲的通知（内部方法，不带锁）"""
        if not self._buffer:
            return True

        notifications = self._buffer[:]
        self._buffer.clear()

        # 批量发送（直接调用 _do_send_batch 避免重复统计）
        result = await self._do_send_batch(notifications)
        return result["failed"] == 0

    async def flush(self) -> bool:
        """刷新缓冲的通知"""
        async with self._lock:
            return await self._flush_buffer()

    async def _get_extra_stats(self) -> dict[str, Any]:
        """获取 Webhook 特定的统计信息"""
        return {
            "type": "webhook",
            "webhook_type": self.webhook_type,
            "webhook_url": (
                self.webhook_url.split("?")[0] if self.webhook_url else None
            ),  # 隐藏查询参数
            "buffer_size": len(self._buffer),
            "has_secret": bool(self.secret),
        }

    async def _do_shutdown(self) -> None:
        """清理资源"""
        await self.close()

    async def close(self) -> Any:
        """关闭插件"""
        # 刷新缓冲
        await self.flush()

        # 取消刷新任务
        if self._flush_task:
            self._flush_task.cancel()

        # 关闭HTTP会话
        if self._session:
            await self._session.close()

    def __del__(self) -> None:
        """清理资源"""
        try:
            from src.utils.async_utils import safe_create_task

            safe_create_task(self.close())
        except Exception:
            pass
