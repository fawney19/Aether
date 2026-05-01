"""
OpenAI API 端点

- /v1/chat/completions - OpenAI Chat API
- /v1/responses - OpenAI Responses API (CLI)
- /v1/responses/compact - OpenAI Responses Compaction API (CLI)
- /v1/images/generations - OpenAI Images Generations API

注意: /v1/models 端点由 models.py 统一处理，根据请求头返回对应格式
"""

from typing import Any

from fastapi import APIRouter, Depends, Request
from sqlalchemy.orm import Session

from src.api.base.pipeline import get_pipeline
from src.database import get_db

router = APIRouter(tags=["OpenAI API"])
pipeline = get_pipeline()


@router.post("/v1/chat/completions")
async def create_chat_completion(
    http_request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    OpenAI Chat Completions API

    兼容 OpenAI Chat Completions API 格式的代理接口。

    **认证方式**: Bearer Token（API Key 或 JWT Token）

    **请求格式**:
    ```json
    {
        "model": "gpt-4",
        "messages": [{"role": "user", "content": "Hello"}],
        "stream": false
    }
    ```

    **支持的参数**: model, messages, stream, temperature, max_tokens 等标准 OpenAI 参数
    """
    from src.api.handlers.openai import OpenAIChatAdapter

    adapter = OpenAIChatAdapter()
    return await pipeline.run(
        adapter=adapter,
        http_request=http_request,
        db=db,
        mode=adapter.mode,
        api_format_hint=adapter.allowed_api_formats[0],
    )


@router.post("/v1/responses/compact")
async def create_responses_compact(
    http_request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    OpenAI Responses Compaction API (CLI)

    用于压缩/总结之前的 responses，永远非流式。
    Codex CLI 使用 compact 模型后缀（如 gpt-5-compact）时调用此端点。

    **认证方式**: Bearer Token（API Key 或 JWT Token）
    """
    from src.api.handlers.openai_cli import OpenAICompactAdapter

    adapter = OpenAICompactAdapter()
    return await pipeline.run(
        adapter=adapter,
        http_request=http_request,
        db=db,
        mode=adapter.mode,
        api_format_hint=adapter.allowed_api_formats[0],
    )


@router.post("/v1/responses")
async def create_responses(
    http_request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    OpenAI Responses API (CLI)

    兼容 OpenAI Codex CLI 使用的 Responses API 格式，请求透传到上游。

    **认证方式**: Bearer Token（API Key 或 JWT Token）
    """
    from src.api.handlers.openai_cli import OpenAICliAdapter

    adapter = OpenAICliAdapter()
    return await pipeline.run(
        adapter=adapter,
        http_request=http_request,
        db=db,
        mode=adapter.mode,
        api_format_hint=adapter.allowed_api_formats[0],
    )


@router.post("/v1/images/generations")
async def create_image_generation(
    http_request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    OpenAI Images Generations API

    兼容 OpenAI 原生生图端点。支持同步响应与 SSE 流式（`stream=true`，
    推送 `image_generation.partial_image / .completed` 事件）。

    **认证方式**: Bearer Token（API Key 或 JWT Token）

    **请求示例**:
    ```json
    {
        "model": "gpt-image-1",
        "prompt": "a cute cat",
        "n": 1,
        "size": "1024x1024",
        "stream": false
    }
    ```
    """
    from src.api.handlers.openai import OpenAIImageAdapter

    adapter = OpenAIImageAdapter(endpoint_operation="generations")
    return await pipeline.run(
        adapter=adapter,
        http_request=http_request,
        db=db,
        mode=adapter.mode,
        api_format_hint=adapter.allowed_api_formats[0],
    )


@router.post("/v1/images/edits")
async def create_image_edit(
    http_request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    OpenAI Images Edits API

    接受 multipart/form-data：`image`（必需）、`prompt`（必需）、`mask`（可选）、
    `model`、`n`、`size`、`response_format`、`user` 等。

    **认证方式**: Bearer Token（API Key 或 JWT Token）
    """
    from src.api.handlers.openai import OpenAIImageAdapter

    adapter = OpenAIImageAdapter(endpoint_operation="edits")
    return await pipeline.run(
        adapter=adapter,
        http_request=http_request,
        db=db,
        mode=adapter.mode,
        api_format_hint=adapter.allowed_api_formats[0],
    )
