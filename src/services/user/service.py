"""
用户管理服务
"""

import asyncio
from datetime import datetime, timedelta, timezone
from typing import Any, Dict, List, Optional

from sqlalchemy import and_, func
from sqlalchemy.orm import Session

from src.core.logger import logger
from src.core.validators import EmailValidator, PasswordValidator, UsernameValidator
from src.models.database import ApiKey, GlobalModel, Model, Provider, Usage, User, UserRole
from src.services.cache.user_cache import UserCacheService
from src.utils.transaction_manager import retry_on_database_error, transactional



class UserService:
    """用户管理服务"""

    @staticmethod
    @transactional()
    @retry_on_database_error(max_retries=3)
    def create_user(
        db: Session,
        email: str,
        username: str,
        password: str,
        role: UserRole = UserRole.USER,
        quota_usd: Optional[float] = 10.0,
    ) -> User:
        """创建新用户，quota_usd 为 None 表示无限制"""

        # 验证邮箱格式
        valid, error_msg = EmailValidator.validate(email)
        if not valid:
            raise ValueError(error_msg)

        # 验证用户名格式
        valid, error_msg = UsernameValidator.validate(username)
        if not valid:
            raise ValueError(error_msg)

        # 验证密码复杂度
        valid, error_msg = PasswordValidator.validate(password)
        if not valid:
            raise ValueError(error_msg)

        # 检查邮箱是否已存在
        if db.query(User).filter(User.email == email).first():
            raise ValueError(f"邮箱已存在: {email}")

        # 检查用户名是否已存在
        if db.query(User).filter(User.username == username).first():
            raise ValueError(f"用户名已存在: {username}")

        user = User(
            email=email,
            username=username,
            role=role,
            quota_usd=quota_usd,
            is_active=True,
        )
        user.set_password(password)

        db.add(user)
        db.commit()  # 立即提交事务,释放数据库锁
        db.refresh(user)

        logger.info(f"创建新用户: {email} (ID: {user.id}, 角色: {role.value})")
        return user

    @staticmethod
    @transactional()
    def create_user_with_api_key(
        db: Session,
        email: str,
        username: str,
        password: str,
        api_key_name: str = "默认密钥",
        role: UserRole = UserRole.USER,
        quota_usd: Optional[float] = 10.0,
        concurrent_limit: int = 5,
    ) -> tuple[User, ApiKey]:
        """
        创建用户并同时创建API密钥（原子操作）

        Args:
            db: 数据库会话
            email: 邮箱
            username: 用户名
            password: 密码
            api_key_name: API密钥名称
            role: 用户角色
            quota_usd: USD配额，None 表示无限制
            concurrent_limit: 并发限制

        Returns:
            tuple[User, ApiKey]: 用户对象和API密钥对象

        Raises:
            ValueError: 当验证失败时
        """
        # 创建用户
        user = UserService.create_user(
            db=db, email=email, username=username, password=password, role=role, quota_usd=quota_usd
        )

        # 导入API密钥服务（避免循环导入）
        from .apikey import ApiKeyService

        # 创建API密钥（返回值是 (api_key, plain_key)）
        api_key, plain_key = ApiKeyService.create_api_key(
            db=db, user_id=user.id, name=api_key_name, concurrent_limit=concurrent_limit
        )

        logger.info(f"创建用户和API密钥完成: {email} (用户ID: {user.id}, 密钥ID: {api_key.id})")

        # 返回用户对象、API Key对象和明文密钥
        return user, api_key, plain_key

    @staticmethod
    def get_user(db: Session, user_id: str) -> Optional[User]:
        """获取用户"""
        import random
        import time

        # 添加重试机制处理数据库并发问题
        max_retries = 3
        for attempt in range(max_retries):
            try:
                user = db.query(User).filter(User.id == user_id).first()
                return user
            except Exception as e:
                if attempt < max_retries - 1:
                    # 添加随机延迟避免并发冲突
                    time.sleep(random.uniform(0.01, 0.05))
                    db.rollback()  # 回滚事务
                    continue
                else:
                    raise e

    @staticmethod
    def get_user_by_email(db: Session, email: str) -> Optional[User]:
        """通过邮箱获取用户"""
        return db.query(User).filter(User.email == email).first()

    @staticmethod
    def list_users(
        db: Session,
        skip: int = 0,
        limit: int = 100,
        role: Optional[UserRole] = None,
        is_active: Optional[bool] = None,
    ) -> List[User]:
        """列出用户"""
        query = db.query(User)

        if role:
            query = query.filter(User.role == role)
        if is_active is not None:
            query = query.filter(User.is_active == is_active)

        return query.offset(skip).limit(limit).all()

    @staticmethod
    @transactional()
    def update_user(db: Session, user_id: str, **kwargs) -> Optional[User]:
        """更新用户信息"""
        user = db.query(User).filter(User.id == user_id).first()
        if not user:
            return None

        # 可更新的字段
        updatable_fields = [
            "email",
            "username",
            "quota_usd",
            "is_active",
            "role",
            # 访问限制字段
            "allowed_providers",
            "allowed_api_formats",
            "allowed_models",
        ]

        # 允许设置为 None 的字段（表示无限制）
        nullable_fields = ["quota_usd", "allowed_providers", "allowed_api_formats", "allowed_models"]

        for field, value in kwargs.items():
            if field not in updatable_fields:
                continue
            # nullable_fields 中的字段允许设置为 None
            if field in nullable_fields:
                setattr(user, field, value)
            elif value is not None:
                setattr(user, field, value)

        # 如果提供了新密码
        if "password" in kwargs and kwargs["password"]:
            # 验证新密码复杂度
            valid, error_msg = PasswordValidator.validate(kwargs["password"])
            if not valid:
                raise ValueError(error_msg)
            user.set_password(kwargs["password"])

        user.updated_at = datetime.now(timezone.utc)
        db.commit()  # 立即提交事务,释放数据库锁
        db.refresh(user)

        # 清除用户缓存
        asyncio.create_task(UserCacheService.invalidate_user_cache(user.id, user.email))

        logger.debug(f"更新用户信息: {user.email} (ID: {user_id})")
        return user

    @staticmethod
    @transactional()
    def delete_user(db: Session, user_id: str) -> bool:
        """删除用户（硬删除）

        删除流程：
        1. 手动删除关联的子记录（避免 SQLAlchemy ORM 与数据库 CASCADE 冲突）
        2. 删除用户记录
        3. 历史 Usage 记录保留，user_id 会被数据库设为 NULL
        4. 新用户注册时会有新的 UUID，看不到旧用户的记录
        """
        from src.models.database import (
            AnnouncementRead,
            ApiKey,
            UserPreference,
            UserQuota,
        )

        user = db.query(User).filter(User.id == user_id).first()
        if not user:
            return False

        # 记录删除信息用于日志
        email = user.email

        # 手动删除子记录，避免 SQLAlchemy 的 ORM cascade 与数据库 CASCADE 冲突
        # 这些表的数据库外键已经设置了 ON DELETE CASCADE，但 SQLAlchemy 会先尝试 UPDATE 设置为 NULL
        # 所以我们手动删除来避免这个问题
        db.query(UserPreference).filter(UserPreference.user_id == user_id).delete(
            synchronize_session=False
        )
        db.query(UserQuota).filter(UserQuota.user_id == user_id).delete(synchronize_session=False)
        db.query(AnnouncementRead).filter(AnnouncementRead.user_id == user_id).delete(
            synchronize_session=False
        )
        api_key_count = db.query(ApiKey).filter(ApiKey.user_id == user_id).count()
        db.query(ApiKey).filter(ApiKey.user_id == user_id).delete(synchronize_session=False)

        # 现在删除用户（Usage, AuditLog, RequestAttempt 会通过数据库 SET NULL 保留）
        db.delete(user)
        db.commit()  # 立即提交事务,释放数据库锁

        # 清除用户缓存
        asyncio.create_task(UserCacheService.invalidate_user_cache(user_id, email))

        logger.info(f"删除用户: {email} (ID: {user_id}), 同时删除 {api_key_count} 个API密钥")
        return True

    @staticmethod
    @transactional()
    def change_password(
        db: Session, user_id: str, old_password: str, new_password: str
    ) -> tuple[bool, str]:
        """
        更改用户密码

        Args:
            db: 数据库会话
            user_id: 用户ID
            old_password: 旧密码
            new_password: 新密码

        Returns:
            (是否成功, 消息)
        """
        user = db.query(User).filter(User.id == user_id).first()
        if not user:
            return False, "用户不存在"

        # 验证旧密码
        if not user.verify_password(old_password):
            logger.warning(f"密码更改失败 - 旧密码错误: 用户ID {user_id}")
            return False, "旧密码错误"

        # 验证新密码复杂度
        valid, error_msg = PasswordValidator.validate(new_password)
        if not valid:
            return False, error_msg

        # 检查新密码不能与旧密码相同
        if old_password == new_password:
            return False, "新密码不能与旧密码相同"

        # 设置新密码
        user.set_password(new_password)
        user.updated_at = datetime.now(timezone.utc)

        # 清除用户缓存
        asyncio.create_task(UserCacheService.invalidate_user_cache(user.id, user.email))

        logger.info(f"密码更改成功: 用户ID {user_id}")
        return True, "密码更改成功"

    @staticmethod
    def update_user_quota(
        db: Session,
        user_id: str,
        quota_usd: Optional[float] = None,
    ) -> Optional[User]:
        """更新用户配额"""
        user = db.query(User).filter(User.id == user_id).first()
        if not user:
            return None

        if quota_usd is not None:
            user.quota_usd = quota_usd

        db.commit()
        db.refresh(user)

        # 清除用户缓存
        asyncio.create_task(UserCacheService.invalidate_user_cache(user.id, user.email))

        logger.debug(f"更新用户配额: {user.email} (USD: {quota_usd})")
        return user

    @staticmethod
    def get_user_usage_stats(
        db: Session,
        user_id: str,
        start_date: Optional[datetime] = None,
        end_date: Optional[datetime] = None,
    ) -> Dict[str, Any]:
        """获取用户使用统计"""

        query = db.query(Usage).filter(Usage.user_id == user_id)

        if start_date:
            query = query.filter(Usage.created_at >= start_date)
        if end_date:
            query = query.filter(Usage.created_at <= end_date)

        # 统计数据
        stats = db.query(
            func.count(Usage.id).label("total_requests"),
            func.sum(Usage.total_tokens).label("total_tokens"),
            func.sum(Usage.total_cost_usd).label("total_cost_usd"),
            func.avg(Usage.response_time_ms).label("avg_response_time"),
        ).filter(Usage.user_id == user_id)

        if start_date:
            stats = stats.filter(Usage.created_at >= start_date)
        if end_date:
            stats = stats.filter(Usage.created_at <= end_date)

        result = stats.first()

        # 按模型分组统计
        model_stats = db.query(
            Usage.model,
            func.count(Usage.id).label("requests"),
            func.sum(Usage.total_tokens).label("tokens"),
            func.sum(Usage.total_cost_usd).label("cost_usd"),
        ).filter(Usage.user_id == user_id)

        if start_date:
            model_stats = model_stats.filter(Usage.created_at >= start_date)
        if end_date:
            model_stats = model_stats.filter(Usage.created_at <= end_date)

        model_stats = model_stats.group_by(Usage.model).all()

        return {
            "total_requests": result.total_requests or 0,
            "total_tokens": result.total_tokens or 0,
            "total_cost_usd": float(result.total_cost_usd or 0),
            "avg_response_time_ms": float(result.avg_response_time or 0),
            "by_model": [
                {
                    "model": stat.model,
                    "requests": stat.requests,
                    "tokens": stat.tokens,
                    "cost_usd": float(stat.cost_usd),
                }
                for stat in model_stats
            ],
        }

    @staticmethod
    def get_user_available_models(db: Session, user: User) -> List[Model]:
        """获取用户可用的模型

        通过 GlobalModel + Model 关联查询用户可用模型
        逻辑：用户可用提供商 -> Provider 的 Model 实现 -> 关联的 GlobalModel
        """
        # 获取用户可用的提供商
        if user.role == UserRole.ADMIN:
            # 管理员可以使用所有活动提供商
            provider_ids = [
                p.id for p in db.query(Provider.id).filter(Provider.is_active == True).all()
            ]
        else:
            # 普通用户使用关联的提供商
            provider_ids = [p.id for p in user.providers]

        if not provider_ids:
            return []

        # 查询这些提供商的所有活跃 Model（关联 GlobalModel）
        models = (
            db.query(Model)
            .join(GlobalModel, Model.global_model_id == GlobalModel.id)
            .filter(
                and_(
                    Model.provider_id.in_(provider_ids),
                    Model.is_active == True,
                    GlobalModel.is_active == True,
                )
            )
            .all()
        )

        logger.debug(f"用户 {user.email} 可用模型: {len(models)} 个 (提供商数: {len(provider_ids)})")

        return models
