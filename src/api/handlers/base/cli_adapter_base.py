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

import time
import traceback
from typing import Any, Dict, Optional, Tuple, Type

import httpx
from fastapi import HTTPException
from fastapi.responses import JSONResponse

from src.api.base.adapter import ApiAdapter, ApiMode
from src.api.base.context import ApiRequestContext
from src.api.handlers.base.cli_handler_base import CliMessageHandlerBase
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
    HANDLER_CLASS: Type[CliMessageHandlerBase]

    # 适配器配置
    name: str = "cli.base"
    mode = ApiMode.PROXY

    def __init__(self, allowed_api_formats: Optional[list[str]] = None):
        self.allowed_api_formats = allowed_api_formats or [self.FORMAT_ID]

    async def handle(self, context: ApiRequestContext):
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
        logger.info(f"[REQ] {request_id[:8]} | {self.FORMAT_ID} | {getattr(api_key, 'name', 'unknown')} | "
            f"{model} | {'stream' if stream else 'sync'} | quota:{quota_display}")

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
                error_type=(
                    "invalid_request_error" if e.status_code == 400 else "quota_exceeded"
                ),
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
        self, original_request_body: Dict[str, Any], path_params: Dict[str, Any]
    ) -> Dict[str, Any]:
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

    def _extract_message_count(self, payload: Dict[str, Any]) -> int:
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
        payload: Dict[str, Any],
        path_params: Optional[Dict[str, Any]] = None,
    ) -> Dict[str, Any]:
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
        db,
        user,
        api_key,
        model: str,
        stream: bool,
        start_time: float,
        original_headers: Dict[str, str],
        original_request_body: Dict[str, Any],
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
                f"提供商认证失败: {str(e)}"
                if result.metadata.provider != "unknown"
                else "服务端错误: 无可用提供商"
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
        db,
        user,
        api_key,
        model: str,
        stream: bool,
        start_time: float,
        original_headers: Dict[str, str],
        original_request_body: Dict[str, Any],
        client_ip: str,
        request_id: str,
    ) -> JSONResponse:
        """处理未预期的异常"""
        if isinstance(e, ProxyException):
            logger.error(f"{self.FORMAT_ID} 请求处理业务异常: {type(e).__name__}")
        else:
            logger.error(f"{self.FORMAT_ID} 请求处理意外异常",
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
            status_code=500,
            error_type="internal_server_error",
            message="处理请求时发生内部错误")

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

    def get_cache_read_price_for_ttl(
        self,
        tier: dict,
        cache_ttl_minutes: Optional[int] = None,
    ) -> Optional[float]:
        """
        根据缓存 TTL 获取缓存读取价格

        默认实现：检查 cache_ttl_pricing 配置，按 TTL 选择价格
        子类可覆盖此方法实现不同的 TTL 定价逻辑

        Args:
            tier: 当前阶梯配置
            cache_ttl_minutes: 缓存时长（分钟）

        Returns:
            缓存读取价格（每 1M tokens）
        """
        ttl_pricing = tier.get("cache_ttl_pricing")
        if ttl_pricing and cache_ttl_minutes is not None:
            matched_price = None
            for ttl_config in ttl_pricing:
                ttl_limit = ttl_config.get("ttl_minutes", 0)
                if cache_ttl_minutes <= ttl_limit:
                    matched_price = ttl_config.get("cache_read_price_per_1m")
                    break
            if matched_price is not None:
                return matched_price
            # 超过所有配置的 TTL，使用最后一个
            if ttl_pricing:
                return ttl_pricing[-1].get("cache_read_price_per_1m")

        return tier.get("cache_read_price_per_1m")

    def compute_cost(
        self,
        input_tokens: int,
        output_tokens: int,
        cache_creation_input_tokens: int,
        cache_read_input_tokens: int,
        input_price_per_1m: float,
        output_price_per_1m: float,
        cache_creation_price_per_1m: Optional[float],
        cache_read_price_per_1m: Optional[float],
        price_per_request: Optional[float],
        tiered_pricing: Optional[dict] = None,
        cache_ttl_minutes: Optional[int] = None,
    ) -> Dict[str, Any]:
        """
        计算请求成本

        默认实现：支持固定价格和阶梯计费
        子类可覆盖此方法实现完全不同的计费逻辑

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
        tier_index = None
        effective_input_price = input_price_per_1m
        effective_output_price = output_price_per_1m
        effective_cache_creation_price = cache_creation_price_per_1m
        effective_cache_read_price = cache_read_price_per_1m

        # 检查阶梯计费
        if tiered_pricing and tiered_pricing.get("tiers"):
            total_input_context = self.compute_total_input_context(
                input_tokens, cache_read_input_tokens, cache_creation_input_tokens
            )
            tier = self._get_tier_for_tokens(tiered_pricing, total_input_context)

            if tier:
                tier_index = tiered_pricing["tiers"].index(tier)
                effective_input_price = tier.get("input_price_per_1m", input_price_per_1m)
                effective_output_price = tier.get("output_price_per_1m", output_price_per_1m)
                effective_cache_creation_price = tier.get(
                    "cache_creation_price_per_1m", cache_creation_price_per_1m
                )
                effective_cache_read_price = self.get_cache_read_price_for_ttl(
                    tier, cache_ttl_minutes
                )
                if effective_cache_read_price is None:
                    effective_cache_read_price = cache_read_price_per_1m

        # 计算各项成本
        input_cost = (input_tokens / 1_000_000) * effective_input_price
        output_cost = (output_tokens / 1_000_000) * effective_output_price

        cache_creation_cost = 0.0
        cache_read_cost = 0.0
        if cache_creation_input_tokens > 0 and effective_cache_creation_price is not None:
            cache_creation_cost = (
                cache_creation_input_tokens / 1_000_000
            ) * effective_cache_creation_price
        if cache_read_input_tokens > 0 and effective_cache_read_price is not None:
            cache_read_cost = (
                cache_read_input_tokens / 1_000_000
            ) * effective_cache_read_price

        cache_cost = cache_creation_cost + cache_read_cost
        request_cost = price_per_request if price_per_request else 0.0
        total_cost = input_cost + output_cost + cache_cost + request_cost

        return {
            "input_cost": input_cost,
            "output_cost": output_cost,
            "cache_creation_cost": cache_creation_cost,
            "cache_read_cost": cache_read_cost,
            "cache_cost": cache_cost,
            "request_cost": request_cost,
            "total_cost": total_cost,
            "tier_index": tier_index,
        }

    @staticmethod
    def _get_tier_for_tokens(tiered_pricing: dict, total_input_tokens: int) -> Optional[dict]:
        """根据总输入 token 数确定价格阶梯"""
        if not tiered_pricing or "tiers" not in tiered_pricing:
            return None

        tiers = tiered_pricing.get("tiers", [])
        if not tiers:
            return None

        for tier in tiers:
            up_to = tier.get("up_to")
            if up_to is None or total_input_tokens <= up_to:
                return tier

        return tiers[-1] if tiers else None

    # =========================================================================
    # 模型列表查询 - 子类应覆盖此方法
    # =========================================================================

    @classmethod
    async def fetch_models(
        cls,
        client: httpx.AsyncClient,
        base_url: str,
        api_key: str,
        extra_headers: Optional[Dict[str, str]] = None,
    ) -> Tuple[list, Optional[str]]:
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
        request_data: Dict[str, Any],
        extra_headers: Optional[Dict[str, str]] = None,
        # 用量计算参数
        db: Optional[Any] = None,
        user: Optional[Any] = None,
        provider_name: Optional[str] = None,
        provider_id: Optional[str] = None,
        api_key_id: Optional[str] = None,
        model_name: Optional[str] = None,
    ) -> Dict[str, Any]:
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
            db: 数据库会话
            user: 用户对象
            provider_name: 提供商名称
            provider_id: 提供商ID
            api_key_id: API密钥ID
            model_name: 模型名称

        Returns:
            测试响应数据
        """
        from src.api.handlers.base.endpoint_checker import build_safe_headers, run_endpoint_check

        # 构建请求组件
        url = cls.build_endpoint_url(base_url, request_data, model_name)
        base_headers = cls.build_base_headers(api_key)
        protected_keys = cls.get_protected_header_keys()

        # 添加CLI User-Agent
        cli_user_agent = cls.get_cli_user_agent()
        if cli_user_agent:
            base_headers["User-Agent"] = cli_user_agent
            protected_keys = tuple(list(protected_keys) + ["user-agent"])

        headers = build_safe_headers(base_headers, extra_headers, protected_keys)
        body = cls.build_request_body(request_data)

        # 获取有效的模型名称
        effective_model_name = model_name or request_data.get("model")

        return await run_endpoint_check(
            client=client,
            url=url,
            headers=headers,
            json_body=body,
            api_format=cls.name,
            # 用量计算参数（现在强制记录）
            db=db,
            user=user,
            provider_name=provider_name,
            provider_id=provider_id,
            api_key_id=api_key_id,
            model_name=effective_model_name,
        )

    # =========================================================================
    # CLI Adapter 配置方法 - 子类应覆盖这些方法而不是整个 check_endpoint
    # =========================================================================

    @classmethod
    def build_endpoint_url(cls, base_url: str, request_data: Dict[str, Any], model_name: Optional[str] = None) -> str:
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
    def build_base_headers(cls, api_key: str) -> Dict[str, str]:
        """
        构建CLI API认证头 - 子类应覆盖

        Args:
            api_key: API密钥

        Returns:
            基础认证头部字典
        """
        raise NotImplementedError(f"{cls.FORMAT_ID} adapter must implement build_base_headers")

    @classmethod
    def get_protected_header_keys(cls) -> tuple:
        """
        返回CLI API的保护头部key - 子类应覆盖

        Returns:
            保护头部key的元组
        """
        raise NotImplementedError(f"{cls.FORMAT_ID} adapter must implement get_protected_header_keys")

    @classmethod
    def build_request_body(cls, request_data: Dict[str, Any]) -> Dict[str, Any]:
        """
        构建CLI API请求体 - 子类应覆盖

        Args:
            request_data: 请求数据

        Returns:
            请求体字典
        """
        raise NotImplementedError(f"{cls.FORMAT_ID} adapter must implement build_request_body")

    @classmethod
    def get_cli_user_agent(cls) -> Optional[str]:
        """
        获取CLI User-Agent - 子类可覆盖

        Returns:
            CLI User-Agent字符串，如果不需要则为None
        """
        return None

# =========================================================================
# CLI Adapter 注册表 - 用于根据 API format 获取 CLI Adapter 实例
# =========================================================================

_CLI_ADAPTER_REGISTRY: Dict[str, Type["CliAdapterBase"]] = {}
_CLI_ADAPTERS_LOADED = False


def register_cli_adapter(adapter_class: Type["CliAdapterBase"]) -> Type["CliAdapterBase"]:
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


def _ensure_cli_adapters_loaded():
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


def get_cli_adapter_class(api_format: str) -> Optional[Type["CliAdapterBase"]]:
    """根据 API format 获取 CLI Adapter 类"""
    _ensure_cli_adapters_loaded()
    return _CLI_ADAPTER_REGISTRY.get(api_format.upper()) if api_format else None


def get_cli_adapter_instance(api_format: str) -> Optional["CliAdapterBase"]:
    """根据 API format 获取 CLI Adapter 实例"""
    adapter_class = get_cli_adapter_class(api_format)
    if adapter_class:
        return adapter_class()
    return None


def list_registered_cli_formats() -> list[str]:
    """返回所有已注册的 CLI API 格式"""
    _ensure_cli_adapters_loaded()
    return list(_CLI_ADAPTER_REGISTRY.keys())
