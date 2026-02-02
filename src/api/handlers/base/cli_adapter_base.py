"""
CLI Adapter 通用基类

提供 CLI 格式（直接透传请求）的通用适配器逻辑：
- 请求解析和验证
- 审计日志记录
- 错误处理和响应格式化
- Handler 创建和调用
- 计费策略（支持不同 API 格式的差异化计费）

子类只需提供：
- FORMAT_ID: API 格式标识
- HANDLER_CLASS: 对应的 MessageHandler 类
- 可选覆盖 _extract_message_count() 自定义消息计数逻辑
- 可选覆盖 compute_total_input_context() 自定义总输入上下文计算
"""

from __future__ import annotations

import time
import traceback
from typing import Any, ClassVar

import httpx
from fastapi import HTTPException, Request
from fastapi.responses import JSONResponse
from sqlalchemy.orm import Session

from src.api.base.adapter import ApiAdapter, ApiMode
from src.api.base.context import ApiRequestContext
from src.api.handlers.base.cli_handler_base import CliMessageHandlerBase
from src.core.api_format import (
    ApiFamily,
    EndpointKind,
    build_adapter_base_headers_for_endpoint,
    build_adapter_headers_for_endpoint,
    get_adapter_protected_keys_for_endpoint,
    get_auth_handler,
    get_default_auth_method_for_endpoint,
)
from src.core.exceptions import (
    InvalidRequestException,
    ModelNotSupportedException,
    ProviderAuthException,
    ProviderNotAvailableException,
    ProviderRateLimitException,
    ProviderTimeoutException,
    ProxyException,
    QuotaExceededException,
    UpstreamClientException,
)
from src.core.logger import logger
from src.services.billing import calculate_request_cost as _calculate_request_cost
from src.services.request.result import RequestResult
from src.services.usage.recorder import UsageRecorder


