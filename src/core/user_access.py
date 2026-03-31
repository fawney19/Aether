"""用户访问限制解析逻辑。"""

from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from src.models.database import User, UserGroup


@dataclass(frozen=True)
class EffectiveUserAccessConfig:
    """用户最终生效的访问配置。"""

    allowed_providers: list[str] | None
    allowed_api_formats: list[str] | None
    allowed_models: list[str] | None
    rate_limit: int | None


def get_user_group(user: User | None) -> UserGroup | None:
    """返回用户所属分组。"""
    if user is None:
        return None
    group = getattr(user, "group", None)
    return group if group is not None else None


def resolve_user_allowed_providers(user: User | None) -> list[str] | None:
    """解析用户最终生效的 Provider 限制。"""
    if user is None:
        return None
    group = get_user_group(user)
    return getattr(group, "allowed_providers", None) if group else None


def resolve_user_allowed_api_formats(user: User | None) -> list[str] | None:
    """解析用户最终生效的 API 格式限制。"""
    if user is None:
        return None
    group = get_user_group(user)
    return getattr(group, "allowed_api_formats", None) if group else None


def resolve_user_allowed_models(user: User | None) -> list[str] | None:
    """解析用户最终生效的模型限制。"""
    if user is None:
        return None
    group = get_user_group(user)
    return getattr(group, "allowed_models", None) if group else None


def resolve_user_rate_limit(user: User | None) -> int | None:
    """解析用户最终生效的 RPM 限制。"""
    if user is None:
        return None
    group = get_user_group(user)
    return getattr(group, "rate_limit", None) if group else None


def resolve_user_access_config(user: User | None) -> EffectiveUserAccessConfig:
    """解析用户最终生效的访问配置。"""
    return EffectiveUserAccessConfig(
        allowed_providers=resolve_user_allowed_providers(user),
        allowed_api_formats=resolve_user_allowed_api_formats(user),
        allowed_models=resolve_user_allowed_models(user),
        rate_limit=resolve_user_rate_limit(user),
    )
