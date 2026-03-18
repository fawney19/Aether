from __future__ import annotations

from datetime import datetime, timedelta, timezone
from types import SimpleNamespace

import pytest
from fastapi import HTTPException
from sqlalchemy import create_engine
from sqlalchemy.orm import Session, sessionmaker

from src.core.enums import AuthSource, UserRole
from src.models.database import Base, User, UserSession
from src.services.auth.session_service import (
    _DEVICE_ID_PATTERN,
    MAX_SESSIONS_PER_USER,
    TERMINAL_SESSION_RETENTION_DAYS,
    SessionClientContext,
    SessionService,
)


def _make_session(
    *,
    token: str = "refresh-token-value",
    expires_delta: timedelta | None = None,
    revoked: bool = False,
) -> UserSession:
    expires_at = datetime.now(timezone.utc) + (expires_delta or timedelta(days=7))
    session = UserSession(
        user_id="user-1",
        client_device_id="device-1",
        refresh_token_hash="",
        expires_at=expires_at,
    )
    session.set_refresh_token(token)
    if revoked:
        session.revoked_at = datetime.now(timezone.utc)
    return session


def _make_db_session() -> Session:
    engine = create_engine("sqlite:///:memory:")
    Base.metadata.create_all(engine, tables=[User.__table__, UserSession.__table__])
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


def _make_user(db: Session, *, user_id: str = "user-1") -> User:
    user = User(
        id=user_id,
        email=f"{user_id}@example.com",
        email_verified=True,
        username=user_id,
        role=UserRole.USER,
        auth_source=AuthSource.LOCAL,
        is_active=True,
        is_deleted=False,
    )
    db.add(user)
    db.commit()
    return user


def _make_client_context(device_id: str) -> SessionClientContext:
    return SessionClientContext(
        client_device_id=device_id,
        device_label=f"Device {device_id}",
        device_type="desktop",
        browser_name="Chrome",
        browser_version="136.0",
        os_name="macOS",
        os_version="15",
        device_model=None,
        client_hints={},
        ip_address="127.0.0.1",
        user_agent="pytest-agent",
    )


def test_build_client_context_prefers_client_hints() -> None:
    context = SessionService.build_client_context(
        client_device_id="device-1",
        client_ip="127.0.0.1",
        user_agent=(
            "Mozilla/5.0 (Linux; Android 15; Pixel 9) AppleWebKit/537.36 "
            "(KHTML, like Gecko) Chrome/134.0.0.0 Mobile Safari/537.36"
        ),
        headers={
            "sec-ch-ua-platform": '"Android"',
            "sec-ch-ua-platform-version": '"15.0.0"',
            "sec-ch-ua-model": '"Pixel 9"',
            "sec-ch-ua-mobile": "?1",
        },
    )

    assert context.client_device_id == "device-1"
    assert context.device_type == "mobile"
    assert context.os_name == "Android"
    assert context.os_version == "15.0.0"
    assert context.device_model == "Pixel 9"
    assert context.device_label == "Pixel 9"


# ── Refresh token hash roundtrip ──


def test_user_session_refresh_token_hash_roundtrip() -> None:
    session = _make_session(token="refresh-token-value")

    is_valid, is_prev = session.verify_refresh_token("refresh-token-value")
    assert is_valid is True
    assert is_prev is False

    is_valid, is_prev = session.verify_refresh_token("different-token")
    assert is_valid is False
    assert is_prev is False


def test_verify_refresh_token_grace_window() -> None:
    """轮换后短时间内旧 token 仍应通过验证（grace window）。"""
    session = _make_session(token="old-token")
    # 模拟轮换
    session.set_refresh_token("new-token")

    # 新 token 正常通过
    is_valid, is_prev = session.verify_refresh_token("new-token")
    assert is_valid is True
    assert is_prev is False

    # 旧 token 在宽限窗口内通过
    is_valid, is_prev = session.verify_refresh_token("old-token")
    assert is_valid is True
    assert is_prev is True

    # 完全无关的 token 不通过
    is_valid, is_prev = session.verify_refresh_token("random-token")
    assert is_valid is False
    assert is_prev is False


def test_verify_refresh_token_grace_window_expired() -> None:
    """宽限窗口超时后，旧 token 不再通过。"""
    session = _make_session(token="old-token")
    session.set_refresh_token("new-token")
    # 手动把 rotated_at 设为 30 秒前，超过 REFRESH_GRACE_SECONDS
    session.rotated_at = datetime.now(timezone.utc) - timedelta(seconds=30)

    is_valid, is_prev = session.verify_refresh_token("old-token")
    assert is_valid is False
    assert is_prev is False


# ── Session expiry ──


