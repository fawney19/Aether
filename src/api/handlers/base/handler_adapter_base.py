"""
Handler Adapter 公共基类

从 ChatAdapterBase 和 CliAdapterBase 提取的共享逻辑：
- API 格式与头部处理
- 异常处理和错误响应
- 通过 `core.api_format` 注册表解析计费模板与抓模能力
- 端点测试辅助
- 路径参数合并

子类（ChatAdapterBase / CliAdapterBase）只需关注各自的 `handle()` 流程差异。
"""

from __future__ import annotations

import time
from typing import Any, ClassVar

import httpx
from fastapi import Request
from fastapi.responses import JSONResponse
from sqlalchemy.orm import Session

from src.api.base.adapter import ApiAdapter
from src.core.api_format import (
    ApiFamily,
    EndpointKind,
    build_adapter_base_headers_for_endpoint,
    build_adapter_headers_for_endpoint,
    compute_total_input_context_for_api_format,
    fetch_models_for_api_format,
    get_adapter_protected_keys_for_endpoint,
    get_auth_handler,
    get_default_auth_method_for_endpoint,
    resolve_billing_template_for_api_format,
    resolve_header_name_case,
)
from src.core.exceptions import (
    ProviderAuthException,
    ProxyException,
    UpstreamClientException,
)
from src.core.logger import logger
from src.services.billing import calculate_request_cost as _calculate_request_cost
from src.services.request.result import RequestResult
from src.services.usage.recorder import UsageRecorder


