"""
统一的请求头处理模块

职责：
1. 请求头规范化（大小写统一）
2. 客户端 API Key 提取
3. 能力需求检测
4. 上游请求头构建
5. 响应头过滤
6. 日志脱敏
"""

from __future__ import annotations

import json
from collections.abc import Set as AbstractSet
from typing import TYPE_CHECKING, Any

from src.core.api_format.enums import ApiFamily
from src.core.api_format.metadata import (
    get_auth_config_for_endpoint,
    get_extra_headers_for_endpoint,
    get_protected_keys_for_endpoint,
    resolve_endpoint_definition,
)
from src.core.api_format.signature import EndpointSignature, parse_signature_key
from src.core.logger import logger

if TYPE_CHECKING:
    from src.services.provider.fingerprint import FingerprintProfile

# =============================================================================
# 头部常量定义
# =============================================================================

# 通用浏览器指纹 Headers，用于绕过 Cloudflare 等反爬防护
# 基于 Electron 桌面客户端的真实请求头构建，作为所有 adapter 请求的底层默认值
BROWSER_FINGERPRINT_HEADERS: dict[str, str] = {
    "User-Agent": (
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) "
        "AppleWebKit/537.36 (KHTML, like Gecko) "
        "Chrome/140.0.7339.249 Electron/38.7.0 Safari/537.36"
    ),
    "Accept": "application/json",
    "Accept-Encoding": "gzip, deflate, br",
    "Accept-Language": "zh-CN",
    "sec-ch-ua": '"Not=A?Brand";v="24", "Chromium";v="140"',
    "sec-ch-ua-mobile": "?0",
    "sec-ch-ua-platform": '"macOS"',
    "Sec-Fetch-Site": "cross-site",
    "Sec-Fetch-Mode": "cors",
    "Sec-Fetch-Dest": "empty",
}

# Anthropic/Claude 专属 Headers（仅 Claude API family 使用）
# 包含 Stainless SDK 指纹和 direct-browser-access 标记
_ANTHROPIC_EXTRA_HEADERS: dict[str, str] = {
    "anthropic-dangerous-direct-browser-access": "true",
    "x-stainless-os": "Unknown",
    "x-stainless-runtime": "browser:chrome",
    "x-stainless-arch": "unknown",
    "x-stainless-lang": "js",
    "x-stainless-package-version": "0.41.0",
    "x-stainless-runtime-version": "140.0.7339",
    "x-stainless-retry-count": "0",
}

# 转发给上游时需要剔除的头部（系统管理 + 认证替换 + 客户端/代理元数据）
UPSTREAM_DROP_HEADERS: frozenset[str] = frozenset(
    {
        # 认证头 - 会被替换为 Provider 的认证
        "authorization",
        "x-api-key",
        "x-goog-api-key",
        # 系统管理头 - 由 HTTP 客户端重新生成
        "host",
        "content-length",
        "transfer-encoding",
        "connection",
        # 编码头 - 丢弃客户端值，由 BROWSER_FINGERPRINT_HEADERS 统一设置
        "accept-encoding",
        "content-encoding",
        # 反向代理 / 网关注入的头部 - 属于本站基础设施，不应泄露给上游
        "x-real-ip",
        "x-real-proto",
        "x-forwarded-for",
        "x-forwarded-proto",
        "x-forwarded-host",
        "x-forwarded-port",
    }
)

# 最小必脱敏集合（编译时常量，用于快速路径）
# 完整脱敏应使用 SystemConfigService.get_sensitive_headers()
CORE_REDACT_HEADERS: frozenset[str] = frozenset(
    {
        "authorization",
        "x-api-key",
        "x-goog-api-key",
    }
)

# Hop-by-hop 头部 (RFC 7230)
HOP_BY_HOP_HEADERS: frozenset[str] = frozenset(
    {
        "connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
    }
)

# 响应时需要过滤的头部（body-dependent + hop-by-hop）
RESPONSE_DROP_HEADERS: frozenset[str] = (
    frozenset(
        {
            "content-length",
            "content-encoding",
            "transfer-encoding",
            "content-type",
        }
    )
    | HOP_BY_HOP_HEADERS
)


_DEFAULT_CHROME_MAJOR = "140"


def _extract_chrome_major(version: str) -> str:
    value = str(version or "").strip()
    if not value:
        return _DEFAULT_CHROME_MAJOR
    return value.split(".", 1)[0] or _DEFAULT_CHROME_MAJOR


