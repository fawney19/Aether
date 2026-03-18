"""
代理解析服务

集中管理代理 URL 构建、节点信息缓存、系统默认代理回退、代理信息追踪等逻辑。
供 HTTPClientPool、Handler、Provider Ops 等模块调用。
"""

from __future__ import annotations

import asyncio
import gzip
import hashlib
import json
import threading
import time
from typing import Any
from urllib.parse import quote, urlparse

import httpx

from src.core.exceptions import ProxyNodeUnavailableError
from src.core.http_compression import is_gzip_content_encoding
from src.core.logger import logger

# ---------------------------------------------------------------------------
# ProxyNode 信息缓存（降低高频 DB 查询开销）
# ---------------------------------------------------------------------------
_proxy_node_cache: dict[str, tuple[dict[str, Any] | None, float]] = {}
_proxy_node_cache_lock = threading.Lock()
_PROXY_NODE_CACHE_TTL_SECONDS = 3.0
_PROXY_NODE_CACHE_NEGATIVE_TTL_SECONDS = 5.0  # 不可用节点使用更短的 TTL，加速恢复感知
_PROXY_NODE_CACHE_MAX_SIZE = 256

# payload 超过此阈值时 build_*_kwargs_async 才走 to_thread，
# 避免小 payload 承担不必要的线程调度开销
_ASYNC_PAYLOAD_THRESHOLD = 64 * 1024


def _get_proxy_node_info(node_id: str) -> dict[str, Any] | None:
    """
    读取 ProxyNode 信息（带内存 TTL 缓存）

    通过 asyncio.to_thread 在工作线程中运行，使用 _proxy_node_cache_lock 保护
    缓存的并发读写安全。

    Returns:
        aether-proxy 节点: {"ip": str, "port": int, "name": str, ...}
        手动节点: {"is_manual": True, "name": str, "proxy_url": str, ...}
        不存在/非在线: None
    """
    now = time.time()

    # 快速路径：缓存命中
    with _proxy_node_cache_lock:
        cached = _proxy_node_cache.get(node_id)
        if cached:
            value, expires_at = cached
            if now < expires_at:
                return value

    # 缓存未命中，查询 DB
    from src.database import create_session
    from src.models.database import ProxyNode, ProxyNodeStatus

    db = create_session()
    try:
        node = db.query(ProxyNode).filter(ProxyNode.id == node_id).first()
        if not node:
            result: dict[str, Any] | None = None
            ttl = _PROXY_NODE_CACHE_NEGATIVE_TTL_SECONDS
        elif node.tunnel_mode and not node.is_manual:
            if node.status != ProxyNodeStatus.ONLINE or not bool(node.tunnel_connected):
                result = None
                ttl = _PROXY_NODE_CACHE_NEGATIVE_TTL_SECONDS
            else:
                result = {
                    "name": node.name,
                    "ip": node.ip,
                    "port": node.port,
                    "tunnel_mode": True,
                    "tunnel_connected": True,
                }
                ttl = _PROXY_NODE_CACHE_TTL_SECONDS
        elif node.status != ProxyNodeStatus.ONLINE:
            result = None
            ttl = _PROXY_NODE_CACHE_NEGATIVE_TTL_SECONDS
        elif node.is_manual:
            result = {
                "is_manual": True,
                "name": node.name,
                "proxy_url": node.proxy_url,
                "username": node.proxy_username,
                "password": node.proxy_password,
            }
            ttl = _PROXY_NODE_CACHE_TTL_SECONDS
        else:
            result = {
                "name": node.name,
                "ip": node.ip,
                "port": node.port,
                "tunnel_mode": bool(node.tunnel_mode),
                "tunnel_connected": bool(node.tunnel_connected),
            }
            ttl = _PROXY_NODE_CACHE_TTL_SECONDS
    finally:
        db.close()

    # 写回缓存（持锁做淘汰+写入，使用新时间戳以排除 DB 查询耗时）
    write_now = time.time()
    with _proxy_node_cache_lock:
        if len(_proxy_node_cache) >= _PROXY_NODE_CACHE_MAX_SIZE:
            evict_count = _PROXY_NODE_CACHE_MAX_SIZE // 4
            sorted_keys = sorted(_proxy_node_cache, key=lambda k: _proxy_node_cache[k][1])
            for k in sorted_keys[:evict_count]:
                del _proxy_node_cache[k]
        _proxy_node_cache[node_id] = (result, write_now + ttl)

    return result


# ---------------------------------------------------------------------------
# 系统默认代理
# ---------------------------------------------------------------------------
_system_proxy_cache: tuple[dict[str, Any] | None, float] | None = None
_system_proxy_cache_lock = threading.Lock()
_SYSTEM_PROXY_CACHE_TTL = 60.0


