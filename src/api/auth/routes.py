"""
认证相关API端点
"""

from __future__ import annotations

import uuid
from datetime import datetime, timezone
from typing import Any

from fastapi import APIRouter, Depends, HTTPException, Request, Response, status
from fastapi.security import HTTPBearer
from pydantic import ValidationError
from sqlalchemy.orm import Session

from src.api.base.adapter import ApiAdapter, ApiMode
from src.api.base.authenticated_adapter import AuthenticatedApiAdapter
from src.api.base.context import ApiRequestContext
from src.api.base.pipeline import get_pipeline
from src.config import config
from src.core.exceptions import InvalidRequestException
from src.core.logger import logger
from src.database import get_db
from src.models.api import (
    LoginRequest,
    LoginResponse,
    LogoutResponse,
    RefreshTokenResponse,
    RegisterRequest,
    RegisterResponse,
    RegistrationSettingsResponse,
    SendVerificationCodeRequest,
    SendVerificationCodeResponse,
    VerificationStatusRequest,
    VerificationStatusResponse,
    VerifyEmailRequest,
    VerifyEmailResponse,
)
from src.models.database import AuditEventType, User, UserRole
from src.services.auth.refresh_cookie import (
    clear_refresh_token_cookie,
    error_response_with_cleared_cookie,
    set_refresh_token_cookie,
)
from src.services.auth.service import AuthService
from src.services.auth.session_service import SessionService
from src.services.cache.user_cache import UserCacheService
from src.services.email import EmailSenderService, EmailVerificationService
from src.services.rate_limit.ip_limiter import IPRateLimiter
from src.services.system.audit import AuditService
from src.services.system.config import SystemConfigService
from src.services.user.service import UserService
from src.services.wallet import WalletService
from src.utils.request_utils import get_client_ip, get_user_agent


def validate_email_suffix(db: Session, email: str) -> tuple[bool, str | None]:
    """
    验证邮箱后缀是否允许注册

    Args:
        db: 数据库会话
        email: 邮箱地址

    Returns:
        (是否允许, 错误信息)
    """
    # 获取邮箱后缀限制配置
    mode = SystemConfigService.get_config(db, "email_suffix_mode", default="none")

    if mode == "none":
        return True, None

    # 获取邮箱后缀列表
    suffix_list = SystemConfigService.get_config(db, "email_suffix_list", default=[])
    if not suffix_list:
        # 没有配置后缀列表时，不限制
        return True, None

    # 确保 suffix_list 是列表类型
    if isinstance(suffix_list, str):
        suffix_list = [s.strip().lower() for s in suffix_list.split(",") if s.strip()]

    # 获取邮箱后缀
    if "@" not in email:
        return False, "邮箱格式无效"

    email_suffix = email.split("@")[1].lower()

    if mode == "whitelist":
        # 白名单模式：只允许列出的后缀
        if email_suffix not in suffix_list:
            return False, f"该邮箱后缀不在允许列表中，仅支持: {', '.join(suffix_list)}"
    elif mode == "blacklist":
        # 黑名单模式：拒绝列出的后缀
        if email_suffix in suffix_list:
            return False, f"该邮箱后缀 ({email_suffix}) 不允许注册"

    return True, None


def _issue_session_bound_tokens(
    *,
    db: Session,
    user_id: str,
    user_role: UserRole,
    user_created_at: Any,
    user: User | None,
    request: Request,
) -> tuple[str, str, str]:
    session_id = str(uuid.uuid4())
    access_token = AuthService.create_access_token(
        data={
            "user_id": user_id,
            "role": user_role.value,
            "created_at": user_created_at.isoformat() if user_created_at else None,
            "session_id": session_id,
        }
    )
    refresh_token = AuthService.create_refresh_token(
        data={
            "user_id": user_id,
            "created_at": user_created_at.isoformat() if user_created_at else None,
            "session_id": session_id,
            "jti": str(uuid.uuid4()),
        }
    )

    if user is None:
        user = db.query(User).filter(User.id == user_id).first()
    if user is None:
        raise HTTPException(status_code=status.HTTP_401_UNAUTHORIZED, detail="用户不存在")

    client_device_id = SessionService.extract_client_device_id(request)
    client_context = SessionService.build_client_context(
        client_device_id=client_device_id,
        client_ip=get_client_ip(request),
        user_agent=get_user_agent(request),
        headers=dict(request.headers),
    )
    SessionService.create_session(
        db,
        user=user,
        session_id=session_id,
        refresh_token=refresh_token,
        expires_at=AuthService.get_refresh_token_expiry(),
        client=client_context,
    )
    return session_id, access_token, refresh_token


