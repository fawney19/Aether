"""
Gemini Chat Adapter

处理 Gemini API 格式的请求适配
"""

from typing import Any, AsyncIterator, Dict, Optional, Tuple, Type, Union

import httpx
from fastapi import HTTPException, Request
from fastapi.responses import JSONResponse

from src.api.handlers.base.chat_adapter_base import ChatAdapterBase, register_adapter
from src.api.handlers.base.chat_handler_base import ChatHandlerBase
from src.api.handlers.base.endpoint_checker import build_safe_headers, run_endpoint_check
from src.core.logger import logger
from src.models.gemini import GeminiRequest


@register_adapter
class GeminiChatAdapter(ChatAdapterBase):
    """
    Gemini Chat API 适配器

    处理 Gemini Chat 格式的请求
    端点: /v1beta/models/{model}:generateContent
    """

    FORMAT_ID = "GEMINI"
    name = "gemini.chat"

    @property
    def HANDLER_CLASS(self) -> Type[ChatHandlerBase]:
        """延迟导入 Handler 类避免循环依赖"""
        from src.api.handlers.gemini.handler import GeminiChatHandler

        return GeminiChatHandler

    def __init__(self, allowed_api_formats: Optional[list[str]] = None):
        super().__init__(allowed_api_formats or ["GEMINI"])
        logger.info(f"[{self.name}] 初始化 Gemini Chat 适配器 | API格式: {self.allowed_api_formats}")

    def extract_api_key(self, request: Request) -> Optional[str]:
        """从请求中提取 API 密钥 (x-goog-api-key)"""
        return request.headers.get("x-goog-api-key")

    def _merge_path_params(
        self, original_request_body: Dict[str, Any], path_params: Dict[str, Any]  # noqa: ARG002
    ) -> Dict[str, Any]:
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

    def _validate_request_body(self, original_request_body: dict, path_params: dict = None):
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

    def _extract_message_count(self, payload: Dict[str, Any], request_obj) -> int:
        """提取消息数量"""
        contents = payload.get("contents", [])
        if hasattr(request_obj, "contents"):
            contents = request_obj.contents
        return len(contents) if isinstance(contents, list) else 0

    def _build_audit_metadata(self, payload: Dict[str, Any], request_obj) -> Dict[str, Any]:
        """构建 Gemini Chat 特定的审计元数据"""
        role_counts: dict[str, int] = {}

        contents = getattr(request_obj, "contents", []) or []
        for content in contents:
            role = getattr(content, "role", None) or content.get("role", "unknown")
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
    async def fetch_models(
        cls,
        client: httpx.AsyncClient,
        base_url: str,
        api_key: str,
        extra_headers: Optional[Dict[str, str]] = None,
    ) -> Tuple[list, Optional[str]]:
        """查询 Gemini API 支持的模型列表"""
        # 兼容 base_url 已包含 /v1beta 的情况
        base_url_clean = base_url.rstrip("/")
        if base_url_clean.endswith("/v1beta"):
            models_url = f"{base_url_clean}/models?key={api_key}"
        else:
            models_url = f"{base_url_clean}/v1beta/models?key={api_key}"

        headers: Dict[str, str] = {}
        if extra_headers:
            headers.update(extra_headers)

        try:
            response = await client.get(models_url, headers=headers)
            logger.debug(f"Gemini models request to {models_url}: status={response.status_code}")
            if response.status_code == 200:
                data = response.json()
                if "models" in data:
                    # 转换为统一格式
                    return [
                        {
                            "id": m.get("name", "").replace("models/", ""),
                            "owned_by": "google",
                            "display_name": m.get("displayName", ""),
                            "api_format": cls.FORMAT_ID,
                        }
                        for m in data["models"]
                    ], None
                return [], None
            else:
                error_body = response.text[:500] if response.text else "(empty)"
                error_msg = f"HTTP {response.status_code}: {error_body}"
                logger.warning(f"Gemini models request to {models_url} failed: {error_msg}")
                return [], error_msg
        except Exception as e:
            error_msg = f"Request error: {str(e)}"
            logger.warning(f"Failed to fetch Gemini models from {models_url}: {e}")
            return [], error_msg

    @classmethod
    def build_endpoint_url(cls, base_url: str) -> str:
        """构建Gemini API端点URL"""
        base_url = base_url.rstrip("/")
        if base_url.endswith("/v1beta"):
            return base_url  # 子类需要处理model参数
        else:
            return f"{base_url}/v1beta"

    @classmethod
    def build_base_headers(cls, api_key: str) -> Dict[str, str]:
        """构建Gemini API认证头"""
        return {
            "x-goog-api-key": api_key,
            "Content-Type": "application/json",
        }

    @classmethod
    def get_protected_header_keys(cls) -> tuple:
        """返回Gemini API的保护头部key"""
        return ("x-goog-api-key", "content-type")

    @classmethod
    def build_request_body(cls, request_data: Dict[str, Any]) -> Dict[str, Any]:
        """构建Gemini API请求体"""
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
        """测试 Gemini API 模型连接性（非流式）"""
        # Gemini需要从request_data或model_name参数获取model名称
        effective_model_name = model_name or request_data.get("model", "")
        if not effective_model_name:
            return {
                "error": "Model name is required for Gemini API",
                "status_code": 400,
            }

        # 使用基类配置方法，但重写URL构建逻辑
        base_url = cls.build_endpoint_url(base_url)
        url = f"{base_url}/models/{effective_model_name}:generateContent"

        # 构建请求组件
        base_headers = cls.build_base_headers(api_key)
        protected_keys = cls.get_protected_header_keys()
        headers = build_safe_headers(base_headers, extra_headers, protected_keys)
        body = cls.build_request_body(request_data)

        # 使用基类的通用endpoint checker
        from src.api.handlers.base.endpoint_checker import run_endpoint_check
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


def build_gemini_adapter(x_app_header: str = "") -> GeminiChatAdapter:
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
