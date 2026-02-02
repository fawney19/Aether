"""
OpenAI CLI Adapter - 基于通用 CLI Adapter 基类的简化实现

继承 CliAdapterBase，只需配置 FORMAT_ID 和 HANDLER_CLASS。
"""

from __future__ import annotations

from typing import Any

import httpx

from src.api.handlers.base.cli_adapter_base import CliAdapterBase, register_cli_adapter
from src.api.handlers.base.cli_handler_base import CliMessageHandlerBase
from src.api.handlers.openai.adapter import OpenAIChatAdapter
from src.config.settings import config
from src.core.api_format import ApiFamily


@register_cli_adapter
class OpenAICliAdapter(CliAdapterBase):
    """
    OpenAI CLI API 适配器

    处理 /v1/responses 端点的请求。
    """

    FORMAT_ID = "openai:cli"
    API_FAMILY = ApiFamily.OPENAI
    BILLING_TEMPLATE = "openai"  # 使用 OpenAI 计费模板
    name = "openai.cli"

    @property
    def HANDLER_CLASS(self) -> type[CliMessageHandlerBase]:
        """延迟导入 Handler 类避免循环依赖"""
        from src.api.handlers.openai_cli.handler import OpenAICliMessageHandler

        return OpenAICliMessageHandler

    def __init__(self, allowed_api_formats: list[str] | None = None):
        super().__init__(allowed_api_formats)

    # =========================================================================
    # 模型列表查询
    # =========================================================================

    @classmethod
    async def fetch_models(
        cls,
        client: httpx.AsyncClient,
        base_url: str,
        api_key: str,
        extra_headers: dict[str, str] | None = None,
    ) -> tuple[list, str | None]:
        """查询 OpenAI 兼容 API 支持的模型列表（带 CLI User-Agent）"""
        # 复用 OpenAIChatAdapter 的实现，添加 CLI User-Agent
        cli_headers = {"User-Agent": config.internal_user_agent_openai_cli}
        if extra_headers:
            cli_headers.update(extra_headers)
        models, error = await OpenAIChatAdapter.fetch_models(client, base_url, api_key, cli_headers)
        # 更新 api_format 为 CLI 格式
        for m in models:
            m["api_format"] = cls.FORMAT_ID
        return models, error

    @classmethod
    def build_endpoint_url(
        cls, base_url: str, request_data: dict[str, Any], model_name: str | None = None
    ) -> str:
        """构建OpenAI CLI API端点URL

        OPENAI_CLI 格式使用 /responses 端点（Responses API），
        与 OPENAI 格式的 /v1/chat/completions 端点（Chat Completions API）不同。
        """
        base_url = base_url.rstrip("/")
        # OPENAI_CLI 使用 /responses 端点
        if base_url.endswith("/v1"):
            return f"{base_url}/responses"
        else:
            return f"{base_url}/v1/responses"

    # build_request_body 使用基类实现
    # OPENAI -> OPENAI_CLI 无转换器，会直接透传原始请求

    @classmethod
    def get_cli_user_agent(cls) -> str | None:
        """获取OpenAI CLI User-Agent"""
        return config.internal_user_agent_openai_cli


__all__ = ["OpenAICliAdapter"]
