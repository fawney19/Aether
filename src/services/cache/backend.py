"""
缓存后端抽象层

提供统一的缓存接口，支持多种后端实现：
1. LocalCache: 内存缓存（单实例，线程安全）
2. RedisCache: Redis 缓存（分布式）

使用场景：
- ModelCacheService: 模型解析缓存
- 其他需要缓存的服务
"""

from __future__ import annotations

import asyncio
import time
from abc import ABC, abstractmethod
from collections import OrderedDict
from typing import Any

import redis.asyncio as aioredis

from src.clients.redis_client import get_redis_client_sync
from src.core.logger import logger
from src.utils import json_helper as json


class BaseCacheBackend(ABC):
    """缓存后端抽象基类"""

    @abstractmethod
    async def get(self, key: str) -> Any | None:
        """获取缓存值"""
        pass

    @abstractmethod
    async def set(self, key: str, value: Any, ttl: int = 300) -> None:
        """设置缓存值"""
        pass

    @abstractmethod
    async def delete(self, key: str) -> None:
        """删除缓存值"""
        pass

    @abstractmethod
    async def clear(self, pattern: str | None = None) -> None:
        """清空缓存（支持模式匹配）"""
        pass

    @abstractmethod
    async def exists(self, key: str) -> bool:
        """检查键是否存在"""
        pass


class LocalCache(BaseCacheBackend):
    """本地内存缓存后端（LRU + TTL，线程安全）"""

    def __init__(self, max_size: int = 1000, default_ttl: int = 300):
        """
        初始化本地缓存

        Args:
            max_size: 最大缓存条目数
            default_ttl: 默认过期时间（秒）
        """
        self._cache: OrderedDict = OrderedDict()
        self._expiry: dict[str, float] = {}
        self._max_size = max_size
        self._default_ttl = default_ttl
        self._lock = asyncio.Lock()

    async def get(self, key: str) -> Any | None:
        """获取缓存值（线程安全）"""
        async with self._lock:
            if key not in self._cache:
                return None

            # 检查过期
            if key in self._expiry and time.time() > self._expiry[key]:
                # 过期，删除
                del self._cache[key]
                del self._expiry[key]
                return None

            # 更新访问顺序（LRU）
            self._cache.move_to_end(key)
            return self._cache[key]

    async def set(self, key: str, value: Any, ttl: int | None = None) -> None:
        """设置缓存值（线程安全）"""
        async with self._lock:
            if ttl is None:
                ttl = self._default_ttl

            # 如果键已存在，更新访问顺序
            if key in self._cache:
                self._cache.move_to_end(key)

            self._cache[key] = value
            self._expiry[key] = time.time() + ttl

            # 检查容量限制，淘汰最旧项
            if len(self._cache) > self._max_size:
                oldest_key = next(iter(self._cache))
                del self._cache[oldest_key]
                if oldest_key in self._expiry:
                    del self._expiry[oldest_key]

    async def delete(self, key: str) -> None:
        """删除缓存值（线程安全）"""
        async with self._lock:
            if key in self._cache:
                del self._cache[key]
            if key in self._expiry:
                del self._expiry[key]

    async def clear(self, pattern: str | None = None) -> None:
        """清空缓存（线程安全）"""
        async with self._lock:
            if pattern is None:
                # 清空所有
                self._cache.clear()
                self._expiry.clear()
            else:
                # 模式匹配删除（简单实现：支持前缀匹配）
                prefix = pattern.rstrip("*")
                keys_to_delete = [k for k in self._cache.keys() if k.startswith(prefix)]
                for key in keys_to_delete:
                    del self._cache[key]
                    if key in self._expiry:
                        del self._expiry[key]

    async def exists(self, key: str) -> bool:
        """检查键是否存在（线程安全）"""
        async with self._lock:
            if key not in self._cache:
                return False

            # 检查过期
            if key in self._expiry and time.time() > self._expiry[key]:
                del self._cache[key]
                del self._expiry[key]
                return False

            return True

    def get_stats(self) -> dict[str, Any]:
        """获取缓存统计信息"""
        return {
            "backend": "local",
            "size": len(self._cache),
            "max_size": self._max_size,
            "default_ttl": self._default_ttl,
        }


