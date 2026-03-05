"""
用户缓存服务 - 减少数据库查询

架构说明
========
本服务采用混合 async/sync 模式：
- 缓存操作（CacheService）：真正的 async，使用 aioredis
- 数据库查询（db.query）：同步的 SQLAlchemy Session

设计决策
--------
1. 保持 async 方法签名：因为缓存命中时完全异步，性能最优
2. 缓存未命中时的同步查询：FastAPI 会在线程池中执行，不会阻塞事件循环
3. 调用方必须在 async 上下文中使用 await

使用示例
--------
    user = await UserCacheService.get_user_by_id(db, user_id)
    await UserCacheService.invalidate_user_cache(user_id, email)
"""

from __future__ import annotations

from typing import Any

from sqlalchemy.orm import Session

from src.config.constants import CacheTTL
from src.core.cache_service import CacheKeys, CacheService
from src.core.logger import logger
from src.models.database import User


class UserCacheService:
    """用户缓存服务

    提供 User 的缓存查询功能，减少数据库访问。
    所有公开方法均为 async，需要在 async 上下文中调用。
    """

    # 缓存 TTL（秒）- 使用统一常量
    CACHE_TTL = CacheTTL.USER

    @staticmethod
    async def get_user_by_id(db: Session, user_id: str) -> User | None:
        """
        获取用户（带缓存）

        Args:
            db: 数据库会话
            user_id: 用户ID

        Returns:
            User 对象或 None
        """
        cache_key = CacheKeys.user_by_id(user_id)

        # 1. 尝试从缓存获取
        cached_data = await CacheService.get(cache_key)
        if cached_data:
            logger.debug(f"用户缓存命中: {user_id}")
            # 从缓存数据重建 User 对象
            return UserCacheService._dict_to_user(db, cached_data)

        # 2. 缓存未命中，查询数据库
        user = db.query(User).filter(User.id == user_id).first()

        # 3. 写入缓存
        if user:
            user_dict = UserCacheService._user_to_dict(user)
            await CacheService.set(cache_key, user_dict, ttl_seconds=UserCacheService.CACHE_TTL)
            logger.debug(f"用户已缓存: {user_id}")

        return user

    @staticmethod
    async def get_user_by_email(db: Session, email: str) -> User | None:
        """
        通过邮箱获取用户（带缓存）

        Args:
            db: 数据库会话
            email: 用户邮箱

        Returns:
            User 对象或 None
        """
        cache_key = CacheKeys.user_by_email(email)

        # 1. 尝试从缓存获取
        cached_data = await CacheService.get(cache_key)
        if cached_data:
            logger.debug(f"用户缓存命中(邮箱): {email}")
            return UserCacheService._dict_to_user(db, cached_data)

        # 2. 缓存未命中，查询数据库
        user = db.query(User).filter(User.email == email).first()

        # 3. 写入缓存
        if user:
            user_dict = UserCacheService._user_to_dict(user)
            await CacheService.set(cache_key, user_dict, ttl_seconds=UserCacheService.CACHE_TTL)
            logger.debug(f"用户已缓存(邮箱): {email}")

        return user

    @staticmethod
    async def invalidate_user_cache(user_id: str, email: str | None = None) -> Any:
        """
        清除用户缓存

        Args:
            user_id: 用户ID
            email: 用户邮箱（可选）
        """
        # 删除 ID 缓存
        await CacheService.delete(CacheKeys.user_by_id(user_id))

        # 删除邮箱缓存
        if email:
            await CacheService.delete(CacheKeys.user_by_email(email))

        logger.debug(f"用户缓存已清除: {user_id}")

    @staticmethod
    def _user_to_dict(user: User) -> dict:
        """将 User 对象转换为字典（用于缓存）"""
        return {
            "id": user.id,
            "email": user.email,
            "email_verified": user.email_verified,
            "username": user.username,
            "role": user.role.value if user.role else None,
            "is_active": user.is_active,
            "auth_source": user.auth_source.value if user.auth_source else None,
            "created_at": user.created_at.isoformat() if user.created_at else None,
            "last_login_at": user.last_login_at.isoformat() if user.last_login_at else None,
            "model_capability_settings": user.model_capability_settings,
        }

    @staticmethod
    def _dict_to_user(db: Session, user_dict: dict) -> User:
        """
        从字典重建 User 对象

        注意：这是一个"分离"的对象，不在 Session 中
        如果需要修改，需要使用 db.merge() 或重新查询
        """
        from datetime import datetime

        from src.core.enums import AuthSource
        from src.models.database import UserRole

        user = User(
            id=user_dict["id"],
            email=user_dict.get("email"),
            email_verified=user_dict.get("email_verified", False),
            username=user_dict["username"],
            is_active=user_dict["is_active"],
        )

        # 设置可选字段
        if user_dict.get("role"):
            user.role = UserRole(user_dict["role"])

        if user_dict.get("auth_source"):
            user.auth_source = AuthSource(user_dict["auth_source"])

        if user_dict.get("created_at"):
            user.created_at = datetime.fromisoformat(user_dict["created_at"])

        if user_dict.get("last_login_at"):
            user.last_login_at = datetime.fromisoformat(user_dict["last_login_at"])

        if user_dict.get("model_capability_settings") is not None:
            user.model_capability_settings = user_dict["model_capability_settings"]

        return user