def _resolve_sec_ch_platform(fp: FingerprintProfile) -> str:
    os_name = (fp.stainless_os or "").strip().lower()
    platform_info = (fp.platform_info or "").strip().lower()

    if os_name.startswith("win") or "windows" in platform_info:
        return '"Windows"'
    if os_name in {"mac", "macos", "darwin"} or "darwin" in platform_info:
        return '"macOS"'
    return '"Linux"'


def build_browser_fingerprint_headers(fp: FingerprintProfile | None = None) -> dict[str, str]:
    """Build browser fingerprint headers, optionally overridden by per-key fingerprint."""
    headers = {**BROWSER_FINGERPRINT_HEADERS}
    if fp is None:
        return headers

    chrome_major = _extract_chrome_major(fp.chrome_version)
    headers["User-Agent"] = fp.user_agent or headers["User-Agent"]
    headers["sec-ch-ua"] = f'"Not=A?Brand";v="24", "Chromium";v="{chrome_major}"'
    headers["sec-ch-ua-platform"] = _resolve_sec_ch_platform(fp)
    return headers


def build_anthropic_extra_headers(fp: FingerprintProfile | None = None) -> dict[str, str]:
    """Build Anthropic extra headers, optionally overridden by per-key fingerprint."""
    headers = {**_ANTHROPIC_EXTRA_HEADERS}
    if fp is None:
        return headers

    headers["x-stainless-os"] = fp.stainless_os or headers["x-stainless-os"]
    headers["x-stainless-arch"] = fp.stainless_arch or headers["x-stainless-arch"]
    headers["x-stainless-package-version"] = (
        fp.stainless_package_version or headers["x-stainless-package-version"]
    )
    # browser:chrome runtime-version = Chrome 版本；
    # CLI 路径(ClaudeCodeEnvelope)使用 Node.js，其 extra_headers 会以 fp.stainless_runtime_version 覆盖。
    headers["x-stainless-runtime-version"] = (
        fp.chrome_version or headers["x-stainless-runtime-version"]
    )
    return headers


# =============================================================================
# 请求头规范化
# =============================================================================


def normalize_headers(headers: dict[str, str]) -> dict[str, str]:
    """
    将请求头 key 统一为小写

    用于处理 context.original_headers 的大小写敏感问题。
    """

    return {k.lower(): v for k, v in headers.items()}


def get_header_value(headers: dict[str, str], key: str, default: str = "") -> str:
    """
    大小写不敏感地获取请求头值

    Args:
        headers: 原始请求头（可能大小写不一致）
        key: 要获取的 key（任意大小写）
        default: 未找到时的默认值

    Returns:
        头部值，未找到返回 default
    """

    key_lower = key.lower()
    for k, v in headers.items():
        if k.lower() == key_lower:
            return v
    return default


# =============================================================================
# 客户端 API Key 提取
# =============================================================================


def extract_client_api_key_for_endpoint(
    headers: dict[str, str],
    endpoint: str | EndpointSignature | tuple,
) -> str | None:
    """
    新模式：从客户端请求头提取 API Key。

    Args:
        headers: 原始请求头（自动处理大小写）
        endpoint: endpoint signature（`family:kind` / EndpointSignature / (ApiFamily, EndpointKind)）
    """
    auth_header, auth_type = get_auth_config_for_endpoint(endpoint)
    value = get_header_value(headers, auth_header)
    if not value:
        return None

    if auth_type == "bearer":
        if value.lower().startswith("bearer "):
            return value[7:]
        return None

    return value


def extract_client_api_key_for_endpoint_with_query(
    headers: dict[str, str],
    query_params: dict[str, str] | None,
    endpoint: str | EndpointSignature | tuple,
) -> str | None:
    """
    新模式：从客户端请求头或 URL 参数提取 API Key。

    Gemini family 优先级：
    1. URL 参数 ?key=
    2. x-goog-api-key 请求头
    """
    try:
        sig = (
            endpoint
            if isinstance(endpoint, EndpointSignature)
            else (
                parse_signature_key(endpoint)  # type: ignore[arg-type]
                if isinstance(endpoint, str)
                else EndpointSignature(api_family=endpoint[0], endpoint_kind=endpoint[1])
            )  # type: ignore[index]
        )
    except Exception:
        sig = None

    if sig and sig.api_family.value == "gemini":
        query_key = query_params.get("key") if query_params else None
        if query_key:
            return query_key

    return extract_client_api_key_for_endpoint(headers, endpoint)


# =============================================================================
# 能力需求检测
# =============================================================================


