from __future__ import annotations

from datetime import datetime, timedelta, timezone

from sqlalchemy import create_engine
from sqlalchemy.orm import Session, sessionmaker

from src.core.enums import AuthSource, UserRole
from src.models.database import Announcement, AnnouncementRead, Base, User, UserGroup
from src.services.system.announcement import AnnouncementService


def _make_db_session() -> Session:
    engine = create_engine("sqlite:///:memory:")
    Base.metadata.create_all(
        engine,
        tables=[
            UserGroup.__table__,
            User.__table__,
            Announcement.__table__,
            AnnouncementRead.__table__,
        ],
    )
    session_factory = sessionmaker(bind=engine)
    db = session_factory()
    db.info["test_engine"] = engine
    return db


def _close_db_session(db: Session) -> None:
    engine = db.info.pop("test_engine", None)
    try:
        db.close()
    finally:
        if engine is not None:
            engine.dispose()


def _create_user(db: Session, *, user_id: str, role: UserRole) -> User:
    now = datetime.now(timezone.utc)
    user = User(
        id=user_id,
        email=f"{user_id}@example.com",
        email_verified=True,
        username=user_id,
        password_hash=None,
        role=role,
        auth_source=AuthSource.LOCAL,
        group_id=None,
        is_active=True,
        is_deleted=False,
        created_at=now,
        updated_at=now,
    )
    db.add(user)
    db.commit()
    db.refresh(user)
    return user


def _create_announcement(
    db: Session,
    *,
    announcement_id: str,
    author_id: str,
    title: str,
    is_active: bool = True,
    priority: int = 0,
) -> Announcement:
    now = datetime.now(timezone.utc)
    announcement = Announcement(
        id=announcement_id,
        title=title,
        content=f"{title} content",
        type="info",
        priority=priority,
        author_id=author_id,
        is_active=is_active,
        is_pinned=False,
        start_time=now - timedelta(hours=1),
        end_time=now + timedelta(hours=1),
        created_at=now,
        updated_at=now,
    )
    db.add(announcement)
    db.commit()
    db.refresh(announcement)
    return announcement


def _create_read_record(db: Session, *, user_id: str, announcement_id: str) -> AnnouncementRead:
    read = AnnouncementRead(
        user_id=user_id,
        announcement_id=announcement_id,
        read_at=datetime.now(timezone.utc),
    )
    db.add(read)
    db.commit()
    db.refresh(read)
    return read


def test_get_announcements_unread_count_ignores_inactive_reads() -> None:
    db = _make_db_session()
    try:
        admin = _create_user(db, user_id="admin-1", role=UserRole.ADMIN)
        user = _create_user(db, user_id="user-1", role=UserRole.USER)

        active_unread = _create_announcement(
            db,
            announcement_id="ann-active-unread",
            author_id=admin.id,
            title="Active unread",
            priority=30,
        )
        active_read = _create_announcement(
            db,
            announcement_id="ann-active-read",
            author_id=admin.id,
            title="Active read",
            priority=20,
        )
        inactive_read = _create_announcement(
            db,
            announcement_id="ann-inactive-read",
            author_id=admin.id,
            title="Inactive read",
            is_active=False,
            priority=10,
        )

        _create_read_record(db, user_id=user.id, announcement_id=active_read.id)
        _create_read_record(db, user_id=user.id, announcement_id=inactive_read.id)

        result = AnnouncementService.get_announcements(
            db,
            user_id=user.id,
            active_only=True,
            include_read_status=True,
            limit=20,
            offset=0,
        )

        assert result["total"] == 2
        assert result["unread_count"] == 1

        item_status = {item["id"]: item["is_read"] for item in result["items"]}
        assert item_status == {
            active_unread.id: False,
            active_read.id: True,
        }
    finally:
        _close_db_session(db)


def test_mark_all_as_read_only_marks_visible_active_announcements() -> None:
    db = _make_db_session()
    try:
        admin = _create_user(db, user_id="admin-2", role=UserRole.ADMIN)
        user = _create_user(db, user_id="user-2", role=UserRole.USER)

        active_unread = _create_announcement(
            db,
            announcement_id="ann-bulk-active-unread",
            author_id=admin.id,
            title="Bulk active unread",
        )
        active_read = _create_announcement(
            db,
            announcement_id="ann-bulk-active-read",
            author_id=admin.id,
            title="Bulk active read",
        )
        inactive_unread = _create_announcement(
            db,
            announcement_id="ann-bulk-inactive-unread",
            author_id=admin.id,
            title="Bulk inactive unread",
            is_active=False,
        )

        _create_read_record(db, user_id=user.id, announcement_id=active_read.id)

        marked_count = AnnouncementService.mark_all_as_read(db, user.id, active_only=True)

        assert marked_count == 1

        read_ids = {
            row[0]
            for row in db.query(AnnouncementRead.announcement_id)
            .filter(AnnouncementRead.user_id == user.id)
            .all()
        }
        assert read_ids == {active_unread.id, active_read.id}
        assert inactive_unread.id not in read_ids

        result = AnnouncementService.get_announcements(
            db,
            user_id=user.id,
            active_only=True,
            include_read_status=True,
            limit=20,
            offset=0,
        )
        assert result["unread_count"] == 0
        assert all(item["is_read"] is True for item in result["items"])
    finally:
        _close_db_session(db)
