"""
Gemini CLI Adapter - 基于通用 CLI Adapter 基类的实现

继承 CliAdapterBase，处理 Gemini CLI 格式的请求。
"""

from __future__ import annotations

from typing import Any

from fastapi import Request

from src.api.handlers.base.cli_adapter_base import CliAdapterBase, register_cli_adapter
from src.api.handlers.base.cli_handler_base import CliMessageHandlerBase
from src.api.handlers.gemini.adapter import GeminiCapabilityDetector
from src.config.settings import config
from src.core.api_format import ApiFamily, get_auth_handler
from src.core.api_format.enums import AuthMethod


@register_cli_adapter
class GeminiCliAdapter(CliAdapterBase):
    """
    Gemini CLI API 适配器

    处理 Gemini CLI 格式的请求（透传模式，最小验证）。
    """

    FORMAT_ID = "gemini:cli"
    API_FAMILY = ApiFamily.GEMINI
    name = "gemini.cli"

    @property
    def HANDLER_CLASS(self) -> type[CliMessageHandlerBase]:
        """延迟导入 Handler 类避免循环依赖"""
        from src.api.handlers.gemini_cli.handler import GeminiCliMessageHandler

        return GeminiCliMessageHandler

    def __init__(self, allowed_api_formats: list[str] | None = None):
        super().__init__(allowed_api_formats)

    def extract_api_key(self, request: Request) -> str | None:
        """
        从请求中提取 API 密钥 - Gemini CLI 支持 header 和 query 两种方式

        优先级（与 Google SDK 行为一致）：
        1. URL 参数 ?key=
        2. x-goog-api-key 请求头
        """
        handler = get_auth_handler(AuthMethod.GOOG_API_KEY)
        return handler.extract_credentials(request)

    def detect_capability_requirements(
        self,
        headers: dict[str, str],
        request_body: dict[str, Any] | None = None,
    ) -> dict[str, bool]:
        """从请求体检测 Gemini 能力需求（fileData.fileUri -> gemini_files）"""
        return GeminiCapabilityDetector.detect_from_request(headers, request_body)

    def _merge_path_params(
        self, original_request_body: dict[str, Any], path_params: dict[str, Any]  # noqa: ARG002
    ) -> dict[str, Any]:
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

    def _extract_message_count(self, payload: dict[str, Any]) -> int:
        """Gemini CLI 使用 contents 字段"""
        contents = payload.get("contents", [])
        return len(contents) if isinstance(contents, list) else 0

    def _build_audit_metadata(
        self,
        payload: dict[str, Any],
        path_params: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        """Gemini CLI 特定的审计元数据"""
        # 从 path_params 获取 model（Gemini 请求体不含 model）
        model = path_params.get("model", "unknown") if path_params else "unknown"
        contents = payload.get("contents", [])
        generation_config = payload.get("generation_config", {}) or {}

        role_counts: dict[str, int] = {}
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

    @classmethod
    def build_endpoint_url(
        cls,
        base_url: str,
        request_data: dict[str, Any],
        model_name: str | None = None,
        *,
        provider_type: str | None = None,
    ) -> str:
        """构建Gemini CLI API端点URL"""
        from src.utils.url_utils import join_url

        effective_model_name = model_name or request_data.get("model", "")
        if not effective_model_name:
            raise ValueError("Model name is required for Gemini API")

        return join_url(base_url, f"/v1beta/models/{effective_model_name}:generateContent")

    # build_request_body 使用基类实现，通过 format_conversion_registry 自动转换 OPENAI -> GEMINI_CLI

    @classmethod
    def get_cli_user_agent(cls) -> str | None:
        """获取Gemini CLI User-Agent"""
        return config.internal_user_agent_gemini_cli

    @classmethod
    def get_cli_extra_headers(
        cls, *, base_url: str | None = None, provider_type: str | None = None
    ) -> dict[str, str]:
        """获取Gemini CLI额外请求头，包含 x-app: cli 标识"""
        headers = super().get_cli_extra_headers(base_url=base_url, provider_type=provider_type)
        headers["x-app"] = "cli"  # 标识 CLI 模式，让上游使用正确的 adapter
        return headers


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