def detect_capabilities_for_endpoint(
    headers: dict[str, str],
    endpoint: str | EndpointSignature | tuple,
    request_body: dict[str, Any] | None = None,  # noqa: ARG001 - 预留
) -> dict[str, bool]:
    """
    新模式：从请求头检测能力需求。

    当前支持：
    - Claude family: anthropic-beta 头中的 context-1m
    """
    requirements: dict[str, bool] = {}

    try:
        sig = (
            endpoint
            if isinstance(endpoint, EndpointSignature)
            else (
                parse_signature_key(endpoint)  # type: ignore[arg-type]
                if isinstance(endpoint, str)
                else EndpointSignature(api_family=endpoint[0], endpoint_kind=endpoint[1])
            )  # type: ignore[index]
        )
    except Exception:
        sig = None

    if sig and sig.api_family.value == "claude":
        beta_header = get_header_value(headers, "anthropic-beta")
        if "context-1m" in beta_header.lower():
            requirements["context_1m"] = True

    return requirements


# =============================================================================
# 上游请求头构建
# =============================================================================


class HeaderBuilder:
    """
    请求头构建器

    使用 lower-case key 索引确保唯一性和确定的优先级。
    优先级（后者覆盖前者）：原始头部 < endpoint 头部 < extra 头部 < 认证头
    """

    def __init__(self) -> None:
        # key: (original_case_key, value)
        self._headers: dict[str, tuple[str, str]] = {}

    def add(self, key: str, value: str) -> HeaderBuilder:
        """添加单个头部（会覆盖同名头部）"""
        self._headers[key.lower()] = (key, value)
        return self

    def add_many(self, headers: dict[str, str]) -> HeaderBuilder:
        """批量添加头部"""
        for k, v in headers.items():
            self.add(k, v)
        return self

    def add_protected(
        self, headers: dict[str, str], protected_keys: AbstractSet[str]
    ) -> HeaderBuilder:
        """
        添加头部但保护指定的 key 不被覆盖

        用于 endpoint 额外请求头不能覆盖认证头的场景。
        """
        protected_lower = {k.lower() for k in protected_keys}
        for k, v in headers.items():
            if k.lower() not in protected_lower:
                self.add(k, v)
        return self

    def remove(self, keys: frozenset[str]) -> HeaderBuilder:
        """移除指定的头部"""
        for k in keys:
            self._headers.pop(k.lower(), None)
        return self

    def rename(self, from_key: str, to_key: str) -> HeaderBuilder:
        """
        重命名头部（保留原值）

        如果 from_key 不存在，则不做任何操作。
        """
        from_lower = from_key.lower()
        if from_lower in self._headers:
            _, value = self._headers.pop(from_lower)
            self._headers[to_key.lower()] = (to_key, value)
        return self

    def apply_rules(
        self,
        rules: list[dict[str, Any]],
        protected_keys: AbstractSet[str] | None = None,
    ) -> HeaderBuilder:
        """
        应用请求头规则

        支持的规则类型：
        - set: 设置/覆盖头部 {"action": "set", "key": "X-Custom", "value": "fixed"}
        - drop: 删除头部 {"action": "drop", "key": "X-Unwanted"}
        - rename: 重命名头部 {"action": "rename", "from": "X-Old", "to": "X-New"}

        Args:
            rules: 规则列表
            protected_keys: 受保护的 key（不能被 set/drop/rename 修改）
        """
        protected_lower = {k.lower() for k in protected_keys} if protected_keys else set()

        for rule in rules:
            action = rule.get("action")

            if action == "set":
                key = rule.get("key", "")
                value = rule.get("value", "")
                if key and key.lower() not in protected_lower:
                    self.add(key, value)

            elif action == "drop":
                key = rule.get("key", "")
                if key and key.lower() not in protected_lower:
                    self._headers.pop(key.lower(), None)

            elif action == "rename":
                from_key = rule.get("from", "")
                to_key = rule.get("to", "")
                if from_key and to_key:
                    # 两个 key 都不能是受保护的
                    if (
                        from_key.lower() not in protected_lower
                        and to_key.lower() not in protected_lower
                    ):
                        self.rename(from_key, to_key)

        return self

    def build(self) -> dict[str, str]:
        """构建最终的头部字典"""
        result: dict[str, str] = {}
        for original_key, value in self._headers.values():
            result[original_key] = _normalize_header_value_for_httpx(original_key, value)
        return result


