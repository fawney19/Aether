"""
Claude Chat Handler - 基于通用 Chat Handler 基类的简化实现

继承 ChatHandlerBase，只需覆盖格式特定的方法。
代码量从原来的 ~1470 行减少到 ~120 行。
"""

from typing import Any

from src.api.handlers.base.chat_handler_base import ChatHandlerBase
from src.core.api_format import ApiFamily, EndpointKind
from src.core.usage_tokens import extract_cache_creation_tokens_detail


class ClaudeChatHandler(ChatHandlerBase):
    """
    Claude Chat Handler - 处理 Claude Chat/CLI API 格式的请求

    格式特点：
    - 使用 input_tokens/output_tokens
    - 支持 cache_creation_input_tokens/cache_read_input_tokens
    - 请求格式：ClaudeMessagesRequest
    """

    FORMAT_ID = "claude:chat"
    API_FAMILY = ApiFamily.CLAUDE
    ENDPOINT_KIND = EndpointKind.CHAT

    def extract_model_from_request(
        self,
        request_body: dict[str, Any],
        path_params: dict[str, Any] | None = None,  # noqa: ARG002
    ) -> str:
        """
        从请求中提取模型名 - Claude 格式实现

        Claude API 的 model 在请求体顶级字段。

        Args:
            request_body: 请求体
            path_params: URL 路径参数（Claude 不使用）

        Returns:
            模型名
        """
        model = request_body.get("model")
        return str(model) if model else "unknown"

    def apply_mapped_model(
        self,
        request_body: dict[str, Any],
        mapped_model: str,
    ) -> dict[str, Any]:
        """
        将映射后的模型名应用到请求体

        Claude API 的 model 在请求体顶级字段。

        Args:
            request_body: 原始请求体
            mapped_model: 映射后的模型名

        Returns:
            更新了 model 字段的请求体
        """
        result = dict(request_body)
        result["model"] = mapped_model
        return result

    async def _convert_request(self, request: Any) -> Any:
        """
        将请求转换为 Claude 格式的 Pydantic 对象

        注意：此方法只做类型转换（dict → Pydantic），不做跨格式转换。
        跨格式转换由调度/执行层（TaskService + RequestDispatcher）在选中候选后、发送请求前执行，
        并受全局开关和端点配置控制。

        Args:
            request: 原始请求对象（应已是 Claude 格式）

        Returns:
            ClaudeMessagesRequest 对象
        """
        from src.models.claude import ClaudeMessagesRequest

        # 如果已经是 Claude 格式 Pydantic 对象，直接返回
        if isinstance(request, ClaudeMessagesRequest):
            return request

        # 如果是字典，转换为 Pydantic 对象（假设已是 Claude 格式）
        if isinstance(request, dict):
            return ClaudeMessagesRequest(**request)

        return request

    def _extract_usage(self, response: dict) -> dict[str, int]:
        """
        从 Claude 响应中提取 token 使用情况

        Claude 格式使用：
        - input_tokens / output_tokens
        - cache_creation_input_tokens / cache_read_input_tokens
        - 新格式：claude_cache_creation_5_m_tokens / claude_cache_creation_1_h_tokens
        """
        usage = response.get("usage", {})
        total, t5m, t1h = extract_cache_creation_tokens_detail(usage)

        return {
            "input_tokens": usage.get("input_tokens", 0),
            "output_tokens": usage.get("output_tokens", 0),
            "cache_creation_input_tokens": total,
            "cache_read_input_tokens": usage.get("cache_read_input_tokens", 0),
            "cache_ttl_minutes": 60 if t1h > 0 else (5 if total > 0 else None),
        }

    def _normalize_response(self, response: dict[str, Any]) -> dict[str, Any]:
        """
        规范化 Claude 响应

        Args:
            response: 原始响应

        Returns:
            规范化后的响应
        """
        # 作为中转站，直接透传响应，不做标准化处理
        return response
