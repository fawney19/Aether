"""
全局Redis客户端管理

提供统一的Redis客户端访问，确保所有服务使用同一个连接池

熔断器说明:
- 连续失败达到阈值后开启熔断
- 熔断期间返回明确的状态而非静默失败
- 调用方可以根据状态决定降级策略
"""

import os
import time
from enum import Enum
from typing import Optional

import redis.asyncio as aioredis
from src.core.logger import logger
from redis.asyncio import sentinel as redis_sentinel


class RedisState(Enum):
    """Redis 连接状态"""

    NOT_INITIALIZED = "not_initialized"  # 未初始化
    CONNECTED = "connected"  # 已连接
    CIRCUIT_OPEN = "circuit_open"  # 熔断中
    DISCONNECTED = "disconnected"  # 断开连接


class RedisClientManager:
    """
    Redis客户端管理器（单例）

    提供 Redis 连接管理、熔断器保护和状态监控。
    """

    _instance: Optional["RedisClientManager"] = None
    _redis: Optional[aioredis.Redis] = None

    def __new__(cls):
        """单例模式"""
        if cls._instance is None:
            cls._instance = super().__new__(cls)
        return cls._instance

    def __init__(self):
        # 避免重复初始化
        if getattr(self, "_initialized", False):
            return

        self._initialized = True
        self._circuit_open_until: Optional[float] = None
        self._consecutive_failures: int = 0
        self._circuit_threshold = int(os.getenv("REDIS_CIRCUIT_BREAKER_THRESHOLD", "3"))
        self._circuit_reset_seconds = int(os.getenv("REDIS_CIRCUIT_BREAKER_RESET_SECONDS", "60"))
        self._last_error: Optional[str] = None  # 记录最后一次错误

    def get_state(self) -> RedisState:
        """
        获取 Redis 连接状态

        Returns:
            当前连接状态枚举值
        """
        if self._redis is not None:
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
        logger.info("Redis 熔断器手动重置")
        self._circuit_open_until = None
        self._consecutive_failures = 0
        self._last_error = None

    async def initialize(self, require_redis: bool = False) -> Optional[aioredis.Redis]:
        """
        初始化Redis连接

        Args:
            require_redis: 是否强制要求Redis连接成功，如果为True则连接失败时抛出异常

        Returns:
            Redis客户端实例，如果连接失败返回None（当require_redis=False时）

        Raises:
            RuntimeError: 当require_redis=True且连接失败时
        """
        if self._redis is not None:
            return self._redis

        # 检查熔断状态
        if self._circuit_open_until and time.time() < self._circuit_open_until:
            remaining = self._circuit_open_until - time.time()
            logger.warning(
                "Redis 客户端处于熔断状态，跳过初始化，剩余 {:.1f} 秒 (last_error: {})",
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
                self._redis = sentinel.master_for(
                    service_name=sentinel_service,
                    max_connections=redis_max_conn,
                    decode_responses=True,
                    socket_connect_timeout=5.0,
                    health_check_interval=30,  # 每 30 秒检查连接健康状态
                )
                safe_url = f"sentinel://{sentinel_service}"
            else:
                self._redis = await aioredis.from_url(
                    redis_url,
                    encoding="utf-8",
                    decode_responses=True,
                    socket_timeout=5.0,
                    socket_connect_timeout=5.0,
                    max_connections=redis_max_conn,
                    health_check_interval=30,  # 每 30 秒检查连接健康状态
                )
                safe_url = redis_url.split("@")[-1] if "@" in redis_url else redis_url

            # 测试连接
            await self._redis.ping()
            logger.info(f"[OK] 全局Redis客户端初始化成功: {safe_url}")
            self._consecutive_failures = 0
            self._circuit_open_until = None
            return self._redis
        except Exception as e:
            error_msg = str(e)
            self._last_error = error_msg
            logger.error(f"[ERROR] Redis连接失败: {error_msg}")

            self._consecutive_failures += 1
            if self._consecutive_failures >= self._circuit_threshold:
                self._circuit_open_until = time.time() + self._circuit_reset_seconds
                logger.warning(
                    "Redis 初始化连续失败 {} 次，开启熔断 {} 秒。"
                    "熔断期间以下功能将降级: 缓存亲和性、分布式并发控制、RPM限流。"
                    "可通过管理 API /api/admin/system/redis/reset-circuit 手动重置。",
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

            logger.warning(
                "[WARN] Redis 不可用，以下功能将降级运行（仅在单实例环境下安全）:\n"
                "  - 缓存亲和性: 禁用（每次请求随机选择 Endpoint）\n"
                "  - 分布式并发控制: 降级为本地计数\n"
                "  - RPM 限流: 降级为本地限流"
            )
            self._redis = None
            return None

    async def close(self) -> None:
        """关闭Redis连接"""
        if self._redis:
            await self._redis.close()
            self._redis = None
            logger.info("全局Redis客户端已关闭")

    def get_client(self) -> Optional[aioredis.Redis]:
        """
        获取Redis客户端（非异步）

        注意：必须先调用initialize()初始化

        Returns:
            Redis客户端实例或None
        """
        return self._redis


# 全局单例
_redis_manager: Optional[RedisClientManager] = None


async def get_redis_client(require_redis: bool = False) -> Optional[aioredis.Redis]:
    """
    获取全局Redis客户端

    Args:
        require_redis: 是否强制要求Redis连接成功，如果为True则连接失败时抛出异常

    Returns:
        Redis客户端实例，如果未初始化或连接失败返回None（当require_redis=False时）

    Raises:
        RuntimeError: 当require_redis=True且连接失败时
    """
    global _redis_manager

    if _redis_manager is None:
        _redis_manager = RedisClientManager()
    # 如果尚未连接（例如启动时降级、或 close() 后），尝试重新初始化。
    # initialize() 内部包含熔断器逻辑，避免频繁重试导致抖动。
    if _redis_manager.get_client() is None:
        await _redis_manager.initialize(require_redis=require_redis)

    return _redis_manager.get_client()


def get_redis_client_sync() -> Optional[aioredis.Redis]:
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
    """关闭全局Redis客户端"""
    global _redis_manager

    if _redis_manager:
        await _redis_manager.close()


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

    Returns:
        是否成功重置
    """
    global _redis_manager

    if _redis_manager is None:
        return False

    _redis_manager.reset_circuit_breaker()
    return True
