"""
统一的 Models API 端点

根据请求头认证方式自动返回对应格式:
- x-api-key + anthropic-version -> Claude 格式
- x-goog-api-key (header) 或 ?key= 参数 -> Gemini 格式
- Authorization: Bearer (bearer) -> OpenAI 格式
"""

from typing import Optional, Tuple, Union

from fastapi import APIRouter, Depends, Query, Request
from fastapi.responses import JSONResponse
from sqlalchemy.orm import Session

from src.api.base.models_service import (
    ModelInfo,
    find_model_by_id,
    get_available_provider_ids,
    list_available_models,
)
from src.core.api_format_metadata import API_FORMAT_DEFINITIONS, ApiFormatDefinition
from src.core.enums import APIFormat
from src.core.logger import logger
from src.database import get_db
from src.models.database import ApiKey, User
from src.services.auth.service import AuthService

router = APIRouter(tags=["Models API"])

# 各格式对应的 API 格式列表
# 注意: CLI 格式是透传格式，Models API 只返回非 CLI 格式的端点支持的模型
_CLAUDE_FORMATS = [APIFormat.CLAUDE.value]
_OPENAI_FORMATS = [APIFormat.OPENAI.value]
_GEMINI_FORMATS = [APIFormat.GEMINI.value]


def _extract_api_key_from_request(
    request: Request, definition: ApiFormatDefinition
) -> Optional[str]:
    """根据格式定义从请求中提取 API Key"""
    auth_header = definition.auth_header.lower()
    auth_type = definition.auth_type

    header_value = request.headers.get(auth_header)
    if not header_value:
        # Gemini 还支持 ?key= 参数
        if definition.api_format in (APIFormat.GEMINI, APIFormat.GEMINI_CLI):
            return request.query_params.get("key")
        return None

    if auth_type == "bearer":
        # Bearer token: "Bearer xxx"
        if header_value.lower().startswith("bearer "):
            return header_value[7:].strip()
        return None
    else:
        # header 类型: 直接使用值
        return header_value


def _detect_api_format_and_key(request: Request) -> Tuple[str, Optional[str]]:
    """
    根据请求头检测 API 格式并提取 API Key

    检测顺序:
    1. x-api-key + anthropic-version -> Claude
    2. x-goog-api-key (header) 或 ?key= -> Gemini
    3. Authorization: Bearer -> OpenAI (默认)

    Returns:
        (api_format, api_key) 元组
    """
    # Claude: x-api-key + anthropic-version (必须同时存在)
    claude_def = API_FORMAT_DEFINITIONS[APIFormat.CLAUDE]
    claude_key = _extract_api_key_from_request(request, claude_def)
    if claude_key and request.headers.get("anthropic-version"):
        return "claude", claude_key

    # Gemini: x-goog-api-key (header 类型) 或 ?key=
    gemini_def = API_FORMAT_DEFINITIONS[APIFormat.GEMINI]
    gemini_key = _extract_api_key_from_request(request, gemini_def)
    if gemini_key:
        return "gemini", gemini_key

    # OpenAI: Authorization: Bearer (默认)
    # 注意: 如果只有 x-api-key 但没有 anthropic-version，也走 OpenAI 格式
    openai_def = API_FORMAT_DEFINITIONS[APIFormat.OPENAI]
    openai_key = _extract_api_key_from_request(request, openai_def)
    # 如果 OpenAI 格式没有 key，但有 x-api-key，也用它（兼容）
    if not openai_key and claude_key:
        openai_key = claude_key
    return "openai", openai_key


def _get_formats_for_api(api_format: str) -> list[str]:
    """获取对应 API 格式的端点格式列表"""
    if api_format == "claude":
        return _CLAUDE_FORMATS
    elif api_format == "gemini":
        return _GEMINI_FORMATS
    else:
        return _OPENAI_FORMATS


def _authenticate(db: Session, api_key: Optional[str]) -> Tuple[Optional[User], Optional[ApiKey]]:
    """
    认证 API Key

    Returns:
        (user, api_key_record) 元组，认证失败返回 (None, None)
    """
    if not api_key:
        logger.debug("[Models] 认证失败: 未提供 API Key")
        return None, None

    result = AuthService.authenticate_api_key(db, api_key)
    if not result:
        logger.debug("[Models] 认证失败: API Key 无效")
        return None, None

    user, key_record = result
    logger.debug(f"[Models] 认证成功: {user.email} (Key: {key_record.name})")
    return result


def _build_auth_error_response(api_format: str) -> JSONResponse:
    """根据 API 格式构建认证错误响应"""
    if api_format == "claude":
        return JSONResponse(
            status_code=401,
            content={
                "type": "error",
                "error": {
                    "type": "authentication_error",
                    "message": "Invalid API key provided",
                },
            },
        )
    elif api_format == "gemini":
        return JSONResponse(
            status_code=401,
            content={
                "error": {
                    "code": 401,
                    "message": "API key not valid. Please pass a valid API key.",
                    "status": "UNAUTHENTICATED",
                }
            },
        )
    else:
        return JSONResponse(
            status_code=401,
            content={
                "error": {
                    "message": "Incorrect API key provided. You can find your API key at https://platform.openai.com/account/api-keys.",
                    "type": "invalid_request_error",
                    "param": None,
                    "code": "invalid_api_key",
                }
            },
        )


