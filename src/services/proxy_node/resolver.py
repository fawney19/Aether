"""
代理解析服务

集中管理代理 URL 构建、节点信息缓存、系统默认代理回退、代理信息追踪等逻辑。
供 HTTPClientPool、Handler、Provider Ops 等模块调用。
"""

from __future__ import annotations

import base64
import gzip as _gzip
import hashlib
import hmac as _hmac
import time
from typing import Any
from urllib.parse import quote, urlparse

import httpx

from src.config import config
from src.core.exceptions import ProxyNodeUnavailableError
from src.core.logger import logger
from src.utils import json_helper as _json

# ---------------------------------------------------------------------------
# ProxyNode 信息缓存（降低高频 DB 查询开销）
# ---------------------------------------------------------------------------
_proxy_node_cache: dict[str, tuple[dict[str, Any] | None, float]] = {}
_PROXY_NODE_CACHE_TTL_SECONDS = 60.0
_PROXY_NODE_CACHE_MAX_SIZE = 256


def _get_proxy_node_info(node_id: str) -> dict[str, Any] | None:
    """
    读取 ProxyNode 信息（带内存 TTL 缓存）

    NOTE: 使用同步 DB session（create_session），在 async 上下文中会短暂阻塞
    事件循环。60s TTL 缓存覆盖绝大多数请求，阻塞仅发生在缓存未命中时。
    若后续 delegate 模式导致调用频率显著上升，应考虑改为 run_in_executor 包装。

    Returns:
        aether-proxy 节点: {"ip": str, "port": int, "name": str, ...}
        手动节点: {"is_manual": True, "name": str, "proxy_url": str, ...}
        不存在/非在线: None
    """
    now = time.time()
    cached = _proxy_node_cache.get(node_id)
    if cached:
        value, expires_at = cached
        if now < expires_at:
            return value

    # 防止无效 node_id 导致缓存无限膨胀：淘汰最旧的条目而非全部清除
    if len(_proxy_node_cache) >= _PROXY_NODE_CACHE_MAX_SIZE:
        # 按过期时间排序，删除最旧的 25%
        evict_count = _PROXY_NODE_CACHE_MAX_SIZE // 4
        sorted_keys = sorted(_proxy_node_cache, key=lambda k: _proxy_node_cache[k][1])
        for k in sorted_keys[:evict_count]:
            del _proxy_node_cache[k]

    from src.database import create_session
    from src.models.database import ProxyNode, ProxyNodeStatus

    db = create_session()
    try:
        node = db.query(ProxyNode).filter(ProxyNode.id == node_id).first()
        if not node or node.status != ProxyNodeStatus.ONLINE:
            _proxy_node_cache[node_id] = (None, now + _PROXY_NODE_CACHE_TTL_SECONDS)
            return None

        if node.is_manual:
            value: dict[str, Any] = {
                "is_manual": True,
                "name": node.name,
                "proxy_url": node.proxy_url,
                "username": node.proxy_username,
                "password": node.proxy_password,
            }
        else:
            value = {
                "name": node.name,
                "ip": node.ip,
                "port": node.port,
                "tls_enabled": bool(node.tls_enabled),
                "tls_cert_fingerprint": node.tls_cert_fingerprint,
            }

        _proxy_node_cache[node_id] = (value, now + _PROXY_NODE_CACHE_TTL_SECONDS)
        return value
    finally:
        db.close()


# ---------------------------------------------------------------------------
# HMAC 签名
# ---------------------------------------------------------------------------


def build_hmac_proxy_url(ip: str, port: int, *, tls_enabled: bool = False) -> str:
    """
    构建带 HMAC BasicAuth 的 httpx proxy URL

    格式: http(s)://hmac:{timestamp}.{signature}@{ip}:{port}
    signature = HMAC-SHA256(PROXY_HMAC_KEY, "{timestamp}") 的 hex

    签名不再包含 node_id，避免 proxy 重新注册后 Aether 端缓存的旧 node_id
    与 proxy 端新 node_id 不一致导致的认证失败窗口。

    当 tls_enabled=True 时使用 https:// scheme。
    """
    if not config.proxy_hmac_key:
        logger.error("PROXY_HMAC_KEY 未配置，无法使用 ProxyNode 代理")
        raise ProxyNodeUnavailableError("PROXY_HMAC_KEY 未配置，无法使用 ProxyNode 代理")

    timestamp = str(int(time.time()))
    payload = timestamp.encode("utf-8")
    signature = _hmac.new(
        config.proxy_hmac_key.encode("utf-8"),
        payload,
        hashlib.sha256,
    ).hexdigest()

    host = f"[{ip}]" if ":" in ip else ip
    scheme = "https" if tls_enabled else "http"
    return f"{scheme}://hmac:{timestamp}.{signature}@{host}:{int(port)}"


