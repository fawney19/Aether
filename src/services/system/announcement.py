"""
公告系统服务
"""

from __future__ import annotations

from datetime import datetime, timezone

from sqlalchemy import func, or_
from sqlalchemy.orm import Session

from src.core.exceptions import ForbiddenException, NotFoundException
from src.core.logger import logger
from src.models.database import Announcement, AnnouncementRead, User, UserRole


class AnnouncementService:
    """公告系统服务"""

    @staticmethod
    def create_announcement(
        db: Session,
        author_id: str,  # UUID
        title: str,
        content: str,
        type: str = "info",
        priority: int = 0,
        is_pinned: bool = False,
        start_time: datetime | None = None,
        end_time: datetime | None = None,
    ) -> Announcement:
        """创建公告"""
        # 验证作者是否为管理员
        author = db.query(User).filter(User.id == author_id).first()
        if not author or author.role != UserRole.ADMIN:
            raise ForbiddenException("Only administrators can create announcements")

        # 验证类型
        if type not in ["info", "warning", "maintenance", "important"]:
            raise ValueError("Invalid announcement type")

        announcement = Announcement(
            title=title,
            content=content,
            type=type,
            priority=priority,
            author_id=author_id,
            is_pinned=is_pinned,
            start_time=start_time,
            end_time=end_time,
            is_active=True,
        )

        db.add(announcement)
        db.commit()
        db.refresh(announcement)

        logger.info(f"Created announcement: {announcement.id} - {title}")
        return announcement

    @staticmethod
    def get_announcements(
        db: Session,
        user_id: str | None = None,  # UUID
        active_only: bool = True,
        include_read_status: bool = False,
        limit: int = 50,
        offset: int = 0,
    ) -> dict:
        """获取公告列表"""
        filtered_query = db.query(Announcement)

        # 筛选条件
        if active_only:
            now = datetime.now(timezone.utc)
            filtered_query = filtered_query.filter(
                Announcement.is_active == True,
                or_(Announcement.start_time == None, Announcement.start_time <= now),
                or_(Announcement.end_time == None, Announcement.end_time >= now),
            )

        # 分页
        total = int(filtered_query.with_entities(func.count(Announcement.id)).scalar() or 0)

        # 排序：置顶优先，然后按优先级和创建时间
        ordered_query = filtered_query.order_by(
            Announcement.is_pinned.desc(),
            Announcement.priority.desc(),
            Announcement.created_at.desc(),
        )
        announcements = ordered_query.offset(offset).limit(limit).all()

        # 获取已读状态
        read_announcement_ids = set()
        unread_count = 0

        if user_id and include_read_status:
            current_page_ids = [announcement.id for announcement in announcements]
            if current_page_ids:
                read_records = (
                    db.query(AnnouncementRead.announcement_id)
                    .filter(
                        AnnouncementRead.user_id == user_id,
                        AnnouncementRead.announcement_id.in_(current_page_ids),
                    )
                    .all()
                )
                read_announcement_ids = {record[0] for record in read_records}

            unread_query = filtered_query.filter(
                ~Announcement.id.in_(
                    db.query(AnnouncementRead.announcement_id).filter(
                        AnnouncementRead.user_id == user_id
                    )
                )
            )
            unread_count = int(unread_query.with_entities(func.count(Announcement.id)).scalar() or 0)

        # 构建返回数据
        items = []
        for announcement in announcements:
            item = {
                "id": announcement.id,
                "title": announcement.title,
                "content": announcement.content,
                "type": announcement.type,
                "priority": announcement.priority,
                "is_pinned": announcement.is_pinned,
                "is_active": announcement.is_active,
                "author": {"id": announcement.author.id, "username": announcement.author.username},
                "start_time": announcement.start_time,
                "end_time": announcement.end_time,
                "created_at": announcement.created_at,
                "updated_at": announcement.updated_at,
            }

            if include_read_status and user_id:
                item["is_read"] = announcement.id in read_announcement_ids

            items.append(item)

        result = {"items": items, "total": total}

        if include_read_status and user_id:
            result["unread_count"] = unread_count

        return result

    @staticmethod
    def get_announcement(db: Session, announcement_id: str) -> Announcement:  # UUID
        """获取单个公告"""
        announcement = db.query(Announcement).filter(Announcement.id == announcement_id).first()

        if not announcement:
            raise NotFoundException("Announcement not found")

        return announcement

    @staticmethod
    def update_announcement(
        db: Session,
        announcement_id: str,  # UUID
        user_id: str,  # UUID
        title: str | None = None,
        content: str | None = None,
        type: str | None = None,
        priority: int | None = None,
        is_active: bool | None = None,
        is_pinned: bool | None = None,
        start_time: datetime | None = None,
        end_time: datetime | None = None,
    ) -> Announcement:
        """更新公告"""
        # 验证用户是否为管理员
        user = db.query(User).filter(User.id == user_id).first()
        if not user or user.role != UserRole.ADMIN:
            raise ForbiddenException("Only administrators can update announcements")

        announcement = AnnouncementService.get_announcement(db, announcement_id)

        # 更新提供的字段
        if title is not None:
            announcement.title = title
        if content is not None:
            announcement.content = content
        if type is not None:
            if type not in ["info", "warning", "maintenance", "important"]:
                raise ValueError("Invalid announcement type")
            announcement.type = type
        if priority is not None:
            announcement.priority = priority
        if is_active is not None:
            announcement.is_active = is_active
        if is_pinned is not None:
            announcement.is_pinned = is_pinned
        if start_time is not None:
            announcement.start_time = start_time
        if end_time is not None:
            announcement.end_time = end_time

        db.commit()
        db.refresh(announcement)

        logger.info(f"Updated announcement: {announcement_id}")
        return announcement

    @staticmethod
    def delete_announcement(db: Session, announcement_id: str, user_id: str) -> None:  # UUID
        """删除公告"""
        # 验证用户是否为管理员
        user = db.query(User).filter(User.id == user_id).first()
        if not user or user.role != UserRole.ADMIN:
            raise ForbiddenException("Only administrators can delete announcements")

        announcement = AnnouncementService.get_announcement(db, announcement_id)

        db.delete(announcement)
        db.commit()

        logger.info(f"Deleted announcement: {announcement_id}")

    @staticmethod
    def mark_as_read(db: Session, announcement_id: str, user_id: str) -> None:  # UUID
        """标记公告为已读"""
        # 检查公告是否存在
        announcement = AnnouncementService.get_announcement(db, announcement_id)

        # 检查是否已经标记为已读
        existing = (
            db.query(AnnouncementRead)
            .filter(
                AnnouncementRead.user_id == user_id,
                AnnouncementRead.announcement_id == announcement_id,
            )
            .first()
        )

        if not existing:
            read_record = AnnouncementRead(user_id=user_id, announcement_id=announcement_id)
            db.add(read_record)
            db.commit()

            logger.info(f"User {user_id} marked announcement {announcement_id} as read")

    @staticmethod
    def mark_all_as_read(db: Session, user_id: str, active_only: bool = True) -> int:
        """将当前可见公告全部标记为已读。"""
        query = db.query(Announcement.id)

        if active_only:
            now = datetime.now(timezone.utc)
            query = query.filter(
                Announcement.is_active == True,
                or_(Announcement.start_time == None, Announcement.start_time <= now),
                or_(Announcement.end_time == None, Announcement.end_time >= now),
            )

        announcement_ids = [row[0] for row in query.all()]
        if not announcement_ids:
            return 0

        existing_ids = {
            row[0]
            for row in (
                db.query(AnnouncementRead.announcement_id)
                .filter(
                    AnnouncementRead.user_id == user_id,
                    AnnouncementRead.announcement_id.in_(announcement_ids),
                )
                .all()
            )
        }
        pending_ids = [announcement_id for announcement_id in announcement_ids if announcement_id not in existing_ids]
        if not pending_ids:
            return 0

        now = datetime.now(timezone.utc)
        db.add_all(
            AnnouncementRead(
                user_id=user_id,
                announcement_id=announcement_id,
                read_at=now,
            )
            for announcement_id in pending_ids
        )
        db.commit()

        logger.info(f"User {user_id} marked {len(pending_ids)} announcements as read")
        return len(pending_ids)

    @staticmethod
    def get_active_announcements(db: Session, user_id: str | None = None) -> dict:  # UUID
        """获取当前有效的公告（首页展示用）"""
        return AnnouncementService.get_announcements(
            db=db,
            user_id=user_id,
            active_only=True,
            include_read_status=True if user_id else False,
            limit=10,
        )
