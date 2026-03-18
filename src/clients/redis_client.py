"""
全局Redis客户端管理

提供统一的Redis客户端访问，确保所有服务使用同一个连接池

熔断器说明:
- 连续失败达到阈值后开启熔断
- 熔断期间返回明确的状态而非静默失败
- 调用方可以根据状态决定降级策略
"""

from __future__ import annotations

import asyncio
import os
import time
from enum import Enum

import redis.asyncio as aioredis
from redis.asyncio import sentinel as redis_sentinel

from src.core.logger import logger


class RedisState(Enum):
    """Redis 连接状态"""

    NOT_INITIALIZED = "not_initialized"  # 未初始化
    CONNECTED = "connected"  # 已连接
    CIRCUIT_OPEN = "circuit_open"  # 熔断中
    DISCONNECTED = "disconnected"  # 断开连接


class RedisClientManager:
    """
    Redis 客户端管理器

    提供 Redis 连接管理、熔断器保护和状态监控。
    """

    def __init__(
        self,
        *,
        client_name: str,
        encoding_errors: str = "strict",
        degraded_warning: str | None = None,
    ) -> None:
        self._redis: aioredis.Redis | None = None
        self._redis_by_loop: dict[int, aioredis.Redis] = {}
        self._primary_loop_key: int | None = None
        self._client_name = client_name
        self._encoding_errors = encoding_errors
        self._degraded_warning = degraded_warning
        self._circuit_open_until: float | None = None
        self._consecutive_failures: int = 0
        self._circuit_threshold = int(os.getenv("REDIS_CIRCUIT_BREAKER_THRESHOLD", "3"))
        self._circuit_reset_seconds = int(os.getenv("REDIS_CIRCUIT_BREAKER_RESET_SECONDS", "60"))
        self._last_error: str | None = None  # 记录最后一次错误

    @staticmethod
    def _current_loop_key() -> int | None:
        try:
            return id(asyncio.get_running_loop())
        except RuntimeError:
            return None

    def get_state(self) -> RedisState:
        """
        获取 Redis 连接状态

        Returns:
            当前连接状态枚举值
        """
        if self._redis_by_loop or self._redis is not None:
            return RedisState.CONNECTED
        if self._circuit_open_until and time.time() < self._circuit_open_until:
            return RedisState.CIRCUIT_OPEN
        if self._last_error:
            return RedisState.DISCONNECTED
        return RedisState.NOT_INITIALIZED

    def get_circuit_info(self) -> dict:
        """
        获取熔断器详细信息

        Returns:
            包含熔断器状态的字典
        """
        state = self.get_state()
        info = {
            "state": state.value,
            "consecutive_failures": self._consecutive_failures,
            "circuit_threshold": self._circuit_threshold,
            "last_error": self._last_error,
        }

        if state == RedisState.CIRCUIT_OPEN and self._circuit_open_until:
            info["circuit_remaining_seconds"] = max(0, self._circuit_open_until - time.time())

        return info

    def reset_circuit_breaker(self) -> None:
        """
        手动重置熔断器（用于管理后台紧急恢复）
        """
        logger.info("{} 熔断器手动重置", self._client_name)
        self._circuit_open_until = None
        self._consecutive_failures = 0
        self._last_error = None

    async def initialize(self, require_redis: bool = False) -> aioredis.Redis | None:
        """
        初始化Redis连接

        Args:
            require_redis: 是否强制要求Redis连接成功，如果为True则连接失败时抛出异常

        Returns:
            Redis客户端实例，如果连接失败返回None（当require_redis=False时）

        Raises:
            RuntimeError: 当require_redis=True且连接失败时
        """
        loop_key = self._current_loop_key()
        if loop_key is not None:
            existing_client = self._redis_by_loop.get(loop_key)
            if existing_client is not None:
                return existing_client

        # 检查熔断状态
        if self._circuit_open_until and time.time() < self._circuit_open_until:
            remaining = self._circuit_open_until - time.time()
            logger.warning(
                "{} 处于熔断状态，跳过初始化，剩余 {:.1f} 秒 (last_error: {})",
                self._client_name,
                remaining,
                self._last_error,
            )
            if require_redis:
                raise RuntimeError(
                    f"Redis 处于熔断状态，剩余 {remaining:.1f} 秒。"
                    f"最后错误: {self._last_error}。"
                    "使用管理 API 重置熔断器或等待自动恢复。"
                )
            return None

        # 优先使用 REDIS_URL，如果没有则根据密码构建 URL
        redis_url = os.getenv("REDIS_URL")
        redis_max_conn = int(os.getenv("REDIS_MAX_CONNECTIONS", "50"))
        sentinel_hosts = os.getenv("REDIS_SENTINEL_HOSTS")
        sentinel_service = os.getenv("REDIS_SENTINEL_SERVICE_NAME", "mymaster")
        redis_password = os.getenv("REDIS_PASSWORD")

        if not redis_url and not sentinel_hosts:
            # 本地开发模式：从 REDIS_PASSWORD 构建 URL
            if redis_password:
                redis_url = f"redis://:{redis_password}@localhost:6379/0"
            else:
                redis_url = "redis://localhost:6379/0"

        try:
            if sentinel_hosts:
                sentinel_list = []
                for host in sentinel_hosts.split(","):
                    host = host.strip()
                    if not host:
                        continue
                    if ":" in host:
                        hostname, port = host.split(":", 1)
                        sentinel_list.append((hostname, int(port)))
                    else:
                        sentinel_list.append((host, 26379))

                sentinel_kwargs = {
                    "password": redis_password,
                    "socket_timeout": 5.0,
                }
                sentinel = redis_sentinel.Sentinel(
                    sentinel_list,
                    **sentinel_kwargs,
                )
                client = sentinel.master_for(
                    service_name=sentinel_service,
                    max_connections=redis_max_conn,
                    decode_responses=True,
                    encoding_errors=self._encoding_errors,
                    socket_connect_timeout=5.0,
                    health_check_interval=30,  # 每 30 秒检查连接健康状态
                )
                safe_url = f"sentinel://{sentinel_service}"
            else:
                client = await aioredis.from_url(
                    redis_url,
                    encoding="utf-8",
                    decode_responses=True,
                    encoding_errors=self._encoding_errors,
                    socket_timeout=5.0,
                    socket_connect_timeout=5.0,
                    max_connections=redis_max_conn,
                    health_check_interval=30,  # 每 30 秒检查连接健康状态
                )
                safe_url = redis_url.split("@")[-1] if "@" in redis_url else redis_url

            # 测试连接
            await client.ping()
            if loop_key is not None:
                self._redis_by_loop[loop_key] = client
                if self._primary_loop_key is None:
                    self._primary_loop_key = loop_key
                    self._redis = client
            elif self._redis is None:
                self._redis = client
            logger.info("[OK] {} 初始化成功: {}", self._client_name, safe_url)
            self._consecutive_failures = 0
            self._circuit_open_until = None
            return client
        except Exception as e:
            error_msg = str(e)
            self._last_error = error_msg
            logger.error("[ERROR] {} 连接失败: {}", self._client_name, error_msg)

            self._consecutive_failures += 1
            if self._consecutive_failures >= self._circuit_threshold:
                self._circuit_open_until = time.time() + self._circuit_reset_seconds
                logger.warning(
                    "{} 初始化连续失败 {} 次，开启熔断 {} 秒。"
                    "可通过管理 API /api/admin/system/redis/reset-circuit 手动重置。",
                    self._client_name,
                    self._consecutive_failures,
                    self._circuit_reset_seconds,
                )

            if require_redis:
                # 强制要求Redis时，抛出异常拒绝启动
                raise RuntimeError(
                    f"Redis连接失败: {error_msg}\n"
                    "缓存亲和性功能需要Redis支持，请确保Redis服务正常运行。\n"
                    "检查事项：\n"
                    "1. Redis服务是否已启动（docker compose up -d redis）\n"
                    "2. 环境变量 REDIS_URL 或 REDIS_PASSWORD 是否配置正确\n"
                    "3. Redis端口（默认6379）是否可访问"
                ) from e

            if self._degraded_warning:
                logger.warning(self._degraded_warning)
            return None

    async def close(self) -> None:
        """关闭Redis连接"""
        clients = list(self._redis_by_loop.values())
        if self._redis is not None and not clients:
            clients = [self._redis]

        seen_client_ids: set[int] = set()
        for client in clients:
            client_id = id(client)
            if client_id in seen_client_ids:
                continue
            seen_client_ids.add(client_id)
            await client.close()

        if clients:
            logger.info("{} 已关闭", self._client_name)

        self._redis = None
        self._redis_by_loop.clear()
        self._primary_loop_key = None

    def get_current_loop_client(self) -> aioredis.Redis | None:
        """获取当前事件循环绑定的 Redis 客户端。"""
        loop_key = self._current_loop_key()
        if loop_key is None:
            return self.get_client()
        return self._redis_by_loop.get(loop_key)

    def get_client(self) -> aioredis.Redis | None:
        """
        获取Redis客户端（非异步）

        注意：必须先调用initialize()初始化

        Returns:
            Redis客户端实例或None
        """
        if self._redis is not None:
            return self._redis
        if self._primary_loop_key is not None:
            primary = self._redis_by_loop.get(self._primary_loop_key)
            if primary is not None:
                self._redis = primary
                return primary
        if self._redis_by_loop:
            return next(iter(self._redis_by_loop.values()))
        return None


