"""
Claude Chat Adapter - 基于 ChatAdapterBase 的 Claude Chat API 适配器

处理 /v1/messages 端点的 Claude Chat 格式请求。
"""

from __future__ import annotations

from typing import Any

from fastapi import HTTPException, Request
from fastapi.responses import JSONResponse

from src.api.base.adapter import ApiAdapter, ApiMode
from src.api.base.context import ApiRequestContext
from src.api.handlers.base.chat_adapter_base import ChatAdapterBase, register_adapter
from src.api.handlers.base.chat_handler_base import ChatHandlerBase
from src.core.api_format import ApiFamily, get_header_value
from src.core.logger import logger
from src.models.claude import ClaudeMessagesRequest, ClaudeTokenCountRequest


class ClaudeCapabilityDetector:
    """Claude API 能力检测器"""

    @staticmethod
    def detect_from_headers(
        headers: dict[str, str],
        request_body: dict[str, Any] | None = None,
    ) -> dict[str, bool]:
        """
        从 Claude 请求头和请求体检测能力需求

        检测规则:
        - anthropic-beta: context-1m-xxx -> context_1m: True
        - 请求体中 cache_control.ttl = "1h" -> cache_1h: True

        Args:
            headers: 请求头字典
            request_body: 请求体（用于检测 cache_control.ttl）
        """
        requirements: dict[str, bool] = {}

        # 使用统一的大小写不敏感获取
        beta_header = get_header_value(headers, "anthropic-beta")
        if beta_header and "context-1m" in beta_header.lower():
            requirements["context_1m"] = True

        # 从请求体检测 cache_1h
        if request_body and _detect_cache_1h_in_body(request_body):
            requirements["cache_1h"] = True

        return requirements


def _has_cache_1h_ttl(block: dict[str, Any]) -> bool:
    """检查单个内容块是否包含 cache_control.ttl = '1h'"""
    cache_control = block.get("cache_control")
    if isinstance(cache_control, dict):
        return cache_control.get("ttl") == "1h"
    return False


def _detect_cache_1h_in_body(body: dict[str, Any]) -> bool:
    """
    扫描 Claude 请求体，检测是否包含 cache_control.ttl = "1h"

    检查位置：
    - system[].cache_control.ttl
    - messages[].content[].cache_control.ttl
    - tools[].cache_control.ttl
    """
    # 检查 system（数组格式）
    system = body.get("system")
    if isinstance(system, list):
        for block in system:
            if isinstance(block, dict) and _has_cache_1h_ttl(block):
                return True

    # 检查 messages
    messages = body.get("messages")
    if isinstance(messages, list):
        for msg in messages:
            if not isinstance(msg, dict):
                continue
            content = msg.get("content")
            if isinstance(content, list):
                for block in content:
                    if isinstance(block, dict) and _has_cache_1h_ttl(block):
                        return True

    # 检查 tools
    tools = body.get("tools")
    if isinstance(tools, list):
        for tool in tools:
            if isinstance(tool, dict) and _has_cache_1h_ttl(tool):
                return True

    return False


_TOKEN_COUNTER_PLUGIN: Any = None


def _get_token_counter() -> Any:
    global _TOKEN_COUNTER_PLUGIN  # noqa: PLW0603
    if _TOKEN_COUNTER_PLUGIN is None:
        from src.plugins.token.tiktoken_counter import TiktokenCounterPlugin

        _TOKEN_COUNTER_PLUGIN = TiktokenCounterPlugin(name="tiktoken")
    return _TOKEN_COUNTER_PLUGIN