# ---------------------------------------------------------------------------
# 系统默认代理
# ---------------------------------------------------------------------------
_system_proxy_cache: tuple[dict[str, Any] | None, float] | None = None
_SYSTEM_PROXY_CACHE_TTL = 60.0


def invalidate_system_proxy_cache() -> None:
    """手动失效系统代理缓存（在删除节点等操作后调用）"""
    global _system_proxy_cache
    _system_proxy_cache = None


def get_system_proxy_config() -> dict[str, Any] | None:
    """
    获取系统默认代理配置（带 TTL 缓存）

    从 system_configs 表中读取 system_proxy_node_id。
    返回 {"node_id": "...", "enabled": True} 或 None。
    """
    global _system_proxy_cache
    now = time.time()
    if _system_proxy_cache:
        value, expires_at = _system_proxy_cache
        if now < expires_at:
            return value

    from src.database import create_session
    from src.services.system.config import SystemConfigService

    db = create_session()
    try:
        node_id = SystemConfigService.get_config(db, "system_proxy_node_id")
        if node_id and isinstance(node_id, str) and node_id.strip():
            result: dict[str, Any] | None = {"node_id": node_id.strip(), "enabled": True}
        else:
            result = None
        _system_proxy_cache = (result, now + _SYSTEM_PROXY_CACHE_TTL)
        return result
    except Exception as exc:
        logger.warning("获取系统默认代理配置失败: {}", exc)
        _system_proxy_cache = (None, now + _SYSTEM_PROXY_CACHE_TTL)
        return None
    finally:
        db.close()


# ---------------------------------------------------------------------------
# 代理 URL 认证注入
# ---------------------------------------------------------------------------


def inject_auth_into_proxy_url(proxy_url: str, username: str, password: str | None = None) -> str:
    """将用户名密码注入代理 URL（URL 编码处理特殊字符）"""
    parsed = urlparse(proxy_url)
    encoded_username = quote(username, safe="")
    encoded_password = quote(password, safe="") if password else ""
    host_part = parsed.hostname or "localhost"
    if parsed.port:
        host_part = f"{host_part}:{parsed.port}"
    if encoded_password:
        auth_url = f"{parsed.scheme}://{encoded_username}:{encoded_password}@{host_part}"
    else:
        auth_url = f"{parsed.scheme}://{encoded_username}@{host_part}"
    if parsed.path:
        auth_url += parsed.path
    return auth_url


# ---------------------------------------------------------------------------
# TLS 代理参数
# ---------------------------------------------------------------------------


def make_proxy_param(proxy_url: str | None) -> str | httpx.Proxy | None:
    """
    根据代理 URL 返回 httpx 可接受的 proxy 参数。

    对于 https:// scheme 的代理 URL（TLS aether-proxy 节点），返回 httpx.Proxy
    并附带 proxy_ssl_context（CERT_NONE，因为使用自签名证书）。
    其他情况返回普通 URL 字符串。
    """
    if not proxy_url:
        return None

    # https:// 代理需要 ssl_context（自签名证书场景）
    if proxy_url.startswith("https://"):
        from src.utils.ssl_utils import get_proxy_ssl_context

        return httpx.Proxy(url=proxy_url, ssl_context=get_proxy_ssl_context())

    return proxy_url


# ---------------------------------------------------------------------------
# Ops connector 代理解析
# ---------------------------------------------------------------------------


