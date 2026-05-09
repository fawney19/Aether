"""
OpenAI API 端点

- /v1/chat/completions - OpenAI Chat API
- /v1/responses - OpenAI Responses API (CLI)
- /v1/responses/compact - OpenAI Responses Compaction API (CLI)

注意: /v1/models 端点由 models.py 统一处理，根据请求头返回对应格式
"""

from typing import Any

from fastapi import APIRouter, Depends, Request, WebSocket
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


@router.websocket("/v1/responses")
async def create_responses_websocket(websocket: WebSocket) -> None:
    """
    OpenAI Responses WebSocket API (Codex CLI)

    新版 Codex CLI 可通过 WebSocket 连接 /v1/responses。业务处理仍复用
    HTTP Responses pipeline，仅在这里完成 WS JSON frame 与 SSE 事件之间的转换。
    """
    from src.api.handlers.openai_cli.ws_bridge import handle_codex_responses_websocket

    await handle_codex_responses_websocket(websocket, pipeline=pipeline)
