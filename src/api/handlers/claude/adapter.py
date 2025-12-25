"""
Claude Chat Adapter - 基于 ChatAdapterBase 的 Claude Chat API 适配器

处理 /v1/messages 端点的 Claude Chat 格式请求。
"""

from typing import Any, Dict, Optional, Tuple, Type

import httpx
from fastapi import HTTPException, Request
from fastapi.responses import JSONResponse

from src.api.base.adapter import ApiAdapter, ApiMode
from src.api.base.context import ApiRequestContext
from src.api.handlers.base.chat_adapter_base import ChatAdapterBase, register_adapter
from src.api.handlers.base.chat_handler_base import ChatHandlerBase
from src.core.logger import logger
from src.core.optimization_utils import TokenCounter
from src.models.claude import ClaudeMessagesRequest, ClaudeTokenCountRequest


class ClaudeCapabilityDetector:
    """Claude API 能力检测器"""

    @staticmethod
    def detect_from_headers(
        headers: Dict[str, str],
        request_body: Optional[Dict[str, Any]] = None,
    ) -> Dict[str, bool]:
        """
        从 Claude 请求头检测能力需求

        检测规则:
        - anthropic-beta: context-1m-xxx -> context_1m: True

        Args:
            headers: 请求头字典
            request_body: 请求体（Claude 不使用，保留用于接口统一）
        """
        requirements: Dict[str, bool] = {}

        # 检查 anthropic-beta 请求头（大小写不敏感）
        beta_header = None
        for key, value in headers.items():
            if key.lower() == "anthropic-beta":
                beta_header = value
                break

        if beta_header:
            # 检查是否包含 context-1m 标识
            if "context-1m" in beta_header.lower():
                requirements["context_1m"] = True

        return requirements


@register_adapter
class ClaudeChatAdapter(ChatAdapterBase):
    """
    Claude Chat API 适配器

    处理 Claude Chat 格式的请求（/v1/messages 端点，进行格式验证）。
    """

    FORMAT_ID = "CLAUDE"
    name = "claude.chat"

    @property
    def HANDLER_CLASS(self) -> Type[ChatHandlerBase]:
        """延迟导入 Handler 类避免循环依赖"""
        from src.api.handlers.claude.handler import ClaudeChatHandler

        return ClaudeChatHandler

    def __init__(self, allowed_api_formats: Optional[list[str]] = None):
        super().__init__(allowed_api_formats or ["CLAUDE"])
        logger.info(f"[{self.name}] 初始化Chat模式适配器 | API格式: {self.allowed_api_formats}")

    def extract_api_key(self, request: Request) -> Optional[str]:
        """从请求中提取 API 密钥 (x-api-key)"""
        return request.headers.get("x-api-key")

    def detect_capability_requirements(
        self,
        headers: Dict[str, str],
        request_body: Optional[Dict[str, Any]] = None,
    ) -> Dict[str, bool]:
        """检测 Claude 请求中隐含的能力需求"""
        return ClaudeCapabilityDetector.detect_from_headers(headers)

    # =========================================================================
    # Claude 特定的计费逻辑
    # =========================================================================

    def compute_total_input_context(
        self,
        input_tokens: int,
        cache_read_input_tokens: int,
        cache_creation_input_tokens: int = 0,
    ) -> int:
        """
        计算 Claude 的总输入上下文（用于阶梯计费判定）

        Claude 的总输入 = input_tokens + cache_creation_input_tokens + cache_read_input_tokens
        """
        return input_tokens + cache_creation_input_tokens + cache_read_input_tokens

    def _validate_request_body(self, original_request_body: dict, path_params: dict = None):
        """验证请求体"""
        try:
            if not isinstance(original_request_body, dict):
                raise ValueError("Request body must be a JSON object")

            required_fields = ["model", "messages", "max_tokens"]
            missing_fields = [f for f in required_fields if f not in original_request_body]
            if missing_fields:
                raise ValueError(f"Missing required fields: {', '.join(missing_fields)}")

            request = ClaudeMessagesRequest.model_validate(
                original_request_body,
                strict=False,
            )
        except ValueError as e:
            logger.error(f"请求体基本验证失败: {str(e)}")
            raise HTTPException(status_code=400, detail=str(e))
        except Exception as e:
            logger.warning(f"Pydantic验证警告(将继续处理): {str(e)}")
            request = ClaudeMessagesRequest.model_construct(
                model=original_request_body.get("model"),
                max_tokens=original_request_body.get("max_tokens"),
                messages=original_request_body.get("messages", []),
                stream=original_request_body.get("stream", False),
            )
        return request

    def _build_audit_metadata(self, _payload: Dict[str, Any], request_obj) -> Dict[str, Any]:
        """构建 Claude Chat 特定的审计元数据"""
        role_counts: dict[str, int] = {}
        for message in request_obj.messages:
            role_counts[message.role] = role_counts.get(message.role, 0) + 1

        return {
            "action": "claude_messages",
            "model": request_obj.model,
            "stream": bool(request_obj.stream),
            "max_tokens": request_obj.max_tokens,
            "temperature": getattr(request_obj, "temperature", None),
            "top_p": getattr(request_obj, "top_p", None),
            "top_k": getattr(request_obj, "top_k", None),
            "messages_count": len(request_obj.messages),
            "message_roles": role_counts,
            "stop_sequences": len(request_obj.stop_sequences or []),
            "tools_count": len(request_obj.tools or []),
            "system_present": bool(request_obj.system),
            "metadata_present": bool(request_obj.metadata),
            "thinking_enabled": bool(request_obj.thinking),
        }

    @classmethod
    async def fetch_models(
        cls,
        client: httpx.AsyncClient,
        base_url: str,
        api_key: str,
        extra_headers: Optional[Dict[str, str]] = None,
    ) -> Tuple[list, Optional[str]]:
        """查询 Claude API 支持的模型列表"""
        headers = {
            "x-api-key": api_key,
            "Authorization": f"Bearer {api_key}",
            "anthropic-version": "2023-06-01",
        }
        if extra_headers:
            # 防止 extra_headers 覆盖认证头
            safe_headers = {
                k: v for k, v in extra_headers.items()
                if k.lower() not in ("x-api-key", "authorization", "anthropic-version")
            }
            headers.update(safe_headers)

        # 构建 /v1/models URL
        base_url = base_url.rstrip("/")
        if base_url.endswith("/v1"):
            models_url = f"{base_url}/models"
        else:
            models_url = f"{base_url}/v1/models"

        try:
            response = await client.get(models_url, headers=headers)
            logger.debug(f"Claude models request to {models_url}: status={response.status_code}")
            if response.status_code == 200:
                data = response.json()
                models = []
                if "data" in data:
                    models = data["data"]
                elif isinstance(data, list):
                    models = data
                # 为每个模型添加 api_format 字段
                for m in models:
                    m["api_format"] = cls.FORMAT_ID
                return models, None
            else:
                error_body = response.text[:500] if response.text else "(empty)"
                error_msg = f"HTTP {response.status_code}: {error_body}"
                logger.warning(f"Claude models request to {models_url} failed: {error_msg}")
                return [], error_msg
        except Exception as e:
            error_msg = f"Request error: {str(e)}"
            logger.warning(f"Failed to fetch Claude models from {models_url}: {e}")
            return [], error_msg

    @classmethod
    def build_endpoint_url(cls, base_url: str) -> str:
        """构建Claude API端点URL"""
        base_url = base_url.rstrip("/")
        if base_url.endswith("/v1"):
            return f"{base_url}/messages"
        else:
            return f"{base_url}/v1/messages"

    @classmethod
    def build_base_headers(cls, api_key: str) -> Dict[str, str]:
        """构建Claude API认证头"""
        return {
            "x-api-key": api_key,
            "Content-Type": "application/json",
            "anthropic-version": "2023-06-01",
        }

    @classmethod
    def get_protected_header_keys(cls) -> tuple:
        """返回Claude API的保护头部key"""
        return ("x-api-key", "content-type", "anthropic-version")

    @classmethod
    def build_request_body(cls, request_data: Dict[str, Any]) -> Dict[str, Any]:
        """构建Claude API请求体"""
        return {
            "model": request_data.get("model"),
            "max_tokens": request_data.get("max_tokens", 100),
            "messages": request_data.get("messages", []),
        }


