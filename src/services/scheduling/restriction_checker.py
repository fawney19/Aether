"""
访问限制检查

从 CacheAwareScheduler 提取的 ApiKey + User 访问限制合并逻辑。
"""

from __future__ import annotations

from typing import Any

from src.core.logger import logger
from src.core.model_permissions import merge_allowed_models
from src.core.user_access import (
    resolve_user_allowed_api_formats,
    resolve_user_allowed_models,
    resolve_user_allowed_providers,
)
from src.models.database import ApiKey


def get_effective_restrictions(user_api_key: ApiKey | None) -> dict[str, Any]:
    """
    获取有效的访问限制（合并 ApiKey 和 User 的限制）

    逻辑：
    - 如果 ApiKey 和 User 都有限制，取交集
    - 如果只有一方有限制，使用该方的限制
    - 如果都没有限制，返回 None（表示不限制）

    Args:
        user_api_key: 用户 API Key 对象（可能包含 user relationship）

    Returns:
        包含 allowed_providers, allowed_models, allowed_api_formats 的字典
    """
    result: dict[str, Any] = {
        "allowed_providers": None,
        "allowed_models": None,
        "allowed_api_formats": None,
    }

    if not user_api_key:
        return result

    # 获取 User 的限制
    # 注意：这里可能触发 lazy loading，需要确保 session 仍然有效
    try:
        user = user_api_key.user if hasattr(user_api_key, "user") else None
    except Exception as e:
        logger.warning("无法加载 ApiKey 关联的 User: {}，仅使用 ApiKey 级别的限制", e)
        user = None

    # 调试日志
    logger.debug(
        "[_get_effective_restrictions] ApiKey={}..., User={}..., "
        "ApiKey.allowed_models={}, Group.allowed_models={}",
        user_api_key.id[:8],
        user.id[:8] if user else "None",
        user_api_key.allowed_models,
        resolve_user_allowed_models(user) if user else "N/A",
    )

    # 合并 allowed_providers
    result["allowed_providers"] = merge_restriction_sets(
        user_api_key.allowed_providers,
        resolve_user_allowed_providers(user) if user else None,
    )

    # 合并 allowed_models（取交集）
    result["allowed_models"] = merge_allowed_models(
        user_api_key.allowed_models,
        resolve_user_allowed_models(user) if user else None,
    )

    # 合并 allowed_api_formats
    result["allowed_api_formats"] = merge_restriction_sets(
        user_api_key.allowed_api_formats,
        resolve_user_allowed_api_formats(user) if user else None,
    )

    return result


def merge_restriction_sets(key_restriction: Any, user_restriction: Any) -> set[Any] | None:
    """合并两个限制列表，取交集；任一方为空则使用另一方；均空返回 None"""
    key_set = set(key_restriction) if key_restriction else None
    user_set = set(user_restriction) if user_restriction else None
    if key_set and user_set:
        return key_set & user_set
    return key_set or user_set
