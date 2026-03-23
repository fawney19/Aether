"""用户个人 API 端点。"""

from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from typing import Any

from fastapi import APIRouter, Depends, HTTPException, Query, Request
from fastapi.concurrency import run_in_threadpool
from pydantic import ValidationError
from sqlalchemy import and_, func
from sqlalchemy.orm import Session

from src.api.base.authenticated_adapter import AuthenticatedApiAdapter
from src.api.base.context import ApiRequestContext
from src.api.base.models_service import sanitize_public_global_model_config
from src.api.base.pipeline import get_pipeline
from src.core.crypto import crypto_service
from src.core.enums import UserRole
from src.core.exceptions import (
    ForbiddenException,
    InvalidRequestException,
    NotFoundException,
    translate_pydantic_error,
)
from src.core.logger import logger
from src.core.validators import PasswordValidator
from src.database import get_db, get_db_context
from src.models.api import (
    ChangePasswordRequest,
    CreateMyApiKeyRequest,
    PublicGlobalModelListResponse,
    PublicGlobalModelResponse,
    UpdateMyApiKeyRequest,
    UpdatePreferencesRequest,
    UpdateProfileRequest,
)
from src.models.database import (
    ApiKey,
    GlobalModel,
    Model,
    Provider,
    Usage,
    User,
    UserModelUsageCount,
)
from src.services.cache.user_cache import UserCacheService
from src.services.system.config import SystemConfigService
from src.services.user.apikey import ApiKeyService
from src.services.user.bulk_cleanup import pre_clean_api_key
from src.services.user.preference import PreferenceService

router = APIRouter(prefix="/api/users/me", tags=["User Profile"])
pipeline = get_pipeline()

def _update_profile_sync(
    user_id: str,
    request: UpdateProfileRequest,
) -> tuple[dict[str, Any], str | None, str | None]:
    with get_db_context() as db:
        user = db.query(User).filter(User.id == user_id).first()
        if not user:
            raise NotFoundException("用户不存在", "user")

        old_email = user.email
        new_email = old_email

        if request.email:
            existing = (
                db.query(User).filter(User.email == request.email, User.id != user.id).first()
            )
            if existing:
                raise InvalidRequestException("邮箱已被使用")
            user.email = request.email
            new_email = request.email

        if request.username:
            existing = (
                db.query(User).filter(User.username == request.username, User.id != user.id).first()
            )
            if existing:
                raise InvalidRequestException("用户名已被使用")
            user.username = request.username

        user.updated_at = datetime.now(timezone.utc)
        return {"message": "个人信息更新成功"}, old_email, new_email


def _change_password_sync(
    user_id: str,
    request: ChangePasswordRequest,
) -> tuple[dict[str, Any], str | None, str]:
    from src.core.enums import AuthSource

    with get_db_context() as db:
        user = db.query(User).filter(User.id == user_id).first()
        if not user:
            raise NotFoundException("用户不存在", "user")

        if user.auth_source == AuthSource.LDAP:
            raise ForbiddenException("LDAP 用户不能在此修改密码")

        has_password = bool(user.password_hash)
        if has_password:
            if not request.old_password:
                raise InvalidRequestException("请输入当前密码")
            if not user.verify_password(request.old_password):
                raise InvalidRequestException("旧密码错误")

        policy_level = SystemConfigService.get_password_policy_level(db)
        valid, error_msg = PasswordValidator.validate(request.new_password, policy=policy_level)
        if not valid:
            raise InvalidRequestException(error_msg or "密码格式无效")

        user.set_password(request.new_password)
        user.updated_at = datetime.now(timezone.utc)
        action = "修改" if has_password else "设置"
        return {"message": f"密码{action}成功"}, user.email, action


def _create_my_api_key_sync(user_id: str, request: CreateMyApiKeyRequest) -> dict[str, Any]:
    with get_db_context() as db:
        try:
            api_key, plain_key = ApiKeyService.create_api_key(
                db=db,
                user_id=user_id,
                name=request.name,
                rate_limit=request.rate_limit,
            )
        except ValueError as exc:
            raise InvalidRequestException(str(exc)) from exc
        return {
            "id": api_key.id,
            "name": api_key.name,
            "key": plain_key,
            "key_display": api_key.get_display_key(),
            "rate_limit": api_key.rate_limit,
            "message": "API密钥创建成功",
        }


def _delete_my_api_key_sync(user_id: str, key_id: str) -> dict[str, str]:
    with get_db_context() as db:
        api_key = db.query(ApiKey).filter(ApiKey.id == key_id, ApiKey.user_id == user_id).first()
        if not api_key:
            raise NotFoundException("API密钥不存在", "api_key")
        if api_key.is_locked:
            raise ForbiddenException("该密钥已被管理员锁定，无法删除")

        pre_clean_api_key(db, api_key.id)
        db.delete(api_key)
        return {"message": "API密钥已删除"}


