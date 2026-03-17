"""用户个人 API 端点。"""

from __future__ import annotations

from dataclasses import dataclass
from datetime import date, datetime, timedelta, timezone
from typing import Any

from fastapi import APIRouter, Depends, HTTPException, Query, Request
from fastapi.concurrency import run_in_threadpool
from pydantic import ValidationError
from sqlalchemy import and_, func
from sqlalchemy.orm import Session

from src.api.base.authenticated_adapter import AuthenticatedApiAdapter
from src.api.base.context import ApiRequestContext
from src.api.base.pipeline import get_pipeline
from src.config.constants import CacheTTL
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
    UpdateApiKeyProvidersRequest,
    UpdateMyApiKeyRequest,
    UpdatePreferencesRequest,
    UpdateProfileRequest,
    UpdateSessionLabelRequest,
    UserSessionResponse,
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
from src.services.auth.session_service import SessionService
from src.services.cache.user_cache import UserCacheService
from src.services.system.config import SystemConfigService
from src.services.system.time_range import TimeRangeParams
from src.services.usage.query import input_context_expr
from src.services.usage.service import UsageService
from src.services.user.apikey import ApiKeyService
from src.services.user.bulk_cleanup import pre_clean_api_key
from src.services.user.preference import PreferenceService
from src.services.wallet import WalletService
from src.utils.cache_decorator import cache_result

router = APIRouter(prefix="/api/users/me", tags=["User Profile"])
pipeline = get_pipeline()


def _calculate_token_cache_hit_rate(total_input_context: int, cache_read_tokens: int) -> float:
    """计算缓存命中率。

    Args:
        total_input_context: 已归一化的总输入上下文 token 数（由 query.py 按 API 格式精确计算）。
        cache_read_tokens: 缓存读取 token 数。
    """
    context = max(0, int(total_input_context))
    cached = max(0, int(cache_read_tokens))
    if context == 0:
        return 0.0
    return round(cached / context * 100, 2)


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
    current_session_id: str | None = None,
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
            if user.verify_password(request.new_password):
                raise InvalidRequestException("新密码不能与当前密码相同")

        policy_level = SystemConfigService.get_password_policy_level(db)
        valid, error_msg = PasswordValidator.validate(request.new_password, policy=policy_level)
        if not valid:
            raise InvalidRequestException(error_msg or "密码格式无效")

        user.set_password(request.new_password)
        SessionService.revoke_all_user_sessions(
            db,
            user_id=user.id,
            reason="password_changed",
            exclude_session_id=current_session_id,
        )
        user.updated_at = datetime.now(timezone.utc)
        action = "修改" if has_password else "设置"
        return {"message": f"密码{action}成功"}, user.email, action


def _list_user_sessions_sync(user_id: str, current_session_id: str | None) -> list[dict[str, Any]]:
    with get_db_context() as db:
        sessions = SessionService.list_user_sessions(db, user_id=user_id)
        return [
            UserSessionResponse.from_db(s, current_session_id=current_session_id) for s in sessions
        ]


def _update_session_label_sync(
    user_id: str,
    session_id: str,
    request: UpdateSessionLabelRequest,
    current_session_id: str | None,
) -> dict[str, Any]:
    with get_db_context() as db:
        session = SessionService.get_session_for_user(db, user_id=user_id, session_id=session_id)
        if not session:
            raise NotFoundException("会话不存在", "session")
        SessionService.update_session_label(session, request.device_label)
        return UserSessionResponse.from_db(session, current_session_id=current_session_id)


def _revoke_session_sync(
    user_id: str,
    session_id: str,
) -> dict[str, Any]:
    with get_db_context() as db:
        session = SessionService.get_session_for_user(db, user_id=user_id, session_id=session_id)
        if not session:
            raise NotFoundException("会话不存在", "session")
        SessionService.revoke_session(
            db,
            session=session,
            reason="user_session_revoked",
            audit_user_id=user_id,
        )
        return {"message": "设备已退出登录"}


