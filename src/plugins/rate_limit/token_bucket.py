"""令牌桶速率限制策略，支持 Redis 分布式后端"""

from __future__ import annotations

import asyncio
import os
import time
from datetime import datetime, timedelta, timezone
from typing import Any

from src.core.logger import logger

from ...clients.redis_client import get_redis_client_sync
from .base import RateLimitResult, RateLimitStrategy


class TokenBucket:
    """令牌桶实现"""

    def __init__(self, capacity: int, refill_rate: float):
        """
        初始化令牌桶

        Args:
            capacity: 桶容量（最大令牌数）
            refill_rate: 令牌补充速率（每秒）
        """
        self.capacity = capacity
        self.refill_rate = refill_rate
        self.tokens = capacity
        self.last_refill = time.time()
        self.last_access_time = self.last_refill

    def _refill(self) -> None:
        """补充令牌"""
        now = time.time()
        time_passed = now - self.last_refill
        tokens_to_add = time_passed * self.refill_rate

        self.last_access_time = now
        if tokens_to_add > 0:
            self.tokens = min(self.capacity, self.tokens + tokens_to_add)
            self.last_refill = now

    def consume(self, amount: int = 1) -> bool:
        """
        消费令牌

        Args:
            amount: 要消费的令牌数

        Returns:
            是否成功消费
        """
        self._refill()

        if self.tokens >= amount:
            self.tokens -= amount
            return True
        return False

    def get_remaining(self) -> int:
        """获取剩余令牌数"""
        self._refill()
        return int(self.tokens)

    def get_reset_time(self) -> datetime:
        """获取下次完全恢复的时间"""
        if self.tokens >= self.capacity:
            return datetime.now(timezone.utc)

        tokens_needed = self.capacity - self.tokens
        seconds_to_full = tokens_needed / self.refill_rate
        return datetime.now(timezone.utc) + timedelta(seconds=seconds_to_full)