def build_claude_adapter(x_app_header: Optional[str]):
    """根据 x-app 头部构造 Chat 或 Claude Code 适配器。"""
    if x_app_header and x_app_header.lower() == "cli":
        from src.api.handlers.claude_cli.adapter import ClaudeCliAdapter

        return ClaudeCliAdapter()
    return ClaudeChatAdapter()


class ClaudeTokenCountAdapter(ApiAdapter):
    """计算 Claude 请求 Token 数的轻量适配器。"""

    name = "claude.token_count"
    mode = ApiMode.STANDARD

    def extract_api_key(self, request: Request) -> Optional[str]:
        """从请求中提取 API 密钥 (x-api-key 或 Authorization: Bearer)"""
        # 优先检查 x-api-key
        api_key = request.headers.get("x-api-key")
        if api_key:
            return api_key
        # 降级到 Authorization: Bearer
        authorization = request.headers.get("authorization")
        if authorization and authorization.startswith("Bearer "):
            return authorization.replace("Bearer ", "")
        return None

    async def handle(self, context: ApiRequestContext):
        payload = context.ensure_json_body()

        try:
            request = ClaudeTokenCountRequest.model_validate(payload, strict=False)
        except Exception as e:
            logger.error(f"Token count payload invalid: {e}")
            raise HTTPException(status_code=400, detail="Invalid token count payload") from e

        token_counter = TokenCounter()
        total_tokens = 0

        if request.system:
            if isinstance(request.system, str):
                total_tokens += token_counter.count_tokens(request.system, request.model)
            elif isinstance(request.system, list):
                for block in request.system:
                    if hasattr(block, "text"):
                        total_tokens += token_counter.count_tokens(block.text, request.model)

        messages_dict = [
            msg.model_dump() if hasattr(msg, "model_dump") else msg for msg in request.messages
        ]
        total_tokens += token_counter.count_messages_tokens(messages_dict, request.model)

        context.add_audit_metadata(
            action="claude_token_count",
            model=request.model,
            messages_count=len(request.messages),
            system_present=bool(request.system),
            tools_count=len(request.tools or []),
            thinking_enabled=bool(request.thinking),
            input_tokens=total_tokens,
        )

        return JSONResponse({"input_tokens": total_tokens})


__all__ = [
    "ClaudeChatAdapter",
    "ClaudeTokenCountAdapter",
    "build_claude_adapter",
]