def _toggle_my_api_key_sync(user_id: str, key_id: str) -> dict[str, Any]:
    with get_db_context() as db:
        api_key = db.query(ApiKey).filter(ApiKey.id == key_id, ApiKey.user_id == user_id).first()
        if not api_key:
            raise NotFoundException("API密钥不存在", "api_key")
        if api_key.is_locked:
            raise ForbiddenException("该密钥已被管理员锁定，无法修改状态")

        api_key.is_active = not api_key.is_active
        db.commit()
        db.refresh(api_key)
        return {
            "id": api_key.id,
            "is_active": api_key.is_active,
            "message": f"API密钥已{'启用' if api_key.is_active else '禁用'}",
        }


def _update_my_api_key_sync(
    user_id: str,
    key_id: str,
    request: UpdateMyApiKeyRequest,
) -> dict[str, Any]:
    with get_db_context() as db:
        api_key = (
            db.query(ApiKey)
            .filter(
                ApiKey.id == key_id,
                ApiKey.user_id == user_id,
                ApiKey.is_standalone == False,
            )
            .first()
        )
        if not api_key:
            raise NotFoundException("API密钥不存在", "api_key")
        if api_key.is_locked:
            raise ForbiddenException("该密钥已被管理员锁定，无法修改")

        update_data = request.model_dump(exclude_unset=True)
        if "rate_limit" in update_data and update_data["rate_limit"] is None:
            update_data["rate_limit"] = 0

        updated = ApiKeyService.update_api_key(db, key_id, **update_data)
        if not updated:
            raise NotFoundException("API密钥不存在", "api_key")

        return {
            "id": updated.id,
            "name": updated.name,
            "key_display": updated.get_display_key(),
            "is_active": updated.is_active,
            "is_locked": updated.is_locked,
            "force_capabilities": updated.force_capabilities,
            "rate_limit": updated.rate_limit,
            "last_used_at": updated.last_used_at.isoformat() if updated.last_used_at else None,
            "expires_at": updated.expires_at.isoformat() if updated.expires_at else None,
            "created_at": updated.created_at.isoformat(),
            "message": "API密钥已更新",
        }


def _update_api_key_capabilities_sync(
    user_id: str,
    api_key_id: str,
    payload: dict[str, Any],
) -> dict[str, Any]:
    from src.core.key_capabilities import CAPABILITY_DEFINITIONS, CapabilityConfigMode
    from src.models.database import AuditEventType
    from src.services.system.audit import audit_service

    with get_db_context() as db:
        api_key = (
            db.query(ApiKey).filter(ApiKey.id == api_key_id, ApiKey.user_id == user_id).first()
        )
        if not api_key:
            raise NotFoundException("API密钥不存在")
        if api_key.is_locked:
            raise ForbiddenException("该密钥已被管理员锁定，无法修改")

        old_capabilities = api_key.force_capabilities
        force_capabilities = payload.get("force_capabilities")
        if force_capabilities is not None:
            if not isinstance(force_capabilities, dict):
                raise InvalidRequestException("force_capabilities 必须是对象类型")

            for cap_name, cap_value in force_capabilities.items():
                cap_def = CAPABILITY_DEFINITIONS.get(cap_name)
                if not cap_def:
                    raise InvalidRequestException(f"未知的能力类型: {cap_name}")
                if cap_def.config_mode != CapabilityConfigMode.USER_CONFIGURABLE:
                    raise InvalidRequestException(f"能力 {cap_name} 不支持用户配置")
                if not isinstance(cap_value, bool):
                    raise InvalidRequestException(f"能力 {cap_name} 的值必须是布尔类型")

        api_key.force_capabilities = force_capabilities
        api_key.updated_at = datetime.now(timezone.utc)
        audit_service.log_event(
            db=db,
            event_type=AuditEventType.CONFIG_CHANGED,
            description="用户更新 API Key 能力配置",
            user_id=user_id,
            api_key_id=api_key.id,
            metadata={
                "action": "update_api_key_capabilities",
                "old_capabilities": old_capabilities,
                "new_capabilities": force_capabilities,
            },
        )
        return {
            "message": "API密钥能力配置已更新",
            "force_capabilities": api_key.force_capabilities,
        }


