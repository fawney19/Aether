import re
import uuid
from dataclasses import dataclass, field
from datetime import datetime, timezone
from typing import Any
from urllib.parse import parse_qsl, urlencode, urlparse, urlunparse

import httpx
from fastapi import HTTPException, status
from fastapi.concurrency import run_in_threadpool
from sqlalchemy import func
from sqlalchemy.exc import IntegrityError
from sqlalchemy.orm import Session

from src.clients.redis_client import get_redis_client
from src.core.enums import AuthSource, UserRole
from src.core.exceptions import ConfirmationRequiredException, InvalidRequestException
from src.core.logger import logger
from src.core.modules import get_module_registry
from src.database import get_db_context
from src.models.database import OAuthProvider, User, UserOAuthLink
from src.services.auth.oauth.base import OAuthProviderBase
from src.services.auth.oauth.models import OAuthFlowError, OAuthUserInfo
from src.services.auth.oauth.registry import get_oauth_provider_registry
from src.services.auth.oauth.state import consume_oauth_state, create_oauth_state
from src.services.auth.service import AuthService
from src.services.auth.session_service import SessionService
from src.services.cache.user_cache import UserCacheService
from src.services.system.config import SystemConfigService


def _build_oauth_client_kwargs(
    timeout_seconds: float = 5.0, follow_redirects: bool = False
) -> dict[str, Any]:
    """构建 OAuth HTTP 客户端参数（含系统默认代理）"""
    from src.services.proxy_node.resolver import build_proxy_client_kwargs

    return build_proxy_client_kwargs(
        timeout=httpx.Timeout(timeout_seconds), follow_redirects=follow_redirects
    )


@dataclass(frozen=True)
class OAuthCallbackResult:
    redirect_url: str
    refresh_token: str | None = field(default=None, repr=False)