class HandlerAdapterBase(ApiAdapter):
    """
    Chat/CLI Adapter 的公共基类

    封装两者共享的逻辑：
    - API 格式与头部处理
    - 异常处理和错误响应
    - 通过 `core.api_format` 注册表解析计费模板与模型抓取能力
    - 端点测试辅助

    子类（ChatAdapterBase / CliAdapterBase）只需实现 `handle()` 和格式特有的方法。
    """

    # 子类必须覆盖
    FORMAT_ID: str = "UNKNOWN"

    # 结构化标识
    API_FAMILY: ClassVar[ApiFamily | None] = None
    ENDPOINT_KIND: ClassVar[EndpointKind] = EndpointKind.CHAT

    # 兼容性回退：若 api_format 注册表未声明计费模板，则使用该默认值。
    BILLING_TEMPLATE: str = "claude"

    def __init__(self, allowed_api_formats: list[str] | None = None):
        self.allowed_api_formats = allowed_api_formats or [self.FORMAT_ID]

    # =========================================================================
    # API 格式与头部处理
    # =========================================================================

    def extract_api_key(self, request: Request) -> str | None:
        """从请求中提取 API 密钥"""
        auth_method = get_default_auth_method_for_endpoint(self.FORMAT_ID)
        handler = get_auth_handler(auth_method)
        return handler.extract_credentials(request)

    @classmethod
    def build_base_headers(cls, api_key: str) -> dict[str, str]:
        """构建基础认证头"""
        return build_adapter_base_headers_for_endpoint(cls.FORMAT_ID, api_key)

    @classmethod
    def build_headers_with_extra(
        cls, api_key: str, extra_headers: dict[str, str] | None = None
    ) -> dict[str, str]:
        """构建带额外头部的完整请求头"""
        return build_adapter_headers_for_endpoint(cls.FORMAT_ID, api_key, extra_headers)

    @classmethod
    def get_protected_header_keys(cls) -> tuple[str, ...]:
        """返回不应被 extra_headers 覆盖的头部 key"""
        return get_adapter_protected_keys_for_endpoint(cls.FORMAT_ID)

    # =========================================================================
    # 路径参数合并
    # =========================================================================

    def _merge_path_params(
        self, original_request_body: dict[str, Any], path_params: dict[str, Any]
    ) -> dict[str, Any]:
        """合并 URL 路径参数到请求体 - 子类可覆盖"""
        merged = original_request_body.copy()
        for key, value in path_params.items():
            if key not in merged:
                merged[key] = value
        return merged

    # =========================================================================
    # 异常处理
    # =========================================================================

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
        logger.debug("Caught provider exception: {}", type(e).__name__)

        response_time = int((time.time() - start_time) * 1000)

        result = RequestResult.from_exception(
            exception=e,
            api_format=self.FORMAT_ID,
            model=model,
            response_time_ms=response_time,
            is_stream=stream,
        )
        result.request_headers = original_headers
        result.request_body = original_request_body

        if isinstance(e, ProviderAuthException):
            error_message = (
                "上游服务认证失败" if result.metadata.provider != "unknown" else "服务暂时不可用"
            )
            result.error_message = error_message

        if isinstance(e, UpstreamClientException):
            result.status_code = e.status_code
            result.error_message = e.message

        recorder = UsageRecorder(
            db=db,
            user=user,
            api_key=api_key,
            client_ip=client_ip,
            request_id=request_id,
        )
        await recorder.record_failure(result, original_headers, original_request_body)

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
            logger.error("{} 请求处理业务异常: {}: {}", self.FORMAT_ID, type(e).__name__, e)
        else:
            logger.opt(exception=e).error(
                "{} 请求处理意外异常: {}: {}", self.FORMAT_ID, type(e).__name__, e
            )

        response_time = int((time.time() - start_time) * 1000)

        result = RequestResult.from_exception(
            exception=e,
            api_format=self.FORMAT_ID,
            model=model,
            response_time_ms=response_time,
            is_stream=stream,
        )
        result.status_code = 500
        result.error_type = "internal_error"
        result.request_headers = original_headers
        result.request_body = original_request_body

        try:
            recorder = UsageRecorder(
                db=db,
                user=user,
                api_key=api_key,
                client_ip=client_ip,
                request_id=request_id,
            )
            await recorder.record_failure(result, original_headers, original_request_body)
        except Exception as record_error:
            logger.error("记录失败请求时出错: {}", record_error)

        return self._error_response(
            status_code=500, error_type="internal_server_error", message="处理请求时发生内部错误"
        )

    def _error_response(self, status_code: int, error_type: str, message: str) -> JSONResponse:
        """生成错误响应 - 子类可覆盖以自定义格式"""
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
    # 计费能力委托
    # =========================================================================

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
        """计算请求成本"""
        total_input_context = compute_total_input_context_for_api_format(
            self.FORMAT_ID, input_tokens, cache_read_input_tokens, cache_creation_input_tokens
        )
        billing_template = (
            resolve_billing_template_for_api_format(self.FORMAT_ID) or self.BILLING_TEMPLATE
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
            billing_template=billing_template,
        )

    # =========================================================================
    # 模型抓取委托与端点测试
    # =========================================================================

    @classmethod
    async def fetch_models(
        cls,
        client: httpx.AsyncClient,
        base_url: str,
        api_key: str,
        extra_headers: dict[str, str] | None = None,
    ) -> tuple[list, str | None]:
        """查询上游 API 支持的模型列表。"""
        return await fetch_models_for_api_format(
            client,
            api_format=cls.FORMAT_ID,
            base_url=base_url,
            api_key=api_key,
            extra_headers=extra_headers,
        )

    @classmethod
    def build_request_body(
        cls,
        request_data: dict[str, Any] | None = None,
        *,
        base_url: str | None = None,
        provider_type: str | None = None,
    ) -> dict[str, Any]:
        """构建测试请求体，使用转换器注册表自动处理格式转换"""
        from src.api.handlers.base.request_builder import build_test_request_body

        _ = base_url, provider_type
        return build_test_request_body(cls.FORMAT_ID, request_data)

    @staticmethod
    def _validate_test_base_url(base_url: Any) -> str:
        """校验 test-model 场景传入的 base_url。"""
        if not isinstance(base_url, str):
            raise TypeError(f"base_url must be a non-empty string, got {type(base_url).__name__}")

        normalized = base_url.strip()
        if not normalized:
            raise ValueError("base_url must be a non-empty string")
        return normalized

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
        # Provider 上下文（用于 OAuth 认证和特殊路由）
        auth_type: str | None = None,
        provider_type: str | None = None,
        decrypted_auth_config: dict[str, Any] | None = None,
        provider_endpoint: Any | None = None,
        provider_api_key: Any | None = None,
        # 代理配置
        proxy_config: dict[str, Any] | None = None,
        timeout_seconds: float | None = None,
    ) -> dict[str, Any]:
        """
        测试模型连接性（非流式）

        统一的 endpoint 测试方法，支持 OAuth/Antigravity/Kiro 等特殊路由。
        """
        from src.api.handlers.base.endpoint_checker import run_endpoint_check
        from src.api.handlers.base.request_builder import (
            apply_body_rules,
            evaluate_condition,
            get_cache_sensitive_protected_body_keys,
        )
        from src.core.api_format.headers import HeaderBuilder
        from src.core.provider_types import ProviderType

        validated_base_url = cls._validate_test_base_url(base_url)
        is_antigravity = provider_type == ProviderType.ANTIGRAVITY
        is_gemini_cli = provider_type == ProviderType.GEMINI_CLI
        is_vertex = provider_type == ProviderType.VERTEX_AI
        is_kiro = provider_type == ProviderType.KIRO
        is_oauth = auth_type == "oauth"
        vertex_auth_info: Any | None = None

        # ---- URL ----
        if is_kiro:
            from src.services.provider.adapters.kiro.constants import (
                KIRO_GENERATE_ASSISTANT_PATH,
            )
            from src.services.provider.adapters.kiro.models.credentials import KiroAuthConfig

            _kiro_cfg = KiroAuthConfig.from_dict(decrypted_auth_config or {})
            region = _kiro_cfg.effective_api_region()
            effective_base_url = (
                validated_base_url.replace("{region}", region)
                if "{region}" in validated_base_url
                else validated_base_url
            )
            url = f"{str(effective_base_url).rstrip('/')}{KIRO_GENERATE_ASSISTANT_PATH}"
        elif is_antigravity:
            from src.services.provider.adapters.antigravity.constants import (
                V1INTERNAL_PATH_TEMPLATE,
                get_v1internal_extra_headers,
            )
            from src.services.provider.adapters.antigravity.url_availability import (
                url_availability,
            )

            ordered_urls = url_availability.get_ordered_urls(prefer_daily=True)
            effective_base_url = ordered_urls[0] if ordered_urls else validated_base_url
            path = V1INTERNAL_PATH_TEMPLATE.format(action="generateContent")
            url = f"{str(effective_base_url).rstrip('/')}{path}"
        elif is_gemini_cli:
            from src.services.provider.adapters.gemini_cli.constants import V1INTERNAL_PATH_TEMPLATE

            effective_base_url = validated_base_url
            path = V1INTERNAL_PATH_TEMPLATE.format(action="generateContent")
            url = f"{str(effective_base_url).rstrip('/')}{path}"
        elif is_vertex and provider_endpoint is not None and provider_api_key is not None:
            from src.services.provider.auth import get_provider_auth
            from src.services.provider.transport import build_provider_url

            vertex_auth_info = await get_provider_auth(provider_endpoint, provider_api_key)
            effective_auth_config = (
                vertex_auth_info.decrypted_auth_config
                if vertex_auth_info
                else decrypted_auth_config
            )
            if effective_auth_config:
                decrypted_auth_config = effective_auth_config

            effective_model_name = model_name or request_data.get("model", "")
            path_params = {"model": effective_model_name} if effective_model_name else None
            url = build_provider_url(
                provider_endpoint,
                path_params=path_params,
                is_stream=bool(request_data.get("stream", False)),
                key=provider_api_key,
                decrypted_auth_config=effective_auth_config,
            )
        else:
            url = cls.build_endpoint_url(
                validated_base_url,
                request_data,
                model_name,
                provider_type=provider_type,
            )

        # ---- Headers ----
        cli_extra = cls.get_cli_extra_headers(
            base_url=validated_base_url,
            provider_type=provider_type,
        )
        merged_extra = dict(extra_headers) if extra_headers else {}
        merged_extra.update(cli_extra)

        if is_antigravity:
            merged_extra.update(get_v1internal_extra_headers())
        elif is_gemini_cli:
            from src.services.provider.adapters.gemini_cli.constants import (
                get_v1internal_extra_headers,
            )

            merged_extra.update(get_v1internal_extra_headers())

        if is_kiro:
            from src.services.provider.adapters.kiro.headers import (
                build_generate_assistant_headers,
            )
            from src.services.provider.adapters.kiro.models.credentials import KiroAuthConfig
            from src.services.provider.adapters.kiro.token_manager import generate_machine_id

            kiro_cfg = KiroAuthConfig.from_dict(decrypted_auth_config or {})
            region = kiro_cfg.effective_api_region()
            machine_id = generate_machine_id(kiro_cfg)
            kiro_headers = build_generate_assistant_headers(
                host=f"q.{region}.amazonaws.com",
                access_token=api_key,
                machine_id=machine_id,
                kiro_version=kiro_cfg.kiro_version,
                system_version=kiro_cfg.system_version,
                node_version=kiro_cfg.node_version,
            )
            merged_extra.update(kiro_headers)

        if is_vertex and provider_endpoint is not None and provider_api_key is not None:
            headers = dict(merged_extra)
            if (
                vertex_auth_info
                and getattr(vertex_auth_info, "auth_header", None)
                and getattr(vertex_auth_info, "auth_value", None)
            ):
                headers[str(vertex_auth_info.auth_header)] = str(vertex_auth_info.auth_value)
        else:
            headers = cls.build_headers_with_extra(api_key, merged_extra if merged_extra else None)

            if is_oauth:
                from src.core.api_format import get_auth_config_for_endpoint

                default_auth_header, _ = get_auth_config_for_endpoint(cls.FORMAT_ID)
                if default_auth_header.lower() != "authorization":
                    headers.pop(default_auth_header, None)
                auth_header_name = resolve_header_name_case(extra_headers, "Authorization")
                headers[auth_header_name] = f"Bearer {api_key}"

        # ---- Body ----
        body = cls.build_request_body(
            request_data,
            base_url=validated_base_url,
            provider_type=provider_type,
        )

        if body_rules:
            body = apply_body_rules(
                body,
                body_rules,
                protected_keys=get_cache_sensitive_protected_body_keys(cls.FORMAT_ID),
                original_body=body,
            )

        if is_antigravity:
            from src.services.provider.adapters.antigravity.envelope import (
                wrap_v1internal_request,
            )

            project_id = (decrypted_auth_config or {}).get("project_id", "")
            effective_model = model_name or request_data.get("model", "")
            body = wrap_v1internal_request(
                body,
                project_id=project_id,
                model=effective_model,
            )
        elif is_gemini_cli:
            from src.services.provider.adapters.gemini_cli.envelope import wrap_v1internal_request

            project_id = (decrypted_auth_config or {}).get("project_id", "")
            effective_model = model_name or request_data.get("model", "")
            body = wrap_v1internal_request(
                body,
                project_id=project_id,
                model=effective_model,
            )

        if is_kiro:
            from src.services.provider.adapters.kiro.converter import (
                convert_claude_messages_to_conversation_state,
            )

            effective_model = model_name or request_data.get("model", "")
            conversation_state = convert_claude_messages_to_conversation_state(
                body,
                model=effective_model,
            )
            body = {"conversationState": conversation_state}
            if isinstance(kiro_cfg.profile_arn, str) and kiro_cfg.profile_arn.strip():
                body["profileArn"] = kiro_cfg.profile_arn.strip()

        # ---- Header Rules ----
        if header_rules:
            from src.core.api_format import get_auth_config_for_endpoint as _get_auth_cfg

            if is_oauth:
                protected_keys = {"authorization", "content-type"}
            else:
                ep_auth_header, _ = _get_auth_cfg(cls.FORMAT_ID)
                protected_keys = {ep_auth_header.lower(), "content-type"}
                if vertex_auth_info and getattr(vertex_auth_info, "auth_header", None):
                    protected_keys.add(str(vertex_auth_info.auth_header).lower())

            header_builder = HeaderBuilder()
            header_builder.add_many(headers)
            header_builder.apply_rules(
                header_rules,
                protected_keys,
                body=body,
                original_body=body,
                condition_evaluator=evaluate_condition,
            )
            headers = header_builder.build()

        # ---- Execute ----
        effective_model_name = model_name or request_data.get("model")

        return await run_endpoint_check(
            client=client,
            url=url,
            headers=headers,
            json_body=body,
            api_format=cls.FORMAT_ID,
            is_stream=bool(request_data.get("stream", False)),
            db=db,
            user=user,
            provider_name=provider_name,
            provider_id=provider_id,
            api_key_id=api_key_id,
            model_name=effective_model_name,
            proxy_config=proxy_config,
            timeout=timeout_seconds,
        )

    # =========================================================================
    # CLI Adapter 配置方法 - 子类可覆盖
    # =========================================================================

    @classmethod
    def build_endpoint_url(
        cls,
        base_url: str,
        request_data: dict[str, Any] | None = None,
        model_name: str | None = None,
        *,
        provider_type: str | None = None,
    ) -> str:
        """构建 API 端点 URL - 子类应覆盖"""
        return base_url

    @classmethod
    def get_cli_user_agent(cls) -> str | None:
        """获取 CLI User-Agent - 子类可覆盖"""
        return None

    @classmethod
    def get_cli_extra_headers(
        cls, *, base_url: str | None = None, provider_type: str | None = None
    ) -> dict[str, str]:
        """获取额外请求头 - 子类可覆盖"""
        headers: dict[str, str] = {}
        cli_user_agent = cls.get_cli_user_agent()
        if cli_user_agent:
            headers["User-Agent"] = cli_user_agent
        return headers
