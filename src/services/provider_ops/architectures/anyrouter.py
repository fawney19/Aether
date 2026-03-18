"""
Anyrouter 架构

针对 Anyrouter 中转站的预设配置，自动处理 acw_sc__v2 反爬 Cookie。
"""

import base64
import re
from typing import Any

import httpx

from src.core.logger import logger
from src.services.provider_ops.actions import (
    AnyrouterBalanceAction,
    ProviderAction,
)
from src.services.provider_ops.architectures.base import (
    ProviderArchitecture,
    ProviderConnector,
)
from src.services.provider_ops.types import ConnectorAuthType, ProviderActionType
from src.services.provider_ops.utils import extract_cookie_value
from src.utils.ssl_utils import get_ssl_context

# acw_sc__v2 算法常量
_XOR_KEY = "3000176000856006061501533003690027800375"
_UNSBOX_TABLE = [
    0xF,
    0x23,
    0x1D,
    0x18,
    0x21,
    0x10,
    0x1,
    0x26,
    0xA,
    0x9,
    0x13,
    0x1F,
    0x28,
    0x1B,
    0x16,
    0x17,
    0x19,
    0xD,
    0x6,
    0xB,
    0x27,
    0x12,
    0x14,
    0x8,
    0xE,
    0x15,
    0x20,
    0x1A,
    0x2,
    0x1E,
    0x7,
    0x4,
    0x11,
    0x5,
    0x3,
    0x1C,
    0x22,
    0x25,
    0xC,
    0x24,
]


def _compute_acw_sc_v2(arg1: str) -> str:
    """
    计算 acw_sc__v2 Cookie 值

    Args:
        arg1: 从 HTML 中提取的 40 位十六进制字符串

    Returns:
        计算后的 Cookie 值
    """
    # Step 1: unsbox - 根据置换表重排字符
    unsboxed = "".join(arg1[i - 1] for i in _UNSBOX_TABLE)

    # Step 2: hexXor - 与密钥逐字节异或
    result = ""
    for i in range(0, 40, 2):
        a = int(unsboxed[i : i + 2], 16)
        b = int(_XOR_KEY[i : i + 2], 16)
        xored = format(a ^ b, "02x")
        result += xored

    return result


def _parse_session_user_id(cookie_input: str) -> tuple[str | None, str | None]:
    """
    从 session cookie 中解析用户 ID 和用户名

    Anyrouter 的 session cookie 结构:
    base64(timestamp|gob_base64|signature)

    gob 数据中包含:
    - id: 内部数字 ID (gob 编码的整数)
    - username: 用户名
    - role, status, group 等

    Args:
        cookie_input: Cookie 字符串或 session 值

    Returns:
        (user_id, username) 元组，解析失败则返回 (None, None)
    """
    try:
        # 先提取 session 值
        session_cookie = extract_cookie_value(cookie_input, "session")
        # 1. URL-safe base64 解码外层
        padding = 4 - len(session_cookie) % 4
        if padding != 4:
            session_cookie += "=" * padding

        decoded = base64.urlsafe_b64decode(session_cookie)
        text = decoded.decode("utf-8", errors="replace")

        # 2. 分割: timestamp|gob_base64|signature
        parts = text.split("|")
        if len(parts) < 2:
            return None, None

        # 3. 解码 gob 数据 (第二层 base64)
        gob_b64 = parts[1]
        padding2 = 4 - len(gob_b64) % 4
        if padding2 != 4:
            gob_b64 += "=" * padding2

        gob_data = base64.urlsafe_b64decode(gob_b64)

        # 4. 从 gob 数据中解析 id 字段
        # 查找 "\x02id\x03int" 模式，后面跟着 gob 编码的整数
        id_pattern = b"\x02id\x03int"
        id_idx = gob_data.find(id_pattern)
        user_id = None
        if id_idx != -1:
            # 跳过 "\x02id\x03int" (7字节) 和类型标记 (2字节)
            value_start = id_idx + 7 + 2
            if value_start < len(gob_data):
                # 读取第一个字节，检查是否是 00（正数标记）
                first_byte = gob_data[value_start]
                if first_byte == 0:
                    # 下一个字节是长度标记
                    marker = gob_data[value_start + 1]
                    if marker >= 0x80:
                        # 负的表示长度: 256 - marker = 字节数
                        length = 256 - marker
                        if value_start + 2 + length <= len(gob_data):
                            # 读取 length 字节，大端序转整数
                            val = int.from_bytes(
                                gob_data[value_start + 2 : value_start + 2 + length],
                                "big",
                            )
                            # gob zigzag 解码：正整数用 2*n 编码
                            user_id = str(val >> 1)

        # 5. 从 gob 数据中提取用户名
        username = None

        # 查找 username 字段后的值
        # 格式: \x08username\x06string\x0c\x10\x00\x0elinuxdo_129083
        # 其中 \x0e (14) 是用户名的长度
        username_pattern = b"\x08username\x06string"
        username_idx = gob_data.find(username_pattern)
        if username_idx != -1:
            # 跳过模式本身 (17字节) 和 \x0c\x10\x00 (3字节)
            # 第 4 个字节是长度
            length_pos = username_idx + len(username_pattern) + 3
            if length_pos < len(gob_data):
                length_byte = gob_data[length_pos]
                value_start = length_pos + 1
                if length_byte < 128 and value_start + length_byte <= len(gob_data):
                    username = gob_data[value_start : value_start + length_byte].decode(
                        "utf-8", errors="ignore"
                    )

        return user_id, username
    except Exception as e:
        logger.debug(f"解析 Anyrouter session cookie 失败: {e}")
        return None, None


