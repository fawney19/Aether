"""用户管理 API 端点。"""

from __future__ import annotations

from typing import Any

from fastapi import APIRouter, Depends, HTTPException, Query, Request, status
from fastapi.concurrency import run_in_threadpool
from pydantic import ValidationError
from sqlalchemy import func
from sqlalchemy.orm import Session

from src.api.base.admin_adapter import AdminApiAdapter
from src.api.base.context import ApiRequestContext
from src.api.base.pipeline import get_pipeline
from src.config.constants import CacheTTL
from src.core.exceptions import InvalidRequestException, NotFoundException, translate_pydantic_error
from src.core.logger import logger
from src.database import get_db, get_db_context
from src.models.admin_requests import UpdateUserRequest
from src.models.api import (
    CreateApiKeyRequest,
    CreateUserRequest,
    UpdateMyApiKeyRequest,
    UserSessionResponse,
)
from src.models.database import ApiKey, User, UserRole, Wallet
from src.services.auth.session_service import SessionService
from src.services.cache.user_cache import UserCacheService
from src.services.system.config import SystemConfigService
from src.services.user.apikey import ApiKeyService
from src.services.user.bulk_cleanup import pre_clean_api_key
from src.services.user.service import UserService
from src.services.wallet import WalletService
from src.utils.cache_decorator import cache_result

router = APIRouter(prefix="/api/admin/users", tags=["Admin - Users"])
pipeline = get_pipeline()


class _WalletSentinelType:
    pass


_WALLET_SENTINEL = _WalletSentinelType()


def _serialize_user(
    db: Session,
    user: User,
    wallet: Wallet | None | _WalletSentinelType = _WALLET_SENTINEL,
) -> dict[str, Any]:
    resolved_wallet: Wallet | None
    if wallet is _WALLET_SENTINEL:
        resolved_wallet = WalletService.get_wallet(db, user_id=user.id)
    else:
        resolved_wallet = wallet
    return {
        "id": user.id,
        "email": user.email,
        "username": user.username,
        "role": user.role.value,
        "allowed_providers": user.allowed_providers,
        "allowed_api_formats": user.allowed_api_formats,
        "allowed_models": user.allowed_models,
        "rate_limit": user.rate_limit,
        "unlimited": WalletService.is_unlimited_wallet(resolved_wallet),
        "is_active": user.is_active,
        "created_at": user.created_at.isoformat(),
        "updated_at": user.updated_at.isoformat() if user.updated_at else None,
        "last_login_at": user.last_login_at.isoformat() if user.last_login_at else None,
    }


def _create_user_sync(
    request: CreateUserRequest, role: UserRole
) -> tuple[dict[str, Any], dict[str, Any]]:
    with get_db_context() as db:
        if request.unlimited:
            initial_gift_usd = None
        elif request.initial_gift_usd is not None:
            initial_gift_usd = request.initial_gift_usd
        else:
            initial_gift_usd = SystemConfigService.get_config(
                db, "default_user_initial_gift_usd", default=None
            )

        user = UserService.create_user(
            db=db,
            email=request.email,
            username=request.username,
            password=request.password,
            role=role,
            initial_gift_usd=initial_gift_usd,
            unlimited=request.unlimited,
            allowed_providers=request.allowed_providers,
            allowed_api_formats=request.allowed_api_formats,
            allowed_models=request.allowed_models,
            rate_limit=request.rate_limit,
        )
        return _serialize_user(db, user), {
            "action": "create_user",
            "target_user_id": user.id,
            "target_email": user.email,
            "target_username": user.username,
            "target_role": user.role.value,
            "initial_gift_usd": initial_gift_usd,
            "unlimited": request.unlimited,
            "is_active": user.is_active,
        }


