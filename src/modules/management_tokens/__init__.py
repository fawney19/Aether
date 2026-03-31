"""
访问令牌模块

提供管理 API 访问令牌的功能
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from fastapi import APIRouter

from src.core.modules.base import (
    ModuleCategory,
    ModuleDefinition,
    ModuleHealth,
    ModuleMetadata,
)

if TYPE_CHECKING:
    from sqlalchemy.orm import Session


def _get_router() -> Any:
    """延迟导入路由（避免启动时加载重依赖）

    返回一个合并了管理员和用户两个路由的 APIRouter
    - /api/admin/management-tokens: 管理员管理所有用户的令牌
    - /api/me/management-tokens: 用户管理自己的令牌
    """
    from src.api.admin.management_tokens import router as admin_router
    from src.api.user_me.management_tokens import router as user_router

    # 创建一个组合路由器
    combined_router = APIRouter()
    combined_router.include_router(admin_router)
    combined_router.include_router(user_router)

    return combined_router


async def _health_check() -> ModuleHealth:
    """健康检查 - 简化版，不依赖数据库连接"""
    return ModuleHealth.HEALTHY


def _validate_config(db: Session) -> tuple[bool, str]:
    """
    验证配置是否可以启用模块

    访问令牌模块没有特殊配置要求，始终可用
    """
    return True, ""


# ==================== 钩子实现 ====================


async def _authenticate_by_token(db: Any, token: str, client_ip: str) -> tuple[Any, Any] | None:
    """执行 Management Token 认证"""
    from src.services.auth.service import AuthService

    return await AuthService.authenticate_management_token(db, token, client_ip)


def _hook_token_prefix_authenticators(**_kwargs: Any) -> list[dict[str, Any]]:
    """auth.token_prefix_authenticators: 声明 ae_ 前缀认证器"""
    from src.models.database import ManagementToken

    return [
        {
            "prefix": ManagementToken.TOKEN_PREFIX,
            "module": "management_tokens",
            "authenticate": _authenticate_by_token,
        }
    ]


# 访问令牌模块定义
management_tokens_module = ModuleDefinition(
    metadata=ModuleMetadata(
        name="management_tokens",
        display_name="访问令牌",
        description="管理 API 访问令牌，支持细粒度权限控制和 IP 白名单",
        category=ModuleCategory.SECURITY,
        # 可用性控制
        env_key="MANAGEMENT_TOKENS_AVAILABLE",
        default_available=True,
        required_packages=[],
        # 路由配置
        api_prefix="/api/admin/management-tokens, /api/me/management-tokens",
        # 前端配置
        admin_route="/admin/management-tokens",
        admin_menu_icon="KeyRound",
        admin_menu_group="system",
        admin_menu_order=57,
    ),
    router_factory=_get_router,
    health_check=_health_check,
    validate_config=_validate_config,
    hooks={
        "auth.token_prefix_authenticators": _hook_token_prefix_authenticators,
    },
)
