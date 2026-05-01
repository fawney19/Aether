"""
OpenAI API 数据模型定义
"""

from typing import Any

from pydantic import BaseModel, ConfigDict


# 配置允许额外字段，以支持 API 的新特性
class BaseModelWithExtras(BaseModel):
    model_config = ConfigDict(extra="allow")


class OpenAIMessage(BaseModelWithExtras):
    """OpenAI消息模型"""

    role: str
    content: str | list[dict[str, Any]] | None = None
    tool_calls: list[dict[str, Any]] | None = None
    tool_call_id: str | None = None
    name: str | None = None


class OpenAIFunction(BaseModelWithExtras):
    """OpenAI函数定义"""

    name: str
    description: str | None = None
    parameters: dict[str, Any]


class OpenAITool(BaseModelWithExtras):
    """OpenAI工具定义"""

    type: str = "function"
    function: OpenAIFunction


class OpenAIRequest(BaseModelWithExtras):
    """OpenAI请求模型"""

    model: str
    messages: list[OpenAIMessage]
    max_tokens: int | None = None
    temperature: float | None = 1.0
    top_p: float | None = None
    stream: bool | None = False
    stop: str | list[str] | None = None
    tools: list[OpenAITool] | None = None
    tool_choice: str | dict[str, Any] | None = None
    presence_penalty: float | None = None
    frequency_penalty: float | None = None
    n: int | None = None
    seed: int | None = None
    response_format: dict[str, Any] | None = None
    logit_bias: dict[str, float] | None = None
    logprobs: bool | None = None
    top_logprobs: int | None = None
    user: str | None = None


class ResponsesInputMessage(BaseModelWithExtras):
    """Responses API 输入消息"""

    type: str = "message"
    role: str
    content: list[dict[str, Any]]


class ResponsesReasoningConfig(BaseModelWithExtras):
    """Responses API 推理配置"""

    effort: str = "high"  # low, medium, high
    summary: str = "auto"  # auto, off


class ResponsesRequest(BaseModelWithExtras):
    """OpenAI Responses API 请求模型（用于 Claude Code 等客户端）"""

    model: str
    instructions: str | None = None
    input: list[ResponsesInputMessage]
    tools: list[dict[str, Any]] | None = None
    tool_choice: str | dict[str, Any] | None = "auto"
    parallel_tool_calls: bool | None = False
    reasoning: ResponsesReasoningConfig | None = None
    store: bool | None = False
    stream: bool | None = True
    include: list[str] | None = None
    prompt_cache_key: str | None = None
    # 其他参数
    max_tokens: int | None = None
    temperature: float | None = None
    top_p: float | None = None
    stop: str | list[str] | None = None


class OpenAIUsage(BaseModelWithExtras):
    """OpenAI使用统计"""

    prompt_tokens: int
    completion_tokens: int
    total_tokens: int


class OpenAIChoice(BaseModelWithExtras):
    """OpenAI选择结果"""

    index: int
    message: OpenAIMessage
    finish_reason: str | None = None
    logprobs: dict[str, Any] | None = None


class OpenAIResponse(BaseModelWithExtras):
    """OpenAI响应模型"""

    id: str
    object: str = "chat.completion"
    created: int
    model: str
    choices: list[OpenAIChoice]
    usage: OpenAIUsage | None = None
    system_fingerprint: str | None = None


class OpenAIStreamDelta(BaseModelWithExtras):
    """OpenAI流式响应增量"""

    role: str | None = None
    content: str | None = None
    tool_calls: list[dict[str, Any]] | None = None


class OpenAIStreamChoice(BaseModelWithExtras):
    """OpenAI流式响应选择"""

    index: int
    delta: OpenAIStreamDelta
    finish_reason: str | None = None
    logprobs: dict[str, Any] | None = None


class OpenAIStreamResponse(BaseModelWithExtras):
    """OpenAI流式响应模型"""

    id: str
    object: str = "chat.completion.chunk"
    created: int
    model: str
    choices: list[OpenAIStreamChoice]
    system_fingerprint: str | None = None


class OpenAIImageRequest(BaseModelWithExtras):
    """OpenAI Images Generations 请求（/v1/images/generations）

    覆盖 OpenAI 官方规范；`extra="allow"` 允许上游扩展字段透传。
    只做必要校验，尽量宽松以兼容各种"OpenAI 兼容"上游。
    """

    model: str
    prompt: str
    n: int | None = None
    size: str | None = None
    quality: str | None = None
    style: str | None = None
    response_format: str | None = None
    background: str | None = None
    output_format: str | None = None
    output_compression: int | None = None
    moderation: str | None = None
    user: str | None = None
    stream: bool | None = False
    partial_images: int | None = None