_GLOBAL_REDIS_DEGRADED_WARNING = (
    "[WARN] Redis 不可用，以下功能将降级运行（仅在单实例环境下安全）:\n"
    "  - 缓存亲和性: 禁用（每次请求随机选择 Endpoint）\n"
    "  - 分布式并发控制: 降级为本地计数\n"
    "  - RPM 限流: 降级为本地限流"
)
_USAGE_QUEUE_REDIS_DEGRADED_WARNING = (
    "[WARN] Usage Queue Redis 不可用，usage queue 写入与消费将暂时不可用"
)

_redis_manager: RedisClientManager | None = None
_usage_queue_redis_manager: RedisClientManager | None = None


def _get_global_redis_manager() -> RedisClientManager:
    global _redis_manager

    if _redis_manager is None:
        _redis_manager = RedisClientManager(
            client_name="全局Redis客户端",
            encoding_errors="strict",
            degraded_warning=_GLOBAL_REDIS_DEGRADED_WARNING,
        )
    return _redis_manager


def _get_usage_queue_redis_manager() -> RedisClientManager:
    global _usage_queue_redis_manager

    if _usage_queue_redis_manager is None:
        _usage_queue_redis_manager = RedisClientManager(
            client_name="Usage Queue Redis客户端",
            encoding_errors="surrogateescape",
            degraded_warning=_USAGE_QUEUE_REDIS_DEGRADED_WARNING,
        )
    return _usage_queue_redis_manager


