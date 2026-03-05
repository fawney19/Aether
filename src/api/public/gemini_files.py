"""
Gemini Files API 代理端点

代理 Google Gemini Files API，支持文件的上传、查询、删除等操作。

端点列表：
- POST /upload/v1beta/files - 上传文件（可恢复上传）
- GET /v1beta/files - 列出文件
- GET /v1beta/files/{name} - 获取文件元数据
- DELETE /v1beta/files/{name} - 删除文件

认证方式：
- x-goog-api-key 请求头
- ?key= URL 参数

参考文档：
https://ai.google.dev/api/files

优化：HTTP 代理请求期间不持有数据库连接，避免阻塞其他请求。
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Dict, Optional, Tuple
from urllib.parse import urlencode

from fastapi import APIRouter, HTTPException, Request, Response
from fastapi.responses import JSONResponse
from sqlalchemy.orm import Session

from src.clients.http_client import HTTPClientPool
from src.core.api_format import get_auth_handler, get_default_auth_method_for_endpoint
from src.core.crypto import crypto_service
from src.core.logger import logger
from src.database import create_session
from src.models.database import ApiKey, GlobalModel, Model, Provider, ProviderEndpoint, User
from src.services.auth.service import AuthService
from src.services.gemini_files_mapping import delete_file_key_mapping, store_file_key_mapping
from src.services.provider.transport import redact_url_for_log
from src.services.scheduling.aware_scheduler import CacheAwareScheduler, ProviderCandidate
from src.services.usage.service import UsageService


@dataclass
class UpstreamContext:
    """上游请求上下文（不依赖数据库会话）"""

    upstream_key: str
    base_url: str
    file_key_id: str
    user_id: str


router = APIRouter(tags=["Gemini Files API"])

# Gemini Files API 基础 URL
GEMINI_FILES_BASE_URL = "https://generativelanguage.googleapis.com"

# Gemini Files API 无能力限制（任何 Gemini key 都可用）

# 需要从客户端请求中移除的头部（这些会由代理重新设置或不应转发）
HEADERS_TO_REMOVE = frozenset(
    {
        "host",
        "content-length",
        "transfer-encoding",
        "connection",
        "x-goog-api-key",
        "authorization",
    }
)


def _extract_gemini_api_key(request: Request) -> str | None:
    """
    从请求中提取 Gemini API Key

    优先级（与 Google SDK 行为一致）：
    1. URL 参数 ?key=
    2. x-goog-api-key 请求头
    """
    auth_method = get_default_auth_method_for_endpoint("gemini:chat")
    handler = get_auth_handler(auth_method)
    return handler.extract_credentials(request)


def _build_upstream_headers(
    original_headers: dict[str, str],
    upstream_api_key: str,
) -> dict[str, str]:
    """
    构建上游请求头

    Args:
        original_headers: 原始请求头
        upstream_api_key: 上游 API Key

    Returns:
        处理后的请求头字典
    """
    headers = {}

    # 透传非敏感头部
    for name, value in original_headers.items():
        if name.lower() not in HEADERS_TO_REMOVE:
            headers[name] = value

    # 设置认证头
    headers["x-goog-api-key"] = upstream_api_key

    return headers


def _ensure_balance_access(db: Session, user: User, api_key: ApiKey) -> None:
    access_ok, message = UsageService.check_request_balance(db, user, api_key=api_key)
    if access_ok:
        return
    raise HTTPException(
        status_code=429,
        detail={
            "error": {
                "code": 429,
                "message": message or "Insufficient balance",
                "status": "RESOURCE_EXHAUSTED",
            }
        },
    )


def _build_upstream_url(
    base_url: str,
    path: str,
    query_params: dict[str, Any] | None = None,
    is_upload: bool = False,
) -> str:
    """
    构建上游 URL

    Args:
        base_url: 上游基础 URL
        path: API 路径
        query_params: 查询参数
        is_upload: 是否为上传端点

    Returns:
        完整的上游 URL
    """
    # 移除 key 参数（认证通过 header）
    effective_params = dict(query_params) if query_params else {}
    effective_params.pop("key", None)

    # 处理 base_url 可能包含 /v1beta 的情况，避免重复
    normalized_base_url = base_url.rstrip("/")
    if normalized_base_url.endswith("/v1beta"):
        normalized_base_url = normalized_base_url[: -len("/v1beta")]

    # 上传端点使用不同的路径前缀
    if is_upload:
        url = f"{normalized_base_url}/upload{path}"
    else:
        url = f"{normalized_base_url}{path}"

    if effective_params:
        query_string = urlencode(effective_params, doseq=True)
        url = f"{url}?{query_string}"

    return url


def _resolve_files_model_name(
    db: Session,
    user_api_key: ApiKey,
    user: User | None,
) -> str | None:
    """
    为 Files API 选择一个可用的模型名（用于 Key 选择与权限过滤）

    选择顺序:
    1. 用户/Key 的 allowed_models（取交集后选第一个）
    2. 任意支持 Gemini 格式的 GlobalModel
    """
    from src.core.model_permissions import merge_allowed_models

    allowed_models = merge_allowed_models(
        user_api_key.allowed_models,
        user.allowed_models if user else None,
    )
    if allowed_models is not None:
        if not allowed_models:
            return None
        return sorted(allowed_models)[0]

    row = (
        db.query(GlobalModel.name)
        .join(Model, Model.global_model_id == GlobalModel.id)
        .join(Provider, Provider.id == Model.provider_id)
        .join(ProviderEndpoint, ProviderEndpoint.provider_id == Provider.id)
        .filter(
            GlobalModel.is_active == True,
            Model.is_active == True,
            Provider.is_active == True,
            ProviderEndpoint.is_active == True,
            ProviderEndpoint.api_family == "gemini",
        )
        .distinct()
        .order_by(GlobalModel.name.asc())
        .first()
    )
    return row[0] if row else None


async def _select_provider_candidate(
    db: Session,
    user_api_key: ApiKey,
    model_name: str,
    require_files_capability: bool = True,
) -> ProviderCandidate | None:
    """
    选择可用的 Provider/Endpoint/Key 组合

    Args:
        db: 数据库会话
        user_api_key: 用户 API Key
        model_name: 模型名称
        require_files_capability: 是否要求 gemini_files 能力（默认 True）

    Returns:
        匹配的候选，如果没有则返回 None
    """
    scheduler = CacheAwareScheduler()

    # 要求 gemini_files 能力：只有 Google 官方 API 才支持 Files API
    capability_requirements = {"gemini_files": True} if require_files_capability else None

    candidates, _global_model_id, _provider_count = await scheduler.list_all_candidates(
        db=db,
        api_format="gemini:chat",
        model_name=model_name,
        affinity_key=str(user_api_key.id),
        user_api_key=user_api_key,
        max_candidates=10,
        capability_requirements=capability_requirements,
    )
    for candidate in candidates:
        auth_type = getattr(candidate.key, "auth_type", "api_key") or "api_key"
        if auth_type == "api_key":
            return candidate
    return None


async def _resolve_upstream_context(
    request: Request,
    db: Session,
) -> tuple[str, str, str, str]:
    """
    解析上游 Key 与 Base URL（需要外部提供 db session）

    仅允许系统 API Key，选择可用的 Gemini Provider Key（无能力限制）。

    Args:
        request: HTTP 请求
        db: 数据库会话

    Returns:
        (upstream_key, base_url, key_id, user_id)
    """

    client_key = _extract_gemini_api_key(request)
    if not client_key:
        raise HTTPException(
            status_code=401,
            detail={
                "error": {
                    "code": 401,
                    "message": "API key required. Provide via x-goog-api-key header or ?key= parameter.",
                    "status": "UNAUTHENTICATED",
                }
            },
        )

    auth_result = AuthService.authenticate_api_key(db, client_key)
    if not auth_result:
        raise HTTPException(
            status_code=401,
            detail={
                "error": {
                    "code": 401,
                    "message": "API key not valid. Please pass a valid API key.",
                    "status": "UNAUTHENTICATED",
                }
            },
        )

    user, user_api_key = auth_result
    _ensure_balance_access(db, user, user_api_key)
    model_name = _resolve_files_model_name(db, user_api_key, user)
    if not model_name:
        raise HTTPException(
            status_code=503,
            detail={
                "error": {
                    "code": 503,
                    "message": "No available model for Gemini Files API routing",
                    "status": "UNAVAILABLE",
                }
            },
        )

    # 选择可用的 provider candidate（要求 gemini_files 能力）
    candidate = await _select_provider_candidate(
        db, user_api_key, model_name, require_files_capability=True
    )

    if not candidate:
        raise HTTPException(
            status_code=503,
            detail={
                "error": {
                    "code": 503,
                    "message": "No available Gemini key with 'gemini_files' capability. "
                    "Please ensure at least one Provider Key has the 'gemini_files' capability enabled.",
                    "status": "UNAVAILABLE",
                }
            },
        )

    try:
        upstream_key = crypto_service.decrypt(candidate.key.api_key)
    except Exception as exc:
        logger.error("Failed to decrypt provider key for Gemini Files API: {}", exc)
        raise HTTPException(
            status_code=500,
            detail={
                "error": {
                    "code": 500,
                    "message": "Failed to decrypt provider key",
                    "status": "INTERNAL",
                }
            },
        )

    base_url = candidate.endpoint.base_url or GEMINI_FILES_BASE_URL
    return upstream_key, base_url, str(candidate.key.id), str(user.id)


async def _resolve_upstream_context_standalone(request: Request) -> UpstreamContext:
    """
    解析上游上下文（自管理数据库连接，适用于 HTTP 代理场景）

    优化：在返回上下文后立即释放数据库连接，HTTP 请求期间不持有连接。

    Args:
        request: HTTP 请求

    Returns:
        UpstreamContext: 包含所有必要信息的上下文对象
    """
    with create_session() as db:
        upstream_key, base_url, file_key_id, user_id = await _resolve_upstream_context(request, db)
        return UpstreamContext(
            upstream_key=upstream_key,
            base_url=base_url,
            file_key_id=file_key_id,
            user_id=user_id,
        )


async def _proxy_request(
    method: str,
    upstream_url: str,
    headers: dict[str, str],
    content: bytes | None = None,
    json_body: dict[str, Any] | None = None,
    file_key_id: str | None = None,
    user_id: str | None = None,
) -> Response:
    """
    代理请求到上游 Gemini API

    Args:
        method: HTTP 方法
        upstream_url: 上游 URL
        headers: 请求头
        content: 原始请求体（二进制）
        json_body: JSON 请求体
        file_key_id: 上游 Provider Key ID，用于成功响应时存储 file→key 映射
        user_id: 用户 ID，用于文件映射的权限验证

    Returns:
        FastAPI Response 对象
    """
    client = await HTTPClientPool.get_default_client_async()

    try:
        if method.upper() == "GET":
            response = await client.get(upstream_url, headers=headers)
        elif method.upper() == "DELETE":
            response = await client.delete(upstream_url, headers=headers)
        elif method.upper() == "POST":
            if content is not None:
                response = await client.post(upstream_url, headers=headers, content=content)
            elif json_body is not None:
                response = await client.post(upstream_url, headers=headers, json=json_body)
            else:
                response = await client.post(upstream_url, headers=headers)
        else:
            raise HTTPException(status_code=405, detail="Method not allowed")

        # 构建响应头（排除 hop-by-hop 头部）
        response_headers = {}
        hop_by_hop = {"connection", "keep-alive", "transfer-encoding", "upgrade"}
        for name, value in response.headers.items():
            if name.lower() not in hop_by_hop:
                response_headers[name] = value

        if (
            file_key_id
            and response.status_code < 300
            and response.headers.get("content-type", "").startswith("application/json")
        ):
            try:
                payload = response.json()
                file_name = None
                file_obj = None

                if isinstance(payload, dict):
                    # 单文件上传响应
                    file_name = payload.get("name")
                    file_obj = payload

                    # 嵌套格式：{"file": {...}}
                    if not file_name and isinstance(payload.get("file"), dict):
                        file_name = payload["file"].get("name")
                        file_obj = payload["file"]

                    if file_name and file_obj:
                        display_name = file_obj.get("displayName") or file_obj.get("display_name")
                        mime_type = file_obj.get("mimeType") or file_obj.get("mime_type")
                        await store_file_key_mapping(
                            file_name,
                            file_key_id,
                            user_id=user_id,
                            display_name=display_name,
                            mime_type=mime_type,
                        )
                        logger.debug(
                            f"Gemini file→key 映射已存储: {file_name} → key_id={file_key_id}"
                        )

                    # 为 list_files 响应中的所有文件建立映射
                    # 这是正确的：Gemini API 按 Key 隔离文件，返回的文件必然属于当前 Key
                    files_list = payload.get("files")
                    if isinstance(files_list, list):
                        mapped_count = 0
                        for item in files_list:
                            if isinstance(item, dict) and item.get("name"):
                                item_display_name = item.get("displayName") or item.get(
                                    "display_name"
                                )
                                item_mime_type = item.get("mimeType") or item.get("mime_type")
                                await store_file_key_mapping(
                                    item["name"],
                                    file_key_id,
                                    user_id=user_id,
                                    display_name=item_display_name,
                                    mime_type=item_mime_type,
                                )
                                mapped_count += 1
                        if mapped_count > 0:
                            logger.debug(
                                "Gemini list_files 批量映射已存储: {} 个文件 → key_id={}",
                                mapped_count,
                                file_key_id,
                            )
            except (ValueError, KeyError) as e:
                logger.debug("Failed to store Gemini file mapping: {}", e)

        return Response(
            content=response.content,
            status_code=response.status_code,
            headers=response_headers,
            media_type=response.headers.get("content-type", "application/json"),
        )

    except Exception as e:
        sanitized_error = redact_url_for_log(str(e))
        logger.error("Gemini Files API proxy error: {}", sanitized_error)
        return JSONResponse(
            status_code=502,
            content={
                "error": {
                    "code": 502,
                    "message": "Upstream request failed",
                    "status": "BAD_GATEWAY",
                }
            },
        )


# ==============================================================================
# 文件上传端点
# ==============================================================================


@router.post("/upload/v1beta/files")
async def upload_file(
    request: Request,
) -> Any:
    """
    上传文件到 Gemini Files API

    支持可恢复上传协议（Resumable Upload Protocol）：
    1. 初始请求：设置元数据，获取上传 URL
    2. 上传请求：上传实际文件内容

    **认证方式**:
    - `x-goog-api-key` 请求头，或
    - `?key=` URL 参数

    **请求头（可恢复上传）**:
    - `X-Goog-Upload-Protocol: resumable`
    - `X-Goog-Upload-Command: start` | `upload, finalize`
    - `X-Goog-Upload-Header-Content-Length`: 文件大小
    - `X-Goog-Upload-Header-Content-Type`: 文件 MIME 类型

    **请求体（初始请求）**:
    ```json
    {
        "file": {
            "display_name": "文件名"
        }
    }
    ```

    优化：HTTP 代理期间不持有数据库连接
    """
    # 阶段 1：解析上下文（短暂持有数据库连接）
    ctx = await _resolve_upstream_context_standalone(request)

    # 阶段 2：读取请求体
    body = await request.body()

    # 阶段 3：代理请求（不持有数据库连接）
    upstream_url = _build_upstream_url(
        ctx.base_url,
        "/v1beta/files",
        dict(request.query_params),
        is_upload=True,
    )

    headers = _build_upstream_headers(dict(request.headers), ctx.upstream_key)

    logger.debug("Gemini Files upload proxy: POST {}", redact_url_for_log(upstream_url))

    return await _proxy_request(
        "POST",
        upstream_url,
        headers,
        content=body,
        file_key_id=ctx.file_key_id,
        user_id=ctx.user_id,
    )


# ==============================================================================
# 文件列表端点
# ==============================================================================


@router.get("/v1beta/files")
async def list_files(
    request: Request,
    pageSize: int | None = None,
    pageToken: str | None = None,
) -> Any:
    """
    列出已上传的文件

    优化：HTTP 代理期间不持有数据库连接

    **认证方式**:
    - `x-goog-api-key` 请求头，或
    - `?key=` URL 参数

    **查询参数**:
    - `pageSize`: 每页返回的文件数量（默认 10，最大 100）
    - `pageToken`: 分页令牌

    **响应格式**:
    ```json
    {
        "files": [
            {
                "name": "files/abc-123",
                "displayName": "文件名",
                "mimeType": "image/jpeg",
                "sizeBytes": "12345",
                "createTime": "2024-01-01T00:00:00Z",
                "updateTime": "2024-01-01T00:00:00Z",
                "expirationTime": "2024-01-03T00:00:00Z",
                "sha256Hash": "...",
                "uri": "https://...",
                "state": "ACTIVE"
            }
        ],
        "nextPageToken": "..."
    }
    ```
    """
    # 阶段 1：解析上下文（短暂持有数据库连接）
    ctx = await _resolve_upstream_context_standalone(request)

    # 阶段 2：代理请求（不持有数据库连接）
    query_params = dict(request.query_params)
    if pageSize is not None:
        query_params["pageSize"] = pageSize
    if pageToken is not None:
        query_params["pageToken"] = pageToken

    upstream_url = _build_upstream_url(ctx.base_url, "/v1beta/files", query_params)
    headers = _build_upstream_headers(dict(request.headers), ctx.upstream_key)

    logger.debug("Gemini Files list proxy: GET {}", redact_url_for_log(upstream_url))

    return await _proxy_request(
        "GET", upstream_url, headers, file_key_id=ctx.file_key_id, user_id=ctx.user_id
    )


# ==============================================================================
# 下载文件内容端点（用于视频等媒体文件）
# 注意：必须在 /v1beta/files/{file_name:path} 之前注册，否则会被通配符路由捕获
# ==============================================================================


async def _find_video_task_by_id(
    db: Session, short_id: str, user_id: str
) -> tuple[str | None, str | None]:
    """
    通过短 ID 查找视频任务，返回其 provider key 和 video_url

    Args:
        db: 数据库会话
        short_id: 视频任务的短 ID（VideoTask.short_id，Gemini 风格）
        user_id: 用户 ID（用于权限验证）

    Returns:
        (upstream_key, video_url) - 如果找到任务返回 key 和 url，否则返回 (None, None)
    """
    from src.models.database import ProviderAPIKey, VideoTask

    logger.debug(
        "[Files Download] Searching video task: short_id={}, user_id={}", short_id, user_id
    )

    # 通过 short_id 查找，同时验证用户权限
    task = (
        db.query(VideoTask)
        .filter(VideoTask.short_id == short_id, VideoTask.user_id == user_id)
        .first()
    )

    if not task:
        logger.debug("[Files Download] No video task found: short_id={}", short_id)
        return None, None

    if not task.video_url:
        logger.debug("[Files Download] Task found but no video_url: short_id={}", short_id)
        return None, None

    if not task.key_id:
        logger.debug("[Files Download] Task found but no key_id: short_id={}", short_id)
        return None, task.video_url

    # 获取 provider key
    provider_key = db.query(ProviderAPIKey).filter(ProviderAPIKey.id == task.key_id).first()
    if not provider_key or not provider_key.api_key:
        logger.debug("[Files Download] Provider key not found: key_id={}", task.key_id)
        return None, task.video_url

    try:
        upstream_key = crypto_service.decrypt(provider_key.api_key)
        logger.debug("[Files Download] Found key for task: short_id={}", short_id)
        return upstream_key, task.video_url
    except Exception as e:
        logger.error("[Files Download] Failed to decrypt key: {}", e)
        return None, task.video_url


@router.get("/v1beta/files/{file_id}:download")
async def download_file(
    file_id: str,
    request: Request,
) -> Any:
    """
    下载文件（官方 Gemini API 格式）

    **认证方式**:
    - `x-goog-api-key` 请求头，或
    - `?key=` URL 参数

    **路径参数**:
    - `file_id`: 文件 ID
      - 以 `aev_` 开头：视频任务下载（如 `aev_sknuzqlo8sds`，Gemini 风格短 ID）
      - 其他：普通 Gemini 文件下载（透传到上游）

    **查询参数**:
    - `alt=media`: 可选，保持与官方 API 兼容

    **示例**:
    ```
    GET /v1beta/files/aev_{short_id}:download?alt=media  # 视频任务
    GET /v1beta/files/{gemini_file_id}:download?alt=media  # 普通文件
    ```

    优化：HTTP 下载期间不持有数据库连接
    """
    import httpx
    from fastapi import HTTPException
    from fastapi.responses import JSONResponse, Response

    # ========== 阶段 1：数据库操作（短暂持有连接）==========
    client_key = _extract_gemini_api_key(request)
    if not client_key:
        raise HTTPException(
            status_code=401,
            detail={
                "error": {"code": 401, "message": "API key required", "status": "UNAUTHENTICATED"}
            },
        )

    # 在数据库会话内完成所有查询
    with create_session() as db:
        auth_result = AuthService.authenticate_api_key(db, client_key)
        if not auth_result:
            raise HTTPException(
                status_code=401,
                detail={
                    "error": {
                        "code": 401,
                        "message": "API key not valid",
                        "status": "UNAUTHENTICATED",
                    }
                },
            )

        user, _user_api_key = auth_result
        _ensure_balance_access(db, user, _user_api_key)

        # 根据前缀判断处理方式
        if file_id.startswith("aev_"):
            # 视频任务下载：使用短 ID 查找
            short_id = file_id[4:]  # 去掉 "aev_" 前缀
            logger.debug("[Files Download] Video task: short_id={}, user_id={}", short_id, user.id)
            upstream_key, video_url = await _find_video_task_by_id(db, short_id, user.id)
            if not upstream_key or not video_url:
                raise HTTPException(
                    status_code=404,
                    detail={
                        "error": {
                            "code": 404,
                            "message": f"Video not found or not ready: {file_id}",
                            "status": "NOT_FOUND",
                        }
                    },
                )
            upstream_url = video_url
        else:
            # 普通文件下载：透传到 Gemini
            try:
                upstream_key, base_url, _file_key_id, _user_id = await _resolve_upstream_context(
                    request, db
                )
            except HTTPException:
                raise HTTPException(
                    status_code=404,
                    detail={
                        "error": {
                            "code": 404,
                            "message": f"File not found: {file_id}",
                            "status": "NOT_FOUND",
                        }
                    },
                )
            file_name = f"files/{file_id}" if not file_id.startswith("files/") else file_id
            upstream_url = _build_upstream_url(
                base_url,
                f"/v1beta/{file_name}:download",
                dict(request.query_params),
            )

    # ========== 阶段 2：HTTP 下载（不持有数据库连接）==========
    headers = _build_upstream_headers(dict(request.headers), upstream_key)

    logger.debug("Gemini Files download proxy: GET {}", redact_url_for_log(upstream_url))

    # 使用 follow_redirects=True 跟随重定向（Gemini 文件下载会重定向）
    try:
        from src.services.proxy_node.resolver import build_proxy_client_kwargs

        async with httpx.AsyncClient(
            **build_proxy_client_kwargs(timeout=httpx.Timeout(300.0), follow_redirects=True)
        ) as client:
            response = await client.get(upstream_url, headers=headers)
    except Exception as exc:
        logger.error("Gemini Files download failed: {}", exc)
        raise HTTPException(status_code=502, detail="Failed to download file")

    if response.status_code >= 400:
        content: dict[str, Any]
        if response.headers.get("content-type", "").startswith("application/json"):
            try:
                content = response.json()
            except Exception:
                content = {"error": response.text}
        else:
            content = {"error": response.text}
        return JSONResponse(content=content, status_code=response.status_code)

    # 返回文件内容
    return Response(
        content=response.content,
        status_code=response.status_code,
        headers={
            k: v
            for k, v in response.headers.items()
            if k.lower() not in {"transfer-encoding", "connection", "keep-alive"}
        },
        media_type=response.headers.get("content-type", "application/octet-stream"),
    )


# ==============================================================================
# 获取文件元数据端点
# ==============================================================================


@router.get("/v1beta/files/{file_name:path}")
async def get_file(
    file_name: str,
    request: Request,
) -> Any:
    """
    获取指定文件的元数据

    **认证方式**:
    - `x-goog-api-key` 请求头，或
    - `?key=` URL 参数

    **路径参数**:
    - `file_name`: 文件名（格式：files/xxx 或 xxx）

    **响应格式**:
    ```json
    {
        "name": "files/abc-123",
        "displayName": "文件名",
        "mimeType": "image/jpeg",
        "sizeBytes": "12345",
        "createTime": "2024-01-01T00:00:00Z",
        "updateTime": "2024-01-01T00:00:00Z",
        "expirationTime": "2024-01-03T00:00:00Z",
        "sha256Hash": "...",
        "uri": "https://...",
        "state": "ACTIVE"
    }
    ```

    优化：HTTP 代理期间不持有数据库连接
    """
    # 阶段 1：解析上下文（短暂持有数据库连接）
    ctx = await _resolve_upstream_context_standalone(request)

    # 阶段 2：代理请求（不持有数据库连接）
    # 规范化文件名（确保以 files/ 开头）
    if not file_name.startswith("files/"):
        file_name = f"files/{file_name}"

    upstream_url = _build_upstream_url(
        ctx.base_url,
        f"/v1beta/{file_name}",
        dict(request.query_params),
    )
    headers = _build_upstream_headers(dict(request.headers), ctx.upstream_key)

    logger.debug("Gemini Files get proxy: GET {}", redact_url_for_log(upstream_url))

    return await _proxy_request(
        "GET", upstream_url, headers, file_key_id=ctx.file_key_id, user_id=ctx.user_id
    )


# ==============================================================================
# 删除文件端点
# ==============================================================================


@router.delete("/v1beta/files/{file_name:path}")
async def delete_file(
    file_name: str,
    request: Request,
) -> Any:
    """
    删除指定文件

    **认证方式**:
    - `x-goog-api-key` 请求头，或
    - `?key=` URL 参数

    **路径参数**:
    - `file_name`: 文件名（格式：files/xxx 或 xxx）

    **响应格式**:
    成功时返回空 JSON 对象：`{}`

    优化：HTTP 代理期间不持有数据库连接
    """
    # 阶段 1：解析上下文（短暂持有数据库连接）
    ctx = await _resolve_upstream_context_standalone(request)

    # 阶段 2：代理请求（不持有数据库连接）
    # 规范化文件名（确保以 files/ 开头）
    if not file_name.startswith("files/"):
        file_name = f"files/{file_name}"

    upstream_url = _build_upstream_url(
        ctx.base_url,
        f"/v1beta/{file_name}",
        dict(request.query_params),
    )
    headers = _build_upstream_headers(dict(request.headers), ctx.upstream_key)

    logger.debug("Gemini Files delete proxy: DELETE {}", redact_url_for_log(upstream_url))

    response = await _proxy_request("DELETE", upstream_url, headers)
    if response.status_code < 300:
        await delete_file_key_mapping(file_name)
    else:
        logger.debug(
            "Gemini Files delete failed, skip mapping cleanup: status={}", response.status_code
        )
    return response