def _revoke_other_sessions_sync(
    user_id: str,
    current_session_id: str | None,
) -> dict[str, Any]:
    with get_db_context() as db:
        revoked_count = SessionService.revoke_all_user_sessions(
            db,
            user_id=user_id,
            reason="logout_other_sessions",
            exclude_session_id=current_session_id,
        )
        return {"message": "其他设备已退出登录", "revoked_count": revoked_count}


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
            "allowed_providers": updated.allowed_providers,
            "force_capabilities": updated.force_capabilities,
            "rate_limit": updated.rate_limit,
            "last_used_at": updated.last_used_at.isoformat() if updated.last_used_at else None,
            "expires_at": updated.expires_at.isoformat() if updated.expires_at else None,
            "created_at": updated.created_at.isoformat(),
            "message": "API密钥已更新",
        }


def _update_api_key_providers_sync(
    user_id: str,
    api_key_id: str,
    request: UpdateApiKeyProvidersRequest,
) -> dict[str, str]:
    with get_db_context() as db:
        api_key = (
            db.query(ApiKey).filter(ApiKey.id == api_key_id, ApiKey.user_id == user_id).first()
        )
        if not api_key:
            raise NotFoundException("API密钥不存在")
        if api_key.is_locked:
            raise ForbiddenException("该密钥已被管理员锁定，无法修改")

        if request.allowed_providers is not None and len(request.allowed_providers) > 0:
            provider_ids = [cfg.provider_id for cfg in request.allowed_providers]
            valid = (
                db.query(Provider.id)
                .filter(Provider.id.in_(provider_ids), Provider.is_active.is_(True))
                .all()
            )
            valid_ids = {p.id for p in valid}
            invalid = set(provider_ids) - valid_ids
            if invalid:
                raise InvalidRequestException(f"无效的提供商ID: {', '.join(invalid)}")

        api_key.allowed_providers = (
            [cfg.provider_id for cfg in request.allowed_providers]
            if request.allowed_providers is not None
            else None
        )
        api_key.updated_at = datetime.now(timezone.utc)
        return {"message": "API密钥可用提供商已更新"}


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
            default_provider_id=request.default_provider_id,
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


def _build_time_range_params(
    start_date: date | None,
    end_date: date | None,
    preset: str | None,
    timezone_name: str | None,
    tz_offset_minutes: int | None,
) -> TimeRangeParams | None:
    if not preset and start_date is None and end_date is None:
        return None
    try:
        return TimeRangeParams(
            start_date=start_date,
            end_date=end_date,
            preset=preset,
            timezone=timezone_name,
            tz_offset_minutes=tz_offset_minutes or 0,
        ).validate_and_resolve()
    except Exception as exc:
        raise HTTPException(status_code=400, detail=str(exc)) from exc


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


@router.get("/sessions")
async def list_my_sessions(request: Request, db: Session = Depends(get_db)) -> Any:
    """列出当前用户的登录会话。"""
    adapter = ListMySessionsAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.delete("/sessions/others")
async def revoke_other_sessions(request: Request, db: Session = Depends(get_db)) -> Any:
    """退出当前设备之外的所有登录会话。"""
    adapter = RevokeOtherSessionsAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.patch("/sessions/{session_id}")