def _update_user_sync(
    user_id: str,
    request: UpdateUserRequest,
) -> tuple[dict[str, Any], dict[str, Any], bool, str | None]:
    with get_db_context() as db:
        existing_user = UserService.get_user(db, user_id)
        if not existing_user:
            raise NotFoundException("用户不存在", "user")

        update_data = request.model_dump(exclude_unset=True)
        old_role = existing_user.role
        existing_wallet = WalletService.get_or_create_wallet(db, user=existing_user)
        unlimited_before = WalletService.is_unlimited_wallet(existing_wallet)

        requested_unlimited = update_data.pop("unlimited", None)

        if "role" in update_data and update_data["role"]:
            if not hasattr(update_data["role"], "value"):
                update_data["role"] = UserRole[update_data["role"].upper()]

        user = UserService.update_user(db, user_id, **update_data)
        if not user:
            raise NotFoundException("用户不存在", "user")

        changed_fields = list(update_data.keys())
        if requested_unlimited is not None:
            wallet = WalletService.get_or_create_wallet(db, user=user)
            if wallet is not None:
                WalletService.set_wallet_limit_mode(
                    db,
                    wallet=wallet,
                    limit_mode="unlimited" if requested_unlimited else "finite",
                )
            changed_fields.append("unlimited")

        role_changed = "role" in update_data and update_data["role"] != old_role
        response = _serialize_user(db, user)
        return (
            response,
            {
                "action": "update_user",
                "target_user_id": user.id,
                "updated_fields": changed_fields,
                "role_before": old_role.value if old_role else None,
                "role_after": user.role.value,
                "unlimited_before": unlimited_before,
                "unlimited_after": (
                    requested_unlimited if requested_unlimited is not None else unlimited_before
                ),
                "is_active": user.is_active,
            },
            role_changed,
            user.email,
        )


def _delete_user_sync(user_id: str) -> tuple[dict[str, Any], dict[str, Any], str | None]:
    with get_db_context() as db:
        user = UserService.get_user(db, user_id)
        if not user:
            raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="用户不存在")

        if user.role == UserRole.ADMIN:
            admin_count = int(
                db.query(func.count(User.id)).filter(User.role == UserRole.ADMIN).scalar() or 0
            )
            if admin_count <= 1:
                raise InvalidRequestException("不能删除最后一个管理员账户")

        try:
            success = UserService.delete_user(db, user_id)
        except ValueError as exc:
            raise InvalidRequestException(str(exc)) from exc
        if not success:
            raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="用户不存在")

        return (
            {"message": "用户删除成功"},
            {
                "action": "delete_user",
                "target_user_id": user.id,
                "target_email": user.email,
                "target_role": user.role.value,
            },
            user.email,
        )


def _list_user_sessions_sync(user_id: str) -> tuple[list[dict[str, Any]], dict[str, Any]]:
    with get_db_context() as db:
        user = UserService.get_user(db, user_id)
        if not user:
            raise NotFoundException("用户不存在", "user")
        sessions = SessionService.list_user_sessions(db, user_id=user_id)
        return (
            [UserSessionResponse.from_db(s) for s in sessions],
            {
                "action": "list_user_sessions",
                "target_user_id": user_id,
                "session_count": len(sessions),
            },
        )


def _revoke_user_session_sync(
    user_id: str,
    session_id: str,
    admin_user_id: str,
) -> tuple[dict[str, Any], dict[str, Any]]:
    with get_db_context() as db:
        user = UserService.get_user(db, user_id)
        if not user:
            raise NotFoundException("用户不存在", "user")
        session = SessionService.get_session_for_user(db, user_id=user_id, session_id=session_id)
        if not session:
            raise NotFoundException("会话不存在", "session")
        SessionService.revoke_session(
            db,
            session=session,
            reason="admin_session_revoked",
            audit_user_id=admin_user_id,
        )
        return (
            {"message": "用户设备已强制下线"},
            {
                "action": "revoke_user_session",
                "target_user_id": user_id,
                "session_id": session_id,
            },
        )