class OAuthService:
    """OAuth 核心业务服务（v1）。"""

    @staticmethod
    def _handle_login_sync(provider_type: str, oauth_user: OAuthUserInfo) -> User:
        now = datetime.now(timezone.utc)

        with get_db_context() as db:
            existing_link = (
                db.query(UserOAuthLink)
                .filter(
                    UserOAuthLink.provider_type == provider_type,
                    UserOAuthLink.provider_user_id == oauth_user.id,
                )
                .first()
            )
            if existing_link:
                linked_user = db.query(User).filter(User.id == existing_link.user_id).first()
                if not linked_user or not linked_user.is_active or linked_user.is_deleted:
                    raise OAuthFlowError("account_disabled", "用户不存在或已禁用")

                linked_user.last_login_at = now
                existing_link.last_login_at = now
                db.commit()
                db.expunge(linked_user)
                return linked_user

            enable_registration = SystemConfigService.get_config(
                db, "enable_registration", default=False
            )
            if not enable_registration:
                raise OAuthFlowError("registration_disabled")

            email = oauth_user.email
            if email:
                if not OAuthService._validate_email_suffix(db, email):
                    raise OAuthFlowError("email_suffix_denied")

                existing_user = db.query(User).filter(User.email == email).first()
                if existing_user and not existing_user.is_deleted:
                    if existing_user.auth_source == AuthSource.LOCAL:
                        raise OAuthFlowError("email_exists_local")
                    if existing_user.auth_source == AuthSource.LDAP:
                        raise OAuthFlowError("email_is_ldap")
                    raise OAuthFlowError("email_is_oauth")

            base_username = (
                oauth_user.username
                or (email.split("@", 1)[0] if email else None)
                or f"user_{uuid.uuid4().hex[:8]}"
            )
            default_initial_gift = SystemConfigService.get_config(
                db, "default_user_initial_gift_usd", default=None
            )

            user: User | None = None
            last_error: Exception | None = None
            for _ in range(3):
                try:
                    username = OAuthService._generate_unique_username(db, base_username)
                    user = User(
                        email=email,
                        email_verified=bool(oauth_user.email_verified) if email else False,
                        username=username,
                        password_hash=None,
                        auth_source=AuthSource.OAUTH,
                        role=UserRole.USER,
                        is_active=True,
                        last_login_at=now,
                    )
                    db.add(user)
                    db.flush()

                    from src.services.wallet import WalletService

                    WalletService.initialize_user_wallet(
                        db,
                        user=user,
                        initial_gift_usd=default_initial_gift,
                        unlimited=False,
                        description="OAuth 注册初始赠款",
                    )

                    db.commit()
                    db.refresh(user)
                    last_error = None
                    break
                except IntegrityError as e:
                    db.rollback()
                    last_error = e
                except Exception as e:
                    db.rollback()
                    last_error = e

            if last_error is not None or user is None:
                raise OAuthFlowError("provider_error", "user_create_failed")

            assert user.id is not None
            try:
                link = UserOAuthLink(
                    user_id=user.id,
                    provider_type=provider_type,
                    provider_user_id=oauth_user.id,
                    provider_username=oauth_user.username,
                    provider_email=email,
                    extra_data=oauth_user.raw,
                    linked_at=now,
                    last_login_at=now,
                )
                db.add(link)
                db.commit()
            except IntegrityError as e:
                db.rollback()
                constraint = OAuthService._get_constraint_name(e)
                if constraint == "uq_oauth_provider_user":
                    existing_link = (
                        db.query(UserOAuthLink)
                        .filter(
                            UserOAuthLink.provider_type == provider_type,
                            UserOAuthLink.provider_user_id == oauth_user.id,
                        )
                        .first()
                    )
                    if existing_link:
                        existing_user = (
                            db.query(User).filter(User.id == existing_link.user_id).first()
                        )
                        if (
                            existing_user
                            and existing_user.is_active
                            and not existing_user.is_deleted
                        ):
                            existing_user.last_login_at = now
                            existing_link.last_login_at = now
                            db.commit()
                            db.expunge(existing_user)
                            return existing_user
                    raise OAuthFlowError("oauth_already_bound")
                raise OAuthFlowError("provider_error", "link_create_failed")

            db.expunge(user)
            return user

    @staticmethod
    def _handle_bind_sync(
        user_id: str, provider_type: str, oauth_user: OAuthUserInfo
    ) -> UserOAuthLink:
        now = datetime.now(timezone.utc)

        with get_db_context() as db:
            user = db.query(User).filter(User.id == user_id).first()
            if not user or not user.is_active or user.is_deleted:
                raise OAuthFlowError("user_not_found")
            if user.auth_source == AuthSource.LDAP:
                raise OAuthFlowError("ldap_no_oauth")

            link = UserOAuthLink(
                user_id=user.id,
                provider_type=provider_type,
                provider_user_id=oauth_user.id,
                provider_username=oauth_user.username,
                provider_email=oauth_user.email,
                extra_data=oauth_user.raw,
                linked_at=now,
            )

            try:
                db.add(link)
                db.commit()
                db.refresh(link)
                db.expunge(link)
                return link
            except IntegrityError as e:
                db.rollback()
                constraint = OAuthService._get_constraint_name(e)

                if constraint == "uq_oauth_provider_user":
                    existing = (
                        db.query(UserOAuthLink)
                        .filter(
                            UserOAuthLink.provider_type == provider_type,
                            UserOAuthLink.provider_user_id == oauth_user.id,
                        )
                        .first()
                    )
                    if existing and existing.user_id == user.id:
                        db.expunge(existing)
                        return existing
                    raise OAuthFlowError("oauth_already_bound")

                if constraint == "uq_user_oauth_provider":
                    raise OAuthFlowError("already_bound_provider")

                raise OAuthFlowError("provider_error", "bind_failed")

    @staticmethod
    def _upsert_provider_config_sync(provider_type: str, data: Any) -> OAuthProvider:
        with get_db_context() as db:
            provider = OAuthService._get_provider_impl(provider_type)
            if not provider:
                raise InvalidRequestException("不支持的 provider_type")

            OAuthService._validate_provider_config(provider, data)

            row = (
                db.query(OAuthProvider).filter(OAuthProvider.provider_type == provider_type).first()
            )
            creating = row is None
            if not row:
                row = OAuthProvider(provider_type=provider_type)
                db.add(row)

            if row.is_enabled and data.is_enabled is False:
                affected = OAuthService._check_provider_disable_safety(db, provider_type)
                if affected and not getattr(data, "force", False):
                    raise ConfirmationRequiredException(
                        message=f"禁用该 Provider 会导致 {len(affected)} 个用户无法登录",
                        affected_count=len(affected),
                        action="disable_oauth_provider",
                    )

            row.display_name = data.display_name
            row.client_id = data.client_id
            row.authorization_url_override = data.authorization_url_override
            row.token_url_override = data.token_url_override
            row.userinfo_url_override = data.userinfo_url_override
            row.scopes = data.scopes
            row.redirect_uri = data.redirect_uri
            row.frontend_callback_url = data.frontend_callback_url
            row.attribute_mapping = data.attribute_mapping
            row.extra_config = data.extra_config
            row.is_enabled = data.is_enabled

            if data.client_secret is not None:
                secret_value = data.client_secret.strip()
                if secret_value == "__CLEAR__":
                    row.client_secret_encrypted = None
                elif secret_value:
                    row.set_client_secret(secret_value)

            try:
                db.commit()
                db.refresh(row)
            except Exception:
                db.rollback()
                raise

            db.expunge(row)
            if creating:
                logger.info("OAuth provider 配置已创建: {}", provider_type)
            else:
                logger.info("OAuth provider 配置已更新: {}", provider_type)
            return row

    @staticmethod
    def _delete_provider_config_sync(provider_type: str) -> None:
        with get_db_context() as db:
            row = (
                db.query(OAuthProvider).filter(OAuthProvider.provider_type == provider_type).first()
            )
            if not row:
                raise InvalidRequestException("Provider 配置不存在")

            if row.is_enabled:
                affected = OAuthService._check_provider_disable_safety(db, provider_type)
                if affected:
                    raise InvalidRequestException(
                        f"删除该 Provider 会导致部分用户无法登录（数量: {len(affected)}），已阻止操作"
                    )

            db.delete(row)

    @staticmethod
    def _unbind_provider_sync(user_id: str, provider_type: str) -> None:
        with get_db_context() as db:
            OAuthService._require_module_active(db)
            user = db.query(User).filter(User.id == user_id).first()
            if not user:
                raise InvalidRequestException("用户不存在")

            if user.auth_source == AuthSource.LDAP:
                raise HTTPException(status_code=403, detail="LDAP 用户不允许解绑 OAuth")

            link = (
                db.query(UserOAuthLink)
                .filter(
                    UserOAuthLink.user_id == user.id, UserOAuthLink.provider_type == provider_type
                )
                .first()
            )
            if not link:
                raise InvalidRequestException("未绑定该 Provider")

            total_links = (
                db.query(func.count(UserOAuthLink.id))
                .filter(UserOAuthLink.user_id == user.id)
                .scalar()
                or 0
            )

            if user.auth_source == AuthSource.OAUTH and total_links <= 1:
                raise InvalidRequestException("OAUTH 用户必须至少保留一个 OAuth 绑定")

            if user.auth_source == AuthSource.LOCAL and not user.password_hash and total_links <= 1:
                raise InvalidRequestException("请先设置密码后再解绑")

            from src.core.modules.hooks import AUTH_CHECK_EXCLUSIVE_MODE, get_hook_dispatcher

            is_exclusive = get_hook_dispatcher().dispatch_sync(AUTH_CHECK_EXCLUSIVE_MODE, db=db)
            if (
                is_exclusive
                and user.auth_source == AuthSource.LOCAL
                and user.role != UserRole.ADMIN
            ):
                if total_links <= 1:
                    raise InvalidRequestException("当前处于 LDAP 专属模式，解绑后将无法登录")

            db.delete(link)

    @staticmethod
    def _require_module_active(db: Session) -> None:
        registry = get_module_registry()
        if not registry.is_active("oauth", db):
            raise HTTPException(
                status_code=status.HTTP_503_SERVICE_UNAVAILABLE, detail="OAuth 模块未启用"
            )

    @staticmethod
    def _get_provider_impl(provider_type: str) -> OAuthProviderBase | None:
        registry = get_oauth_provider_registry()
        registry.discover_providers()
        provider = registry.get_provider(provider_type)
        return provider

    @staticmethod
    def _get_provider_config(db: Session, provider_type: str) -> OAuthProvider:
        row = db.query(OAuthProvider).filter(OAuthProvider.provider_type == provider_type).first()
        if not row:
            raise InvalidRequestException("Provider 配置不存在")
        return row

    @staticmethod
    def _get_enabled_provider_config(db: Session, provider_type: str) -> OAuthProvider:
        row = OAuthService._get_provider_config(db, provider_type)
        if not row.is_enabled:
            raise OAuthFlowError("provider_disabled", "provider 未启用")
        return row

    @staticmethod
    def _build_frontend_error_redirect(
        frontend_callback_url: str, *, error_code: str, error_detail: str = ""
    ) -> str:
        parsed = urlparse(frontend_callback_url)
        query = dict(parse_qsl(parsed.query, keep_blank_values=True))
        query["error_code"] = error_code
        if error_detail:
            query["error_detail"] = (error_detail[:200]).strip()
        return urlunparse(parsed._replace(query=urlencode(query), fragment=""))

    @staticmethod
    def _build_frontend_bind_success_redirect(frontend_callback_url: str, display_name: str) -> str:
        parsed = urlparse(frontend_callback_url)
        query = dict(parse_qsl(parsed.query, keep_blank_values=True))
        query["oauth_bound"] = display_name
        return urlunparse(parsed._replace(query=urlencode(query), fragment=""))

    @staticmethod
    def _build_frontend_login_success_redirect(
        frontend_callback_url: str, *, access_token: str
    ) -> str:
        parsed = urlparse(frontend_callback_url)
        fragment = urlencode(
            {
                "access_token": access_token,
                "token_type": "bearer",
                "expires_in": 86400,
            }
        )
        # fragment 不会被发送回后端，适配当前 localStorage 登录态方案
        return urlunparse(parsed._replace(fragment=fragment))

    @staticmethod
    async def list_public_providers(db: Session) -> list[dict[str, str]]:
        registry = get_module_registry()
        if not registry.is_active("oauth", db):
            return []

        supported = get_oauth_provider_registry()
        supported.discover_providers()

        rows = (
            db.query(OAuthProvider)
            .filter(OAuthProvider.is_enabled.is_(True))
            .order_by(OAuthProvider.provider_type.asc())
            .all()
        )

        result: list[dict[str, str]] = []
        for row in rows:
            provider_type_value = row.provider_type
            if not provider_type_value:
                continue
            provider_type_str = str(provider_type_value)
            if supported.get_provider(provider_type_str) is None:
                continue
            display_name = row.display_name or provider_type_str
            result.append({"provider_type": provider_type_str, "display_name": str(display_name)})
        return result

    @staticmethod
    async def build_login_authorize_url(
        db: Session, provider_type: str, client_device_id: str | None = None
    ) -> str:
        OAuthService._require_module_active(db)
        normalized_device_id = (
            SessionService._normalize_device_id(client_device_id) if client_device_id else None
        )
        if not normalized_device_id:
            raise HTTPException(status_code=400, detail="缺少或无效的设备标识")

        provider = OAuthService._get_provider_impl(provider_type)
        if not provider:
            raise HTTPException(status_code=404, detail="不支持的 OAuth provider")

        try:
            config = OAuthService._get_enabled_provider_config(db, provider_type)
        except OAuthFlowError as exc:
            raise HTTPException(
                status_code=status.HTTP_503_SERVICE_UNAVAILABLE, detail=exc.error_code
            )

        redis = await get_redis_client(require_redis=True)
        if redis is None:
            raise HTTPException(status_code=503, detail="Redis 不可用")

        state = await create_oauth_state(
            redis,
            provider_type=provider_type,
            action="login",
            user_id=None,
            client_device_id=normalized_device_id,
        )
        return provider.get_authorization_url(config, state)

    @staticmethod
    async def build_bind_authorize_url(
        db: Session,
        user: User,
        provider_type: str,
        client_device_id: str | None = None,
    ) -> str:
        OAuthService._require_module_active(db)

        if user.auth_source == AuthSource.LDAP:
            raise HTTPException(status_code=403, detail="LDAP 用户不允许绑定 OAuth")

        normalized_device_id: str | None = None
        if client_device_id is not None:
            normalized_device_id = SessionService._normalize_device_id(client_device_id)
            if not normalized_device_id:
                raise HTTPException(status_code=400, detail="缺少或无效的设备标识")

        provider = OAuthService._get_provider_impl(provider_type)
        if not provider:
            raise HTTPException(status_code=404, detail="不支持的 OAuth provider")

        try:
            config = OAuthService._get_enabled_provider_config(db, provider_type)
        except OAuthFlowError as exc:
            raise HTTPException(
                status_code=status.HTTP_503_SERVICE_UNAVAILABLE, detail=exc.error_code
            )

        redis = await get_redis_client(require_redis=True)
        if redis is None:
            raise HTTPException(status_code=503, detail="Redis 不可用")

        state = await create_oauth_state(
            redis,
            provider_type=provider_type,
            action="bind",
            user_id=user.id,
            client_device_id=normalized_device_id,
        )
        return provider.get_authorization_url(config, state)

    @staticmethod
    def _sanitize_username(raw: str | None) -> str:
        if not raw or not raw.strip():
            return f"user_{uuid.uuid4().hex[:8]}"

        cleaned = re.sub(r"[^a-zA-Z0-9_-]", "_", raw.strip())
        cleaned = re.sub(r"_+", "_", cleaned).strip("_")

        if cleaned and cleaned[0].isdigit():
            cleaned = f"u_{cleaned}"

        # 预留后缀空间，避免后续重试超长
        max_len = 90
        if len(cleaned) > max_len:
            cleaned = cleaned[:max_len]

        return cleaned or f"user_{uuid.uuid4().hex[:8]}"

    @staticmethod
    def _generate_unique_username(db: Session, base: str, max_retries: int = 3) -> str:
        base = OAuthService._sanitize_username(base)
        candidates = [base]
        for i in range(max_retries - 1):
            suffix_len = 4 if i == 0 else 8
            candidates.append(f"{base}_{uuid.uuid4().hex[:suffix_len]}")

        for cand in candidates:
            exists = db.query(User).filter(User.username == cand).first()
            if not exists:
                return cand
        raise ValueError("无法生成唯一用户名")

    @staticmethod
    def _validate_email_suffix(db: Session, email: str) -> bool:
        mode = SystemConfigService.get_config(db, "email_suffix_mode", default="none")
        if mode == "none":
            return True

        suffix_list = SystemConfigService.get_config(db, "email_suffix_list", default=[])
        if isinstance(suffix_list, str):
            suffix_list = [s.strip().lower() for s in suffix_list.split(",") if s.strip()]

        if not suffix_list:
            return True

        if "@" not in email:
            return False
        email_suffix = email.split("@", 1)[1].lower()

        if mode == "whitelist":
            return email_suffix in suffix_list
        if mode == "blacklist":
            return email_suffix not in suffix_list
        return True

    @staticmethod
    def _get_constraint_name(err: IntegrityError) -> str | None:
        orig = getattr(err, "orig", None)
        diag = getattr(orig, "diag", None)
        name = getattr(diag, "constraint_name", None)
        return str(name) if name else None

    @staticmethod
    async def handle_callback(
        *,
        db: Session,
        provider_type: str,
        state: str,
        code: str | None,
        error: str | None,
        error_description: str | None,
        client_ip: str | None,
        user_agent: str,
        headers: dict[str, str],
    ) -> OAuthCallbackResult:
        OAuthService._require_module_active(db)

        provider = OAuthService._get_provider_impl(provider_type)
        if not provider:
            raise HTTPException(status_code=404, detail="不支持的 OAuth provider")

        config = OAuthService._get_provider_config(db, provider_type)
        frontend_callback_url = config.frontend_callback_url
        if not frontend_callback_url:
            # 无法重定向到前端时，直接返回 500（配置错误）
            raise HTTPException(status_code=500, detail="frontend_callback_url 未配置")
        frontend_callback_url = str(frontend_callback_url)

        # display_name 用于 bind 成功 toast；兜底为 provider_type
        display_name = str(config.display_name or provider_type)

        # provider 被禁用时仍引导回前端（给出明确提示）
        if not config.is_enabled:
            return OAuthCallbackResult(
                redirect_url=OAuthService._build_frontend_error_redirect(
                    frontend_callback_url, error_code="provider_disabled"
                )
            )

        # provider 侧 error
        if error:
            code_map = "authorization_denied" if error == "access_denied" else "provider_error"
            return OAuthCallbackResult(
                redirect_url=OAuthService._build_frontend_error_redirect(
                    frontend_callback_url,
                    error_code=code_map,
                    error_detail=error_description or error,
                )
            )

        if not code or not state:
            return OAuthCallbackResult(
                redirect_url=OAuthService._build_frontend_error_redirect(
                    frontend_callback_url, error_code="invalid_callback"
                )
            )

        # 一次性消费 state
        try:
            redis = await get_redis_client(require_redis=True)
            if redis is None:
                raise RuntimeError("redis unavailable")
            state_data = await consume_oauth_state(redis, state)
        except Exception as exc:
            logger.warning("OAuth state 消费失败: {}", exc)
            state_data = None

        if not state_data:
            return OAuthCallbackResult(
                redirect_url=OAuthService._build_frontend_error_redirect(
                    frontend_callback_url, error_code="invalid_state"
                )
            )
        normalized_device_id = None
        if state_data.client_device_id:
            normalized_device_id = SessionService._normalize_device_id(state_data.client_device_id)
            if not normalized_device_id:
                return OAuthCallbackResult(
                    redirect_url=OAuthService._build_frontend_error_redirect(
                        frontend_callback_url, error_code="invalid_state"
                    )
                )
        if state_data.action == "login" and not normalized_device_id:
            return OAuthCallbackResult(
                redirect_url=OAuthService._build_frontend_error_redirect(
                    frontend_callback_url, error_code="invalid_state"
                )
            )

        if state_data.provider_type != provider_type:
            return OAuthCallbackResult(
                redirect_url=OAuthService._build_frontend_error_redirect(
                    frontend_callback_url, error_code="invalid_state"
                )
            )

        if state_data.action not in ("login", "bind"):
            return OAuthCallbackResult(
                redirect_url=OAuthService._build_frontend_error_redirect(
                    frontend_callback_url, error_code="invalid_state"
                )
            )

        if state_data.action == "bind" and not state_data.user_id:
            return OAuthCallbackResult(
                redirect_url=OAuthService._build_frontend_error_redirect(
                    frontend_callback_url, error_code="invalid_bind_state"
                )
            )

        try:
            token = await provider.exchange_code(config, code)
            oauth_user = await provider.get_user_info(config, token.access_token)
        except OAuthFlowError as exc:
            return OAuthCallbackResult(
                redirect_url=OAuthService._build_frontend_error_redirect(
                    frontend_callback_url, error_code=exc.error_code, error_detail=exc.detail
                )
            )
        except Exception as exc:
            logger.warning("OAuth callback 处理失败: {}", exc)
            return OAuthCallbackResult(
                redirect_url=OAuthService._build_frontend_error_redirect(
                    frontend_callback_url, error_code="provider_error"
                )
            )

        if state_data.action == "bind":
            try:
                await OAuthService._handle_bind(
                    db, user_id=state_data.user_id or "", config=config, oauth_user=oauth_user
                )
            except OAuthFlowError as exc:
                return OAuthCallbackResult(
                    redirect_url=OAuthService._build_frontend_error_redirect(
                        frontend_callback_url, error_code=exc.error_code, error_detail=exc.detail
                    )
                )

            return OAuthCallbackResult(
                redirect_url=OAuthService._build_frontend_bind_success_redirect(
                    frontend_callback_url, display_name
                )
            )

        # login
        try:
            user = await OAuthService._handle_login(db, config=config, oauth_user=oauth_user)
        except OAuthFlowError as exc:
            return OAuthCallbackResult(
                redirect_url=OAuthService._build_frontend_error_redirect(
                    frontend_callback_url, error_code=exc.error_code, error_detail=exc.detail
                )
            )

        assert user.id is not None
        assert user.role is not None

        session_id = str(uuid.uuid4())
        access_token = AuthService.create_access_token(
            data={
                "user_id": user.id,
                "role": user.role.value,
                "created_at": user.created_at.isoformat() if user.created_at else None,
                "session_id": session_id,
            }
        )
        refresh_token = AuthService.create_refresh_token(
            data={
                "user_id": user.id,
                "created_at": user.created_at.isoformat() if user.created_at else None,
                "session_id": session_id,
                "jti": str(uuid.uuid4()),
            }
        )
        client_context = SessionService.build_client_context(
            client_device_id=normalized_device_id,
            client_ip=client_ip,
            user_agent=user_agent,
            headers=headers,
        )
        SessionService.create_session(
            db,
            user=user,
            session_id=session_id,
            refresh_token=refresh_token,
            expires_at=AuthService.get_refresh_token_expiry(),
            client=client_context,
        )
        db.commit()

        return OAuthCallbackResult(
            redirect_url=OAuthService._build_frontend_login_success_redirect(
                frontend_callback_url, access_token=access_token
            ),
            refresh_token=refresh_token,
        )

    @staticmethod
    async def _handle_login(
        db: Session, *, config: OAuthProvider, oauth_user: OAuthUserInfo
    ) -> User:
        user = await run_in_threadpool(
            OAuthService._handle_login_sync,
            config.provider_type,
            oauth_user,
        )
        assert user.id is not None
        await UserCacheService.invalidate_user_cache(user.id, user.email)
        return user

    @staticmethod
    async def _handle_bind(
        db: Session, *, user_id: str, config: OAuthProvider, oauth_user: OAuthUserInfo
    ) -> UserOAuthLink:
        return await run_in_threadpool(
            OAuthService._handle_bind_sync,
            user_id,
            config.provider_type,
            oauth_user,
        )

    @staticmethod
    async def list_bindable_providers(db: Session, user: User) -> list[dict[str, str]]:
        OAuthService._require_module_active(db)

        if user.auth_source == AuthSource.LDAP:
            return []

        supported = get_oauth_provider_registry()
        supported.discover_providers()

        enabled_rows = (
            db.query(OAuthProvider)
            .filter(OAuthProvider.is_enabled.is_(True))
            .order_by(OAuthProvider.provider_type.asc())
            .all()
        )
        linked_types = {
            provider_type
            for (provider_type,) in db.query(UserOAuthLink.provider_type)
            .filter(UserOAuthLink.user_id == user.id)
            .all()
        }

        result: list[dict[str, str]] = []
        for row in enabled_rows:
            provider_type_value = row.provider_type
            if not provider_type_value:
                continue
            provider_type_str = str(provider_type_value)
            if provider_type_str in linked_types:
                continue
            if supported.get_provider(provider_type_str) is None:
                continue
            display_name = row.display_name or provider_type_str
            result.append({"provider_type": provider_type_str, "display_name": str(display_name)})
        return result

    @staticmethod
    async def list_user_links(db: Session, user: User) -> list[dict[str, Any]]:
        OAuthService._require_module_active(db)

        rows = (
            db.query(UserOAuthLink, OAuthProvider)
            .join(OAuthProvider, UserOAuthLink.provider_type == OAuthProvider.provider_type)
            .filter(UserOAuthLink.user_id == user.id)
            .order_by(UserOAuthLink.linked_at.desc())
            .all()
        )

        result: list[dict[str, Any]] = []
        for link, provider in rows:
            result.append(
                {
                    "provider_type": link.provider_type,
                    "display_name": provider.display_name,
                    "provider_username": link.provider_username,
                    "provider_email": link.provider_email,
                    "linked_at": link.linked_at.isoformat() if link.linked_at else None,
                    "last_login_at": link.last_login_at.isoformat() if link.last_login_at else None,
                    "provider_enabled": bool(provider.is_enabled),
                }
            )
        return result

    @staticmethod
    def _check_provider_disable_safety(db: Session, provider_type: str) -> list[str]:
        """
        v1 简化版防锁号检查：
        - 只检查活跃用户（is_active && !is_deleted）
        - OAUTH 用户：禁用后必须仍有其它启用的 OAuth provider 绑定
        - LOCAL 用户：ldap_exclusive=true 且非 admin 时，同上
        """
        from src.core.modules.hooks import AUTH_CHECK_EXCLUSIVE_MODE, get_hook_dispatcher

        ldap_exclusive = get_hook_dispatcher().dispatch_sync(AUTH_CHECK_EXCLUSIVE_MODE, db=db)

        users = (
            db.query(User.id, User.auth_source, User.role)
            .join(UserOAuthLink, User.id == UserOAuthLink.user_id)
            .filter(
                User.is_active.is_(True),
                User.is_deleted.is_(False),
                UserOAuthLink.provider_type == provider_type,
            )
            .all()
        )

        affected: list[str] = []
        for user_id, auth_source, role in users:
            other_enabled_count = (
                db.query(func.count(UserOAuthLink.id))
                .join(OAuthProvider, UserOAuthLink.provider_type == OAuthProvider.provider_type)
                .filter(
                    UserOAuthLink.user_id == user_id,
                    UserOAuthLink.provider_type != provider_type,
                    OAuthProvider.is_enabled.is_(True),
                )
                .scalar()
                or 0
            )

            locked = False
            if auth_source == AuthSource.OAUTH:
                if other_enabled_count == 0:
                    locked = True
            elif auth_source == AuthSource.LOCAL and ldap_exclusive:
                is_admin = role == UserRole.ADMIN
                if not is_admin and other_enabled_count == 0:
                    locked = True

            if locked:
                affected.append(str(user_id))

        return affected

    @staticmethod
    async def upsert_provider_config(db: Session, provider_type: str, data: Any) -> OAuthProvider:
        return await run_in_threadpool(
            OAuthService._upsert_provider_config_sync, provider_type, data
        )

    @staticmethod
    async def delete_provider_config(db: Session, provider_type: str) -> None:
        await run_in_threadpool(OAuthService._delete_provider_config_sync, provider_type)

    @staticmethod
    def _validate_provider_config(provider: OAuthProviderBase, data: Any) -> None:
        # frontend_callback_url 校验：必须绝对 URL，path 以 /auth/callback 结尾（允许 basePath）
        OAuthService._validate_frontend_callback_url(data.frontend_callback_url)

        # redirect_uri：允许本地 http，其余建议 https（v1：仅做基本校验）
        OAuthService._validate_redirect_uri(data.redirect_uri)

        # 覆盖端点：必须 https 且 hostname 命中 provider 白名单
        for field_name in (
            "authorization_url_override",
            "token_url_override",
            "userinfo_url_override",
        ):
            value = getattr(data, field_name)
            if value:
                OAuthService._validate_url_override(provider, value)

    @staticmethod
    def _validate_frontend_callback_url(url: str) -> None:
        parsed = urlparse(url)
        if not parsed.scheme or not parsed.netloc:
            raise InvalidRequestException("frontend_callback_url 必须是绝对 URL")

        if parsed.scheme not in {"http", "https"}:
            raise InvalidRequestException("frontend_callback_url scheme 必须是 http/https")

        path = (parsed.path or "").rstrip("/")
        if not path.endswith("/auth/callback"):
            raise InvalidRequestException("frontend_callback_url 路径必须以 /auth/callback 结尾")

    @staticmethod
    def _validate_redirect_uri(url: str) -> None:
        parsed = urlparse(url)
        if not parsed.scheme or not parsed.netloc:
            raise InvalidRequestException("redirect_uri 必须是绝对 URL")
        if parsed.scheme not in {"http", "https"}:
            raise InvalidRequestException("redirect_uri scheme 必须是 http/https")

    @staticmethod
    def _validate_url_override(provider: OAuthProviderBase, url: str) -> None:
        parsed = urlparse(url)
        if parsed.scheme != "https" or not parsed.netloc:
            raise InvalidRequestException("端点覆盖必须是 https 绝对 URL")

        host = (parsed.hostname or "").lower().rstrip(".")
        allowed = False
        for domain in provider.allowed_domains:
            d = domain.lower().rstrip(".")
            if host == d or host.endswith(f".{d}"):
                allowed = True
                break
        if not allowed:
            raise InvalidRequestException("端点覆盖不在允许的域名白名单中")

    @staticmethod
    async def test_provider_config(db: Session, provider_type: str) -> dict[str, Any]:
        provider = OAuthService._get_provider_impl(provider_type)
        if not provider:
            return {
                "authorization_url_reachable": False,
                "token_url_reachable": False,
                "secret_status": "unknown",
                "details": "provider 未安装/不可用",
            }

        cfg = OAuthService._get_provider_config(db, provider_type)

        # Read all required fields first, then release DB connection before any awaits.
        # This prevents holding a pooled connection while doing network I/O.
        auth_url = provider.get_effective_authorization_url(cfg)
        token_url = provider.get_effective_token_url(cfg)
        redirect_uri = cfg.redirect_uri
        client_id = cfg.client_id
        has_secret = bool(cfg.client_secret_encrypted)
        client_secret = cfg.get_client_secret() if has_secret else None

        # Release DB connection (safe only when session has no pending changes).
        try:
            has_pending_changes = bool(db.new) or bool(db.dirty) or bool(db.deleted)
        except Exception:
            has_pending_changes = False
        if not has_pending_changes:
            original_expire_on_commit = getattr(db, "expire_on_commit", True)
            db.expire_on_commit = False
            try:
                if db.in_transaction():
                    db.commit()
            except Exception:
                try:
                    db.rollback()
                except Exception:
                    pass
            finally:
                db.expire_on_commit = original_expire_on_commit

        async def _reachable(url: str) -> bool:
            try:
                async with httpx.AsyncClient(
                    **_build_oauth_client_kwargs(5.0, follow_redirects=False)
                ) as client:
                    await client.get(url)
                return True
            except Exception:
                return False

        authorization_url_reachable = await _reachable(auth_url)
        token_url_reachable = await _reachable(token_url)

        secret_status = "unknown"
        details = ""

        if has_secret and client_secret:
            # 使用无效 code 做一次 token 请求（仅做粗略判定）
            try:
                async with httpx.AsyncClient(**_build_oauth_client_kwargs(5.0)) as client:
                    resp = await client.post(
                        token_url,
                        data={
                            "grant_type": "authorization_code",
                            "code": "invalid",
                            "redirect_uri": redirect_uri,
                            "client_id": client_id,
                            "client_secret": client_secret,
                        },
                    )
                try:
                    body = resp.json()
                except Exception:
                    body = {}

                err = str(body.get("error") or "").lower()
                if err in {"invalid_client", "unauthorized_client"}:
                    secret_status = "invalid"
                elif err in {"invalid_grant", "invalid_code"}:
                    secret_status = "likely_valid"
                else:
                    secret_status = "unknown"
                details = f"status={resp.status_code}"
            except Exception as exc:
                secret_status = "unknown"
                details = str(exc)

        return {
            "authorization_url_reachable": bool(authorization_url_reachable),
            "token_url_reachable": bool(token_url_reachable),
            "secret_status": secret_status,
            "details": details,
        }

    @staticmethod
    async def test_provider_config_with_data(
        provider_type: str,
        client_id: str,
        client_secret: str | None,
        authorization_url_override: str | None,
        token_url_override: str | None,
        redirect_uri: str,
    ) -> dict[str, Any]:
        """使用传入的表单数据测试配置，而非从数据库读取"""
        provider = OAuthService._get_provider_impl(provider_type)
        if not provider:
            return {
                "authorization_url_reachable": False,
                "token_url_reachable": False,
                "secret_status": "unknown",
                "details": "provider 未安装/不可用",
            }

        # 使用传入的 override URL 或 provider 默认值
        auth_url = authorization_url_override or provider.authorization_url
        token_url = token_url_override or provider.token_url

        async def _reachable(url: str) -> bool:
            try:
                async with httpx.AsyncClient(
                    **_build_oauth_client_kwargs(5.0, follow_redirects=False)
                ) as client:
                    await client.get(url)
                return True
            except Exception:
                return False

        authorization_url_reachable = await _reachable(auth_url)
        token_url_reachable = await _reachable(token_url)

        secret_status = "unknown"
        details = ""

        if client_secret:
            # 使用无效 code 做一次 token 请求（仅做粗略判定）
            try:
                async with httpx.AsyncClient(**_build_oauth_client_kwargs(5.0)) as client:
                    resp = await client.post(
                        token_url,
                        data={
                            "grant_type": "authorization_code",
                            "code": "invalid",
                            "redirect_uri": redirect_uri,
                            "client_id": client_id,
                            "client_secret": client_secret,
                        },
                    )
                try:
                    body = resp.json()
                except Exception:
                    body = {}

                err = str(body.get("error") or "").lower()
                if err in {"invalid_client", "unauthorized_client"}:
                    secret_status = "invalid"
                elif err in {"invalid_grant", "invalid_code"}:
                    secret_status = "likely_valid"
                else:
                    secret_status = "unknown"
                details = f"status={resp.status_code}"
            except Exception as exc:
                secret_status = "unknown"
                details = str(exc)
        else:
            secret_status = "not_provided"

        return {
            "authorization_url_reachable": bool(authorization_url_reachable),
            "token_url_reachable": bool(token_url_reachable),
            "secret_status": secret_status,
            "details": details,
        }

    @staticmethod
    async def unbind_provider(db: Session, user: User, provider_type: str) -> None:
        await run_in_threadpool(OAuthService._unbind_provider_sync, user.id, provider_type)
        if user.id is not None:
            await UserCacheService.invalidate_user_cache(user.id, user.email)
