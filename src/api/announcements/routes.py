"""公告系统 API 端点。"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from fastapi import APIRouter, Depends, Query, Request
from pydantic import ValidationError
from sqlalchemy.orm import Session

from src.api.base.adapter import ApiAdapter, ApiMode
from src.api.base.admin_adapter import AdminApiAdapter
from src.api.base.authenticated_adapter import AuthenticatedApiAdapter
from src.api.base.context import ApiRequestContext
from src.api.base.pipeline import get_pipeline
from src.core.exceptions import InvalidRequestException, translate_pydantic_error
from src.database import get_db
from src.models.api import CreateAnnouncementRequest, UpdateAnnouncementRequest
from src.models.database import User
from src.services.system.announcement import AnnouncementService
from src.utils.auth_utils import authenticate_user_from_bearer_token

router = APIRouter(prefix="/api/announcements", tags=["Announcements"])
pipeline = get_pipeline()


# ============== 公共端点（所有用户可访问） ==============


@router.get("")
async def list_announcements(
    request: Request,
    active_only: bool = Query(True, description="只返回有效公告"),
    limit: int = Query(50, description="返回数量限制"),
    offset: int = Query(0, description="偏移量"),
    db: Session = Depends(get_db),
) -> Any:
    """
    获取公告列表

    获取公告列表，支持分页和筛选。如果用户已登录，返回包含已读状态。

    **查询参数**:
    - `active_only`: 是否只返回有效公告，默认 true
    - `limit`: 返回数量限制，默认 50
    - `offset`: 分页偏移量，默认 0

    **返回字段**:
    - `items`: 公告列表，每条公告包含：
      - `id`: 公告 ID
      - `title`: 标题
      - `content`: 内容
      - `type`: 类型（info/warning/error/success）
      - `priority`: 优先级
      - `is_pinned`: 是否置顶
      - `is_read`: 是否已读（仅登录用户）
      - `author`: 作者信息
      - `start_time`: 生效开始时间
      - `end_time`: 生效结束时间
      - `created_at`: 创建时间
    - `total`: 总数
    - `unread_count`: 未读数量（仅登录用户）
    """
    adapter = ListAnnouncementsAdapter(active_only=active_only, limit=limit, offset=offset)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/active")
async def get_active_announcements(
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    获取当前有效的公告

    获取当前时间范围内有效的公告列表，用于首页展示。

    **返回字段**:
    - `items`: 有效公告列表
    - `total`: 有效公告总数
    - `unread_count`: 未读数量（仅登录用户）
    """
    adapter = GetActiveAnnouncementsAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/{announcement_id}")
