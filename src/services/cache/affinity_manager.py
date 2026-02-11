"""
缓存亲和性管理器 (Cache Affinity Manager) - 支持 Redis 或内存存储

职责:
1. 跟踪请求API Key的Provider+Key缓存状态
2. 管理缓存有效期
3. 提供缓存统计和分析
4. 自动失效不支持缓存的Provider

设计原理:
- 每个API Key使用某个Provider的Key后，在缓存TTL期内，应该继续使用同一个Key
- 这样可以最大化利用提供商的Prompt Caching机制
- 当Key故障时，自动失效该Key的缓存亲和性
- 当Provider关闭缓存支持时，自动失效所有相关亲和性

注意：
- affinity_key 参数通常为请求使用的 API Key ID（api_key_id）
- 这样可以支持"独立余额Key"场景，每个Key有自己的缓存亲和性
"""

from __future__ import annotations

import asyncio
import os
import time
from contextlib import asynccontextmanager
from typing import Any, NamedTuple

from src.config.constants import CacheTTL
from src.core.logger import logger
from src.utils import json_helper as json


class CacheAffinity(NamedTuple):
    """缓存亲和性信息"""

    provider_id: str
    endpoint_id: str
    key_id: str
    api_format: str  # API格式 (claude/openai)
    model_name: str  # 模型名称
    created_at: float  # 创建时间戳
    expire_at: float  # 过期时间戳
    request_count: int  # 使用次数