def _normalize_header_value_for_httpx(key: str, value: str) -> str:
    """将 header 值归一化为 httpx/h11 可发送的 ASCII 字符串。

    说明：
    - 当前 httpx/h11 栈会对 str 类型 header 值执行 ASCII 编码。
    - 若值包含非 ASCII 字符（如中文），会抛出 UnicodeEncodeError。
    - 因此这里统一做 ASCII 归一化，确保请求可稳定发出。
    """
    if value.isascii():
        return value

    key_lower = key.lower()

    # Codex CLI 元数据是 JSON 字符串，优先重编码为 ASCII JSON，语义最稳定。
    if key_lower == "x-codex-turn-metadata":
        try:
            normalized = json.dumps(json.loads(value), ensure_ascii=True, separators=(",", ":"))
            logger.debug(
                "Header '{}' contains non-ASCII chars, normalized as ASCII JSON",
                key,
            )
            return normalized
        except Exception:
            # 非法 JSON 时走通用兜底，避免阻断请求。
            pass

    # 兜底：仅将非 ASCII 字符替换为 \uXXXX，保留 ASCII 字符原样
    escaped = "".join(c if c.isascii() else f"\\u{ord(c):04x}" for c in value)
    logger.warning(
        "Header '{}' contains non-ASCII chars, escaped for httpx compatibility",
        key,
    )
    return escaped


def build_upstream_headers_for_endpoint(
    original_headers: dict[str, str],
    endpoint: str | EndpointSignature | tuple,
    provider_api_key: str,
    *,
    endpoint_headers: dict[str, str] | None = None,
    extra_headers: dict[str, str] | None = None,
    drop_headers: frozenset[str] | None = None,
    header_rules: list[dict[str, Any]] | None = None,
) -> dict[str, str]:
    """
    新模式：构建发送给上游 Provider 的请求头（基于 endpoint signature）。

    优先级（后者覆盖前者）：
    1. 原始头部（排除 drop_headers）
    2. endpoint 配置头部
    3. header_rules（用户自定义的请求头规则，支持 set/drop/rename）
    4. extra_headers
    5. 认证头（最高优先级，始终设置）
    """
    if drop_headers is None:
        drop_headers = UPSTREAM_DROP_HEADERS

    auth_header, auth_type = get_auth_config_for_endpoint(endpoint)
    auth_value = f"Bearer {provider_api_key}" if auth_type == "bearer" else provider_api_key

    protected_keys = {auth_header.lower(), "content-type"}

    builder = HeaderBuilder()

    for k, v in original_headers.items():
        if k.lower() not in drop_headers:
            builder.add(k, v)

    if endpoint_headers:
        builder.add_protected(endpoint_headers, protected_keys)

    # 应用用户自定义的请求头规则（认证头受保护）
    if header_rules:
        builder.apply_rules(header_rules, protected_keys)

    if extra_headers:
        builder.add_many(extra_headers)

    builder.add(auth_header, auth_value)

    result = builder.build()
    if not any(k.lower() == "content-type" for k in result):
        result["Content-Type"] = "application/json"

    return result


def merge_headers_with_protection(
    base_headers: dict[str, str],
    extra_headers: dict[str, str] | None,
    protected_keys: frozenset[str] | set[str],
) -> dict[str, str]:
    """
    合并头部但保护指定的 key 不被覆盖

    等价于原 build_safe_headers 的功能。

    Args:
        base_headers: 基础头部
        extra_headers: 要合并的额外头部
        protected_keys: 受保护的 key 集合

    Returns:
        合并后的头部
    """
    if not extra_headers:
        return dict(base_headers)

    builder = HeaderBuilder()
    builder.add_many(base_headers)
    builder.add_protected(extra_headers, protected_keys)
    return builder.build()


# =============================================================================
# 响应头过滤
# =============================================================================


def filter_response_headers(
    headers: dict[str, str] | None,
    drop_headers: frozenset[str] | None = None,
) -> dict[str, str]:
    """
    过滤上游响应头中不应透传给客户端的字段

    Args:
        headers: 上游响应头
        drop_headers: 要剔除的头部集合（None 使用默认值）

    Returns:
        过滤后的头部
    """
    if not headers:
        return {}

    if drop_headers is None:
        drop_headers = RESPONSE_DROP_HEADERS

    return {k: v for k, v in headers.items() if k.lower() not in drop_headers}


# =============================================================================
# 日志脱敏
# =============================================================================