def invalidate_proxy_node_cache(node_id: str) -> None:
    """主动清除指定节点的信息缓存（tunnel 断开时调用，避免使用过期的连接状态）"""
    with _proxy_node_cache_lock:
        _proxy_node_cache.pop(node_id, None)


def invalidate_system_proxy_cache() -> None:
    """手动失效系统代理缓存（在删除节点等操作后调用）"""
    global _system_proxy_cache
    with _system_proxy_cache_lock:
        _system_proxy_cache = None


def get_system_proxy_config() -> dict[str, Any] | None:
    """
    获取系统默认代理配置（带 TTL 缓存）

    从 system_configs 表中读取 system_proxy_node_id。
    返回 {"node_id": "...", "enabled": True} 或 None。
    """
    global _system_proxy_cache
    now = time.time()

    # 快速路径：缓存命中
    with _system_proxy_cache_lock:
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
    except Exception as exc:
        logger.warning("获取系统默认代理配置失败: {}", exc)
        result = None
    finally:
        db.close()

    with _system_proxy_cache_lock:
        _system_proxy_cache = (result, now + _SYSTEM_PROXY_CACHE_TTL)
    return result


async def get_system_proxy_config_async() -> dict[str, Any] | None:
    """异步读取系统默认代理配置，避免在事件循环中执行同步 DB 查询。"""
    return await asyncio.to_thread(get_system_proxy_config)


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


def _resolve_effective_node(
    connector_config: dict[str, Any] | None,
) -> tuple[str | None, dict[str, Any] | None]:
    """
    从 connector_config 或系统默认代理中解析有效的 proxy_node_id 及其信息。

    Returns:
        (node_id, node_info) 或 (None, None)
    """
    if connector_config:
        node_id = connector_config.get("proxy_node_id")
        if isinstance(node_id, str) and node_id.strip():
            nid = node_id.strip()
            return nid, _get_proxy_node_info(nid)

    # 回退：系统默认代理
    system_proxy = get_system_proxy_config()
    if system_proxy:
        node_id_sys = system_proxy.get("node_id")
        if isinstance(node_id_sys, str) and node_id_sys.strip():
            nid = node_id_sys.strip()
            return nid, _get_proxy_node_info(nid)

    return None, None


def resolve_ops_proxy_config(
    connector_config: dict[str, Any] | None,
) -> tuple[str | httpx.Proxy | None, str | None]:
    """
    一次解析 ops connector 的代理参数和 tunnel 节点 ID

    合并 resolve_ops_proxy + resolve_ops_tunnel_node_id，避免重复调用
    _resolve_effective_node。两个返回值互斥：tunnel 模式时 proxy 为 None，
    非 tunnel 模式时 tunnel_node_id 为 None。

    优先级：
    1. connector_config.proxy_node_id（新格式）
    2. connector_config.proxy（旧格式 URL 字符串）
    3. 系统默认代理节点

    Returns:
        (proxy, tunnel_node_id)
    """
    from .tunnel_transport import is_tunnel_node

    node_id, node_info = _resolve_effective_node(connector_config)
    if node_id and node_info:
        if is_tunnel_node(node_info):
            return None, node_id
        try:
            url = build_proxy_url({"node_id": node_id, "enabled": True})
            return make_proxy_param(url), None
        except Exception as exc:
            logger.warning("解析 proxy_node_id={} 失败，回退到直连: {}", node_id, exc)
            return None, None

    # 旧格式：直接返回 proxy URL 字符串
    if connector_config:
        proxy = connector_config.get("proxy")
        if isinstance(proxy, str) and proxy.strip():
            return proxy, None

    return None, None


async def resolve_ops_proxy_config_async(
    connector_config: dict[str, Any] | None,
) -> tuple[str | httpx.Proxy | None, str | None]:
    """异步解析 ops connector 代理配置，避免同步 DB 查询阻塞事件循环。"""
    return await asyncio.to_thread(resolve_ops_proxy_config, connector_config)


def resolve_ops_proxy(
    connector_config: dict[str, Any] | None,
) -> str | httpx.Proxy | None:
    """从 ops connector.config 中解析代理参数（含系统默认回退）

    tunnel 模式节点不返回代理 URL。
    如需同时获取 tunnel_node_id，请使用 resolve_ops_proxy_config 避免重复解析。
    """
    proxy, _ = resolve_ops_proxy_config(connector_config)
    return proxy


