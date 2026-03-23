"""
统一的 Models API 端点

根据请求头认证方式自动返回对应格式:
- x-api-key + anthropic-version -> Claude 格式
- x-goog-api-key (header) 或 ?key= 参数 -> Gemini 格式
- Authorization: Bearer (bearer) -> OpenAI 格式
"""

from fastapi import APIRouter, Depends, Query, Request
from fastapi.responses import JSONResponse
from sqlalchemy.orm import Session

from src.api.base.models_service import (
    AccessRestrictions,
    ModelInfo,
    find_model_by_id,
    get_available_provider_ids,
    get_compatible_provider_formats,
    list_available_models,
)
from src.core.api_format import (
    detect_request_context,
)
from src.core.api_format.conversion import (
    format_conversion_registry,
    register_default_normalizers,
)
from src.core.logger import logger
from src.database import get_db
from src.models.database import ApiKey, User
from src.services.auth.service import AuthService
from src.services.provider.format import normalize_endpoint_signature

router = APIRouter(tags=["System Catalog"])
OPENAI_MODEL_OWNER = "aether"

# 各格式对应的 API 格式列表（包括对应的 CLI 格式）
_CLAUDE_FORMATS = ["claude:chat", "claude:cli"]
_OPENAI_FORMATS = ["openai:chat", "openai:cli", "openai:compact"]
_GEMINI_FORMATS = ["gemini:chat", "gemini:cli"]

# 所有格式（用于格式转换时的查询）
_ALL_CHAT_FORMATS = [
    *_CLAUDE_FORMATS,
    *_OPENAI_FORMATS,
    *_GEMINI_FORMATS,
]


def _detect_api_format_and_key(request: Request) -> tuple[str, str | None]:
    """
    根据请求头检测 API 格式并提取 API Key

    检测顺序:
    1. x-api-key + anthropic-version -> Claude
    2. x-goog-api-key (header) 或 ?key= -> Gemini
    3. Authorization: Bearer -> OpenAI (默认)

    Returns:
        (api_format, api_key) 元组
    """
    context = detect_request_context(request)
    return context.endpoint.key, context.credentials


def _get_formats_for_api(api_format: str) -> list[str]:
    """获取对应 API 格式的端点格式列表"""
    fam = (api_format.split(":", 1)[0] if api_format else "").strip().lower()
    if fam == "claude":
        return _CLAUDE_FORMATS
    if fam == "gemini":
        return _GEMINI_FORMATS
    return _OPENAI_FORMATS


def _is_format_conversion_enabled(db: Session) -> bool:
    """检查全局格式转换开关（从数据库配置读取，默认开启）"""
    from src.services.system.config import SystemConfigService

    return SystemConfigService.is_format_conversion_enabled(db)


def _get_convertible_formats(client_format: str) -> list[str]:
    """
    获取客户端格式可转换到的所有目标格式列表

    始终返回所有有转换器的格式（包括客户端格式本身），
    由下游 get_compatible_provider_formats 按三层开关（全局/Provider/端点）精确过滤。
    """
    client_format_norm = normalize_endpoint_signature(client_format)

    # 收集所有可转换的格式
    register_default_normalizers()
    convertible_formats: list[str] = []
    for target_format in _ALL_CHAT_FORMATS:
        target_norm = normalize_endpoint_signature(target_format)
        # 相同格式始终可用
        if target_norm == client_format_norm:
            convertible_formats.append(target_norm)
            continue

        # 检查是否有双向转换器
        if format_conversion_registry.can_convert_full(
            client_format_norm,
            target_norm,
            require_stream=False,
        ):
            convertible_formats.append(target_norm)

    # 去重并保持稳定顺序
    return list(dict.fromkeys(convertible_formats)) if convertible_formats else [client_format_norm]


def _flatten_provider_formats(provider_to_formats: dict[str, set[str]]) -> list[str]:
    """合并 Provider 格式映射为唯一格式列表"""
    if not provider_to_formats:
        return []
    all_formats: set[str] = set()
    for formats in provider_to_formats.values():
        all_formats.update(formats)
    return sorted(all_formats)


