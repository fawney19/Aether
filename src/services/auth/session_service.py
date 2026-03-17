from __future__ import annotations

import re
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from typing import Any, Mapping

from fastapi import HTTPException, Request, status
from sqlalchemy import and_, or_
from sqlalchemy.orm import Session

from src.core.logger import logger
from src.models.database import AuditEventType, User, UserSession
from src.services.system.audit import AuditService

SESSION_TOUCH_INTERVAL_SECONDS = 300
CLIENT_DEVICE_ID_HEADER = "X-Client-Device-Id"
MAX_SESSIONS_PER_USER = 20
TERMINAL_SESSION_RETENTION_DAYS = 30
_DEVICE_ID_PATTERN = re.compile(r"^[a-zA-Z0-9\-_]{1,128}$")


def _strip_hint(value: str | None) -> str | None:
    if value is None:
        return None
    cleaned = value.strip().strip('"').strip()
    return cleaned or None


def _parse_browser(user_agent: str) -> tuple[str | None, str | None]:
    patterns = [
        ("Edge", r"Edg/([\d.]+)"),
        ("Opera", r"OPR/([\d.]+)"),
        ("Chrome", r"Chrome/([\d.]+)"),
        ("Firefox", r"Firefox/([\d.]+)"),
        ("Safari", r"Version/([\d.]+).*Safari"),
    ]
    for name, pattern in patterns:
        match = re.search(pattern, user_agent)
        if match:
            return name, match.group(1)
    return None, None


def _parse_os(
    user_agent: str, client_hints: Mapping[str, str | None]
) -> tuple[str | None, str | None]:
    platform = _strip_hint(client_hints.get("sec-ch-ua-platform"))
    platform_version = _strip_hint(client_hints.get("sec-ch-ua-platform-version"))
    if platform:
        return platform, platform_version

    patterns = [
        ("Windows", r"Windows NT ([\d.]+)"),
        ("macOS", r"Mac OS X ([\d_]+)"),
        ("iOS", r"(?:iPhone OS|CPU OS) ([\d_]+)"),
        ("Android", r"Android ([\d.]+)"),
        ("Linux", r"Linux"),
    ]
    for name, pattern in patterns:
        match = re.search(pattern, user_agent)
        if match:
            version = match.group(1).replace("_", ".") if match.lastindex else None
            return name, version
    return None, None


def _parse_device_type(user_agent: str, client_hints: Mapping[str, str | None]) -> str:
    ch_mobile = _strip_hint(client_hints.get("sec-ch-ua-mobile"))
    ua_lower = user_agent.lower()
    if ch_mobile == "?1":
        return "mobile"
    if "ipad" in ua_lower or "tablet" in ua_lower:
        return "tablet"
    if any(marker in ua_lower for marker in ("iphone", "android", "mobile")):
        return "mobile"
    if any(marker in ua_lower for marker in ("macintosh", "windows", "linux", "x11")):
        return "desktop"
    return "unknown"


def _build_device_label(
    *,
    browser_name: str | None,
    os_name: str | None,
    device_model: str | None,
    device_type: str,
) -> str:
    if device_model:
        return device_model
    if browser_name and os_name:
        return f"{browser_name} / {os_name}"
    if browser_name:
        return browser_name
    if os_name:
        return os_name
    if device_type == "mobile":
        return "移动设备"
    if device_type == "tablet":
        return "平板设备"
    if device_type == "desktop":
        return "桌面设备"
    return "未知设备"


@dataclass(frozen=True)
class SessionClientContext:
    client_device_id: str
    device_label: str
    device_type: str
    browser_name: str | None
    browser_version: str | None
    os_name: str | None
    os_version: str | None
    device_model: str | None
    client_hints: dict[str, str | None]
    ip_address: str | None
    user_agent: str


