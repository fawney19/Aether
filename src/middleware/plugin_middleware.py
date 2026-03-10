"""
统一的插件中间件（纯 ASGI 实现）
负责协调所有插件的调用

注意：使用纯 ASGI middleware 而非 BaseHTTPMiddleware，
以避免 Starlette 已知的流式响应兼容性问题。
"""

from __future__ import annotations

import hashlib
import time
from typing import TYPE_CHECKING

from starlette.requests import Request

if TYPE_CHECKING:
    from sqlalchemy.orm import Session
from starlette.types import ASGIApp, Message, Receive, Scope, Send

from src.config import config
from src.core.logger import logger
from src.plugins.manager import get_plugin_manager
from src.plugins.rate_limit.base import RateLimitResult


class PluginMiddleware:
    """
    统一的插件调用中间件（纯 ASGI 实现）

    职责:
    - 性能监控
    - 限流控制 (可选)
    - 数据库会话生命周期管理

    注意: 认证由各路由通过 Depends() 显式声明，不在中间件层处理

    为什么使用纯 ASGI 而非 BaseHTTPMiddleware:
    - BaseHTTPMiddleware 会缓冲整个响应体，对流式响应不友好
    - BaseHTTPMiddleware 与 StreamingResponse 存在已知兼容性问题
    - 纯 ASGI 可以直接透传流式响应，无额外开销
    """

    def __init__(self, app: ASGIApp) -> None:
        self.app = app
        self.plugin_manager = get_plugin_manager()

        # 从配置读取速率限制值
        self.llm_api_rate_limit = config.llm_api_rate_limit
        self.public_api_rate_limit = config.public_api_rate_limit

        # 完全跳过限流的路径（静态资源、文档等）
        self.skip_rate_limit_paths = [
            "/health",
            "/docs",
            "/redoc",
            "/openapi.json",
            "/favicon.ico",
            "/static/",
            "/assets/",
            "/api/admin/",  # 管理后台已有JWT认证，不需要额外限流
            "/api/auth/",  # 认证端点（由路由层的 IPRateLimiter 处理）
            "/api/users/",  # 用户端点
            "/api/monitoring/",  # 监控端点
        ]

        # LLM API 端点（需要特殊的速率限制策略）
        self.llm_api_paths = [
            "/v1/messages",
            "/v1/chat/completions",
            "/v1/responses",
            "/v1/completions",
        ]

    async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
        """ASGI 入口点"""
        if scope["type"] != "http":
            # 非 HTTP 请求（如 WebSocket）直接透传
            await self.app(scope, receive, send)
            return

        # 构建 Request 对象以便复用现有逻辑
        request = Request(scope, receive, send)

        # 记录请求开始时间
        start_time = time.time()

        # 设置 request.state 属性
        # 注意：Starlette 的 Request 对象总是有 state 属性（State 实例）
        request.state.request_id = request.headers.get("x-request-id", "")
        request.state.start_time = start_time
        # 标记：若请求过程中通过 Depends(get_db) 创建了会话，则由本中间件统一管理其生命周期
        request.state.db_managed_by_middleware = True

        # 1. 限流检查（在调用下游之前）
        rate_limit_result = await self._call_rate_limit_plugins(request)
        if rate_limit_result and not rate_limit_result.allowed:
            # 限流触发，返回429
            await self._send_rate_limit_response(send, rate_limit_result)
            return

        # 2. 预处理插件调用
        await self._call_pre_request_plugins(request)

        # 用于捕获响应状态码
        response_status_code: int = 0

        async def send_wrapper(message: Message) -> None:
            nonlocal response_status_code

            if message["type"] == "http.response.start":
                response_status_code = message.get("status", 0)
                await self._maybe_release_streaming_db_session(request, message)

            await send(message)

        # 3. 调用下游应用
        exception_occurred: Exception | None = None
        try:
            await self.app(scope, receive, send_wrapper)
        except Exception as e:
            exception_occurred = e
            # 错误处理插件调用
            await self._call_error_plugins(request, e, start_time)
            raise
        finally:
            # 4. 数据库会话清理（无论成功与否）
            await self._cleanup_db_session(request, exception_occurred)

        # 5. 后处理插件调用（仅在成功时）
        if not exception_occurred and response_status_code > 0:
            await self._call_post_request_plugins(request, response_status_code, start_time)

    async def _send_rate_limit_response(self, send: Send, result: RateLimitResult) -> None:
        """发送 429 限流响应"""
        import json

        body = json.dumps(
            {
                "type": "error",
                "error": {
                    "type": "rate_limit_error",
                    "message": result.message or "Rate limit exceeded",
                },
            }
        ).encode("utf-8")

        headers = [(b"content-type", b"application/json")]
        if result.headers:
            for key, value in result.headers.items():
                headers.append((key.lower().encode(), str(value).encode()))

        await send(
            {
                "type": "http.response.start",
                "status": 429,
                "headers": headers,
            }
        )
        await send(
            {
                "type": "http.response.body",
                "body": body,
            }
        )

    def _finalize_db_session(
        self,
        db: Session,
        *,
        should_commit: bool,
        should_rollback: bool,
        log_prefix: str = "",
    ) -> None:
        """统一的数据库会话清理逻辑

        Args:
            db: SQLAlchemy 会话
            should_commit: 是否需要提交（仅当 should_rollback=False 时生效）
            should_rollback: 是否需要回滚（优先于 commit）
            log_prefix: 日志前缀，用于区分调用场景
        """
        try:
            if should_rollback:
                try:
                    db.rollback()
                except Exception as rollback_error:
                    logger.debug(f"{log_prefix}回滚事务时出错（可忽略）: {rollback_error}")
            elif should_commit:
                try:
                    db.commit()
                except Exception as commit_error:
                    logger.error(f"{log_prefix}提交失败: {commit_error}")
                    try:
                        db.rollback()
                    except Exception:
                        pass
        finally:
            try:
                db.close()
            except Exception as close_error:
                logger.debug(f"{log_prefix}关闭数据库连接时出错（可忽略）: {close_error}")

    async def _cleanup_db_session(self, request: Request, exception: Exception | None) -> None:
        """清理数据库会话

        事务策略：
        - 如果 request.state.tx_committed_by_route 为 True，说明路由已自行提交，中间件只负责 close
        - 否则由中间件统一 commit/rollback

        这避免了双重提交的问题，同时保持向后兼容。
        """
        from sqlalchemy.orm import Session

        if getattr(request.state, "db_released_early", False):
            return
        if not getattr(request.state, "db_managed_by_middleware", False):
            return

        db = getattr(request.state, "db", None)
        if not isinstance(db, Session):
            return

        tx_committed_by_route = getattr(request.state, "tx_committed_by_route", False)
        self._finalize_db_session(
            db,
            should_commit=not tx_committed_by_route and exception is None,
            should_rollback=exception is not None,
        )

    async def _maybe_release_streaming_db_session(self, request: Request, message: Message) -> None:
        """在 SSE 响应开始时提前释放请求级 DB session。"""
        if getattr(request.state, "db_released_early", False):
            return
        if not getattr(request.state, "db_managed_by_middleware", False):
            return

        headers = message.get("headers") or []
        content_type = None
        for key, value in headers:
            if key.lower() == b"content-type":
                content_type = value.decode("utf-8", errors="ignore").lower()
                break

        if not content_type or "text/event-stream" not in content_type:
            return

        from sqlalchemy.orm import Session

        db = getattr(request.state, "db", None)
        if not isinstance(db, Session):
            return

        tx_committed_by_route = getattr(request.state, "tx_committed_by_route", False)
        self._finalize_db_session(
            db,
            should_commit=not tx_committed_by_route,
            should_rollback=False,
            log_prefix="流式响应提前",
        )
        request.state.db = None
        request.state.db_released_early = True

    def _get_client_ip(self, request: Request) -> str:
        """
        获取客户端 IP 地址，支持代理头

        优先级：X-Real-IP > X-Forwarded-For > 直连 IP
        X-Real-IP 由最外层 Nginx 设置，最可靠
        """
        # 优先检查 X-Real-IP（由最外层 Nginx 设置，最可靠）
        real_ip = request.headers.get("x-real-ip")
        if real_ip:
            return real_ip.strip()

        # 检查 X-Forwarded-For，取第一个 IP（原始客户端）
        forwarded_for = request.headers.get("x-forwarded-for")
        if forwarded_for:
            ips = [ip.strip() for ip in forwarded_for.split(",") if ip.strip()]
            if ips:
                return ips[0]

        # 回退到直连 IP
        if request.client:
            return request.client.host

        return "unknown"

    def _is_llm_api_path(self, path: str) -> bool:
        """检查是否为 LLM API 端点"""
        for llm_path in self.llm_api_paths:
            if path.startswith(llm_path):
                return True
        return False

    async def _get_rate_limit_key_and_config(
        self, request: Request
    ) -> tuple[str | None, int | None]:
        """
        获取速率限制的key和配置

        策略说明:
        - /v1/messages, /v1/chat/completions 等 LLM API: 按 API Key 限流
        - /api/public/* 端点: 使用服务器级别 IP 限制
        - /api/admin/* 端点: 跳过（在 skip_rate_limit_paths 中跳过）
        - /api/auth/* 端点: 跳过（由路由层的 IPRateLimiter 处理）

        Returns:
            (key, rate_limit_value) - key用于标识限制对象，rate_limit_value是限制值
        """
        path = request.url.path

        # LLM API 端点: 按 API Key 或 IP 限流
        if self._is_llm_api_path(path):
            # 尝试从请求头获取 API Key
            auth_header = request.headers.get("authorization", "")
            api_key = request.headers.get("x-api-key", "")

            if auth_header.lower().startswith("bearer "):
                api_key = auth_header[7:]

            if api_key:
                # 使用 API Key 的哈希作为限制 key（避免日志泄露完整 key）
                key_hash = hashlib.sha256(api_key.encode()).hexdigest()[:16]
                key = f"llm_api_key:{key_hash}"
                request.state.rate_limit_key_type = "api_key"
            else:
                # 无 API Key 时使用 IP 限制（更严格）
                client_ip = self._get_client_ip(request)
                key = f"llm_ip:{client_ip}"
                request.state.rate_limit_key_type = "ip"

            rate_limit = self.llm_api_rate_limit
            request.state.rate_limit_value = rate_limit
            return key, rate_limit

        # /api/public/* 端点: 使用服务器级别 IP 地址作为限制 key
        if path.startswith("/api/public/"):
            client_ip = self._get_client_ip(request)
            key = f"public_ip:{client_ip}"
            rate_limit = self.public_api_rate_limit
            request.state.rate_limit_key_type = "public_ip"
            request.state.rate_limit_value = rate_limit
            return key, rate_limit

        # 其他端点不应用速率限制（或已在 skip_rate_limit_paths 中跳过）
        return None, None

    async def _call_rate_limit_plugins(self, request: Request) -> RateLimitResult | None:
        """调用限流插件"""

        # 跳过不需要限流的路径（支持前缀匹配）
        for skip_path in self.skip_rate_limit_paths:
            if request.url.path == skip_path or request.url.path.startswith(skip_path):
                return None

        # 获取限流插件
        rate_limit_plugin = self.plugin_manager.get_plugin("rate_limit")
        if not rate_limit_plugin or not rate_limit_plugin.enabled:
            # 如果没有限流插件，允许通过
            return None

        # 获取速率限制的 key 和配置
        key, rate_limit_value = await self._get_rate_limit_key_and_config(request)
        if not key:
            # 不需要限流的端点（如未分类路径），静默跳过
            return None

        try:
            # 检查速率限制，传入数据库配置的限制值
            result = await rate_limit_plugin.check_limit(
                key=key,
                endpoint=request.url.path,
                method=request.method,
                rate_limit=rate_limit_value,  # 传入配置的限制值
            )
            # 类型检查：确保返回的是RateLimitResult类型
            if isinstance(result, RateLimitResult):
                # 如果检查通过，实际消耗令牌
                if result.allowed:
                    await rate_limit_plugin.consume(
                        key=key,
                        amount=1,
                        rate_limit=rate_limit_value,
                    )
                else:
                    # 限流触发，记录日志
                    logger.warning(
                        "速率限制触发: {}",
                        getattr(request.state, "rate_limit_key_type", "unknown"),
                    )
                return result
            return None
        except ConnectionError as e:
            # Redis 连接错误：根据配置决定
            logger.warning(f"Rate limit connection error: {e}")
            if config.rate_limit_fail_open:
                return None
            else:
                return RateLimitResult(
                    allowed=False,
                    remaining=0,
                    retry_after=30,
                    message="Rate limit service unavailable",
                )
        except TimeoutError as e:
            # 超时错误：可能是负载过高，根据配置决定
            logger.warning(f"Rate limit timeout: {e}")
            if config.rate_limit_fail_open:
                return None
            else:
                return RateLimitResult(
                    allowed=False, remaining=0, retry_after=30, message="Rate limit service timeout"
                )
        except Exception as e:
            logger.error(f"Rate limit error: {type(e).__name__}: {e}")
            # 其他异常：根据配置决定
            if config.rate_limit_fail_open:
                # fail-open: 异常时放行请求（优先可用性）
                return None
            else:
                # fail-close: 异常时拒绝请求（优先安全性）
                return RateLimitResult(
                    allowed=False, remaining=0, retry_after=60, message="Rate limit service error"
                )

    async def _call_pre_request_plugins(self, request: Request) -> None:
        """调用请求前的插件（当前保留扩展点）"""
        pass

    async def _call_post_request_plugins(
        self, request: Request, status_code: int, start_time: float
    ) -> None:
        """调用请求后的插件"""

        duration = time.time() - start_time

        # 监控插件 - 记录指标
        monitor_plugin = self.plugin_manager.get_plugin("monitor")
        if monitor_plugin and monitor_plugin.enabled:
            try:
                # 使用路由模板而非实际路径，避免动态段导致 Prometheus 标签基数爆炸
                route = request.scope.get("route")
                endpoint_label = route.path if route and hasattr(route, "path") else "unknown"

                monitor_labels = {
                    "method": request.method,
                    "endpoint": endpoint_label,
                    "status": str(status_code),
                    "status_class": f"{status_code // 100}xx",
                }

                # 记录请求计数
                await monitor_plugin.increment(
                    "http_requests_total",
                    labels=monitor_labels,
                )

                # 记录请求时长
                await monitor_plugin.timing(
                    "http_request_duration",
                    duration,
                    labels=monitor_labels,
                )
            except Exception as e:
                logger.error(f"Monitor plugin failed: {e}")

    async def _call_error_plugins(
        self, request: Request, error: Exception, start_time: float
    ) -> None:
        """调用错误处理插件"""
        from fastapi import HTTPException

        duration = time.time() - start_time

        # 通知插件 - 发送严重错误通知
        if not isinstance(error, HTTPException) or error.status_code >= 500:
            notification_plugin = self.plugin_manager.get_plugin("notification")
            if notification_plugin and notification_plugin.enabled:
                try:
                    await notification_plugin.send_error(
                        error=error,
                        context={
                            "endpoint": f"{request.method} {request.url.path}",
                            "request_id": getattr(request.state, "request_id", ""),
                            "duration": duration,
                        },
                    )
                except Exception as e:
                    logger.error(f"Notification plugin failed: {e}")