async def get_announcement(
    announcement_id: str,  # UUID
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    获取单个公告详情

    获取指定公告的详细信息。

    **路径参数**:
    - `announcement_id`: 公告 ID（UUID）

    **返回字段**:
    - `id`: 公告 ID
    - `title`: 标题
    - `content`: 内容
    - `type`: 类型（info/warning/error/success）
    - `priority`: 优先级
    - `is_pinned`: 是否置顶
    - `author`: 作者信息（id, username）
    - `start_time`: 生效开始时间
    - `end_time`: 生效结束时间
    - `created_at`: 创建时间
    - `updated_at`: 更新时间
    """
    adapter = GetAnnouncementAdapter(announcement_id=announcement_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.patch("/{announcement_id}/read-status")
async def mark_announcement_as_read(
    announcement_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    标记公告为已读

    将指定公告标记为当前用户已读。需要登录。

    **路径参数**:
    - `announcement_id`: 公告 ID（UUID）

    **返回字段**:
    - `message`: 操作结果信息
    """
    adapter = MarkAnnouncementReadAdapter(announcement_id=announcement_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/read-all")
async def mark_all_announcements_as_read(
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    标记全部公告为已读

    将当前用户可见的全部有效公告标记为已读。需要登录。

    **返回字段**:
    - `message`: 操作结果信息
    - `marked_count`: 本次新增的已读数量
    """
    adapter = MarkAllAnnouncementsReadAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


# ============== 管理员端点 ==============


@router.post("")
async def create_announcement(
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    创建公告

    创建新的系统公告。需要管理员权限。

    **请求体字段**:
    - `title`: 公告标题（必填）
    - `content`: 公告内容（必填）
    - `type`: 公告类型（info/warning/error/success），默认 info
    - `priority`: 优先级（0-100），默认 0
    - `is_pinned`: 是否置顶，默认 false
    - `start_time`: 生效开始时间（可选）
    - `end_time`: 生效结束时间（可选）

    **返回字段**:
    - `id`: 新创建的公告 ID
    - `title`: 公告标题
    - `message`: 操作结果信息
    """
    adapter = CreateAnnouncementAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.put("/{announcement_id}")
async def update_announcement(
    announcement_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    更新公告

    更新指定公告的信息。需要管理员权限。

    **路径参数**:
    - `announcement_id`: 公告 ID（UUID）

    **请求体字段（均为可选）**:
    - `title`: 公告标题
    - `content`: 公告内容
    - `type`: 公告类型（info/warning/error/success）
    - `priority`: 优先级（0-100）
    - `is_active`: 是否启用
    - `is_pinned`: 是否置顶
    - `start_time`: 生效开始时间
    - `end_time`: 生效结束时间

    **返回字段**:
    - `message`: 操作结果信息
    """
    adapter = UpdateAnnouncementAdapter(announcement_id=announcement_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.delete("/{announcement_id}")
async def delete_announcement(
    announcement_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    删除公告

    删除指定的公告。需要管理员权限。

    **路径参数**:
    - `announcement_id`: 公告 ID（UUID）

    **返回字段**:
    - `message`: 操作结果信息
    """
    adapter = DeleteAnnouncementAdapter(announcement_id=announcement_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


# ============== 用户公告端点 ==============


@router.get("/users/me/unread-count")
async def get_my_unread_announcement_count(
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    获取我的未读公告数量

    获取当前用户的未读公告数量。需要登录。

    **返回字段**:
    - `unread_count`: 未读公告数量
    """
    adapter = UnreadAnnouncementCountAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


# ============== Pipeline 适配器 ==============


class AnnouncementOptionalAuthAdapter(ApiAdapter):
    """允许匿名访问，但可选解析Bearer以获取用户上下文。"""

    mode = ApiMode.PUBLIC

    async def authorize(self, context: ApiRequestContext) -> None:  # type: ignore[override]
        context.extra["optional_user"] = await self._resolve_optional_user(context)
        return None

    async def _resolve_optional_user(self, context: ApiRequestContext) -> User | None:
        if context.user:
            return context.user

        authorization = context.request.headers.get("authorization")
        if not authorization or not authorization.lower().startswith("bearer "):
            return None

        token = authorization[7:].strip()
        try:
            return await authenticate_user_from_bearer_token(token, context.db, context.request)
        except Exception:
            return None

    def get_optional_user(self, context: ApiRequestContext) -> User | None:
        return context.extra.get("optional_user")


@dataclass
class ListAnnouncementsAdapter(AnnouncementOptionalAuthAdapter):
    active_only: bool
    limit: int
    offset: int

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        optional_user = self.get_optional_user(context)
        return AnnouncementService.get_announcements(
            db=context.db,
            user_id=optional_user.id if optional_user else None,
            active_only=self.active_only,
            include_read_status=True if optional_user else False,
            limit=self.limit,
            offset=self.offset,
        )


class GetActiveAnnouncementsAdapter(AnnouncementOptionalAuthAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        optional_user = self.get_optional_user(context)
        return AnnouncementService.get_active_announcements(
            db=context.db,
            user_id=optional_user.id if optional_user else None,
        )


@dataclass
class GetAnnouncementAdapter(AnnouncementOptionalAuthAdapter):
    announcement_id: str

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        announcement = AnnouncementService.get_announcement(context.db, self.announcement_id)
        return {
            "id": announcement.id,
            "title": announcement.title,
            "content": announcement.content,
            "type": announcement.type,
            "priority": announcement.priority,
            "is_pinned": announcement.is_pinned,
            "author": {"id": announcement.author.id, "username": announcement.author.username},
            "start_time": announcement.start_time,
            "end_time": announcement.end_time,
            "created_at": announcement.created_at,
            "updated_at": announcement.updated_at,
        }


class AnnouncementUserAdapter(AuthenticatedApiAdapter):
    """需要登录但不要求管理员的公告适配器基类。"""

    pass


class MarkAnnouncementReadAdapter(AnnouncementUserAdapter):
    def __init__(self, announcement_id: str):
        self.announcement_id = announcement_id

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        AnnouncementService.mark_as_read(context.db, self.announcement_id, context.user.id)
        return {"message": "公告已标记为已读"}


class MarkAllAnnouncementsReadAdapter(AnnouncementUserAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        marked_count = AnnouncementService.mark_all_as_read(
            context.db,
            context.user.id,
            active_only=True,
        )
        return {"message": "已全部标记为已读", "marked_count": marked_count}


class UnreadAnnouncementCountAdapter(AnnouncementUserAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        result = AnnouncementService.get_announcements(
            db=context.db,
            user_id=context.user.id,
            active_only=True,
            include_read_status=True,
            limit=1,
            offset=0,
        )
        return {"unread_count": result.get("unread_count", 0)}


class CreateAnnouncementAdapter(AdminApiAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        payload = context.ensure_json_body()
        try:
            req = CreateAnnouncementRequest.model_validate(payload)
        except ValidationError as e:
            errors = e.errors()
            if errors:
                raise InvalidRequestException(translate_pydantic_error(errors[0]))
            raise InvalidRequestException("请求数据验证失败")

        announcement = AnnouncementService.create_announcement(
            db=context.db,
            author_id=context.user.id,
            title=req.title,
            content=req.content,
            type=req.type,
            priority=req.priority,
            is_pinned=req.is_pinned,
            start_time=req.start_time,
            end_time=req.end_time,
        )
        return {"id": announcement.id, "title": announcement.title, "message": "公告创建成功"}


@dataclass
class UpdateAnnouncementAdapter(AdminApiAdapter):
    announcement_id: str

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        payload = context.ensure_json_body()
        try:
            req = UpdateAnnouncementRequest.model_validate(payload)
        except ValidationError as e:
            errors = e.errors()
            if errors:
                raise InvalidRequestException(translate_pydantic_error(errors[0]))
            raise InvalidRequestException("请求数据验证失败")

        AnnouncementService.update_announcement(
            db=context.db,
            announcement_id=self.announcement_id,
            user_id=context.user.id,
            title=req.title,
            content=req.content,
            type=req.type,
            priority=req.priority,
            is_active=req.is_active,
            is_pinned=req.is_pinned,
            start_time=req.start_time,
            end_time=req.end_time,
        )
        return {"message": "公告更新成功"}


@dataclass
class DeleteAnnouncementAdapter(AdminApiAdapter):
    announcement_id: str

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        AnnouncementService.delete_announcement(context.db, self.announcement_id, context.user.id)
        return {"message": "公告已删除"}