async def update_my_session_label(
    session_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """修改某个登录设备的显示名称。"""
    adapter = UpdateMySessionLabelAdapter(session_id=session_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.delete("/sessions/{session_id}")
async def revoke_my_session(
    session_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """退出指定登录会话。"""
    adapter = RevokeMySessionAdapter(session_id=session_id)
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


# ============== 使用统计 ==============


@router.get("/usage")
async def get_my_usage(
    request: Request,
    start_date: date | None = Query(None, description="开始日期（YYYY-MM-DD）"),
    end_date: date | None = Query(None, description="结束日期（YYYY-MM-DD）"),
    preset: str | None = Query(None, description="时间预设（today/last7days 等）"),
    timezone_name: str | None = Query(None, alias="timezone"),
    tz_offset_minutes: int | None = Query(None, description="时区偏移（分钟）"),
    search: str | None = Query(None, description="搜索关键词（密钥名、模型名）"),
    limit: int = Query(100, ge=1, le=200, description="每页记录数，默认100，最大200"),
    offset: int = Query(0, ge=0, le=2000, description="偏移量，用于分页，最大2000"),
    db: Session = Depends(get_db),
) -> Any:
    """
    获取使用统计

    获取当前用户的 API 使用统计数据，包括总量汇总、按模型/提供商分组统计及详细记录。

    **返回字段**:
    - `total_requests`: 总请求数
    - `total_tokens`: 总 Token 数
    - `total_cost`: 总成本（USD）
    - `summary_by_model`: 按模型分组统计（含 `cache_read_tokens`、`cache_hit_rate`）
    - `summary_by_provider`: 按提供商分组统计（含 `cache_read_tokens`、`cache_hit_rate`）
    - `records`: 详细使用记录列表
    - `pagination`: 分页信息
    """
    time_range = _build_time_range_params(
        start_date, end_date, preset, timezone_name, tz_offset_minutes
    )
    adapter = GetUsageAdapter(time_range=time_range, search=search, limit=limit, offset=offset)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/usage/active")
async def get_my_active_requests(
    request: Request,
    ids: str | None = Query(None, description="请求 ID 列表，逗号分隔"),
    db: Session = Depends(get_db),
) -> Any:
    """
    获取活跃请求状态

    查询正在进行中的请求状态，用于前端轮询更新流式请求的进度。

    **查询参数**:
    - `ids`: 要查询的请求 ID 列表，逗号分隔
    """
    adapter = GetActiveRequestsAdapter(ids=ids)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/usage/interval-timeline")
async def get_my_interval_timeline(
    request: Request,
    hours: int = Query(24, ge=1, le=720, description="分析最近多少小时的数据"),
    limit: int = Query(2000, ge=100, le=20000, description="最大返回数据点数量"),
    db: Session = Depends(get_db),
) -> Any:
    """
    获取请求间隔时间线

    获取请求间隔时间线数据，用于散点图展示请求分布情况。

    **返回**: 包含时间戳和间隔时间的数据点列表
    """
    adapter = GetMyIntervalTimelineAdapter(hours=hours, limit=limit)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/usage/heatmap")
async def get_my_activity_heatmap(
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    获取活动热力图数据

    获取过去 365 天的活动热力图数据，用于展示每日使用频率。
    此接口有 5 分钟缓存。

    **返回**: 包含日期和请求数量的数据列表
    """
    adapter = GetMyActivityHeatmapAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/providers")
async def list_available_providers(request: Request, db: Session = Depends(get_db)) -> Any:
    """
    获取可用提供商列表

    获取当前用户可用的所有提供商及其模型信息。

    **返回字段**: id, name, display_name, endpoints, models 等
    """
    adapter = ListAvailableProvidersAdapter()
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


# ============== API密钥与提供商关联 ==============


# UpdateApiKeyProvidersRequest 已移至 src/models/api.py


@router.put("/api-keys/{api_key_id}/providers")
async def update_api_key_providers(
    api_key_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    更新 API 密钥可用提供商

    设置指定 API 密钥可以使用哪些提供商。未设置时使用用户默认权限。

    **路径参数**:
    - `api_key_id`: API 密钥 ID

    **请求体**:
    - `allowed_providers`: 允许的提供商 ID 列表
    """
    adapter = UpdateApiKeyProvidersAdapter(api_key_id=api_key_id)
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

        current_session_id = getattr(context.request.state, "user_session_id", None)
        result, email, action = await run_in_threadpool(
            _change_password_sync,
            user.id,
            request,
            current_session_id,
        )
        logger.info(f"用户{action}密码: {email}")
        return result


class ListMySessionsAdapter(AuthenticatedApiAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        current_session_id = getattr(context.request.state, "user_session_id", None)
        return await run_in_threadpool(
            _list_user_sessions_sync,
            context.user.id,
            current_session_id,
        )


class UpdateMySessionLabelAdapter(AuthenticatedApiAdapter):
    def __init__(self, session_id: str):
        self.session_id = session_id

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        payload = context.ensure_json_body()
        try:
            request = UpdateSessionLabelRequest.model_validate(payload)
        except ValidationError as e:
            errors = e.errors()
            if errors:
                raise InvalidRequestException(translate_pydantic_error(errors[0]))
            raise InvalidRequestException("请求数据验证失败")

        current_session_id = getattr(context.request.state, "user_session_id", None)
        return await run_in_threadpool(
            _update_session_label_sync,
            context.user.id,
            self.session_id,
            request,
            current_session_id,
        )


class RevokeMySessionAdapter(AuthenticatedApiAdapter):
    def __init__(self, session_id: str):
        self.session_id = session_id

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        return await run_in_threadpool(
            _revoke_session_sync,
            context.user.id,
            self.session_id,
        )


class RevokeOtherSessionsAdapter(AuthenticatedApiAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        current_session_id = getattr(context.request.state, "user_session_id", None)
        return await run_in_threadpool(
            _revoke_other_sessions_sync,
            context.user.id,
            current_session_id,
        )


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
                    "allowed_providers": key.allowed_providers,
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
            "allowed_providers": api_key.allowed_providers,
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
class GetUsageAdapter(AuthenticatedApiAdapter):
    """获取用户使用统计的适配器"""

    time_range: TimeRangeParams | None
    search: str | None = None
    limit: int = 100
    offset: int = 0

    @cache_result(
        key_prefix="user:usage:records",
        ttl=3,  # 使用记录页强调实时性，避免 15s 缓存导致列表滞后
        user_specific=True,
        vary_by=[
            "time_range.start_date",
            "time_range.end_date",
            "time_range.preset",
            "time_range.timezone",
            "time_range.tz_offset_minutes",
            "search",
            "limit",
            "offset",
        ],
    )
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        from sqlalchemy import or_
        from sqlalchemy.orm import load_only

        from src.models.database import ProviderEndpoint
        from src.utils.database_helpers import escape_like_pattern, safe_truncate_escaped

        db = context.db
        user = context.user
        start_utc = end_utc = None
        if self.time_range:
            start_utc, end_utc = self.time_range.to_utc_datetime_range()
        summary_list = UsageService.get_usage_summary(
            db=db,
            user_id=user.id,
            start_date=start_utc,
            end_date=end_utc,
            group_by=None,
        )

        # 过滤掉 unknown/pending provider 的记录（请求未到达任何提供商）
        filtered_summary = [
            item
            for item in summary_list
            if item.get("provider") not in ("unknown", "pending", None)
        ]

        total_requests = 0
        total_input_tokens = 0
        total_output_tokens = 0
        total_tokens = 0
        total_cost = 0.0
        total_actual_cost = 0.0
        model_summary = {}
        provider_summary = {}
        for item in filtered_summary:
            total_requests += item["requests"]
            total_input_tokens += item["input_tokens"]
            total_output_tokens += item["output_tokens"]
            total_tokens += item["total_tokens"]
            total_cost += item["total_cost_usd"]
            if user.role == UserRole.ADMIN:
                total_actual_cost += item.get("actual_total_cost_usd", 0.0)

            model_name = item["model"]
            base_stats = {
                "model": model_name,
                "requests": 0,
                "input_tokens": 0,
                "output_tokens": 0,
                "total_tokens": 0,
                "cache_read_tokens": 0,
                "cache_creation_tokens": 0,
                "total_input_context": 0,
                "cache_hit_rate": 0.0,
                "total_cost_usd": 0.0,
            }
            # 管理员可以看到真实成本
            if user.role == UserRole.ADMIN:
                base_stats["actual_total_cost_usd"] = 0.0

            stats = model_summary.setdefault(model_name, base_stats)
            stats["requests"] += item["requests"]
            stats["input_tokens"] += item["input_tokens"]
            stats["output_tokens"] += item["output_tokens"]
            stats["total_tokens"] += item["total_tokens"]
            stats["cache_read_tokens"] += int(item.get("cache_read_tokens", 0) or 0)
            stats["cache_creation_tokens"] += int(item.get("cache_creation_tokens", 0) or 0)
            stats["total_input_context"] += int(item.get("total_input_context", 0) or 0)
            stats["total_cost_usd"] += item["total_cost_usd"]
            # 管理员可以看到真实成本
            if user.role == UserRole.ADMIN:
                stats["actual_total_cost_usd"] += item.get("actual_total_cost_usd", 0.0)

            provider_name = item["provider"]
            provider_base_stats = {
                "provider": provider_name,
                "requests": 0,
                "total_tokens": 0,
                "output_tokens": 0,
                "cache_read_tokens": 0,
                "cache_creation_tokens": 0,
                "total_input_context": 0,
                "total_cost_usd": 0.0,
                "success_count": 0,
                "total_response_time_ms": 0.0,
                "response_time_count": 0,
            }
            provider_stats = provider_summary.setdefault(provider_name, provider_base_stats)
            provider_stats["requests"] += item["requests"]
            provider_stats["total_tokens"] += item["total_tokens"]
            provider_stats["output_tokens"] += item.get("output_tokens", 0) or 0
            provider_stats["cache_read_tokens"] += int(item.get("cache_read_tokens", 0) or 0)
            provider_stats["cache_creation_tokens"] += int(
                item.get("cache_creation_tokens", 0) or 0
            )
            provider_stats["total_input_context"] += int(item.get("total_input_context", 0) or 0)
            provider_stats["total_cost_usd"] += item["total_cost_usd"]
            provider_stats["success_count"] += int(item.get("success_count", 0) or 0)
            success_response_time_count = int(item.get("success_response_time_count", 0) or 0)
            if success_response_time_count > 0:
                provider_stats["total_response_time_ms"] += float(
                    item.get("success_response_time_sum_ms", 0.0) or 0.0
                )
                provider_stats["response_time_count"] += success_response_time_count

        for model_stats in model_summary.values():
            model_stats["cache_hit_rate"] = _calculate_token_cache_hit_rate(
                total_input_context=int(model_stats.get("total_input_context", 0) or 0),
                cache_read_tokens=int(model_stats.get("cache_read_tokens", 0) or 0),
            )

        summary_by_model = sorted(model_summary.values(), key=lambda x: x["requests"], reverse=True)
        summary_by_provider = []
        for provider_stats in provider_summary.values():
            avg_response_time_ms = (
                provider_stats["total_response_time_ms"] / provider_stats["response_time_count"]
                if provider_stats["response_time_count"] > 0
                else 0
            )
            success_rate = (
                (provider_stats["success_count"] / provider_stats["requests"] * 100)
                if provider_stats["requests"] > 0
                else 100
            )
            summary_by_provider.append(
                {
                    "provider": provider_stats["provider"],
                    "requests": provider_stats["requests"],
                    "total_tokens": provider_stats["total_tokens"],
                    "total_input_context": provider_stats["total_input_context"],
                    "output_tokens": provider_stats["output_tokens"],
                    "cache_read_tokens": provider_stats["cache_read_tokens"],
                    "cache_creation_tokens": provider_stats["cache_creation_tokens"],
                    "cache_hit_rate": _calculate_token_cache_hit_rate(
                        total_input_context=int(provider_stats.get("total_input_context", 0) or 0),
                        cache_read_tokens=int(provider_stats.get("cache_read_tokens", 0) or 0),
                    ),
                    "total_cost_usd": provider_stats["total_cost_usd"],
                    "success_rate": round(success_rate, 2),
                    "avg_response_time_ms": round(avg_response_time_ms, 2),
                }
            )
        summary_by_provider = sorted(summary_by_provider, key=lambda x: x["requests"], reverse=True)

        # 按 api_format 聚合统计（独立查询，因为 get_usage_summary 按 provider+model 分组无此维度）
        api_format_query = db.query(
            Usage.api_format,
            func.count(Usage.id).label("request_count"),
            func.sum(Usage.total_tokens).label("total_tokens"),
            func.sum(Usage.cache_read_input_tokens).label("cache_read_tokens"),
            func.sum(input_context_expr()).label("total_input_context"),
            func.sum(Usage.output_tokens).label("output_tokens"),
            func.sum(Usage.cache_creation_input_tokens).label("cache_creation_tokens"),
            func.sum(Usage.total_cost_usd).label("total_cost_usd"),
            func.avg(Usage.response_time_ms).label("avg_response_time_ms"),
        ).filter(
            Usage.user_id == user.id,
            Usage.status.notin_(["pending", "streaming"]),
            Usage.provider_name.notin_(["unknown", "pending"]),
            Usage.api_format.isnot(None),
        )
        if start_utc and end_utc:
            api_format_query = api_format_query.filter(
                Usage.created_at >= start_utc, Usage.created_at < end_utc
            )
        api_format_stats = (
            api_format_query.group_by(Usage.api_format).order_by(func.count(Usage.id).desc()).all()
        )
        summary_by_api_format = [
            {
                "api_format": api_format or "unknown",
                "request_count": count,
                "total_tokens": int(total_tokens or 0),
                "total_input_context": int(total_input_context or 0),
                "output_tokens": int(output_tokens or 0),
                "cache_read_tokens": int(cache_read_tokens or 0),
                "cache_creation_tokens": int(cache_creation_tokens or 0),
                "cache_hit_rate": _calculate_token_cache_hit_rate(
                    total_input_context=total_input_context,
                    cache_read_tokens=cache_read_tokens,
                ),
                "total_cost_usd": float(total_cost_usd or 0),
                "avg_response_time_ms": float(avg_response_time_ms or 0),
            }
            for (
                api_format,
                count,
                total_tokens,
                cache_read_tokens,
                total_input_context,
                output_tokens,
                cache_creation_tokens,
                total_cost_usd,
                avg_response_time_ms,
            ) in api_format_stats
        ]

        query = (
            db.query(Usage, ApiKey, ProviderEndpoint)
            .outerjoin(ApiKey, Usage.api_key_id == ApiKey.id)
            .outerjoin(ProviderEndpoint, Usage.provider_endpoint_id == ProviderEndpoint.id)
            .filter(Usage.user_id == user.id)
        )
        if start_utc and end_utc:
            query = query.filter(Usage.created_at >= start_utc, Usage.created_at < end_utc)

        # 通用搜索：密钥名、模型名
        # 支持空格分隔的组合搜索，多个关键词之间是 AND 关系
        if self.search and self.search.strip():
            keywords = [kw for kw in self.search.strip().split() if kw][:10]
            for keyword in keywords:
                escaped = safe_truncate_escaped(escape_like_pattern(keyword), 100)
                search_pattern = f"%{escaped}%"
                query = query.filter(
                    or_(
                        ApiKey.name.ilike(search_pattern, escape="\\"),
                        Usage.model.ilike(search_pattern, escape="\\"),
                    )
                )

        # 计算总数用于分页
        # Perf: avoid Query.count() building a subquery selecting many columns
        total_records = int(query.with_entities(func.count(Usage.id)).scalar() or 0)

        # Perf: do not load large request/response columns for list view
        query = query.options(
            load_only(
                Usage.id,
                Usage.user_id,
                Usage.api_key_id,
                Usage.provider_name,
                Usage.model,
                Usage.target_model,
                Usage.input_tokens,
                Usage.output_tokens,
                Usage.total_tokens,
                Usage.total_cost_usd,
                Usage.response_time_ms,
                Usage.first_byte_time_ms,
                Usage.is_stream,
                Usage.status,
                Usage.created_at,
                Usage.cache_creation_input_tokens,
                Usage.cache_read_input_tokens,
                Usage.status_code,
                Usage.error_message,
                Usage.api_format,
                Usage.endpoint_api_format,
                Usage.has_format_conversion,
                Usage.input_price_per_1m,
                Usage.output_price_per_1m,
                Usage.cache_creation_price_per_1m,
                Usage.cache_read_price_per_1m,
                Usage.actual_total_cost_usd,
                Usage.rate_multiplier,
            ),
            load_only(ApiKey.id, ApiKey.name, ApiKey.key_encrypted),
            load_only(ProviderEndpoint.id, ProviderEndpoint.api_format),
        )
        usage_records = (
            query.order_by(Usage.created_at.desc()).offset(self.offset).limit(self.limit).all()
        )

        # 复用 summary 聚合中的成功请求响应时间，避免额外 AVG SQL
        total_success_response_time_ms = sum(
            float(item.get("success_response_time_sum_ms", 0.0) or 0.0) for item in summary_list
        )
        total_success_response_count = sum(
            int(item.get("success_response_time_count", 0) or 0) for item in summary_list
        )
        avg_response_time = (
            total_success_response_time_ms / total_success_response_count / 1000.0
            if total_success_response_count > 0
            else 0.0
        )
        wallet = WalletService.get_wallet(db, user_id=user.id)

        # 构建响应数据
        response_data = {
            "total_requests": total_requests,
            "total_input_tokens": total_input_tokens,
            "total_output_tokens": total_output_tokens,
            "total_tokens": total_tokens,
            "total_cost": total_cost,
            "avg_response_time": avg_response_time,
            "billing": WalletService.serialize_wallet_summary(wallet),
            "summary_by_model": summary_by_model,
            "summary_by_provider": summary_by_provider,
            "summary_by_api_format": summary_by_api_format,
            # 分页信息
            "pagination": {
                "total": total_records,
                "limit": self.limit,
                "offset": self.offset,
                "has_more": self.offset + self.limit < total_records,
            },
            "records": self._build_usage_records(
                usage_records, is_admin=(user.role == UserRole.ADMIN)
            ),
        }

        # 管理员可以看到真实成本
        if user.role == UserRole.ADMIN:
            response_data["total_actual_cost"] = total_actual_cost
            # 为每条记录添加真实成本和倍率信息
            for i, (r, _, _) in enumerate(usage_records):
                # 确保字段有值，避免前端显示 -
                actual_cost = (
                    r.actual_total_cost_usd if r.actual_total_cost_usd is not None else 0.0
                )
                rate_mult = r.rate_multiplier if r.rate_multiplier is not None else 1.0
                response_data["records"][i]["actual_cost"] = actual_cost
                response_data["records"][i]["rate_multiplier"] = rate_mult

        return response_data

    def _build_usage_records(self, usage_records: list, is_admin: bool = False) -> list:
        """构建使用记录列表，包含格式转换信息的回填逻辑

        Args:
            usage_records: 使用记录列表
            is_admin: 是否为管理员，管理员可以看到模型映射信息
        """
        from src.core.api_format.metadata import can_passthrough_endpoint
        from src.core.api_format.signature import normalize_signature_key

        records = []
        for r, api_key, endpoint in usage_records:
            # 格式转换追踪（兼容历史数据：尽量回填可展示信息）
            api_format = r.api_format
            endpoint_api_format = r.endpoint_api_format or (
                endpoint.api_format if endpoint else None
            )

            has_format_conversion = r.has_format_conversion
            if has_format_conversion is None:
                # 新模式：仅对 signature 进行推断（历史旧值保持 False，避免解析失败）
                client_raw = str(api_format or "").strip()
                endpoint_raw = str(endpoint_api_format or "").strip()
                if client_raw and endpoint_raw and ":" in client_raw and ":" in endpoint_raw:
                    client_fmt = normalize_signature_key(client_raw)
                    endpoint_fmt = normalize_signature_key(endpoint_raw)
                    has_format_conversion = not can_passthrough_endpoint(client_fmt, endpoint_fmt)
                else:
                    has_format_conversion = False

            records.append(
                {
                    "id": r.id,
                    "model": r.model,
                    # 只有管理员可以看到模型映射信息，普通用户只能看到请求的模型
                    "target_model": r.target_model if is_admin else None,
                    "api_format": api_format,
                    "endpoint_api_format": endpoint_api_format,
                    "has_format_conversion": bool(has_format_conversion),
                    "input_tokens": r.input_tokens,
                    "output_tokens": r.output_tokens,
                    "total_tokens": r.total_tokens,
                    "cost": float(r.total_cost_usd or 0),
                    "response_time_ms": r.response_time_ms,
                    "first_byte_time_ms": r.first_byte_time_ms,
                    "is_stream": r.is_stream,
                    "status": r.status,  # 请求状态: pending, streaming, completed, failed
                    "created_at": r.created_at.isoformat(),
                    "cache_creation_input_tokens": r.cache_creation_input_tokens,
                    "cache_read_input_tokens": r.cache_read_input_tokens,
                    "status_code": r.status_code,
                    "error_message": r.error_message,
                    "input_price_per_1m": r.input_price_per_1m,
                    "output_price_per_1m": r.output_price_per_1m,
                    "cache_creation_price_per_1m": r.cache_creation_price_per_1m,
                    "cache_read_price_per_1m": r.cache_read_price_per_1m,
                    "api_key": (
                        {
                            "id": str(api_key.id),
                            "name": api_key.name,
                            "display": api_key.get_display_key(),
                        }
                        if api_key
                        else None
                    ),
                }
            )
        return records


@dataclass
class GetActiveRequestsAdapter(AuthenticatedApiAdapter):
    """轻量级活跃请求状态查询适配器（用于用户端轮询）"""

    ids: str | None = None

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        from src.services.usage import UsageService

        db = context.db
        user = context.user
        id_list = None
        if self.ids:
            id_list = [id.strip() for id in self.ids.split(",") if id.strip()]
            if not id_list:
                return {"requests": []}

        requests = UsageService.get_active_requests_status(
            db=db,
            ids=id_list,
            user_id=user.id,
            maintain_status=True,
        )
        return {"requests": requests}


@dataclass
class GetMyIntervalTimelineAdapter(AuthenticatedApiAdapter):
    """获取当前用户的请求间隔时间线适配器"""

    hours: int
    limit: int

    @cache_result(
        key_prefix="user:usage:interval_timeline",
        ttl=CacheTTL.ADMIN_USAGE_AGGREGATION,
        user_specific=True,
        vary_by=["hours", "limit"],
    )
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        user = context.user

        result = UsageService.get_interval_timeline(
            db=db,
            hours=self.hours,
            limit=self.limit,
            user_id=str(user.id),
        )

        return result


class GetMyActivityHeatmapAdapter(AuthenticatedApiAdapter):
    """获取用户活动热力图数据的适配器（带 Redis 缓存）"""

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        user = context.user
        result = await UsageService.get_cached_heatmap(
            db=context.db,
            user_id=user.id,
            include_actual_cost=user.role == UserRole.ADMIN,
        )
        context.add_audit_metadata(action="activity_heatmap")
        return result


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
                config=gm.config,
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


class ListAvailableProvidersAdapter(AuthenticatedApiAdapter):
    """获取可用提供商列表的适配器"""

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        from sqlalchemy.orm import selectinload

        db = context.db

        # 使用 selectinload 预加载所有关联数据，避免 N+1 查询
        providers = (
            db.query(Provider)
            .options(
                selectinload(Provider.endpoints),
                selectinload(Provider.models).selectinload(Model.global_model),
            )
            .filter(Provider.is_active.is_(True))
            .all()
        )

        result = []
        for provider in providers:
            # 直接使用预加载的 endpoints，无需额外查询
            endpoints_data = [
                {
                    "id": ep.id,
                    "api_format": ep.api_format if ep.api_format else None,
                    "base_url": ep.base_url,
                    "is_active": ep.is_active,
                }
                for ep in provider.endpoints
            ]

            models_data = []
            # 直接使用预加载的 models，无需额外查询
            direct_models = provider.models
            for model in direct_models:
                global_model = model.global_model
                display_name = (
                    global_model.display_name if global_model else model.provider_model_name
                )
                unified_name = global_model.name if global_model else model.provider_model_name
                models_data.append(
                    {
                        "id": model.id,
                        "name": unified_name,
                        "display_name": display_name,
                        "input_price_per_1m": model.input_price_per_1m,
                        "output_price_per_1m": model.output_price_per_1m,
                        "cache_creation_price_per_1m": model.cache_creation_price_per_1m,
                        "cache_read_price_per_1m": model.cache_read_price_per_1m,
                        "supports_vision": model.supports_vision,
                        "supports_function_calling": model.supports_function_calling,
                        "supports_streaming": model.supports_streaming,
                    }
                )

            result.append(
                {
                    "id": provider.id,
                    "name": provider.name,
                    "description": provider.description,
                    "provider_priority": provider.provider_priority,
                    "endpoints": endpoints_data,
                    "models": models_data,
                }
            )
        return result


@dataclass
class UpdateApiKeyProvidersAdapter(AuthenticatedApiAdapter):
    """更新 API 密钥可用提供商的适配器"""

    api_key_id: str

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        user = context.user
        payload = context.ensure_json_body()
        try:
            request = UpdateApiKeyProvidersRequest.model_validate(payload)
        except ValidationError as e:
            errors = e.errors()
            if errors:
                raise InvalidRequestException(translate_pydantic_error(errors[0]))
            raise InvalidRequestException("请求数据验证失败")

        result = await run_in_threadpool(
            _update_api_key_providers_sync,
            user.id,
            self.api_key_id,
            request,
        )
        logger.debug(f"用户 {user.id} 更新API密钥 {self.api_key_id} 的可用提供商")
        return result


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
            "default_provider_id": preferences.default_provider_id,
            "default_provider": (
                preferences.default_provider.name if preferences.default_provider else None
            ),
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