async def get_redis_client(require_redis: bool = False) -> aioredis.Redis | None:
    """
    获取全局Redis客户端

    Args:
        require_redis: 是否强制要求Redis连接成功，如果为True则连接失败时抛出异常

    Returns:
        Redis客户端实例，如果未初始化或连接失败返回None（当require_redis=False时）

    Raises:
        RuntimeError: 当require_redis=True且连接失败时
    """
    manager = _get_global_redis_manager()
    # 如果尚未连接（例如启动时降级、或 close() 后），尝试重新初始化。
    # initialize() 内部包含熔断器逻辑，避免频繁重试导致抖动。
    if manager.get_current_loop_client() is None:
        await manager.initialize(require_redis=require_redis)

    return manager.get_current_loop_client()


async def get_usage_queue_redis_client(require_redis: bool = False) -> aioredis.Redis | None:
    """
    获取 Usage Queue 专用 Redis 客户端。

    与全局 Redis 客户端隔离，专门用于 usage queue 的 msgpack/surrogateescape 编解码链路。
    """
    manager = _get_usage_queue_redis_manager()
    if manager.get_current_loop_client() is None:
        await manager.initialize(require_redis=require_redis)

    return manager.get_current_loop_client()


def get_redis_client_sync() -> aioredis.Redis | None:
    """
    同步获取Redis客户端（不会初始化）

    Returns:
        Redis客户端实例或None
    """
    global _redis_manager

    if _redis_manager is None:
        return None

    return _redis_manager.get_client()


async def close_redis_client() -> None:
    """关闭 Redis 客户端（包含全局客户端和 Usage Queue 专用客户端）"""
    if _redis_manager:
        await _redis_manager.close()
    if _usage_queue_redis_manager:
        await _usage_queue_redis_manager.close()


def get_redis_state() -> RedisState:
    """
    获取 Redis 连接状态（同步方法）

    Returns:
        Redis 连接状态枚举
    """
    global _redis_manager

    if _redis_manager is None:
        return RedisState.NOT_INITIALIZED

    return _redis_manager.get_state()


def get_redis_circuit_info() -> dict:
    """
    获取 Redis 熔断器详细信息（同步方法）

    Returns:
        熔断器状态字典
    """
    global _redis_manager

    if _redis_manager is None:
        return {
            "state": RedisState.NOT_INITIALIZED.value,
            "consecutive_failures": 0,
            "circuit_threshold": 3,
            "last_error": None,
        }

    return _redis_manager.get_circuit_info()


def reset_redis_circuit_breaker() -> bool:
    """
    手动重置 Redis 熔断器（同步方法）

    同时重置全局 Redis 客户端与 Usage Queue 专用客户端，
    避免其中一方仍停留在熔断状态导致功能未恢复。

    Returns:
        是否至少重置了一个客户端
    """
    reset_any = False

    if _redis_manager is not None:
        _redis_manager.reset_circuit_breaker()
        reset_any = True
    if _usage_queue_redis_manager is not None:
        _usage_queue_redis_manager.reset_circuit_breaker()
        reset_any = True

    return reset_any
