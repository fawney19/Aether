"""
用户偏好设置服务
"""

from __future__ import annotations

from sqlalchemy.orm import Session

from src.core.exceptions import NotFoundException
from src.core.logger import logger
from src.models.database import Provider, User, UserPreference
from src.services.wallet import WalletService


class PreferenceService:
    """用户偏好设置服务"""

    @staticmethod
    def get_or_create_preferences(db: Session, user_id: str) -> UserPreference:  # UUID
        """获取或创建用户偏好设置"""
        preferences = db.query(UserPreference).filter(UserPreference.user_id == user_id).first()

        if not preferences:
            # 创建默认偏好设置
            preferences = UserPreference(
                user_id=user_id,
                theme="light",
                language="zh-CN",
                timezone="Asia/Shanghai",
                email_notifications=True,
                usage_alerts=True,
                announcement_notifications=True,
            )
            db.add(preferences)
            db.commit()
            db.refresh(preferences)
            logger.info(f"Created default preferences for user {user_id}")

        return preferences

    @staticmethod
    def update_preferences(
        db: Session,
        user_id: str,  # UUID
        avatar_url: str | None = None,
        bio: str | None = None,
        default_provider_id: str | None = None,  # UUID
        theme: str | None = None,
        language: str | None = None,
        timezone: str | None = None,
        email_notifications: bool | None = None,
        usage_alerts: bool | None = None,
        announcement_notifications: bool | None = None,
    ) -> UserPreference:
        """更新用户偏好设置"""
        preferences = PreferenceService.get_or_create_preferences(db, user_id)

        # 更新提供的字段
        if avatar_url is not None:
            preferences.avatar_url = avatar_url
        if bio is not None:
            preferences.bio = bio
        if default_provider_id is not None:
            # 验证提供商是否存在且活跃
            provider = (
                db.query(Provider)
                .filter(Provider.id == default_provider_id, Provider.is_active == True)
                .first()
            )
            if not provider:
                raise NotFoundException("Provider not found or inactive")
            preferences.default_provider_id = default_provider_id
        if theme is not None:
            if theme not in ["light", "dark", "auto", "system"]:
                raise ValueError("Invalid theme. Must be 'light', 'dark', 'auto', or 'system'")
            preferences.theme = theme
        if language is not None:
            preferences.language = language
        if timezone is not None:
            preferences.timezone = timezone
        if email_notifications is not None:
            preferences.email_notifications = email_notifications
        if usage_alerts is not None:
            preferences.usage_alerts = usage_alerts
        if announcement_notifications is not None:
            preferences.announcement_notifications = announcement_notifications

        db.commit()
        db.refresh(preferences)
        logger.info(f"Updated preferences for user {user_id}")

        return preferences

    @staticmethod
    def get_user_with_preferences(db: Session, user_id: str) -> dict:  # UUID
        """获取用户信息及其偏好设置"""
        user = db.query(User).filter(User.id == user_id).first()
        if not user:
            raise NotFoundException("User not found")

        preferences = PreferenceService.get_or_create_preferences(db, user_id)
        wallet = WalletService.get_wallet(db, user_id=user.id)
        billing = WalletService.serialize_wallet_summary(wallet)

        # 构建返回数据
        user_data = {
            "id": user.id,
            "email": user.email,
            "username": user.username,
            "role": user.role.value,
            "is_active": user.is_active,
            "created_at": user.created_at,
            "last_login_at": user.last_login_at,
            "auth_source": user.auth_source.value if user.auth_source else "local",
            "has_password": bool(user.password_hash),
            "preferences": {
                "avatar_url": preferences.avatar_url,
                "bio": preferences.bio,
                "default_provider": (
                    preferences.default_provider.name if preferences.default_provider else None
                ),
                "theme": preferences.theme,
                "language": preferences.language,
                "timezone": preferences.timezone,
                "notifications": {
                    "email": preferences.email_notifications,
                    "usage_alerts": preferences.usage_alerts,
                    "announcements": preferences.announcement_notifications,
                },
            },
            "billing": billing,
            "stats": {
                "total_cost": billing["total_consumed"],
                "total_cost_all_time": billing["total_consumed"],
                "api_keys_count": len(user.api_keys),
            },
        }

        return user_data