def _update_preferences_sync(user_id: str, request: UpdatePreferencesRequest) -> dict[str, str]:
    with get_db_context() as db:
        PreferenceService.update_preferences(
            db=db,
            user_id=user_id,
            avatar_url=request.avatar_url,
            bio=request.bio,
            theme=request.theme,
            language=request.language,
            timezone=request.timezone,
            email_notifications=request.email_notifications,
            usage_alerts=request.usage_alerts,
            announcement_notifications=request.announcement_notifications,
        )
        return {"message": "偏好设置更新成功"}


def _update_model_capability_settings_sync(
    user_id: str,
    payload: dict[str, Any],
) -> tuple[dict[str, Any], str | None]:
    from src.core.key_capabilities import CAPABILITY_DEFINITIONS, CapabilityConfigMode
    from src.models.database import AuditEventType
    from src.services.system.audit import audit_service

    with get_db_context() as db:
        user = db.query(User).filter(User.id == user_id).first()
        if not user:
            raise NotFoundException("用户不存在")

        old_settings = user.model_capability_settings
        settings = payload.get("model_capability_settings")
        if settings is not None:
            if not isinstance(settings, dict):
                raise InvalidRequestException("model_capability_settings 必须是对象类型")

            for model_name, capabilities in settings.items():
                if not isinstance(model_name, str):
                    raise InvalidRequestException("模型名称必须是字符串")
                if not isinstance(capabilities, dict):
                    raise InvalidRequestException(f"模型 {model_name} 的能力配置必须是对象类型")

                for cap_name, cap_value in capabilities.items():
                    cap_def = CAPABILITY_DEFINITIONS.get(cap_name)
                    if not cap_def:
                        raise InvalidRequestException(f"未知的能力类型: {cap_name}")
                    if cap_def.config_mode != CapabilityConfigMode.USER_CONFIGURABLE:
                        raise InvalidRequestException(f"能力 {cap_name} 不支持用户配置")
                    if not isinstance(cap_value, bool):
                        raise InvalidRequestException(f"能力 {cap_name} 的值必须是布尔类型")

        user.model_capability_settings = settings
        user.updated_at = datetime.now(timezone.utc)
        audit_service.log_event(
            db=db,
            event_type=AuditEventType.CONFIG_CHANGED,
            description="用户更新模型能力配置",
            user_id=user.id,
            metadata={
                "action": "update_model_capability_settings",
                "old_settings": old_settings,
                "new_settings": settings,
            },
        )
        return {
            "message": "模型能力配置已更新",
            "model_capability_settings": user.model_capability_settings,
        }, user.email
@router.get("")
async def get_my_profile(request: Request, db: Session = Depends(get_db)) -> Any:
    """
    获取当前用户信息

    返回当前登录用户的完整信息，包括基本信息和偏好设置。

    **返回字段**: id, email, username, role, is_active, billing, preferences 等
    """
    adapter = MeProfileAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.put("")
async def update_my_profile(request: Request, db: Session = Depends(get_db)) -> None:
    """
    更新个人信息

    更新当前用户的邮箱或用户名。

    **请求体**:
    - `email`: 新邮箱地址（可选）
    - `username`: 新用户名（可选）
    """
    adapter = UpdateProfileAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.patch("/password")
async def change_my_password(request: Request, db: Session = Depends(get_db)) -> Any:
    """
    修改密码

    修改当前用户的登录密码。

    **请求体**:
    - `old_password`: 当前密码
    - `new_password`: 新密码（至少 6 位）
    """
    adapter = ChangePasswordAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


# ============== API密钥管理 ==============


@router.get("/api-keys")
async def list_my_api_keys(request: Request, db: Session = Depends(get_db)) -> Any:
    """
    获取 API 密钥列表

    返回当前用户的所有 API 密钥，包含使用统计信息。
    密钥值仅显示前后几位，完整密钥需通过详情接口获取。

    **返回字段**: id, name, key_display, is_active, total_requests, total_cost_usd, last_used_at 等
    """
    adapter = ListMyApiKeysAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/api-keys")
async def create_my_api_key(request: Request, db: Session = Depends(get_db)) -> Any:
    """
    创建 API 密钥

    为当前用户创建新的 API 密钥。创建成功后会返回完整的密钥值，请妥善保存。

    **请求体**:
    - `name`: 密钥名称

    **返回**: 包含完整密钥值的响应（仅此一次显示完整密钥）
    """
    adapter = CreateMyApiKeyAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/api-keys/{key_id}")
