"""
OpenAI CLI Message Handler - 基于通用 CLI Handler 基类的简化实现

继承 CliMessageHandlerBase，只需覆盖格式特定的配置和事件处理逻辑。
代码量从原来的 900+ 行减少到 ~100 行。
"""

from typing import Any, Dict, Optional

from src.api.handlers.base.cli_handler_base import (
    CliMessageHandlerBase,
    StreamContext,
)


class OpenAICliMessageHandler(CliMessageHandlerBase):
    """
    OpenAI CLI Message Handler - 处理 OpenAI CLI Responses API 格式

    使用新三层架构 (Provider -> ProviderEndpoint -> ProviderAPIKey)
    通过 FallbackOrchestrator 实现自动故障转移、健康监控和并发控制

    响应格式特点：
    - 使用 output[] 数组而非 content[]
    - 使用 output_text 类型而非普通 text
    - 流式事件：response.output_text.delta, response.output_text.done

    模型字段：请求体顶级 model 字段
    """

    FORMAT_ID = "OPENAI_CLI"

    def extract_model_from_request(
        self,
        request_body: Dict[str, Any],
        path_params: Optional[Dict[str, Any]] = None,  # noqa: ARG002
    ) -> str:
        """
        从请求中提取模型名 - OpenAI 格式实现

        OpenAI API 的 model 在请求体顶级字段。

        Args:
            request_body: 请求体
            path_params: URL 路径参数（OpenAI 不使用）

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
        OpenAI CLI (Responses API) 的 model 在请求体顶级字段。

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
        处理 OpenAI CLI 格式的 SSE 事件

        事件类型：
        - response.output_text.delta: 文本增量
        - response.completed: 响应完成（包含 usage）
        """
        # 提取 response_id
        if not ctx.response_id:
            response_obj = data.get("response")
            if isinstance(response_obj, dict) and response_obj.get("id"):
                ctx.response_id = response_obj["id"]
            elif "id" in data:
                ctx.response_id = data["id"]

        # 处理文本增量
        if event_type in ["response.output_text.delta", "response.outtext.delta"]:
            delta = data.get("delta")
            if isinstance(delta, str):
                ctx.append_text(delta)
            elif isinstance(delta, dict) and "text" in delta:
                ctx.append_text(delta["text"])

        # 处理完成事件
        elif event_type == "response.completed":
            ctx.has_completion = True
            response_obj = data.get("response")
            if isinstance(response_obj, dict):
                ctx.final_response = response_obj

                usage_obj = response_obj.get("usage")
                if isinstance(usage_obj, dict):
                    ctx.final_usage = usage_obj
                    ctx.input_tokens = usage_obj.get("input_tokens", 0)
                    ctx.output_tokens = usage_obj.get("output_tokens", 0)

                    details = usage_obj.get("input_tokens_details")
                    if isinstance(details, dict):
                        ctx.cached_tokens = details.get("cached_tokens", 0)

                # 如果没有收集到文本，从 output 中提取
                if not ctx.collected_text and "output" in response_obj:
                    for output_item in response_obj.get("output", []):
                        if output_item.get("type") != "message":
                            continue
                        for content_item in output_item.get("content", []):
                            if content_item.get("type") == "output_text":
                                text = content_item.get("text", "")
                                if text:
                                    ctx.append_text(text)

        # 备用：从顶层 usage 提取
        usage_obj = data.get("usage")
        if isinstance(usage_obj, dict) and not ctx.final_usage:
            ctx.final_usage = usage_obj
            ctx.input_tokens = usage_obj.get("input_tokens", 0)
            ctx.output_tokens = usage_obj.get("output_tokens", 0)

            details = usage_obj.get("input_tokens_details")
            if isinstance(details, dict):
                ctx.cached_tokens = details.get("cached_tokens", 0)

        # 备用：从 response 字段提取
        response_obj = data.get("response")
        if isinstance(response_obj, dict) and not ctx.final_response:
            ctx.final_response = response_obj

    def _extract_response_metadata(
        self,
        response: Dict[str, Any],
    ) -> Dict[str, Any]:
        """
        从 OpenAI 响应中提取元数据

        提取 model、status、response_id 等字段作为元数据。

        Args:
            response: OpenAI API 响应

        Returns:
            提取的元数据字典
        """
        metadata: Dict[str, Any] = {}

        # 提取模型名称（实际使用的模型）
        if "model" in response:
            metadata["model"] = response["model"]

        # 提取响应 ID
        if "id" in response:
            metadata["response_id"] = response["id"]

        # 提取状态
        if "status" in response:
            metadata["status"] = response["status"]

        # 提取对象类型
        if "object" in response:
            metadata["object"] = response["object"]

        # 提取系统指纹（如果存在）
        if "system_fingerprint" in response:
            metadata["system_fingerprint"] = response["system_fingerprint"]

        return metadata

    def _finalize_stream_metadata(self, ctx: StreamContext) -> None:
        """
        从流上下文中提取最终元数据

        在流传输完成后调用，从收集的事件中提取元数据。

        Args:
            ctx: 流上下文
        """
        # 从 response_id 提取响应 ID
        if ctx.response_id:
            ctx.response_metadata["response_id"] = ctx.response_id

        # 从 final_response 提取更多元数据
        if ctx.final_response and isinstance(ctx.final_response, dict):
            if "model" in ctx.final_response:
                ctx.response_metadata["model"] = ctx.final_response["model"]
            if "status" in ctx.final_response:
                ctx.response_metadata["status"] = ctx.final_response["status"]
            if "object" in ctx.final_response:
                ctx.response_metadata["object"] = ctx.final_response["object"]
            if "system_fingerprint" in ctx.final_response:
                ctx.response_metadata["system_fingerprint"] = ctx.final_response["system_fingerprint"]

        # 如果没有从响应中获取到 model，使用上下文中的
        if "model" not in ctx.response_metadata and ctx.model:
            ctx.response_metadata["model"] = ctx.model