def resolve_ops_tunnel_node_id(
    connector_config: dict[str, Any] | None,
) -> str | None:
    """解析 ops connector 的 tunnel 节点 ID

    如需同时获取 proxy，请使用 resolve_ops_proxy_config 避免重复解析。
    """
    _, tunnel_node_id = resolve_ops_proxy_config(connector_config)
    return tunnel_node_id


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

    # tunnel 模式优先：当代理节点为 tunnel 模式时，使用 tunnel transport 工厂
    delegate_cfg = resolve_delegate_config(proxy_config)
    if delegate_cfg and delegate_cfg.get("tunnel"):
        from src.services.proxy_node.tunnel_transport import create_tunnel_transport

        timeout_secs = timeout if isinstance(timeout, (int, float)) else 60.0
        kwargs["transport"] = create_tunnel_transport(delegate_cfg["node_id"], timeout=timeout_secs)
        return kwargs

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
            message = f"代理节点 {node_id} 不可用"
            raise ProxyNodeUnavailableError(message, node_id=node_id)

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

        # tunnel 模式节点：不构建 proxy URL（通过 TunnelTransport 处理）
        from .tunnel_transport import is_tunnel_node

        if is_tunnel_node(node_info):
            return None

        # aether-proxy 节点均为 tunnel 模式，不应走到这里
        logger.warning("非 tunnel 模式的 aether-proxy 节点不再支持: node_id={}", node_id)
        return None

    proxy_url: str | None = proxy_config.get("url")
    if not proxy_url:
        return None

    username = proxy_config.get("username")
    password = proxy_config.get("password")

    # 只要有用户名就添加认证信息（密码可以为空）
    if username:
        return inject_auth_into_proxy_url(proxy_url, username, password)

    return proxy_url


async def build_proxy_url_async(proxy_config: dict[str, Any] | None) -> str | None:
    """异步构建代理 URL，避免 ProxyNode 查询阻塞事件循环。"""
    if not proxy_config:
        return None
    return await asyncio.to_thread(build_proxy_url, proxy_config)


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
        info: dict[str, Any] = {"node_id": node_id, "node_name": node_name, "source": source}
        if node_info and node_info.get("is_manual"):
            info["is_manual"] = True
        return info

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


async def resolve_proxy_info_async(proxy_config: dict[str, Any] | None) -> dict[str, Any] | None:
    """异步解析代理摘要信息，避免事件循环被同步代理解析阻塞。"""
    return await asyncio.to_thread(resolve_proxy_info, proxy_config)


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

    # ProxyNode 模式：基于 node_id 缓存
    node_id = proxy_config.get("node_id")
    if isinstance(node_id, str) and node_id.strip():
        return f"proxy_node:{node_id.strip()}"

    # 构建代理 URL 作为缓存键的基础
    proxy_url = build_proxy_url(proxy_config)
    if not proxy_url:
        return "__no_proxy__"

    # 使用 MD5 哈希来避免过长的键名
    return f"proxy:{hashlib.md5(proxy_url.encode()).hexdigest()[:16]}"


# ---------------------------------------------------------------------------
# Tunnel 代理配置解析
# ---------------------------------------------------------------------------


def resolve_delegate_config(proxy_config: dict[str, Any] | None) -> dict[str, Any] | None:
    """
    解析 tunnel 代理配置（仅 aether-proxy tunnel 节点支持）

    无特定代理时自动回退到系统默认代理。
    tunnel 模式节点返回 {"tunnel": True, "node_id": str}，
    调用方应使用 TunnelTransport。

    Returns:
        {"tunnel": True, "node_id": str} 或 None
    """
    effective_config = proxy_config

    if not effective_config or not effective_config.get("enabled", True):
        effective_config = get_system_proxy_config()

    if not effective_config or not effective_config.get("enabled", True):
        return None

    node_id = effective_config.get("node_id")
    if not isinstance(node_id, str) or not node_id.strip():
        return None

    node_id = node_id.strip()
    node_info = _get_proxy_node_info(node_id)
    if not node_info or node_info.get("is_manual"):
        return None

    from .tunnel_transport import is_tunnel_node

    if is_tunnel_node(node_info):
        return {"tunnel": True, "node_id": node_id}

    return None


async def resolve_delegate_config_async(
    proxy_config: dict[str, Any] | None,
) -> dict[str, Any] | None:
    """异步解析 tunnel 代理配置，避免同步 DB 查询阻塞事件循环。"""
    return await asyncio.to_thread(resolve_delegate_config, proxy_config)


# ---------------------------------------------------------------------------
# 统一上游请求参数构建
# ---------------------------------------------------------------------------


