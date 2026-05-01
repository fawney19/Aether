"""用户分组服务。"""

from __future__ import annotations

from datetime import datetime, timezone
from typing import Any

from sqlalchemy import func
from sqlalchemy.orm import Session, selectinload

from src.models.database import User, UserGroup, UserGroupModelGroup
from src.services.model.group_service import ModelGroupBindingPayload, ModelGroupService
from src.services.scheduling.scheduling_config import SchedulingConfig
from src.utils.transaction_manager import retry_on_database_error, transactional

DEFAULT_USER_GROUP_NAME = "默认分组"
DEFAULT_USER_GROUP_DESCRIPTION = "系统默认分组，所有未显式指定分组的用户都会归入此组。"


class UserGroupService:
    """用户分组服务。"""

    @staticmethod
    def get_group(db: Session, group_id: str) -> UserGroup | None:
        return (
            db.query(UserGroup)
            .options(
                selectinload(UserGroup.model_group_links).selectinload(UserGroupModelGroup.model_group)
            )
            .filter(UserGroup.id == group_id)
            .first()
        )

    @staticmethod
    def get_group_by_name(db: Session, name: str) -> UserGroup | None:
        return (
            db.query(UserGroup)
            .options(
                selectinload(UserGroup.model_group_links).selectinload(UserGroupModelGroup.model_group)
            )
            .filter(UserGroup.name == name)
            .first()
        )

    @staticmethod
    def get_default_group(db: Session) -> UserGroup | None:
        return (
            db.query(UserGroup)
            .options(
                selectinload(UserGroup.model_group_links).selectinload(UserGroupModelGroup.model_group)
            )
            .filter(UserGroup.is_default.is_(True))
            .order_by(UserGroup.created_at.asc(), UserGroup.id.asc())
            .first()
        )

    @staticmethod
    def get_required_default_group(db: Session) -> UserGroup:
        group = UserGroupService.get_default_group(db)
        if group is not None:
            return group

        group = UserGroupService.get_group_by_name(db, DEFAULT_USER_GROUP_NAME)
        if group is not None:
            return group

        raise ValueError("默认用户分组不存在，请先初始化系统默认用户分组")

    @staticmethod
    def list_groups(db: Session) -> list[tuple[UserGroup, int]]:
        user_count = func.count(User.id).label("user_count")
        return (
            db.query(UserGroup, user_count)
            .outerjoin(User, User.group_id == UserGroup.id)
            .group_by(UserGroup.id)
            .order_by(UserGroup.is_default.desc(), UserGroup.name.asc())
            .all()
        )

    @staticmethod
    @transactional()
    @retry_on_database_error(max_retries=3)
    def create_group(
        db: Session,
        *,
        name: str,
        description: str | None = None,
        allowed_api_formats: list[str] | None = None,
        rate_limit: int | None = None,
        scheduling_mode: str = SchedulingConfig.SCHEDULING_MODE_CACHE_AFFINITY,
        model_group_bindings: list[ModelGroupBindingPayload] | None = None,
    ) -> UserGroup:
        if UserGroupService.get_group_by_name(db, name):
            raise ValueError(f"用户分组已存在: {name}")

        group = UserGroup(
            name=name,
            description=description,
            is_default=False,
            allowed_api_formats=allowed_api_formats,
            rate_limit=rate_limit,
            scheduling_mode=SchedulingConfig.normalize_scheduling_mode(scheduling_mode),
        )
        db.add(group)
        db.flush()

        if model_group_bindings:
            ModelGroupService.replace_user_group_bindings(
                db,
                group.id,
                model_group_bindings,
                commit=False,
            )
        else:
            ModelGroupService.ensure_user_group_default_binding(db, group, commit=False)

        db.commit()
        created = UserGroupService.get_group(db, str(group.id))
        if created is None:
            raise ValueError("用户分组创建后读取失败")
        return created

    @staticmethod
    @transactional()
    @retry_on_database_error(max_retries=3)
    def update_group(db: Session, group_id: str, **kwargs: Any) -> UserGroup | None:
        group = UserGroupService.get_group(db, group_id)
        if not group:
            return None

        if "name" in kwargs and kwargs["name"] is not None:
            existing = UserGroupService.get_group_by_name(db, kwargs["name"])
            if existing and existing.id != group_id:
                raise ValueError(f"用户分组已存在: {kwargs['name']}")

        updatable_fields = [
            "name",
            "description",
            "allowed_api_formats",
            "rate_limit",
            "scheduling_mode",
        ]
        nullable_fields = ["description", "allowed_api_formats", "rate_limit"]

        for field, value in kwargs.items():
            if field not in updatable_fields:
                continue
            if field == "scheduling_mode" and value is not None:
                setattr(group, field, SchedulingConfig.normalize_scheduling_mode(value))
                continue
            if field in nullable_fields:
                setattr(group, field, value)
            elif value is not None:
                setattr(group, field, value)

        bindings = kwargs.get("model_group_bindings")
        if bindings is not None:
            ModelGroupService.replace_user_group_bindings(
                db,
                group.id,
                bindings,
                commit=False,
            )

        group.updated_at = datetime.now(timezone.utc)
        db.commit()
        updated = UserGroupService.get_group(db, str(group.id))
        if updated is None:
            raise ValueError("用户分组更新后读取失败")
        return updated

    @staticmethod
    @transactional()
    @retry_on_database_error(max_retries=3)
    def delete_group(db: Session, group_id: str) -> bool:
        group = UserGroupService.get_group(db, group_id)
        if not group:
            return False
        if group.is_default:
            raise ValueError("默认分组不能删除")

        assigned_users = (
            db.query(func.count(User.id)).filter(User.group_id == group_id).scalar() or 0
        )
        if int(assigned_users) > 0:
            raise ValueError("该分组仍有关联用户，请先移除分组成员")

        # 订阅产品/计划通过外键 RESTRICT 引用分组，必须前置检查以避免数据库层面的
        # IntegrityError 上浮为 500。延迟 import 避免与订阅服务循环依赖。
        from src.models.database import SubscriptionPlan, SubscriptionProduct

        plan_count = int(
            db.query(func.count(SubscriptionPlan.id))
            .filter(SubscriptionPlan.user_group_id == group_id)
            .scalar()
            or 0
        )
        if plan_count > 0:
            raise ValueError(
                f"该分组仍被 {plan_count} 个订阅计划引用，请先删除或改绑相关订阅计划"
            )

        product_count = int(
            db.query(func.count(SubscriptionProduct.id))
            .filter(SubscriptionProduct.user_group_id == group_id)
            .scalar()
            or 0
        )
        if product_count > 0:
            raise ValueError(
                f"该分组仍被 {product_count} 个订阅产品引用，请先删除或改绑相关订阅产品"
            )

        db.delete(group)
        db.commit()
        return True

    @staticmethod
    def resolve_effective_scheduling_mode(db: Session, user: User | None) -> str:
        if user is None:
            return SchedulingConfig.SCHEDULING_MODE_CACHE_AFFINITY

        from src.services.subscription import SubscriptionService

        effective_group = SubscriptionService.resolve_effective_user_group(db, user)
        group_mode = getattr(effective_group, "scheduling_mode", None) if effective_group else None
        return SchedulingConfig.normalize_scheduling_mode(group_mode)