async def _get_acw_cookie(
    base_url: str,
    timeout: float = 10,
    proxy: str | httpx.Proxy | None = None,
    tunnel_node_id: str | None = None,
) -> str | None:
    """
    获取 acw_sc__v2 Cookie

    首先请求目标 URL，如果返回包含 arg1 的反爬页面，则计算 Cookie 值。

    Args:
        base_url: 目标站点 URL
        timeout: 请求超时时间
        proxy: 代理地址
        tunnel_node_id: tunnel 模式节点 ID（优先于 proxy）

    Returns:
        Cookie 字符串 (acw_sc__v2=xxx)，如果不需要或获取失败则返回 None
    """
    try:
        # 构建 client 参数
        client_kwargs: dict[str, Any] = {
            "timeout": timeout,
            "verify": get_ssl_context(),
        }
        if tunnel_node_id:
            from src.services.proxy_node.tunnel_transport import create_tunnel_transport

            client_kwargs["transport"] = create_tunnel_transport(tunnel_node_id, timeout=timeout)
        elif proxy:
            client_kwargs["proxy"] = proxy
            logger.debug(f"获取 acw_sc__v2 Cookie 使用代理: {proxy}")

        async with httpx.AsyncClient(**client_kwargs) as client:
            resp = await client.get(
                base_url,
                headers={
                    "User-Agent": (
                        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) "
                        "AppleWebKit/537.36 (KHTML, like Gecko) "
                        "Chrome/120.0.0.0 Safari/537.36"
                    ),
                },
                follow_redirects=False,
            )

            # 尝试从响应中提取 arg1
            match = re.search(r"var\s+arg1\s*=\s*'([0-9a-fA-F]{40})'", resp.text)
            if not match:
                # 没有反爬页面，不需要 Cookie
                return None

            cookie_value = _compute_acw_sc_v2(match.group(1))
            return f"acw_sc__v2={cookie_value}"

    except Exception as e:
        logger.debug(f"获取 acw_sc__v2 Cookie 失败: {e}")
        return None


class AnyrouterConnector(ProviderConnector):
    """
    Anyrouter 专用连接器

    特点：
    - 使用 Cookie 认证（session）
    - 自动补充 acw_sc__v2 反爬 Cookie
    - 自动解析 user_id 用于 New-Api-User header
    """

    auth_type = ConnectorAuthType.COOKIE
    display_name = "Anyrouter Cookie"

    def __init__(self, base_url: str, config: dict[str, Any] | None = None):
        super().__init__(base_url, config)
        self._session_cookie: str | None = None
        self._acw_cookie: str | None = None
        self._user_id: str | None = None

    async def connect(self, credentials: dict[str, Any]) -> bool:
        """建立连接"""
        session_cookie = credentials.get("session_cookie")
        if not session_cookie:
            self._set_error("Session Cookie 不能为空")
            return False

        # 提取纯 session 值（支持完整 Cookie 字符串或仅 session 值）
        self._session_cookie = extract_cookie_value(session_cookie, "session")

        # 解析 user_id
        self._user_id, _ = _parse_session_user_id(session_cookie)

        # 尝试获取反爬 Cookie（使用配置中的代理）
        self._acw_cookie = await _get_acw_cookie(
            self.base_url, proxy=self._proxy, tunnel_node_id=self._tunnel_node_id
        )

        self._set_connected()
        return True

    async def disconnect(self) -> None:
        """断开连接"""
        self._session_cookie = None
        self._acw_cookie = None
        self._user_id = None
        self._set_disconnected()

    async def is_authenticated(self) -> bool:
        """检查是否已认证"""
        return self._session_cookie is not None

    def _apply_auth(self, request: httpx.Request) -> httpx.Request:
        """为请求应用认证信息"""
        cookies = []

        # 添加反爬 Cookie
        if self._acw_cookie:
            cookies.append(self._acw_cookie)

        # 添加 session Cookie
        if self._session_cookie:
            cookies.append(f"session={self._session_cookie}")

        if cookies:
            request.headers["Cookie"] = "; ".join(cookies)

        # 添加 New-Api-User header
        if self._user_id:
            request.headers["New-Api-User"] = self._user_id

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
                    "x-default-value": "https://anyrouter.top",
                },
                "session_cookie": {
                    "type": "string",
                    "title": "Session Cookie",
                    "description": "从浏览器复制的 session Cookie 值",
                    "x-sensitive": True,
                    "x-input-type": "password",
                },
            },
            "required": ["session_cookie"],
            "x-field-groups": [
                {"fields": ["base_url"]},
                {"fields": ["session_cookie"]},
            ],
            "x-auth-type": "cookie",
            "x-default-base-url": "https://anyrouter.top",
            "x-validation": [
                {
                    "type": "required",
                    "fields": ["session_cookie"],
                    "message": "请填写 Session Cookie",
                },
            ],
            "x-quota-divisor": 500000,
            "x-currency": "USD",
        }


