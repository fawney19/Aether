"""
Claude CLI Adapter - 基于通用 CLI Adapter 基类的简化实现

继承 CliAdapterBase，只需配置 FORMAT_ID 和 HANDLER_CLASS。
"""

from __future__ import annotations

from typing import Any

from src.api.handlers.base.cli_adapter_base import CliAdapterBase, register_cli_adapter
from src.api.handlers.base.cli_handler_base import CliMessageHandlerBase
from src.api.handlers.claude.adapter import ClaudeCapabilityDetector
from src.config.settings import config
from src.core.api_format import ApiFamily


@register_cli_adapter
class ClaudeCliAdapter(CliAdapterBase):
    """
    Claude CLI API 适配器

    处理 Claude CLI 格式的请求（/v1/messages 端点，使用 Bearer 认证）。
    """

    FORMAT_ID = "claude:cli"
    API_FAMILY = ApiFamily.CLAUDE
    name = "claude.cli"

    @property
    def HANDLER_CLASS(self) -> type[CliMessageHandlerBase]:
        """延迟导入 Handler 类避免循环依赖"""
        from src.api.handlers.claude_cli.handler import ClaudeCliMessageHandler

        return ClaudeCliMessageHandler

    def __init__(self, allowed_api_formats: list[str] | None = None):
        super().__init__(allowed_api_formats)

    def detect_capability_requirements(
        self,
        headers: dict[str, str],
        request_body: dict[str, Any] | None = None,
    ) -> dict[str, bool]:
        """检测 Claude CLI 请求中隐含的能力需求"""
        return ClaudeCapabilityDetector.detect_from_headers(headers, request_body)

    def _extract_message_count(self, payload: dict[str, Any]) -> int:
        """Claude CLI 使用 messages 字段"""
        messages = payload.get("messages", [])
        return len(messages) if isinstance(messages, list) else 0

    def _build_audit_metadata(
        self,
        payload: dict[str, Any],
        path_params: dict[str, Any] | None = None,  # noqa: ARG002
    ) -> dict[str, Any]:
        """Claude CLI 特定的审计元数据"""
        model = payload.get("model", "unknown")
        stream = payload.get("stream", False)
        messages = payload.get("messages", [])

        role_counts = {}
        for msg in messages:
            role = msg.get("role", "unknown")
            role_counts[role] = role_counts.get(role, 0) + 1

        return {
            "action": "claude_cli_request",
            "model": model,
            "stream": bool(stream),
            "max_tokens": payload.get("max_tokens"),
            "messages_count": len(messages),
            "message_roles": role_counts,
            "temperature": payload.get("temperature"),
            "top_p": payload.get("top_p"),
            "tool_count": len(payload.get("tools") or []),
            "system_present": bool(payload.get("system")),
        }

    @classmethod
    def build_endpoint_url(
        cls,
        base_url: str,
        request_data: dict[str, Any],
        model_name: str | None = None,
        *,
        provider_type: str | None = None,
    ) -> str:
        """构建Claude CLI API端点URL"""
        from src.utils.url_utils import join_url

        return join_url(base_url, "/v1/messages")

    # build_request_body 使用基类实现，通过 format_conversion_registry 自动转换 OPENAI -> CLAUDE_CLI

    @classmethod
    def get_cli_user_agent(cls) -> str | None:
        """获取Claude CLI User-Agent"""
        return config.internal_user_agent_claude_cli


__all__ = ["ClaudeCliAdapter"]