def resolve_ops_proxy(
    connector_config: dict[str, Any] | None,
) -> str | httpx.Proxy | None:
    """
    从 ops connector.config 中解析代理参数（含系统默认回退）

    优先级：
    1. connector_config.proxy_node_id（新格式）
    2. connector_config.proxy（旧格式 URL 字符串）
    3. 系统默认代理节点

    Args:
        connector_config: connector 的 config 字典

    Returns:
        httpx 可接受的代理参数（str 或 httpx.Proxy），或 None
    """
    if connector_config:
        # 新格式：proxy_node_id -> 通过 build_proxy_url 解析
        node_id = connector_config.get("proxy_node_id")
        if isinstance(node_id, str) and node_id.strip():
            try:
                url = build_proxy_url({"node_id": node_id.strip(), "enabled": True})
                return make_proxy_param(url)
            except Exception as exc:
                logger.warning("解析 proxy_node_id={} 失败，回退到直连: {}", node_id, exc)
                return None

        # 旧格式：直接返回 proxy URL 字符串
        proxy = connector_config.get("proxy")
        if isinstance(proxy, str) and proxy.strip():
            return proxy

    # 回退：系统默认代理
    system_proxy = get_system_proxy_config()
    if system_proxy:
        try:
            url = build_proxy_url(system_proxy)
            return make_proxy_param(url)
        except Exception as exc:
            logger.warning("构建系统默认代理 URL 失败: {}", exc)
            return None

    return None


# ---------------------------------------------------------------------------
# Key 级别代理优先解析
# ---------------------------------------------------------------------------


def resolve_effective_proxy(
    provider_proxy: dict[str, Any] | None,
    key_proxy: dict[str, Any] | None = None,
) -> dict[str, Any] | None:
    """
    解析有效的代理配置，Key 级别代理优先于 Provider 级别代理。

    Args:
        provider_proxy: Provider 级别代理配置
        key_proxy: Key 级别代理配置（可选），非 None 且 enabled 时覆盖 Provider 级别

    Returns:
        有效的代理配置字典，或 None（无代理）
    """
    if key_proxy and key_proxy.get("enabled", True):
        return key_proxy
    return provider_proxy


def resolve_proxy_param(
    proxy_config: dict[str, Any] | None = None,
) -> str | httpx.Proxy | None:
    """
    将代理配置解析为 httpx 可接受的代理参数（含系统默认回退）

    优先级：proxy_config -> 系统默认代理 -> None（直连）

    Args:
        proxy_config: 代理配置字典（通常来自 resolve_effective_proxy 的返回值）

    Returns:
        httpx 可接受的 proxy 参数，或 None
    """
    url = build_proxy_url(proxy_config) if proxy_config else None
    if not url:
        sys_proxy = get_system_proxy_config()
        if sys_proxy:
            try:
                url = build_proxy_url(sys_proxy)
            except Exception as exc:
                logger.warning("resolve_proxy_param: 构建系统默认代理 URL 失败: {}", exc)
                url = None
    return make_proxy_param(url)


def build_proxy_client_kwargs(
    proxy_config: dict[str, Any] | None = None,
    *,
    timeout: float = 30.0,
    verify: Any | None = None,
    **extra: Any,
) -> dict[str, Any]:
    """
    构建包含代理配置的 httpx.AsyncClient 初始化参数。

    将 resolve_proxy_param + dict 构建 + 条件 proxy 赋值合并为一步，
    减少调用方的样板代码。

    Args:
        proxy_config: 代理配置字典（通常来自 resolve_effective_proxy）
        timeout: 请求超时（秒）
        verify: SSL 验证参数，None 时自动使用 get_ssl_context()
        **extra: 其他 httpx.AsyncClient 参数（如 follow_redirects）

    Returns:
        可直接解包传给 httpx.AsyncClient 的参数字典
    """
    if verify is None:
        from src.utils.ssl_utils import get_ssl_context

        verify = get_ssl_context()

    kwargs: dict[str, Any] = {"timeout": timeout, "verify": verify, **extra}
    proxy_param = resolve_proxy_param(proxy_config)
    if proxy_param:
        kwargs["proxy"] = proxy_param
    return kwargs


# ---------------------------------------------------------------------------
# 代理 URL 构建
# ---------------------------------------------------------------------------


