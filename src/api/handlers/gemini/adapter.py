"""
Gemini Chat Adapter

处理 Gemini API 格式的请求适配
"""

from __future__ import annotations

from typing import Any

import httpx
from fastapi import HTTPException, Request
from fastapi.responses import JSONResponse

from src.api.handlers.base.chat_adapter_base import ChatAdapterBase, register_adapter
from src.api.handlers.base.chat_handler_base import ChatHandlerBase
from src.core.api_format import ApiFamily, get_auth_handler, resolve_header_name_case
from src.core.api_format.enums import AuthMethod
from src.core.logger import logger
from src.models.gemini import GeminiRequest
from src.services.gemini_files_mapping import extract_file_names_from_request


class GeminiCapabilityDetector:
    """Gemini API 能力检测器"""

    @staticmethod
    def detect_from_request(
        headers: dict[str, str],  # noqa: ARG004 - 预留
        request_body: dict[str, Any] | None = None,
    ) -> dict[str, bool]:
        """
        从请求体检测 Gemini 能力需求

        检测规则:
        - fileData.fileUri -> gemini_files: True
        """
        requirements: dict[str, bool] = {}
        if request_body and extract_file_names_from_request(request_body):
            requirements["gemini_files"] = True
        return requirements


@register_adapter
class GeminiChatAdapter(ChatAdapterBase):
    """
    Gemini Chat API 适配器

    处理 Gemini Chat 格式的请求
    端点: /v1beta/models/{model}:generateContent
    """

    FORMAT_ID = "gemini:chat"
    API_FAMILY = ApiFamily.GEMINI
    name = "gemini.chat"

    @property
    def HANDLER_CLASS(self) -> type[ChatHandlerBase]:
        """延迟导入 Handler 类避免循环依赖"""
        from src.api.handlers.gemini.handler import GeminiChatHandler

        return GeminiChatHandler

    def __init__(self, allowed_api_formats: list[str] | None = None):
        super().__init__(allowed_api_formats)
        logger.info(
            f"[{self.name}] 初始化 Gemini Chat 适配器 | API格式: {self.allowed_api_formats}"
        )

    def extract_api_key(self, request: Request) -> str | None:
        """
        从请求中提取 API 密钥 - Gemini 支持 header 和 query 两种方式

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
        合并 URL 路径参数到请求体 - Gemini 特化版本

        Gemini API 特点:
        - model 不合并到请求体（通过 extract_model_from_request 从 path_params 获取）
        - stream 不合并到请求体（Gemini API 通过 URL 端点区分流式/非流式）

        Handler 层的 extract_model_from_request 会从 path_params 获取 model，
        prepare_provider_request_body 会确保发送给 Gemini API 的请求体不含 model。

        Args:
            original_request_body: 原始请求体字典
            path_params: URL 路径参数字典（不使用）

        Returns:
            原始请求体（不合并任何 path_params）
        """
        return original_request_body.copy()

    def _validate_request_body(
        self, original_request_body: dict, path_params: dict | None = None
    ) -> None:
        """验证请求体"""
        path_params = path_params or {}
        is_stream = path_params.get("stream", False)
        model = path_params.get("model", "unknown")

        try:
            if not isinstance(original_request_body, dict):
                raise ValueError("Request body must be a JSON object")

            # Gemini 必需字段: contents
            if "contents" not in original_request_body:
                raise ValueError("Missing required field: contents")

            request = GeminiRequest.model_validate(
                original_request_body,
                strict=False,
            )
        except ValueError as e:
            logger.error(f"请求体基本验证失败: {str(e)}")
            raise HTTPException(status_code=400, detail=str(e))
        except Exception as e:
            logger.warning(f"Pydantic验证警告(将继续处理): {str(e)}")
            request = GeminiRequest.model_construct(
                contents=original_request_body.get("contents", []),
            )

        # 设置 model（从 path_params 获取，用于日志和审计）
        request.model = model
        # 设置 stream 属性（用于 ChatAdapterBase 判断流式模式）
        request.stream = is_stream
        return request

    def _extract_message_count(self, payload: dict[str, Any], request_obj: Any) -> int:
        """提取消息数量"""
        contents = payload.get("contents", [])
        if hasattr(request_obj, "contents"):
            contents = request_obj.contents
        return len(contents) if isinstance(contents, list) else 0

    def _build_audit_metadata(self, payload: dict[str, Any], request_obj: Any) -> dict[str, Any]:
        """构建 Gemini Chat 特定的审计元数据"""
        role_counts: dict[str, int] = {}

        contents = getattr(request_obj, "contents", []) or []
        for content in contents:
            if isinstance(content, dict):
                role = content.get("role", "unknown")
            else:
                role = getattr(content, "role", None) or "unknown"
            role_counts[role] = role_counts.get(role, 0) + 1

        generation_config = getattr(request_obj, "generation_config", None) or {}
        if hasattr(generation_config, "dict"):
            generation_config = generation_config.dict()
        elif not isinstance(generation_config, dict):
            generation_config = {}

        # 判断流式模式
        stream = getattr(request_obj, "stream", False)

        return {
            "action": "gemini_generate_content",
            "model": getattr(request_obj, "model", payload.get("model", "unknown")),
            "stream": bool(stream),
            "max_output_tokens": generation_config.get("max_output_tokens"),
            "temperature": generation_config.get("temperature"),
            "top_p": generation_config.get("top_p"),
            "top_k": generation_config.get("top_k"),
            "contents_count": len(contents),
            "content_roles": role_counts,
            "tools_count": len(getattr(request_obj, "tools", None) or []),
            "system_instruction_present": bool(getattr(request_obj, "system_instruction", None)),
            "safety_settings_count": len(getattr(request_obj, "safety_settings", None) or []),
        }

    def _error_response(self, status_code: int, error_type: str, message: str) -> JSONResponse:
        """生成 Gemini 格式的错误响应"""
        # Gemini 错误响应格式
        return JSONResponse(
            status_code=status_code,
            content={
                "error": {
                    "code": status_code,
                    "message": message,
                    "status": error_type.upper(),
                }
            },
        )

    @classmethod
    def build_endpoint_url(
        cls,
        base_url: str,
        request_data: dict[str, Any] | None = None,
        model_name: str | None = None,
        *,
        provider_type: str | None = None,
    ) -> str:
        """构建Gemini API端点URL"""
        base_url = base_url.rstrip("/")
        if base_url.endswith("/v1beta"):
            return base_url  # 子类需要处理model参数
        else:
            return f"{base_url}/v1beta"

    # build_request_body 使用基类实现，通过 format_conversion_registry 自动转换 OPENAI -> GEMINI

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
        # Provider 上下文
        auth_type: str | None = None,
        provider_type: str | None = None,
        decrypted_auth_config: dict[str, Any] | None = None,
        provider_endpoint: Any | None = None,
        provider_api_key: Any | None = None,
        # 代理配置
        proxy_config: dict[str, Any] | None = None,
        timeout_seconds: float | None = None,
    ) -> dict[str, Any]:
        """测试 Gemini API 模型连接性（非流式）"""
        from src.api.handlers.base.endpoint_checker import run_endpoint_check
        from src.api.handlers.base.request_builder import (
            apply_body_rules,
            evaluate_condition,
            get_cache_sensitive_protected_body_keys,
        )
        from src.core.api_format.headers import HeaderBuilder

        # Gemini需要从request_data或model_name参数获取model名称
        effective_model_name = model_name or request_data.get("model", "")
        if not effective_model_name:
            return {
                "error": "Model name is required for Gemini API",
                "status_code": 400,
            }

        is_antigravity = provider_type and provider_type.lower() == "antigravity"
        is_gemini_cli = provider_type and provider_type.lower() == "gemini_cli"
        is_vertex = provider_type and provider_type.lower() == "vertex_ai"
        is_oauth = auth_type == "oauth"
        vertex_auth_info: Any | None = None

        # Antigravity provider 使用 v1internal 路径，而非标准 Gemini API 路径
        if is_antigravity:
            from src.services.provider.adapters.antigravity.constants import (
                V1INTERNAL_PATH_TEMPLATE,
                get_v1internal_extra_headers,
            )
            from src.services.provider.adapters.antigravity.url_availability import url_availability

            ordered_urls = url_availability.get_ordered_urls(prefer_daily=True)
            ag_base = ordered_urls[0] if ordered_urls else base_url
            path = V1INTERNAL_PATH_TEMPLATE.format(action="generateContent")
            url = f"{str(ag_base).rstrip('/')}{path}"
        elif is_gemini_cli:
            from src.services.provider.adapters.gemini_cli.constants import V1INTERNAL_PATH_TEMPLATE

            path = V1INTERNAL_PATH_TEMPLATE.format(action="generateContent")
            url = f"{str(base_url).rstrip('/')}{path}"
        elif is_vertex and provider_endpoint is not None and provider_api_key is not None:
            # Vertex AI: test-model 必须走统一 provider transport/auth，
            # 否则会错误命中普通 Gemini URL（导致 404）。
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
            url = build_provider_url(
                provider_endpoint,
                path_params={"model": effective_model_name},
                is_stream=bool(request_data.get("stream", False)),
                key=provider_api_key,
                decrypted_auth_config=effective_auth_config,
            )
        else:
            # 使用基类配置方法，但重写URL构建逻辑
            base_url_resolved = cls.build_endpoint_url(base_url)
            url = f"{base_url_resolved}/models/{effective_model_name}:generateContent"

        # 构建请求组件
        # Antigravity 需要特定的 User-Agent
        merged_extra = dict(extra_headers) if extra_headers else {}
        if is_antigravity:
            merged_extra.update(get_v1internal_extra_headers())
        elif is_gemini_cli:
            from src.services.provider.adapters.gemini_cli.constants import (
                get_v1internal_extra_headers,
            )

            merged_extra.update(get_v1internal_extra_headers())
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

            # OAuth 统一处理：替换端点默认认证头（x-goog-api-key）为 Authorization: Bearer
            if is_oauth:
                from src.core.api_format import get_auth_config_for_endpoint

                default_auth_header, _ = get_auth_config_for_endpoint(cls.FORMAT_ID)
                if default_auth_header.lower() != "authorization":
                    headers.pop(default_auth_header, None)
                auth_header_name = resolve_header_name_case(extra_headers, "Authorization")
                headers[auth_header_name] = f"Bearer {api_key}"

        body = cls.build_request_body(request_data)

        # 应用请求体规则（在格式转换后应用，确保规则效果不被覆盖）
        if body_rules:
            body = apply_body_rules(
                body,
                body_rules,
                protected_keys=get_cache_sensitive_protected_body_keys(cls.FORMAT_ID),
                original_body=body,
            )

        # Antigravity 需要将请求体包装为 v1internal 信封格式
        if is_antigravity:
            from src.services.provider.adapters.antigravity.envelope import wrap_v1internal_request

            project_id = (decrypted_auth_config or {}).get("project_id", "")
            body = wrap_v1internal_request(
                body,
                project_id=project_id,
                model=effective_model_name,
                request_type="endpoint_test",
            )
        elif is_gemini_cli:
            from src.services.provider.adapters.gemini_cli.envelope import wrap_v1internal_request

            project_id = (decrypted_auth_config or {}).get("project_id", "")
            body = wrap_v1internal_request(
                body,
                project_id=project_id,
                model=effective_model_name,
            )

        # 应用请求头规则（在请求头构建后应用）
        if header_rules:
            # 获取认证头名称，防止被规则覆盖
            from src.core.api_format import get_auth_config_for_endpoint

            auth_header, _ = get_auth_config_for_endpoint(cls.FORMAT_ID)
            protected_keys = {auth_header.lower(), "content-type"}
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

        return await run_endpoint_check(
            client=client,
            url=url,
            headers=headers,
            json_body=body,
            api_format=cls.FORMAT_ID,
            is_stream=bool(request_data.get("stream", False)),
            # 用量计算参数（现在强制记录）
            db=db,
            user=user,
            provider_name=provider_name,
            provider_id=provider_id,
            api_key_id=api_key_id,
            model_name=effective_model_name,
            proxy_config=proxy_config,
            timeout=timeout_seconds,
        )


def build_gemini_adapter(x_app_header: str = "") -> GeminiChatAdapter:  # noqa: ARG001
    """
    根据请求头构建适当的 Gemini 适配器

    Args:
        x_app_header: X-App 请求头值

    Returns:
        GeminiChatAdapter 实例
    """
    # 目前只有一种 Gemini 适配器
    # 未来可以根据 x_app_header 返回不同的适配器（如 CLI 模式）
    return GeminiChatAdapter()


__all__ = ["GeminiChatAdapter", "build_gemini_adapter"]