def _get_family(api_format: str) -> str:
    """从 endpoint signature 提取协议族（如 'openai:chat' -> 'openai'）。"""
    return (str(api_format).split(":", 1)[0] if api_format else "").strip().lower()


def _build_empty_list_response(api_format: str) -> dict:
    """根据 API 格式构建空列表响应"""
    fam = _get_family(api_format)
    if fam == "claude":
        return {"data": [], "has_more": False, "first_id": None, "last_id": None}
    elif fam == "gemini":
        return {"models": []}
    else:
        return {"object": "list", "data": []}


def _filter_formats_by_restrictions(
    formats: list[str], restrictions: AccessRestrictions, api_format: str
) -> tuple[list[str], dict | None]:
    """
    根据访问限制过滤 API 格式

    Returns:
        (过滤后的格式列表, 空响应或None)
        如果过滤后为空，返回对应格式的空响应
    """
    if restrictions.allowed_api_formats is None:
        return formats, None
    filtered = [f for f in formats if restrictions.is_api_format_allowed(f)]
    if not filtered:
        logger.info(f"[Models] API Key 不允许访问格式 {api_format}")
        return [], _build_empty_list_response(api_format)
    return filtered, None


def _authenticate(db: Session, api_key: str | None) -> tuple[User | None, ApiKey | None]:
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
    fam = _get_family(api_format)
    if fam == "claude":
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
    elif fam == "gemini":
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
    before_id: str | None,
    after_id: str | None,
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
            "owned_by": OPENAI_MODEL_OWNER,
        }
        for m in models
    ]
    return {"object": "list", "data": data}


def _build_gemini_list_response(
    models: list[ModelInfo],
    page_size: int,
    page_token: str | None,
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
            "inputTokenLimit": m.context_limit if m.context_limit is not None else 128000,
            "outputTokenLimit": m.output_limit if m.output_limit is not None else 8192,
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
        "owned_by": OPENAI_MODEL_OWNER,
    }