def build_proxy_url(proxy_config: dict[str, Any]) -> str | None:
    """
    根据代理配置构建完整的代理 URL

    Args:
        proxy_config: 代理配置字典，支持两种模式：
            - 手动 URL 模式: {url, username, password, enabled}
            - ProxyNode 模式: {node_id, enabled}

    Returns:
        完整的代理 URL，如 socks5://user:pass@host:port
        如果 enabled=False 或无配置，返回 None
    """
    if not proxy_config:
        return None

    # 检查 enabled 字段，默认为 True（兼容旧数据）
    if not proxy_config.get("enabled", True):
        return None

    # ProxyNode 模式（aether-proxy 或手动节点）
    node_id = proxy_config.get("node_id")
    if isinstance(node_id, str) and node_id.strip():
        node_id = node_id.strip()
        node_info = _get_proxy_node_info(node_id)
        if not node_info:
            logger.warning("代理节点不可用（离线或不存在）: node_id={}", node_id)
            raise ProxyNodeUnavailableError(f"代理节点 {node_id} 不可用", node_id=node_id)

        # 手动节点：直接使用存储的代理 URL（含认证信息）
        if node_info.get("is_manual"):
            manual_url = node_info.get("proxy_url")
            if not manual_url:
                raise ProxyNodeUnavailableError(
                    f"手动代理节点 {node_id} 缺少 proxy_url", node_id=node_id
                )
            username = node_info.get("username")
            password = node_info.get("password")
            if username:
                return inject_auth_into_proxy_url(manual_url, username, password)
            return manual_url

        # aether-proxy 节点：使用 HMAC 认证
        return build_hmac_proxy_url(
            node_info["ip"],
            node_info["port"],
            tls_enabled=node_info.get("tls_enabled", False),
        )

    proxy_url: str | None = proxy_config.get("url")
    if not proxy_url:
        return None

    username = proxy_config.get("username")
    password = proxy_config.get("password")

    # 只要有用户名就添加认证信息（密码可以为空）
    if username:
        return inject_auth_into_proxy_url(proxy_url, username, password)

    return proxy_url


# ---------------------------------------------------------------------------
# 代理信息追踪（日志/usage）
# ---------------------------------------------------------------------------


def resolve_proxy_info(proxy_config: dict[str, Any] | None) -> dict[str, Any] | None:
    """
    解析代理配置的摘要信息（用于日志和 usage 记录）

    不构建实际的代理 URL，仅返回可读的代理标识信息。

    Returns:
        {"node_id": "xxx", "node_name": "proxy-01", "source": "provider"} 或
        {"url": "socks5://host:port", "source": "provider"} 或
        {"node_id": "xxx", "node_name": "...", "source": "system"} 或
        None (直连)
    """
    source = "provider"
    effective_config = proxy_config

    # 无 provider 级代理时，尝试系统默认代理
    if not effective_config or not effective_config.get("enabled", True):
        effective_config = get_system_proxy_config()
        source = "system"

    if not effective_config or not effective_config.get("enabled", True):
        return None

    # ProxyNode 模式
    node_id = effective_config.get("node_id")
    if isinstance(node_id, str) and node_id.strip():
        node_id = node_id.strip()
        node_info = _get_proxy_node_info(node_id)
        node_name = node_info.get("name", "unknown") if node_info else "offline"
        return {"node_id": node_id, "node_name": node_name, "source": source}

    # 旧格式 URL 模式
    proxy_url = effective_config.get("url")
    if proxy_url:
        # 脱敏：只保留 scheme + host + port
        try:
            parsed = urlparse(proxy_url)
            host_part = parsed.hostname or "unknown"
            if parsed.port:
                host_part = f"{host_part}:{parsed.port}"
            safe_url = f"{parsed.scheme}://{host_part}"
        except Exception:
            safe_url = "unknown"
        return {"url": safe_url, "source": source}

    return None


def get_proxy_label(proxy_info: dict[str, Any] | None) -> str:
    """从 proxy_info 中提取简短的代理标签（用于日志）"""
    if not proxy_info:
        return "direct"
    return proxy_info.get("node_name") or proxy_info.get("url") or "unknown"


# ---------------------------------------------------------------------------
# 代理缓存键计算（供 HTTPClientPool 使用）
# ---------------------------------------------------------------------------


def compute_proxy_cache_key(proxy_config: dict[str, Any] | None) -> str:
    """
    计算代理配置的缓存键

    Args:
        proxy_config: 代理配置字典

    Returns:
        缓存键字符串，无代理时返回 "__no_proxy__"
    """
    if not proxy_config:
        return "__no_proxy__"

    # enabled=False 时视为无代理（兼容旧数据）
    if not proxy_config.get("enabled", True):
        return "__no_proxy__"

    # ProxyNode 模式：基于 node_id + 时间桶缓存，避免签名随时间变化导致 cache key 爆炸
    node_id = proxy_config.get("node_id")
    if isinstance(node_id, str) and node_id.strip():
        time_bucket = int(time.time() / 240)  # 240s bucket, within 300s HMAC tolerance
        return f"proxy_node:{node_id.strip()}:{time_bucket}"

    # 构建代理 URL 作为缓存键的基础
    proxy_url = build_proxy_url(proxy_config)
    if not proxy_url:
        return "__no_proxy__"

    # 使用 MD5 哈希来避免过长的键名
    return f"proxy:{hashlib.md5(proxy_url.encode()).hexdigest()[:16]}"


