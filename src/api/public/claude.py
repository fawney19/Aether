"""
Claude API 端点

- /v1/messages - Claude Messages API
- /v1/messages/count_tokens - Token Count API

注意: /v1/models 端点由 models.py 统一处理，根据请求头返回对应格式
"""

from fastapi import APIRouter, Depends, Request
from sqlalchemy.orm import Session

from src.api.base.pipeline import ApiRequestPipeline
from src.api.handlers.claude import (
    ClaudeTokenCountAdapter,
    build_claude_adapter,
)
from src.core.api_format_metadata import get_api_format_definition
from src.core.enums import APIFormat
from src.database import get_db

_claude_def = get_api_format_definition(APIFormat.CLAUDE)
router = APIRouter(tags=["Claude API"], prefix=_claude_def.path_prefix)
pipeline = ApiRequestPipeline()


@router.post("/v1/messages")
async def create_message(
    http_request: Request,
    db: Session = Depends(get_db),
):
    """统一入口：根据 x-app 自动在标准/Claude Code 之间切换。"""
    adapter = build_claude_adapter(http_request.headers.get("x-app", ""))
    return await pipeline.run(
        adapter=adapter,
        http_request=http_request,
        db=db,
        mode=adapter.mode,
        api_format_hint=adapter.allowed_api_formats[0],
    )


@router.post("/v1/messages/count_tokens")
async def count_tokens(
    http_request: Request,
    db: Session = Depends(get_db),
):
    adapter = ClaudeTokenCountAdapter()
    return await pipeline.run(
        adapter=adapter,
        http_request=http_request,
        db=db,
        mode=adapter.mode,
    )