def _revoke_all_user_sessions_sync(
    user_id: str, admin_user_id: str
) -> tuple[dict[str, Any], dict[str, Any]]:
    with get_db_context() as db:
        user = UserService.get_user(db, user_id)
        if not user:
            raise NotFoundException("用户不存在", "user")
        revoked_count = SessionService.revoke_all_user_sessions(
            db,
            user_id=user_id,
            reason="admin_revoke_all_sessions",
        )
        return (
            {"message": "已强制下线该用户所有设备", "revoked_count": revoked_count},
            {
                "action": "revoke_all_user_sessions",
                "target_user_id": user_id,
                "admin_user_id": admin_user_id,
                "revoked_count": revoked_count,
            },
        )


def _create_user_key_sync(
    user_id: str,
    key_data: CreateApiKeyRequest,
) -> tuple[dict[str, Any], dict[str, Any], str | None]:
    with get_db_context() as db:
        user = db.query(User).filter(User.id == user_id).first()
        if not user:
            raise NotFoundException("用户不存在", "user")

        api_key, plain_key = ApiKeyService.create_api_key(
            db=db,
            user_id=user_id,
            name=key_data.name,
            allowed_providers=key_data.allowed_providers,
            allowed_models=key_data.allowed_models,
            rate_limit=key_data.rate_limit,
            expire_days=key_data.expire_days,
            is_standalone=False,
        )

        return (
            {
                "id": api_key.id,
                "key": plain_key,
                "name": api_key.name,
                "key_display": api_key.get_display_key(),
                "rate_limit": api_key.rate_limit,
                "expires_at": api_key.expires_at.isoformat() if api_key.expires_at else None,
                "created_at": api_key.created_at.isoformat(),
                "message": "API Key创建成功，请妥善保存完整密钥",
            },
            {
                "action": "create_user_api_key",
                "target_user_id": user_id,
                "key_id": api_key.id,
            },
            user.email,
        )


def _delete_user_key_sync(user_id: str, key_id: str) -> tuple[dict[str, Any], dict[str, Any]]:
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
            raise NotFoundException("API Key不存在或不属于该用户", "api_key")

        pre_clean_api_key(db, api_key.id)
        db.delete(api_key)

        return {"message": "API Key已删除"}, {
            "action": "delete_user_api_key",
            "target_user_id": user_id,
            "key_id": key_id,
        }


def _update_user_key_sync(
    user_id: str,
    key_id: str,
    request: UpdateMyApiKeyRequest,
) -> tuple[dict[str, Any], dict[str, Any]]:
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
            raise NotFoundException("API Key不存在或不属于该用户", "api_key")

        update_data = request.model_dump(exclude_unset=True)
        if "rate_limit" in update_data and update_data["rate_limit"] is None:
            update_data["rate_limit"] = 0

        updated_key = ApiKeyService.update_api_key(db, key_id, **update_data)
        if not updated_key:
            raise NotFoundException("API Key不存在或不属于该用户", "api_key")

        return (
            {
                "id": updated_key.id,
                "name": updated_key.name,
                "key_display": updated_key.get_display_key(),
                "is_active": updated_key.is_active,
                "is_locked": updated_key.is_locked,
                "total_requests": updated_key.total_requests,
                "total_cost_usd": float(updated_key.total_cost_usd or 0),
                "rate_limit": updated_key.rate_limit,
                "expires_at": (
                    updated_key.expires_at.isoformat() if updated_key.expires_at else None
                ),
                "last_used_at": (
                    updated_key.last_used_at.isoformat() if updated_key.last_used_at else None
                ),
                "created_at": updated_key.created_at.isoformat(),
                "message": "API Key更新成功",
            },
            {
                "action": "update_user_api_key",
                "target_user_id": user_id,
                "key_id": key_id,
                "updated_fields": list(update_data.keys()),
            },
        )


def _toggle_user_key_lock_sync(
    user_id: str,
    key_id: str,
) -> tuple[dict[str, Any], dict[str, Any]]:
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
            raise NotFoundException("API Key不存在或不属于该用户", "api_key")

        api_key.is_locked = not api_key.is_locked
        db.commit()
        db.refresh(api_key)

        return (
            {
                "id": api_key.id,
                "is_locked": api_key.is_locked,
                "message": f"API密钥已{'锁定' if api_key.is_locked else '解锁'}",
            },
            {
                "action": "toggle_user_api_key_lock",
                "target_user_id": user_id,
                "key_id": key_id,
                "new_lock_status": "locked" if api_key.is_locked else "unlocked",
            },
        )


