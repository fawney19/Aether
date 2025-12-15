"""
Claude CLI Message Handler - 基于通用 CLI Handler 基类的简化实现

继承 CliMessageHandlerBase，只需覆盖格式特定的配置和事件处理逻辑。
验证新架构的有效性：代码量从数百行减少到 ~80 行。
"""

from typing import Any, Dict, Optional

from src.api.handlers.base.cli_handler_base import (
    CliMessageHandlerBase,
    StreamContext,
)
from src.api.handlers.base.utils import extract_cache_creation_tokens


class ClaudeCliMessageHandler(CliMessageHandlerBase):
    """
    Claude CLI Message Handler - 处理 Claude CLI API 格式

    使用新三层架构 (Provider -> ProviderEndpoint -> ProviderAPIKey)
    通过 FallbackOrchestrator 实现自动故障转移、健康监控和并发控制

    响应格式特点：
    - 使用 content[] 数组
    - 使用 text 类型
    - 流式事件：message_start, content_block_delta, message_delta, message_stop
    - 支持 cache_creation_input_tokens 和 cache_read_input_tokens

    模型字段：请求体顶级 model 字段
    """

    FORMAT_ID = "CLAUDE_CLI"

    def extract_model_from_request(
        self,
        request_body: Dict[str, Any],
        path_params: Optional[Dict[str, Any]] = None,  # noqa: ARG002
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
        request_body: Dict[str, Any],
        mapped_model: str,
    ) -> Dict[str, Any]:
        """
        Claude API 的 model 在请求体顶级

        Args:
            request_body: 原始请求体
            mapped_model: 映射后的模型名

        Returns:
            更新了 model 字段的请求体
        """
        result = dict(request_body)
        result["model"] = mapped_model
        return result

    def _process_event_data(
        self,
        ctx: StreamContext,
        event_type: str,
        data: Dict[str, Any],
    ) -> None:
        """
        处理 Claude CLI 格式的 SSE 事件

        事件类型：
        - message_start: 消息开始，包含初始 usage（含缓存 tokens）
        - content_block_delta: 文本增量
        - message_delta: 消息增量，包含最终 usage
        - message_stop: 消息结束
        """
        # 处理 message_start 事件
        if event_type == "message_start":
            message = data.get("message", {})
            if message.get("id"):
                ctx.response_id = message["id"]

            # 提取初始 usage（包含缓存 tokens）
            usage = message.get("usage", {})
            if usage:
                ctx.input_tokens = usage.get("input_tokens", 0)

                cache_read = usage.get("cache_read_input_tokens", 0)
                if cache_read:
                    ctx.cached_tokens = cache_read

                cache_creation = extract_cache_creation_tokens(usage)
                if cache_creation:
                    ctx.cache_creation_tokens = cache_creation

        # 处理文本增量
        elif event_type == "content_block_delta":
            delta = data.get("delta", {})
            if delta.get("type") == "text_delta":
                text = delta.get("text", "")
                if text:
                    ctx.append_text(text)

        # 处理消息增量（包含最终 usage）
        elif event_type == "message_delta":
            usage = data.get("usage", {})
            if usage:
                if "input_tokens" in usage:
                    ctx.input_tokens = usage["input_tokens"]
                if "output_tokens" in usage:
                    ctx.output_tokens = usage["output_tokens"]

                # 更新缓存读取 tokens
                if "cache_read_input_tokens" in usage:
                    ctx.cached_tokens = usage["cache_read_input_tokens"]

                # 更新缓存创建 tokens
                cache_creation = extract_cache_creation_tokens(usage)
                if cache_creation > 0:
                    ctx.cache_creation_tokens = cache_creation

            # 检查是否结束
            delta = data.get("delta", {})
            if delta.get("stop_reason"):
                ctx.has_completion = True
                ctx.final_response = data

        # 处理消息结束
        elif event_type == "message_stop":
            ctx.has_completion = True

    def _extract_response_metadata(
        self,
        response: Dict[str, Any],
    ) -> Dict[str, Any]:
        """
        从 Claude 响应中提取元数据

        提取 model、stop_reason 等字段作为元数据。

        Args:
            response: Claude API 响应

        Returns:
            提取的元数据字典
        """
        metadata: Dict[str, Any] = {}

        # 提取模型名称（实际使用的模型）
        if "model" in response:
            metadata["model"] = response["model"]

        # 提取停止原因
        if "stop_reason" in response:
            metadata["stop_reason"] = response["stop_reason"]

        # 提取消息 ID
        if "id" in response:
            metadata["message_id"] = response["id"]

        # 提取消息类型
        if "type" in response:
            metadata["type"] = response["type"]

        return metadata

    def _finalize_stream_metadata(self, ctx: StreamContext) -> None:
        """
        从流上下文中提取最终元数据

        在流传输完成后调用，从收集的事件中提取元数据。

        Args:
            ctx: 流上下文
        """
        # 从 response_id 提取消息 ID
        if ctx.response_id:
            ctx.response_metadata["message_id"] = ctx.response_id

        # 从 final_response 提取停止原因（message_delta 事件中的 delta.stop_reason）
        if ctx.final_response:
            delta = ctx.final_response.get("delta", {})
            if "stop_reason" in delta:
                ctx.response_metadata["stop_reason"] = delta["stop_reason"]

        # 记录模型名称
        if ctx.model:
            ctx.response_metadata["model"] = ctx.model