def _maybe_compress_payload(
    payload: Any,
    headers: dict[str, str],
    client_content_encoding: str | None = None,
) -> tuple[bytes, dict[str, str]]:
    """
    将 payload 序列化为 JSON bytes，并按客户端请求行为决定是否 gzip 压缩。

    NOTE: 使用紧凑分隔符 ``(",", ":")`` 序列化（无空格），相比 httpx ``json=``
    参数的默认 ``json.dumps``（带空格分隔符）体积更小，所有上游 API 均兼容。

    Returns:
        (body_bytes, updated_headers)
    """
    json_bytes = json.dumps(payload, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    normalized_headers = {k: v for k, v in headers.items() if k.lower() != "content-encoding"}

    if is_gzip_content_encoding(client_content_encoding):
        compressed = gzip.compress(json_bytes, compresslevel=6)
        normalized_headers = {**normalized_headers, "Content-Encoding": "gzip"}
        return compressed, normalized_headers

    return json_bytes, normalized_headers


async def _maybe_compress_payload_async(
    payload: Any,
    headers: dict[str, str],
    client_content_encoding: str | None = None,
) -> tuple[bytes, dict[str, str]]:
    """异步版 _maybe_compress_payload，仅在 payload 超过阈值时走线程池。"""
    json_bytes = json.dumps(payload, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    normalized_headers = {k: v for k, v in headers.items() if k.lower() != "content-encoding"}

    if is_gzip_content_encoding(client_content_encoding):
        if len(json_bytes) >= _ASYNC_PAYLOAD_THRESHOLD:
            compressed = await asyncio.to_thread(gzip.compress, json_bytes, 6)
        else:
            compressed = gzip.compress(json_bytes, compresslevel=6)
        return compressed, {**normalized_headers, "Content-Encoding": "gzip"}

    return json_bytes, normalized_headers


def build_post_kwargs(
    _delegate_cfg: dict[str, Any] | None = None,
    *,
    url: str,
    headers: dict[str, str],
    payload: Any,
    timeout: float,
    client_content_encoding: str | None = None,
    refresh_auth: bool = False,
) -> dict[str, Any]:
    """
    构建上游 POST 请求的 httpx kwargs

    返回的 dict 可直接传给 ``http_client.post(**kwargs)``。

    ``_delegate_cfg`` 和 ``refresh_auth`` 已废弃（tunnel 模式下认证由 transport 层处理），
    保留仅为兼容现有调用方签名。
    """
    content, final_headers = _maybe_compress_payload(
        payload,
        headers,
        client_content_encoding=client_content_encoding,
    )
    return {
        "url": url,
        "content": content,
        "headers": final_headers,
        "timeout": httpx.Timeout(timeout),
    }


async def build_post_kwargs_async(
    _delegate_cfg: dict[str, Any] | None = None,
    *,
    url: str,
    headers: dict[str, str],
    payload: Any,
    timeout: float,
    client_content_encoding: str | None = None,
    refresh_auth: bool = False,
) -> dict[str, Any]:
    """构建 POST kwargs，大 payload 的序列化/压缩走线程池避免阻塞事件循环。"""
    content, final_headers = await _maybe_compress_payload_async(
        payload, headers, client_content_encoding
    )
    return {
        "url": url,
        "content": content,
        "headers": final_headers,
        "timeout": httpx.Timeout(timeout),
    }


def build_stream_kwargs(
    _delegate_cfg: dict[str, Any] | None = None,
    *,
    url: str,
    headers: dict[str, str],
    payload: Any,
    timeout: float | None = None,
    client_content_encoding: str | None = None,
) -> dict[str, Any]:
    """
    构建上游 stream 请求的 httpx kwargs

    返回的 dict 可直接传给 ``http_client.stream(**kwargs)``。
    当 ``timeout`` 为 None 时由外层 asyncio.wait_for 控制超时。

    ``_delegate_cfg`` 已废弃，保留仅为兼容现有调用方签名。
    """
    content, final_headers = _maybe_compress_payload(
        payload,
        headers,
        client_content_encoding=client_content_encoding,
    )
    kwargs: dict[str, Any] = {
        "method": "POST",
        "url": url,
        "content": content,
        "headers": final_headers,
    }
    if timeout is not None:
        kwargs["timeout"] = httpx.Timeout(timeout)
    return kwargs


async def build_stream_kwargs_async(
    _delegate_cfg: dict[str, Any] | None = None,
    *,
    url: str,
    headers: dict[str, str],
    payload: Any,
    timeout: float | None = None,
    client_content_encoding: str | None = None,
) -> dict[str, Any]:
    """构建 stream kwargs，大 payload 的序列化/压缩走线程池避免阻塞事件循环。"""
    content, final_headers = await _maybe_compress_payload_async(
        payload, headers, client_content_encoding
    )
    kwargs: dict[str, Any] = {
        "method": "POST",
        "url": url,
        "content": content,
        "headers": final_headers,
    }
    if timeout is not None:
        kwargs["timeout"] = httpx.Timeout(timeout)
    return kwargs
