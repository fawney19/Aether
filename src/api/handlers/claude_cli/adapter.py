"""
Claude CLI Adapter - 基于通用 CLI Adapter 基类的简化实现

继承 CliAdapterBase，只需配置 FORMAT_ID 和 HANDLER_CLASS。
"""

from typing import Any, AsyncIterator, Dict, Optional, Tuple, Type, Union

import httpx
from fastapi import Request

from src.api.handlers.base.cli_adapter_base import CliAdapterBase, register_cli_adapter
from src.api.handlers.base.cli_handler_base import CliMessageHandlerBase
from src.api.handlers.claude.adapter import ClaudeCapabilityDetector, ClaudeChatAdapter
from src.config.settings import config


@register_cli_adapter
class ClaudeCliAdapter(CliAdapterBase):
    """
    Claude CLI API 适配器

    处理 Claude CLI 格式的请求（/v1/messages 端点，使用 Bearer 认证）。
    """

    FORMAT_ID = "CLAUDE_CLI"
    name = "claude.cli"

    @property
    def HANDLER_CLASS(self) -> Type[CliMessageHandlerBase]:
        """延迟导入 Handler 类避免循环依赖"""
        from src.api.handlers.claude_cli.handler import ClaudeCliMessageHandler

        return ClaudeCliMessageHandler

    def __init__(self, allowed_api_formats: Optional[list[str]] = None):
        super().__init__(allowed_api_formats or ["CLAUDE_CLI"])

    def extract_api_key(self, request: Request) -> Optional[str]:
        """从请求中提取 API 密钥 (Authorization: Bearer)"""
        authorization = request.headers.get("authorization")
        if authorization and authorization.startswith("Bearer "):
            return authorization.replace("Bearer ", "")
        return None

    def detect_capability_requirements(
        self,
        headers: Dict[str, str],
        request_body: Optional[Dict[str, Any]] = None,
    ) -> Dict[str, bool]:
        """检测 Claude CLI 请求中隐含的能力需求"""
        return ClaudeCapabilityDetector.detect_from_headers(headers)

    # =========================================================================
    # Claude CLI 特定的计费逻辑
    # =========================================================================

    def compute_total_input_context(
        self,
        input_tokens: int,
        cache_read_input_tokens: int,
        cache_creation_input_tokens: int = 0,
    ) -> int:
        """
        计算 Claude CLI 的总输入上下文（用于阶梯计费判定）

        Claude 的总输入 = input_tokens + cache_creation_input_tokens + cache_read_input_tokens
        """
        return input_tokens + cache_creation_input_tokens + cache_read_input_tokens

    def _extract_message_count(self, payload: Dict[str, Any]) -> int:
        """Claude CLI 使用 messages 字段"""
        messages = payload.get("messages", [])
        return len(messages) if isinstance(messages, list) else 0

    def _build_audit_metadata(
        self,
        payload: Dict[str, Any],
        path_params: Optional[Dict[str, Any]] = None,  # noqa: ARG002
    ) -> Dict[str, Any]:
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

    # =========================================================================
    # 模型列表查询
    # =========================================================================

    @classmethod
    async def fetch_models(
        cls,
        client: httpx.AsyncClient,
        base_url: str,
        api_key: str,
        extra_headers: Optional[Dict[str, str]] = None,
    ) -> Tuple[list, Optional[str]]:
        """查询 Claude API 支持的模型列表（带 CLI User-Agent）"""
        # 复用 ClaudeChatAdapter 的实现，添加 CLI User-Agent
        cli_headers = {"User-Agent": config.internal_user_agent_claude_cli}
        if extra_headers:
            cli_headers.update(extra_headers)
        models, error = await ClaudeChatAdapter.fetch_models(
            client, base_url, api_key, cli_headers
        )
        # 更新 api_format 为 CLI 格式
        for m in models:
            m["api_format"] = cls.FORMAT_ID
        return models, error

    @classmethod
    def build_endpoint_url(cls, base_url: str, request_data: Dict[str, Any], model_name: Optional[str] = None) -> str:
        """构建Claude CLI API端点URL"""
        base_url = base_url.rstrip("/")
        if base_url.endswith("/v1"):
            return f"{base_url}/messages"
        else:
            return f"{base_url}/v1/messages"

    @classmethod
    def build_base_headers(cls, api_key: str) -> Dict[str, str]:
        """构建Claude CLI API认证头"""
        return {
            "Authorization": f"Bearer {api_key}",
            "Content-Type": "application/json",
        }

    @classmethod
    def get_protected_header_keys(cls) -> tuple:
        """返回Claude CLI API的保护头部key"""
        return ("authorization", "content-type")

    @classmethod
    def build_request_body(cls, request_data: Dict[str, Any]) -> Dict[str, Any]:
        """构建Claude CLI API请求体"""
        return {
            "model": request_data.get("model"),
            "max_tokens": request_data.get("max_tokens", 100),
            "messages": request_data.get("messages", []),
        }

    @classmethod
    def get_cli_user_agent(cls) -> Optional[str]:
        """获取Claude CLI User-Agent"""
        return config.internal_user_agent_claude_cli


__all__ = ["ClaudeCliAdapter"]
