"""
Gemini API 专属端点

托管 Gemini API 相关路由:
- /v1beta/models/{model}:generateContent
- /v1beta/models/{model}:streamGenerateContent

注意:
- Gemini API 的 model 在 URL 路径中，而不是请求体中
- /v1beta/models (列表) 和 /v1beta/models/{model} (详情) 由 models.py 统一处理
"""

from fastapi import APIRouter, Depends, Request
from sqlalchemy.orm import Session

from src.api.base.pipeline import ApiRequestPipeline
from src.api.handlers.gemini import build_gemini_adapter
from src.api.handlers.gemini_cli import build_gemini_cli_adapter
from src.core.api_format_metadata import get_api_format_definition
from src.core.enums import APIFormat
from src.database import get_db

# 从配置获取路径前缀
_gemini_def = get_api_format_definition(APIFormat.GEMINI)

router = APIRouter(tags=["Gemini API"], prefix=_gemini_def.path_prefix)
pipeline = ApiRequestPipeline()


def _is_cli_request(request: Request) -> bool:
    """
    判断是否为 CLI 请求

    检查顺序:
    1. x-app header 包含 "cli"
    2. user-agent 包含 "GeminiCLI" 或 "gemini-cli"
    """
    # 检查 x-app header
    x_app = request.headers.get("x-app", "")
    if "cli" in x_app.lower():
        return True

    # 检查 user-agent
    user_agent = request.headers.get("user-agent", "")
    user_agent_lower = user_agent.lower()
    if "geminicli" in user_agent_lower or "gemini-cli" in user_agent_lower:
        return True

    return False


@router.post("/v1beta/models/{model}:generateContent")
async def generate_content(
    model: str,
    http_request: Request,
    db: Session = Depends(get_db),
):
    """
    Gemini generateContent 端点

    非流式生成内容请求
    """
    # 根据 user-agent 或 x-app header 选择适配器
    if _is_cli_request(http_request):
        adapter = build_gemini_cli_adapter()
    else:
        adapter = build_gemini_adapter()

    return await pipeline.run(
        adapter=adapter,
        http_request=http_request,
        db=db,
        mode=adapter.mode,
        api_format_hint=adapter.allowed_api_formats[0],
        # 将 model 注入到请求体中，stream 用于内部判断流式模式
        path_params={"model": model, "stream": False},
    )


@router.post("/v1beta/models/{model}:streamGenerateContent")
async def stream_generate_content(
    model: str,
    http_request: Request,
    db: Session = Depends(get_db),
):
    """
    Gemini streamGenerateContent 端点

    流式生成内容请求

    注意: Gemini API 通过 URL 端点区分流式/非流式，不需要在请求体中添加 stream 字段
    """
    # 根据 user-agent 或 x-app header 选择适配器
    if _is_cli_request(http_request):
        adapter = build_gemini_cli_adapter()
    else:
        adapter = build_gemini_adapter()

    return await pipeline.run(
        adapter=adapter,
        http_request=http_request,
        db=db,
        mode=adapter.mode,
        api_format_hint=adapter.allowed_api_formats[0],
        # model 注入到请求体，stream 用于内部判断流式模式（不发送到 API）
        path_params={"model": model, "stream": True},
    )


# 兼容 v1 路径（部分 SDK 可能使用 generateContent）
@router.post("/v1/models/{model}:generateContent")
async def generate_content_v1(
    model: str,
    http_request: Request,
    db: Session = Depends(get_db),
):
    """v1 兼容端点"""
    return await generate_content(model, http_request, db)


@router.post("/v1/models/{model}:streamGenerateContent")
async def stream_generate_content_v1(
    model: str,
    http_request: Request,
    db: Session = Depends(get_db),
):
    """v1 兼容端点"""
    return await stream_generate_content(model, http_request, db)
