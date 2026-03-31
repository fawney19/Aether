"""模型分组服务。"""

from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime, timezone
from decimal import Decimal
from typing import Any

from sqlalchemy import and_, func
from sqlalchemy.orm import Session, selectinload

from src.models.database import (
    GlobalModel,
    ModelGroup,
    ModelGroupModel,
    ModelGroupRoute,
    Provider,
    ProviderAPIKey,
    User,
    UserGroup,
    UserGroupModelGroup,
)
from src.utils.transaction_manager import retry_on_database_error, transactional

DEFAULT_MODEL_GROUP_NAME = "default"
DEFAULT_MODEL_GROUP_DISPLAY_NAME = "默认模型分组"
DEFAULT_MODEL_GROUP_DESCRIPTION = "系统默认模型分组，供默认用户分组与新建分组按需绑定。"


@dataclass(frozen=True, slots=True)
class ModelGroupBindingPayload:
    model_group_id: str
    priority: int = 100
    is_active: bool = True


@dataclass(frozen=True, slots=True)
class ModelGroupRoutePayload:
    provider_id: str
    provider_api_key_id: str | None = None
    priority: int = 50
    user_billing_multiplier_override: float | None = None
    is_active: bool = True
    notes: str | None = None


class ModelGroupService:
    """模型分组服务。"""

    @staticmethod
    def get_group(db: Session, group_id: str) -> ModelGroup | None:
        return db.query(ModelGroup).filter(ModelGroup.id == group_id).first()

    @staticmethod
    def get_group_by_name(db: Session, name: str) -> ModelGroup | None:
        return db.query(ModelGroup).filter(ModelGroup.name == name).first()

    @staticmethod
    def get_default_group(db: Session) -> ModelGroup | None:
        return (
            db.query(ModelGroup)
            .filter(ModelGroup.is_default.is_(True))
            .order_by(ModelGroup.sort_order.asc(), ModelGroup.created_at.asc(), ModelGroup.id.asc())
            .first()
        )

    @staticmethod
    def get_or_create_default_group(db: Session, *, commit: bool = False) -> ModelGroup:
        group = ModelGroupService.get_default_group(db)
        if group is None:
            group = ModelGroupService.get_group_by_name(db, DEFAULT_MODEL_GROUP_NAME)

        if group is None:
            group = ModelGroup(
                name=DEFAULT_MODEL_GROUP_NAME,
                display_name=DEFAULT_MODEL_GROUP_DISPLAY_NAME,
                description=DEFAULT_MODEL_GROUP_DESCRIPTION,
                default_user_billing_multiplier=Decimal("1.0"),
                routing_mode="inherit",
                is_default=True,
                is_active=True,
                sort_order=0,
            )
            db.add(group)
        else:
            group.is_default = True
            if not group.display_name:
                group.display_name = DEFAULT_MODEL_GROUP_DISPLAY_NAME
            if not group.description:
                group.description = DEFAULT_MODEL_GROUP_DESCRIPTION
            if not group.routing_mode:
                group.routing_mode = "inherit"
            if group.sort_order is None:
                group.sort_order = 0
            group.updated_at = datetime.now(timezone.utc)

        if commit:
            db.commit()
            db.refresh(group)
        else:
            db.flush()
        return group

    @staticmethod
    def ensure_user_group_default_binding(
        db: Session,
        user_group: UserGroup,
        *,
        commit: bool = False,
    ) -> UserGroupModelGroup:
        default_model_group = ModelGroupService.get_or_create_default_group(db, commit=False)
        link = (
            db.query(UserGroupModelGroup)
            .filter(
                UserGroupModelGroup.user_group_id == user_group.id,
                UserGroupModelGroup.model_group_id == default_model_group.id,
            )
            .first()
        )
        if link is None:
            link = UserGroupModelGroup(
                user_group_id=user_group.id,
                model_group_id=default_model_group.id,
                priority=100,
                is_active=True,
            )
            db.add(link)
        else:
            link.is_active = True
            if link.priority is None:
                link.priority = 100
            link.updated_at = datetime.now(timezone.utc)

        if commit:
            db.commit()
            db.refresh(link)
        else:
            db.flush()
        return link

    @staticmethod
    def list_groups(db: Session) -> list[tuple[ModelGroup, int, int]]:
        ModelGroupService.get_or_create_default_group(db, commit=True)
        model_count = func.count(func.distinct(ModelGroupModel.global_model_id)).label("model_count")
        user_group_count = func.count(func.distinct(UserGroupModelGroup.user_group_id)).label(
            "user_group_count"
        )
        return (
            db.query(ModelGroup, model_count, user_group_count)
            .outerjoin(ModelGroupModel, ModelGroupModel.model_group_id == ModelGroup.id)
            .outerjoin(
                UserGroupModelGroup,
                and_(
                    UserGroupModelGroup.model_group_id == ModelGroup.id,
                    UserGroupModelGroup.is_active.is_(True),
                ),
            )
            .group_by(ModelGroup.id)
            .order_by(ModelGroup.is_default.desc(), ModelGroup.sort_order.asc(), ModelGroup.name.asc())
            .all()
        )

    @staticmethod
    def get_group_detail(db: Session, group_id: str) -> ModelGroup | None:
        return (
            db.query(ModelGroup)
            .options(
                selectinload(ModelGroup.model_links).selectinload(ModelGroupModel.global_model),
                selectinload(ModelGroup.route_links).selectinload(ModelGroupRoute.provider),
                selectinload(ModelGroup.route_links).selectinload(ModelGroupRoute.provider_api_key),
                selectinload(ModelGroup.user_group_links).selectinload(UserGroupModelGroup.user_group),
            )
            .filter(ModelGroup.id == group_id)
            .first()
        )

    @staticmethod
    def list_groups_for_global_model(db: Session, global_model_id: str) -> list[ModelGroup]:
        return (
            db.query(ModelGroup)
            .join(ModelGroupModel, ModelGroupModel.model_group_id == ModelGroup.id)
            .filter(ModelGroupModel.global_model_id == global_model_id)
            .order_by(ModelGroup.sort_order.asc(), ModelGroup.name.asc())
            .all()
        )

    @staticmethod
    @transactional()
    @retry_on_database_error(max_retries=3)
    def create_group(
        db: Session,
        *,
        name: str,
        display_name: str,
        description: str | None = None,
        default_user_billing_multiplier: float | Decimal = 1.0,
        routing_mode: str = "inherit",
        is_active: bool = True,
        sort_order: int = 100,
        commit: bool = True,
    ) -> ModelGroup:
        if ModelGroupService.get_group_by_name(db, name):
            raise ValueError(f"模型分组已存在: {name}")

        group = ModelGroup(
            name=name,
            display_name=display_name,
            description=description,
            default_user_billing_multiplier=Decimal(str(default_user_billing_multiplier)),
            routing_mode=routing_mode,
            is_default=False,
            is_active=is_active,
            sort_order=sort_order,
        )
        db.add(group)
        if commit:
            db.commit()
            db.refresh(group)
        else:
            db.flush()
        return group

    @staticmethod
    @transactional()
    @retry_on_database_error(max_retries=3)
    def update_group(db: Session, group_id: str, **kwargs: Any) -> ModelGroup | None:
        group = ModelGroupService.get_group(db, group_id)
        if not group:
            return None

        name = kwargs.get("name")
        if name:
            existing = ModelGroupService.get_group_by_name(db, name)
            if existing and existing.id != group_id:
                raise ValueError(f"模型分组已存在: {name}")

        commit = bool(kwargs.pop("commit", True))

        updatable_fields = {
            "name",
            "display_name",
            "description",
            "default_user_billing_multiplier",
            "routing_mode",
            "is_active",
            "sort_order",
        }
        for field, value in kwargs.items():
            if field not in updatable_fields:
                continue
            if field == "default_user_billing_multiplier" and value is not None:
                setattr(group, field, Decimal(str(value)))
                continue
            if value is not None or field in {"description"}:
                setattr(group, field, value)

        group.updated_at = datetime.now(timezone.utc)
        if commit:
            db.commit()
            db.refresh(group)
        else:
            db.flush()
        return group

    @staticmethod
    @transactional()
    @retry_on_database_error(max_retries=3)
    def delete_group(db: Session, group_id: str) -> bool:
        group = ModelGroupService.get_group_detail(db, group_id)
        if not group:
            return False
        if group.is_default:
            raise ValueError("默认模型分组不能删除")
        if group.user_group_links:
            raise ValueError("该模型分组仍有关联的用户分组，请先移除绑定")
        db.delete(group)
        db.commit()
        return True

    @staticmethod
    @transactional()
    @retry_on_database_error(max_retries=3)
    def replace_group_models(
        db: Session,
        group_id: str,
        global_model_ids: list[str],
        *,
        commit: bool = True,
    ) -> ModelGroup:
        group = ModelGroupService.get_group_detail(db, group_id)
        if not group:
            raise ValueError("模型分组不存在")

        normalized_ids: list[str] = []
        seen: set[str] = set()
        for global_model_id in global_model_ids:
            item = str(global_model_id or "").strip()
            if not item or item in seen:
                continue
            seen.add(item)
            normalized_ids.append(item)

        if normalized_ids:
            valid_ids = {
                row[0]
                for row in db.query(GlobalModel.id)
                .filter(GlobalModel.id.in_(normalized_ids), GlobalModel.is_active.is_(True))
                .all()
            }
            missing = [model_id for model_id in normalized_ids if model_id not in valid_ids]
            if missing:
                raise ValueError(f"模型不存在或不可用: {', '.join(missing)}")

        existing_links = {str(link.global_model_id): link for link in group.model_links}
        keep_ids = set(normalized_ids)

        for model_id, link in existing_links.items():
            if model_id not in keep_ids:
                db.delete(link)

        for model_id in normalized_ids:
            if model_id in existing_links:
                continue
            db.add(ModelGroupModel(model_group_id=group.id, global_model_id=model_id))

        group.updated_at = datetime.now(timezone.utc)
        if commit:
            db.commit()
            db.refresh(group)
        else:
            db.flush()
        return group

    @staticmethod
    @transactional()
    @retry_on_database_error(max_retries=3)
    def replace_group_routes(
        db: Session,
        group_id: str,
        routes: list[ModelGroupRoutePayload],
        *,
        commit: bool = True,
    ) -> ModelGroup:
        group = ModelGroupService.get_group_detail(db, group_id)
        if not group:
            raise ValueError("模型分组不存在")

        existing: dict[tuple[str, str], list[ModelGroupRoute]] = {}
        for link in group.route_links:
            existing.setdefault(
                (str(link.provider_id), str(link.provider_api_key_id or "")),
                [],
            ).append(link)
        requested: dict[tuple[str, str], ModelGroupRoutePayload] = {}
        for route in routes:
            provider_id = str(route.provider_id or "").strip()
            provider_api_key_id = str(route.provider_api_key_id or "").strip()
            if not provider_id:
                raise ValueError("provider_id 不能为空")
            key = (provider_id, provider_api_key_id)
            requested[key] = route

        if requested:
            provider_ids = list({provider_id for provider_id, _ in requested.keys()})
            valid_provider_ids = {
                row[0]
                for row in db.query(Provider.id)
                .filter(Provider.id.in_(provider_ids), Provider.is_active.is_(True))
                .all()
            }
            missing_provider_ids = [
                provider_id for provider_id in provider_ids if provider_id not in valid_provider_ids
            ]
            if missing_provider_ids:
                raise ValueError(f"Provider 不存在或不可用: {', '.join(missing_provider_ids)}")

            provider_api_key_ids = [key_id for _, key_id in requested.keys() if key_id]
            if provider_api_key_ids:
                provider_keys = (
                    db.query(ProviderAPIKey.id, ProviderAPIKey.provider_id)
                    .filter(ProviderAPIKey.id.in_(provider_api_key_ids))
                    .all()
                )
                provider_key_to_provider = {
                    str(key_id): str(provider_id) for key_id, provider_id in provider_keys
                }
                missing_key_ids = [
                    key_id for key_id in provider_api_key_ids if key_id not in provider_key_to_provider
                ]
                if missing_key_ids:
                    raise ValueError(f"Provider Key 不存在: {', '.join(missing_key_ids)}")
                mismatched = [
                    key_id
                    for provider_id, key_id in requested.keys()
                    if key_id and provider_key_to_provider.get(key_id) != provider_id
                ]
                if mismatched:
                    raise ValueError(
                        "Provider Key 与 Provider 不匹配: " + ", ".join(sorted(set(mismatched)))
                    )

        for key, links in existing.items():
            if key not in requested:
                for link in links:
                    db.delete(link)

        for key, payload in requested.items():
            provider_id, provider_api_key_id = key
            existing_links = existing.get(key, [])
            route = existing_links[0] if existing_links else None
            for duplicate in existing_links[1:]:
                db.delete(duplicate)
            if route is None:
                route = ModelGroupRoute(
                    model_group_id=group.id,
                    provider_id=provider_id,
                    provider_api_key_id=provider_api_key_id or None,
                )
                db.add(route)
            route.provider_api_key_id = provider_api_key_id or None
            route.priority = int(payload.priority)
            route.user_billing_multiplier_override = (
                Decimal(str(payload.user_billing_multiplier_override))
                if payload.user_billing_multiplier_override is not None
                else None
            )
            route.is_active = bool(payload.is_active)
            route.notes = payload.notes

        group.updated_at = datetime.now(timezone.utc)
        if commit:
            db.commit()
            db.refresh(group)
        else:
            db.flush()
        return group

    @staticmethod
    @transactional()
    @retry_on_database_error(max_retries=3)
    def replace_user_group_bindings(
        db: Session,
        user_group_id: str,
        bindings: list[ModelGroupBindingPayload],
        *,
        commit: bool = True,
    ) -> UserGroup:
        user_group = db.query(UserGroup).filter(UserGroup.id == user_group_id).first()
        if not user_group:
            raise ValueError("用户分组不存在")

        normalized: list[ModelGroupBindingPayload] = []
        seen: set[str] = set()
        for binding in bindings:
            model_group_id = str(binding.model_group_id or "").strip()
            if not model_group_id or model_group_id in seen:
                continue
            seen.add(model_group_id)
            normalized.append(
                ModelGroupBindingPayload(
                    model_group_id=model_group_id,
                    priority=int(binding.priority),
                    is_active=bool(binding.is_active),
                )
            )

        valid_group_ids = {
            row[0]
            for row in db.query(ModelGroup.id)
            .filter(ModelGroup.id.in_([binding.model_group_id for binding in normalized]))
            .all()
        }
        missing = [binding.model_group_id for binding in normalized if binding.model_group_id not in valid_group_ids]
        if missing:
            raise ValueError(f"模型分组不存在: {', '.join(missing)}")

        existing = {str(link.model_group_id): link for link in user_group.model_group_links}
        keep_ids = {binding.model_group_id for binding in normalized}

        for model_group_id, link in existing.items():
            if model_group_id not in keep_ids:
                db.delete(link)

        for binding in normalized:
            link = existing.get(binding.model_group_id)
            if link is None:
                link = UserGroupModelGroup(
                    user_group_id=user_group.id,
                    model_group_id=binding.model_group_id,
                )
                db.add(link)
            link.priority = binding.priority
            link.is_active = binding.is_active
            link.updated_at = datetime.now(timezone.utc)

        user_group.updated_at = datetime.now(timezone.utc)
        if commit:
            db.commit()
            db.refresh(user_group)
        else:
            db.flush()
        return user_group

    @staticmethod
    @transactional()
    @retry_on_database_error(max_retries=3)
    def replace_global_model_memberships(
        db: Session,
        global_model_id: str,
        model_group_ids: list[str],
        *,
        commit: bool = True,
    ) -> GlobalModel:
        global_model = db.query(GlobalModel).filter(GlobalModel.id == global_model_id).first()
        if global_model is None:
            raise ValueError("模型不存在")

        normalized_ids: list[str] = []
        seen: set[str] = set()
        for model_group_id in model_group_ids:
            item = str(model_group_id or "").strip()
            if not item or item in seen:
                continue
            seen.add(item)
            normalized_ids.append(item)

        if normalized_ids:
            valid_ids = {
                row[0]
                for row in db.query(ModelGroup.id)
                .filter(ModelGroup.id.in_(normalized_ids), ModelGroup.is_active.is_(True))
                .all()
            }
            missing = [model_group_id for model_group_id in normalized_ids if model_group_id not in valid_ids]
            if missing:
                raise ValueError(f"模型分组不存在或不可用: {', '.join(missing)}")

        existing_links = {str(link.model_group_id): link for link in global_model.model_group_links}
        keep_ids = set(normalized_ids)

        for model_group_id, link in existing_links.items():
            if model_group_id not in keep_ids:
                db.delete(link)

        for model_group_id in normalized_ids:
            if model_group_id in existing_links:
                continue
            db.add(ModelGroupModel(model_group_id=model_group_id, global_model_id=global_model.id))

        if commit:
            db.commit()
            db.refresh(global_model)
        else:
            db.flush()
        return global_model

    @staticmethod
    def get_matching_user_group_model_groups(
        db: Session,
        user: User | None,
        global_model_id: str,
    ) -> list[UserGroupModelGroup]:
        if user is None or not getattr(user, "group_id", None):
            return []

        return (
            db.query(UserGroupModelGroup)
            .options(
                selectinload(UserGroupModelGroup.model_group).selectinload(ModelGroup.route_links),
                selectinload(UserGroupModelGroup.model_group).selectinload(ModelGroup.model_links),
            )
            .join(ModelGroup, UserGroupModelGroup.model_group_id == ModelGroup.id)
            .join(
                ModelGroupModel,
                and_(
                    ModelGroupModel.model_group_id == ModelGroup.id,
                    ModelGroupModel.global_model_id == global_model_id,
                ),
            )
            .filter(
                UserGroupModelGroup.user_group_id == user.group_id,
                UserGroupModelGroup.is_active.is_(True),
                ModelGroup.is_active.is_(True),
            )
            .order_by(
                UserGroupModelGroup.priority.asc(),
                ModelGroup.sort_order.asc(),
                ModelGroup.created_at.asc(),
                ModelGroup.id.asc(),
            )
            .all()
        )