def test_user_session_expired_property_handles_aware_datetime() -> None:
    session = _make_session(expires_delta=timedelta(minutes=-1))
    assert session.is_expired is True


def test_user_session_not_expired() -> None:
    session = _make_session(expires_delta=timedelta(days=7))
    assert session.is_expired is False


def test_user_session_revoked_property() -> None:
    session = _make_session(revoked=True)
    assert session.is_revoked is True

    session2 = _make_session(revoked=False)
    assert session2.is_revoked is False


# ── Device ID validation ──


def test_device_id_pattern_accepts_valid_ids() -> None:
    assert _DEVICE_ID_PATTERN.match("abc-123_DEF")
    assert _DEVICE_ID_PATTERN.match("a" * 128)
    assert _DEVICE_ID_PATTERN.match("simple-uuid-v4-like-id")


def test_device_id_pattern_rejects_invalid_ids() -> None:
    assert not _DEVICE_ID_PATTERN.match("")
    assert not _DEVICE_ID_PATTERN.match("a" * 129)
    assert not _DEVICE_ID_PATTERN.match("has spaces")
    assert not _DEVICE_ID_PATTERN.match("has<script>xss</script>")
    assert not _DEVICE_ID_PATTERN.match("日本語")


def test_normalize_device_id_strips_and_validates() -> None:
    assert SessionService._normalize_device_id("  valid-id  ") == "valid-id"
    assert SessionService._normalize_device_id("a" * 200) is not None  # truncated to 128
    assert SessionService._normalize_device_id("<script>") is None
    assert SessionService._normalize_device_id("   ") is None


def test_extract_client_device_id_requires_valid_value() -> None:
    request = SimpleNamespace(headers={}, query_params={})

    with pytest.raises(HTTPException, match="缺少或无效的设备标识"):
        SessionService.extract_client_device_id(request)  # type: ignore[arg-type]


def test_assert_session_device_matches_rejects_mismatch() -> None:
    session = _make_session()
    session.client_device_id = "device-expected"

    with pytest.raises(HTTPException, match="设备标识与登录会话不匹配"):
        SessionService.assert_session_device_matches(session, "device-actual")


# ── set_refresh_token preserves previous hash ──


def test_set_refresh_token_stores_previous_hash() -> None:
    session = _make_session(token="token-1")
    original_hash = session.refresh_token_hash
    assert session.prev_refresh_token_hash is None or session.prev_refresh_token_hash == ""

    session.set_refresh_token("token-2")
    assert session.prev_refresh_token_hash == original_hash
    assert session.rotated_at is not None
    assert session.refresh_token_hash != original_hash


def test_touch_session_skips_recent_activity() -> None:
    now = datetime.now(timezone.utc)
    session = UserSession(
        user_id="user-1",
        client_device_id="device-1",
        refresh_token_hash="",
        expires_at=now + timedelta(days=7),
        last_seen_at=now,
        ip_address="127.0.0.1",
        user_agent="old-agent",
    )

    touched = SessionService.touch_session(
        session,
        client_ip="192.168.0.1",
        user_agent="new-agent",
    )

    assert touched is False
    assert session.last_seen_at == now
    assert session.ip_address == "127.0.0.1"
    assert session.user_agent == "old-agent"


def test_touch_session_updates_stale_session() -> None:
    now = datetime.now(timezone.utc)
    last_seen_at = now - timedelta(minutes=10)
    session = UserSession(
        user_id="user-1",
        client_device_id="device-1",
        refresh_token_hash="",
        expires_at=now + timedelta(days=7),
        last_seen_at=last_seen_at,
        ip_address="127.0.0.1",
        user_agent="old-agent",
    )

    touched = SessionService.touch_session(
        session,
        client_ip="192.168.0.1",
        user_agent="new-agent",
    )

    assert touched is True
    assert session.last_seen_at is not None and session.last_seen_at > last_seen_at
    assert session.ip_address == "192.168.0.1"
    assert session.user_agent == "new-agent"


def test_get_session_for_user_can_lock_for_update() -> None:
    expected = object()

    class DummyQuery:
        def __init__(self) -> None:
            self.locked = False

        def filter(self, *_args: object, **_kwargs: object) -> "DummyQuery":
            return self

        def with_for_update(self) -> "DummyQuery":
            self.locked = True
            return self

        def first(self) -> object:
            return expected

    query = DummyQuery()
    db = SimpleNamespace(query=lambda _model: query)

    result = SessionService.get_session_for_user(
        db,  # type: ignore[arg-type]
        user_id="user-1",
        session_id="session-1",
        lock_for_update=True,
    )

    assert result is expected
    assert query.locked is True