class TokenBucketStrategy(RateLimitStrategy):
    """
    令牌桶算法速率限制策略

    特点：
    - 允许突发流量
    - 平均速率受限
    - 适合处理不均匀的流量模式
    """

    DEFAULT_MAX_BUCKETS = 10000
    DEFAULT_BUCKET_EXPIRY = 3600

    def __init__(self) -> None:
        super().__init__("token_bucket")
        self.buckets: dict[str, TokenBucket] = {}
        self._lock = asyncio.Lock()

        # 默认配置
        self.default_capacity = 100  # 默认桶容量
        self.default_refill_rate = 10  # 默认每秒补充10个令牌
        self.max_buckets = self.DEFAULT_MAX_BUCKETS
        self.bucket_expiry = self.DEFAULT_BUCKET_EXPIRY
        self._last_cleanup_time: float = time.time()
        self._cleanup_interval = 300  # 每 5 分钟检查一次清理

        # 可选的 Redis 后端
        self._redis_backend: RedisTokenBucketBackend | None = None
        self._redis_checked = False
        self._backend_mode = os.getenv("RATE_LIMIT_BACKEND", "auto").lower()

    def _resolve_bucket_config(self, key: str, rate_limit: int | None = None) -> tuple[int, float]:
        """解析指定 key 当前应使用的桶容量和补充速率。"""
        if rate_limit is not None:
            return rate_limit, rate_limit / 60.0
        if key.startswith("api_key:"):
            return (
                self.config.get("api_key_capacity", self.default_capacity),
                self.config.get("api_key_refill_rate", self.default_refill_rate),
            )
        if key.startswith("user:"):
            return (
                self.config.get("user_capacity", self.default_capacity * 2),
                self.config.get("user_refill_rate", self.default_refill_rate * 2),
            )
        return self.default_capacity, self.default_refill_rate

    def _get_bucket(self, key: str, rate_limit: int | None = None) -> TokenBucket:
        """
        获取或创建令牌桶

        Args:
            key: 限制键
            rate_limit: 每分钟请求限制（来自数据库配置），如果提供则使用此值

        Returns:
            令牌桶实例
        """
        capacity, refill_rate = self._resolve_bucket_config(key, rate_limit)
        bucket = self.buckets.get(key)
        if bucket is None:
            bucket = TokenBucket(capacity, refill_rate)
            self.buckets[key] = bucket
            return bucket

        if bucket.capacity != capacity or bucket.refill_rate != refill_rate:
            bucket._refill()
            bucket.capacity = capacity
            bucket.refill_rate = refill_rate
            bucket.tokens = min(bucket.tokens, capacity)

        return bucket

    def _cleanup_expired_buckets(self) -> int:
        """清理长时间未访问的桶，避免 key 集合无限增长。"""
        current_time = time.time()
        expired_keys = [
            key
            for key, bucket in self.buckets.items()
            if current_time - bucket.last_access_time > self.bucket_expiry
        ]

        for key in expired_keys:
            del self.buckets[key]

        if expired_keys:
            logger.info("清理了 {} 个过期的令牌桶", len(expired_keys))

        return len(expired_keys)

    def _evict_lru_buckets(self, count: int) -> int:
        """达到容量上限时淘汰最久未使用的桶。"""
        if not self.buckets or count <= 0:
            return 0

        sorted_keys = sorted(self.buckets, key=lambda key: self.buckets[key].last_access_time)
        evicted = 0
        for key in sorted_keys[:count]:
            del self.buckets[key]
            evicted += 1

        if evicted:
            logger.warning("LRU 淘汰了 {} 个令牌桶（达到容量上限）", evicted)

        return evicted

    async def _maybe_cleanup(self) -> None:
        """定期清理或淘汰桶，控制进程内桶数量。"""
        current_time = time.time()
        if current_time - self._last_cleanup_time > self._cleanup_interval:
            self._cleanup_expired_buckets()
            self._last_cleanup_time = current_time

        if len(self.buckets) >= self.max_buckets:
            evict_count = max(1, self.max_buckets // 10)
            self._evict_lru_buckets(evict_count)

    def _want_redis_backend(self) -> bool:
        return self._backend_mode in {"auto", "redis"}

    async def _ensure_backend(self) -> None:
        if self._redis_checked:
            return
        self._redis_checked = True
        if not self._want_redis_backend():
            return
        redis_client = get_redis_client_sync()
        if redis_client:
            self._redis_backend = RedisTokenBucketBackend(redis_client)
            logger.info("速率限制改用 Redis 令牌桶后端")
        elif self._backend_mode == "redis":
            logger.warning("RATE_LIMIT_BACKEND=redis 但 Redis 客户端不可用，回退到内存桶")

    async def check_limit(self, key: str, **kwargs: Any) -> RateLimitResult:
        """
        检查速率限制

        Args:
            key: 限制键
            **kwargs: 额外参数，包括 rate_limit (从数据库配置)

        Returns:
            速率限制检查结果
        """
        await self._ensure_backend()

        rate_limit = kwargs.get("rate_limit")
        amount = kwargs.get("amount", 1)

        if self._redis_backend:
            return await self._redis_backend.peek(
                key=key,
                capacity=self._resolve_capacity(key, rate_limit),
                refill_rate=self._resolve_refill_rate(key, rate_limit),
                amount=amount,
            )

        async with self._lock:
            await self._maybe_cleanup()
            bucket = self._get_bucket(key, rate_limit)
            remaining = bucket.get_remaining()
            reset_at = bucket.get_reset_time()

            allowed = remaining >= amount

            retry_after = None
            if not allowed:
                tokens_needed = amount - remaining
                retry_after = int(tokens_needed / bucket.refill_rate) + 1

            return RateLimitResult(
                allowed=allowed,
                remaining=remaining,
                reset_at=reset_at,
                retry_after=retry_after,
                message=(
                    None
                    if allowed
                    else f"Rate limit exceeded. Please retry after {retry_after} seconds."
                ),
            )

    async def consume(self, key: str, amount: int = 1, **kwargs: Any) -> bool:
        """
        消费令牌

        Args:
            key: 限制键
            amount: 消费数量

        Returns:
            是否成功消费
        """
        await self._ensure_backend()

        if self._redis_backend:
            success, remaining = await self._redis_backend.consume(
                key=key,
                capacity=self._resolve_capacity(key, kwargs.get("rate_limit")),
                refill_rate=self._resolve_refill_rate(key, kwargs.get("rate_limit")),
                amount=amount,
            )
            if success:
                logger.debug("Redis 令牌消费成功")
            else:
                logger.warning("Redis 令牌消费失败")
            return success

        async with self._lock:
            await self._maybe_cleanup()
            bucket = self._get_bucket(key, kwargs.get("rate_limit"))
            success = bucket.consume(amount)

            if success:
                logger.debug(f"令牌消费成功")
            else:
                logger.warning(f"令牌消费失败：超出速率限制")

            return success

    async def reset(self, key: str) -> Any:
        """
        重置令牌桶

        Args:
            key: 限制键
        """
        await self._ensure_backend()

        if self._redis_backend:
            await self._redis_backend.reset(key)
            return

        async with self._lock:
            if key in self.buckets:
                bucket = self.buckets[key]
                bucket.tokens = bucket.capacity
                bucket.last_refill = time.time()

                logger.info(f"令牌桶已重置")

    async def get_stats(self, key: str) -> dict[str, Any]:
        """
        获取统计信息

        Args:
            key: 限制键

        Returns:
            统计信息
        """
        await self._ensure_backend()

        if self._redis_backend:
            return await self._redis_backend.get_stats(
                key,
                capacity=self._resolve_capacity(key),
                refill_rate=self._resolve_refill_rate(key),
            )

        async with self._lock:
            await self._maybe_cleanup()
            bucket = self._get_bucket(key)
            return {
                "strategy": "token_bucket",
                "key": key,
                "capacity": bucket.capacity,
                "remaining": bucket.get_remaining(),
                "refill_rate": bucket.refill_rate,
                "reset_at": bucket.get_reset_time().isoformat(),
            }

    def configure(self, config: dict[str, Any]) -> Any:
        """
        配置策略

        支持的配置项：
        - api_key_capacity: API Key的桶容量
        - api_key_refill_rate: API Key的令牌补充速率
        - user_capacity: 用户的桶容量
        - user_refill_rate: 用户的令牌补充速率
        """
        super().configure(config)
        self.default_capacity = config.get("default_capacity", self.default_capacity)
        self.default_refill_rate = config.get("default_refill_rate", self.default_refill_rate)
        self.max_buckets = int(config.get("max_buckets", self.max_buckets))
        self.bucket_expiry = int(config.get("bucket_expiry", self.bucket_expiry))
        self._cleanup_interval = int(config.get("cleanup_interval", self._cleanup_interval))

    def _resolve_capacity(self, key: str, rate_limit: int | None = None) -> int:
        capacity, _ = self._resolve_bucket_config(key, rate_limit)
        return capacity

    def _resolve_refill_rate(self, key: str, rate_limit: int | None = None) -> float:
        _, refill_rate = self._resolve_bucket_config(key, rate_limit)
        return refill_rate


class RedisTokenBucketBackend:
    """使用 Redis 存储令牌桶状态，支持多实例共享"""

    _SCRIPT = """
    local key = KEYS[1]
    local now = tonumber(ARGV[1])
    local capacity = tonumber(ARGV[2])
    local refill_rate = tonumber(ARGV[3])
    local amount = tonumber(ARGV[4])

    local data = redis.call('HMGET', key, 'tokens', 'timestamp')
    local tokens = tonumber(data[1])
    local last_refill = tonumber(data[2])

    if tokens == nil then
        tokens = capacity
        last_refill = now
    end

    local delta = math.max(0, now - last_refill)
    local refill = delta * refill_rate
    tokens = math.min(capacity, tokens + refill)

    local allowed = 0
    local retry_after = 0
    if tokens >= amount then
        tokens = tokens - amount
        allowed = 1
    else
        retry_after = math.ceil((amount - tokens) / refill_rate)
    end

    redis.call('HMSET', key, 'tokens', tokens, 'timestamp', now)
    local ttl = math.max(1, math.ceil(capacity / refill_rate))
    redis.call('EXPIRE', key, ttl)
    return {allowed, tokens, retry_after}
    """

    def __init__(self, redis_client: Any) -> None:
        self.redis = redis_client
        self._consume_script = self.redis.register_script(self._SCRIPT)

    def _redis_key(self, key: str) -> str:
        return f"rate_limit:bucket:{key}"

    async def peek(
        self,
        key: str,
        capacity: int,
        refill_rate: float,
        amount: int,
    ) -> RateLimitResult:
        bucket_key = self._redis_key(key)
        data = await self.redis.hmget(bucket_key, "tokens", "timestamp")
        tokens = data[0]
        last_refill = data[1]

        if tokens is None or last_refill is None:
            remaining = capacity
            reset_at = datetime.now(timezone.utc) + timedelta(seconds=capacity / refill_rate)
        else:
            tokens_value = float(tokens)
            last_refill_value = float(last_refill)
            delta = max(0.0, time.time() - last_refill_value)
            tokens_value = min(capacity, tokens_value + delta * refill_rate)
            remaining = int(tokens_value)
            reset_after = 0 if tokens_value >= capacity else (capacity - tokens_value) / refill_rate
            reset_at = datetime.now(timezone.utc) + timedelta(seconds=reset_after)

        allowed = remaining >= amount
        retry_after = None
        if not allowed:
            needed = max(0, amount - remaining)
            retry_after = int(needed / refill_rate) + 1

        return RateLimitResult(
            allowed=allowed,
            remaining=int(remaining),
            reset_at=reset_at,
            retry_after=retry_after,
            message=(
                None
                if allowed
                else f"Rate limit exceeded. Please retry after {retry_after} seconds."
            ),
        )

    async def consume(
        self,
        key: str,
        capacity: int,
        refill_rate: float,
        amount: int,
    ) -> tuple[bool, int]:
        result = await self._consume_script(
            keys=[self._redis_key(key)],
            args=[time.time(), capacity, refill_rate, amount],
        )
        allowed = bool(result[0])
        remaining = int(float(result[1]))
        return allowed, remaining

    async def reset(self, key: str) -> Any:
        await self.redis.delete(self._redis_key(key))

    async def get_stats(self, key: str, capacity: int, refill_rate: float) -> dict[str, Any]:
        data = await self.redis.hmget(self._redis_key(key), "tokens", "timestamp")
        tokens = data[0]
        timestamp = data[1]
        return {
            "strategy": "token_bucket",
            "key": key,
            "capacity": capacity,
            "remaining": float(tokens) if tokens else capacity,
            "refill_rate": refill_rate,
            "last_refill": float(timestamp) if timestamp else time.time(),
            "backend": "redis",
        }
