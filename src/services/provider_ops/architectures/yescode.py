"""
YesCode 架构

针对 YesCode 中转站的预设配置。
"""

from typing import Any

import httpx

from src.services.provider_ops.actions import ProviderAction
from src.services.provider_ops.actions.yescode_balance import (
    YesCodeBalanceAction,
    fetch_yescode_combined_data,
    parse_yescode_balance_extra,
)
from src.services.provider_ops.architectures.base import (
    ProviderArchitecture,
    ProviderConnector,
    VerifyResult,
)
from src.services.provider_ops.types import ConnectorAuthType, ProviderActionType
from src.utils.ssl_utils import get_ssl_context


def _extract_cookies(cookie_string: str) -> dict[str, str]:
    """
    从完整的 Cookie 字符串中提取 yescode_auth 和 yescode_csrf

    Args:
        cookie_string: Cookie 字符串

    Returns:
        包含 yescode_auth 和 yescode_csrf 的字典
    """
    result: dict[str, str] = {}
    for part in cookie_string.split(";"):
        part = part.strip()
        if "=" in part:
            key, value = part.split("=", 1)
            key = key.strip()
            if key in ("yescode_auth", "yescode_csrf"):
                result[key] = value.strip()
    return result


def _build_cookie_header(cookie_string: str) -> str:
    """
    从输入的 Cookie 字符串构建请求用的 Cookie header

    支持两种输入格式：
    1. 完整 Cookie: "yescode_auth=xxx; yescode_csrf=yyy"
    2. 仅 auth 值: "eyJhbGciOiJI..."

    Args:
        cookie_string: Cookie 字符串或 auth 值

    Returns:
        Cookie header 值
    """
    # 如果包含 "yescode_auth="，说明是完整 Cookie 字符串
    if "yescode_auth=" in cookie_string:
        cookies = _extract_cookies(cookie_string)
        parts = []
        if "yescode_auth" in cookies:
            parts.append(f"yescode_auth={cookies['yescode_auth']}")
        if "yescode_csrf" in cookies:
            parts.append(f"yescode_csrf={cookies['yescode_csrf']}")
        return "; ".join(parts)
    # 否则认为直接是 auth 值
    return f"yescode_auth={cookie_string.strip()}"


class YesCodeConnector(ProviderConnector):
    """
    YesCode 专用连接器

    特点：
    - 使用 Cookie 认证（yescode_auth JWT + yescode_csrf）
    """

    auth_type = ConnectorAuthType.COOKIE
    display_name = "YesCode Cookie"

    def __init__(self, base_url: str, config: dict[str, Any] | None = None):
        super().__init__(base_url, config)
        self._auth_cookie: str | None = None

    async def connect(self, credentials: dict[str, Any]) -> bool:
        """建立连接"""
        auth_cookie = credentials.get("auth_cookie")
        if not auth_cookie:
            self._set_error("Auth Cookie 不能为空")
            return False

        # 构建 Cookie header
        self._auth_cookie = _build_cookie_header(auth_cookie)

        self._set_connected()
        return True

    async def disconnect(self) -> None:
        """断开连接"""
        self._auth_cookie = None
        self._set_disconnected()

    async def is_authenticated(self) -> bool:
        """检查是否已认证"""
        return self._auth_cookie is not None

    def _apply_auth(self, request: httpx.Request) -> httpx.Request:
        """为请求应用认证信息"""
        if self._auth_cookie:
            request.headers["Cookie"] = self._auth_cookie

        return request

    @classmethod
    def get_credentials_schema(cls) -> dict[str, Any]:
        """获取凭据配置 schema"""
        return {
            "type": "object",
            "properties": {
                "base_url": {
                    "type": "string",
                    "title": "站点地址",
                    "description": "API 基础地址",
                    "x-default-value": "https://co.yes.vg",
                },
                "auth_cookie": {
                    "type": "string",
                    "title": "Auth Cookie",
                    "description": "从浏览器复制的 Cookie（包含 yescode_auth 和 yescode_csrf）",
                    "x-sensitive": True,
                    "x-input-type": "password",
                },
            },
            "required": ["auth_cookie"],
            "x-field-groups": [
                {"fields": ["base_url"]},
                {"fields": ["auth_cookie"]},
            ],
            "x-auth-type": "cookie",
            "x-default-base-url": "https://co.yes.vg",
            "x-validation": [
                {
                    "type": "required",
                    "fields": ["auth_cookie"],
                    "message": "请填写 Auth Cookie",
                },
            ],
            "x-quota-divisor": None,
            "x-currency": "USD",
            "x-balance-extra-format": [
                {
                    "label": "天",
                    "type": "weekly_spent",
                    "source_limit": "daily_limit",
                    "source_spent": "daily_spent",
                    "source_resets_at": "daily_resets_at",
                },
                {
                    "label": "周",
                    "type": "weekly_spent",
                    "source_limit": "weekly_limit",
                    "source_spent": "weekly_spent",
                    "source_resets_at": "weekly_resets_at",
                },
            ],
        }