class SessionService:
    """用户设备会话服务。"""

    @staticmethod
    def _utcnow() -> datetime:
        return datetime.now(timezone.utc)

    @staticmethod
    def _active_sessions_query(db: Session, *, user_id: str) -> Any:
        return db.query(UserSession).filter(
            UserSession.user_id == user_id,
            UserSession.revoked_at.is_(None),
            UserSession.expires_at > SessionService._utcnow(),
        )

    @staticmethod
    def cleanup_user_sessions(db: Session, *, user_id: str) -> int:
        """清理该用户已进入终态且超过保留期的会话记录。"""
        cutoff = SessionService._utcnow() - timedelta(days=TERMINAL_SESSION_RETENTION_DAYS)
        deleted = (
            db.query(UserSession)
            .filter(UserSession.user_id == user_id)
            .filter(
                or_(
                    UserSession.expires_at < cutoff,
                    and_(UserSession.revoked_at.is_not(None), UserSession.revoked_at < cutoff),
                )
            )
            .delete(synchronize_session=False)
        )
        if deleted:
            db.flush()
        return int(deleted or 0)

    @staticmethod
    def _normalize_device_id(raw: str) -> str | None:
        """校验并规范化 device id，非法值返回 None。"""
        cleaned = raw.strip()[:128]
        if not cleaned:
            return None
        if _DEVICE_ID_PATTERN.match(cleaned):
            return cleaned
        # 不符合格式的视为无效
        return None

    @staticmethod
    def extract_client_device_id(request: Request) -> str:
        header_value = request.headers.get(CLIENT_DEVICE_ID_HEADER)
        if header_value:
            normalized = SessionService._normalize_device_id(header_value)
            if normalized:
                return normalized

        query_value = request.query_params.get("client_device_id")
        if query_value:
            normalized = SessionService._normalize_device_id(query_value)
            if normalized:
                return normalized

        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="缺少或无效的设备标识",
        )

    @staticmethod
    def build_client_context(
        *,
        client_device_id: str,
        client_ip: str | None,
        user_agent: str,
        headers: Mapping[str, str],
    ) -> SessionClientContext:
        normalized_headers = {str(key).lower(): value for key, value in headers.items()}
        client_hints = {
            "sec-ch-ua": normalized_headers.get("sec-ch-ua"),
            "sec-ch-ua-platform": normalized_headers.get("sec-ch-ua-platform"),
            "sec-ch-ua-platform-version": normalized_headers.get("sec-ch-ua-platform-version"),
            "sec-ch-ua-model": normalized_headers.get("sec-ch-ua-model"),
            "sec-ch-ua-mobile": normalized_headers.get("sec-ch-ua-mobile"),
        }
        browser_name, browser_version = _parse_browser(user_agent)
        os_name, os_version = _parse_os(user_agent, client_hints)
        device_model = _strip_hint(client_hints.get("sec-ch-ua-model"))
        device_type = _parse_device_type(user_agent, client_hints)
        device_label = _build_device_label(
            browser_name=browser_name,
            os_name=os_name,
            device_model=device_model,
            device_type=device_type,
        )
        return SessionClientContext(
            client_device_id=client_device_id,
            device_label=device_label,
            device_type=device_type,
            browser_name=browser_name,
            browser_version=browser_version,
            os_name=os_name,
            os_version=os_version,
            device_model=device_model,
            client_hints=client_hints,
            ip_address=client_ip,
            user_agent=user_agent,
        )

    @staticmethod
    def get_active_session(db: Session, session_id: str, user_id: str) -> UserSession | None:
        session = (
            db.query(UserSession)
            .filter(UserSession.id == session_id, UserSession.user_id == user_id)
            .first()
        )
        if not session:
            return None
        if session.is_revoked or session.is_expired:
            return None
        return session

    @staticmethod
    def assert_session_device_matches(session: UserSession, client_device_id: str) -> None:
        if session.client_device_id != client_device_id:
            raise HTTPException(
                status_code=status.HTTP_401_UNAUTHORIZED,
                detail="设备标识与登录会话不匹配",
            )

    @staticmethod
    def create_session(
        db: Session,
        *,
        user: User,
        session_id: str,
        refresh_token: str,
        expires_at: datetime,
        client: SessionClientContext,
        revoke_existing_same_device: bool = True,
    ) -> UserSession:
        now = SessionService._utcnow()
        SessionService.cleanup_user_sessions(db, user_id=user.id)
        if revoke_existing_same_device:
            existing_sessions = (
                SessionService._active_sessions_query(db, user_id=user.id)
                .filter(UserSession.client_device_id == client.client_device_id)
                .all()
            )
            for existing in existing_sessions:
                existing.revoked_at = now
                existing.revoke_reason = "replaced_by_new_login"
                existing.updated_at = now

        # 如果活跃会话数超出限制，淘汰最旧的会话
        active_count = SessionService._active_sessions_query(db, user_id=user.id).count()
        if active_count >= MAX_SESSIONS_PER_USER:
            oldest_sessions = (
                SessionService._active_sessions_query(db, user_id=user.id)
                .order_by(UserSession.last_seen_at.asc())
                .limit(active_count - MAX_SESSIONS_PER_USER + 1)
                .all()
            )
            for old_session in oldest_sessions:
                old_session.revoked_at = now
                old_session.revoke_reason = "session_limit_exceeded"
                old_session.updated_at = now

        session = UserSession(
            id=session_id,
            user_id=user.id,
            client_device_id=client.client_device_id,
            device_label=client.device_label,
            device_type=client.device_type,
            browser_name=client.browser_name,
            browser_version=client.browser_version,
            os_name=client.os_name,
            os_version=client.os_version,
            device_model=client.device_model,
            ip_address=client.ip_address,
            user_agent=client.user_agent[:1000],
            client_hints=client.client_hints,
            last_seen_at=now,
            expires_at=expires_at,
            revoked_at=None,
            revoke_reason=None,
        )
        session.set_refresh_token(refresh_token)
        db.add(session)
        db.flush()
        return session

    @staticmethod
    def rotate_refresh_token(
        session: UserSession,
        *,
        refresh_token: str,
        expires_at: datetime,
        client_ip: str | None,
        user_agent: str,
    ) -> None:
        session.set_refresh_token(refresh_token)
        session.expires_at = expires_at
        session.last_seen_at = datetime.now(timezone.utc)
        if client_ip:
            session.ip_address = client_ip
        if user_agent:
            session.user_agent = user_agent[:1000]

    @staticmethod
    def touch_session(
        session: UserSession,
        *,
        client_ip: str | None,
        user_agent: str,
    ) -> None:
        now = datetime.now(timezone.utc)
        last_seen_at = session.last_seen_at
        if last_seen_at.tzinfo is None:
            last_seen_at = last_seen_at.replace(tzinfo=timezone.utc)
        if (now - last_seen_at).total_seconds() < SESSION_TOUCH_INTERVAL_SECONDS:
            return

        session.last_seen_at = now
        if client_ip:
            session.ip_address = client_ip
        if user_agent:
            session.user_agent = user_agent[:1000]

    @staticmethod
    def revoke_session(
        db: Session,
        *,
        session: UserSession,
        reason: str,
        audit_user_id: str | None = None,
        ip_address: str | None = None,
        user_agent: str | None = None,
    ) -> None:
        if session.revoked_at is not None:
            return

        now = datetime.now(timezone.utc)
        session.revoked_at = now
        session.revoke_reason = reason[:100]
        session.updated_at = now

        if reason == "refresh_token_reused":
            AuditService.log_event(
                db=db,
                event_type=AuditEventType.SUSPICIOUS_ACTIVITY,
                description="Detected refresh token reuse; session revoked",
                user_id=audit_user_id or session.user_id,
                ip_address=ip_address,
                user_agent=user_agent,
                metadata={
                    "session_id": session.id,
                    "client_device_id": session.client_device_id,
                    "reason": reason,
                },
            )

    @staticmethod
    def revoke_all_user_sessions(
        db: Session,
        *,
        user_id: str,
        reason: str,
        exclude_session_id: str | None = None,
    ) -> int:
        now = SessionService._utcnow()
        SessionService.cleanup_user_sessions(db, user_id=user_id)
        sessions = SessionService._active_sessions_query(db, user_id=user_id).all()
        count = 0
        for session in sessions:
            if exclude_session_id and session.id == exclude_session_id:
                continue
            session.revoked_at = now
            session.revoke_reason = reason[:100]
            session.updated_at = now
            count += 1
        return count

    @staticmethod
    def list_user_sessions(db: Session, *, user_id: str) -> list[UserSession]:
        SessionService.cleanup_user_sessions(db, user_id=user_id)
        return (
            SessionService._active_sessions_query(db, user_id=user_id)
            .order_by(UserSession.last_seen_at.desc(), UserSession.created_at.desc())
            .all()
        )

    @staticmethod
    def update_session_label(session: UserSession, device_label: str) -> None:
        normalized = device_label.strip()
        if not normalized:
            raise ValueError("设备名称不能为空")
        session.device_label = normalized[:120]
        session.updated_at = datetime.now(timezone.utc)

    @staticmethod
    def get_session_for_user(
        db: Session,
        *,
        user_id: str,
        session_id: str,
        lock_for_update: bool = False,
    ) -> UserSession | None:
        query = db.query(UserSession).filter(
            UserSession.user_id == user_id,
            UserSession.id == session_id,
        )
        if lock_for_update:
            # 串行化 refresh token 轮换，避免并发刷新把合法会话误判为重放攻击。
            query = query.with_for_update()
        return query.first()

    @staticmethod
    def validate_refresh_session(
        db: Session,
        *,
        user_id: str,
        session_id: str,
        refresh_token: str,
        ip_address: str | None = None,
        user_agent: str | None = None,
    ) -> tuple[UserSession | None, bool]:
        session = SessionService.get_session_for_user(
            db,
            user_id=user_id,
            session_id=session_id,
            lock_for_update=True,
        )
        if not session or session.is_revoked or session.is_expired:
            return None, False
        is_valid, is_prev = session.verify_refresh_token(refresh_token)
        if not is_valid:
            logger.warning("Refresh token mismatch for session {}", session_id)
            SessionService.revoke_session(
                db,
                session=session,
                reason="refresh_token_reused",
                audit_user_id=user_id,
                ip_address=ip_address,
                user_agent=user_agent,
            )
            db.flush()
            return None, False
        if is_prev:
            logger.info("Grace window hit for session {} (prev token used)", session_id)
        return session, is_prev
