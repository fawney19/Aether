"""
用户管理服务
"""

from __future__ import annotations

from datetime import datetime, timezone
from typing import Any

from sqlalchemy import and_, func, or_
from sqlalchemy.orm import Session, contains_eager

from src.core.logger import logger
from src.core.validators import EmailValidator, PasswordValidator, UsernameValidator
from src.models.database import ApiKey, GlobalModel, Model, Provider, Usage, User, UserRole
from src.services.cache.user_cache import UserCacheService
from src.services.system.config import SystemConfigService
from src.services.user.bulk_cleanup import batch_nullify_fk, pre_clean_api_key
from src.utils.async_utils import safe_create_task
from src.utils.transaction_manager import retry_on_database_error, transactional


class UserService:
    """用户管理服务"""

    @staticmethod
    @transactional()
    @retry_on_database_error(max_retries=3)
    def create_user(
        db: Session,
        email: str | None,
        username: str,
        password: str,
        role: UserRole = UserRole.USER,
        initial_gift_usd: float | None = 10.0,
        unlimited: bool = False,
        email_verified: bool = False,
        allowed_providers: list[str] | None = None,
        allowed_api_formats: list[str] | None = None,
        allowed_models: list[str] | None = None,
        rate_limit: int | None = None,
    ) -> User:
        """创建新用户。"""

        # 验证邮箱格式（仅当提供邮箱时）
        if email is not None:
            valid, error_msg = EmailValidator.validate(email)
            if not valid:
                raise ValueError(error_msg)
            # 检查邮箱是否已存在
            if db.query(User).filter(User.email == email).first():
                raise ValueError(f"邮箱已存在: {email}")

        # 验证用户名格式
        valid, error_msg = UsernameValidator.validate(username)
        if not valid:
            raise ValueError(error_msg)

        # 验证密码复杂度
        policy_level = SystemConfigService.get_password_policy_level(db)
        valid, error_msg = PasswordValidator.validate(password, policy=policy_level)
        if not valid:
            raise ValueError(error_msg)

        # 检查用户名是否已存在
        if db.query(User).filter(User.username == username).first():
            raise ValueError(f"用户名已存在: {username}")

        user = User(
            email=email,
            email_verified=email_verified if email else False,
            username=username,
            role=role,
            is_active=True,
            allowed_providers=allowed_providers,
            allowed_api_formats=allowed_api_formats,
            allowed_models=allowed_models,
            rate_limit=rate_limit,
        )
        user.set_password(password)

        db.add(user)
        db.flush()

        from src.services.wallet import WalletService

        WalletService.initialize_user_wallet(
            db,
            user=user,
            initial_gift_usd=initial_gift_usd,
            unlimited=unlimited,
            description="用户初始赠款",
        )

        db.commit()  # 立即提交事务,释放数据库锁
        db.refresh(user)

        log_identifier = email if email else username
        logger.info(f"创建新用户: {log_identifier} (ID: {user.id}, 角色: {role.value})")
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
        initial_gift_usd: float | None = 10.0,
        unlimited: bool = False,
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
            initial_gift_usd: 初始赠款（USD）
            unlimited: 是否无限制
            concurrent_limit: 并发限制

        Returns:
            tuple[User, ApiKey]: 用户对象和API密钥对象

        Raises:
            ValueError: 当验证失败时
        """
        # 创建用户
        user = UserService.create_user(
            db=db,
            email=email,
            username=username,
            password=password,
            role=role,
            initial_gift_usd=initial_gift_usd,
            unlimited=unlimited,
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
    def get_user(db: Session, user_id: str) -> User | None:
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
    def get_user_by_email(db: Session, email: str) -> User | None:
        """通过邮箱获取用户"""
        return db.query(User).filter(User.email == email).first()

    @staticmethod
    def list_users(
        db: Session,
        skip: int = 0,
        limit: int = 100,
        role: UserRole | None = None,
        is_active: bool | None = None,
    ) -> list[User]:
        """列出用户"""
        query = db.query(User)

        if role:
            query = query.filter(User.role == role)
        if is_active is not None:
            query = query.filter(User.is_active == is_active)

        return (
            query.order_by(User.created_at.desc(), User.id.desc()).offset(skip).limit(limit).all()
        )

    @staticmethod
    @transactional()
    def update_user(db: Session, user_id: str, **kwargs: Any) -> User | None:
        """更新用户信息"""
        user = db.query(User).filter(User.id == user_id).first()
        if not user:
            return None

        # 可更新的字段
        updatable_fields = [
            "email",
            "username",
            "is_active",
            "role",
            # 访问限制字段
            "allowed_providers",
            "allowed_api_formats",
            "allowed_models",
            "rate_limit",
        ]

        # 允许设置为 None 的字段（表示无限制）
        nullable_fields = [
            "allowed_providers",
            "allowed_api_formats",
            "allowed_models",
            "rate_limit",
        ]

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
            policy_level = SystemConfigService.get_password_policy_level(db)
            valid, error_msg = PasswordValidator.validate(kwargs["password"], policy=policy_level)
            if not valid:
                raise ValueError(error_msg)
            user.set_password(kwargs["password"])

        user.updated_at = datetime.now(timezone.utc)
        db.commit()  # 立即提交事务,释放数据库锁
        db.refresh(user)

        # 清除用户缓存
        safe_create_task(UserCacheService.invalidate_user_cache(user.id, user.email))

        logger.debug(f"更新用户信息: {user.email} (ID: {user_id})")
        return user

    @staticmethod
    def delete_user(db: Session, user_id: str) -> bool:
        """删除用户（硬删除）

        删除流程：
        1. 检查未完结账务，阻止删除
        2. 预清理 Usage / RequestCandidate 等大表外键
        3. 手动删除 ORM cascade 冲突的子记录
        4. 删除用户记录
        5. 财务记录（Wallet/PaymentOrder/RefundRequest/WalletTransaction）和
           Usage 记录保留，外键 SET NULL，由自动清理策略统一回收
        """
        from src.models.database import (
            AnnouncementRead,
            ApiKey,
            PaymentOrder,
            RefundRequest,
            RequestCandidate,
            UserPreference,
            Wallet,
        )

        user = db.query(User).filter(User.id == user_id).first()
        if not user:
            return False

        # 记录删除信息用于日志
        email = user.email

        # 删除前阻断未完结账务，避免删除导致资金状态不一致。
        wallet_ids = [
            wallet_id
            for (wallet_id,) in (
                db.query(Wallet.id)
                .outerjoin(ApiKey, Wallet.api_key_id == ApiKey.id)
                .filter(or_(Wallet.user_id == user_id, ApiKey.user_id == user_id))
                .all()
            )
        ]
        if wallet_ids:
            pending_refund_count = (
                db.query(RefundRequest)
                .filter(
                    RefundRequest.wallet_id.in_(wallet_ids),
                    RefundRequest.status.in_(["pending_approval", "approved", "processing"]),
                )
                .count()
            )
            if pending_refund_count > 0:
                raise ValueError("用户存在未完结退款，禁止删除")

            pending_order_count = (
                db.query(PaymentOrder)
                .filter(
                    PaymentOrder.wallet_id.in_(wallet_ids),
                    PaymentOrder.status.in_(["pending", "paid"]),
                )
                .count()
            )
            if pending_order_count > 0:
                raise ValueError("用户存在未完结充值订单，禁止删除")

        api_key_ids = [
            api_key_id
            for (api_key_id,) in db.query(ApiKey.id).filter(ApiKey.user_id == user_id).all()
        ]
        api_key_count = len(api_key_ids)

        # 注意：batch_nullify_fk 内部分批 commit，预清理部分不可回滚。
        # 这是预期行为：SET NULL 是幂等操作，即使后续步骤失败，
        # 已置空的外键不影响数据完整性，重新执行删除即可。
        try:
            for api_key_id in api_key_ids:
                pre_clean_api_key(db, api_key_id)

            batch_nullify_fk(db, Usage, "user_id", user_id)
            batch_nullify_fk(db, RequestCandidate, "user_id", user_id)

            db.query(UserPreference).filter(UserPreference.user_id == user_id).delete(
                synchronize_session=False
            )
            db.query(AnnouncementRead).filter(AnnouncementRead.user_id == user_id).delete(
                synchronize_session=False
            )

            # 财务记录（Wallet/WalletTransaction/PaymentOrder/RefundRequest/PaymentCallback）
            # 和 Usage / RequestCandidate / VideoTask 记录全部保留，数据库外键 SET NULL 自动断开关联。
            db.query(ApiKey).filter(ApiKey.user_id == user_id).delete(synchronize_session=False)

            # 现在删除用户（Usage, AuditLog, RequestAttempt 等会通过数据库 SET NULL 保留）
            db.delete(user)
            db.commit()  # 立即提交事务,释放数据库锁
        except Exception:
            db.rollback()
            raise

        # 清除用户缓存
        safe_create_task(UserCacheService.invalidate_user_cache(user_id, email))

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
        policy_level = SystemConfigService.get_password_policy_level(db)
        valid, error_msg = PasswordValidator.validate(new_password, policy=policy_level)
        if not valid:
            return False, error_msg

        # 检查新密码不能与旧密码相同
        if old_password == new_password:
            return False, "新密码不能与旧密码相同"

        # 设置新密码
        user.set_password(new_password)
        user.updated_at = datetime.now(timezone.utc)

        # 清除用户缓存
        safe_create_task(UserCacheService.invalidate_user_cache(user.id, user.email))

        logger.info(f"密码更改成功: 用户ID {user_id}")
        return True, "密码更改成功"

    @staticmethod
    def get_user_usage_stats(
        db: Session,
        user_id: str,
        start_date: datetime | None = None,
        end_date: datetime | None = None,
    ) -> dict[str, Any]:
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
    def get_user_available_models(db: Session, user: User) -> list[Model]:
        """获取用户可用的模型

        通过 GlobalModel + Model 关联查询用户可用模型
        逻辑：使用 AccessRestrictions 统一处理 allowed_providers 和 allowed_models 限制
        """
        from src.core.access_restrictions import AccessRestrictions

        # 使用 AccessRestrictions 类来处理限制（与 /v1/models 逻辑一致）
        restrictions = AccessRestrictions.from_api_key_and_user(api_key=None, user=user)

        # 获取所有活跃的 Provider ID
        all_active_provider_ids = [
            p.id for p in db.query(Provider.id).filter(Provider.is_active == True).all()
        ]

        if not all_active_provider_ids:
            return []

        # 查询所有活跃的 Model（关联 GlobalModel，contains_eager 避免循环中懒加载）
        all_models = (
            db.query(Model)
            .join(GlobalModel, Model.global_model_id == GlobalModel.id)
            .options(contains_eager(Model.global_model))
            .filter(
                and_(
                    Model.provider_id.in_(all_active_provider_ids),
                    Model.is_active == True,
                    GlobalModel.is_active == True,
                )
            )
            .all()
        )

        # 应用访问限制过滤
        filtered_models = []
        for model in all_models:
            model_name = (
                model.global_model.name if model.global_model else model.provider_model_name
            )
            # 使用 AccessRestrictions.is_model_allowed 检查模型是否可访问
            if restrictions.is_model_allowed(model_name, model.provider_id):
                filtered_models.append(model)

        logger.debug(f"用户 {user.email} 可用模型: {len(filtered_models)} 个")

        return filtered_models