def redact_headers_for_log(
    headers: dict[str, str],
    redact_keys: frozenset[str] | None = None,
) -> dict[str, str]:
    """
    将敏感头部值替换为 *** 用于日志记录

    Args:
        headers: 原始头部
        redact_keys: 要脱敏的 key 集合（None 使用 CORE_REDACT_HEADERS）

    Returns:
        脱敏后的头部

    Note:
        完整的脱敏应该使用 SystemConfigService.get_sensitive_headers()
        来获取用户配置的敏感头列表。
    """
    if redact_keys is None:
        redact_keys = CORE_REDACT_HEADERS

    return {k: "***" if k.lower() in redact_keys else v for k, v in headers.items()}


# =============================================================================
# Adapter 统一接口
# =============================================================================


def build_adapter_base_headers_for_endpoint(
    endpoint: str | EndpointSignature | tuple,
    api_key: str,
    *,
    include_extra: bool = True,
) -> dict[str, str]:
    """
    新模式：根据 endpoint signature 构建基础请求头。

    浏览器指纹 headers 作为底层默认值注入，Claude API family 额外注入 Anthropic 专属 header。
    认证头和 extra_headers 会覆盖它们。
    """
    auth_header, auth_type = get_auth_config_for_endpoint(endpoint)
    auth_value = f"Bearer {api_key}" if auth_type == "bearer" else api_key

    # 以浏览器指纹为底层默认值，绕过 Cloudflare 等反爬防护
    headers: dict[str, str] = build_browser_fingerprint_headers()

    # Claude API family 额外注入 Anthropic 专属 header
    definition = resolve_endpoint_definition(endpoint)
    if definition and definition.api_family == ApiFamily.CLAUDE:
        headers.update(build_anthropic_extra_headers())

    headers[auth_header] = auth_value
    headers["Content-Type"] = "application/json"

    if include_extra:
        extra = get_extra_headers_for_endpoint(endpoint)
        if extra:
            headers.update(extra)

    return headers


def build_adapter_headers_for_endpoint(
    endpoint: str | EndpointSignature | tuple,
    api_key: str,
    extra_headers: dict[str, str] | None = None,
) -> dict[str, str]:
    """
    新模式：构建完整的 Adapter 请求头（包含 extra_headers）。
    """
    base = build_adapter_base_headers_for_endpoint(endpoint, api_key)
    if not extra_headers:
        return base
    protected = get_protected_keys_for_endpoint(endpoint)
    return merge_headers_with_protection(base, extra_headers, protected)


def get_adapter_protected_keys_for_endpoint(
    endpoint: str | EndpointSignature | tuple,
) -> tuple[str, ...]:
    """新模式：获取 Adapter 的受保护头部 key。"""
    return tuple(get_protected_keys_for_endpoint(endpoint))


# =============================================================================
# Header Rules 工具函数
# =============================================================================


def extract_set_headers_from_rules(
    header_rules: list[dict[str, Any]] | None,
) -> dict[str, str] | None:
    """
    从 header_rules 中提取 set 操作生成的头部字典

    用于需要构造额外请求头的场景（如模型列表查询、模型测试等）。
    注意：drop 和 rename 操作在这里不适用，因为它们用于修改已存在的头部。

    Args:
        header_rules: 请求头规则列表 [{"action": "set", "key": "X-Custom", "value": "val"}, ...]

    Returns:
        set 操作生成的头部字典，如果没有则返回 None
    """
    if not header_rules:
        return None

    headers: dict[str, str] = {}
    for rule in header_rules:
        if rule.get("action") == "set":
            key = rule.get("key", "")
            value = rule.get("value", "")
            if key:
                headers[key] = value

    return headers if headers else None


def get_extra_headers_from_endpoint(endpoint: Any) -> dict[str, str] | None:
    """
    从 endpoint 提取额外请求头

    用于需要构造额外请求头的场景（如模型列表查询、模型测试等）。

    Args:
        endpoint: ProviderEndpoint 对象

    Returns:
        额外请求头字典，如果没有则返回 None
    """
    header_rules = getattr(endpoint, "header_rules", None)
    return extract_set_headers_from_rules(header_rules)


# =============================================================================
# 请求头辅助工具
# =============================================================================


def set_accept_if_absent(headers: dict[str, str], value: str = "text/event-stream") -> None:
    """Set the ``Accept`` header only if not already present (case-insensitive check).

    Used by stream handlers to request SSE format from upstream without overriding
    provider-specific Accept headers (e.g. Kiro's ``application/vnd.amazon.eventstream``).
    """
    if not any(k.lower() == "accept" for k in headers):
        headers["Accept"] = value