def _build_gemini_model_response(model_info: ModelInfo) -> dict:
    """构建 Gemini 格式的模型详情响应"""
    return {
        "name": f"models/{model_info.id}",
        "baseModelId": model_info.id,
        "version": "001",
        "displayName": model_info.display_name,
        "description": model_info.description or f"Model {model_info.id}",
        "inputTokenLimit": (
            model_info.context_limit if model_info.context_limit is not None else 128000
        ),
        "outputTokenLimit": (
            model_info.output_limit if model_info.output_limit is not None else 8192
        ),
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
    fam = _get_family(api_format)
    if fam == "claude":
        return JSONResponse(
            status_code=404,
            content={
                "type": "error",
                "error": {"type": "not_found_error", "message": f"Model '{model_id}' not found"},
            },
        )
    elif fam == "gemini":
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
    before_id: str | None = Query(None, description="返回此 ID 之前的结果 (Claude)"),
    after_id: str | None = Query(None, description="返回此 ID 之后的结果 (Claude)"),
    limit: int = Query(20, ge=1, le=1000, description="返回数量限制 (Claude)"),
    # Gemini 分页参数
    page_size: int = Query(50, alias="pageSize", ge=1, le=1000, description="每页数量 (Gemini)"),
    page_token: str | None = Query(None, alias="pageToken", description="分页 token (Gemini)"),
    db: Session = Depends(get_db),
) -> dict | JSONResponse:
    """
    列出可用模型（统一端点）

    根据请求头中的认证方式自动检测 API 格式，并返回相应格式的模型列表。
    此接口兼容 Claude、OpenAI 和 Gemini 三种 API 格式。

    **格式检测规则**
    - x-api-key + anthropic-version → Claude 格式
    - x-goog-api-key 或 ?key= → Gemini 格式
    - Authorization: Bearer → OpenAI 格式（默认）

    **查询参数**

    Claude 格式：
    - before_id: 返回此 ID 之前的结果，用于向前分页
    - after_id: 返回此 ID 之后的结果，用于向后分页
    - limit: 返回数量限制，默认 20，范围 1-1000

    Gemini 格式：
    - pageSize: 每页数量，默认 50，范围 1-1000
    - pageToken: 分页 token，用于获取下一页

    **返回字段**

    Claude 格式：
    - data: 模型列表，每个模型包含：
      - id: 模型标识符
      - type: "model"
      - display_name: 显示名称
      - created_at: 创建时间（ISO 8601 格式）
    - has_more: 是否有更多结果
    - first_id: 当前页第一个模型 ID
    - last_id: 当前页最后一个模型 ID

    OpenAI 格式：
    - object: "list"
    - data: 模型列表，每个模型包含：
      - id: 模型标识符
      - object: "model"
      - created: Unix 时间戳
      - owned_by: 提供商名称

    Gemini 格式：
    - models: 模型列表，每个模型包含：
      - name: 模型资源名称（如 models/gemini-pro）
      - baseModelId: 基础模型 ID
      - version: 版本号
      - displayName: 显示名称
      - description: 描述信息
      - inputTokenLimit: 输入 token 上限
      - outputTokenLimit: 输出 token 上限
      - supportedGenerationMethods: 支持的生成方法
      - temperature: 默认温度参数
      - maxTemperature: 最大温度参数
      - topP: Top-P 参数
      - topK: Top-K 参数
    - nextPageToken: 下一页的 token（如果有更多结果）

    **错误响应**
    401: API Key 无效或未提供（格式根据检测到的 API 格式返回）
    """
    api_format, api_key = _detect_api_format_and_key(request)
    logger.info(f"[Models] GET /v1/models | format={api_format}")

    # 认证
    user, key_record = _authenticate(db, api_key)
    if not user:
        return _build_auth_error_response(api_format)

    # 构建访问限制
    restrictions = AccessRestrictions.from_api_key_and_user(key_record, user)

    # 获取可用格式（包括可转换的格式）
    global_conversion_enabled = _is_format_conversion_enabled(db)
    candidate_formats = _get_convertible_formats(api_format)
    candidate_formats, empty_response = _filter_formats_by_restrictions(
        candidate_formats, restrictions, api_format
    )
    if empty_response is not None:
        return empty_response

    provider_to_formats = get_compatible_provider_formats(
        db, api_format, candidate_formats, global_conversion_enabled
    )
    formats = _flatten_provider_formats(provider_to_formats)

    available_provider_ids = get_available_provider_ids(db, formats, provider_to_formats)
    if not available_provider_ids:
        return _build_empty_list_response(api_format)

    models = await list_available_models(
        db,
        available_provider_ids,
        formats,
        restrictions,
        provider_to_formats=provider_to_formats,
        client_format=api_format,
    )
    logger.debug(f"[Models] 返回 {len(models)} 个模型")

    if _get_family(api_format) == "claude":
        return _build_claude_list_response(models, before_id, after_id, limit)
    elif _get_family(api_format) == "gemini":
        return _build_gemini_list_response(models, page_size, page_token)
    else:
        return _build_openai_list_response(models)


