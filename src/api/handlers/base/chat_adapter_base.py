"""
Chat Adapter 通用基类

提供 Chat 格式（进行请求验证和标准化）的通用适配器逻辑：
- 请求解析和验证（Pydantic）
- 审计日志记录
- Handler 创建和调用

公共逻辑（异常处理、计费、头部构建等）继承自 HandlerAdapterBase。

子类只需提供：
- FORMAT_ID: API 格式标识
- HANDLER_CLASS: 对应的 ChatHandlerBase 子类
- _validate_request_body(): 请求验证逻辑
"""

from __future__ import annotations

from abc import abstractmethod
from typing import Any

from fastapi import HTTPException
from fastapi.responses import JSONResponse
from sqlalchemy.orm import Session

from src.api.base.adapter import ApiMode
from src.api.base.context import ApiRequestContext
from src.api.handlers.base.chat_handler_base import ChatHandlerBase
from src.api.handlers.base.handler_adapter_base import HandlerAdapterBase
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


class ChatAdapterBase(HandlerAdapterBase):
    """
    Chat Adapter 通用基类

    提供 Chat 格式的通用适配器逻辑，子类只需配置：
    - FORMAT_ID: API 格式标识
    - HANDLER_CLASS: ChatHandlerBase 子类
    - name: 适配器名称
    """

    HANDLER_CLASS: type[ChatHandlerBase]

    # 适配器配置
    name: str = "chat.base"
    mode = ApiMode.STANDARD

    async def handle(self, context: ApiRequestContext) -> Any:
        """处理 Chat API 请求"""
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

        original_request_body = context.ensure_json_body()

        # 合并 path_params 到请求体（如 Gemini API 的 model 在 URL 路径中）
        if context.path_params:
            original_request_body = self._merge_path_params(
                original_request_body, context.path_params
            )

        # 验证和解析请求
        request_obj = self._validate_request_body(original_request_body, context.path_params)
        if isinstance(request_obj, JSONResponse):
            return request_obj

        stream = getattr(request_obj, "stream", False)
        model = getattr(request_obj, "model", "unknown")

        # 添加审计元数据
        audit_metadata = self._build_audit_metadata(original_request_body, request_obj)
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
            handler = self._create_handler(
                db=db,
                user=user,
                api_key=api_key,
                request_id=request_id,
                client_ip=client_ip,
                user_agent=user_agent,
                start_time=start_time,
                perf_metrics=context.extra.get("perf"),
                api_family=self.API_FAMILY.value if self.API_FAMILY else None,
                endpoint_kind=self.ENDPOINT_KIND.value if self.ENDPOINT_KIND else None,
            )

            # 处理请求
            if stream:
                return await handler.process_stream(
                    request=request_obj,
                    http_request=http_request,
                    original_headers=original_headers,
                    original_request_body=original_request_body,
                    query_params=query_params,
                    client_content_encoding=context.client_content_encoding,
                )
            return await handler.process_sync(
                request=request_obj,
                http_request=http_request,
                original_headers=original_headers,
                original_request_body=original_request_body,
                query_params=query_params,
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
            logger.info(f"客户端请求错误: {e.error_type}")
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

    def _create_handler(
        self,
        *,
        db: Session,
        user: Any,
        api_key: Any,
        request_id: str,
        client_ip: str,
        user_agent: str,
        start_time: float,
        perf_metrics: dict[str, Any] | None = None,
        api_family: str | None = None,
        endpoint_kind: str | None = None,
    ) -> Any:
        """创建 Handler 实例 - 子类可覆盖"""
        return self.HANDLER_CLASS(
            db=db,
            user=user,
            api_key=api_key,
            request_id=request_id,
            client_ip=client_ip,
            user_agent=user_agent,
            start_time=start_time,
            allowed_api_formats=self.allowed_api_formats,
            adapter_detector=self.detect_capability_requirements,
            perf_metrics=perf_metrics,
            api_family=api_family,
            endpoint_kind=endpoint_kind,
        )

    @abstractmethod
    def _validate_request_body(
        self, original_request_body: dict, path_params: dict | None = None
    ) -> None:
        """验证请求体 - 子类必须实现"""
        pass

    def _extract_message_count(self, payload: dict[str, Any], request_obj: Any) -> int:
        """提取消息数量 - 子类可覆盖"""
        messages = payload.get("messages", [])
        if hasattr(request_obj, "messages"):
            messages = request_obj.messages
        return len(messages) if isinstance(messages, list) else 0

    def _build_audit_metadata(self, payload: dict[str, Any], request_obj: Any) -> dict[str, Any]:
        """构建审计日志元数据 - 子类可覆盖"""
        model = getattr(request_obj, "model", payload.get("model", "unknown"))
        stream = getattr(request_obj, "stream", payload.get("stream", False))
        messages_count = self._extract_message_count(payload, request_obj)

        return {
            "action": f"{self.FORMAT_ID.lower()}_request",
            "model": model,
            "stream": bool(stream),
            "max_tokens": getattr(request_obj, "max_tokens", payload.get("max_tokens")),
            "messages_count": messages_count,
            "temperature": getattr(request_obj, "temperature", payload.get("temperature")),
            "top_p": getattr(request_obj, "top_p", payload.get("top_p")),
        }


# =========================================================================
# Adapter 注册表
# =========================================================================

_ADAPTER_REGISTRY: dict[str, type[ChatAdapterBase]] = {}
_ADAPTERS_LOADED = False


def register_adapter(adapter_class: type[ChatAdapterBase]) -> type[ChatAdapterBase]:
    """
    注册 Adapter 类到注册表

    用法：
        @register_adapter
        class ClaudeChatAdapter(ChatAdapterBase):
            FORMAT_ID = "CLAUDE"
            ...

    Args:
        adapter_class: Adapter 类

    Returns:
        注册的 Adapter 类（支持作为装饰器使用）
    """
    format_id = adapter_class.FORMAT_ID
    if format_id and format_id != "UNKNOWN":
        _ADAPTER_REGISTRY[format_id.upper()] = adapter_class
    return adapter_class


def _ensure_adapters_loaded() -> None:
    """确保所有 Adapter 已被加载（触发注册）"""
    global _ADAPTERS_LOADED
    if _ADAPTERS_LOADED:
        return

    # 导入各个 Adapter 模块以触发 @register_adapter 装饰器
    try:
        from src.api.handlers.claude import adapter as _  # noqa: F401
    except ImportError:
        pass
    try:
        from src.api.handlers.openai import adapter as _  # noqa: F401
    except ImportError:
        pass
    try:
        from src.api.handlers.gemini import adapter as _  # noqa: F401
    except ImportError:
        pass

    _ADAPTERS_LOADED = True


def get_adapter_class(api_format: str) -> type[ChatAdapterBase] | None:
    """
    根据 API format 获取 Adapter 类

    Args:
        api_format: API 格式标识（如 "openai:chat", "claude:chat", "gemini:chat"）

    Returns:
        对应的 Adapter 类，如果未找到返回 None
    """
    _ensure_adapters_loaded()
    return _ADAPTER_REGISTRY.get(api_format.upper()) if api_format else None


def get_adapter_instance(api_format: str) -> ChatAdapterBase | None:
    """
    根据 API format 获取 Adapter 实例

    Args:
        api_format: API 格式标识

    Returns:
        Adapter 实例，如果未找到返回 None
    """
    adapter_class = get_adapter_class(api_format)
    if adapter_class:
        return adapter_class()
    return None


def list_registered_formats() -> list[str]:
    """返回所有已注册的 API 格式"""
    _ensure_adapters_loaded()
    return list(_ADAPTER_REGISTRY.keys())
