"""
Sub2API 架构

针对 Sub2API 风格的中转站优化的预设配置。
支持两种认证方式：
1. 账号密码登录（自动获取 JWT，过期自动刷新，refresh 失败自动重新登录）
2. Refresh Token（从浏览器 localStorage 获取，自动续期，适合 OAuth 用户）
"""

import time
from collections.abc import AsyncIterator, Callable
from contextlib import asynccontextmanager
from typing import Any

import httpx

from src.core.logger import logger
from src.services.provider_ops.actions import ProviderAction
from src.services.provider_ops.actions.sub2api_balance import Sub2ApiBalanceAction
from src.services.provider_ops.architectures.base import (
    ProviderArchitecture,
    ProviderConnector,
    VerifyResult,
)
from src.services.provider_ops.types import ConnectorAuthType, ProviderActionType
from src.utils.ssl_utils import get_ssl_context


def _calc_expires_at(token_data: dict[str, Any]) -> float:
    """从 token 响应数据计算过期时间（秒级时间戳，提前 60s）"""
    token_expires_at = token_data.get("token_expires_at")
    if token_expires_at is not None:
        # Sub2API 返回毫秒级绝对时间戳
        return token_expires_at / 1000 - 60
    expires_in = token_data.get("expires_in", 900)
    return time.time() + expires_in - 60


async def _do_login(
    client: httpx.AsyncClient,
    email: str,
    password: str,
) -> dict[str, Any]:
    """调用 Sub2API 登录接口，返回 token_data。失败抛 ValueError。"""
    resp = await client.post(
        "/api/v1/auth/login",
        json={"email": email, "password": password},
    )
    data = resp.json()
    if resp.status_code != 200 or data.get("code", -1) != 0:
        raise ValueError(data.get("message", f"登录失败 (HTTP {resp.status_code})"))
    return data.get("data", {})


async def _do_refresh(
    client: httpx.AsyncClient,
    refresh_token: str,
) -> dict[str, Any]:
    """调用 Sub2API refresh 接口，返回 token_data。失败抛 ValueError。"""
    resp = await client.post(
        "/api/v1/auth/refresh",
        json={"refresh_token": refresh_token},
    )
    data = resp.json()
    if resp.status_code != 200 or data.get("code", -1) != 0:
        raise ValueError(data.get("message", "Refresh Token 无效或已过期"))
    return data.get("data", {})


def _collect_updated_credentials(
    token_data: dict[str, Any],
    old_refresh_token: str | None = None,
) -> dict[str, Any]:
    """从 token 响应中提取需要持久化的凭据变更"""
    updated: dict[str, Any] = {}
    new_refresh_token = token_data.get("refresh_token")
    if new_refresh_token and new_refresh_token != old_refresh_token:
        updated["refresh_token"] = new_refresh_token
    access_token = token_data.get("access_token")
    if access_token:
        updated["_cached_access_token"] = access_token
        updated["_cached_token_expires_at"] = _calc_expires_at(token_data)
    return updated


class _Sub2ApiTokenMixin:
    """Sub2API JWT token 管理公共逻辑

    与 ProviderConnector 配合使用（MRO 中由 ProviderConnector 提供实际属性初始化）。
    以下类型注解声明 mixin 依赖的协议属性，不会创建新的实例属性。
    """

    # Mixin 自身管理的 token 状态（提供默认值防止子类遗漏初始化）
    _access_token: str | None = None
    _refresh_token: str | None = None
    _token_expires_at: float = 0
    # 以下属性由 ProviderConnector.__init__ 初始化，仅作协议声明
    _on_credentials_updated: Callable[[dict[str, Any]], None] | None
    base_url: str
    _timeout: int | float
    _proxy: str | httpx.Proxy | None
    _tunnel_node_id: str | None

    @asynccontextmanager
    async def _get_raw_client(self) -> AsyncIterator[httpx.AsyncClient]:
        """获取不带 auth hook 的裸 HTTP 客户端（用于登录/刷新 token）"""
        transport = None
        if self._tunnel_node_id:
            from src.services.proxy_node.tunnel_transport import create_tunnel_transport

            transport = create_tunnel_transport(self._tunnel_node_id, timeout=self._timeout)
        elif self._proxy:
            transport = httpx.AsyncHTTPTransport(proxy=self._proxy)
        async with httpx.AsyncClient(
            base_url=self.base_url,
            timeout=self._timeout,
            transport=transport,
            verify=get_ssl_context(),
        ) as client:
            yield client

    def _update_tokens(self, token_data: dict[str, Any]) -> None:
        """更新实例 token 状态并通过回调持久化变更"""
        old_refresh_token = self._refresh_token
        self._access_token = token_data.get("access_token")
        self._refresh_token = token_data.get("refresh_token", self._refresh_token)
        self._token_expires_at = _calc_expires_at(token_data)

        if self._on_credentials_updated:
            updated = _collect_updated_credentials(token_data, old_refresh_token)
            if updated:
                self._on_credentials_updated(updated)

    async def _refresh(self) -> bool:
        """使用 refresh_token 续期"""
        if not self._refresh_token:
            return False

        try:
            async with self._get_raw_client() as client:
                token_data = await _do_refresh(client, self._refresh_token)
                self._update_tokens(token_data)
                logger.debug("Sub2API token 续期成功")
                return True
        except ValueError as e:
            logger.warning("Sub2API refresh_token 续期失败: {}", e)
            return False
        except Exception as e:
            logger.warning("Sub2API refresh_token 续期异常: {}", e)
            return False


