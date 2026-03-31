"""用户访问限制解析逻辑。"""

from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from src.models.database import ModelGroup, User, UserGroup, UserGroupModelGroup


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


def resolve_user_model_group_links(user: User | None) -> list[UserGroupModelGroup]:
    """返回用户组下已启用的模型分组绑定，按优先级排序。"""
    group = get_user_group(user)
    if group is None:
        return []

    links = list(getattr(group, "model_group_links", None) or [])
    active_links: list[UserGroupModelGroup] = []
    for link in links:
        if not getattr(link, "is_active", True):
            continue
        model_group = getattr(link, "model_group", None)
        if model_group is None or not getattr(model_group, "is_active", True):
            continue
        active_links.append(link)

    active_links.sort(
        key=lambda link: (
            int(getattr(link, "priority", 100)),
            int(getattr(getattr(link, "model_group", None), "sort_order", 100)),
            str(getattr(link, "id", "") or ""),
        )
    )
    return active_links


def resolve_user_model_groups(user: User | None) -> list[ModelGroup]:
    """返回用户可见的模型分组列表。"""
    return [
        link.model_group
        for link in resolve_user_model_group_links(user)
        if getattr(link, "model_group", None) is not None
    ]


def resolve_user_allowed_providers(user: User | None) -> list[str] | None:
    """解析用户 Provider 限制。

    ModelGroup 的 Provider 规则是按模型生效的，不再提供全局 Provider 白名单。
    这里保留接口兼容，统一返回 None。
    """
    if user is None:
        return None
    return None


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

    links = resolve_user_model_group_links(user)
    if not links:
        return []

    allowed_models: set[str] = set()
    for link in links:
        model_group = getattr(link, "model_group", None)
        if model_group is None:
            continue
        for model_link in list(getattr(model_group, "model_links", None) or []):
            global_model = getattr(model_link, "global_model", None)
            model_name = getattr(global_model, "name", None)
            if isinstance(model_name, str) and model_name.strip():
                allowed_models.add(model_name.strip())

    return sorted(allowed_models)


def resolve_user_allowed_providers_for_global_model(
    user: User | None,
    global_model_id: str,
) -> dict[str, int] | None:
    """解析指定模型下的 Provider 优先级覆盖。"""
    if user is None:
        return None

    provider_priorities: dict[str, int] = {}
    for link in resolve_user_model_group_links(user):
        model_group = getattr(link, "model_group", None)
        if model_group is None:
            continue

        contains_model = any(
            str(getattr(model_link, "global_model_id", "") or "") == str(global_model_id)
            for model_link in list(getattr(model_group, "model_links", None) or [])
        )
        if not contains_model:
            continue

        for route_link in list(getattr(model_group, "route_links", None) or []):
            if not getattr(route_link, "is_active", True):
                continue
            provider_id = str(getattr(route_link, "provider_id", "") or "").strip()
            if not provider_id:
                continue
            priority = int(getattr(route_link, "priority", 50) or 50)
            existing = provider_priorities.get(provider_id)
            provider_priorities[provider_id] = (
                priority if existing is None else min(existing, priority)
            )

    return provider_priorities if provider_priorities else {}


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
