"""
Gemini CLI Message Handler - 基于通用 CLI Handler 基类的实现

继承 CliMessageHandlerBase，处理 Gemini CLI API 格式的请求。
"""

from typing import Any, Dict, Optional

from src.api.handlers.base.cli_handler_base import (
    CliMessageHandlerBase,
    StreamContext,
)


class GeminiCliMessageHandler(CliMessageHandlerBase):
    """
    Gemini CLI Message Handler - 处理 Gemini CLI API 格式

    使用新三层架构 (Provider -> ProviderEndpoint -> ProviderAPIKey)
    通过 FallbackOrchestrator 实现自动故障转移、健康监控和并发控制

    响应格式特点：
    - Gemini 使用 JSON 数组格式流式响应（非 SSE）
    - 每个 chunk 包含 candidates、usageMetadata 等字段
    - finish_reason: STOP, MAX_TOKENS, SAFETY, RECITATION, OTHER
    - Token 使用: promptTokenCount (输入), thoughtsTokenCount + candidatesTokenCount (输出), cachedContentTokenCount (缓存)

    Gemini API 特殊处理：
    - model 在 URL 路径中而非请求体，如 /v1beta/models/{model}:generateContent
    - 请求体中的 model 字段用于内部路由，不发送给 API
    """

    FORMAT_ID = "GEMINI_CLI"

    def extract_model_from_request(
        self,
        request_body: Dict[str, Any],  # noqa: ARG002 - 基类签名要求
        path_params: Optional[Dict[str, Any]] = None,
    ) -> str:
        """
        从请求中提取模型名 - Gemini 格式实现

        Gemini API 的 model 在 URL 路径中而非请求体：
        /v1beta/models/{model}:generateContent

        Args:
            request_body: 请求体（Gemini 不包含 model）
            path_params: URL 路径参数（包含 model）

        Returns:
            模型名，如果无法提取则返回 "unknown"
        """
        # Gemini: model 从 URL 路径参数获取
        if path_params and "model" in path_params:
            return str(path_params["model"])
        return "unknown"

    def prepare_provider_request_body(
        self,
        request_body: Dict[str, Any],
    ) -> Dict[str, Any]:
        """
        准备发送给 Gemini API 的请求体 - 移除 model 字段

        Gemini API 要求 model 只在 URL 路径中，请求体中的 model 字段
        会导致某些代理返回 404 错误。

        Args:
            request_body: 请求体

        Returns:
            不含 model 字段的请求体
        """
        result = dict(request_body)
        result.pop("model", None)
        return result

    def get_model_for_url(
        self,
        request_body: Dict[str, Any],
        mapped_model: Optional[str],
    ) -> Optional[str]:
        """
        Gemini 需要将 model 放入 URL 路径中

        Args:
            request_body: 请求体
            mapped_model: 映射后的模型名（如果有）

        Returns:
            用于 URL 路径的模型名
        """
        # 优先使用映射后的模型名，否则使用请求体中的
        return mapped_model or request_body.get("model")

    def _extract_usage_from_event(self, event: Dict[str, Any]) -> Dict[str, int]:
        """
        从 Gemini 事件中提取 token 使用情况

        调用 GeminiStreamParser.extract_usage 作为单一实现源

        Args:
            event: Gemini 流式响应事件

        Returns:
            包含 input_tokens, output_tokens, cached_tokens 的字典
        """
        from src.api.handlers.gemini.stream_parser import GeminiStreamParser

        usage = GeminiStreamParser().extract_usage(event)

        if not usage:
            return {
                "input_tokens": 0,
                "output_tokens": 0,
                "cached_tokens": 0,
            }

        return {
            "input_tokens": usage.get("input_tokens", 0),
            "output_tokens": usage.get("output_tokens", 0),
            "cached_tokens": usage.get("cached_tokens", 0),
        }

    def _process_event_data(
        self,
        ctx: StreamContext,
        _event_type: str,
        data: Dict[str, Any],
    ) -> None:
        """
        处理 Gemini CLI 格式的流式事件

        Gemini 的流式响应是 JSON 数组格式，每个元素结构如下:
        {
            "candidates": [{
                "content": {"parts": [{"text": "..."}], "role": "model"},
                "finishReason": "STOP",
                "safetyRatings": [...]
            }],
            "usageMetadata": {
                "promptTokenCount": 10,
                "candidatesTokenCount": 20,
                "totalTokenCount": 30,
                "cachedContentTokenCount": 5
            },
            "modelVersion": "gemini-1.5-pro"
        }

        注意: Gemini 流解析器会将每个 JSON 对象作为一个"事件"传递
        event_type 在这里可能为空或是自定义的标记
        """
        # 提取候选响应
        candidates = data.get("candidates", [])
        if candidates:
            candidate = candidates[0]
            content = candidate.get("content", {})

            # 提取文本内容
            parts = content.get("parts", [])
            for part in parts:
                if "text" in part:
                    ctx.append_text(part["text"])

            # 检查结束原因
            finish_reason = candidate.get("finishReason")
            if finish_reason in ("STOP", "MAX_TOKENS", "SAFETY", "RECITATION", "OTHER"):
                ctx.has_completion = True
                ctx.final_response = data

        # 提取使用量信息（复用 GeminiStreamParser.extract_usage）
        usage = self._extract_usage_from_event(data)
        if usage["input_tokens"] > 0 or usage["output_tokens"] > 0:
            ctx.input_tokens = usage["input_tokens"]
            ctx.output_tokens = usage["output_tokens"]
            ctx.cached_tokens = usage["cached_tokens"]

        # 提取模型版本作为响应 ID
        model_version = data.get("modelVersion")
        if model_version:
            if not ctx.response_id:
                ctx.response_id = f"gemini-{model_version}"
            # 存储到 response_metadata 供 Usage 记录使用
            ctx.response_metadata["model_version"] = model_version

        # 检查错误
        if "error" in data:
            ctx.has_completion = True
            ctx.final_response = data

    def _extract_response_metadata(
        self,
        response: Dict[str, Any],
    ) -> Dict[str, Any]:
        """
        从 Gemini 响应中提取元数据

        提取 modelVersion 字段，记录实际使用的模型版本。

        Args:
            response: Gemini API 响应

        Returns:
            包含 model_version 的元数据字典
        """
        metadata: Dict[str, Any] = {}
        model_version = response.get("modelVersion")
        if model_version:
            metadata["model_version"] = model_version
        return metadata