class Sub2ApiConnector(_Sub2ApiTokenMixin, ProviderConnector):
    """
    Sub2API 连接器（账号密码模式）

    使用 email + password 登录获取 JWT Token，支持自动刷新：
    - 登录后获取 access_token + refresh_token
    - access_token 过期前自动使用 refresh_token 续期
    - refresh_token 也过期时自动重新登录
    """

    auth_type = ConnectorAuthType.SESSION_LOGIN
    display_name = "账号密码"

    def __init__(self, base_url: str, config: dict[str, Any] | None = None):
        super().__init__(base_url, config)
        self._access_token: str | None = None
        self._refresh_token: str | None = None
        self._token_expires_at: float = 0
        self._email: str | None = None
        self._password: str | None = None

    async def connect(self, credentials: dict[str, Any]) -> bool:
        email = credentials.get("email", "").strip()
        password = credentials.get("password", "").strip()
        if not email or not password:
            self._set_error("邮箱和密码不能为空")
            return False

        self._email = email
        self._password = password

        # 如果有缓存的 access_token 且未过期，直接复用，避免不必要的登录
        cached_access_token = credentials.get("_cached_access_token", "")
        cached_expires_at = credentials.get("_cached_token_expires_at", 0)
        if cached_access_token and time.time() < cached_expires_at:
            self._access_token = cached_access_token
            self._token_expires_at = cached_expires_at
            # 恢复 refresh_token 以便 access_token 过期后可刷新而非重新登录
            self._refresh_token = credentials.get("refresh_token", "").strip() or None
            self._set_connected()
            return True

        return await self._login()

    async def _login(self) -> bool:
        """使用 email + password 登录获取 token pair"""
        try:
            async with self._get_raw_client() as client:
                token_data = await _do_login(client, self._email or "", self._password or "")
                self._update_tokens(token_data)
                self._set_connected()
                logger.debug(
                    "Sub2API 登录成功: {}", self._email[:3] + "***" if self._email else "N/A"
                )
                return True

        except ValueError as e:
            self._set_error(str(e))
            return False
        except httpx.TimeoutException:
            self._set_error("登录请求超时")
            return False
        except httpx.RequestError as e:
            self._set_error(f"登录网络错误: {e}")
            return False
        except Exception as e:
            self._set_error(f"登录失败: {e}")
            return False

    async def _ensure_token(self) -> None:
        """确保 access_token 有效，过期则自动刷新或重新登录"""
        if self._access_token and time.time() < self._token_expires_at:
            return

        if self._refresh_token and await self._refresh():
            return

        logger.info("Sub2API token 已过期，尝试重新登录")
        if not await self._login():
            logger.error("Sub2API 重新登录失败: {}", self._last_error)

    async def disconnect(self) -> None:
        self._access_token = None
        self._refresh_token = None
        self._token_expires_at = 0
        self._email = None
        self._password = None
        self._set_disconnected()

    async def is_authenticated(self) -> bool:
        if not self._access_token:
            return False
        return self._refresh_token is not None or self._email is not None

    def _apply_auth(self, request: httpx.Request) -> httpx.Request:
        if self._access_token:
            request.headers["Authorization"] = f"Bearer {self._access_token}"
        return request

    async def _auth_hook(self, request: httpx.Request) -> None:
        await self._ensure_token()
        self._apply_auth(request)

    async def refresh_auth(self, credentials: dict[str, Any]) -> bool:
        if await self._refresh():
            return True
        self._email = credentials.get("email", self._email)
        self._password = credentials.get("password", self._password)
        return await self._login()

    @classmethod
    def get_credentials_schema(cls) -> dict[str, Any]:
        return {
            "type": "object",
            "properties": {
                "base_url": {
                    "type": "string",
                    "title": "站点地址",
                    "description": "API 基础地址",
                },
                "email": {
                    "type": "string",
                    "title": "邮箱",
                    "description": "Sub2API 登录邮箱",
                },
                "password": {
                    "type": "string",
                    "title": "密码",
                    "description": "Sub2API 登录密码",
                    "x-sensitive": True,
                    "x-input-type": "password",
                },
            },
            "required": ["email", "password"],
            "x-field-groups": [
                {"fields": ["base_url"]},
                {"fields": ["email"]},
                {"fields": ["password"]},
            ],
            "x-auth-type": "session_login",
            "x-auth-method": "jwt",
            "x-validation": [
                {
                    "type": "required",
                    "fields": ["email", "password"],
                    "message": "请填写邮箱和密码",
                },
            ],
        }


