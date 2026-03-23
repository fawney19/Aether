"""视频任务管理 API 路由。

管理员可以查看所有视频任务，用户只能查看自己的任务。
"""

from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime, timezone
from typing import Any, AsyncIterator

from fastapi import APIRouter, Depends, HTTPException, Query, Request
from fastapi.responses import StreamingResponse
from sqlalchemy import func
from sqlalchemy.orm import Session

from src.api.base.authenticated_adapter import AuthenticatedApiAdapter
from src.api.base.context import ApiRequestContext
from src.api.base.pipeline import get_pipeline
from src.clients.http_client import HTTPClientPool
from src.config.constants import CacheTTL
from src.core.crypto import crypto_service
from src.core.enums import UserRole
from src.core.logger import logger
from src.database import get_db
from src.models.database import Provider, ProviderAPIKey, ProviderEndpoint, User, VideoTask
from src.utils.cache_decorator import cache_result

router = APIRouter(prefix="/api/admin/video-tasks", tags=["Admin - Video Tasks"])
pipeline = get_pipeline()


@router.get("")
async def list_video_tasks(
    request: Request,
    status: str | None = Query(None, description="Filter by status"),
    user_id: str | None = Query(None, description="Filter by user ID (admin only)"),
    model: str | None = Query(None, description="Filter by model"),
    page: int = Query(1, ge=1, description="Page number"),
    page_size: int = Query(20, ge=1, le=100, description="Items per page"),
    db: Session = Depends(get_db),
) -> Any:
    """
    获取视频任务列表

    管理员可以查看所有用户的任务，普通用户只能查看自己的任务。

    **查询参数**:
    - `status`: 按状态筛选（pending/submitted/processing/completed/failed/cancelled）
    - `user_id`: 按用户 ID 筛选（仅管理员）
    - `model`: 按模型筛选
    - `page`: 页码，默认 1
    - `page_size`: 每页数量，默认 20，最大 100

    **返回字段**:
    - `items`: 任务列表
    - `total`: 总数
    - `page`: 当前页码
    - `page_size`: 每页数量
    - `pages`: 总页数
    """
    adapter = VideoTaskListAdapter(
        status=status,
        user_id=user_id,
        model=model,
        page=page,
        page_size=page_size,
    )
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/stats")
async def get_video_task_stats(
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    获取视频任务统计

    **返回字段**:
    - `total`: 总任务数
    - `by_status`: 按状态分组的数量
    - `by_model`: 按模型分组的数量（前 10）
    - `today_count`: 今日任务数
    """
    adapter = VideoTaskStatsAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/{task_id}")
async def get_video_task_detail(
    task_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    获取视频任务详情

    **路径参数**:
    - `task_id`: 任务 ID

    **返回字段**:
    - 任务的完整信息，包括请求体、响应、状态等
    """
    adapter = VideoTaskDetailAdapter(task_id=task_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/{task_id}/cancel")
async def cancel_video_task(
    task_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    取消视频任务

    **路径参数**:
    - `task_id`: 任务 ID

    **返回**:
    - 更新后的任务信息
    """
    adapter = VideoTaskCancelAdapter(task_id=task_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/{task_id}/video")
async def proxy_video_stream(
    task_id: str,
    request: Request,
    token: str | None = Query(None, description="JWT access token"),
    db: Session = Depends(get_db),
) -> StreamingResponse:
    """
    代理视频流（用于需要认证的视频链接）

    **路径参数**:
    - `task_id`: 任务 ID

    **查询参数**:
    - `token`: JWT access token（用于 video 标签请求）

    **返回**:
    - 视频流
    """
    from src.utils.auth_utils import authenticate_user_from_bearer_token

    # 尝试从多个来源获取 token：query param > cookie > header
    auth_token = token
    if not auth_token:
        auth_token = request.cookies.get("access_token")
    if not auth_token:
        auth_header = request.headers.get("Authorization")
        if auth_header and auth_header.startswith("Bearer "):
            auth_token = auth_header[7:]

    if not auth_token:
        raise HTTPException(status_code=401, detail="Unauthorized")

    try:
        user = await authenticate_user_from_bearer_token(auth_token, db, request)
    except HTTPException:
        raise
    except Exception:
        raise HTTPException(status_code=401, detail="Unauthorized")

    # 查询任务
    query = db.query(VideoTask).filter(VideoTask.id == task_id)
    if user.role != UserRole.ADMIN:
        query = query.filter(VideoTask.user_id == user.id)

    task = query.first()
    if not task:
        raise HTTPException(status_code=404, detail="Video task not found")

    if not task.video_url:
        raise HTTPException(status_code=404, detail="Video not available")

    # 检查是否需要代理（Google API 链接需要认证）
    video_url = task.video_url
    needs_proxy = "generativelanguage.googleapis.com" in video_url

    if not needs_proxy:
        # 不需要代理，重定向到原始 URL
        from fastapi.responses import RedirectResponse

        return RedirectResponse(url=video_url)

    # 需要代理：获取 provider key 进行认证
    if not task.key_id:
        raise HTTPException(status_code=500, detail="Missing provider key")

    key = db.query(ProviderAPIKey).filter(ProviderAPIKey.id == task.key_id).first()
    if not key or not key.api_key:
        raise HTTPException(status_code=500, detail="Provider key not found")

    try:
        api_key = crypto_service.decrypt(key.api_key)
    except Exception:
        raise HTTPException(status_code=500, detail="Failed to decrypt provider key")

    # 构建认证头
    headers = {"x-goog-api-key": api_key}

    async def stream_video() -> AsyncIterator[bytes]:
        """流式下载并返回视频"""
        try:
            client = await HTTPClientPool.get_default_client_async()
            async with client.stream("GET", video_url, headers=headers) as response:
                if response.status_code >= 400:
                    logger.warning(
                        "Video proxy failed: task={} status={}", task_id, response.status_code
                    )
                    return
                async for chunk in response.aiter_bytes(chunk_size=65536):
                    yield chunk
        except Exception as e:
            logger.exception("Video proxy error: task={} error={}", task_id, str(e))

    return StreamingResponse(
        stream_video(),
        media_type="video/mp4",
        headers={
            "Content-Disposition": f'inline; filename="video_{task_id}.mp4"',
            "Cache-Control": "private, max-age=3600",
        },
    )


# ==================== Adapters ====================


@dataclass
class VideoTaskListAdapter(AuthenticatedApiAdapter):
    """视频任务列表适配器"""

    status: str | None
    user_id: str | None
    model: str | None
    page: int
    page_size: int

    @cache_result(
        key_prefix="admin:video_tasks:list",
        ttl=min(5, CacheTTL.ADMIN_USAGE_RECORDS),
        user_specific=True,
        vary_by=["status", "user_id", "model", "page", "page_size"],
    )
    async def handle(self, context: ApiRequestContext) -> Any:
        db = context.db
        user = context.user
        is_admin = user.role == UserRole.ADMIN

        query = db.query(VideoTask)

        # 权限过滤：普通用户只能看自己的任务
        if not is_admin:
            query = query.filter(VideoTask.user_id == user.id)
        elif self.user_id:
            # 管理员可以按用户筛选
            query = query.filter(VideoTask.user_id == self.user_id)

        # 状态筛选
        if self.status:
            query = query.filter(VideoTask.status == self.status)

        # 模型筛选
        if self.model:
            escaped = self.model.replace("%", "\\%").replace("_", "\\_")
            query = query.filter(VideoTask.model.ilike(f"%{escaped}%"))

        # 统计总数（避免 Query.count() 生成大子查询）
        total = int(query.with_entities(func.count(VideoTask.id)).scalar() or 0)

        # 分页
        offset = (self.page - 1) * self.page_size
        tasks = (
            query.order_by(VideoTask.created_at.desc()).offset(offset).limit(self.page_size).all()
        )

        # 获取用户信息映射
        user_ids = list(set(t.user_id for t in tasks if t.user_id))
        users_map = {}
        if user_ids:
            users = db.query(User).filter(User.id.in_(user_ids)).all()
            users_map = {u.id: u.username for u in users}

        # 获取 Provider 信息映射
        provider_ids = list(set(t.provider_id for t in tasks if t.provider_id))
        providers_map = {}
        if provider_ids:
            providers = db.query(Provider).filter(Provider.id.in_(provider_ids)).all()
            providers_map = {p.id: p.name for p in providers}

        items = []
        for task in tasks:
            items.append(
                {
                    "id": task.id,
                    "external_task_id": task.external_task_id,
                    "user_id": task.user_id,
                    "username": users_map.get(task.user_id, "Unknown"),
                    "model": task.model,
                    "prompt": (
                        task.prompt[:100] + "..."
                        if task.prompt and len(task.prompt) > 100
                        else task.prompt
                    ),
                    "status": task.status,
                    "progress_percent": task.progress_percent,
                    "progress_message": task.progress_message,
                    "provider_id": task.provider_id,
                    "provider_name": providers_map.get(task.provider_id, "Unknown"),
                    "duration_seconds": task.duration_seconds,
                    "resolution": task.resolution,
                    "aspect_ratio": task.aspect_ratio,
                    "video_url": task.video_url,
                    "error_code": task.error_code,
                    "error_message": task.error_message,
                    "poll_count": task.poll_count,
                    "max_poll_count": task.max_poll_count,
                    "created_at": task.created_at.isoformat() if task.created_at else None,
                    "completed_at": task.completed_at.isoformat() if task.completed_at else None,
                    "submitted_at": task.submitted_at.isoformat() if task.submitted_at else None,
                }
            )

        pages = (total + self.page_size - 1) // self.page_size

        return {
            "items": items,
            "total": total,
            "page": self.page,
            "page_size": self.page_size,
            "pages": pages,
        }


@dataclass
class VideoTaskStatsAdapter(AuthenticatedApiAdapter):
    """视频任务统计适配器"""

    @cache_result(
        key_prefix="admin:video_tasks:stats",
        ttl=min(5, CacheTTL.ADMIN_USAGE_RECORDS),
        user_specific=True,
    )
    async def handle(self, context: ApiRequestContext) -> Any:
        db = context.db
        user = context.user
        is_admin = user.role == UserRole.ADMIN

        base_query = db.query(VideoTask)
        if not is_admin:
            base_query = base_query.filter(VideoTask.user_id == user.id)

        # 总数（避免 Query.count() 生成大子查询）
        total = int(base_query.with_entities(func.count(VideoTask.id)).scalar() or 0)

        # 按状态分组
        status_stats = (
            base_query.with_entities(
                VideoTask.status,
                func.count(VideoTask.id).label("count"),
            )
            .group_by(VideoTask.status)
            .all()
        )
        by_status = {stat.status: stat.count for stat in status_stats}

        # 按模型分组（前 10）
        model_stats = (
            base_query.with_entities(
                VideoTask.model,
                func.count(VideoTask.id).label("count"),
            )
            .group_by(VideoTask.model)
            .order_by(func.count(VideoTask.id).desc())
            .limit(10)
            .all()
        )
        by_model = {stat.model: stat.count for stat in model_stats}

        # 今日任务数
        today = datetime.now(timezone.utc).replace(hour=0, minute=0, second=0, microsecond=0)
        today_count = int(
            base_query.filter(VideoTask.created_at >= today)
            .with_entities(func.count(VideoTask.id))
            .scalar()
            or 0
        )

        # 管理员额外统计
        result = {
            "total": total,
            "by_status": by_status,
            "by_model": by_model,
            "today_count": today_count,
        }

        if is_admin:
            # 活跃用户数（有视频任务的用户）
            active_users = db.query(func.count(func.distinct(VideoTask.user_id))).scalar() or 0
            result["active_users"] = active_users

            # 处理中的任务数
            processing_count = (
                db.query(func.count(VideoTask.id))
                .filter(VideoTask.status.in_(["submitted", "queued", "processing"]))
                .scalar()
                or 0
            )
            result["processing_count"] = processing_count

        return result


@dataclass
class VideoTaskDetailAdapter(AuthenticatedApiAdapter):
    """视频任务详情适配器"""

    task_id: str

    async def handle(self, context: ApiRequestContext) -> Any:
        db = context.db
        user = context.user
        is_admin = user.role == UserRole.ADMIN

        query = db.query(VideoTask).filter(VideoTask.id == self.task_id)
        if not is_admin:
            query = query.filter(VideoTask.user_id == user.id)

        task = query.first()
        if not task:
            raise HTTPException(status_code=404, detail="Video task not found")

        # 获取用户信息
        task_user = db.query(User).filter(User.id == task.user_id).first()
        username = task_user.username if task_user else "Unknown"

        # 获取 Provider 信息
        provider = db.query(Provider).filter(Provider.id == task.provider_id).first()
        provider_name = provider.name if provider else "Unknown"

        # 获取 Endpoint 信息
        endpoint = (
            db.query(ProviderEndpoint).filter(ProviderEndpoint.id == task.endpoint_id).first()
        )
        endpoint_info = None
        if endpoint:
            endpoint_info = {
                "id": endpoint.id,
                "base_url": endpoint.base_url,
                "api_format": str(endpoint.api_format),
            }

        return {
            "id": task.id,
            "external_task_id": task.external_task_id,
            "user_id": task.user_id,
            "username": username,
            "api_key_id": task.api_key_id,
            "provider_id": task.provider_id,
            "provider_name": provider_name,
            "endpoint_id": task.endpoint_id,
            "endpoint": endpoint_info,
            "key_id": task.key_id,
            "client_api_format": task.client_api_format,
            "provider_api_format": task.provider_api_format,
            "format_converted": task.format_converted,
            "model": task.model,
            "prompt": task.prompt,
            "original_request_body": task.original_request_body,
            "converted_request_body": task.converted_request_body,
            "duration_seconds": task.duration_seconds,
            "resolution": task.resolution,
            "aspect_ratio": task.aspect_ratio,
            "size": task.size,
            "status": task.status,
            "progress_percent": task.progress_percent,
            "progress_message": task.progress_message,
            "video_url": task.video_url,
            "video_urls": task.video_urls,
            "thumbnail_url": task.thumbnail_url,
            "video_size_bytes": task.video_size_bytes,
            "video_duration_seconds": task.video_duration_seconds,
            "video_expires_at": (
                task.video_expires_at.isoformat() if task.video_expires_at else None
            ),
            "stored_video_path": task.stored_video_path,
            "storage_provider": task.storage_provider,
            "error_code": task.error_code,
            "error_message": task.error_message,
            "retry_count": task.retry_count,
            "max_retries": task.max_retries,
            "poll_interval_seconds": task.poll_interval_seconds,
            "next_poll_at": task.next_poll_at.isoformat() if task.next_poll_at else None,
            "poll_count": task.poll_count,
            "max_poll_count": task.max_poll_count,
            "created_at": task.created_at.isoformat() if task.created_at else None,
            "updated_at": task.updated_at.isoformat() if task.updated_at else None,
            "submitted_at": task.submitted_at.isoformat() if task.submitted_at else None,
            "completed_at": task.completed_at.isoformat() if task.completed_at else None,
            "request_metadata": task.request_metadata,
        }


@dataclass
class VideoTaskCancelAdapter(AuthenticatedApiAdapter):
    """视频任务取消适配器"""

    task_id: str

    async def handle(self, context: ApiRequestContext) -> Any:
        from src.core.api_format.conversion.internal_video import VideoStatus

        db = context.db
        user = context.user
        is_admin = user.role == UserRole.ADMIN

        query = db.query(VideoTask).filter(VideoTask.id == self.task_id)
        if not is_admin:
            query = query.filter(VideoTask.user_id == user.id)

        task = query.first()
        if not task:
            raise HTTPException(status_code=404, detail="Video task not found")

        # 只能取消进行中的任务
        if task.status in [
            VideoStatus.COMPLETED.value,
            VideoStatus.FAILED.value,
            VideoStatus.CANCELLED.value,
        ]:
            raise HTTPException(
                status_code=400, detail=f"Cannot cancel task with status: {task.status}"
            )

        # 更新状态
        task.status = VideoStatus.CANCELLED.value
        task.updated_at = datetime.now(timezone.utc)
        task.completed_at = datetime.now(timezone.utc)
        db.commit()

        return {
            "id": task.id,
            "status": task.status,
            "message": "Task cancelled successfully",
        }