def test_create_session_session_limit_ignores_expired_sessions() -> None:
    db = _make_db_session()
    try:
        user = _make_user(db)
        now = datetime.now(timezone.utc)
        for idx in range(MAX_SESSIONS_PER_USER - 1):
            session = UserSession(
                id=f"active-{idx}",
                user_id=user.id,
                client_device_id=f"active-device-{idx}",
                refresh_token_hash="",
                expires_at=now + timedelta(days=7),
                last_seen_at=now + timedelta(minutes=idx),
            )
            session.set_refresh_token(f"active-token-{idx}")
            db.add(session)

        expired = UserSession(
            id="expired-1",
            user_id=user.id,
            client_device_id="expired-device",
            refresh_token_hash="",
            expires_at=now - timedelta(minutes=5),
            last_seen_at=now - timedelta(days=1),
        )
        expired.set_refresh_token("expired-token")
        db.add(expired)
        db.commit()

        created = SessionService.create_session(
            db,
            user=user,
            session_id="new-session",
            refresh_token="new-refresh-token",
            expires_at=now + timedelta(days=7),
            client=_make_client_context("new-device"),
        )
        db.commit()

        assert created.id == "new-session"
        active_sessions = SessionService.list_user_sessions(db, user_id=user.id)
        assert len(active_sessions) == MAX_SESSIONS_PER_USER
        assert all(session.revoke_reason != "session_limit_exceeded" for session in active_sessions)
    finally:
        _close_db_session(db)


def test_revoke_all_user_sessions_skips_expired_sessions() -> None:
    db = _make_db_session()
    try:
        user = _make_user(db)
        now = datetime.now(timezone.utc)

        active = UserSession(
            id="active-1",
            user_id=user.id,
            client_device_id="active-device",
            refresh_token_hash="",
            expires_at=now + timedelta(days=7),
            last_seen_at=now,
        )
        active.set_refresh_token("active-token")
        expired = UserSession(
            id="expired-1",
            user_id=user.id,
            client_device_id="expired-device",
            refresh_token_hash="",
            expires_at=now - timedelta(minutes=1),
            last_seen_at=now - timedelta(days=1),
        )
        expired.set_refresh_token("expired-token")
        db.add_all([active, expired])
        db.commit()

        revoked_count = SessionService.revoke_all_user_sessions(
            db,
            user_id=user.id,
            reason="security_review",
        )

        db.flush()
        db.refresh(active)
        db.refresh(expired)

        assert revoked_count == 1
        assert active.revoked_at is not None
        assert expired.revoked_at is None
    finally:
        _close_db_session(db)


def test_list_user_sessions_prunes_old_terminal_sessions() -> None:
    db = _make_db_session()
    try:
        user = _make_user(db)
        now = datetime.now(timezone.utc)

        active = UserSession(
            id="active-1",
            user_id=user.id,
            client_device_id="active-device",
            refresh_token_hash="",
            expires_at=now + timedelta(days=7),
            last_seen_at=now,
        )
        active.set_refresh_token("active-token")

        old_expired = UserSession(
            id="expired-old",
            user_id=user.id,
            client_device_id="expired-old-device",
            refresh_token_hash="",
            expires_at=now - timedelta(days=TERMINAL_SESSION_RETENTION_DAYS + 5),
            last_seen_at=now - timedelta(days=40),
        )
        old_expired.set_refresh_token("expired-old-token")

        old_revoked = UserSession(
            id="revoked-old",
            user_id=user.id,
            client_device_id="revoked-old-device",
            refresh_token_hash="",
            expires_at=now + timedelta(days=7),
            last_seen_at=now - timedelta(days=35),
        )
        old_revoked.set_refresh_token("revoked-old-token")
        old_revoked.revoked_at = now - timedelta(days=TERMINAL_SESSION_RETENTION_DAYS + 1)
        old_revoked.revoke_reason = "manual_revoke"

        recent_expired = UserSession(
            id="expired-recent",
            user_id=user.id,
            client_device_id="expired-recent-device",
            refresh_token_hash="",
            expires_at=now - timedelta(days=1),
            last_seen_at=now - timedelta(days=2),
        )
        recent_expired.set_refresh_token("expired-recent-token")

        db.add_all([active, old_expired, old_revoked, recent_expired])
        db.commit()

        sessions = SessionService.list_user_sessions(db, user_id=user.id)
        remaining_ids = {session.id for session in db.query(UserSession).all()}

        assert [session.id for session in sessions] == ["active-1"]
        assert "expired-old" not in remaining_ids
        assert "revoked-old" not in remaining_ids
        assert "expired-recent" in remaining_ids
    finally:
        _close_db_session(db)