class Sub2ApiRefreshTokenConnector(_Sub2ApiTokenMixin, ProviderConnector):
    """
    Sub2API 连接器（Refresh Token 模式）

    适合 OAuth 登录用户（如 LinuxDo），从浏览器 localStorage 获取 refresh_token。
    - 首次连接时用 refresh_token 换取 access_token
    - access_token 过期前自动续期
    - refresh_token 过期后需手动更新（无法自动重新登录）
    """

    auth_type = ConnectorAuthType.API_KEY
    display_name = "Refresh Token"

    def __init__(self, base_url: str, config: dict[str, Any] | None = None):
        super().__init__(base_url, config)
        self._access_token: str | None = None
        self._refresh_token: str | None = None
        self._token_expires_at: float = 0

    async def connect(self, credentials: dict[str, Any]) -> bool:
        refresh_token = credentials.get("refresh_token", "").strip()

        if not refresh_token:
            self._set_error("请填写 Refresh Token")
            return False

        self._refresh_token = refresh_token

        # 如果有缓存的 access_token 且未过期，直接复用，不消耗 refresh_token
        cached_access_token = credentials.get("_cached_access_token", "")
        cached_expires_at = credentials.get("_cached_token_expires_at", 0)
        if cached_access_token and time.time() < cached_expires_at:
            self._access_token = cached_access_token
            self._token_expires_at = cached_expires_at
            self._set_connected()
            return True

        # 首次连接或 access_token 已过期，用 refresh_token 换取
        if not await self._refresh():
            self._refresh_token = None  # 清理，避免残留无效状态
            self._set_error("Refresh Token 无效或已过期")
            return False

        self._set_connected()
        return True

    async def _ensure_token(self) -> None:
        """确保 access_token 有效，有 refresh_token 时自动续期"""
        if self._access_token and time.time() < self._token_expires_at:
            return
        if self._refresh_token:
            await self._refresh()

    async def disconnect(self) -> None:
        self._access_token = None
        self._refresh_token = None
        self._token_expires_at = 0
        self._set_disconnected()

    async def is_authenticated(self) -> bool:
        return self._refresh_token is not None

    def _apply_auth(self, request: httpx.Request) -> httpx.Request:
        if self._access_token:
            request.headers["Authorization"] = f"Bearer {self._access_token}"
        return request

    async def _auth_hook(self, request: httpx.Request) -> None:
        await self._ensure_token()
        self._apply_auth(request)

    async def refresh_auth(self, credentials: dict[str, Any]) -> bool:
        if self._refresh_token and await self._refresh():
            return True
        return await self.connect(credentials)

    @classmethod
    def get_credentials_schema(cls) -> dict[str, Any]:
        return {
            "type": "object",
            "properties": {
                "base_url": {
                    "type": "string",
                    "title": "站点地址",
                    "description": "API 基础地址",
                },
                "refresh_token": {
                    "type": "string",
                    "title": "Refresh Token",
                    "description": ("从浏览器 F12 > Application > Local Storage 获取"),
                    "x-sensitive": True,
                    "x-input-type": "password",
                    "x-help": "浏览器控制台执行 localStorage.getItem('refresh_token') 获取",
                },
            },
            "required": ["refresh_token"],
            "x-field-groups": [
                {"fields": ["base_url"]},
                {"fields": ["refresh_token"]},
            ],
            "x-auth-type": "api_key",
            "x-auth-method": "bearer",
            "x-validation": [
                {
                    "type": "required",
                    "fields": ["refresh_token"],
                    "message": "请填写 Refresh Token",
                },
            ],
        }