class RedisCache(BaseCacheBackend):
    """Redis 缓存后端（分布式）"""

    def __init__(
        self, redis_client: aioredis.Redis, key_prefix: str = "cache", default_ttl: int = 300
    ):
        """
        初始化 Redis 缓存

        Args:
            redis_client: Redis 客户端实例
            key_prefix: 缓存键前缀
            default_ttl: 默认过期时间（秒）
        """
        self._redis = redis_client
        self._key_prefix = key_prefix
        self._default_ttl = default_ttl

    def _make_key(self, key: str) -> str:
        """构造完整的 Redis 键"""
        return f"{self._key_prefix}:{key}"

    async def get(self, key: str) -> Any | None:
        """获取缓存值"""
        try:
            redis_key = self._make_key(key)
            value = await self._redis.get(redis_key)
            if value is None:
                return None

            # 尝试 JSON 反序列化
            try:
                return json.loads(value)
            except (json.JSONDecodeError, TypeError):
                # 如果不是 JSON，直接返回字符串
                return value
        except Exception as e:
            logger.error(f"[RedisCache] 获取缓存失败: {key}, 错误: {e}")
            return None

    async def set(self, key: str, value: Any, ttl: int | None = None) -> None:
        """设置缓存值"""
        if ttl is None:
            ttl = self._default_ttl

        try:
            redis_key = self._make_key(key)

            # 序列化值
            if isinstance(value, (dict, list, tuple)):
                serialized = json.dumps(value)
            elif isinstance(value, (int, float, bool)):
                serialized = json.dumps(value)
            else:
                serialized = str(value)

            await self._redis.setex(redis_key, ttl, serialized)
        except Exception as e:
            logger.error(f"[RedisCache] 设置缓存失败: {key}, 错误: {e}")

    async def delete(self, key: str) -> None:
        """删除缓存值"""
        try:
            redis_key = self._make_key(key)
            await self._redis.delete(redis_key)
        except Exception as e:
            logger.error(f"[RedisCache] 删除缓存失败: {key}, 错误: {e}")

    async def clear(self, pattern: str | None = None) -> None:
        """清空缓存"""
        try:
            if pattern is None:
                # 清空所有带前缀的键
                pattern = "*"

            redis_pattern = self._make_key(pattern)
            cursor = 0
            deleted_count = 0

            while True:
                cursor, keys = await self._redis.scan(cursor, match=redis_pattern, count=100)
                if keys:
                    await self._redis.delete(*keys)
                    deleted_count += len(keys)
                if cursor == 0:
                    break

            logger.info(f"[RedisCache] 清空缓存: {redis_pattern}, 删除 {deleted_count} 个键")
        except Exception as e:
            logger.error(f"[RedisCache] 清空缓存失败: {pattern}, 错误: {e}")

    async def exists(self, key: str) -> bool:
        """检查键是否存在"""
        try:
            redis_key = self._make_key(key)
            return await self._redis.exists(redis_key) > 0
        except Exception as e:
            logger.error(f"[RedisCache] 检查键存在失败: {key}, 错误: {e}")
            return False

    async def publish_invalidation(self, channel: str, key: str) -> None:
        """发布缓存失效消息（用于分布式同步）"""
        try:
            message = json.dumps({"key": key, "timestamp": time.time()})
            await self._redis.publish(channel, message)
            logger.debug(f"[RedisCache] 发布缓存失效: {channel} -> {key}")
        except Exception as e:
            logger.error(f"[RedisCache] 发布缓存失效失败: {channel}, {key}, 错误: {e}")

    def get_stats(self) -> dict[str, Any]:
        """获取缓存统计信息"""
        return {
            "backend": "redis",
            "key_prefix": self._key_prefix,
            "default_ttl": self._default_ttl,
        }


# 缓存后端工厂
_cache_backends: dict[str, BaseCacheBackend] = {}


async def get_cache_backend(
    name: str, backend_type: str = "auto", max_size: int = 1000, ttl: int = 300
) -> BaseCacheBackend:
    """
    获取缓存后端实例

    Args:
        name: 缓存名称（用于区分不同的缓存实例）
        backend_type: 后端类型 (auto/local/redis)
        max_size: LocalCache 的最大容量
        ttl: 默认过期时间（秒）

    Returns:
        BaseCacheBackend 实例
    """
    cache_key = f"{name}:{backend_type}"

    if cache_key in _cache_backends:
        return _cache_backends[cache_key]

    # 根据类型创建缓存后端
    if backend_type == "redis":
        # 尝试使用 Redis
        redis_client = get_redis_client_sync()

        if redis_client is None:
            logger.warning(f"[CacheBackend] Redis 未初始化，{name} 降级为本地缓存")
            backend = LocalCache(max_size=max_size, default_ttl=ttl)
        else:
            backend = RedisCache(redis_client=redis_client, key_prefix=name, default_ttl=ttl)
            logger.info(f"[CacheBackend] {name} 使用 Redis 缓存")

    elif backend_type == "local":
        # 强制使用本地缓存
        backend = LocalCache(max_size=max_size, default_ttl=ttl)
        logger.info(f"[CacheBackend] {name} 使用本地缓存")

    else:  # auto
        # 自动选择：优先 Redis，降级到 Local
        redis_client = get_redis_client_sync()

        if redis_client is not None:
            backend = RedisCache(redis_client=redis_client, key_prefix=name, default_ttl=ttl)
            logger.debug(f"[CacheBackend] {name} 自动选择 Redis 缓存")
        else:
            backend = LocalCache(max_size=max_size, default_ttl=ttl)
            logger.debug(f"[CacheBackend] {name} 自动选择本地缓存（Redis 不可用）")

    _cache_backends[cache_key] = backend
    return backend
