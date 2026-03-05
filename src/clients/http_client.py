"""
全局HTTP客户端池管理
避免每次请求都创建新的AsyncClient,提高性能

性能优化说明：
1. 默认客户端：无代理场景，全局复用单一客户端
2. 代理客户端缓存：相同代理配置复用同一客户端，避免重复创建
3. 连接池复用：Keep-alive 连接减少 TCP 握手开销
"""

from __future__ import annotations

import asyncio
import time
from contextlib import asynccontextmanager
from typing import Any

import httpx

from src.config import config
from src.core.logger import logger
from src.services.provider.fingerprint import KNOWN_IMPERSONATE_PROFILES
from src.services.proxy_node.resolver import (
    build_proxy_url,
    compute_proxy_cache_key,
    get_system_proxy_config,
    make_proxy_param,
)
from src.utils.ssl_utils import get_ssl_context, get_ssl_context_for_profile

# 模块级锁，避免类属性延迟初始化的竞态条件
_proxy_clients_lock = asyncio.Lock()
_default_client_lock = asyncio.Lock()


class HTTPClientPool:
    """
    全局HTTP客户端池单例

    管理可重用的httpx.AsyncClient实例,避免频繁创建/销毁连接

    性能优化：
    1. 默认客户端：无代理场景复用
    2. 代理客户端缓存：相同代理配置复用同一客户端
    3. LRU 淘汰：代理客户端超过上限时淘汰最久未使用的
    """

    _instance: HTTPClientPool | None = None
    _default_client: httpx.AsyncClient | None = None
    _clients: dict[str, httpx.AsyncClient] = {}
    # 代理客户端缓存：{cache_key: (client, last_used_time)}
    _proxy_clients: dict[str, tuple[httpx.AsyncClient, float]] = {}
    # 代理客户端缓存上限（避免内存泄漏）
    _max_proxy_clients: int = 50
    # Tunnel 客户端缓存：{node_id: client}
    _tunnel_clients: dict[str, httpx.AsyncClient] = {}

    def __new__(cls) -> "HTTPClientPool":
        if cls._instance is None:
            cls._instance = super().__new__(cls)
        return cls._instance

    @classmethod
    async def get_default_client_async(cls) -> httpx.AsyncClient:
        """
        获取默认的HTTP客户端（异步线程安全版本）

        用于大多数HTTP请求,具有合理的默认配置
        """
        if cls._default_client is not None:
            return cls._default_client

        async with _default_client_lock:
            # 双重检查，避免重复创建
            if cls._default_client is None:
                cls._default_client = httpx.AsyncClient(
                    http2=config.enable_http2,
                    verify=get_ssl_context(),  # 使用 certifi 证书
                    timeout=httpx.Timeout(
                        connect=config.http_connect_timeout,
                        read=config.http_read_timeout,
                        write=config.http_write_timeout,
                        pool=config.http_pool_timeout,
                    ),
                    limits=httpx.Limits(
                        max_connections=config.http_max_connections,
                        max_keepalive_connections=config.http_keepalive_connections,
                        keepalive_expiry=config.http_keepalive_expiry,
                    ),
                    follow_redirects=True,  # 跟随重定向
                )
                logger.info(
                    "全局HTTP客户端池已初始化: max_connections={}, keepalive={}, keepalive_expiry={}s",
                    config.http_max_connections,
                    config.http_keepalive_connections,
                    config.http_keepalive_expiry,
                )
        return cls._default_client

    @classmethod
    def get_default_client(cls) -> httpx.AsyncClient:
        """
        获取默认的HTTP客户端（同步版本，向后兼容）

        ⚠️ 注意：此方法在高并发首次调用时可能存在竞态条件，
        推荐使用 get_default_client_async() 异步版本。
        """
        if cls._default_client is None:
            cls._default_client = httpx.AsyncClient(
                http2=config.enable_http2,
                verify=get_ssl_context(),  # 使用 certifi 证书
                timeout=httpx.Timeout(
                    connect=config.http_connect_timeout,
                    read=config.http_read_timeout,
                    write=config.http_write_timeout,
                    pool=config.http_pool_timeout,
                ),
                limits=httpx.Limits(
                    max_connections=config.http_max_connections,
                    max_keepalive_connections=config.http_keepalive_connections,
                    keepalive_expiry=config.http_keepalive_expiry,
                ),
                follow_redirects=True,  # 跟随重定向
            )
            logger.info(
                "全局HTTP客户端池已初始化: max_connections={}, keepalive={}, keepalive_expiry={}s",
                config.http_max_connections,
                config.http_keepalive_connections,
                config.http_keepalive_expiry,
            )
        return cls._default_client

    @classmethod
    def get_client(cls, name: str, **kwargs: Any) -> httpx.AsyncClient:
        """
        获取或创建命名的HTTP客户端

        用于需要特定配置的场景(如不同的超时设置、代理等)

        Args:
            name: 客户端标识符
            **kwargs: httpx.AsyncClient的配置参数
        """
        if name not in cls._clients:
            # 合并默认配置和自定义配置
            default_config = {
                "http2": config.enable_http2,
                "verify": get_ssl_context(),
                "timeout": httpx.Timeout(
                    connect=config.http_connect_timeout,
                    read=config.http_read_timeout,
                    write=config.http_write_timeout,
                    pool=config.http_pool_timeout,
                ),
                "follow_redirects": True,
            }
            default_config.update(kwargs)

            cls._clients[name] = httpx.AsyncClient(**default_config)  # type: ignore[arg-type]
            logger.debug("创建命名HTTP客户端: {}", name)

        return cls._clients[name]

    @classmethod
    def _get_proxy_clients_lock(cls) -> asyncio.Lock:
        """获取代理客户端缓存锁（模块级单例，避免竞态条件）"""
        return _proxy_clients_lock

    @classmethod
    async def _evict_lru_proxy_client(cls) -> None:
        """淘汰最久未使用的代理客户端"""
        if len(cls._proxy_clients) < cls._max_proxy_clients:
            return

        # 找到最久未使用的客户端
        oldest_key = min(cls._proxy_clients.keys(), key=lambda k: cls._proxy_clients[k][1])
        old_client, _ = cls._proxy_clients.pop(oldest_key)

        # 异步关闭旧客户端
        try:
            await old_client.aclose()
            logger.debug("淘汰代理客户端: {}", oldest_key)
        except Exception as e:
            logger.warning("关闭代理客户端失败: {}", e)

    @classmethod
    async def get_proxy_client(
        cls,
        proxy_config: dict[str, Any] | None = None,
        tls_profile: str | None = None,
    ) -> httpx.AsyncClient:
        """
        获取代理客户端（带缓存复用）

        相同代理配置会复用同一个客户端，大幅减少连接建立开销。
        当 proxy_config 为 None 时，自动回退到系统默认代理节点。
        注意：返回的客户端使用默认超时配置，如需自定义超时请在请求时传递 timeout 参数。

        Args:
            proxy_config: 代理配置字典，为 None 时使用系统默认代理

        Returns:
            可复用的 httpx.AsyncClient 实例
        """
        # 无特定代理时，回退到系统默认代理
        if not proxy_config:
            proxy_config = get_system_proxy_config()

        # tunnel 模式检查：tunnel 节点走专用的 TunnelTransport 客户端
        from src.services.proxy_node.resolver import resolve_delegate_config

        delegate_cfg = resolve_delegate_config(proxy_config)
        if delegate_cfg and delegate_cfg.get("tunnel"):
            return await cls._get_tunnel_client(delegate_cfg["node_id"])

        cache_key = compute_proxy_cache_key(proxy_config)
        tls_profile_key = str(tls_profile or "").strip().lower()
        if tls_profile_key:
            cache_key = f"{cache_key}::tls:{tls_profile_key}"

        # 无代理时返回默认客户端
        if cache_key == "__no_proxy__":
            return await cls.get_default_client_async()

        lock = cls._get_proxy_clients_lock()
        async with lock:
            # 检查缓存
            if cache_key in cls._proxy_clients:
                client, _ = cls._proxy_clients[cache_key]
                # 健康检查：如果客户端已关闭，移除并重新创建
                if client.is_closed:
                    del cls._proxy_clients[cache_key]
                    logger.debug("代理客户端已关闭，将重新创建: {}", cache_key)
                else:
                    # 更新最后使用时间
                    cls._proxy_clients[cache_key] = (client, time.time())
                    if tls_profile_key:
                        logger.debug(
                            "复用代理客户端 TLS profile={} key={}", tls_profile_key, cache_key
                        )
                    return client

            # 淘汰旧客户端（如果超过上限）
            await cls._evict_lru_proxy_client()

            # 添加代理配置
            proxy_url = build_proxy_url(proxy_config) if proxy_config else None

            # curl_cffi Transport: real TLS fingerprint impersonation.
            # Supports:
            # - "claude_code_nodejs" (legacy alias, uses default chrome120 impersonate)
            # - direct chrome impersonate profile names (e.g. "chrome124")
            use_curl_cffi_tls = False
            if tls_profile_key:
                use_curl_cffi_tls = (
                    tls_profile_key == "claude_code_nodejs"
                    or tls_profile_key in KNOWN_IMPERSONATE_PROFILES
                )

            if use_curl_cffi_tls:
                from src.clients.curl_cffi_transport import (
                    CURL_CFFI_AVAILABLE,
                    CurlCffiTransport,
                )

                if CURL_CFFI_AVAILABLE:
                    transport_kwargs: dict[str, Any] = {"proxy": proxy_url}
                    if tls_profile_key != "claude_code_nodejs":
                        transport_kwargs["impersonate"] = tls_profile_key

                    transport = CurlCffiTransport(**transport_kwargs)
                    client = httpx.AsyncClient(
                        transport=transport,
                        follow_redirects=True,
                        timeout=httpx.Timeout(
                            connect=config.http_connect_timeout,
                            read=config.http_read_timeout,
                            write=config.http_write_timeout,
                            pool=config.http_pool_timeout,
                        ),
                    )
                    cls._proxy_clients[cache_key] = (client, time.time())
                    logger.info(
                        "创建 curl_cffi TLS 指纹客户端: profile={}, proxy={}",
                        tls_profile_key,
                        proxy_url or "direct",
                    )
                    return client
                else:
                    logger.warning(
                        "curl_cffi 不可用，回退到 best-effort TLS 配置 (profile={})",
                        tls_profile_key,
                    )

            # 创建新客户端（使用默认超时，请求时可覆盖）
            client_config: dict[str, Any] = {
                "http2": config.enable_http2,
                "verify": get_ssl_context_for_profile(tls_profile),
                "follow_redirects": True,
                "limits": httpx.Limits(
                    max_connections=config.http_max_connections,
                    max_keepalive_connections=config.http_keepalive_connections,
                    keepalive_expiry=config.http_keepalive_expiry,
                ),
                "timeout": httpx.Timeout(
                    connect=config.http_connect_timeout,
                    read=config.http_read_timeout,
                    write=config.http_write_timeout,
                    pool=config.http_pool_timeout,
                ),
            }

            proxy_param = make_proxy_param(proxy_url)
            if proxy_param:
                client_config["proxy"] = proxy_param

            client = httpx.AsyncClient(**client_config)  # type: ignore[arg-type]
            cls._proxy_clients[cache_key] = (client, time.time())

            proxy_label = "none"
            if proxy_config:
                proxy_label = str(
                    proxy_config.get("node_id") or proxy_config.get("url") or "unknown"
                )
            logger.debug(
                "创建代理客户端(缓存): {}, 缓存数量: {}", proxy_label, len(cls._proxy_clients)
            )
            if tls_profile_key:
                logger.debug("创建代理客户端 TLS profile={} key={}", tls_profile_key, cache_key)

            return client

    @classmethod
    async def close_all(cls) -> None:
        """关闭所有HTTP客户端"""
        if cls._default_client is not None:
            await cls._default_client.aclose()
            cls._default_client = None
            logger.info("默认HTTP客户端已关闭")

        for name, client in cls._clients.items():
            await client.aclose()
            logger.debug("命名HTTP客户端已关闭: {}", name)

        cls._clients.clear()

        # 关闭代理客户端缓存
        for cache_key, (client, _) in cls._proxy_clients.items():
            try:
                await client.aclose()
                logger.debug("代理客户端已关闭: {}", cache_key)
            except Exception as e:
                logger.warning("关闭代理客户端失败: {}", e)

        cls._proxy_clients.clear()

        # 关闭 tunnel 客户端缓存
        for nid, client in cls._tunnel_clients.items():
            try:
                await client.aclose()
                logger.debug("tunnel 客户端已关闭: {}", nid)
            except Exception as e:
                logger.warning("关闭 tunnel 客户端失败: {}", e)

        cls._tunnel_clients.clear()

        # 关闭 curl_cffi session 缓存
        try:
            from src.clients.curl_cffi_transport import CURL_CFFI_AVAILABLE, close_all_sessions

            if CURL_CFFI_AVAILABLE:
                await close_all_sessions()
                logger.debug("curl_cffi sessions 已关闭")
        except Exception as e:
            logger.debug("关闭 curl_cffi sessions 失败: {}", e)

        logger.info("所有HTTP客户端已关闭")

    @classmethod
    @asynccontextmanager
    async def get_temp_client(cls, **kwargs: Any) -> Any:
        """
        获取临时HTTP客户端(上下文管理器)

        用于一次性请求,使用后自动关闭

        用法:
            async with HTTPClientPool.get_temp_client() as client:
                response = await client.get('https://example.com')
        """
        default_config = {
            "http2": config.enable_http2,
            "verify": get_ssl_context(),
            "timeout": httpx.Timeout(
                connect=config.http_connect_timeout,
                read=config.http_read_timeout,
                write=config.http_write_timeout,
                pool=config.http_pool_timeout,
            ),
        }
        default_config.update(kwargs)

        client = httpx.AsyncClient(**default_config)  # type: ignore[arg-type]
        try:
            yield client
        finally:
            await client.aclose()

    @classmethod
    def create_client_with_proxy(
        cls,
        proxy_config: dict[str, Any] | None = None,
        timeout: httpx.Timeout | None = None,
        tls_profile: str | None = None,
        **kwargs: Any,
    ) -> httpx.AsyncClient:
        """
        创建带代理配置的HTTP客户端

        ⚠️ 性能警告：此方法每次都创建新客户端，推荐使用 get_proxy_client() 复用连接。

        Args:
            proxy_config: 代理配置字典，包含 url, username, password
            timeout: 超时配置
            **kwargs: 其他 httpx.AsyncClient 配置参数

        Returns:
            配置好的 httpx.AsyncClient 实例（调用者需要负责关闭）
        """
        client_config: dict[str, Any] = {
            "http2": config.enable_http2,
            "verify": get_ssl_context_for_profile(tls_profile),
            "follow_redirects": True,
        }

        if timeout:
            client_config["timeout"] = timeout
        else:
            client_config["timeout"] = httpx.Timeout(
                connect=config.http_connect_timeout,
                read=config.http_read_timeout,
                write=config.http_write_timeout,
                pool=config.http_pool_timeout,
            )

        # 无特定代理时，回退到系统默认代理（与 get_proxy_client 行为一致）
        if proxy_config is None:
            proxy_config = get_system_proxy_config()

        # 添加代理配置
        proxy_url = build_proxy_url(proxy_config) if proxy_config else None
        proxy_param = make_proxy_param(proxy_url)
        if proxy_param:
            client_config["proxy"] = proxy_param
            logger.debug("创建带代理的HTTP客户端(一次性): {}", proxy_config.get("url", "unknown"))

        client_config.update(kwargs)
        return httpx.AsyncClient(**client_config)  # type: ignore[arg-type]

    @classmethod
    async def get_upstream_client(
        cls,
        delegate_cfg: dict[str, Any] | None,
        proxy_config: dict[str, Any] | None = None,
        tls_profile: str | None = None,
    ) -> httpx.AsyncClient:
        """
        获取可复用的上游请求客户端（自动选择 tunnel/代理模式）

        tunnel 模式(delegate_cfg.tunnel=True)：返回 TunnelTransport 客户端
        直连/代理模式：返回代理客户端（含系统默认代理回退）
        """
        if delegate_cfg and delegate_cfg.get("tunnel"):
            return await cls._get_tunnel_client(delegate_cfg["node_id"])
        return await cls.get_proxy_client(proxy_config=proxy_config, tls_profile=tls_profile)

    @classmethod
    async def create_upstream_stream_client(
        cls,
        delegate_cfg: dict[str, Any] | None,
        proxy_config: dict[str, Any] | None = None,
        timeout: httpx.Timeout | None = None,
        tls_profile: str | None = None,
    ) -> httpx.AsyncClient:
        """
        创建上游流式请求客户端（自动选择 tunnel/代理模式）

        调用者需负责关闭返回的客户端。
        """
        if delegate_cfg and delegate_cfg.get("tunnel"):
            return await cls._get_tunnel_client(delegate_cfg["node_id"], timeout=timeout)
        return cls.create_client_with_proxy(
            proxy_config=proxy_config,
            timeout=timeout,
            tls_profile=tls_profile,
        )

    @classmethod
    async def _get_tunnel_client(
        cls,
        node_id: str,
        timeout: httpx.Timeout | None = None,
    ) -> httpx.AsyncClient:
        """获取使用 TunnelTransport 的 httpx 客户端

        当 timeout 为 None 时（非流式请求），返回按 node_id 缓存的 client，
        调用方不应关闭此 client，其生命周期由 HTTPClientPool 管理。
        当 timeout 非 None 时（流式请求），每次创建新 client，由调用方负责关闭。
        """
        from src.services.proxy_node.tunnel_transport import create_tunnel_transport

        t = timeout or httpx.Timeout(
            connect=config.http_connect_timeout,
            read=config.http_read_timeout,
            write=config.http_write_timeout,
            pool=config.http_pool_timeout,
        )
        timeout_secs = t.read if isinstance(t, httpx.Timeout) else 60.0

        # 流式请求：每次创建新 client（调用方负责关闭）
        if timeout is not None:
            transport = create_tunnel_transport(node_id, timeout=timeout_secs or 60.0)
            return httpx.AsyncClient(transport=transport, timeout=t)

        # 非流式请求：复用缓存的 client（加锁与 proxy_clients 保持一致）
        lock = cls._get_proxy_clients_lock()
        async with lock:
            existing = cls._tunnel_clients.get(node_id)
            if existing and not existing.is_closed:
                return existing

            transport = create_tunnel_transport(node_id, timeout=timeout_secs or 60.0)
            client = httpx.AsyncClient(transport=transport, timeout=t)
            cls._tunnel_clients[node_id] = client
            return client

    @classmethod
    def get_pool_stats(cls) -> dict[str, Any]:
        """获取连接池统计信息"""
        return {
            "default_client_active": cls._default_client is not None,
            "named_clients_count": len(cls._clients),
            "proxy_clients_count": len(cls._proxy_clients),
            "max_proxy_clients": cls._max_proxy_clients,
            "tunnel_clients_count": len(cls._tunnel_clients),
        }


# 便捷访问函数
def get_http_client() -> httpx.AsyncClient:
    """获取默认HTTP客户端的便捷函数"""
    return HTTPClientPool.get_default_client()


async def close_http_clients() -> None:
    """关闭所有HTTP客户端的便捷函数"""
    await HTTPClientPool.close_all()
