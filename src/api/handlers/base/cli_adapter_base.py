"""
CLI Adapter 通用基类

提供 CLI 格式（直接透传请求）的通用适配器逻辑：
- 请求解析和验证
- 审计日志记录
- Handler 创建和调用

公共逻辑（异常处理、计费、头部构建等）继承自 HandlerAdapterBase。

子类只需提供：
- FORMAT_ID: API 格式标识
- HANDLER_CLASS: 对应的 MessageHandler 类
- 可选覆盖 _extract_message_count() 自定义消息计数逻辑
- 可选覆盖 compute_total_input_context() 自定义总输入上下文计算
"""

from __future__ import annotations

from typing import Any

from fastapi import HTTPException
from fastapi.responses import JSONResponse

from src.api.base.adapter import ApiMode
from src.api.base.context import ApiRequestContext
from src.api.handlers.base.cli_handler_base import CliMessageHandlerBase
from src.api.handlers.base.handler_adapter_base import HandlerAdapterBase
from src.core.api_format import EndpointKind
from src.core.exceptions import (
    InvalidRequestException,
    ModelNotSupportedException,
    ProviderAuthException,
    ProviderNotAvailableException,
    ProviderRateLimitException,
    ProviderTimeoutException,
    BalanceInsufficientException,
    UpstreamClientException,
)
from src.core.logger import logger


