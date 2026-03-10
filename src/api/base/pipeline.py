from __future__ import annotations

import time
from enum import Enum
from typing import TYPE_CHECKING, Any

from fastapi import HTTPException, Request
from fastapi.responses import JSONResponse
from sqlalchemy.exc import SQLAlchemyError
from sqlalchemy.orm import Session
from starlette.requests import ClientDisconnect

from src.config.settings import config
from src.core.enums import UserRole
from src.core.exceptions import BalanceInsufficientException
from src.core.logger import logger
from src.models.database import ApiKey, AuditEventType, User
from src.services.auth.service import AuthService
from src.services.system.audit import AuditService
from src.services.usage.service import UsageService
from src.services.wallet import WalletService
from src.utils.perf import PerfRecorder

if TYPE_CHECKING:
    from src.models.database import ManagementToken

from .adapter import ApiAdapter, ApiMode
from .context import ApiRequestContext

# 高频轮询端点，抑制其 debug 日志以减少噪音
QUIET_POLLING_PATHS: set[str] = {
    "/api/admin/usage/active",
    "/api/admin/usage/records",
    "/api/admin/usage/stats",
    "/api/admin/usage/aggregation/stats",
    "/api/admin/health/status",
}


class ApiRequestPipeline:
    """负责统一执行认证、余额校验、上下文构建等通用逻辑的管道。"""

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
        api_format_hint: str | None = None,
        path_params: dict[str, Any] | None = None,
    ) -> Any:
        perf_labels = {
            "mode": getattr(mode, "value", str(mode)),
            "adapter": adapter.name,
        }
        perf_sampled = PerfRecorder.should_store_sample()
        if perf_sampled:
            setattr(http_request.state, "perf_sampled", True)
            setattr(
                http_request.state,
                "perf_metrics",
                {"pipeline": {}, "sample_rate": getattr(config, "perf_store_sample_rate", 1.0)},
            )

        def _record_perf_metric(key: str, duration: float | None) -> None:
            if duration is None:
                return
            perf_metrics = getattr(http_request.state, "perf_metrics", None)
            if not isinstance(perf_metrics, dict):
                return
            bucket = perf_metrics.setdefault("pipeline", {})
            bucket[key] = int(duration * 1000)

        # 高频轮询端点抑制 debug 日志
        is_quiet = http_request.url.path in QUIET_POLLING_PATHS
        if not is_quiet:
            logger.debug(
                "[Pipeline] {} {} | adapter={}, mode={}",
                http_request.method,
                http_request.url.path,
                adapter.__class__.__name__,
                mode,
            )
        auth_start = PerfRecorder.start(force=perf_sampled)
        try:
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
                user, api_key = self._authenticate_client(http_request, db, adapter, quiet=is_quiet)
                management_token = None
        finally:
            auth_duration = PerfRecorder.stop(auth_start, "pipeline_auth", labels=perf_labels)
            _record_perf_metric("auth_ms", auth_duration)

        raw_body = None
        if http_request.method in {"POST", "PUT", "PATCH"}:
            try:
                import asyncio

                # 添加超时防止卡死
                body_start = PerfRecorder.start(force=perf_sampled)
                body_size = 0
                try:
                    raw_body = await asyncio.wait_for(
                        http_request.body(), timeout=config.request_body_timeout
                    )
                    body_size = len(raw_body) if raw_body is not None else 0
                finally:
                    body_duration = PerfRecorder.stop(
                        body_start,
                        "pipeline_body_read",
                        labels=perf_labels,
                        log_hint=f"size={body_size}",
                    )
                    _record_perf_metric("body_read_ms", body_duration)
                    if perf_sampled:
                        perf_metrics = getattr(http_request.state, "perf_metrics", None)
                        if isinstance(perf_metrics, dict):
                            perf_metrics.setdefault("pipeline", {})["body_bytes"] = int(body_size)
            except TimeoutError:
                timeout_sec = int(config.request_body_timeout)
                logger.error(f"读取请求体超时({timeout_sec}s),可能客户端未发送完整请求体")
                raise HTTPException(
                    status_code=408,
                    detail=f"Request timeout: body not received within {timeout_sec} seconds",
                )
            except ClientDisconnect:
                logger.warning(
                    "[Pipeline] 客户端在读取请求体期间断开连接: {} {}",
                    http_request.method,
                    http_request.url.path,
                )
                return JSONResponse(
                    status_code=499,
                    content={"error": "client_disconnected", "message": "Client closed request"},
                )
        context_start = PerfRecorder.start(force=perf_sampled)
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
        context_duration = PerfRecorder.stop(
            context_start, "pipeline_context_build", labels=perf_labels
        )
        _record_perf_metric("context_build_ms", context_duration)
        # 存储 management_token 到 context（用于权限检查）
        if management_token:
            context.management_token = management_token
        # 存储 quiet 标志到 context，用于审计日志判断
        context.quiet_logging = is_quiet
        if mode != ApiMode.ADMIN and user:
            remaining = self._calculate_balance_remaining(db, user, api_key=api_key)
            context.balance_remaining = remaining
        # authorize 可能是异步的，需要检查并 await
        authorize_start = PerfRecorder.start(force=perf_sampled)
        try:
            authorize_result = adapter.authorize(context)
            if hasattr(authorize_result, "__await__"):
                await authorize_result
        finally:
            authorize_duration = PerfRecorder.stop(
                authorize_start, "pipeline_authorize", labels=perf_labels
            )
            _record_perf_metric("authorize_ms", authorize_duration)

        try:
            handle_start = PerfRecorder.start(force=perf_sampled)
            response = await adapter.handle(context)
            handle_duration = PerfRecorder.stop(handle_start, "pipeline_handle", labels=perf_labels)
            _record_perf_metric("handle_ms", handle_duration)
            status_code = getattr(response, "status_code", None)
            self._record_audit_event(context, adapter, success=True, status_code=status_code)
            return response
        except HTTPException as exc:
            handle_duration = PerfRecorder.stop(handle_start, "pipeline_handle", labels=perf_labels)
            _record_perf_metric("handle_ms", handle_duration)
            err_detail = exc.detail if isinstance(exc.detail, str) else str(exc.detail)
            self._record_audit_event(
                context,
                adapter,
                success=False,
                status_code=exc.status_code,
                error=err_detail,
            )
            raise
        except ClientDisconnect:
            handle_duration = PerfRecorder.stop(handle_start, "pipeline_handle", labels=perf_labels)
            _record_perf_metric("handle_ms", handle_duration)
            logger.warning(
                "[Pipeline] 客户端在处理期间断开连接: {} {}",
                context.request.method,
                context.request.url.path,
            )
            self._record_audit_event(
                context,
                adapter,
                success=False,
                status_code=499,
                error="client_disconnected",
            )
            return JSONResponse(
                status_code=499,
                content={"error": "client_disconnected", "message": "Client closed request"},
            )
        except Exception as exc:
            handle_duration = PerfRecorder.stop(handle_start, "pipeline_handle", labels=perf_labels)
            _record_perf_metric("handle_ms", handle_duration)
            if isinstance(exc, SQLAlchemyError):
                # SQL 执行失败后事务会进入 aborted 状态；先回滚，避免审计写入二次报错。
                try:
                    context.db.rollback()
                except Exception as rollback_exc:
                    logger.debug(f"[Pipeline] 回滚失败（可忽略）: {rollback_exc}")
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
        self, request: Request, db: Session, adapter: ApiAdapter, **_kw: object
    ) -> tuple[User, ApiKey]:
        # 使用 adapter 的 extract_api_key 方法，支持不同 API 格式的认证头
        client_api_key = adapter.extract_api_key(request)
        if not client_api_key:
            raise HTTPException(status_code=401, detail="请提供API密钥")

        auth_result = self.auth_service.authenticate_api_key(db, client_api_key)
        if not auth_result:
            raise HTTPException(status_code=401, detail="无效的API密钥")

        user, api_key = auth_result
        if not user or not api_key:
            raise HTTPException(status_code=401, detail="无效的API密钥")

        request.state.user_id = user.id
        request.state.api_key_id = api_key.id

        # 检查余额（支持独立 Key）
        access_ok, _message = self.usage_service.check_request_balance(db, user, api_key=api_key)
        if not access_ok:
            remaining = self._calculate_balance_remaining(db, user, api_key=api_key)
            raise BalanceInsufficientException(balance_type="USD", remaining=remaining)

        return user, api_key

    async def _try_token_prefix_auth(
        self, token: str, request: Request, db: Session
    ) -> tuple[User, Any] | None:
        """尝试通过模块注册的 token 前缀认证器认证

        Returns:
            (User, token_record) 元组，或 None（无前缀匹配）

        Raises:
            HTTPException: 前缀匹配但认证失败时抛出 401
        """
        from src.core.modules.hooks import AUTH_TOKEN_PREFIX_AUTHENTICATORS, get_hook_dispatcher
        from src.utils.request_utils import get_client_ip

        authenticators = await get_hook_dispatcher().dispatch(
            AUTH_TOKEN_PREFIX_AUTHENTICATORS, db=db
        )
        for auth_info in authenticators or []:
            prefix = auth_info.get("prefix", "")
            authenticate_fn = auth_info.get("authenticate")
            if prefix and token.startswith(prefix):
                if not authenticate_fn:
                    logger.warning("Token prefix '{}' has no authenticate callback", prefix)
                    raise HTTPException(status_code=401, detail="认证服务不可用")
                client_ip = get_client_ip(request)
                result = await authenticate_fn(db, token, client_ip)
                if result:
                    return result
                # 前缀匹配但认证失败
                module_name = auth_info.get("module", "unknown")
                raise HTTPException(status_code=401, detail=f"无效或过期的 Token ({module_name})")
        return None  # 无前缀匹配

    async def _authenticate_admin(
        self, request: Request, db: Session
    ) -> tuple[User, ManagementToken | None]:
        """管理员认证，支持 JWT 和 Management Token 两种方式"""
        authorization = request.headers.get("authorization")
        if not authorization or not authorization.lower().startswith("bearer "):
            raise HTTPException(status_code=401, detail="缺少管理员凭证")

        token = authorization[7:].strip()

        # 通过钩子检查是否匹配模块注册的 token 前缀（如 ae_）
        token_auth_result = await self._try_token_prefix_auth(token, request, db)
        if token_auth_result is not None:
            user, management_token = token_auth_result

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
    ) -> tuple[User, ManagementToken | None]:
        """用户认证，支持 JWT 和 Management Token 两种方式"""
        authorization = request.headers.get("authorization")
        if not authorization or not authorization.lower().startswith("bearer "):
            raise HTTPException(status_code=401, detail="缺少用户凭证")

        token = authorization[7:].strip()

        # 通过钩子检查是否匹配模块注册的 token 前缀（如 ae_）
        token_auth_result = await self._try_token_prefix_auth(token, request, db)
        if token_auth_result is not None:
            user, management_token = token_auth_result

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
    ) -> tuple[User, ManagementToken]:
        """Management Token 认证"""
        authorization = request.headers.get("authorization")
        if not authorization or not authorization.lower().startswith("bearer "):
            raise HTTPException(status_code=401, detail="缺少 Management Token")

        token = authorization[7:].strip()

        # 通过钩子检查是否匹配模块注册的 token 前缀
        # _try_token_prefix_auth 会在前缀匹配但认证失败时直接抛 HTTPException
        token_auth_result = await self._try_token_prefix_auth(token, request, db)
        if token_auth_result is not None:
            user, management_token = token_auth_result

            # 存储到 request.state
            request.state.user_id = user.id
            request.state.management_token_id = management_token.id

            return user, management_token

        raise HTTPException(
            status_code=401,
            detail="无效的 Token 格式，需要 Management Token",
        )

    def _calculate_balance_remaining(
        self, db: Session, user: User | None, api_key: ApiKey | None = None
    ) -> float | None:
        if not user:
            return None
        balance = WalletService.get_balance_snapshot(db, user=user, api_key=api_key)
        if balance is None:
            return None
        return float(balance)

    def _record_audit_event(
        self,
        context: ApiRequestContext,
        adapter: ApiAdapter,
        *,
        success: bool,
        status_code: int | None = None,
        error: str | None = None,
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
        status_code: int | None,
        error: str | None,
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
            "balance_remaining": context.balance_remaining,
            "success": success,
            # 传递 quiet_logging 标志给审计服务，用于抑制高频轮询日志
            "quiet_logging": getattr(context, "quiet_logging", False),
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

    def _sanitize_metadata(self, value: Any, depth: int = 0) -> None:
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