class Sub2ApiArchitecture(ProviderArchitecture):
    """
    Sub2API 架构

    特点：
    - 支持两种认证方式：账号密码 / Refresh Token
    - 验证端点: /api/v1/auth/me
    - balance 为充值余额，points 为赠送余额
    - 余额查询同时获取订阅概览信息
    """

    architecture_id = "sub2api"
    display_name = "Sub2API"
    description = "Sub2API 风格中转站的预设配置"

    supported_connectors: list[type[ProviderConnector]] = [
        Sub2ApiConnector,
        Sub2ApiRefreshTokenConnector,
    ]

    supported_actions: list[type[ProviderAction]] = [Sub2ApiBalanceAction]

    default_action_configs: dict[ProviderActionType, dict[str, Any]] = {
        ProviderActionType.QUERY_BALANCE: {
            "endpoint": "/api/v1/auth/me?timezone=Asia/Shanghai",
            "subscription_endpoint": "/api/v1/subscriptions/summary",
            "method": "GET",
        },
    }

    def get_credentials_schema(self) -> dict[str, Any]:
        return Sub2ApiConnector.get_credentials_schema()

    def get_verify_endpoint(self) -> str:
        return "/api/v1/auth/me?timezone=Asia/Shanghai"

    def build_verify_headers(
        self,
        config: dict[str, Any],
        credentials: dict[str, Any],
    ) -> dict[str, str]:
        headers: dict[str, str] = {}
        access_token = credentials.get("_access_token", "")
        if access_token:
            headers["Authorization"] = f"Bearer {access_token}"
        return headers

    async def prepare_verify_config(
        self,
        base_url: str,
        config: dict[str, Any],
        credentials: dict[str, Any],
    ) -> tuple[dict[str, Any], dict[str, Any]]:
        """
        验证前预处理：根据凭据类型选择登录方式

        - 有 email + password -> 账号密码登录
        - 有 refresh_token -> 用 refresh_token 换 access_token

        Returns:
            (extra_config, updated_credentials):
                extra_config 为空；updated_credentials 包含需持久化的凭据变更
                （Token Rotation 后的新 refresh_token、缓存的 access_token 等）
        """
        base_url = base_url.rstrip("/")

        from src.services.proxy_node.resolver import resolve_ops_proxy_config_async

        proxy, tunnel_node_id = await resolve_ops_proxy_config_async(config)
        client_kwargs: dict[str, Any] = {
            "base_url": base_url,
            "timeout": 30.0,
            "verify": get_ssl_context(),
        }
        if tunnel_node_id:
            from src.services.proxy_node.tunnel_transport import create_tunnel_transport

            client_kwargs["transport"] = create_tunnel_transport(tunnel_node_id, timeout=30.0)
        elif proxy:
            client_kwargs["proxy"] = proxy

        email = credentials.get("email", "").strip()
        password = credentials.get("password", "").strip()
        refresh_token = credentials.get("refresh_token", "").strip()

        try:
            if email and password:
                async with httpx.AsyncClient(**client_kwargs) as client:
                    token_data = await _do_login(client, email, password)

            elif refresh_token:
                async with httpx.AsyncClient(**client_kwargs) as client:
                    token_data = await _do_refresh(client, refresh_token)

            else:
                raise ValueError("请填写账号密码或 Refresh Token")

        except ValueError:
            raise
        except Exception as e:
            raise ValueError(f"验证失败: {e}") from e

        access_token = token_data.get("access_token", "")
        credentials["_access_token"] = access_token

        updated_credentials = _collect_updated_credentials(
            token_data, old_refresh_token=refresh_token or None
        )

        return {}, updated_credentials

    def parse_verify_response(
        self,
        status_code: int,
        data: dict[str, Any],
    ) -> VerifyResult:
        if status_code == 401:
            return VerifyResult(success=False, message=self._auth_fail_message(401))
        if status_code == 403:
            return VerifyResult(success=False, message=self._auth_fail_message(403))
        if status_code != 200:
            return VerifyResult(success=False, message=f"验证失败：HTTP {status_code}")

        code = data.get("code")
        if code != 0:
            message = data.get("message", "验证失败")
            return VerifyResult(success=False, message=message)

        user_data = data.get("data", {})
        return self._build_verify_result(user_data, data)

    def _build_verify_result(
        self, user_data: dict[str, Any], raw_data: dict[str, Any] | None = None
    ) -> VerifyResult:
        balance = float(user_data.get("balance") or 0)
        points = float(user_data.get("points") or 0)

        return VerifyResult(
            success=True,
            username=user_data.get("username") or user_data.get("email"),
            display_name=user_data.get("username") or user_data.get("email"),
            email=user_data.get("email"),
            quota=balance + points,
            extra={
                "balance": balance,
                "points": points,
                "status": user_data.get("status"),
                "concurrency": user_data.get("concurrency"),
            },
        )
