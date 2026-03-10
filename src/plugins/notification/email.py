"""
邮件通知插件
通过SMTP发送邮件通知
"""

import asyncio
import smtplib
from email.mime.multipart import MIMEMultipart
from email.mime.text import MIMEText
from typing import Any

aiosmtplib: Any
try:
    import aiosmtplib as _aiosmtplib
except ImportError:
    AIOSMTPLIB_AVAILABLE = False
    aiosmtplib = None
else:
    AIOSMTPLIB_AVAILABLE = True
    aiosmtplib = _aiosmtplib

from src.core.logger import logger
from src.utils.async_utils import run_in_executor

from .base import Notification, NotificationLevel, NotificationPlugin


class EmailNotificationPlugin(NotificationPlugin):
    """
    邮件通知插件
    支持HTML和纯文本邮件
    """

    def __init__(self, name: str = "email", config: dict[str, Any] | None = None):
        super().__init__(name, config or {})

        # SMTP配置
        self.smtp_host = config.get("smtp_host") if config else None
        self.smtp_port = config.get("smtp_port", 587) if config else 587
        self.smtp_user = config.get("smtp_user") if config else None
        self.smtp_password = config.get("smtp_password") if config else None
        self.use_tls = config.get("use_tls", True) if config else True
        self.use_ssl = config.get("use_ssl", False) if config else False

        # 邮件配置
        self.from_email = config.get("from_email") if config else None
        self.from_name = config.get("from_name", "Aether") if config else "Aether"
        self.to_emails = config.get("to_emails", []) if config else []
        self.cc_emails = config.get("cc_emails", []) if config else []
        self.bcc_emails = config.get("bcc_emails", []) if config else []

        # 模板配置
        self.use_html = config.get("use_html", True) if config else True
        self.subject_prefix = config.get("subject_prefix", "[Aether]") if config else "[Aether]"

        # 缓冲配置
        self._buffer: list[Notification] = []
        self._buffer_max_size = config.get("buffer_max_size", 500) if config else 500
        self._lock = asyncio.Lock()
        self._flush_task: asyncio.Task[None] | None = None

        # 验证配置
        config_errors = []
        if not self.smtp_host:
            config_errors.append("缺少 smtp_host")
        if not self.from_email:
            config_errors.append("缺少 from_email")
        if not self.to_emails:
            config_errors.append("缺少 to_emails")

        if config_errors:
            self.enabled = False
            for error in config_errors:
                logger.warning(f"Email 插件配置错误: {error}，插件已禁用")
            return

        # 注意: 不在这里启动刷新任务,因为可能还没有运行的事件循环
        # 需要在应用启动后调用 initialize() 方法来启动任务

    async def initialize(self) -> bool:
        """
        初始化插件（在事件循环运行后调用）
        启动后台任务等需要事件循环的操作

        Returns:
            初始化成功返回 True，失败返回 False
        """
        if not self.enabled:
            # 配置无效，插件被禁用
            return False

        if self._flush_task is None:
            self._start_flush_task()

        return True

    def _start_flush_task(self) -> None:
        """启动定时刷新任务"""

        async def flush_loop() -> None:
            while self.enabled:
                await asyncio.sleep(self.flush_interval)
                await self.flush()

        try:
            # 获取当前运行的事件循环
            loop = asyncio.get_running_loop()
            self._flush_task = loop.create_task(flush_loop())
        except RuntimeError:
            # 没有运行的事件循环,任务将在 initialize() 中创建
            logger.warning("Email 插件刷新任务等待事件循环创建")
            pass

    def _format_html_email(self, notifications: list[Notification]) -> str:
        """格式化HTML邮件"""
        # 颜色映射
        color_map = {
            NotificationLevel.INFO: "#28a745",
            NotificationLevel.WARNING: "#ffc107",
            NotificationLevel.ERROR: "#dc3545",
            NotificationLevel.CRITICAL: "#721c24",
        }

        # 构建HTML
        html = """
        <html>
        <head>
            <style>
                body { font-family: Arial, sans-serif; line-height: 1.6; }
                .notification { margin: 20px 0; padding: 15px; border-left: 5px solid; }
                .info { border-left-color: #28a745; background-color: #d4edda; }
                .warning { border-left-color: #ffc107; background-color: #fff3cd; }
                .error { border-left-color: #dc3545; background-color: #f8d7da; }
                .critical { border-left-color: #721c24; background-color: #f8d7da; }
                .title { font-weight: bold; font-size: 1.2em; margin-bottom: 10px; }
                .metadata { margin-top: 10px; font-size: 0.9em; color: #666; }
                .timestamp { font-size: 0.8em; color: #999; }
            </style>
        </head>
        <body>
            <h2>Notifications from Aether</h2>
        """

        for notification in notifications:
            level_class = notification.level.value
            html += f"""
            <div class="notification {level_class}">
                <div class="title">{notification.title}</div>
                <div class="message">{notification.message}</div>
            """

            if notification.metadata:
                html += '<div class="metadata">'
                for key, value in notification.metadata.items():
                    html += f"<strong>{key}:</strong> {value}<br>"
                html += "</div>"

            html += f"""
                <div class="timestamp">{notification.timestamp.isoformat()}</div>
            </div>
            """

        html += """
        </body>
        </html>
        """

        return html

    def _format_text_email(self, notifications: list[Notification]) -> str:
        """格式化纯文本邮件"""
        lines = ["Notifications from Aether", "=" * 50, ""]

        for notification in notifications:
            lines.append(f"[{notification.level.value.upper()}] {notification.title}")
            lines.append("-" * 40)
            lines.append(notification.message)

            if notification.metadata:
                lines.append("")
                for key, value in notification.metadata.items():
                    lines.append(f"  {key}: {value}")

            lines.append(f"\nTime: {notification.timestamp.isoformat()}")
            lines.append("=" * 50)
            lines.append("")

        return "\n".join(lines)

    async def _send_email_async(self, subject: str, body: str, is_html: bool = True) -> bool:
        """异步发送邮件"""
        if AIOSMTPLIB_AVAILABLE:
            # 使用异步SMTP
            message = MIMEMultipart("alternative")
            message["Subject"] = f"{self.subject_prefix} {subject}"
            message["From"] = f"{self.from_name} <{self.from_email}>"
            message["To"] = ", ".join(self.to_emails)

            if self.cc_emails:
                message["Cc"] = ", ".join(self.cc_emails)

            # 添加内容
            if is_html:
                message.attach(MIMEText(body, "html"))
            else:
                message.attach(MIMEText(body, "plain"))

            try:
                # 发送邮件
                if self.use_ssl:
                    await aiosmtplib.send(
                        message,
                        hostname=self.smtp_host,
                        port=self.smtp_port,
                        use_tls=True,
                        username=self.smtp_user,
                        password=self.smtp_password,
                    )
                else:
                    await aiosmtplib.send(
                        message,
                        hostname=self.smtp_host,
                        port=self.smtp_port,
                        start_tls=self.use_tls,
                        username=self.smtp_user,
                        password=self.smtp_password,
                    )
                return True

            except Exception as e:
                logger.error(f"异步邮件发送失败: {e}")
                return False
        else:
            # 使用同步SMTP（在线程中运行）
            return await run_in_executor(self._send_email_sync, subject, body, is_html)

    def _send_email_sync(self, subject: str, body: str, is_html: bool = True) -> bool:
        """同步发送邮件"""
        message = MIMEMultipart("alternative")
        message["Subject"] = f"{self.subject_prefix} {subject}"
        message["From"] = f"{self.from_name} <{self.from_email}>"
        message["To"] = ", ".join(self.to_emails)

        if self.cc_emails:
            message["Cc"] = ", ".join(self.cc_emails)

        # 添加内容
        if is_html:
            message.attach(MIMEText(body, "html"))
        else:
            message.attach(MIMEText(body, "plain"))

        try:
            smtp_host = self.smtp_host
            assert smtp_host is not None

            # 连接SMTP服务器
            server: smtplib.SMTP
            if self.use_ssl:
                server = smtplib.SMTP_SSL(smtp_host, self.smtp_port)
            else:
                server = smtplib.SMTP(smtp_host, self.smtp_port)
                if self.use_tls:
                    server.starttls()

            # 登录
            if self.smtp_user and self.smtp_password:
                server.login(self.smtp_user, self.smtp_password)

            # 发送邮件
            all_recipients = self.to_emails + self.cc_emails + self.bcc_emails
            server.send_message(message, to_addrs=all_recipients)
            server.quit()

            return True

        except Exception as e:
            logger.error(f"同步邮件发送失败: {e}")
            return False

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
                logger.warning("Email 通知缓冲区溢出，丢弃 {} 条旧通知", drop_count)

            self._buffer.append(notification)

            # 如果是严重通知，立即发送
            if notification.level == NotificationLevel.CRITICAL:
                return await self._flush_buffer()

            # 如果缓冲区满，自动刷新
            if len(self._buffer) >= self.batch_size:
                return await self._flush_buffer()

        return True

    async def _do_send_batch(self, notifications: list[Notification]) -> dict[str, int]:
        """实际批量发送通知"""
        if not notifications:
            return {"total": 0, "sent": 0, "failed": 0}

        # 准备邮件内容
        subject = f"Batch Notifications ({len(notifications)} items)"

        # 检查是否有严重通知
        critical_count = sum(1 for n in notifications if n.level == NotificationLevel.CRITICAL)
        if critical_count > 0:
            subject = f"[CRITICAL] {subject}"

        # 格式化邮件内容
        if self.use_html:
            body = self._format_html_email(notifications)
        else:
            body = self._format_text_email(notifications)

        # 发送邮件
        success = await self._send_email_async(subject, body, self.use_html)

        return {
            "total": len(notifications),
            "sent": len(notifications) if success else 0,
            "failed": 0 if success else len(notifications),
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
        """获取 Email 特定的统计信息"""
        return {
            "type": "email",
            "smtp_host": self.smtp_host,
            "smtp_port": self.smtp_port,
            "from_email": self.from_email,
            "recipients_count": len(self.to_emails),
            "buffer_size": len(self._buffer),
            "use_html": self.use_html,
            "aiosmtplib_available": AIOSMTPLIB_AVAILABLE,
        }

    async def close(self) -> None:
        """关闭插件"""
        # 刷新缓冲
        await self.flush()

        # 取消刷新任务
        if self._flush_task:
            self._flush_task.cancel()

    def __del__(self) -> None:
        """清理资源"""
        try:
            from src.utils.async_utils import safe_create_task

            safe_create_task(self.close())
        except Exception:
            pass