async def _logout_with_refresh_cookie_fallback(
    request: Request,
    db: Session,
) -> dict[str, Any] | None:
    refresh_token_value = request.cookies.get(config.auth_refresh_cookie_name)
    if not refresh_token_value:
        return None

    try:
        token_payload = await AuthService.verify_token(refresh_token_value, token_type="refresh")
        user_id = token_payload.get("user_id")
        session_id = token_payload.get("session_id")
        if not user_id or not session_id:
            return None

        client_device_id = SessionService.extract_client_device_id(request)
        session = SessionService.get_session_for_user(
            db,
            user_id=str(user_id),
            session_id=str(session_id),
        )
        user = db.query(User).filter(User.id == user_id).first()

        if session and not session.is_revoked and not session.is_expired:
            SessionService.assert_session_device_matches(session, client_device_id)
            SessionService.revoke_session(
                db,
                session=session,
                reason="user_logout",
                audit_user_id=str(user_id),
                ip_address=get_client_ip(request),
                user_agent=get_user_agent(request),
            )

        AuditService.log_event(
            db=db,
            event_type=AuditEventType.LOGOUT,
            description=f"User logged out via refresh cookie: {user.email if user else user_id}",
            user_id=str(user_id),
            ip_address=get_client_ip(request),
            user_agent=get_user_agent(request),
            metadata={
                "user_id": str(user_id),
                "email": user.email if user else None,
                "logout_via": "refresh_cookie",
            },
        )
        db.commit()
        request.state.tx_committed_by_route = True
        logger.info("用户通过 refresh cookie 登出成功: {}", user.email if user else user_id)
        return LogoutResponse(message="登出成功", success=True).model_dump()
    except HTTPException as exc:
        logger.info("Refresh cookie logout fallback skipped: {}", exc.detail)
        return None
    except Exception as exc:
        logger.warning("Refresh cookie logout fallback failed: {}", exc)
        return None


router = APIRouter(prefix="/api/auth", tags=["Authentication"])
security = HTTPBearer()
pipeline = get_pipeline()


# API端点
@router.get("/registration-settings", response_model=RegistrationSettingsResponse)
async def registration_settings(request: Request, db: Session = Depends(get_db)) -> Any:
    """
    获取注册相关配置

    返回系统注册配置，包括是否开放注册、是否需要邮箱验证等。
    此接口为公开接口，无需认证。
    """
    adapter = AuthRegistrationSettingsAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/settings")
async def auth_settings(request: Request, db: Session = Depends(get_db)) -> Any:
    """
    获取认证设置

    返回系统支持的认证方式，如本地认证、LDAP 认证等。
    前端据此判断显示哪些登录选项。此接口为公开接口，无需认证。
    """
    adapter = AuthSettingsAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/login", response_model=LoginResponse, response_model_exclude_none=True)
async def login(request: Request, response: Response, db: Session = Depends(get_db)) -> Any:
    """
    用户登录

    使用邮箱和密码登录，成功后返回 JWT access_token。

    - **access_token**: 用于后续 API 调用，有效期 24 小时
    - **refresh_token**: 通过 HttpOnly Cookie 下发，不出现在响应体

    速率限制: 20次/分钟/IP
    """
    adapter = AuthLoginAdapter()
    result = await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)
    refresh_token = result.pop("_refresh_token", None) if isinstance(result, dict) else None
    if refresh_token:
        set_refresh_token_cookie(response, refresh_token)
    return result


@router.post("/refresh", response_model=RefreshTokenResponse, response_model_exclude_none=True)
async def refresh_token(request: Request, response: Response, db: Session = Depends(get_db)) -> Any:
    """
    刷新访问令牌

    使用 HttpOnly Cookie 中的 refresh_token 获取新的 access_token。
    原 refresh_token 刷新后失效并轮换 Cookie。
    """
    adapter = AuthRefreshAdapter()
    try:
        result = await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)
    except HTTPException as exc:
        return error_response_with_cleared_cookie(exc)
    refresh_token_value = result.pop("_refresh_token", None) if isinstance(result, dict) else None
    if refresh_token_value:
        set_refresh_token_cookie(response, refresh_token_value)
    return result