# ============================================================================
# 响应构建函数
# ============================================================================


def _build_claude_list_response(
    models: list[ModelInfo],
    before_id: Optional[str],
    after_id: Optional[str],
    limit: int,
) -> dict:
    """构建 Claude 格式的列表响应"""
    model_data_list = [
        {
            "id": m.id,
            "type": "model",
            "display_name": m.display_name,
            "created_at": m.created_at,
        }
        for m in models
    ]

    # 处理分页
    start_idx = 0
    if after_id:
        for i, m in enumerate(model_data_list):
            if m["id"] == after_id:
                start_idx = i + 1
                break

    end_idx = len(model_data_list)
    if before_id:
        for i, m in enumerate(model_data_list):
            if m["id"] == before_id:
                end_idx = i
                break

    paginated = model_data_list[start_idx:end_idx][:limit]

    first_id = paginated[0]["id"] if paginated else None
    last_id = paginated[-1]["id"] if paginated else None
    has_more = len(model_data_list[start_idx:end_idx]) > limit

    return {
        "data": paginated,
        "has_more": has_more,
        "first_id": first_id,
        "last_id": last_id,
    }


def _build_openai_list_response(models: list[ModelInfo]) -> dict:
    """构建 OpenAI 格式的列表响应"""
    data = [
        {
            "id": m.id,
            "object": "model",
            "created": m.created_timestamp,
            "owned_by": m.provider_name,
        }
        for m in models
    ]
    return {"object": "list", "data": data}


def _build_gemini_list_response(
    models: list[ModelInfo],
    page_size: int,
    page_token: Optional[str],
) -> dict:
    """构建 Gemini 格式的列表响应"""
    # 处理分页
    start_idx = 0
    if page_token:
        try:
            start_idx = int(page_token)
        except ValueError:
            start_idx = 0

    end_idx = start_idx + page_size
    paginated_models = models[start_idx:end_idx]

    models_data = [
        {
            "name": f"models/{m.id}",
            "baseModelId": m.id,
            "version": "001",
            "displayName": m.display_name,
            "description": m.description or f"Model {m.id}",
            "inputTokenLimit": 128000,
            "outputTokenLimit": 8192,
            "supportedGenerationMethods": ["generateContent", "countTokens"],
            "temperature": 1.0,
            "maxTemperature": 2.0,
            "topP": 0.95,
            "topK": 64,
        }
        for m in paginated_models
    ]

    response: dict = {"models": models_data}
    if end_idx < len(models):
        response["nextPageToken"] = str(end_idx)

    return response


def _build_claude_model_response(model_info: ModelInfo) -> dict:
    """构建 Claude 格式的模型详情响应"""
    return {
        "id": model_info.id,
        "type": "model",
        "display_name": model_info.display_name,
        "created_at": model_info.created_at,
    }


def _build_openai_model_response(model_info: ModelInfo) -> dict:
    """构建 OpenAI 格式的模型详情响应"""
    return {
        "id": model_info.id,
        "object": "model",
        "created": model_info.created_timestamp,
        "owned_by": model_info.provider_name,
    }


def _build_gemini_model_response(model_info: ModelInfo) -> dict:
    """构建 Gemini 格式的模型详情响应"""
    return {
        "name": f"models/{model_info.id}",
        "baseModelId": model_info.id,
        "version": "001",
        "displayName": model_info.display_name,
        "description": model_info.description or f"Model {model_info.id}",
        "inputTokenLimit": 128000,
        "outputTokenLimit": 8192,
        "supportedGenerationMethods": ["generateContent", "countTokens"],
        "temperature": 1.0,
        "maxTemperature": 2.0,
        "topP": 0.95,
        "topK": 64,
    }


# ============================================================================
# 404 响应
# ============================================================================


def _build_404_response(model_id: str, api_format: str) -> JSONResponse:
    """根据 API 格式构建 404 响应"""
    if api_format == "claude":
        return JSONResponse(
            status_code=404,
            content={
                "type": "error",
                "error": {"type": "not_found_error", "message": f"Model '{model_id}' not found"},
            },
        )
    elif api_format == "gemini":
        return JSONResponse(
            status_code=404,
            content={
                "error": {
                    "code": 404,
                    "message": f"models/{model_id} is not found",
                    "status": "NOT_FOUND",
                }
            },
        )
    else:
        return JSONResponse(
            status_code=404,
            content={
                "error": {
                    "message": f"The model '{model_id}' does not exist",
                    "type": "invalid_request_error",
                    "param": "model",
                    "code": "model_not_found",
                }
            },
        )


# ============================================================================
# 路由端点
# ============================================================================