# 管理员端点
@router.post("")
async def create_user_endpoint(request: Request, db: Session = Depends(get_db)) -> Any:
    """
    创建用户

    创建新用户账号（管理员专用）。

    **请求体**:
    - `email`: 邮箱地址
    - `username`: 用户名
    - `password`: 密码
    - `role`: 角色（user/admin）
    - `initial_gift_usd`: 初始赠款（USD，可选）
    - `unlimited`: 是否无限制
    """
    adapter = AdminCreateUserAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("")
async def list_users(
    request: Request,
    skip: int = Query(0, ge=0, description="跳过记录数"),
    limit: int = Query(100, ge=1, le=1000, description="返回记录数"),
    role: str | None = Query(None, description="按角色筛选（user/admin）"),
    is_active: bool | None = Query(None, description="按状态筛选"),
    db: Session = Depends(get_db),
) -> Any:
    """
    获取用户列表

    分页获取用户列表，支持按角色和状态筛选。

    **返回字段**: id, email, username, role, unlimited, is_active, created_at 等
    """
    adapter = AdminListUsersAdapter(skip=skip, limit=limit, role=role, is_active=is_active)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/{user_id}")
async def get_user(user_id: str, request: Request, db: Session = Depends(get_db)) -> Any:
    """
    获取用户详情

    获取指定用户的详细信息。

    **路径参数**:
    - `user_id`: 用户 ID (UUID)
    """
    adapter = AdminGetUserAdapter(user_id=user_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/{user_id}/sessions")
async def list_user_sessions(user_id: str, request: Request, db: Session = Depends(get_db)) -> Any:
    """获取用户登录设备列表。"""
    adapter = AdminListUserSessionsAdapter(user_id=user_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.delete("/{user_id}/sessions/{session_id}")
async def revoke_user_session(
    user_id: str,
    session_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """强制下线用户的单个设备会话。"""
    adapter = AdminRevokeUserSessionAdapter(user_id=user_id, session_id=session_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.delete("/{user_id}/sessions")
async def revoke_all_user_sessions(
    user_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """强制下线用户的全部设备会话。"""
    adapter = AdminRevokeAllUserSessionsAdapter(user_id=user_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.put("/{user_id}")
async def update_user(
    user_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    更新用户信息

    更新指定用户的信息，包括角色、无限制开关、权限等。

    **路径参数**:
    - `user_id`: 用户 ID (UUID)

    **请求体** (均为可选):
    - `email`: 邮箱地址
    - `username`: 用户名
    - `role`: 角色
    - `unlimited`: 是否无限制
    - `is_active`: 是否启用
    - `allowed_providers`: 允许的提供商列表
    - `allowed_models`: 允许的模型列表
    """
    adapter = AdminUpdateUserAdapter(user_id=user_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.delete("/{user_id}")
async def delete_user(user_id: str, request: Request, db: Session = Depends(get_db)) -> None:
    """
    删除用户

    永久删除指定用户。不能删除最后一个管理员账户。

    **路径参数**:
    - `user_id`: 用户 ID (UUID)
    """
    adapter = AdminDeleteUserAdapter(user_id=user_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/{user_id}/api-keys")
async def get_user_api_keys(
    user_id: str,
    request: Request,
    is_active: bool | None = Query(None, description="按状态筛选"),
    db: Session = Depends(get_db),
) -> Any:
    """
    获取用户的 API 密钥列表

    获取指定用户的所有 API 密钥（不包括独立密钥）。

    **路径参数**:
    - `user_id`: 用户 ID (UUID)
    """
    adapter = AdminGetUserKeysAdapter(user_id=user_id, is_active=is_active)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/{user_id}/api-keys")
async def create_user_api_key(
    user_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    为用户创建 API 密钥

    为指定用户创建新的 API 密钥。

    **路径参数**:
    - `user_id`: 用户 ID (UUID)

    **请求体**:
    - `name`: 密钥名称
    - `allowed_providers`: 允许的提供商（可选）
    - `allowed_models`: 允许的模型（可选）
    - `rate_limit`: 速率限制（可选）
    - `expire_days`: 过期天数（可选）

    **返回**: 包含完整密钥值的响应（仅此一次显示）
    """
    adapter = AdminCreateUserKeyAdapter(user_id=user_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.delete("/{user_id}/api-keys/{key_id}")
async def delete_user_api_key(
    user_id: str,
    key_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    删除用户的 API 密钥

    删除指定用户的指定 API 密钥。

    **路径参数**:
    - `user_id`: 用户 ID (UUID)
    - `key_id`: 密钥 ID
    """
    adapter = AdminDeleteUserKeyAdapter(user_id=user_id, key_id=key_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.put("/{user_id}/api-keys/{key_id}")
async def update_user_api_key(
    user_id: str,
    key_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    更新用户的 API 密钥

    更新指定用户的普通 API 密钥基础配置。

    **路径参数**:
    - `user_id`: 用户 ID (UUID)
    - `key_id`: 密钥 ID
    """
    adapter = AdminUpdateUserKeyAdapter(user_id=user_id, key_id=key_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.patch("/{user_id}/api-keys/{key_id}/lock")
async def toggle_user_api_key_lock(
    user_id: str,
    key_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    切换用户 API 密钥锁定状态

    仅支持普通用户 Key（非独立 Key）。

    **路径参数**:
    - `user_id`: 用户 ID (UUID)
    - `key_id`: 密钥 ID
    """
    adapter = AdminToggleUserKeyLockAdapter(user_id=user_id, key_id=key_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/{user_id}/api-keys/{key_id}/full-key")
async def get_user_api_key_full_key(
    user_id: str,
    key_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> Any:
    """
    获取用户 API 密钥完整值

    仅支持普通用户 Key（非独立 Key）。

    **路径参数**:
    - `user_id`: 用户 ID (UUID)
    - `key_id`: 密钥 ID
    """
    adapter = AdminGetUserKeyFullKeyAdapter(user_id=user_id, key_id=key_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


# ============== 管理员适配器实现 ==============


class AdminCreateUserAdapter(AdminApiAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        payload = context.ensure_json_body()
        try:
            request = CreateUserRequest.model_validate(payload)
        except ValidationError as e:
            errors = e.errors()
            if errors:
                raise InvalidRequestException(translate_pydantic_error(errors[0]))
            raise InvalidRequestException("请求数据验证失败")
        try:
            role = (
                request.role if hasattr(request.role, "value") else UserRole[request.role.upper()]
            )
        except (KeyError, AttributeError):
            raise InvalidRequestException("角色参数不合法")

        try:
            response, audit_meta = await run_in_threadpool(_create_user_sync, request, role)
        except ValueError as exc:
            raise InvalidRequestException(str(exc)) from exc
        context.add_audit_metadata(**audit_meta)
        return response


class AdminListUsersAdapter(AdminApiAdapter):
    def __init__(self, skip: int, limit: int, role: str | None, is_active: bool | None):
        self.skip = skip
        self.limit = limit
        self.role = role
        self.is_active = is_active

    @cache_result(
        key_prefix="admin:users:list",
        ttl=CacheTTL.USER,
        user_specific=False,
        vary_by=["skip", "limit", "role", "is_active"],
    )
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        try:
            role_enum = UserRole[self.role.upper()] if self.role else None
        except KeyError as exc:
            raise InvalidRequestException("角色参数不合法") from exc
        users = UserService.list_users(db, self.skip, self.limit, role_enum, self.is_active)
        wallets_by_user_id = WalletService.get_wallets_by_user_ids(db, [user.id for user in users])
        return [_serialize_user(db, user, wallets_by_user_id.get(user.id)) for user in users]


class AdminGetUserAdapter(AdminApiAdapter):
    def __init__(self, user_id: str):
        self.user_id = user_id

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        user = UserService.get_user(db, self.user_id)
        if not user:
            raise NotFoundException("用户不存在", "user")

        context.add_audit_metadata(
            action="get_user_detail",
            target_user_id=user.id,
            target_role=user.role.value,
            include_history=bool(user.last_login_at),
        )

        return _serialize_user(db, user)


class AdminListUserSessionsAdapter(AdminApiAdapter):
    def __init__(self, user_id: str):
        self.user_id = user_id

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        response, audit_meta = await run_in_threadpool(_list_user_sessions_sync, self.user_id)
        context.add_audit_metadata(**audit_meta)
        return response


class AdminRevokeUserSessionAdapter(AdminApiAdapter):
    def __init__(self, user_id: str, session_id: str):
        self.user_id = user_id
        self.session_id = session_id

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        admin_user_id = context.user.id if context.user else ""
        response, audit_meta = await run_in_threadpool(
            _revoke_user_session_sync, self.user_id, self.session_id, admin_user_id
        )
        context.add_audit_metadata(**audit_meta)
        return response


class AdminRevokeAllUserSessionsAdapter(AdminApiAdapter):
    def __init__(self, user_id: str):
        self.user_id = user_id

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        admin_user_id = context.user.id if context.user else ""
        response, audit_meta = await run_in_threadpool(
            _revoke_all_user_sessions_sync, self.user_id, admin_user_id
        )
        context.add_audit_metadata(**audit_meta)
        return response


class AdminUpdateUserAdapter(AdminApiAdapter):
    def __init__(self, user_id: str):
        self.user_id = user_id

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        payload = context.ensure_json_body()
        try:
            request = UpdateUserRequest.model_validate(payload)
        except ValidationError as e:
            errors = e.errors()
            if errors:
                raise InvalidRequestException(translate_pydantic_error(errors[0]))
            raise InvalidRequestException("请求数据验证失败")

        response, audit_meta, role_changed, user_email = await run_in_threadpool(
            _update_user_sync,
            self.user_id,
            request,
        )
        if user_email:
            await UserCacheService.invalidate_user_cache(self.user_id, user_email)
        if role_changed:
            from src.services.usage.service import UsageService

            await UsageService.clear_user_heatmap_cache(self.user_id)
        context.add_audit_metadata(**audit_meta)
        return response


class AdminDeleteUserAdapter(AdminApiAdapter):
    def __init__(self, user_id: str):
        self.user_id = user_id

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        response, audit_meta, user_email = await run_in_threadpool(_delete_user_sync, self.user_id)
        if user_email:
            await UserCacheService.invalidate_user_cache(self.user_id, user_email)
        context.add_audit_metadata(**audit_meta)
        return response


class AdminGetUserKeysAdapter(AdminApiAdapter):
    """获取用户的API Keys"""

    def __init__(self, user_id: str, is_active: bool | None):
        self.user_id = user_id
        self.is_active = is_active

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db

        # 验证用户存在
        user = db.query(User).filter(User.id == self.user_id).first()
        if not user:
            raise NotFoundException("用户不存在", "user")

        # 获取用户的Keys（不包括独立Keys）
        api_keys = ApiKeyService.list_user_api_keys(
            db=db, user_id=self.user_id, is_active=self.is_active
        )

        context.add_audit_metadata(
            action="list_user_api_keys",
            target_user_id=self.user_id,
            total=len(api_keys),
        )

        return {
            "api_keys": [
                {
                    "id": key.id,
                    "name": key.name,
                    "key_display": key.get_display_key(),
                    "is_active": key.is_active,
                    "is_locked": key.is_locked,
                    "total_requests": key.total_requests,
                    "total_cost_usd": float(key.total_cost_usd or 0),
                    "rate_limit": key.rate_limit,
                    "expires_at": key.expires_at.isoformat() if key.expires_at else None,
                    "last_used_at": key.last_used_at.isoformat() if key.last_used_at else None,
                    "created_at": key.created_at.isoformat(),
                }
                for key in api_keys
            ],
            "total": len(api_keys),
            "user_email": user.email,
            "username": user.username,
        }


class AdminCreateUserKeyAdapter(AdminApiAdapter):
    """为用户创建API Key"""

    def __init__(self, user_id: str):
        self.user_id = user_id

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        payload = context.ensure_json_body()
        try:
            key_data = CreateApiKeyRequest.model_validate(payload)
        except ValidationError as e:
            errors = e.errors()
            if errors:
                raise InvalidRequestException(translate_pydantic_error(errors[0]))
            raise InvalidRequestException("请求数据验证失败")

        response, audit_meta, user_email = await run_in_threadpool(
            _create_user_key_sync,
            self.user_id,
            key_data,
        )
        if user_email:
            logger.info(
                "管理员为用户创建API Key: 用户 {}, Key ID {}", user_email, audit_meta["key_id"]
            )
        context.add_audit_metadata(**audit_meta)
        return response


class AdminDeleteUserKeyAdapter(AdminApiAdapter):
    """删除用户的API Key"""

    def __init__(self, user_id: str, key_id: str):
        self.user_id = user_id
        self.key_id = key_id

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        response, audit_meta = await run_in_threadpool(
            _delete_user_key_sync,
            self.user_id,
            self.key_id,
        )
        context.add_audit_metadata(**audit_meta)
        return response


class AdminUpdateUserKeyAdapter(AdminApiAdapter):
    """更新用户的普通 API Key"""

    def __init__(self, user_id: str, key_id: str):
        self.user_id = user_id
        self.key_id = key_id

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        payload = context.ensure_json_body()
        try:
            request = UpdateMyApiKeyRequest.model_validate(payload)
        except ValidationError as e:
            errors = e.errors()
            if errors:
                raise InvalidRequestException(translate_pydantic_error(errors[0]))
            raise InvalidRequestException("请求数据验证失败")

        response, audit_meta = await run_in_threadpool(
            _update_user_key_sync,
            self.user_id,
            self.key_id,
            request,
        )
        context.add_audit_metadata(**audit_meta)
        return response


class AdminToggleUserKeyLockAdapter(AdminApiAdapter):
    """切换用户普通 API Key 的锁定状态"""

    def __init__(self, user_id: str, key_id: str):
        self.user_id = user_id
        self.key_id = key_id

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        response, audit_meta = await run_in_threadpool(
            _toggle_user_key_lock_sync,
            self.user_id,
            self.key_id,
        )
        context.add_audit_metadata(**audit_meta)
        return response


class AdminGetUserKeyFullKeyAdapter(AdminApiAdapter):
    """获取用户普通 API Key 的完整密钥"""

    def __init__(self, user_id: str, key_id: str):
        self.user_id = user_id
        self.key_id = key_id

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        from src.core.crypto import crypto_service

        db = context.db

        api_key = (
            db.query(ApiKey)
            .filter(
                ApiKey.id == self.key_id,
                ApiKey.user_id == self.user_id,
                ApiKey.is_standalone == False,  # 仅普通用户Key
            )
            .first()
        )
        if not api_key:
            raise NotFoundException("API Key不存在或不属于该用户", "api_key")
        if not api_key.key_encrypted:
            raise InvalidRequestException("该密钥没有存储完整密钥信息")

        try:
            full_key = crypto_service.decrypt(api_key.key_encrypted)
        except Exception as exc:
            logger.error(
                f"解密用户API密钥失败: 用户ID {self.user_id}, Key ID {self.key_id}, 错误: {exc}"
            )
            raise HTTPException(status_code=500, detail="解密密钥失败")

        context.add_audit_metadata(
            action="view_user_api_key_full_key",
            target_user_id=self.user_id,
            key_id=self.key_id,
        )

        return {"key": full_key}