@router.post("/register", response_model=RegisterResponse)
async def register(request: Request, db: Session = Depends(get_db)) -> Any:
    """
    用户注册

    创建新用户账号。需要系统开放注册功能。
    如果系统开启了邮箱验证，需先通过 /send-verification-code 和 /verify-email 完成邮箱验证。

    速率限制: 10次/分钟/IP
    """
    adapter = AuthRegisterAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/me")
async def get_current_user_info(request: Request, db: Session = Depends(get_db)) -> Any:
    """
    获取当前用户信息

    返回当前登录用户的基本信息，包括邮箱、用户名、角色、钱包信息等。
    需要 Bearer Token 认证。
    """
    adapter = AuthCurrentUserAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/logout", response_model=LogoutResponse)
async def logout(request: Request, response: Response, db: Session = Depends(get_db)) -> Any:
    """
    用户登出

    将当前 Token 加入黑名单，使其失效。
    即使 access_token 过期，也始终清除 refresh_token cookie。
    """
    try:
        adapter = AuthLogoutAdapter()
        result = await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)
    except HTTPException as exc:
        fallback_result = await _logout_with_refresh_cookie_fallback(request, db)
        if fallback_result is not None:
            clear_refresh_token_cookie(response)
            return fallback_result
        # 即使认证失败（如 access_token 过期），也要清除 cookie 防止会话残留
        return error_response_with_cleared_cookie(exc)
    clear_refresh_token_cookie(response)
    return result


@router.post("/send-verification-code", response_model=SendVerificationCodeResponse)
async def send_verification_code(request: Request, db: Session = Depends(get_db)) -> None:
    """
    发送邮箱验证码

    向指定邮箱发送验证码，用于注册前的邮箱验证。
    验证码有效期 5 分钟，同一邮箱 60 秒内只能发送一次。

    速率限制: 5次/分钟/IP
    """
    adapter = AuthSendVerificationCodeAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/verify-email", response_model=VerifyEmailResponse)
async def verify_email(request: Request, db: Session = Depends(get_db)) -> Any:
    """
    验证邮箱验证码

    验证邮箱收到的验证码是否正确。
    验证成功后，邮箱会被标记为已验证状态，可用于注册。

    速率限制: 20次/分钟/IP
    """
    adapter = AuthVerifyEmailAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/verification-status", response_model=VerificationStatusResponse)
async def verification_status(request: Request, db: Session = Depends(get_db)) -> Any:
    """
    查询邮箱验证状态

    查询指定邮箱的验证状态，包括是否有待验证的验证码、是否已验证等。

    速率限制: 20次/分钟/IP
    """
    adapter = AuthVerificationStatusAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


# ============== 适配器实现 ==============


class AuthPublicAdapter(ApiAdapter):
    mode = ApiMode.PUBLIC

    def authorize(self, context: ApiRequestContext) -> None:  # type: ignore[override]
        return None


