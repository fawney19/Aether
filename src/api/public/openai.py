"""
OpenAI API 端点

- /v1/chat/completions - OpenAI Chat API
- /v1/responses - OpenAI Responses API (CLI)

注意: /v1/models 端点由 models.py 统一处理，根据请求头返回对应格式
"""

from fastapi import APIRouter, Depends, Request
from sqlalchemy.orm import Session

from src.api.base.pipeline import ApiRequestPipeline
from src.api.handlers.openai import OpenAIChatAdapter
from src.api.handlers.openai_cli import OpenAICliAdapter
from src.core.api_format_metadata import get_api_format_definition
from src.core.enums import APIFormat
from src.database import get_db

_openai_def = get_api_format_definition(APIFormat.OPENAI)
router = APIRouter(tags=["OpenAI API"], prefix=_openai_def.path_prefix)
pipeline = ApiRequestPipeline()


@router.post("/v1/chat/completions")
async def create_chat_completion(
    http_request: Request,
    db: Session = Depends(get_db),
):
    adapter = OpenAIChatAdapter()
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
):
    adapter = OpenAICliAdapter()
    return await pipeline.run(
        adapter=adapter,
        http_request=http_request,
        db=db,
        mode=adapter.mode,
        api_format_hint=adapter.allowed_api_formats[0],
    )