class AnyrouterArchitecture(ProviderArchitecture):
    """
    Anyrouter 架构预设

    针对 Anyrouter 中转站优化的预设配置。

    特点：
    - 使用 Cookie 认证（session）
    - 自动处理 acw_sc__v2 反爬 Cookie
    - 验证端点: /api/user/self
    - quota 单位是 1/500000 美元
    """

    architecture_id = "anyrouter"
    display_name = "Anyrouter"
    description = "Anyrouter 中转站预设配置，使用 Cookie 认证"

    supported_connectors: list[type[ProviderConnector]] = [
        AnyrouterConnector,
    ]

    supported_actions: list[type[ProviderAction]] = [AnyrouterBalanceAction]

    default_action_configs: dict[ProviderActionType, dict[str, Any]] = {
        ProviderActionType.QUERY_BALANCE: {
            "endpoint": "/api/user/self",
            "method": "GET",
            "quota_divisor": 500000,  # 与 New API 相同
            "checkin_endpoint": "/api/user/sign_in",  # 自动签到端点
        },
    }

    def get_credentials_schema(self) -> dict[str, Any]:
        """Anyrouter 使用 session_cookie 认证"""
        return AnyrouterConnector.get_credentials_schema()

    def get_verify_endpoint(self) -> str:
        """验证端点"""
        return "/api/user/self"

    async def prepare_verify_config(
        self,
        base_url: str,
        config: dict[str, Any],
        credentials: dict[str, Any],
    ) -> dict[str, Any]:
        """
        验证前获取 acw_sc__v2 Cookie

        Args:
            base_url: API 基础地址
            config: 连接器配置
            credentials: 凭据信息

        Returns:
            包含 acw_cookie 的配置
        """
        # 从 config 获取代理配置（支持 proxy_node_id、tunnel 和旧的 proxy URL）
        from src.services.proxy_node.resolver import resolve_ops_proxy_config_async

        proxy, tunnel_node_id = await resolve_ops_proxy_config_async(config)
        acw_cookie = await _get_acw_cookie(base_url, proxy=proxy, tunnel_node_id=tunnel_node_id)
        if acw_cookie:
            return {"acw_cookie": acw_cookie}
        return {}

    def build_verify_headers(
        self,
        config: dict[str, Any],
        credentials: dict[str, Any],
    ) -> dict[str, str]:
        """
        构建 Anyrouter 的验证请求 Headers

        使用 Cookie 认证，不使用 Authorization。
        同时添加 New-Api-User header。
        """
        headers: dict[str, str] = {}

        cookies = []

        # 添加反爬 Cookie
        acw_cookie = config.get("acw_cookie")
        if acw_cookie:
            cookies.append(acw_cookie)

        # 添加 session Cookie
        cookie_input = credentials.get("session_cookie")
        if cookie_input:
            # 提取 session 值（支持完整 Cookie 字符串或仅 session 值）
            session_value = extract_cookie_value(cookie_input, "session")
            cookies.append(f"session={session_value}")

            # 从 session 解析 user_id 并添加 New-Api-User header
            user_id, _ = _parse_session_user_id(cookie_input)
            if user_id:
                headers["New-Api-User"] = user_id

        if cookies:
            headers["Cookie"] = "; ".join(cookies)

        return headers

    def _auth_fail_message(self, status_code: int) -> str:
        """Cookie 认证的错误消息"""
        if status_code == 401:
            return "Cookie 已失效，请重新配置"
        return "Cookie 已失效或无权限"