async def get_my_api_key(
    key_id: str,
    request: Request,
    include_key: bool = Query(False, description="是否返回完整密钥"),
    db: Session = Depends(get_db),
) -> Any:
    """
    获取 API 密钥详情

    获取指定 API 密钥的详细信息。

    **路径参数**:
    - `key_id`: 密钥 ID

    **查询参数**:
    - `include_key`: 设为 true 时返回完整解密后的密钥值
    """
    if include_key:
        adapter = GetMyFullKeyAdapter(key_id=key_id)
    else:
        adapter = GetMyApiKeyDetailAdapter(key_id=key_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.delete("/api-keys/{key_id}")
async def delete_my_api_key(key_id: str, request: Request, db: Session = Depends(get_db)) -> None:
    """
    删除 API 密钥

    永久删除指定的 API 密钥，删除后无法恢复。

    **路径参数**:
    - `key_id`: 密钥 ID
    """
    adapter = DeleteMyApiKeyAdapter(key_id=key_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.put("/api-keys/{key_id}")
async def update_my_api_key(key_id: str, request: Request, db: Session = Depends(get_db)) -> Any:
    """
    更新 API 密钥

    更新指定 API 密钥的基础配置。

    **路径参数**:
    - `key_id`: 密钥 ID
    """
    adapter = UpdateMyApiKeyAdapter(key_id=key_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.patch("/api-keys/{key_id}")
async def toggle_my_api_key(key_id: str, request: Request, db: Session = Depends(get_db)) -> Any:
    """
    切换 API 密钥状态

    启用或禁用指定的 API 密钥。禁用后该密钥将无法用于 API 调用。

    **路径参数**:
    - `key_id`: 密钥 ID
    """
    adapter = ToggleMyApiKeyAdapter(key_id=key_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/available-models")
async def list_available_models(
    request: Request,
    skip: int = Query(0, ge=0, description="跳过记录数"),
    limit: int = Query(100, ge=1, le=1000, description="返回记录数限制"),
    search: str | None = Query(None, description="搜索关键词"),
    db: Session = Depends(get_db),
) -> Any:
    """
    获取用户可用的模型列表

    根据用户权限返回可用的 GlobalModel 列表。
    - 管理员：可以看到所有活跃提供商的模型
    - 普通用户：只能看到关联提供商的模型

    **查询参数**:
    - skip: 跳过的记录数，用于分页，默认 0
    - limit: 返回记录数限制，默认 100，范围 1-1000
    - search: 可选，搜索关键词，支持模糊匹配模型名称

    **返回字段**:
    - models: 模型列表
    - total: 符合条件的模型总数
    """
    adapter = ListAvailableModelsAdapter(skip=skip, limit=limit, search=search)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/endpoint-status")
async def get_endpoint_status(request: Request, db: Session = Depends(get_db)) -> Any:
    """
    获取端点健康状态

    获取各 API 格式端点的健康状态（简化版，不包含敏感信息）。

    **返回**: 按 API 格式分组的端点健康状态
    """
    adapter = GetEndpointStatusAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.put("/api-keys/{api_key_id}/capabilities")
async def update_api_key_capabilities(
    api_key_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    更新 API 密钥能力配置

    设置指定 API 密钥的强制能力配置（如是否启用代码执行等）。

    **路径参数**:
    - `api_key_id`: API 密钥 ID

    **请求体**:
    - `force_capabilities`: 能力配置字典，如 `{"code_execution": true}`
    """
    adapter = UpdateApiKeyCapabilitiesAdapter(api_key_id=api_key_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


# ============== 偏好设置 ==============


@router.get("/preferences")
async def get_my_preferences(request: Request, db: Session = Depends(get_db)) -> Any:
    """
    获取偏好设置

    获取当前用户的偏好设置，包括主题、语言、通知配置等。

    **返回字段**: avatar_url, bio, theme, language, timezone, notifications 等
    """
    adapter = GetPreferencesAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.put("/preferences")
async def update_my_preferences(request: Request, db: Session = Depends(get_db)) -> None:
    """
    更新偏好设置

    更新当前用户的偏好设置。

    **请求体**:
    - `theme`: 主题（light/dark）
    - `language`: 语言
    - `timezone`: 时区
    - `email_notifications`: 邮件通知开关
    - `usage_alerts`: 用量告警开关
    - 等
    """
    adapter = UpdatePreferencesAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/model-capabilities")
async def get_model_capability_settings(request: Request, db: Session = Depends(get_db)) -> Any:
    """
    获取模型能力配置

    获取用户针对各模型的能力配置（如是否启用特定功能）。

    **返回**: model_capability_settings 字典
    """
    adapter = GetModelCapabilitySettingsAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.put("/model-capabilities")
async def update_model_capability_settings(request: Request, db: Session = Depends(get_db)) -> None:
    """
    更新模型能力配置

    更新用户针对各模型的能力配置。

    **请求体**:
    - `model_capability_settings`: 模型能力配置字典，格式为 `{"model_name": {"capability": true}}`
    """
    adapter = UpdateModelCapabilitySettingsAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


# ============== Pipeline 适配器 ==============


class MeProfileAdapter(AuthenticatedApiAdapter):
    """获取当前用户信息的适配器"""

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        return PreferenceService.get_user_with_preferences(context.db, context.user.id)


class UpdateProfileAdapter(AuthenticatedApiAdapter):
    """更新用户个人信息的适配器"""

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        user = context.user
        payload = context.ensure_json_body()
        try:
            request = UpdateProfileRequest.model_validate(payload)
        except ValidationError as e:
            errors = e.errors()
            if errors:
                raise InvalidRequestException(translate_pydantic_error(errors[0]))
            raise InvalidRequestException("请求数据验证失败")

        result, old_email, new_email = await run_in_threadpool(
            _update_profile_sync,
            user.id,
            request,
        )
        await UserCacheService.invalidate_user_cache(user.id, old_email)
        if new_email and new_email != old_email:
            await UserCacheService.invalidate_user_cache(user.id, new_email)
        return result


class ChangePasswordAdapter(AuthenticatedApiAdapter):
    """修改用户密码的适配器"""

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        user = context.user
        payload = context.ensure_json_body()
        try:
            request = ChangePasswordRequest.model_validate(payload)
        except ValidationError as e:
            errors = e.errors()
            if errors:
                raise InvalidRequestException(translate_pydantic_error(errors[0]))
            raise InvalidRequestException("请求数据验证失败")

        result, email, action = await run_in_threadpool(
            _change_password_sync,
            user.id,
            request,
        )
        logger.info(f"用户{action}密码: {email}")
        return result


class ListMyApiKeysAdapter(AuthenticatedApiAdapter):
    """获取用户 API 密钥列表的适配器"""

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        user = context.user

        # 一次性查询所有 API keys
        api_keys = (
            db.query(ApiKey)
            .filter(ApiKey.user_id == user.id)
            .order_by(ApiKey.created_at.desc())
            .all()
        )

        if not api_keys:
            return []

        # 批量查询所有 API keys 的统计数据（单次查询）
        api_key_ids = [key.id for key in api_keys]
        stats_query = (
            db.query(
                Usage.api_key_id,
                func.count(Usage.id).label("requests"),
                func.sum(Usage.total_cost_usd).label("cost"),
                func.max(Usage.created_at).label("last_used"),
            )
            .filter(Usage.api_key_id.in_(api_key_ids))
            .group_by(Usage.api_key_id)
            .all()
        )

        # 构建统计数据映射
        stats_map = {
            row.api_key_id: {
                "total_requests": row.requests or 0,
                "total_cost_usd": float(row.cost or 0),
                "last_used_at": row.last_used,
            }
            for row in stats_query
        }

        result = []
        for key in api_keys:
            # 从映射中获取统计，没有则使用默认值
            real_stats = stats_map.get(
                key.id,
                {"total_requests": 0, "total_cost_usd": 0.0, "last_used_at": None},
            )

            result.append(
                {
                    "id": key.id,
                    "name": key.name,
                    "key_display": key.get_display_key(),
                    "is_active": key.is_active,
                    "is_locked": key.is_locked,
                    "last_used_at": (
                        real_stats["last_used_at"].isoformat()
                        if real_stats["last_used_at"]
                        else None
                    ),
                    "created_at": key.created_at.isoformat(),
                    "total_requests": real_stats["total_requests"],
                    "total_cost_usd": real_stats["total_cost_usd"],
                    "rate_limit": key.rate_limit,
                    "force_capabilities": key.force_capabilities,
                }
            )
        return result


class CreateMyApiKeyAdapter(AuthenticatedApiAdapter):
    """创建 API 密钥的适配器"""

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        payload = context.ensure_json_body()
        try:
            request = CreateMyApiKeyRequest.model_validate(payload)
        except ValidationError as e:
            errors = e.errors()
            if errors:
                raise InvalidRequestException(translate_pydantic_error(errors[0]))
            raise InvalidRequestException("请求数据验证失败")

        return await run_in_threadpool(_create_my_api_key_sync, context.user.id, request)


@dataclass
class GetMyFullKeyAdapter(AuthenticatedApiAdapter):
    """获取 API 密钥完整密钥值的适配器"""

    key_id: str

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        user = context.user

        # 查找API密钥，确保属于当前用户
        api_key = (
            db.query(ApiKey).filter(ApiKey.id == self.key_id, ApiKey.user_id == user.id).first()
        )
        if not api_key:
            raise NotFoundException("API密钥不存在", "api_key")

        # 解密完整密钥
        if not api_key.key_encrypted:
            raise HTTPException(status_code=400, detail="该密钥没有存储完整密钥信息")

        try:
            full_key = crypto_service.decrypt(api_key.key_encrypted)
        except Exception as e:
            logger.error(f"解密API密钥失败: Key ID {self.key_id}, 错误: {e}")
            raise HTTPException(status_code=500, detail="解密密钥失败")

        logger.info(f"用户 {user.email} 查看完整API密钥: Key ID {self.key_id}")

        return {
            "key": full_key,
        }


@dataclass
class GetMyApiKeyDetailAdapter(AuthenticatedApiAdapter):
    """获取 API 密钥详情的适配器（不包含完整密钥值）"""

    key_id: str

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        user = context.user

        api_key = (
            db.query(ApiKey).filter(ApiKey.id == self.key_id, ApiKey.user_id == user.id).first()
        )
        if not api_key:
            raise NotFoundException("API密钥不存在", "api_key")

        return {
            "id": api_key.id,
            "name": api_key.name,
            "key_display": api_key.get_display_key(),
            "is_active": api_key.is_active,
            "is_locked": api_key.is_locked,
            "force_capabilities": api_key.force_capabilities,
            "rate_limit": api_key.rate_limit,
            "last_used_at": api_key.last_used_at.isoformat() if api_key.last_used_at else None,
            "expires_at": api_key.expires_at.isoformat() if api_key.expires_at else None,
            "created_at": api_key.created_at.isoformat(),
        }


@dataclass
class UpdateMyApiKeyAdapter(AuthenticatedApiAdapter):
    """更新 API 密钥基础配置的适配器"""

    key_id: str

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        payload = context.ensure_json_body()
        try:
            request = UpdateMyApiKeyRequest.model_validate(payload)
        except ValidationError as e:
            errors = e.errors()
            if errors:
                raise InvalidRequestException(translate_pydantic_error(errors[0]))
            raise InvalidRequestException("请求数据验证失败")

        return await run_in_threadpool(
            _update_my_api_key_sync, context.user.id, self.key_id, request
        )


@dataclass
class DeleteMyApiKeyAdapter(AuthenticatedApiAdapter):
    """删除 API 密钥的适配器"""

    key_id: str

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        return await run_in_threadpool(_delete_my_api_key_sync, context.user.id, self.key_id)


@dataclass
class ToggleMyApiKeyAdapter(AuthenticatedApiAdapter):
    """切换 API 密钥启用/禁用状态的适配器"""

    key_id: str

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        return await run_in_threadpool(_toggle_my_api_key_sync, context.user.id, self.key_id)


@dataclass
class ListAvailableModelsAdapter(AuthenticatedApiAdapter):
    """获取用户可用模型列表的适配器

    考虑格式转换：如果全局格式转换启用，会包含通过格式转换可访问的模型。
    这与 /v1/models API 的逻辑保持一致。
    """

    skip: int
    limit: int
    search: str | None

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        from sqlalchemy import or_

        from src.api.base.models_service import AccessRestrictions
        from src.services.system.config import SystemConfigService

        db = context.db
        user = context.user

        # 使用 AccessRestrictions 类来处理限制（与 /v1/models 逻辑一致）
        restrictions = AccessRestrictions.from_api_key_and_user(api_key=None, user=user)

        # 检查全局格式转换开关（从数据库配置读取）
        global_conversion_enabled = SystemConfigService.is_format_conversion_enabled(db)

        # 获取所有可用的 Provider ID（考虑格式转换）
        available_provider_ids = self._get_all_available_provider_ids(db, global_conversion_enabled)

        if not available_provider_ids:
            return {"models": [], "total": 0}

        # 查询所有活跃的 GlobalModel 及其关联的 Model
        id_query = (
            db.query(GlobalModel.id, GlobalModel.name, Model.provider_id)
            .join(Model, Model.global_model_id == GlobalModel.id)
            .filter(
                and_(
                    Model.provider_id.in_(available_provider_ids),
                    Model.is_active == True,
                    GlobalModel.is_active == True,
                )
            )
        )

        # 搜索过滤
        if self.search:
            search_term = f"%{self.search}%"
            id_query = id_query.filter(
                or_(
                    GlobalModel.name.ilike(search_term),
                    GlobalModel.display_name.ilike(search_term),
                )
            )

        # 获取所有匹配的记录
        all_matches = id_query.all()

        # 应用访问限制过滤
        allowed_global_model_ids = set()
        for global_model_id, model_name, provider_id in all_matches:
            # 使用 AccessRestrictions.is_model_allowed 检查模型是否可访问
            # 它会同时检查 allowed_providers 和 allowed_models
            if restrictions.is_model_allowed(model_name, provider_id):
                allowed_global_model_ids.add(global_model_id)

        # 统计总数
        total = len(allowed_global_model_ids)

        if not allowed_global_model_ids:
            return {"models": [], "total": 0}

        # 分页并获取完整的 GlobalModel 对象
        models = (
            db.query(GlobalModel)
            .filter(GlobalModel.id.in_(allowed_global_model_ids))
            .order_by(GlobalModel.name)
            .offset(self.skip)
            .limit(self.limit)
            .all()
        )

        # 查询当前用户的每模型调用次数
        user_usage_rows = (
            db.query(UserModelUsageCount.model, UserModelUsageCount.usage_count)
            .filter(UserModelUsageCount.user_id == user.id)
            .all()
        )
        user_usage_map: dict[str, int] = {row.model: row.usage_count for row in user_usage_rows}

        # 转换为响应格式（复用 PublicGlobalModelResponse schema）
        model_responses = [
            PublicGlobalModelResponse(
                id=gm.id,
                name=gm.name,
                display_name=gm.display_name,
                is_active=gm.is_active,
                default_price_per_request=gm.default_price_per_request,
                default_tiered_pricing=gm.default_tiered_pricing,
                supported_capabilities=gm.supported_capabilities,
                config=sanitize_public_global_model_config(gm.config),
                usage_count=user_usage_map.get(gm.name, 0),
            )
            for gm in models
        ]

        logger.debug(f"用户 {user.email} 可用模型: {len(model_responses)} 个")
        return PublicGlobalModelListResponse(models=model_responses, total=total)

    def _get_all_available_provider_ids(
        self, db: Session, global_conversion_enabled: bool
    ) -> set[str]:
        """
        获取所有可用的 Provider ID（考虑格式转换）

        用户模型目录需要显示通过任何客户端格式（OPENAI/CLAUDE/GEMINI）可访问的模型并集。
        与 /v1/models 逻辑一致，确保返回的 Provider 都有活跃的端点和 Key。

        优化：将 DB 查询从 6 次减少到 2 次
        - 一次性查询所有活跃端点
        - 在内存中进行格式兼容性过滤
        - 一次性查询 Key 可用性
        """
        from sqlalchemy import tuple_

        from src.api.base.models_service import get_available_provider_ids
        from src.core.api_format.conversion.compatibility import is_format_compatible
        from src.core.api_format.signature import make_signature_key
        from src.models.database import ProviderEndpoint

        # 所有 Chat/CLI endpoint signature（用于计算“可访问并集”）
        all_formats = [
            "openai:chat",
            "openai:cli",
            "openai:compact",
            "claude:chat",
            "claude:cli",
            "gemini:chat",
            "gemini:cli",
        ]

        target_pairs = [(f.split(":", 1)[0], f.split(":", 1)[1]) for f in all_formats]

        # 步骤 1：一次性查询所有活跃端点（单次 DB 查询）
        endpoint_rows = (
            db.query(
                ProviderEndpoint.provider_id,
                ProviderEndpoint.api_family,
                ProviderEndpoint.endpoint_kind,
                ProviderEndpoint.format_acceptance_config,
                Provider.enable_format_conversion,
            )
            .join(Provider, ProviderEndpoint.provider_id == Provider.id)
            .filter(
                Provider.is_active.is_(True),
                ProviderEndpoint.is_active.is_(True),
                ProviderEndpoint.api_family.isnot(None),
                ProviderEndpoint.endpoint_kind.isnot(None),
                tuple_(ProviderEndpoint.api_family, ProviderEndpoint.endpoint_kind).in_(
                    target_pairs
                ),
            )
            .all()
        )

        if not endpoint_rows:
            return set()

        # 步骤 2：在内存中对每种客户端格式进行兼容性过滤
        # 只要端点能被任意一种客户端格式访问，就将其 Provider 加入结果
        provider_to_formats: dict[str, set[str]] = {}

        for (
            provider_id,
            api_family,
            endpoint_kind,
            format_acceptance_config,
            provider_conversion_enabled,
        ) in endpoint_rows:
            if not provider_id or not api_family or not endpoint_kind:
                continue

            endpoint_format = make_signature_key(str(api_family), str(endpoint_kind))
            skip_endpoint_check = global_conversion_enabled or bool(provider_conversion_enabled)

            # 检查该端点是否能被任意客户端格式访问
            for client_format in all_formats:
                is_compatible, _, _ = is_format_compatible(
                    client_format,
                    endpoint_format,
                    format_acceptance_config,
                    is_stream=False,
                    effective_conversion_enabled=global_conversion_enabled,
                    skip_endpoint_check=skip_endpoint_check,
                )
                if is_compatible:
                    provider_to_formats.setdefault(provider_id, set()).add(endpoint_format)
                    break  # 只要有一种客户端格式能访问就够了

        if not provider_to_formats:
            return set()

        # 步骤 3：检查 Provider 是否有活跃的 Key（单次 DB 查询）
        formats = sorted({f for fmts in provider_to_formats.values() for f in fmts})
        return get_available_provider_ids(db, formats, provider_to_formats)


@dataclass
class UpdateApiKeyCapabilitiesAdapter(AuthenticatedApiAdapter):
    """更新 API Key 的强制能力配置"""

    api_key_id: str

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        from src.core.key_capabilities import CAPABILITY_DEFINITIONS, CapabilityConfigMode
        from src.models.database import AuditEventType
        from src.services.system.audit import audit_service

        db = context.db
        user = context.user
        payload = context.ensure_json_body()

        result = await run_in_threadpool(
            _update_api_key_capabilities_sync,
            user.id,
            self.api_key_id,
            payload,
        )
        logger.debug(
            f"用户 {user.id} 更新API密钥 {self.api_key_id} 的强制能力配置: {result['force_capabilities']}"
        )
        return result


class GetPreferencesAdapter(AuthenticatedApiAdapter):
    """获取用户偏好设置的适配器"""

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        preferences = PreferenceService.get_or_create_preferences(context.db, context.user.id)
        return {
            "avatar_url": preferences.avatar_url,
            "bio": preferences.bio,
            "theme": preferences.theme,
            "language": preferences.language,
            "timezone": preferences.timezone,
            "notifications": {
                "email": preferences.email_notifications,
                "usage_alerts": preferences.usage_alerts,
                "announcements": preferences.announcement_notifications,
            },
        }


class UpdatePreferencesAdapter(AuthenticatedApiAdapter):
    """更新用户偏好设置的适配器"""

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        payload = context.ensure_json_body()
        try:
            request = UpdatePreferencesRequest.model_validate(payload)
        except ValidationError as e:
            errors = e.errors()
            if errors:
                raise InvalidRequestException(translate_pydantic_error(errors[0]))
            raise InvalidRequestException("请求数据验证失败")

        return await run_in_threadpool(_update_preferences_sync, context.user.id, request)


class GetModelCapabilitySettingsAdapter(AuthenticatedApiAdapter):
    """获取用户的模型能力配置"""

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        user = context.user
        return {
            "model_capability_settings": user.model_capability_settings or {},
        }


class UpdateModelCapabilitySettingsAdapter(AuthenticatedApiAdapter):
    """更新用户的模型能力配置"""

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        from src.core.key_capabilities import CAPABILITY_DEFINITIONS, CapabilityConfigMode
        from src.models.database import AuditEventType
        from src.services.system.audit import audit_service

        payload = context.ensure_json_body()
        result, email = await run_in_threadpool(
            _update_model_capability_settings_sync,
            context.user.id,
            payload,
        )
        await UserCacheService.invalidate_user_cache(context.user.id, email)
        logger.debug(
            f"用户 {context.user.id} 更新模型能力配置: {result['model_capability_settings']}"
        )
        return result


class GetEndpointStatusAdapter(AuthenticatedApiAdapter):
    """获取端点状态（简化版，不包含敏感信息）"""

    # 类级别缓存实例（延迟初始化）
    _cache_backend = None
    _cache_ttl = 60  # 缓存60秒

    @classmethod
    async def _get_cache(cls) -> Any:
        """获取缓存后端实例（懒加载）"""
        if cls._cache_backend is None:
            from src.services.cache.backend import get_cache_backend

            cls._cache_backend = await get_cache_backend(
                name="endpoint_status",
                backend_type="auto",
                ttl=cls._cache_ttl,  # 使用 ttl 而不是 default_ttl
            )
        return cls._cache_backend

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        from src.services.health.endpoint import EndpointHealthService

        db = context.db

        # 尝试从缓存获取
        cache = await self._get_cache()
        cache_key = "endpoint_status:all"

        try:
            cached = await cache.get(cache_key)
            if cached is not None:
                return cached
        except Exception:
            pass  # 缓存失败不影响正常流程

        # 使用共享服务获取健康状态（普通用户视图）
        result = EndpointHealthService.get_endpoint_health_by_format(
            db=db,
            lookback_hours=6,
            include_admin_fields=False,  # 不包含敏感的管理员字段
            use_cache=True,
        )

        # 写入缓存
        try:
            await cache.set(cache_key, result, ttl=self._cache_ttl)
        except Exception:
            pass  # 缓存失败不影响正常流程

        return result