@router.get("/v1/models", response_model=None)
async def list_models(
    request: Request,
    # Claude 分页参数
    before_id: Optional[str] = Query(None, description="返回此 ID 之前的结果 (Claude)"),
    after_id: Optional[str] = Query(None, description="返回此 ID 之后的结果 (Claude)"),
    limit: int = Query(20, ge=1, le=1000, description="返回数量限制 (Claude)"),
    # Gemini 分页参数
    page_size: int = Query(50, alias="pageSize", ge=1, le=1000, description="每页数量 (Gemini)"),
    page_token: Optional[str] = Query(None, alias="pageToken", description="分页 token (Gemini)"),
    db: Session = Depends(get_db),
) -> Union[dict, JSONResponse]:
    """
    List models - 根据请求头认证方式返回对应格式

    - x-api-key -> Claude 格式
    - x-goog-api-key 或 ?key= -> Gemini 格式
    - Authorization: Bearer -> OpenAI 格式
    """
    api_format, api_key = _detect_api_format_and_key(request)
    logger.info(f"[Models] GET /v1/models | format={api_format}")

    # 认证
    user, _ = _authenticate(db, api_key)
    if not user:
        return _build_auth_error_response(api_format)

    formats = _get_formats_for_api(api_format)

    available_provider_ids = get_available_provider_ids(db, formats)
    if not available_provider_ids:
        if api_format == "claude":
            return {"data": [], "has_more": False, "first_id": None, "last_id": None}
        elif api_format == "gemini":
            return {"models": []}
        else:
            return {"object": "list", "data": []}

    models = await list_available_models(db, available_provider_ids, formats)
    logger.debug(f"[Models] 返回 {len(models)} 个模型")

    if api_format == "claude":
        return _build_claude_list_response(models, before_id, after_id, limit)
    elif api_format == "gemini":
        return _build_gemini_list_response(models, page_size, page_token)
    else:
        return _build_openai_list_response(models)


@router.get("/v1/models/{model_id:path}", response_model=None)
async def retrieve_model(
    model_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Union[dict, JSONResponse]:
    """
    Retrieve model - 根据请求头认证方式返回对应格式
    """
    api_format, api_key = _detect_api_format_and_key(request)

    # Gemini 格式的 name 带 "models/" 前缀，需要移除
    if api_format == "gemini" and model_id.startswith("models/"):
        model_id = model_id[7:]

    logger.info(f"[Models] GET /v1/models/{model_id} | format={api_format}")

    # 认证
    user, _ = _authenticate(db, api_key)
    if not user:
        return _build_auth_error_response(api_format)

    formats = _get_formats_for_api(api_format)

    available_provider_ids = get_available_provider_ids(db, formats)
    model_info = find_model_by_id(db, model_id, available_provider_ids, formats)

    if not model_info:
        return _build_404_response(model_id, api_format)

    if api_format == "claude":
        return _build_claude_model_response(model_info)
    elif api_format == "gemini":
        return _build_gemini_model_response(model_info)
    else:
        return _build_openai_model_response(model_info)


# Gemini 专用路径 /v1beta/models
@router.get("/v1beta/models", response_model=None)
async def list_models_gemini(
    request: Request,
    page_size: int = Query(50, alias="pageSize", ge=1, le=1000),
    page_token: Optional[str] = Query(None, alias="pageToken"),
    db: Session = Depends(get_db),
) -> Union[dict, JSONResponse]:
    """List models (Gemini v1beta 端点)"""
    logger.info("[Models] GET /v1beta/models | format=gemini")

    # 从 x-goog-api-key 或 ?key= 提取 API Key
    gemini_def = API_FORMAT_DEFINITIONS[APIFormat.GEMINI]
    api_key = _extract_api_key_from_request(request, gemini_def)

    # 认证
    user, _ = _authenticate(db, api_key)
    if not user:
        return _build_auth_error_response("gemini")

    available_provider_ids = get_available_provider_ids(db, _GEMINI_FORMATS)
    if not available_provider_ids:
        return {"models": []}

    models = await list_available_models(db, available_provider_ids, _GEMINI_FORMATS)
    logger.debug(f"[Models] 返回 {len(models)} 个模型")
    response = _build_gemini_list_response(models, page_size, page_token)
    logger.debug(f"[Models] Gemini 响应: {response}")
    return response


@router.get("/v1beta/models/{model_name:path}", response_model=None)
async def get_model_gemini(
    request: Request,
    model_name: str,
    db: Session = Depends(get_db),
) -> Union[dict, JSONResponse]:
    """Get model (Gemini v1beta 端点)"""
    # 移除 "models/" 前缀（如果有）
    model_id = model_name[7:] if model_name.startswith("models/") else model_name
    logger.info(f"[Models] GET /v1beta/models/{model_id} | format=gemini")

    # 从 x-goog-api-key 或 ?key= 提取 API Key
    gemini_def = API_FORMAT_DEFINITIONS[APIFormat.GEMINI]
    api_key = _extract_api_key_from_request(request, gemini_def)

    # 认证
    user, _ = _authenticate(db, api_key)
    if not user:
        return _build_auth_error_response("gemini")

    available_provider_ids = get_available_provider_ids(db, _GEMINI_FORMATS)
    model_info = find_model_by_id(db, model_id, available_provider_ids, _GEMINI_FORMATS)

    if not model_info:
        return _build_404_response(model_id, "gemini")

    return _build_gemini_model_response(model_info)
