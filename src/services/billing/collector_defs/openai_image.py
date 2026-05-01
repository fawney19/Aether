from __future__ import annotations

from typing import Any

# OpenAI image generations (/v1/images/generations)
# 响应结构（gpt-image-1 / gpt-image-2 等）：
#   {
#     "created": 1234,
#     "data": [{"b64_json": "..."}, ...],
#     "usage": {
#       "input_tokens": <int>,
#       "output_tokens": <int>,
#       "total_tokens": <int>,
#       "input_tokens_details": {"text_tokens": <int>, "image_tokens": <int>}
#     }
#   }
#
# 流式（stream=true）会以 SSE 推送 partial_image 事件，最终 image_generation.completed
# 帧内含 "usage"。由 handler 负责从尾帧提取 usage 后写入 response_body，
# 此处采集器按 usage.* 路径读取即可（复用 chat 的 token 计费管线）。
COLLECTORS: list[dict[str, Any]] = [
    {
        "api_format": "openai:image",
        "task_type": "image",
        "dimension_name": "input_tokens",
        "source_type": "response",
        "source_path": "usage.input_tokens",
        "value_type": "int",
        "priority": 10,
        "is_enabled": True,
    },
    {
        "api_format": "openai:image",
        "task_type": "image",
        "dimension_name": "output_tokens",
        "source_type": "response",
        "source_path": "usage.output_tokens",
        "value_type": "int",
        "priority": 10,
        "is_enabled": True,
    },
    # 图片数量（用于"固定单价 × n"计费场景）
    {
        "api_format": "openai:image",
        "task_type": "image",
        "dimension_name": "image_count",
        "source_type": "request",
        "source_path": "n",
        "value_type": "int",
        "default_value": "1",
        "priority": 10,
        "is_enabled": True,
    },
    # 图片尺寸 / 质量（供阶梯定价使用；若未启用则忽略即可）
    {
        "api_format": "openai:image",
        "task_type": "image",
        "dimension_name": "image_size",
        "source_type": "request",
        "source_path": "size",
        "value_type": "string",
        "priority": 10,
        "is_enabled": True,
    },
    {
        "api_format": "openai:image",
        "task_type": "image",
        "dimension_name": "image_quality",
        "source_type": "request",
        "source_path": "quality",
        "value_type": "string",
        "priority": 10,
        "is_enabled": True,
    },
]
