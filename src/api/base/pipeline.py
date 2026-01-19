from __future__ import annotations

import time
from enum import Enum
from typing import TYPE_CHECKING, Any, Optional, Tuple

from fastapi import HTTPException, Request
from sqlalchemy.orm import Session

from src.config.settings import config
from src.core.enums import UserRole
from src.core.exceptions import QuotaExceededException
from src.core.logger import logger
from src.models.database import ApiKey, AuditEventType, User
from src.services.auth.service import AuthService
from src.services.system.audit import AuditService
from src.services.usage.service import UsageService

if TYPE_CHECKING:
    from src.models.database import ManagementToken

from .adapter import ApiAdapter, ApiMode
from .context import ApiRequestContext



class ApiRequestPipeline:
    """负责统一执行认证、配额校验、上下文构建等通用逻辑的管道。"""

    def __init__(
        self,
        auth_service: AuthService = AuthService,
        usage_service: UsageService = UsageService,
        audit_service: AuditService = AuditService,
    ):
        self.auth_service = auth_service
        self.usage_service = usage_service
        self.audit_service = audit_service

    async def run(
        self,
        adapter: ApiAdapter,
        http_request: Request,
        db: Session,
        *,
        mode: ApiMode = ApiMode.STANDARD,
        api_format_hint: Optional[str] = None,
        path_params: Optional[dict[str, Any]] = None,
    ):
        logger.debug(f"[Pipeline] START | path={http_request.url.path}")
        logger.debug(f"[Pipeline] Running with mode={mode}, adapter={adapter.__class__.__name__}, "
            f"adapter.mode={adapter.mode}, path={http_request.url.path}")
        if mode == ApiMode.ADMIN:
            user, management_token = await self._authenticate_admin(http_request, db)
            api_key = None
        elif mode == ApiMode.USER:
            user, management_token = await self._authenticate_user(http_request, db)
            api_key = None
        elif mode == ApiMode.PUBLIC:
            user = None
            api_key = None
            management_token = None
        elif mode == ApiMode.MANAGEMENT:
            user, management_token = await self._authenticate_management(http_request, db)
            api_key = None
        else:
            logger.debug("[Pipeline] 调用 _authenticate_client")
            user, api_key = self._authenticate_client(http_request, db, adapter)
            management_token = None
            logger.debug(f"[Pipeline] 认证完成 | user={user.username if user else None}")

        raw_body = None
        if http_request.method in {"POST", "PUT", "PATCH"}:
            try:
                import asyncio

                # 添加超时防止卡死
                raw_body = await asyncio.wait_for(
                    http_request.body(), timeout=config.request_body_timeout
                )
                logger.debug(f"[Pipeline] Raw body读取完成 | size={len(raw_body) if raw_body is not None else 0} bytes")
            except asyncio.TimeoutError:
                timeout_sec = int(config.request_body_timeout)
                logger.error(f"读取请求体超时({timeout_sec}s),可能客户端未发送完整请求体")
                raise HTTPException(
                    status_code=408,
                    detail=f"Request timeout: body not received within {timeout_sec} seconds",
                )
        else:
            logger.debug(f"[Pipeline] 非写请求跳过读取Body | method={http_request.method}")

        context = ApiRequestContext.build(
            request=http_request,
            db=db,
            user=user,
            api_key=api_key,
            raw_body=raw_body,
            mode=mode.value,
            api_format_hint=api_format_hint,
            path_params=path_params,
        )
        # 存储 management_token 到 context（用于权限检查）
        if management_token:
            context.management_token = management_token
        logger.debug(f"[Pipeline] Context构建完成 | adapter={adapter.name} | request_id={context.request_id}")

        if mode != ApiMode.ADMIN and user:
            context.quota_remaining = self._calculate_quota_remaining(user)

        logger.debug(f"[Pipeline] Adapter={adapter.name} | RequestID={context.request_id}")

        logger.debug(f"[Pipeline] Calling authorize on {adapter.__class__.__name__}, user={context.user}")
        # authorize 可能是异步的，需要检查并 await
        authorize_result = adapter.authorize(context)
        if hasattr(authorize_result, "__await__"):
            await authorize_result

        try:
            response = await adapter.handle(context)
            status_code = getattr(response, "status_code", None)
            self._record_audit_event(context, adapter, success=True, status_code=status_code)
            return response
        except HTTPException as exc:
            err_detail = exc.detail if isinstance(exc.detail, str) else str(exc.detail)
            self._record_audit_event(
                context,
                adapter,
                success=False,
                status_code=exc.status_code,
                error=err_detail,
            )
            raise
        except Exception as exc:
            self._record_audit_event(
                context,
                adapter,
                success=False,
                status_code=500,
                error=str(exc),
            )
            raise

    # --------------------------------------------------------------------- #
    # Internal helpers
    # --------------------------------------------------------------------- #

    def _authenticate_client(
        self, request: Request, db: Session, adapter: ApiAdapter
    ) -> Tuple[User, ApiKey]:
        logger.debug("[Pipeline._authenticate_client] 开始")
        # 使用 adapter 的 extract_api_key 方法，支持不同 API 格式的认证头
        client_api_key = adapter.extract_api_key(request)
        logger.debug(f"[Pipeline._authenticate_client] 提取API密钥完成 | key_prefix={client_api_key[:8] if client_api_key else None}...")
        if not client_api_key:
            raise HTTPException(status_code=401, detail="请提供API密钥")

        logger.debug("[Pipeline._authenticate_client] 调用 auth_service.authenticate_api_key")
        auth_result = self.auth_service.authenticate_api_key(db, client_api_key)
        logger.debug(f"[Pipeline._authenticate_client] 认证结果 | result={bool(auth_result)}")
        if not auth_result:
            raise HTTPException(status_code=401, detail="无效的API密钥")

        user, api_key = auth_result
        if not user or not api_key:
            raise HTTPException(status_code=401, detail="无效的API密钥")

        request.state.user_id = user.id
        request.state.api_key_id = api_key.id

        # 检查配额或余额（支持独立Key）
        quota_ok, message = self.usage_service.check_user_quota(db, user, api_key=api_key)
        if not quota_ok:
            # 根据Key类型计算剩余额度
            if api_key.is_standalone:
                # 独立Key：显示剩余余额
                remaining = (
                    None
                    if api_key.current_balance_usd is None
                    else float(api_key.current_balance_usd - (api_key.balance_used_usd or 0))
                )
            else:
                # 普通Key：显示用户配额剩余
                remaining = (
                    None
                    if user.quota_usd is None or user.quota_usd < 0
                    else float(user.quota_usd - user.used_usd)
                )
            raise QuotaExceededException(quota_type="USD", remaining=remaining)

        return user, api_key

    async def _authenticate_admin(
        self, request: Request, db: Session
    ) -> Tuple[User, Optional["ManagementToken"]]:
        """管理员认证，支持 JWT 和 Management Token 两种方式"""
        from src.models.database import ManagementToken
        from src.utils.request_utils import get_client_ip

        authorization = request.headers.get("authorization")
        if not authorization or not authorization.lower().startswith("bearer "):
            raise HTTPException(status_code=401, detail="缺少管理员凭证")

        token = authorization[7:].strip()

        # 检查是否为 Management Token（ae_ 前缀）
        if token.startswith(ManagementToken.TOKEN_PREFIX):
            client_ip = get_client_ip(request)
            result = await self.auth_service.authenticate_management_token(db, token, client_ip)

            if not result:
                raise HTTPException(status_code=401, detail="无效或过期的 Management Token")

            user, management_token = result

            # 检查管理员权限
            if user.role != UserRole.ADMIN:
                logger.warning(f"非管理员尝试通过 Management Token 访问管理端点: {user.email}")
                raise HTTPException(status_code=403, detail="需要管理员权限")

            # 存储到 request.state
            request.state.user_id = user.id
            request.state.management_token_id = management_token.id

            return user, management_token

        # JWT 认证
        try:
            payload = await self.auth_service.verify_token(token, token_type="access")
        except HTTPException:
            raise
        except Exception as exc:
            logger.error(f"Admin token 验证失败: {exc}")
            raise HTTPException(status_code=401, detail="无效的管理员令牌")

        user_id = payload.get("user_id")
        if not user_id:
            raise HTTPException(status_code=401, detail="无效的管理员令牌")

        # 直接查询数据库，确保返回的是当前 Session 绑定的对象
        user = db.query(User).filter(User.id == user_id).first()
        if not user or not user.is_active or user.is_deleted:
            raise HTTPException(status_code=403, detail="用户不存在或已禁用")

        if not self.auth_service.token_identity_matches_user(payload, user):
            raise HTTPException(status_code=403, detail="无效的管理员令牌")

        # 检查管理员权限
        if user.role != UserRole.ADMIN:
            logger.warning(f"非管理员尝试通过 JWT 访问管理端点: {user.email}")
            raise HTTPException(status_code=403, detail="需要管理员权限")

        request.state.user_id = user.id
        return user, None

    async def _authenticate_user(
        self, request: Request, db: Session
    ) -> Tuple[User, Optional["ManagementToken"]]:
        """用户认证，支持 JWT 和 Management Token 两种方式

        认证来源优先级：
        1. Authorization header (Bearer token)
        2. URL query parameter (access_token) - 用于浏览器直接跳转场景如 OAuth 绑定
        """
        from src.models.database import ManagementToken
        from src.utils.request_utils import get_client_ip

        token: Optional[str] = None

        # 优先从 Authorization header 获取
        authorization = request.headers.get("authorization")
        if authorization and authorization.lower().startswith("bearer "):
            token = authorization[7:].strip()

        # 如果 header 中没有，尝试从 query 参数获取（用于浏览器跳转场景）
        if not token:
            token = request.query_params.get("access_token")

        if not token:
            raise HTTPException(status_code=401, detail="缺少用户凭证")

        # 检查是否为 Management Token（ae_ 前缀）
        if token.startswith(ManagementToken.TOKEN_PREFIX):
            client_ip = get_client_ip(request)
            result = await self.auth_service.authenticate_management_token(db, token, client_ip)

            if not result:
                raise HTTPException(status_code=401, detail="无效或过期的 Management Token")

            user, management_token = result

            request.state.user_id = user.id
            request.state.management_token_id = management_token.id

            return user, management_token

        # JWT 认证
        try:
            payload = await self.auth_service.verify_token(token, token_type="access")
        except HTTPException:
            raise
        except Exception as exc:
            logger.error(f"User token 验证失败: {exc}")
            raise HTTPException(status_code=401, detail="无效的用户令牌")

        user_id = payload.get("user_id")
        if not user_id:
            raise HTTPException(status_code=401, detail="无效的用户令牌")

        user = db.query(User).filter(User.id == user_id).first()
        if not user or not user.is_active or user.is_deleted:
            raise HTTPException(status_code=403, detail="用户不存在或已禁用")

        if not self.auth_service.token_identity_matches_user(payload, user):
            raise HTTPException(status_code=403, detail="无效的用户令牌")

        request.state.user_id = user.id
        return user, None

    async def _authenticate_management(
        self, request: Request, db: Session
    ) -> Tuple[User, "ManagementToken"]:
        """Management Token 认证"""
        from src.models.database import ManagementToken
        from src.utils.request_utils import get_client_ip

        authorization = request.headers.get("authorization")
        if not authorization or not authorization.lower().startswith("bearer "):
            raise HTTPException(status_code=401, detail="缺少 Management Token")

        token = authorization[7:].strip()

        # 检查是否为 Management Token 格式
        if not token.startswith(ManagementToken.TOKEN_PREFIX):
            raise HTTPException(
                status_code=401,
                detail=f"无效的 Token 格式，需要 Management Token ({ManagementToken.TOKEN_PREFIX}xxx)",
            )

        client_ip = get_client_ip(request)

        result = await self.auth_service.authenticate_management_token(db, token, client_ip)

        if not result:
            raise HTTPException(status_code=401, detail="无效或过期的 Management Token")

        user, management_token = result

        # 存储到 request.state
        request.state.user_id = user.id
        request.state.management_token_id = management_token.id

        return user, management_token

    def _calculate_quota_remaining(self, user: Optional[User]) -> Optional[float]:
        if not user:
            return None
        if user.quota_usd is None or user.quota_usd < 0:
            return None
        return max(float(user.quota_usd - user.used_usd), 0.0)

    def _record_audit_event(
        self,
        context: ApiRequestContext,
        adapter: ApiAdapter,
        *,
        success: bool,
        status_code: Optional[int] = None,
        error: Optional[str] = None,
    ) -> None:
        """记录审计事件

        事务策略：复用请求级 Session，不单独提交。
        审计记录随主事务一起提交，由中间件统一管理。
        """
        if not getattr(adapter, "audit_log_enabled", True):
            return

        if context.db is None:
            return

        event_type = adapter.audit_success_event if success else adapter.audit_failure_event
        if not event_type:
            if not success and status_code == 401:
                event_type = AuditEventType.UNAUTHORIZED_ACCESS
            else:
                event_type = (
                    AuditEventType.REQUEST_SUCCESS if success else AuditEventType.REQUEST_FAILED
                )

        metadata = self._build_audit_metadata(
            context=context,
            adapter=adapter,
            success=success,
            status_code=status_code,
            error=error,
        )

        try:
            # 复用请求级 Session，不创建新的连接
            # 审计记录随主事务一起提交，由中间件统一管理
            self.audit_service.log_event(
                db=context.db,
                event_type=event_type,
                description=f"{context.request.method} {context.request.url.path} via {adapter.name}",
                user_id=context.user.id if context.user else None,
                api_key_id=context.api_key.id if context.api_key else None,
                ip_address=context.client_ip,
                user_agent=context.user_agent,
                request_id=context.request_id,
                status_code=status_code,
                error_message=error,
                metadata=metadata,
            )
        except Exception as exc:
            # 审计失败不应影响主请求，仅记录警告
            logger.warning(f"[Audit] Failed to record event for adapter={adapter.name}: {exc}")

    def _build_audit_metadata(
        self,
        context: ApiRequestContext,
        adapter: ApiAdapter,
        *,
        success: bool,
        status_code: Optional[int],
        error: Optional[str],
    ) -> dict:
        duration_ms = max((time.time() - context.start_time) * 1000, 0.0)
        request = context.request
        path_params = {}
        try:
            path_params = dict(getattr(request, "path_params", {}) or {})
        except Exception:
            path_params = {}

        metadata: dict[str, Any] = {
            "path": request.url.path,
            "path_params": path_params,
            "method": request.method,
            "adapter": adapter.name,
            "adapter_class": adapter.__class__.__name__,
            "adapter_mode": getattr(adapter.mode, "value", str(adapter.mode)),
            "mode": context.mode,
            "api_format_hint": context.api_format_hint,
            "query": context.query_params,
            "duration_ms": round(duration_ms, 2),
            "request_body_bytes": len(context.raw_body or b""),
            "has_body": bool(context.raw_body),
            "request_content_type": request.headers.get("content-type"),
            "quota_remaining": context.quota_remaining,
            "success": success,
        }
        if status_code is not None:
            metadata["status_code"] = status_code

        if context.user and getattr(context.user, "role", None):
            role = context.user.role
            metadata["user_role"] = getattr(role, "value", role)

        if context.api_key:
            if getattr(context.api_key, "name", None):
                metadata["api_key_name"] = context.api_key.name
            # 使用脱敏后的密钥显示
            if hasattr(context.api_key, "get_display_key"):
                metadata["api_key_display"] = context.api_key.get_display_key()

        extra_details: dict[str, Any] = {}
        if context.audit_metadata:
            extra_details.update(context.audit_metadata)

        try:
            adapter_details = adapter.get_audit_metadata(
                context,
                success=success,
                status_code=status_code,
                error=error,
            )
            if adapter_details:
                extra_details.update(adapter_details)
        except Exception as exc:
            logger.warning(f"[Audit] Adapter metadata failed: {adapter.__class__.__name__}: {exc}")

        if extra_details:
            metadata["details"] = extra_details

        if error:
            metadata["error"] = error

        return self._sanitize_metadata(metadata)

    def _sanitize_metadata(self, value: Any, depth: int = 0):
        if value is None:
            return None
        if depth > 5:
            return str(value)
        if isinstance(value, (str, int, float, bool)):
            return value
        if isinstance(value, Enum):
            return value.value
        if isinstance(value, dict):
            sanitized = {}
            for key, val in value.items():
                cleaned = self._sanitize_metadata(val, depth + 1)
                if cleaned is not None:
                    sanitized[str(key)] = cleaned
            return sanitized
        if isinstance(value, (list, tuple, set)):
            return [self._sanitize_metadata(item, depth + 1) for item in value]
        if hasattr(value, "isoformat"):
            try:
                return value.isoformat()
            except Exception:
                return str(value)
        return str(value)