# ---------------------------------------------------------------------------
# 代发模式 (Delegate API)
# ---------------------------------------------------------------------------


def _build_hmac_auth_header() -> str:
    """
    构建代发请求的 Authorization 头

    格式: Basic base64(hmac:{timestamp}.{signature})
    签名算法与 build_hmac_proxy_url 相同（仅使用 timestamp，不含 node_id）。
    """
    if not config.proxy_hmac_key:
        raise ProxyNodeUnavailableError("PROXY_HMAC_KEY 未配置，无法使用代发模式")

    timestamp = str(int(time.time()))
    payload = timestamp.encode("utf-8")
    signature = _hmac.new(
        config.proxy_hmac_key.encode("utf-8"),
        payload,
        hashlib.sha256,
    ).hexdigest()

    cred = f"hmac:{timestamp}.{signature}"
    encoded = base64.b64encode(cred.encode()).decode()
    return f"Basic {encoded}"


def resolve_delegate_config(proxy_config: dict[str, Any] | None) -> dict[str, Any] | None:
    """
    解析代发配置（仅 aether-proxy 节点支持，手动节点/旧格式 URL 不支持）

    无特定代理时自动回退到系统默认代理。
    auth_header 延迟生成：通过 ``fresh_auth_header()`` 闭包在每次请求 / 重试时
    获取新鲜的 HMAC 签名，避免长生命周期内时间戳过期。

    Returns:
        {"delegate_url": str, "node_id": str, "tls_enabled": bool,
         "auth_header": str,                 # 首次生成的签名（兼容旧调用）
         "fresh_auth_header": Callable}      # 延迟生成签名的闭包
        或 None
    """
    effective_config = proxy_config

    if not effective_config or not effective_config.get("enabled", True):
        effective_config = get_system_proxy_config()

    if not effective_config or not effective_config.get("enabled", True):
        return None

    node_id = effective_config.get("node_id")
    if not isinstance(node_id, str) or not node_id.strip():
        return None  # 旧格式 URL 模式不支持代发

    node_id = node_id.strip()
    node_info = _get_proxy_node_info(node_id)
    if not node_info or node_info.get("is_manual"):
        return None  # 手动节点不支持代发

    tls_enabled = node_info.get("tls_enabled", False)
    host = f"[{node_info['ip']}]" if ":" in node_info["ip"] else node_info["ip"]
    scheme = "https" if tls_enabled else "http"
    delegate_url = f"{scheme}://{host}:{int(node_info['port'])}/_aether/delegate"

    # 每次调用生成新鲜签名（避免长连接内时间戳过期）
    def _fresh() -> str:
        return _build_hmac_auth_header()

    return {
        "delegate_url": delegate_url,
        "auth_header": _fresh(),  # 立即生成一份，兼容旧调用方
        "fresh_auth_header": _fresh,
        "node_id": node_id,
        "tls_enabled": tls_enabled,
    }


# ---------------------------------------------------------------------------
# 代发请求参数构建（消除 handler 层重复代码）
# ---------------------------------------------------------------------------

_JSON_CT = "application/json"