class YesCodeArchitecture(ProviderArchitecture):
    """
    YesCode 架构预设

    针对 YesCode 中转站优化的预设配置。

    特点：
    - 使用 Cookie 认证（yescode_auth JWT + yescode_csrf）
    - 验证端点: /api/v1/user/balance
    - 余额单位直接是美元
    - 支持每周限额查询
    """

    architecture_id = "yescode"
    display_name = "YesCode"
    description = "YesCode 中转站预设配置，使用 Cookie 认证"

    supported_connectors: list[type[ProviderConnector]] = [
        YesCodeConnector,
    ]

    supported_actions: list[type[ProviderAction]] = [
        YesCodeBalanceAction,
    ]

    default_action_configs: dict[ProviderActionType, dict[str, Any]] = {
        ProviderActionType.QUERY_BALANCE: {
            "endpoint": "/api/v1/user/balance",
            "method": "GET",
            "response_mapping": {
                "total_available": "total_balance",
            },
        },
    }

    def get_credentials_schema(self) -> dict[str, Any]:
        """YesCode 使用 auth_cookie 认证"""
        return YesCodeConnector.get_credentials_schema()

    def get_verify_endpoint(self) -> str:
        """验证端点 - 使用 profile 接口获取完整信息"""
        return "/api/v1/auth/profile"

    async def prepare_verify_config(
        self,
        base_url: str,
        config: dict[str, Any],
        credentials: dict[str, Any],
    ) -> dict[str, Any]:
        """
        预获取合并数据（balance + profile）

        验证时并发调用两个接口获取完整数据。
        """
        extra_config: dict[str, Any] = {}

        cookie_input = credentials.get("auth_cookie")
        if not cookie_input:
            return extra_config

        cookie_header = _build_cookie_header(cookie_input)

        # 获取代理配置（支持 proxy_node_id、tunnel 和旧的 proxy URL）
        from src.services.proxy_node.resolver import resolve_ops_proxy_config_async

        proxy, tunnel_node_id = await resolve_ops_proxy_config_async(config)

        try:
            # 构建 client 参数
            client_kwargs: dict[str, Any] = {
                "headers": {"Cookie": cookie_header},
                "timeout": 10.0,
                "verify": get_ssl_context(),
            }
            if tunnel_node_id:
                from src.services.proxy_node.tunnel_transport import create_tunnel_transport

                client_kwargs["transport"] = create_tunnel_transport(tunnel_node_id, timeout=10.0)
            elif proxy:
                client_kwargs["proxy"] = proxy

            # 创建临时 client 获取合并数据
            async with httpx.AsyncClient(**client_kwargs) as client:
                combined_data = await fetch_yescode_combined_data(client, base_url)
                extra_config["_combined_data"] = combined_data
        except Exception:
            # 如果调用失败，不影响验证流程（会回退到单独调用 profile）
            pass

        return extra_config

    def build_verify_headers(
        self,
        config: dict[str, Any],
        credentials: dict[str, Any],
    ) -> dict[str, str]:
        """
        构建 YesCode 的验证请求 Headers

        使用 Cookie 认证，不使用 Authorization。
        """
        headers: dict[str, str] = {}

        # 添加 Cookie
        cookie_input = credentials.get("auth_cookie")
        if cookie_input:
            headers["Cookie"] = _build_cookie_header(cookie_input)

        return headers

    def parse_verify_response(
        self,
        status_code: int,
        data: dict[str, Any],
    ) -> VerifyResult:
        """解析 YesCode 验证响应（使用预获取的合并数据）"""
        if status_code == 401:
            return VerifyResult(success=False, message="Cookie 已失效，请重新配置")
        if status_code == 403:
            return VerifyResult(success=False, message="Cookie 已失效或无权限")
        if status_code != 200:
            return VerifyResult(success=False, message=f"验证失败：HTTP {status_code}")

        # 优先使用预获取的合并数据（包含 balance + profile）
        combined_data = data.get("_combined_data")
        if combined_data:
            # 检查是否有有效数据
            if "_profile_data" not in combined_data and "_balance_data" not in combined_data:
                return VerifyResult(success=False, message="Cookie 已失效，请重新配置")

            # 使用公共函数解析余额
            extra = parse_yescode_balance_extra(combined_data)

            total_available = extra.pop("_total_available", 0)
            extra.pop("_subscription_available", None)

            return VerifyResult(
                success=True,
                username=combined_data.get("username"),
                display_name=combined_data.get("username"),
                email=combined_data.get("email"),
                quota=total_available,
                extra=extra if extra else None,
            )

        # 回退：仅使用 profile 数据（旧逻辑，当 prepare_verify_config 失败时）
        if "username" not in data:
            return VerifyResult(success=False, message="响应格式无效")

        # 构造兼容格式供公共函数使用
        compat_data = {
            "pay_as_you_go_balance": data.get("pay_as_you_go_balance", 0),
            "subscription_balance": data.get("subscription_balance", 0),
            "weekly_spent_balance": data.get("current_week_spend", 0),
            "subscription_plan": data.get("subscription_plan"),
            "last_week_reset": data.get("last_week_reset"),
            "last_daily_balance_add": data.get("last_daily_balance_add"),
        }

        extra = parse_yescode_balance_extra(compat_data)

        total_available = extra.pop("_total_available", 0)
        extra.pop("_subscription_available", None)

        return VerifyResult(
            success=True,
            username=data.get("username"),
            display_name=data.get("username"),
            email=data.get("email"),
            quota=total_available,
            extra=extra if extra else None,
        )
