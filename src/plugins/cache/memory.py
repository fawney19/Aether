"""
内存缓存插件
基于Python字典的简单内存缓存实现
"""

from __future__ import annotations

import asyncio
import time
from collections import OrderedDict
from typing import Any

from .base import CachePlugin


class MemoryCachePlugin(CachePlugin):
    """
    内存缓存插件
    使用OrderedDict实现LRU缓存
    """

    def __init__(self, name: str = "memory", config: dict[str, Any] | None = None):
        super().__init__(name, config)
        self._cache: OrderedDict = OrderedDict()
        self._expiry: dict[str, float] = {}
        self._lock = asyncio.Lock()
        self._hits = 0
        self._misses = 0
        self._evictions = 0
        self._cleanup_task = None
        self._cleanup_interval = 60  # 默认值

        # 启动清理任务
        if config is not None:
            self._cleanup_interval = config.get("cleanup_interval", 60)

        try:
            self._start_cleanup_task()
        except Exception:
            pass  # 事件循环尚未启动，将在首次 set 时延迟启动

    def _start_cleanup_task(self) -> None:
        """启动后台清理任务"""

        async def cleanup_loop() -> None:
            while self.enabled:
                await asyncio.sleep(self._cleanup_interval)
                await self._cleanup_expired()

        try:
            loop = asyncio.get_running_loop()
            self._cleanup_task = loop.create_task(cleanup_loop())
        except RuntimeError:
            # 没有运行的事件循环，稍后再启动
            pass

    async def _cleanup_expired(self) -> None:
        """清理过期的缓存项"""
        now = time.time()
        expired_keys = []

        async with self._lock:
            for key, expiry in self._expiry.items():
                if expiry < now:
                    expired_keys.append(key)

            for key in expired_keys:
                self._cache.pop(key, None)
                self._expiry.pop(key, None)
                self._evictions += 1

    def _check_size(self) -> None:
        """检查并维护缓存大小限制"""
        if len(self._cache) >= self.max_size:
            # 删除最老的项（LRU）
            key = next(iter(self._cache))
            self._cache.pop(key)
            self._expiry.pop(key, None)
            self._evictions += 1

    async def get(self, key: str) -> Any | None:
        """获取缓存值"""
        async with self._lock:
            # 检查是否过期
            if key in self._expiry:
                if self._expiry[key] < time.time():
                    # 已过期，删除
                    self._cache.pop(key, None)
                    self._expiry.pop(key)
                    self._misses += 1
                    return None

            # 获取值并更新访问顺序（LRU）
            if key in self._cache:
                value = self._cache.pop(key)
                self._cache[key] = value  # 移到末尾
                self._hits += 1
                return self.deserialize(value) if isinstance(value, str) else value
            else:
                self._misses += 1
                return None

    def _ensure_cleanup_task(self) -> None:
        """确保清理任务已启动（延迟启动，在事件循环可用后调用）"""
        if self._cleanup_task is None or self._cleanup_task.done():
            try:
                self._start_cleanup_task()
            except Exception:
                pass

    async def set(self, key: str, value: Any, ttl: int | None = None) -> bool:
        """设置缓存值"""
        self._ensure_cleanup_task()
        async with self._lock:
            # 检查大小限制
            if key not in self._cache:
                self._check_size()

            # 序列化值
            if not isinstance(value, str):
                value = self.serialize(value)

            # 设置值
            self._cache[key] = value
            self._cache.move_to_end(key)  # 移到末尾（最新）

            # 设置过期时间
            if ttl is None:
                ttl = self.default_ttl
            if ttl > 0:
                self._expiry[key] = time.time() + ttl

            return True

    async def delete(self, key: str) -> bool:
        """删除缓存项"""
        async with self._lock:
            if key in self._cache:
                self._cache.pop(key)
                self._expiry.pop(key, None)
                return True
            return False

    async def exists(self, key: str) -> bool:
        """检查缓存项是否存在"""
        async with self._lock:
            # 检查是否过期
            if key in self._expiry:
                if self._expiry[key] < time.time():
                    # 已过期，删除
                    self._cache.pop(key, None)
                    self._expiry.pop(key)
                    return False
            return key in self._cache

    async def clear(self) -> bool:
        """清空所有缓存"""
        async with self._lock:
            self._cache.clear()
            self._expiry.clear()
            return True

    async def get_many(self, keys: list[str]) -> dict[str, Any]:
        """批量获取缓存值"""
        result = {}
        for key in keys:
            value = await self.get(key)
            if value is not None:
                result[key] = value
        return result

    async def set_many(self, items: dict[str, Any], ttl: int | None = None) -> bool:
        """批量设置缓存值"""
        success = True
        for key, value in items.items():
            if not await self.set(key, value, ttl):
                success = False
        return success

    async def get_stats(self) -> dict[str, Any]:
        """获取缓存统计信息"""
        total_requests = self._hits + self._misses
        hit_rate = self._hits / total_requests if total_requests > 0 else 0

        return {
            "type": "memory",
            "size": len(self._cache),
            "max_size": self.max_size,
            "hits": self._hits,
            "misses": self._misses,
            "hit_rate": hit_rate,
            "evictions": self._evictions,
            "cleanup_interval": self._cleanup_interval,
        }

    async def _do_shutdown(self) -> None:
        """清理资源"""
        # 取消清理任务
        if self._cleanup_task:
            self._cleanup_task.cancel()
            try:
                await self._cleanup_task
            except asyncio.CancelledError:
                pass

    def __del__(self) -> None:
        """清理资源"""
        if hasattr(self, "_cleanup_task") and self._cleanup_task:
            self._cleanup_task.cancel()