class CacheAffinityManager:
    """
    缓存亲和性管理器（支持 Redis 或内存存储）

    存储结构:
    ----------------------
    Key格式: cache_affinity:{affinity_key}:{api_format}:{model_name}
    - affinity_key: 通常为请求使用的 API Key ID（支持独立余额Key场景）
    - api_format: API格式 (claude/openai)
    - model_name: 模型名称（区分不同模型的缓存亲和性）
    Value格式: JSON/Dict
    {
        "provider_id": "xxx",
        "endpoint_id": "yyy",
        "key_id": "zzz",
        "model_name": "claude-3-5-sonnet-20241022",
        "created_at": 1234567890.123,
        "expire_at": 1234567890.123,
        "request_count": 5
    }
    TTL: 自动过期

    设计改进:
    - 每个API Key可以对多个API格式和模型分别维护缓存亲和性
    - 不同模型请求使用独立的缓存亲和性，避免模型切换导致的缓存失效
    - 某个端点故障切换不会影响其他端点的亲和性
    - 更精确的缓存命中率统计
    - 支持"独立余额Key"场景，每个Key有独立的缓存亲和性
    """

    # 默认缓存TTL（秒）- 使用统一常量
    DEFAULT_CACHE_TTL = CacheTTL.CACHE_AFFINITY

    def __init__(
        self, redis_client: Any | None = None, default_ttl: int = DEFAULT_CACHE_TTL
    ) -> None:
        """
        初始化缓存亲和性管理器

        Args:
            redis_client: Redis客户端（可选）
            default_ttl: 默认缓存TTL（秒）
        """
        self.redis = redis_client
        self.default_ttl = default_ttl
        self._memory_store: dict[str, dict[str, Any]] = {}
        self._memory_lock: asyncio.Lock | None = None

        # L1 缓存（即使使用 Redis 也启用，减少网络往返）
        self._l1_cache_ttl = int(os.getenv("CACHE_AFFINITY_L1_TTL", str(CacheTTL.L1_LOCAL)))
        self._l1_cache: dict[str, tuple[float, dict[str, Any]]] = {}
        self._l1_lock = asyncio.Lock()
        self._l1_max_size = int(os.getenv("CACHE_AFFINITY_L1_MAX_SIZE", "1000"))  # 最大缓存条目数
        self._l1_last_cleanup = time.time()

        # 请求级别锁，避免同一用户+端点同时更新造成抖动
        self._request_locks: dict[str, asyncio.Lock] = {}

        # 统计信息
        self._stats = {
            "total_affinities": 0,
            "cache_hits": 0,
            "cache_misses": 0,
            "cache_invalidations": 0,
            "provider_switches": 0,
            "key_switches": 0,
        }

        if self.redis:
            logger.debug("CacheAffinityManager: 使用Redis存储")
        else:
            logger.debug(
                "CacheAffinityManager: Redis不可用，回退到内存存储(仅适用于单实例/开发环境)"
            )

    def _is_memory_backend(self) -> bool:
        """是否处于内存模式"""
        return self.redis is None

    def _get_memory_lock(self) -> asyncio.Lock:
        """懒初始化内存锁"""
        if self._memory_lock is None:
            self._memory_lock = asyncio.Lock()
        return self._memory_lock

    def _get_cache_key(self, affinity_key: str, api_format: str, model_name: str) -> str:
        """
        生成Redis Key

        Args:
            affinity_key: 亲和性标识符（通常为API Key ID）
            api_format: API格式 (claude/openai)
            model_name: 模型名称

        Returns:
            格式化的缓存键: cache_affinity:{affinity_key}:{api_format}:{model_name}
        """
        return f"cache_affinity:{affinity_key}:{api_format}:{model_name}"

    async def _get_l1_entry(self, cache_key: str) -> dict[str, Any] | None:
        async with self._l1_lock:
            record = self._l1_cache.get(cache_key)
            if not record:
                return None
            expire_at, payload = record
            if time.time() > expire_at:
                self._l1_cache.pop(cache_key, None)
                return None
            return dict(payload)

    async def _set_l1_entry(self, cache_key: str, payload: dict[str, Any] | None) -> None:
        async with self._l1_lock:
            if not payload:
                self._l1_cache.pop(cache_key, None)
                return
            expire_at = time.time() + max(1, self._l1_cache_ttl)
            self._l1_cache[cache_key] = (expire_at, dict(payload))

            # 定期清理过期条目（每 60 秒最多一次）
            current_time = time.time()
            if current_time - self._l1_last_cleanup > 60:
                self._cleanup_l1_cache_unlocked(current_time)
                self._l1_last_cleanup = current_time

    def _cleanup_l1_cache_unlocked(self, current_time: float) -> int:
        """清理过期的 L1 缓存条目（需要在持有锁的情况下调用）

        Returns:
            清理的条目数量
        """
        expired_keys = [
            key for key, (expire_at, _) in self._l1_cache.items() if current_time > expire_at
        ]
        for key in expired_keys:
            self._l1_cache.pop(key, None)

        # 如果缓存仍然过大，按过期时间排序移除最旧的条目
        if len(self._l1_cache) > self._l1_max_size:
            sorted_items = sorted(
                self._l1_cache.items(), key=lambda x: x[1][0]  # 按 expire_at 排序
            )
            # 移除最旧的 20% 条目
            remove_count = len(self._l1_cache) - int(self._l1_max_size * 0.8)
            for key, _ in sorted_items[:remove_count]:
                self._l1_cache.pop(key, None)
            expired_keys.extend([k for k, _ in sorted_items[:remove_count]])

        if expired_keys:
            logger.debug(
                f"L1 缓存清理: 移除 {len(expired_keys)} 个条目，当前 {len(self._l1_cache)} 个"
            )

        return len(expired_keys)

    @asynccontextmanager
    async def _acquire_request_lock(self, cache_key: str) -> None:
        lock = self._request_locks.get(cache_key)
        if lock is None:
            lock = asyncio.Lock()
            self._request_locks[cache_key] = lock
        await lock.acquire()
        try:
            yield
        finally:
            lock.release()

    async def _load_affinity_dict(self, cache_key: str) -> dict[str, Any] | None:
        """读取缓存亲和性字典"""
        # 先尝试L1缓存
        l1_value = await self._get_l1_entry(cache_key)
        if l1_value is not None:
            return l1_value

        if not self._is_memory_backend():
            data = await self.redis.get(cache_key)
            if not data:
                return None
            value = json.loads(data)
            await self._set_l1_entry(cache_key, value)
            return value

        lock = self._get_memory_lock()
        async with lock:
            record = self._memory_store.get(cache_key)
            if record:
                await self._set_l1_entry(cache_key, record)
            return dict(record) if record else None

    async def _save_affinity_dict(
        self, cache_key: str, ttl: int, affinity_dict: dict[str, Any]
    ) -> None:
        """存储缓存亲和性字典"""
        if not self._is_memory_backend():
            await self.redis.setex(cache_key, ttl, json.dumps(affinity_dict))
            await self._set_l1_entry(cache_key, affinity_dict)
            return

        lock = self._get_memory_lock()
        async with lock:
            self._memory_store[cache_key] = dict(affinity_dict)
        await self._set_l1_entry(cache_key, affinity_dict)

    async def _delete_affinity_key(self, cache_key: str) -> None:
        """删除缓存亲和性"""
        if not self._is_memory_backend():
            await self.redis.delete(cache_key)
        else:
            lock = self._get_memory_lock()
            async with lock:
                self._memory_store.pop(cache_key, None)

        await self._set_l1_entry(cache_key, None)

    async def _snapshot_memory_items(self) -> dict[str, dict[str, Any]]:
        """复制内存存储内容（仅内存模式使用）"""
        lock = self._get_memory_lock()
        async with lock:
            return {k: dict(v) for k, v in self._memory_store.items()}

    async def get_affinity(
        self, affinity_key: str, api_format: str, model_name: str
    ) -> CacheAffinity | None:
        """
        获取指定亲和性标识符对特定API格式和模型的缓存亲和性

        Args:
            affinity_key: 亲和性标识符（通常为API Key ID）
            api_format: API格式 (claude/openai)
            model_name: 模型名称

        Returns:
            CacheAffinity对象，如果不存在或已过期则返回None
        """
        try:
            cache_key = self._get_cache_key(affinity_key, api_format, model_name)
            async with self._acquire_request_lock(cache_key):
                affinity_dict = await self._load_affinity_dict(cache_key)

                if not affinity_dict:
                    self._stats["cache_misses"] += 1
                    return None

                # 检查是否过期（双重检查，防止TTL未及时清理）
                current_time = time.time()
                if current_time > affinity_dict["expire_at"]:
                    await self._delete_affinity_key(cache_key)
                    self._stats["cache_misses"] += 1
                    return None

                self._stats["cache_hits"] += 1

                return CacheAffinity(
                    provider_id=affinity_dict["provider_id"],
                    endpoint_id=affinity_dict["endpoint_id"],
                    key_id=affinity_dict["key_id"],
                    api_format=affinity_dict.get("api_format", api_format),
                    model_name=affinity_dict.get("model_name", model_name),
                    created_at=affinity_dict["created_at"],
                    expire_at=affinity_dict["expire_at"],
                    request_count=affinity_dict["request_count"],
                )

        except Exception as e:
            logger.exception(f"获取缓存亲和性失败: {e}")
            self._stats["cache_misses"] += 1
            return None

    async def set_affinity(
        self,
        affinity_key: str,
        provider_id: str,
        endpoint_id: str,
        key_id: str,
        api_format: str,
        model_name: str,
        supports_caching: bool = True,
        ttl: int | None = None,
    ) -> None:
        """
        设置指定亲和性标识符对特定API格式和模型的缓存亲和性

        Args:
            affinity_key: 亲和性标识符（通常为API Key ID）
            provider_id: Provider ID
            endpoint_id: Endpoint ID
            key_id: Key ID
            api_format: API格式 (claude/openai)
            model_name: 模型名称
            supports_caching: 该Provider是否支持缓存
            ttl: 缓存有效期（秒），如果不提供则使用默认值

        注意：每次调用都会刷新过期时间（滑动窗口机制），以保持对同一个Provider/Endpoint/Key的亲和性
        """
        if not supports_caching:
            # 不支持缓存的Provider不记录亲和性
            logger.debug(f"Provider {provider_id[:8]}... 不支持缓存，跳过亲和性记录")
            return

        ttl = ttl or self.default_ttl
        current_time = time.time()
        expire_at = current_time + ttl  # 每次都刷新过期时间
        cache_key = self._get_cache_key(affinity_key, api_format, model_name)

        try:
            async with self._acquire_request_lock(cache_key):
                existing_dict = await self._load_affinity_dict(cache_key)
                existing_affinity: CacheAffinity | None = None
                if existing_dict and current_time <= existing_dict.get("expire_at", 0):
                    existing_affinity = CacheAffinity(
                        provider_id=existing_dict["provider_id"],
                        endpoint_id=existing_dict["endpoint_id"],
                        key_id=existing_dict["key_id"],
                        api_format=existing_dict.get("api_format", api_format),
                        model_name=existing_dict.get("model_name", model_name),
                        created_at=existing_dict["created_at"],
                        expire_at=existing_dict["expire_at"],
                        request_count=existing_dict.get("request_count", 0),
                    )

                if existing_affinity:
                    created_at = existing_affinity.created_at
                    request_count = existing_affinity.request_count + 1

                    # 检查是否切换了 Provider/Endpoint/Key
                    if (
                        existing_affinity.provider_id != provider_id
                        or existing_affinity.endpoint_id != endpoint_id
                        or existing_affinity.key_id != key_id
                    ):
                        self._stats["key_switches"] += 1
                        logger.debug(
                            f"Key {affinity_key[:8]}... 在 {api_format} 格式下切换后端: "
                            f"[{existing_affinity.provider_id[:8]}.../{existing_affinity.endpoint_id[:8]}.../"
                            f"{existing_affinity.key_id[:8]}...] → "
                            f"[{provider_id[:8]}.../{endpoint_id[:8]}.../{key_id[:8]}...], 重置计数器"
                        )
                        created_at = current_time
                        request_count = 1
                    else:
                        logger.debug(
                            f"刷新缓存亲和性: key={affinity_key[:8]}..., api_format={api_format}, "
                            f"provider={provider_id[:8]}..., endpoint={endpoint_id[:8]}..., "
                            f"provider_key={key_id[:8]}..., ttl+={ttl}s"
                        )
                else:
                    created_at = current_time
                    request_count = 1
                    self._stats["total_affinities"] += 1

                affinity_dict = {
                    "provider_id": provider_id,
                    "endpoint_id": endpoint_id,
                    "key_id": key_id,
                    "api_format": api_format,
                    "model_name": model_name,
                    "created_at": created_at,
                    "expire_at": expire_at,
                    "request_count": request_count,
                }

                await self._save_affinity_dict(cache_key, ttl, affinity_dict)

            logger.debug(
                f"设置缓存亲和性: key={affinity_key[:8]}..., api_format={api_format}, "
                f"model={model_name}, provider={provider_id[:8]}..., endpoint={endpoint_id[:8]}..., "
                f"provider_key={key_id[:8]}..., ttl={ttl}s"
            )
        except Exception as e:
            logger.exception(f"设置缓存亲和性失败: {e}")

    async def invalidate_affinity(
        self,
        affinity_key: str,
        api_format: str,
        model_name: str,
        key_id: str | None = None,
        provider_id: str | None = None,
        endpoint_id: str | None = None,
    ) -> None:
        """
        失效指定亲和性标识符对特定API格式和模型的缓存亲和性

        Args:
            affinity_key: 亲和性标识符（通常为API Key ID）
            api_format: API格式 (claude/openai)
            model_name: 模型名称
            key_id: Provider Key ID（可选，如果提供则只在Key匹配时失效）
            provider_id: Provider ID（可选，如果提供则只在Provider匹配时失效）
            endpoint_id: Endpoint ID（可选，如果提供则只在Endpoint匹配时失效）
        """
        existing_affinity = await self.get_affinity(affinity_key, api_format, model_name)

        if not existing_affinity:
            return

        # 检查是否匹配过滤条件
        should_invalidate = True

        if key_id and existing_affinity.key_id != key_id:
            should_invalidate = False

        if provider_id and existing_affinity.provider_id != provider_id:
            should_invalidate = False

        if endpoint_id and existing_affinity.endpoint_id != endpoint_id:
            should_invalidate = False

        if not should_invalidate:
            logger.debug(
                f"跳过失效: affinity_key={affinity_key[:8]}..., api_format={api_format}, "
                f"model={model_name}, 过滤条件不匹配 (key={key_id}, provider={provider_id}, endpoint={endpoint_id})"
            )
            return

        try:
            cache_key = self._get_cache_key(affinity_key, api_format, model_name)
            async with self._acquire_request_lock(cache_key):
                await self._delete_affinity_key(cache_key)

            self._stats["cache_invalidations"] += 1

            logger.debug(
                f"失效缓存亲和性: affinity_key={affinity_key[:8]}..., api_format={api_format}, "
                f"model={model_name}, provider={existing_affinity.provider_id[:8]}..., "
                f"endpoint={existing_affinity.endpoint_id[:8]}..., "
                f"provider_key={existing_affinity.key_id[:8]}..."
            )
        except Exception as e:
            logger.exception(f"删除缓存亲和性失败: {e}")

    async def invalidate_all_for_provider(self, provider_id: str) -> int:
        """
        失效所有与指定Provider相关的缓存亲和性

        用途：当Provider关闭缓存支持时调用

        Args:
            provider_id: Provider ID

        Returns:
            失效的亲和性数量
        """
        try:
            invalidated_count = 0

            if not self._is_memory_backend():
                pattern = "cache_affinity:*"
                keys = await self.redis.keys(pattern)
            else:
                keys = list((await self._snapshot_memory_items()).keys())

            for key in keys:
                affinity_dict = await self._load_affinity_dict(key)
                if not affinity_dict:
                    continue

                if affinity_dict.get("provider_id") == provider_id:
                    await self._delete_affinity_key(key)
                    invalidated_count += 1
                    self._stats["cache_invalidations"] += 1

            if invalidated_count > 0:
                logger.debug(
                    f"批量失效Provider缓存亲和性: provider={provider_id[:8]}..., "
                    f"失效数量={invalidated_count}"
                )

            return invalidated_count

        except Exception as e:
            logger.exception(f"批量失效Provider缓存亲和性失败: {e}")
            return 0

    async def clear_all(self) -> int:
        """
        清除所有缓存亲和性（管理功能）

        Returns:
            清除的数量
        """
        try:
            if not self._is_memory_backend():
                keys = await self.redis.keys("cache_affinity:*")
                if keys:
                    await self.redis.delete(*keys)
                    logger.debug(f"清除所有Redis缓存亲和性: {len(keys)} 个")
                    return len(keys)
                return 0

            lock = self._get_memory_lock()
            async with lock:
                count = len(self._memory_store)
                self._memory_store.clear()
            if count:
                logger.debug(f"清除所有内存缓存亲和性: {count} 个")
            return count
        except Exception as e:
            logger.exception(f"清除缓存亲和性失败: {e}")
            return 0

    def get_stats(self) -> dict[str, Any]:
        """获取统计信息"""
        cache_hit_rate = 0.0
        total_requests = self._stats["cache_hits"] + self._stats["cache_misses"]

        if total_requests > 0:
            cache_hit_rate = self._stats["cache_hits"] / total_requests

        storage_type = "redis" if not self._is_memory_backend() else "memory"

        return {
            "storage_type": storage_type,
            "total_affinities": self._stats["total_affinities"],
            "cache_hits": self._stats["cache_hits"],
            "cache_misses": self._stats["cache_misses"],
            "cache_hit_rate": cache_hit_rate,
            "cache_invalidations": self._stats["cache_invalidations"],
            "provider_switches": self._stats["provider_switches"],
            "key_switches": self._stats["key_switches"],
            "config": {
                "default_ttl": self.default_ttl,
            },
        }

    async def list_affinities(self) -> list[dict[str, Any]]:
        """获取所有缓存亲和性列表

        返回的每条记录包含：
        - affinity_key: 亲和性标识符（通常是 API Key ID）
        - provider_id, endpoint_id, key_id: Provider 相关信息
        - api_format, model_name: API 格式和模型名称
        - created_at, expire_at, request_count: 缓存元数据
        """
        results: list[dict[str, Any]] = []

        try:
            pattern = "cache_affinity:*"
            cursor = 0

            if not self._is_memory_backend():
                while True:
                    cursor, keys = await self.redis.scan(cursor=cursor, match=pattern, count=200)

                    if keys:
                        values = await self.redis.mget(*keys)
                        for cache_key, data in zip(keys, values):
                            if not data:
                                continue

                            try:
                                affinity = json.loads(data)
                                # 解析 cache_affinity:{affinity_key}:{api_format}:{model_name}
                                parts = cache_key.split(":")
                                affinity_key_value = parts[1] if len(parts) > 1 else cache_key
                                api_format = (
                                    parts[2]
                                    if len(parts) > 2
                                    else affinity.get("api_format", "unknown")
                                )
                                model_name = (
                                    parts[3]
                                    if len(parts) > 3
                                    else affinity.get("model_name", "unknown")
                                )

                                affinity["affinity_key"] = affinity_key_value
                                if "api_format" not in affinity:
                                    affinity["api_format"] = api_format
                                if "model_name" not in affinity:
                                    affinity["model_name"] = model_name
                                results.append(affinity)
                            except json.JSONDecodeError as e:
                                logger.exception(f"解析缓存亲和性记录失败: {cache_key} - {e}")

                    if cursor == 0:
                        break
            else:
                snapshot = await self._snapshot_memory_items()
                expired_keys: list[str] = []
                current_time = time.time()

                for cache_key, affinity in snapshot.items():
                    if current_time > affinity["expire_at"]:
                        expired_keys.append(cache_key)
                        continue

                    # 解析 cache_affinity:{affinity_key}:{api_format}:{model_name}
                    parts = cache_key.split(":")
                    affinity_key_value = parts[1] if len(parts) > 1 else cache_key
                    api_format = (
                        parts[2] if len(parts) > 2 else affinity.get("api_format", "unknown")
                    )
                    model_name = (
                        parts[3] if len(parts) > 3 else affinity.get("model_name", "unknown")
                    )

                    affinity_with_key = dict(affinity)
                    affinity_with_key["affinity_key"] = affinity_key_value
                    if "api_format" not in affinity_with_key:
                        affinity_with_key["api_format"] = api_format
                    if "model_name" not in affinity_with_key:
                        affinity_with_key["model_name"] = model_name
                    results.append(affinity_with_key)

                # 清理过期的键
                if expired_keys:
                    async with self._get_memory_lock():
                        for key in expired_keys:
                            self._memory_store.pop(key, None)

        except Exception as e:
            logger.exception(f"获取缓存亲和性列表失败: {e}")

        return results


# 全局单例
_affinity_manager: CacheAffinityManager | None = None


async def get_affinity_manager(redis_client: Any | None = None) -> CacheAffinityManager:
    """
    获取全局CacheAffinityManager实例（若Redis不可用则降级为内存模式）

    Args:
        redis_client: Redis客户端（可选）

    Returns:
        CacheAffinityManager实例
    """
    global _affinity_manager

    if _affinity_manager is None:
        _affinity_manager = CacheAffinityManager(redis_client)
    elif redis_client and _affinity_manager.redis is None:
        # 当最初使用内存后 Redis 可用时，升级为 Redis 存储
        _affinity_manager = CacheAffinityManager(redis_client)

    return _affinity_manager
