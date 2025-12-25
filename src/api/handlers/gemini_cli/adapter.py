"""
Gemini CLI Adapter - 基于通用 CLI Adapter 基类的实现

继承 CliAdapterBase，处理 Gemini CLI 格式的请求。
"""

from typing import Any, AsyncIterator, Dict, Optional, Tuple, Type, Union

import httpx
from fastapi import Request

from src.api.handlers.base.cli_adapter_base import CliAdapterBase, register_cli_adapter
from src.api.handlers.base.cli_handler_base import CliMessageHandlerBase
from src.api.handlers.gemini.adapter import GeminiChatAdapter
from src.config.settings import config


@register_cli_adapter
class GeminiCliAdapter(CliAdapterBase):
    """
    Gemini CLI API 适配器

    处理 Gemini CLI 格式的请求（透传模式，最小验证）。
    """

    FORMAT_ID = "GEMINI_CLI"
    name = "gemini.cli"

    @property
    def HANDLER_CLASS(self) -> Type[CliMessageHandlerBase]:
        """延迟导入 Handler 类避免循环依赖"""
        from src.api.handlers.gemini_cli.handler import GeminiCliMessageHandler

        return GeminiCliMessageHandler

    def __init__(self, allowed_api_formats: Optional[list[str]] = None):
        super().__init__(allowed_api_formats or ["GEMINI_CLI"])

    def extract_api_key(self, request: Request) -> Optional[str]:
        """从请求中提取 API 密钥 (x-goog-api-key)"""
        return request.headers.get("x-goog-api-key")

    def _merge_path_params(
        self, original_request_body: Dict[str, Any], path_params: Dict[str, Any]  # noqa: ARG002
    ) -> Dict[str, Any]:
        """
        合并 URL 路径参数到请求体 - Gemini CLI 特化版本

        Gemini API 特点:
        - model 不合并到请求体（Gemini 原生请求体不含 model，通过 URL 路径传递）
        - stream 不合并到请求体（Gemini API 通过 URL 端点区分流式/非流式）

        基类已经从 path_params 获取 model 和 stream 用于日志和路由判断。

        Args:
            original_request_body: 原始请求体字典
            path_params: URL 路径参数字典（包含 model、stream 等）

        Returns:
            原始请求体（不合并任何 path_params）
        """
        # Gemini: 不合并任何 path_params 到请求体
        return original_request_body.copy()

    def _extract_message_count(self, payload: Dict[str, Any]) -> int:
        """Gemini CLI 使用 contents 字段"""
        contents = payload.get("contents", [])
        return len(contents) if isinstance(contents, list) else 0

    def _build_audit_metadata(
        self,
        payload: Dict[str, Any],
        path_params: Optional[Dict[str, Any]] = None,
    ) -> Dict[str, Any]:
        """Gemini CLI 特定的审计元数据"""
        # 从 path_params 获取 model（Gemini 请求体不含 model）
        model = path_params.get("model", "unknown") if path_params else "unknown"
        contents = payload.get("contents", [])
        generation_config = payload.get("generation_config", {}) or {}

        role_counts: Dict[str, int] = {}
        for content in contents:
            role = content.get("role", "unknown") if isinstance(content, dict) else "unknown"
            role_counts[role] = role_counts.get(role, 0) + 1

        return {
            "action": "gemini_cli_request",
            "model": model,
            "stream": bool(payload.get("stream", False)),
            "max_output_tokens": generation_config.get("max_output_tokens"),
            "contents_count": len(contents),
            "content_roles": role_counts,
            "temperature": generation_config.get("temperature"),
            "top_p": generation_config.get("top_p"),
            "top_k": generation_config.get("top_k"),
            "tools_count": len(payload.get("tools") or []),
            "system_instruction_present": bool(payload.get("system_instruction")),
            "safety_settings_count": len(payload.get("safety_settings") or []),
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
        """查询 Gemini API 支持的模型列表（带 CLI User-Agent）"""
        # 复用 GeminiChatAdapter 的实现，添加 CLI User-Agent
        cli_headers = {"User-Agent": config.internal_user_agent_gemini_cli}
        if extra_headers:
            cli_headers.update(extra_headers)
        models, error = await GeminiChatAdapter.fetch_models(
            client, base_url, api_key, cli_headers
        )
        # 更新 api_format 为 CLI 格式
        for m in models:
            m["api_format"] = cls.FORMAT_ID
        return models, error

    @classmethod
    def build_endpoint_url(cls, base_url: str, request_data: Dict[str, Any], model_name: Optional[str] = None) -> str:
        """构建Gemini CLI API端点URL"""
        effective_model_name = model_name or request_data.get("model", "")
        if not effective_model_name:
            raise ValueError("Model name is required for Gemini API")

        base_url = base_url.rstrip("/")
        if base_url.endswith("/v1beta"):
            prefix = base_url
        else:
            prefix = f"{base_url}/v1beta"
        return f"{prefix}/models/{effective_model_name}:generateContent"

    @classmethod
    def build_base_headers(cls, api_key: str) -> Dict[str, str]:
        """构建Gemini CLI API认证头"""
        return {
            "x-goog-api-key": api_key,
            "Content-Type": "application/json",
        }

    @classmethod
    def get_protected_header_keys(cls) -> tuple:
        """返回Gemini CLI API的保护头部key"""
        return ("x-goog-api-key", "content-type")

    @classmethod
    def build_request_body(cls, request_data: Dict[str, Any]) -> Dict[str, Any]:
        """构建Gemini CLI API请求体"""
        return {
            "contents": request_data.get("messages", []),
            "generationConfig": {
                "maxOutputTokens": request_data.get("max_tokens", 100),
                "temperature": request_data.get("temperature", 0.7),
            },
            "safetySettings": [
                {"category": "HARM_CATEGORY_HARASSMENT", "threshold": "BLOCK_NONE"}
            ],
        }

    @classmethod
    def get_cli_user_agent(cls) -> Optional[str]:
        """获取Gemini CLI User-Agent"""
        return config.internal_user_agent_gemini_cli


def build_gemini_cli_adapter(x_app_header: str = "") -> GeminiCliAdapter:
    """
    构建 Gemini CLI 适配器

    Args:
        x_app_header: X-App 请求头值（预留扩展）

    Returns:
        GeminiCliAdapter 实例
    """
    return GeminiCliAdapter()


__all__ = ["GeminiCliAdapter", "build_gemini_cli_adapter"]