@router.get("/v1/models/{model_id:path}", response_model=None)
async def retrieve_model(
    model_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> dict | JSONResponse:
    """
    获取单个模型详情（统一端点）

    根据请求头中的认证方式自动检测 API 格式，并返回相应格式的模型详情。
    此接口兼容 Claude、OpenAI 和 Gemini 三种 API 格式。

    **格式检测规则**
    - x-api-key + anthropic-version → Claude 格式
    - x-goog-api-key 或 ?key= → Gemini 格式
    - Authorization: Bearer → OpenAI 格式（默认）

    **路径参数**
    - model_id: 模型标识符（Gemini 格式支持 models/ 前缀，会自动移除）

    **返回字段**

    Claude 格式：
    - id: 模型标识符
    - type: "model"
    - display_name: 显示名称
    - created_at: 创建时间（ISO 8601 格式）

    OpenAI 格式：
    - id: 模型标识符
    - object: "model"
    - created: Unix 时间戳
    - owned_by: 提供商名称

    Gemini 格式：
    - name: 模型资源名称（如 models/gemini-pro）
    - baseModelId: 基础模型 ID
    - version: 版本号
    - displayName: 显示名称
    - description: 描述信息
    - inputTokenLimit: 输入 token 上限
    - outputTokenLimit: 输出 token 上限
    - supportedGenerationMethods: 支持的生成方法
    - temperature: 默认温度参数
    - maxTemperature: 最大温度参数
    - topP: Top-P 参数
    - topK: Top-K 参数

    **错误响应**
    401: API Key 无效或未提供
    404: 模型不存在或不可访问
    """
    api_format, api_key = _detect_api_format_and_key(request)

    # Gemini 格式的 name 带 "models/" 前缀，需要移除
    if _get_family(api_format) == "gemini" and model_id.startswith("models/"):
        model_id = model_id[7:]

    logger.info(f"[Models] GET /v1/models/{model_id} | format={api_format}")

    # 认证
    user, key_record = _authenticate(db, api_key)
    if not user:
        return _build_auth_error_response(api_format)

    # 构建访问限制
    restrictions = AccessRestrictions.from_api_key_and_user(key_record, user)

    # 获取可用格式（包括可转换的格式）
    global_conversion_enabled = _is_format_conversion_enabled(db)
    candidate_formats = _get_convertible_formats(api_format)
    candidate_formats, _ = _filter_formats_by_restrictions(
        candidate_formats, restrictions, api_format
    )
    provider_to_formats = get_compatible_provider_formats(
        db, api_format, candidate_formats, global_conversion_enabled
    )
    formats = _flatten_provider_formats(provider_to_formats)
    if not formats:
        return _build_404_response(model_id, api_format)

    available_provider_ids = get_available_provider_ids(db, formats, provider_to_formats)
    model_info = find_model_by_id(
        db,
        model_id,
        available_provider_ids,
        formats,
        restrictions,
        provider_to_formats=provider_to_formats,
    )

    if not model_info:
        return _build_404_response(model_id, api_format)

    if _get_family(api_format) == "claude":
        return _build_claude_model_response(model_info)
    elif _get_family(api_format) == "gemini":
        return _build_gemini_model_response(model_info)
    else:
        return _build_openai_model_response(model_info)


# Gemini 专用路径 /v1beta/models
@router.get("/v1beta/models", response_model=None)
async def list_models_gemini(
    request: Request,
    page_size: int = Query(50, alias="pageSize", ge=1, le=1000),
    page_token: str | None = Query(None, alias="pageToken"),
    db: Session = Depends(get_db),
) -> dict | JSONResponse:
    """
    列出可用模型（Gemini v1beta 专用端点）

    Gemini API 的专用模型列表端点，使用 x-goog-api-key 或 ?key= 参数进行认证。
    返回 Gemini 格式的模型列表。

    **查询参数**
    - pageSize: 每页数量，默认 50，范围 1-1000
    - pageToken: 分页 token，用于获取下一页

    **返回字段**
    - models: 模型列表，每个模型包含：
      - name: 模型资源名称（如 models/gemini-pro）
      - baseModelId: 基础模型 ID
      - version: 版本号
      - displayName: 显示名称
      - description: 描述信息
      - inputTokenLimit: 输入 token 上限
      - outputTokenLimit: 输出 token 上限
      - supportedGenerationMethods: 支持的生成方法列表
      - temperature: 默认温度参数
      - maxTemperature: 最大温度参数
      - topP: Top-P 参数
      - topK: Top-K 参数
    - nextPageToken: 下一页的 token（如果有更多结果）

    **错误响应**
    401: API Key 无效或未提供
    """
    logger.info("[Models] GET /v1beta/models | format=gemini")

    api_format, api_key = _detect_api_format_and_key(request)

    # 认证
    user, key_record = _authenticate(db, api_key)
    if not user:
        return _build_auth_error_response(api_format)

    # 构建访问限制
    restrictions = AccessRestrictions.from_api_key_and_user(key_record, user)

    # 获取可用格式（包括可转换的格式）
    global_conversion_enabled = _is_format_conversion_enabled(db)
    candidate_formats = _get_convertible_formats(api_format)
    candidate_formats, empty_response = _filter_formats_by_restrictions(
        candidate_formats, restrictions, api_format
    )
    if empty_response is not None:
        return empty_response

    provider_to_formats = get_compatible_provider_formats(
        db, api_format, candidate_formats, global_conversion_enabled
    )
    formats = _flatten_provider_formats(provider_to_formats)

    available_provider_ids = get_available_provider_ids(db, formats, provider_to_formats)
    if not available_provider_ids:
        return {"models": []}

    models = await list_available_models(
        db,
        available_provider_ids,
        formats,
        restrictions,
        provider_to_formats=provider_to_formats,
        client_format=api_format,
    )
    logger.debug(f"[Models] 返回 {len(models)} 个模型")
    response = _build_gemini_list_response(models, page_size, page_token)
    logger.debug(f"[Models] Gemini 响应: {response}")
    return response


@router.get("/v1beta/models/{model_name:path}", response_model=None)
async def get_model_gemini(
    request: Request,
    model_name: str,
    db: Session = Depends(get_db),
) -> dict | JSONResponse:
    """
    获取单个模型详情（Gemini v1beta 专用端点）

    Gemini API 的专用模型详情端点，使用 x-goog-api-key 或 ?key= 参数进行认证。
    返回 Gemini 格式的模型详情。

    **路径参数**
    - model_name: 模型名称或资源路径（支持 models/ 前缀，会自动移除）

    **返回字段**
    - name: 模型资源名称（如 models/gemini-pro）
    - baseModelId: 基础模型 ID
    - version: 版本号
    - displayName: 显示名称
    - description: 描述信息
    - inputTokenLimit: 输入 token 上限
    - outputTokenLimit: 输出 token 上限
    - supportedGenerationMethods: 支持的生成方法列表
    - temperature: 默认温度参数
    - maxTemperature: 最大温度参数
    - topP: Top-P 参数
    - topK: Top-K 参数

    **错误响应**
    401: API Key 无效或未提供
    404: 模型不存在或不可访问
    """
    # 移除 "models/" 前缀（如果有）
    model_id = model_name[7:] if model_name.startswith("models/") else model_name
    logger.info(f"[Models] GET /v1beta/models/{model_id} | format=gemini")

    api_format, api_key = _detect_api_format_and_key(request)

    # 认证
    user, key_record = _authenticate(db, api_key)
    if not user:
        return _build_auth_error_response(api_format)

    # 构建访问限制
    restrictions = AccessRestrictions.from_api_key_and_user(key_record, user)

    # 获取可用格式（包括可转换的格式）
    global_conversion_enabled = _is_format_conversion_enabled(db)
    candidate_formats = _get_convertible_formats(api_format)
    candidate_formats, _ = _filter_formats_by_restrictions(
        candidate_formats, restrictions, api_format
    )
    provider_to_formats = get_compatible_provider_formats(
        db, api_format, candidate_formats, global_conversion_enabled
    )
    formats = _flatten_provider_formats(provider_to_formats)
    if not formats:
        return _build_404_response(model_id, api_format)

    available_provider_ids = get_available_provider_ids(db, formats, provider_to_formats)
    model_info = find_model_by_id(
        db,
        model_id,
        available_provider_ids,
        formats,
        restrictions,
        provider_to_formats=provider_to_formats,
    )

    if not model_info:
        return _build_404_response(model_id, api_format)

    return _build_gemini_model_response(model_info)