class CliAdapterBase(ApiAdapter):
    """
    CLI Adapter 通用基类

    提供 CLI 格式的通用适配器逻辑，子类只需配置：
    - FORMAT_ID: API 格式标识
    - HANDLER_CLASS: MessageHandler 类
    - name: 适配器名称
    """

    # 子类必须覆盖
    FORMAT_ID: str = "UNKNOWN"
    HANDLER_CLASS: type[CliMessageHandlerBase]

    # 新架构：结构化标识（逐步替代直接依赖 FORMAT_ID 的语义）
    API_FAMILY: ClassVar[ApiFamily | None] = None
    ENDPOINT_KIND: ClassVar[EndpointKind] = EndpointKind.CLI

    # 适配器配置
    name: str = "cli.base"
    mode = ApiMode.PROXY

    # 计费模板配置（子类可覆盖，如 "claude", "openai", "gemini"）
    BILLING_TEMPLATE: str = "claude"

    def __init__(self, allowed_api_formats: list[str] | None = None):
        self.allowed_api_formats = allowed_api_formats or [self.FORMAT_ID]

    # =========================================================================
    # API 格式与头部处理 - 使用统一的 headers.py 函数
    # =========================================================================

    def extract_api_key(self, request: Request) -> str | None:
        """
        从请求中提取 API 密钥

        使用 AuthHandler 新流程，根据 API 格式选择认证方式。
        """
        auth_method = get_default_auth_method_for_endpoint(self.FORMAT_ID)
        handler = get_auth_handler(auth_method)
        return handler.extract_credentials(request)

    @classmethod
    def build_base_headers(cls, api_key: str) -> dict[str, str]:
        """
        构建 CLI API 认证头

        使用统一的头部处理函数。
        """
        return build_adapter_base_headers_for_endpoint(cls.FORMAT_ID, api_key)

    @classmethod
    def build_headers_with_extra(
        cls, api_key: str, extra_headers: dict[str, str] | None = None
    ) -> dict[str, str]:
        """
        构建带额外头部的完整请求头

        使用统一的头部处理函数，自动保护关键头部不被覆盖。
        """
        return build_adapter_headers_for_endpoint(cls.FORMAT_ID, api_key, extra_headers)

    @classmethod
    def get_protected_header_keys(cls) -> tuple[str, ...]:
        """
        返回 CLI API 的保护头部 key

        使用统一的头部处理函数。
        """
        return get_adapter_protected_keys_for_endpoint(cls.FORMAT_ID)

    async def handle(self, context: ApiRequestContext) -> Any:
        """处理 CLI API 请求"""
        http_request = context.request
        user = context.user
        api_key = context.api_key
        db = context.db
        request_id = context.request_id
        quota_remaining_value = context.quota_remaining
        start_time = context.start_time
        client_ip = context.client_ip
        user_agent = context.user_agent
        original_headers = context.original_headers
        query_params = context.query_params  # 获取查询参数

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
        quota_display = (
            "unlimited" if quota_remaining_value is None else f"${quota_remaining_value:.2f}"
        )

        # 请求开始日志
        logger.info(
            f"[REQ] {request_id[:8]} | {self.FORMAT_ID} | {getattr(api_key, 'name', 'unknown')} | "
            f"{model} | {'stream' if stream else 'sync'} | quota:{quota_display}"
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
            )

            # 处理请求
            if stream:
                return await handler.process_stream(
                    original_request_body=original_request_body,
                    original_headers=original_headers,
                    query_params=query_params,
                    path_params=context.path_params,
                    http_request=http_request,
                )
            return await handler.process_sync(
                original_request_body=original_request_body,
                original_headers=original_headers,
                query_params=query_params,
                path_params=context.path_params,
            )

        except HTTPException:
            raise

        except (
            ModelNotSupportedException,
            QuotaExceededException,
            InvalidRequestException,
        ) as e:
            logger.debug(f"客户端请求错误: {e.error_type}")
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

    def _merge_path_params(
        self, original_request_body: dict[str, Any], path_params: dict[str, Any]
    ) -> dict[str, Any]:
        """
        合并 URL 路径参数到请求体 - 子类可覆盖

        默认实现：直接将 path_params 中的字段合并到请求体（不覆盖已有字段）

        Args:
            original_request_body: 原始请求体字典
            path_params: URL 路径参数字典

        Returns:
            合并后的请求体字典
        """
        merged = original_request_body.copy()
        for key, value in path_params.items():
            if key not in merged:
                merged[key] = value
        return merged

    def _extract_message_count(self, payload: dict[str, Any]) -> int:
        """
        提取消息数量 - 子类可覆盖

        默认实现：从 input 字段提取
        """
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
        """
        构建审计日志元数据 - 子类可覆盖

        Args:
            payload: 请求体
            path_params: URL 路径参数（用于获取 model 等）
        """
        # 优先从请求体获取 model，其次从 path_params
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

    async def _handle_provider_exception(
        self,
        e: Exception,
        *,
        db: Session,
        user: Any,
        api_key: Any,
        model: str,
        stream: bool,
        start_time: float,
        original_headers: dict[str, str],
        original_request_body: dict[str, Any],
        client_ip: str,
        request_id: str,
    ) -> JSONResponse:
        """处理 Provider 相关异常"""
        logger.debug(f"Caught provider exception: {type(e).__name__}")

        response_time = int((time.time() - start_time) * 1000)

        # 使用 RequestResult.from_exception 创建统一的失败结果
        # 关键：api_format 从 FORMAT_ID 获取，确保始终有值
        result = RequestResult.from_exception(
            exception=e,
            api_format=self.FORMAT_ID,  # 使用 Adapter 的 FORMAT_ID 作为默认值
            model=model,
            response_time_ms=response_time,
            is_stream=stream,
        )
        result.request_headers = original_headers
        result.request_body = original_request_body

        # 确定错误消息
        if isinstance(e, ProviderAuthException):
            error_message = (
                "上游服务认证失败" if result.metadata.provider != "unknown" else "服务暂时不可用"
            )
            result.error_message = error_message

        # 处理上游客户端错误（如图片处理失败）
        if isinstance(e, UpstreamClientException):
            # 返回 400 状态码和清晰的错误消息
            result.status_code = e.status_code
            result.error_message = e.message

        # 使用 UsageRecorder 记录失败
        recorder = UsageRecorder(
            db=db,
            user=user,
            api_key=api_key,
            client_ip=client_ip,
            request_id=request_id,
        )
        await recorder.record_failure(result, original_headers, original_request_body)

        # 根据异常类型确定错误类型
        if isinstance(e, UpstreamClientException):
            error_type = "invalid_request_error"
        elif result.status_code == 503:
            error_type = "internal_server_error"
        else:
            error_type = "rate_limit_exceeded"

        return self._error_response(
            status_code=result.status_code,
            error_type=error_type,
            message=result.error_message or str(e),
        )

    async def _handle_unexpected_exception(
        self,
        e: Exception,
        *,
        db: Session,
        user: Any,
        api_key: Any,
        model: str,
        stream: bool,
        start_time: float,
        original_headers: dict[str, str],
        original_request_body: dict[str, Any],
        client_ip: str,
        request_id: str,
    ) -> JSONResponse:
        """处理未预期的异常"""
        if isinstance(e, ProxyException):
            logger.error(f"{self.FORMAT_ID} 请求处理业务异常: {type(e).__name__}")
        else:
            logger.error(
                f"{self.FORMAT_ID} 请求处理意外异常",
                exception=e,
                extra_data={
                    "exception_class": e.__class__.__name__,
                    "processing_stage": "request_processing",
                    "model": model,
                    "stream": stream,
                    "traceback_preview": str(traceback.format_exc())[:500],
                },
            )

        response_time = int((time.time() - start_time) * 1000)

        # 使用 RequestResult.from_exception 创建统一的失败结果
        # 关键：api_format 从 FORMAT_ID 获取，确保始终有值
        result = RequestResult.from_exception(
            exception=e,
            api_format=self.FORMAT_ID,  # 使用 Adapter 的 FORMAT_ID 作为默认值
            model=model,
            response_time_ms=response_time,
            is_stream=stream,
        )
        # 对于未预期的异常，强制设置状态码为 500
        result.status_code = 500
        result.error_type = "internal_error"
        result.request_headers = original_headers
        result.request_body = original_request_body

        # 使用 UsageRecorder 记录失败
        recorder = UsageRecorder(
            db=db,
            user=user,
            api_key=api_key,
            client_ip=client_ip,
            request_id=request_id,
        )
        await recorder.record_failure(result, original_headers, original_request_body)

        return self._error_response(
            status_code=500, error_type="internal_server_error", message="处理请求时发生内部错误"
        )

    def _error_response(self, status_code: int, error_type: str, message: str) -> JSONResponse:
        """生成错误响应"""
        return JSONResponse(
            status_code=status_code,
            content={
                "error": {
                    "type": error_type,
                    "message": message,
                }
            },
        )

    # =========================================================================
    # 计费策略相关方法 - 子类可覆盖以实现不同 API 格式的差异化计费
    # =========================================================================

    def compute_total_input_context(
        self,
        input_tokens: int,
        cache_read_input_tokens: int,
        cache_creation_input_tokens: int = 0,
    ) -> int:
        """
        计算总输入上下文（用于阶梯计费判定）

        默认实现：input_tokens + cache_read_input_tokens
        子类可覆盖此方法实现不同的计算逻辑

        Args:
            input_tokens: 输入 token 数
            cache_read_input_tokens: 缓存读取 token 数
            cache_creation_input_tokens: 缓存创建 token 数（部分格式可能需要）

        Returns:
            总输入上下文 token 数
        """
        return input_tokens + cache_read_input_tokens

    def compute_cost(
        self,
        input_tokens: int,
        output_tokens: int,
        cache_creation_input_tokens: int,
        cache_read_input_tokens: int,
        input_price_per_1m: float,
        output_price_per_1m: float,
        cache_creation_price_per_1m: float | None,
        cache_read_price_per_1m: float | None,
        price_per_request: float | None,
        tiered_pricing: dict | None = None,
        cache_ttl_minutes: int | None = None,
    ) -> dict[str, Any]:
        """
        计算请求成本

        使用 billing 模块的配置驱动计费。
        子类可通过设置 BILLING_TEMPLATE 类属性来指定计费模板，
        或覆盖此方法实现完全自定义的计费逻辑。

        Args:
            input_tokens: 输入 token 数
            output_tokens: 输出 token 数
            cache_creation_input_tokens: 缓存创建 token 数
            cache_read_input_tokens: 缓存读取 token 数
            input_price_per_1m: 输入价格（每 1M tokens）
            output_price_per_1m: 输出价格（每 1M tokens）
            cache_creation_price_per_1m: 缓存创建价格（每 1M tokens）
            cache_read_price_per_1m: 缓存读取价格（每 1M tokens）
            price_per_request: 按次计费价格
            tiered_pricing: 阶梯计费配置
            cache_ttl_minutes: 缓存时长（分钟）

        Returns:
            包含各项成本的字典
        """
        # 计算总输入上下文（使用子类可覆盖的方法）
        total_input_context = self.compute_total_input_context(
            input_tokens, cache_read_input_tokens, cache_creation_input_tokens
        )

        return _calculate_request_cost(
            input_tokens=input_tokens,
            output_tokens=output_tokens,
            cache_creation_input_tokens=cache_creation_input_tokens,
            cache_read_input_tokens=cache_read_input_tokens,
            input_price_per_1m=input_price_per_1m,
            output_price_per_1m=output_price_per_1m,
            cache_creation_price_per_1m=cache_creation_price_per_1m,
            cache_read_price_per_1m=cache_read_price_per_1m,
            price_per_request=price_per_request,
            tiered_pricing=tiered_pricing,
            cache_ttl_minutes=cache_ttl_minutes,
            total_input_context=total_input_context,
            billing_template=self.BILLING_TEMPLATE,
        )

    # =========================================================================
    # 模型列表查询 - 子类应覆盖此方法
    # =========================================================================

    @classmethod
    async def fetch_models(
        cls,
        client: httpx.AsyncClient,
        base_url: str,
        api_key: str,
        extra_headers: dict[str, str] | None = None,
    ) -> tuple[list, str | None]:
        """
        查询上游 API 支持的模型列表

        这是 Aether 内部发起的请求（非用户透传），用于：
        - 管理后台查询提供商支持的模型
        - 自动发现可用模型

        Args:
            client: httpx 异步客户端
            base_url: API 基础 URL
            api_key: API 密钥（已解密）
            extra_headers: 端点配置的额外请求头

        Returns:
            (models, error): 模型列表和错误信息
            - models: 模型信息列表，每个模型至少包含 id 字段
            - error: 错误信息，成功时为 None
        """
        # 默认实现返回空列表，子类应覆盖
        return [], f"{cls.FORMAT_ID} adapter does not implement fetch_models"

    @classmethod
    async def check_endpoint(
        cls,
        client: httpx.AsyncClient,
        base_url: str,
        api_key: str,
        request_data: dict[str, Any],
        extra_headers: dict[str, str] | None = None,
        # 端点规则参数
        body_rules: list[dict[str, Any]] | None = None,
        header_rules: list[dict[str, Any]] | None = None,
        # 用量计算参数
        db: Any | None = None,
        user: Any | None = None,
        provider_name: str | None = None,
        provider_id: str | None = None,
        api_key_id: str | None = None,
        model_name: str | None = None,
    ) -> dict[str, Any]:
        """
        测试模型连接性（非流式）

        通用的CLI endpoint测试方法，使用配置方法模式：
        - build_endpoint_url(): 构建请求URL
        - build_base_headers(): 构建基础认证头
        - get_protected_header_keys(): 获取受保护的头部key
        - build_request_body(): 构建请求体
        - get_cli_user_agent(): 获取CLI User-Agent（子类可覆盖）

        Args:
            client: httpx 异步客户端
            base_url: API 基础 URL
            api_key: API 密钥（已解密）
            request_data: 请求数据
            extra_headers: 端点配置的额外请求头
            body_rules: 请求体规则（在格式转换后应用）
            header_rules: 请求头规则（在请求头构建后应用）
            db: 数据库会话
            user: 用户对象
            provider_name: 提供商名称
            provider_id: 提供商ID
            api_key_id: API密钥ID
            model_name: 模型名称

        Returns:
            测试响应数据
        """
        from src.api.handlers.base.endpoint_checker import run_endpoint_check
        from src.api.handlers.base.request_builder import apply_body_rules
        from src.core.api_format.headers import HeaderBuilder

        # 构建请求组件
        url = cls.build_endpoint_url(base_url, request_data, model_name)

        # 合并 CLI 额外头部到 extra_headers
        cli_extra = cls.get_cli_extra_headers()
        merged_extra = dict(extra_headers) if extra_headers else {}
        merged_extra.update(cli_extra)

        # 使用统一的头部构建函数
        headers = cls.build_headers_with_extra(api_key, merged_extra if merged_extra else None)
        body = cls.build_request_body(request_data)

        # 应用请求体规则（在格式转换后应用，确保规则效果不被覆盖）
        if body_rules:
            body = apply_body_rules(body, body_rules)

        # 应用请求头规则（在请求头构建后应用）
        if header_rules:
            # 获取认证头名称，防止被规则覆盖
            from src.core.api_format import get_auth_config_for_endpoint
            auth_header, _ = get_auth_config_for_endpoint(cls.FORMAT_ID)
            protected_keys = {auth_header.lower(), "content-type"}

            header_builder = HeaderBuilder()
            header_builder.add_many(headers)
            header_builder.apply_rules(header_rules, protected_keys)
            headers = header_builder.build()

        # 获取有效的模型名称
        effective_model_name = model_name or request_data.get("model")

        return await run_endpoint_check(
            client=client,
            url=url,
            headers=headers,
            json_body=body,
            api_format=cls.FORMAT_ID,
            # 用量计算参数（现在强制记录）
            db=db,
            user=user,
            provider_name=provider_name,
            provider_id=provider_id,
            api_key_id=api_key_id,
            model_name=effective_model_name,
        )

    # =========================================================================
    # CLI Adapter 配置方法 - 子类应覆盖这些方法
    # =========================================================================

    @classmethod
    def build_endpoint_url(
        cls, base_url: str, request_data: dict[str, Any], model_name: str | None = None
    ) -> str:
        """
        构建CLI API端点URL - 子类应覆盖

        Args:
            base_url: API基础URL
            request_data: 请求数据
            model_name: 模型名称（某些API需要，如Gemini）

        Returns:
            完整的端点URL
        """
        raise NotImplementedError(f"{cls.FORMAT_ID} adapter must implement build_endpoint_url")

    @classmethod
    def build_request_body(cls, request_data: dict[str, Any] | None = None) -> dict[str, Any]:
        """构建测试请求体，使用转换器注册表自动处理格式转换

        Args:
            request_data: 可选的请求数据，会与默认测试请求合并

        Returns:
            转换为目标 API 格式的请求体
        """
        from src.api.handlers.base.request_builder import build_test_request_body

        return build_test_request_body(cls.FORMAT_ID, request_data)

    @classmethod
    def get_cli_user_agent(cls) -> str | None:
        """
        获取CLI User-Agent - 子类可覆盖

        Returns:
            CLI User-Agent字符串，如果不需要则为None
        """
        return None

    @classmethod
    def get_cli_extra_headers(cls) -> dict[str, str]:
        """
        获取CLI额外请求头 - 子类可覆盖

        用于 check_endpoint 测试请求时添加额外的头部。
        默认实现只添加 User-Agent（如果有）。

        Returns:
            额外请求头字典
        """
        headers: dict[str, str] = {}
        cli_user_agent = cls.get_cli_user_agent()
        if cli_user_agent:
            headers["User-Agent"] = cli_user_agent
        return headers


# =========================================================================
# CLI Adapter 注册表 - 用于根据 API format 获取 CLI Adapter 实例
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

    # 导入各个 CLI Adapter 模块以触发 @register_cli_adapter 装饰器
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
    """
    根据 API format 获取 CLI Adapter 类

    Args:
        api_format: API 格式标识（如 "openai:cli", "claude:cli", "gemini:cli"）

    Returns:
        对应的 CLI Adapter 类，如果未找到返回 None
    """
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
