"""
用户/API Key RPM 限制器

支持两层叠加限流：
1. 用户级（或独立 Key 级）总 RPM
2. 普通 Key 子限制 RPM

实现策略：
- Redis 可用时使用分钟桶 + Lua 脚本原子检查/消费
- Redis 不可用时降级为内存计数（仅适用于单实例）
"""

from __future__ import annotations

import asyncio
import time
from dataclasses import dataclass
from datetime import datetime, timezone

import redis.asyncio as aioredis

from src.config.settings import config
from src.core.logger import logger


@dataclass(slots=True)
class RpmCheckResult:
    """RPM 检查结果。"""

    allowed: bool
    scope: str | None = None
    limit: int | None = None
    remaining: int | None = None
    retry_after: int | None = None


class UserRpmLimiter:
    """用户/API Key 双层 RPM 限制器。"""

    _instance: UserRpmLimiter | None = None
    _redis: aioredis.Redis | None = None

    _CHECK_AND_CONSUME_SCRIPT = """
    local user_key = KEYS[1]
    local key_key = KEYS[2]
    local user_limit = tonumber(ARGV[1])
    local key_limit = tonumber(ARGV[2])
    local ttl = tonumber(ARGV[3])
    local retry_after = tonumber(ARGV[4])

    local user_count = 0
    if user_limit > 0 then
        user_count = tonumber(redis.call('GET', user_key) or '0')
        if user_count >= user_limit then
            return {0, 1, user_limit, 0, retry_after}
        end
    end

    local key_count = 0
    if key_limit > 0 then
        key_count = tonumber(redis.call('GET', key_key) or '0')
        if key_count >= key_limit then
            return {0, 2, key_limit, 0, retry_after}
        end
    end

    local remaining = -1
    if user_limit > 0 then
        user_count = redis.call('INCR', user_key)
        redis.call('EXPIRE', user_key, ttl)
        remaining = user_limit - user_count
    end

    if key_limit > 0 then
        key_count = redis.call('INCR', key_key)
        redis.call('EXPIRE', key_key, ttl)
        local key_remaining = key_limit - key_count
        if remaining == -1 or key_remaining < remaining then
            remaining = key_remaining
        end
    end

    return {1, 0, 0, remaining, 0}
    """

    def __new__(cls) -> "UserRpmLimiter":
        if cls._instance is None:
            cls._instance = super().__new__(cls)
        return cls._instance

    def __init__(self) -> None:
        if getattr(self, "_initialized", False):
            return

        self._bucket_seconds = int(config.rpm_bucket_seconds)
        self._key_ttl_seconds = int(config.rpm_key_ttl_seconds)
        self._cleanup_interval_seconds = int(config.rpm_cleanup_interval_seconds)
        self._memory_lock: asyncio.Lock = asyncio.Lock()
        self._memory_counts: dict[str, tuple[int, int]] = {}
        self._cleanup_task: asyncio.Task | None = None
        self._initialized = True

    async def initialize(self) -> None:
        if self._redis is not None:
            return

        try:
            from src.clients.redis_client import get_redis_client

            self._redis = await get_redis_client(require_redis=False)
            if self._redis:
                logger.info("[OK] UserRpmLimiter 已复用全局 Redis 客户端")
                return
        except Exception as exc:
            logger.warning("初始化 UserRpmLimiter Redis 客户端失败，降级为内存模式: {}", exc)

        self._redis = None
        self._start_background_cleanup()

    async def close(self) -> None:
        if self._cleanup_task is not None:
            self._cleanup_task.cancel()
            try:
                await self._cleanup_task
            except asyncio.CancelledError:
                pass
            self._cleanup_task = None

    @property
    def bucket_seconds(self) -> int:
        return self._bucket_seconds

    def get_user_rpm_key(self, user_id: str, bucket: int | None = None) -> str:
        b = bucket if bucket is not None else self._get_rpm_bucket()
        return f"rpm:user:{user_id}:{b}"

    def get_standalone_rpm_key(self, api_key_id: str, bucket: int | None = None) -> str:
        b = bucket if bucket is not None else self._get_rpm_bucket()
        return f"rpm:ukey:{api_key_id}:{b}"

    def get_key_rpm_key(self, api_key_id: str, bucket: int | None = None) -> str:
        b = bucket if bucket is not None else self._get_rpm_bucket()
        return f"rpm:key:{api_key_id}:{b}"

    def get_retry_after(self, now_ts: float | None = None) -> int:
        ts = now_ts if now_ts is not None else time.time()
        elapsed = int(ts % self._bucket_seconds)
        return max(1, self._bucket_seconds - elapsed)

    def get_reset_at(self, now_ts: float | None = None) -> datetime:
        ts = now_ts if now_ts is not None else time.time()
        bucket = self._get_rpm_bucket(ts)
        reset_ts = (bucket + 1) * self._bucket_seconds
        return datetime.fromtimestamp(reset_ts, tz=timezone.utc)

    async def get_scope_count(self, scope_key: str) -> int:
        await self.initialize()

        if self._redis is None:
            async with self._memory_lock:
                bucket = self._get_rpm_bucket()
                self._cleanup_expired_memory_counts(bucket)
                return self._get_memory_count(scope_key)

        try:
            result = await self._redis.get(scope_key)
            return int(result) if result else 0
        except Exception as exc:
            logger.warning("读取 RPM 计数失败，回退内存模式: {}", exc)
            if config.rate_limit_fail_open:
                return 0
            async with self._memory_lock:
                bucket = self._get_rpm_bucket()
                self._cleanup_expired_memory_counts(bucket)
                return self._get_memory_count(scope_key)

    async def check_and_consume(
        self,
        *,
        user_rpm_key: str,
        user_rpm_limit: int,
        key_rpm_key: str,
        key_rpm_limit: int,
    ) -> RpmCheckResult:
        """原子检查并消费两层 RPM 配额。"""

        await self.initialize()

        normalized_user_limit = max(int(user_rpm_limit or 0), 0)
        normalized_key_limit = max(int(key_rpm_limit or 0), 0)

        if normalized_user_limit <= 0 and normalized_key_limit <= 0:
            return RpmCheckResult(allowed=True)

        if self._redis is None:
            return await self._check_and_consume_memory(
                user_rpm_key=user_rpm_key,
                user_rpm_limit=normalized_user_limit,
                key_rpm_key=key_rpm_key,
                key_rpm_limit=normalized_key_limit,
            )

        retry_after = self.get_retry_after()

        try:
            raw_result = await self._redis.eval(
                self._CHECK_AND_CONSUME_SCRIPT,
                2,
                user_rpm_key,
                key_rpm_key,
                normalized_user_limit,
                normalized_key_limit,
                self._key_ttl_seconds,
                retry_after,
            )
            return self._parse_redis_result(raw_result)
        except Exception as exc:
            logger.warning("Redis RPM 检查失败: {}", exc)
            if config.rate_limit_fail_open:
                return RpmCheckResult(allowed=True)
            return await self._check_and_consume_memory(
                user_rpm_key=user_rpm_key,
                user_rpm_limit=normalized_user_limit,
                key_rpm_key=key_rpm_key,
                key_rpm_limit=normalized_key_limit,
            )

    def _parse_redis_result(self, raw_result: object) -> RpmCheckResult:
        values = list(raw_result) if isinstance(raw_result, (list, tuple)) else [raw_result]
        allowed = int(values[0]) == 1
        scope_code = int(values[1]) if len(values) > 1 else 0
        limit = int(values[2]) if len(values) > 2 and values[2] else None
        remaining = int(values[3]) if len(values) > 3 and values[3] is not None else None
        retry_after = int(values[4]) if len(values) > 4 and values[4] else None
        scope = {1: "user", 2: "key"}.get(scope_code)
        return RpmCheckResult(
            allowed=allowed,
            scope=scope,
            limit=limit,
            remaining=remaining,
            retry_after=retry_after,
        )

    async def _check_and_consume_memory(
        self,
        *,
        user_rpm_key: str,
        user_rpm_limit: int,
        key_rpm_key: str,
        key_rpm_limit: int,
    ) -> RpmCheckResult:
        async with self._memory_lock:
            bucket = self._get_rpm_bucket()
            self._cleanup_expired_memory_counts(bucket)

            user_count = self._get_memory_count(user_rpm_key)
            if user_rpm_limit > 0 and user_count >= user_rpm_limit:
                return RpmCheckResult(
                    allowed=False,
                    scope="user",
                    limit=user_rpm_limit,
                    remaining=0,
                    retry_after=self.get_retry_after(),
                )

            key_count = self._get_memory_count(key_rpm_key)
            if key_rpm_limit > 0 and key_count >= key_rpm_limit:
                return RpmCheckResult(
                    allowed=False,
                    scope="key",
                    limit=key_rpm_limit,
                    remaining=0,
                    retry_after=self.get_retry_after(),
                )

            remaining_candidates: list[int] = []

            if user_rpm_limit > 0:
                user_count += 1
                self._set_memory_count(user_rpm_key, user_count)
                remaining_candidates.append(user_rpm_limit - user_count)

            if key_rpm_limit > 0:
                key_count += 1
                self._set_memory_count(key_rpm_key, key_count)
                remaining_candidates.append(key_rpm_limit - key_count)

            remaining = min(remaining_candidates) if remaining_candidates else None
            return RpmCheckResult(allowed=True, remaining=remaining)

    def _start_background_cleanup(self) -> None:
        if self._cleanup_task is not None:
            return

        async def cleanup_loop() -> None:
            while True:
                try:
                    await asyncio.sleep(self._bucket_seconds)
                    async with self._memory_lock:
                        self._cleanup_expired_memory_counts(self._get_rpm_bucket())
                except asyncio.CancelledError:
                    break
                except Exception as exc:
                    logger.debug("UserRpmLimiter 后台清理异常: {}", exc)

        try:
            self._cleanup_task = asyncio.create_task(cleanup_loop())
        except RuntimeError:
            self._cleanup_task = None

    def _get_rpm_bucket(self, now_ts: float | None = None) -> int:
        ts = now_ts if now_ts is not None else time.time()
        return int(ts // self._bucket_seconds)

    def _split_scope_key(self, scope_key: str) -> tuple[str, int]:
        base_key, bucket_str = scope_key.rsplit(":", 1)
        return base_key, int(bucket_str)

    def _get_memory_count(self, scope_key: str) -> int:
        base_key, bucket = self._split_scope_key(scope_key)
        stored = self._memory_counts.get(base_key)
        if not stored:
            return 0
        stored_bucket, count = stored
        if stored_bucket != bucket:
            self._memory_counts.pop(base_key, None)
            return 0
        return count

    def _set_memory_count(self, scope_key: str, count: int) -> None:
        base_key, bucket = self._split_scope_key(scope_key)
        self._memory_counts[base_key] = (bucket, count)

    def _cleanup_expired_memory_counts(self, current_bucket: int) -> None:
        now = time.time()
        expired_keys = [
            base_key
            for base_key, (bucket, _count) in self._memory_counts.items()
            if bucket < current_bucket
        ]
        for base_key in expired_keys:
            self._memory_counts.pop(base_key, None)

        if expired_keys:
            logger.debug(
                "[CLEANUP] 清理了 {} 个过期的用户/API Key RPM 计数（interval={}s）",
                len(expired_keys),
                self._cleanup_interval_seconds,
            )


_user_rpm_limiter: UserRpmLimiter | None = None


async def get_user_rpm_limiter() -> UserRpmLimiter:
    global _user_rpm_limiter
    if _user_rpm_limiter is None:
        _user_rpm_limiter = UserRpmLimiter()
        await _user_rpm_limiter.initialize()
    return _user_rpm_limiter