class AuthLoginAdapter(AuthPublicAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        payload = context.ensure_json_body()

        try:
            login_request = LoginRequest.model_validate(payload)
        except ValidationError as exc:
            errors = []
            for error in exc.errors():
                field = " -> ".join(str(x) for x in error["loc"])
                errors.append(f"{field}: {error['msg']}")
            raise InvalidRequestException("输入验证失败: " + "; ".join(errors))

        client_ip = get_client_ip(context.request)
        user_agent = get_user_agent(context.request)

        # IP 速率限制检查（登录接口：5次/分钟）
        allowed, remaining, reset_after = await IPRateLimiter.check_limit(client_ip, "login")
        if not allowed:
            logger.warning(f"登录请求超过速率限制: IP={client_ip}, 剩余={remaining}")
            raise HTTPException(
                status_code=status.HTTP_429_TOO_MANY_REQUESTS,
                detail=f"登录请求过于频繁，请在 {reset_after} 秒后重试",
            )

        authenticated_user = await AuthService.authenticate_user_threadsafe(
            db, login_request.email, login_request.password, login_request.auth_type
        )
        if not authenticated_user:
            AuditService.log_login_attempt(
                db=db,
                email=login_request.email,
                success=False,
                ip_address=client_ip,
                user_agent=user_agent,
                error_reason="邮箱或密码错误",
            )
            db.commit()
            context.request.state.tx_committed_by_route = True
            raise HTTPException(status_code=status.HTTP_401_UNAUTHORIZED, detail="邮箱或密码错误")

        db_user = db.query(User).filter(User.id == authenticated_user.user_id).first()
        if db_user is None:
            raise HTTPException(status_code=status.HTTP_401_UNAUTHORIZED, detail="用户不存在")

        _, access_token, refresh_token = _issue_session_bound_tokens(
            db=db,
            user_id=authenticated_user.user_id,
            user_role=authenticated_user.role,
            user_created_at=authenticated_user.created_at,
            user=db_user,
            request=context.request,
        )
        AuditService.log_login_attempt(
            db=db,
            email=login_request.email,
            success=True,
            ip_address=client_ip,
            user_agent=user_agent,
            user_id=authenticated_user.user_id,
        )
        db_user.last_login_at = datetime.now(timezone.utc)
        db.commit()
        context.request.state.tx_committed_by_route = True
        await UserCacheService.invalidate_user_cache(
            authenticated_user.user_id,
            authenticated_user.email or "",
        )
        logger.info("用户登录成功: {} (ID: {})", login_request.email, authenticated_user.user_id)

        response = LoginResponse(
            access_token=access_token,
            token_type="bearer",
            expires_in=86400,
            user_id=authenticated_user.user_id,
            email=authenticated_user.email,
            username=authenticated_user.username,
            role=authenticated_user.role.value,
        ).model_dump()
        response["_refresh_token"] = refresh_token
        return response


class AuthRefreshAdapter(AuthPublicAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        db = context.db
        if context.request.headers.get("content-length") not in (None, "0"):
            payload = context.ensure_json_body()
            if payload:
                raise HTTPException(
                    status_code=status.HTTP_400_BAD_REQUEST,
                    detail="刷新接口不接受请求体，请使用 Cookie",
                )

        refresh_token_value = context.request.cookies.get(config.auth_refresh_cookie_name)
        if not refresh_token_value:
            raise HTTPException(status_code=status.HTTP_401_UNAUTHORIZED, detail="缺少刷新令牌")
        try:
            token_payload = await AuthService.verify_token(
                refresh_token_value, token_type="refresh"
            )
            user_id = token_payload.get("user_id")
            session_id = token_payload.get("session_id")
            if not user_id or not session_id:
                raise HTTPException(
                    status_code=status.HTTP_401_UNAUTHORIZED, detail="无效的刷新令牌"
                )

            user = db.query(User).filter(User.id == user_id).first()
            if not user:
                raise HTTPException(
                    status_code=status.HTTP_401_UNAUTHORIZED, detail="无效的刷新令牌"
                )
            if not user.is_active:
                raise HTTPException(status_code=status.HTTP_403_FORBIDDEN, detail="用户已禁用")
            if user.is_deleted:
                raise HTTPException(
                    status_code=status.HTTP_403_FORBIDDEN, detail="用户不存在或已禁用"
                )

            if not AuthService.token_identity_matches_user(token_payload, user):
                raise HTTPException(
                    status_code=status.HTTP_401_UNAUTHORIZED, detail="无效的刷新令牌"
                )

            client_device_id = SessionService.extract_client_device_id(context.request)
            session, is_prev = SessionService.validate_refresh_session(
                db,
                user_id=str(user_id),
                session_id=str(session_id),
                refresh_token=refresh_token_value,
                ip_address=get_client_ip(context.request),
                user_agent=get_user_agent(context.request),
            )
            if session:
                SessionService.assert_session_device_matches(session, client_device_id)
            if not session:
                db.commit()
                context.request.state.tx_committed_by_route = True
                raise HTTPException(
                    status_code=status.HTTP_401_UNAUTHORIZED, detail="登录会话已失效，请重新登录"
                )

            new_access_token = AuthService.create_access_token(
                data={
                    "user_id": user.id,
                    "role": user.role.value,
                    "created_at": user.created_at.isoformat() if user.created_at else None,
                    "session_id": session.id,
                }
            )
            new_refresh_token: str | None = None
            if not is_prev:
                new_refresh_token = AuthService.create_refresh_token(
                    data={
                        "user_id": user.id,
                        "created_at": user.created_at.isoformat() if user.created_at else None,
                        "session_id": session.id,
                        "jti": str(uuid.uuid4()),
                    }
                )
                SessionService.rotate_refresh_token(
                    session,
                    refresh_token=new_refresh_token,
                    expires_at=AuthService.get_refresh_token_expiry(),
                    client_ip=get_client_ip(context.request),
                    user_agent=get_user_agent(context.request),
                )
            db.commit()
            context.request.state.tx_committed_by_route = True
            logger.info(f"令牌刷新成功: user_id={user.id}")
            response = RefreshTokenResponse(
                access_token=new_access_token,
                token_type="bearer",
                expires_in=86400,
            ).model_dump()
            if new_refresh_token:
                response["_refresh_token"] = new_refresh_token
            return response
        except HTTPException:
            raise
        except Exception as exc:
            logger.error(f"刷新令牌失败: {exc}")
            raise HTTPException(status_code=status.HTTP_401_UNAUTHORIZED, detail="刷新令牌失败")


class AuthRegistrationSettingsAdapter(AuthPublicAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        """公开返回注册相关配置"""
        db = context.db

        enable_registration = SystemConfigService.get_config(
            db, "enable_registration", default=False
        )
        require_verification = SystemConfigService.get_config(
            db, "require_email_verification", default=False
        )
        email_configured = EmailSenderService.is_smtp_configured(db)

        # 如果邮箱服务未配置，强制 require_email_verification 为 False
        if not email_configured:
            require_verification = False

        return RegistrationSettingsResponse(
            enable_registration=bool(enable_registration),
            require_email_verification=bool(require_verification),
            email_configured=email_configured,
            password_policy_level=SystemConfigService.get_password_policy_level(db),
        ).model_dump()


class AuthSettingsAdapter(AuthPublicAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        """公开返回认证设置"""
        from src.core.modules.hooks import AUTH_GET_METHODS, get_hook_dispatcher

        db = context.db
        dispatcher = get_hook_dispatcher()
        auth_methods = await dispatcher.dispatch(AUTH_GET_METHODS, db=db)

        # 从钩子返回的认证方法列表中解析各模块状态
        ldap_info = next((m for m in auth_methods if m.get("type") == "ldap"), None)
        ldap_enabled = ldap_info is not None
        ldap_exclusive = ldap_info.get("exclusive", False) if ldap_info else False

        return {
            "local_enabled": not ldap_exclusive,
            "ldap_enabled": ldap_enabled,
            "ldap_exclusive": ldap_exclusive,
        }


class AuthRegisterAdapter(AuthPublicAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        from src.models.database import SystemConfig

        db = context.db
        payload = context.ensure_json_body()
        register_request = RegisterRequest.model_validate(payload)
        client_ip = get_client_ip(context.request)
        user_agent = get_user_agent(context.request)

        # IP 速率限制检查（注册接口：3次/分钟）
        allowed, remaining, reset_after = await IPRateLimiter.check_limit(client_ip, "register")
        if not allowed:
            logger.warning(f"注册请求超过速率限制: IP={client_ip}, 剩余={remaining}")
            raise HTTPException(
                status_code=status.HTTP_429_TOO_MANY_REQUESTS,
                detail=f"注册请求过于频繁，请在 {reset_after} 秒后重试",
            )

        # 通过钩子检查是否有模块阻止本地注册（如 LDAP 排他模式）
        from src.core.modules.hooks import AUTH_CHECK_REGISTRATION, get_hook_dispatcher

        block_result = await get_hook_dispatcher().dispatch(AUTH_CHECK_REGISTRATION, db=db)
        if block_result and block_result.get("blocked"):
            raise HTTPException(
                status_code=status.HTTP_403_FORBIDDEN,
                detail=block_result.get("reason", "注册已被禁止"),
            )

        allow_registration = db.query(SystemConfig).filter_by(key="enable_registration").first()
        if allow_registration and not allow_registration.value:
            AuditService.log_event(
                db=db,
                event_type=AuditEventType.UNAUTHORIZED_ACCESS,
                description=f"Registration attempt rejected - registration disabled: {register_request.username}",
                ip_address=client_ip,
                user_agent=user_agent,
                metadata={"username": register_request.username, "reason": "registration_disabled"},
            )
            db.commit()
            context.request.state.tx_committed_by_route = True
            raise HTTPException(status_code=status.HTTP_403_FORBIDDEN, detail="系统暂不开放注册")

        email = register_request.email
        email_configured = EmailSenderService.is_smtp_configured(db)
        require_verification = SystemConfigService.get_config(
            db, "require_email_verification", default=False
        )

        # 如果邮箱服务未配置，强制不要求邮箱验证
        if not email_configured:
            require_verification = False

        # 如果系统要求邮箱验证，则必须提供邮箱
        if require_verification:
            if not email:
                raise HTTPException(
                    status_code=status.HTTP_400_BAD_REQUEST,
                    detail="系统要求邮箱验证，请填写邮箱",
                )
            # 检查邮箱是否已验证
            is_verified = await EmailVerificationService.is_email_verified(email)
            if not is_verified:
                logger.warning(f"注册失败：邮箱未验证: {email}")
                raise HTTPException(
                    status_code=status.HTTP_400_BAD_REQUEST,
                    detail="请先完成邮箱验证。请发送验证码并验证后再注册。",
                )

        # 如果提供了邮箱，进行后缀验证
        if email:
            suffix_allowed, suffix_error = validate_email_suffix(db, email)
            if not suffix_allowed:
                logger.warning(f"注册失败：邮箱后缀不允许: {email}")
                AuditService.log_event(
                    db=db,
                    event_type=AuditEventType.UNAUTHORIZED_ACCESS,
                    description=f"Registration attempt rejected - email suffix not allowed: {email}",
                    ip_address=client_ip,
                    user_agent=user_agent,
                    metadata={"email": email, "reason": "email_suffix_not_allowed"},
                )
                db.commit()
                context.request.state.tx_committed_by_route = True
                raise HTTPException(
                    status_code=status.HTTP_400_BAD_REQUEST,
                    detail=suffix_error,
                )

        try:
            # 读取系统配置的默认初始赠款
            default_initial_gift = SystemConfigService.get_config(
                db, "default_user_initial_gift_usd", default=None
            )

            # email_verified 逻辑：
            # - 要求邮箱验证且已通过验证：True
            # - 提供了邮箱但不要求验证：False（用户可后续自行验证）
            # - 未提供邮箱：False
            user = UserService.create_user(
                db=db,
                email=email,  # 可以为 None
                username=register_request.username,
                password=register_request.password,
                role=UserRole.USER,
                initial_gift_usd=default_initial_gift,
                email_verified=bool(require_verification and email),
            )
            AuditService.log_event(
                db=db,
                event_type=AuditEventType.USER_CREATED,
                description=f"User registered: {user.username}"
                + (f" ({user.email})" if user.email else ""),
                user_id=user.id,
                ip_address=client_ip,
                user_agent=user_agent,
                metadata={"email": user.email, "username": user.username, "role": user.role.value},
            )

            db.commit()
            context.request.state.tx_committed_by_route = True

            # 注册成功后清除验证状态（在 commit 后清理，即使清理失败也不影响注册结果）
            if require_verification and email:
                try:
                    await EmailVerificationService.clear_verification(email)
                except Exception as e:
                    logger.warning(f"清理验证状态失败: {e}")

            return RegisterResponse(
                user_id=user.id,
                email=user.email,
                username=user.username,
                message="注册成功",
            ).model_dump()
        except ValueError as exc:
            db.rollback()
            AuditService.log_event(
                db=db,
                event_type=AuditEventType.UNAUTHORIZED_ACCESS,
                description=f"Registration failed: {register_request.username} - {exc}",
                ip_address=client_ip,
                user_agent=user_agent,
                metadata={"username": register_request.username, "error": str(exc)},
            )
            db.commit()
            context.request.state.tx_committed_by_route = True
            raise HTTPException(status_code=status.HTTP_400_BAD_REQUEST, detail=str(exc))


class AuthCurrentUserAdapter(AuthenticatedApiAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        user = context.user
        wallet = WalletService.get_wallet(context.db, user_id=user.id)
        return {
            "id": user.id,
            "email": user.email,
            "username": user.username,
            "role": user.role.value,
            "is_active": user.is_active,
            "billing": WalletService.serialize_wallet_summary(wallet),
            "allowed_providers": user.allowed_providers,
            "allowed_api_formats": user.allowed_api_formats,
            "allowed_models": user.allowed_models,
            "created_at": user.created_at.isoformat(),
            "last_login_at": user.last_login_at.isoformat() if user.last_login_at else None,
            "auth_source": user.auth_source.value,
        }


class AuthLogoutAdapter(AuthenticatedApiAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        """用户登出，将 Token 加入黑名单"""
        user = context.user
        client_ip = get_client_ip(context.request)
        current_session_id = getattr(context.request.state, "user_session_id", None)

        # 从 Authorization header 获取 Token（与 pipeline 保持一致的大小写无关匹配）
        auth_header = context.request.headers.get("Authorization") or ""
        if not auth_header.lower().startswith("bearer "):
            raise HTTPException(status_code=status.HTTP_401_UNAUTHORIZED, detail="缺少认证令牌")

        token = auth_header[7:].strip()

        # 将 Token 加入黑名单
        success = await AuthService.logout(token)

        if current_session_id:
            session = SessionService.get_session_for_user(
                context.db, user_id=user.id, session_id=str(current_session_id)
            )
            if session:
                SessionService.revoke_session(
                    context.db,
                    session=session,
                    reason="user_logout",
                    audit_user_id=user.id,
                    ip_address=client_ip,
                    user_agent=get_user_agent(context.request),
                )

        if success or current_session_id:
            # 记录审计日志
            AuditService.log_event(
                db=context.db,
                event_type=AuditEventType.LOGOUT,
                description=f"User logged out: {user.email}",
                user_id=user.id,
                ip_address=client_ip,
                user_agent=get_user_agent(context.request),
                metadata={"user_id": user.id, "email": user.email},
            )
            context.db.commit()
            context.request.state.tx_committed_by_route = True

            logger.info(f"用户登出成功: {user.email}")

            return LogoutResponse(message="登出成功", success=True).model_dump()
        else:
            logger.warning(f"用户登出失败（Redis不可用）: {user.email}")
            return LogoutResponse(message="登出成功（降级模式）", success=False).model_dump()


class AuthSendVerificationCodeAdapter(AuthPublicAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        """发送邮箱验证码"""
        db = context.db
        payload = context.ensure_json_body()

        try:
            send_request = SendVerificationCodeRequest.model_validate(payload)
        except ValidationError as exc:
            errors = []
            for error in exc.errors():
                field = " -> ".join(str(x) for x in error["loc"])
                errors.append(f"{field}: {error['msg']}")
            raise InvalidRequestException("输入验证失败: " + "; ".join(errors))

        client_ip = get_client_ip(context.request)
        email = send_request.email

        # IP 速率限制检查（验证码发送：3次/分钟）
        allowed, remaining, reset_after = await IPRateLimiter.check_limit(
            client_ip, "verification_send"
        )
        if not allowed:
            logger.warning(f"验证码发送请求超过速率限制: IP={client_ip}, 剩余={remaining}")
            raise HTTPException(
                status_code=status.HTTP_429_TOO_MANY_REQUESTS,
                detail=f"请求过于频繁，请在 {reset_after} 秒后重试",
            )

        # 检查邮箱是否已注册
        existing_user = db.query(User).filter(User.email == email).first()
        if existing_user:
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail="该邮箱已被注册，请直接登录或使用其他邮箱",
            )

        # 检查邮箱后缀是否允许
        suffix_allowed, suffix_error = validate_email_suffix(db, email)
        if not suffix_allowed:
            logger.warning(f"邮箱后缀不允许: {email}")
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail=suffix_error,
            )

        # 生成并发送验证码（使用服务中的默认配置）
        success, code_or_error, error_detail = (
            await EmailVerificationService.send_verification_code(email)
        )

        if not success:
            logger.error(f"发送验证码失败: {email}, 错误: {code_or_error}")
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail=error_detail or code_or_error,
            )

        # 发送邮件
        expire_minutes = EmailVerificationService.DEFAULT_CODE_EXPIRE_MINUTES
        email_success, email_error = await EmailSenderService.send_verification_code(
            db=db, to_email=email, code=code_or_error, expire_minutes=expire_minutes
        )

        if not email_success:
            logger.error(f"发送验证码邮件失败: {email}, 错误: {email_error}")
            # 不向用户暴露 SMTP 详细错误信息，防止信息泄露
            raise HTTPException(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                detail="发送验证码失败，请稍后重试",
            )

        logger.info(f"验证码已发送: {email}")

        return SendVerificationCodeResponse(
            message="验证码已发送，请查收邮件",
            success=True,
            expire_minutes=expire_minutes,
        ).model_dump()


class AuthVerifyEmailAdapter(AuthPublicAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        """验证邮箱验证码"""
        payload = context.ensure_json_body()

        try:
            verify_request = VerifyEmailRequest.model_validate(payload)
        except ValidationError as exc:
            errors = []
            for error in exc.errors():
                field = " -> ".join(str(x) for x in error["loc"])
                errors.append(f"{field}: {error['msg']}")
            raise InvalidRequestException("输入验证失败: " + "; ".join(errors))

        client_ip = get_client_ip(context.request)
        email = verify_request.email
        code = verify_request.code

        # IP 速率限制检查（验证码验证：10次/分钟）
        allowed, remaining, reset_after = await IPRateLimiter.check_limit(
            client_ip, "verification_verify"
        )
        if not allowed:
            logger.warning(f"验证码验证请求超过速率限制: IP={client_ip}, 剩余={remaining}")
            raise HTTPException(
                status_code=status.HTTP_429_TOO_MANY_REQUESTS,
                detail=f"请求过于频繁，请在 {reset_after} 秒后重试",
            )

        # 验证验证码
        success, message = await EmailVerificationService.verify_code(email, code)

        if not success:
            logger.warning(f"验证码验证失败: {email}, 原因: {message}")
            raise HTTPException(status_code=status.HTTP_400_BAD_REQUEST, detail=message)

        logger.info(f"邮箱验证成功: {email}")

        return VerifyEmailResponse(message="邮箱验证成功", success=True).model_dump()


class AuthVerificationStatusAdapter(AuthPublicAdapter):
    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        """查询邮箱验证状态"""
        payload = context.ensure_json_body()

        try:
            status_request = VerificationStatusRequest.model_validate(payload)
        except ValidationError as exc:
            errors = []
            for error in exc.errors():
                field = " -> ".join(str(x) for x in error["loc"])
                errors.append(f"{field}: {error['msg']}")
            raise InvalidRequestException("输入验证失败: " + "; ".join(errors))

        client_ip = get_client_ip(context.request)
        email = status_request.email

        # IP 速率限制检查（验证状态查询：20次/分钟）
        allowed, remaining, reset_after = await IPRateLimiter.check_limit(
            client_ip, "verification_status", limit=20
        )
        if not allowed:
            logger.warning(f"验证状态查询请求超过速率限制: IP={client_ip}, 剩余={remaining}")
            raise HTTPException(
                status_code=status.HTTP_429_TOO_MANY_REQUESTS,
                detail=f"请求过于频繁，请在 {reset_after} 秒后重试",
            )

        # 获取验证状态
        status_data = await EmailVerificationService.get_verification_status(email)

        # 计算冷却剩余时间
        cooldown_remaining = None
        if status_data.get("has_pending_code") and status_data.get("created_at"):
            from datetime import datetime, timezone

            created_at = datetime.fromisoformat(status_data["created_at"])
            elapsed = (datetime.now(timezone.utc) - created_at).total_seconds()
            cooldown = EmailVerificationService.SEND_COOLDOWN_SECONDS - int(elapsed)
            if cooldown > 0:
                cooldown_remaining = cooldown

        return VerificationStatusResponse(
            email=email,
            has_pending_code=status_data.get("has_pending_code", False),
            is_verified=status_data.get("is_verified", False),
            cooldown_remaining=cooldown_remaining,
            code_expires_in=status_data.get("code_expires_in"),
        ).model_dump()