def _build_delegate_kwargs_core(
    delegate_cfg: dict[str, Any],
    *,
    url: str,
    headers: dict[str, str],
    payload: Any,
    timeout: float,
    refresh_auth: bool = False,
) -> dict[str, Any]:
    """
    构建代发请求的核心参数（post/stream 共用）

    元数据通过 HTTP headers 传递（X-Delegate-Method/Url/Headers），
    上游请求体 gzip 压缩后直接作为 HTTP body 发送，大幅减少跨国传输耗时。

    Args:
        delegate_cfg: resolve_delegate_config 返回的配置
        url:          上游实际 URL
        headers:      上游请求头
        payload:      上游 JSON body（可以为 None）
        timeout:      上游超时秒数
        refresh_auth: 为 True 时重新生成 HMAC 签名（用于 retry）
    """
    auth = (
        delegate_cfg["fresh_auth_header"]()
        if refresh_auth
        else delegate_cfg.get("auth_header") or delegate_cfg["fresh_auth_header"]()
    )

    # 上游 headers base64 编码
    headers_b64 = base64.b64encode(_json.dumps(headers, ensure_ascii=False).encode("utf-8")).decode(
        "ascii"
    )

    # 构建代发请求 headers（元数据）
    delegate_headers: dict[str, str] = {
        "Authorization": auth,
        "X-Delegate-Method": "POST",
        "X-Delegate-Url": url,
        "X-Delegate-Headers": headers_b64,
        "X-Delegate-Timeout": str(int(timeout)),
    }

    kwargs: dict[str, Any] = {
        "url": delegate_cfg["delegate_url"],
        "headers": delegate_headers,
        "timeout": httpx.Timeout(timeout + 10),
    }

    # body gzip 压缩后直接作为 HTTP content
    if payload is not None:
        body_bytes = _json.dumps(payload, ensure_ascii=False).encode("utf-8")
        compressed = _gzip.compress(body_bytes)
        kwargs["content"] = compressed
        kwargs["headers"]["Content-Encoding"] = "gzip"
        kwargs["headers"]["Content-Type"] = _JSON_CT

    return kwargs


def build_delegate_post_kwargs(
    delegate_cfg: dict[str, Any],
    *,
    url: str,
    headers: dict[str, str],
    payload: Any,
    timeout: float,
    refresh_auth: bool = False,
) -> dict[str, Any]:
    """构建代发 POST 请求的 httpx kwargs（非流式，传给 client.post）"""
    return _build_delegate_kwargs_core(
        delegate_cfg,
        url=url,
        headers=headers,
        payload=payload,
        timeout=timeout,
        refresh_auth=refresh_auth,
    )


def build_delegate_stream_kwargs(
    delegate_cfg: dict[str, Any],
    *,
    url: str,
    headers: dict[str, str],
    payload: Any,
    timeout: float,
    refresh_auth: bool = False,
) -> dict[str, Any]:
    """构建代发 stream 请求的 httpx kwargs（传给 client.stream）"""
    kwargs = _build_delegate_kwargs_core(
        delegate_cfg,
        url=url,
        headers=headers,
        payload=payload,
        timeout=timeout,
        refresh_auth=refresh_auth,
    )
    # stream() 需要显式 method 参数
    kwargs["method"] = "POST"
    return kwargs


# ---------------------------------------------------------------------------
# 统一上游请求参数构建（消除 handler 层 delegate/直连 分支重复）
# ---------------------------------------------------------------------------


def build_post_kwargs(
    delegate_cfg: dict[str, Any] | None,
    *,
    url: str,
    headers: dict[str, str],
    payload: Any,
    timeout: float,
    refresh_auth: bool = False,
) -> dict[str, Any]:
    """
    构建上游 POST 请求的 httpx kwargs（自动选择代发或直连模式）

    返回的 dict 可直接传给 ``http_client.post(**kwargs)``。
    """
    if delegate_cfg:
        return build_delegate_post_kwargs(
            delegate_cfg,
            url=url,
            headers=headers,
            payload=payload,
            timeout=timeout,
            refresh_auth=refresh_auth,
        )
    return {
        "url": url,
        "json": payload,
        "headers": headers,
        "timeout": httpx.Timeout(timeout),
    }


def build_stream_kwargs(
    delegate_cfg: dict[str, Any] | None,
    *,
    url: str,
    headers: dict[str, str],
    payload: Any,
    timeout: float | None = None,
) -> dict[str, Any]:
    """
    构建上游 stream 请求的 httpx kwargs（自动选择代发或直连模式）

    返回的 dict 可直接传给 ``http_client.stream(**kwargs)``。

    当 ``timeout`` 为 None（直连模式下由外层 asyncio.wait_for 控制超时），
    直连分支不设置 timeout；代发分支始终携带 timeout（proxy 协议需要）。
    """
    if delegate_cfg:
        return build_delegate_stream_kwargs(
            delegate_cfg,
            url=url,
            headers=headers,
            payload=payload,
            timeout=timeout or 60,
        )
    kwargs: dict[str, Any] = {
        "method": "POST",
        "url": url,
        "json": payload,
        "headers": headers,
    }
    if timeout is not None:
        kwargs["timeout"] = httpx.Timeout(timeout)
    return kwargs
