"""
Gemini Files 文件管理模块

提供 Gemini Files API 文件上传和管理功能：
- 文件上传到 Google Gemini Files API
- 文件映射管理（file_id → key_id）
- 文件列表查看和删除
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from src.core.modules.base import (
    ModuleCategory,
    ModuleDefinition,
    ModuleHealth,
    ModuleMetadata,
)

if TYPE_CHECKING:
    from sqlalchemy.orm import Session


def _get_router() -> Any:
    """延迟导入路由"""
    from src.api.admin.gemini_files import router

    return router


async def _health_check() -> ModuleHealth:
    """健康检查"""
    # 检查是否有可用的 gemini_files 能力的 Key
    from src.database import create_session
    from src.models.database import ProviderAPIKey

    db = create_session()
    try:
        # 查找有 gemini_files 能力的 Key
        keys = db.query(ProviderAPIKey).filter(ProviderAPIKey.is_active.is_(True)).all()
        has_capable_key = any(
            key.capabilities and key.capabilities.get("gemini_files", False) for key in keys
        )
        if has_capable_key:
            return ModuleHealth.HEALTHY
        return ModuleHealth.DEGRADED
    except Exception:
        return ModuleHealth.UNKNOWN
    finally:
        db.close()


def _validate_config(db: Session) -> tuple[bool, str]:
    """
    验证 Gemini Files 模块配置

    检查项：
    1. 至少有一个有 gemini_files 能力的 Provider Key
    """
    from src.models.database import ProviderAPIKey

    # 查找有 gemini_files 能力的 Key
    keys = db.query(ProviderAPIKey).filter(ProviderAPIKey.is_active.is_(True)).all()
    capable_keys = [
        key for key in keys if key.capabilities and key.capabilities.get("gemini_files", False)
    ]

    if not capable_keys:
        return (
            False,
            "至少启用一个具有「Gemini 文件 API」能力的 Key",
        )

    return True, ""


gemini_files_module = ModuleDefinition(
    metadata=ModuleMetadata(
        name="gemini_files",
        display_name="Gemini Files 文件缓存",
        description="管理 Gemini Files API 上传的文件，支持文件上传、查看和删除",
        category=ModuleCategory.INTEGRATION,
        env_key="GEMINI_FILES_AVAILABLE",
        default_available=True,
        required_packages=[],
        api_prefix="/api/admin/gemini-files",
        admin_route="/admin/gemini-files",
        admin_menu_icon="FileUp",
        admin_menu_group="system",
        admin_menu_order=60,
    ),
    router_factory=_get_router,
    health_check=_health_check,
    validate_config=_validate_config,
)