async def _count_text_tokens_with_fallback(text: str, model: str) -> int:
    """使用 tiktoken 插件计数，失败时回退到轻量估算。"""
    if not text:
        return 0
    try:
        plugin = _get_token_counter()
        if plugin.enabled:
            return await plugin.count_tokens(text, model)
    except Exception as exc:
        logger.debug("tiktoken token 计数失败，使用估算回退: {}", exc)
    # 与旧实现保持一致：按字符估算
    return max(1, len(text) // 4)


async def _count_messages_tokens_with_fallback(messages: list[dict[str, Any]], model: str) -> int:
    """按历史逻辑统计 messages token（每条消息固定开销 + 内容 token）。"""
    total = 0
    for message in messages:
        if not isinstance(message, dict):
            continue
        total += 4  # 角色与分隔符开销

        content = message.get("content", "")
        if isinstance(content, str):
            total += await _count_text_tokens_with_fallback(content, model)
        elif isinstance(content, list):
            for item in content:
                if isinstance(item, dict):
                    text = item.get("text")
                    if isinstance(text, str):
                        total += await _count_text_tokens_with_fallback(text, model)

    return total


@register_adapter
class ClaudeChatAdapter(ChatAdapterBase):
    """
    Claude Chat API 适配器

    处理 Claude Chat 格式的请求（/v1/messages 端点，进行格式验证）。
    """

    FORMAT_ID = "claude:chat"
    API_FAMILY = ApiFamily.CLAUDE
    name = "claude.chat"

    @property
    def HANDLER_CLASS(self) -> type[ChatHandlerBase]:
        """延迟导入 Handler 类避免循环依赖"""
        from src.api.handlers.claude.handler import ClaudeChatHandler

        return ClaudeChatHandler

    def __init__(self, allowed_api_formats: list[str] | None = None):
        super().__init__(allowed_api_formats)
        logger.info(f"[{self.name}] 初始化Chat模式适配器 | API格式: {self.allowed_api_formats}")

    def detect_capability_requirements(
        self,
        headers: dict[str, str],
        request_body: dict[str, Any] | None = None,
    ) -> dict[str, bool]:
        """检测 Claude 请求中隐含的能力需求"""
        return ClaudeCapabilityDetector.detect_from_headers(headers, request_body)

    def _validate_request_body(
        self, original_request_body: dict, path_params: dict | None = None
    ) -> None:
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

    def _build_audit_metadata(self, _payload: dict[str, Any], request_obj: Any) -> dict[str, Any]:
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
    def build_endpoint_url(
        cls,
        base_url: str,
        request_data: dict[str, Any] | None = None,
        model_name: str | None = None,
        *,
        provider_type: str | None = None,
    ) -> str:
        """构建Claude API端点URL"""
        from src.utils.url_utils import join_url

        return join_url(base_url, "/v1/messages")

    # build_request_body 使用基类实现，通过 format_conversion_registry 自动转换 OPENAI -> CLAUDE


def build_claude_adapter(request: Request) -> Any:
    """根据认证头构造 Chat 或 Claude Code 适配器。

    - Authorization: Bearer (且无 x-api-key) -> CLI 模式
    - x-api-key -> Chat 模式
    """
    auth_header = request.headers.get("authorization", "")
    has_bearer = auth_header.lower().startswith("bearer ")
    has_api_key = bool(request.headers.get("x-api-key"))

    if has_bearer and not has_api_key:
        from src.api.handlers.claude_cli.adapter import ClaudeCliAdapter

        return ClaudeCliAdapter()
    return ClaudeChatAdapter()


class ClaudeTokenCountAdapter(ApiAdapter):
    """计算 Claude 请求 Token 数的轻量适配器。"""

    name = "claude.token_count"
    mode = ApiMode.STANDARD
    eager_request_body = False

    def extract_api_key(self, request: Request) -> str | None:
        """从请求中提取 API 密钥 (x-api-key 或 Authorization: Bearer)"""
        from src.core.api_format import get_auth_handler
        from src.core.api_format.enums import AuthMethod

        handler = get_auth_handler(AuthMethod.API_KEY)
        api_key = handler.extract_credentials(request)
        if api_key:
            return api_key

        bearer_handler = get_auth_handler(AuthMethod.BEARER)
        return bearer_handler.extract_credentials(request)

    async def handle(self, context: ApiRequestContext) -> Any:
        payload = await context.ensure_json_body_async()

        try:
            request = ClaudeTokenCountRequest.model_validate(payload, strict=False)
        except Exception as e:
            logger.error(f"Token count payload invalid: {e}")
            raise HTTPException(status_code=400, detail="Invalid token count payload") from e

        total_tokens = 0

        if request.system:
            if isinstance(request.system, str):
                total_tokens += await _count_text_tokens_with_fallback(
                    request.system, request.model
                )
            elif isinstance(request.system, list):
                for block in request.system:
                    if hasattr(block, "text"):
                        total_tokens += await _count_text_tokens_with_fallback(
                            block.text, request.model
                        )

        messages_dict = [
            msg.model_dump() if hasattr(msg, "model_dump") else msg for msg in request.messages
        ]
        total_tokens += await _count_messages_tokens_with_fallback(messages_dict, request.model)

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