class CliAdapterBase(HandlerAdapterBase):
    """
    CLI Adapter 通用基类

    提供 CLI 格式的通用适配器逻辑，子类只需配置：
    - FORMAT_ID: API 格式标识
    - HANDLER_CLASS: MessageHandler 类
    - name: 适配器名称
    """

    HANDLER_CLASS: type[CliMessageHandlerBase]

    # CLI 端点类型覆盖（基类默认 CHAT）
    ENDPOINT_KIND = EndpointKind.CLI

    # 适配器配置
    name: str = "cli.base"
    mode = ApiMode.PROXY

    async def handle(self, context: ApiRequestContext) -> Any:
        """处理 CLI API 请求"""
        http_request = context.request
        user = context.user
        api_key = context.api_key
        db = context.db
        request_id = context.request_id
        balance_remaining_value = context.balance_remaining
        start_time = context.start_time
        client_ip = context.client_ip
        user_agent = context.user_agent
        original_headers = context.original_headers
        query_params = context.query_params

        # Store original headers for downstream envelope checks (e.g. CLI-only restriction).
        # Only relevant for Claude Code CLI format; skip for others to avoid unnecessary coupling.
        if self.FORMAT_ID == "claude:cli":
            from src.services.provider.adapters.claude_code.client_restriction import (
                set_original_request_headers,
            )

            set_original_request_headers(original_headers)

        original_request_body = context.ensure_json_body()

        # 合并 path_params 到请求体（如 Gemini API 的 model 在 URL 路径中）
        if context.path_params:
            original_request_body = self._merge_path_params(
                original_request_body, context.path_params
            )

        # 获取 stream：优先从请求体，其次从 path_params（如 Gemini 通过 URL 端点区分）
        stream = original_request_body.get("stream")
        if stream is None and context.path_params:
            stream = context.path_params.get("stream", False)
        stream = bool(stream)

        # 获取 model：优先从请求体，其次从 path_params（如 Gemini 的 model 在 URL 路径中）
        model = original_request_body.get("model")
        if model is None and context.path_params:
            model = context.path_params.get("model", "unknown")
        model = model or "unknown"

        # 提取请求元数据
        audit_metadata = self._build_audit_metadata(original_request_body, context.path_params)
        context.add_audit_metadata(**audit_metadata)

        # 格式化额度显示
        balance_display = (
            "unlimited" if balance_remaining_value is None else f"${balance_remaining_value:.2f}"
        )

        # 请求开始日志
        logger.info(
            f"[REQ] {request_id[:8]} | {self.FORMAT_ID} | {getattr(api_key, 'name', 'unknown')} | "
            f"{model} | {'stream' if stream else 'sync'} | balance:{balance_display}"
        )

        try:
            # 检查客户端连接
            if await http_request.is_disconnected():
                logger.warning("客户端连接断开")
                raise HTTPException(status_code=499, detail="Client disconnected")

            # 创建 Handler
            handler = self.HANDLER_CLASS(
                db=db,
                user=user,
                api_key=api_key,
                request_id=request_id,
                client_ip=client_ip,
                user_agent=user_agent,
                start_time=start_time,
                allowed_api_formats=self.allowed_api_formats,
                adapter_detector=self.detect_capability_requirements,
                perf_metrics=context.extra.get("perf"),
                api_family=self.API_FAMILY.value if self.API_FAMILY else None,
                endpoint_kind=self.ENDPOINT_KIND.value if self.ENDPOINT_KIND else None,
            )

            # 处理请求
            if stream:
                return await handler.process_stream(
                    original_request_body=original_request_body,
                    original_headers=original_headers,
                    query_params=query_params,
                    path_params=context.path_params,
                    http_request=http_request,
                    client_content_encoding=context.client_content_encoding,
                )
            return await handler.process_sync(
                original_request_body=original_request_body,
                original_headers=original_headers,
                query_params=query_params,
                path_params=context.path_params,
                client_content_encoding=context.client_content_encoding,
                client_accept_encoding=context.client_accept_encoding,
            )

        except HTTPException:
            raise

        except (
            ModelNotSupportedException,
            BalanceInsufficientException,
            InvalidRequestException,
        ) as e:
            logger.debug("客户端请求错误: {}", e.error_type)
            return self._error_response(
                status_code=e.status_code,
                error_type=("invalid_request_error" if e.status_code == 400 else "quota_exceeded"),
                message=e.message,
            )

        except (
            ProviderAuthException,
            ProviderRateLimitException,
            ProviderNotAvailableException,
            ProviderTimeoutException,
            UpstreamClientException,
        ) as e:
            return await self._handle_provider_exception(
                e,
                db=db,
                user=user,
                api_key=api_key,
                model=model,
                stream=stream,
                start_time=start_time,
                original_headers=original_headers,
                original_request_body=original_request_body,
                client_ip=client_ip,
                request_id=request_id,
            )

        except Exception as e:
            return await self._handle_unexpected_exception(
                e,
                db=db,
                user=user,
                api_key=api_key,
                model=model,
                stream=stream,
                start_time=start_time,
                original_headers=original_headers,
                original_request_body=original_request_body,
                client_ip=client_ip,
                request_id=request_id,
            )

    def _extract_message_count(self, payload: dict[str, Any]) -> int:
        """提取消息数量 - 子类可覆盖"""
        if "input" not in payload:
            return 0
        input_data = payload["input"]
        if isinstance(input_data, list):
            return len(input_data)
        if isinstance(input_data, dict) and "messages" in input_data:
            return len(input_data.get("messages", []))
        return 0

    def _build_audit_metadata(
        self,
        payload: dict[str, Any],
        path_params: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        """构建审计日志元数据 - 子类可覆盖"""
        model = payload.get("model")
        if model is None and path_params:
            model = path_params.get("model", "unknown")
        model = model or "unknown"

        stream = payload.get("stream", False)
        messages_count = self._extract_message_count(payload)

        return {
            "action": f"{self.FORMAT_ID.lower()}_request",
            "model": model,
            "stream": bool(stream),
            "max_tokens": payload.get("max_tokens"),
            "messages_count": messages_count,
            "temperature": payload.get("temperature"),
            "top_p": payload.get("top_p"),
            "tool_count": len(payload.get("tools") or []),
            "instructions_present": bool(payload.get("instructions")),
        }


# =========================================================================
# CLI Adapter 注册表
# =========================================================================

_CLI_ADAPTER_REGISTRY: dict[str, type[CliAdapterBase]] = {}
_CLI_ADAPTERS_LOADED = False


def register_cli_adapter(adapter_class: type[CliAdapterBase]) -> type[CliAdapterBase]:
    """
    注册 CLI Adapter 类到注册表

    用法：
        @register_cli_adapter
        class ClaudeCliAdapter(CliAdapterBase):
            FORMAT_ID = "CLAUDE_CLI"
            ...
    """
    format_id = adapter_class.FORMAT_ID
    if format_id and format_id != "UNKNOWN":
        _CLI_ADAPTER_REGISTRY[format_id.upper()] = adapter_class
    return adapter_class


def _ensure_cli_adapters_loaded() -> None:
    """确保所有 CLI Adapter 已被加载（触发注册）"""
    global _CLI_ADAPTERS_LOADED
    if _CLI_ADAPTERS_LOADED:
        return

    try:
        from src.api.handlers.claude_cli import adapter as _  # noqa: F401
    except ImportError:
        pass
    try:
        from src.api.handlers.openai_cli import adapter as _  # noqa: F401
    except ImportError:
        pass
    try:
        from src.api.handlers.gemini_cli import adapter as _  # noqa: F401
    except ImportError:
        pass

    _CLI_ADAPTERS_LOADED = True


def get_cli_adapter_class(api_format: str) -> type[CliAdapterBase] | None:
    """根据 API format 获取 CLI Adapter 类"""
    _ensure_cli_adapters_loaded()
    return _CLI_ADAPTER_REGISTRY.get(api_format.upper()) if api_format else None


def get_cli_adapter_instance(api_format: str) -> CliAdapterBase | None:
    """根据 API format 获取 CLI Adapter 实例"""
    adapter_class = get_cli_adapter_class(api_format)
    if adapter_class:
        return adapter_class()
    return None


def list_registered_cli_formats() -> list[str]:
    """返回所有已注册的 CLI API 格式"""
    _ensure_cli_adapters_loaded()
    return list(_CLI_ADAPTER_REGISTRY.keys())
