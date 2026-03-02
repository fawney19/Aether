"""
账户级登录锁定服务

用于限制单个登录标识（邮箱/用户名）的连续失败次数，防止代理池绕过 IP 限流。
"""

from __future__ import annotations

from src.clients.redis_client import get_redis_client
from src.core.logger import logger


class AccountLockoutService:
    """账户级登录锁定服务。"""

    FAILED_PREFIX = "account:login_failed:"
    LOCKOUT_PREFIX = "account:lockout:"

    MAX_FAILED_ATTEMPTS = 5
    LOCKOUT_DURATION_SECONDS = 900
    FAILED_WINDOW_SECONDS = 300

    @classmethod
    def _failed_key(cls, identifier: str) -> str:
        return f"{cls.FAILED_PREFIX}{identifier}"

    @classmethod
    def _lockout_key(cls, identifier: str) -> str:
        return f"{cls.LOCKOUT_PREFIX}{identifier}"

    @classmethod
    async def is_locked(cls, identifier: str) -> tuple[bool, int]:
        """
        检查登录标识是否被锁定。

        Returns:
            (是否锁定, 剩余锁定秒数)
        """
        if not identifier:
            return False, 0

        redis_client = await get_redis_client(require_redis=False)
        if redis_client is None:
            # fail-open: Redis 不可用时放行
            logger.warning("Redis 不可用，跳过账户锁定检查（降级模式）")
            return False, 0

        try:
            lockout_key = cls._lockout_key(identifier)
            exists = await redis_client.exists(lockout_key)
            if not exists:
                return False, 0

            ttl = await redis_client.ttl(lockout_key)
            remaining = max(0, int(ttl)) if ttl and ttl > 0 else 0
            return True, remaining
        except Exception as exc:
            logger.error(f"检查账户锁定状态失败: {exc}")
            return False, 0

    @classmethod
    async def record_failed_attempt(cls, identifier: str) -> tuple[bool, int]:
        """
        记录一次失败登录尝试。

        Returns:
            (是否触发锁定, 当前失败次数)
        """
        if not identifier:
            return False, 0

        redis_client = await get_redis_client(require_redis=False)
        if redis_client is None:
            # fail-open: Redis 不可用时放行
            logger.warning("Redis 不可用，跳过账户失败计数（降级模式）")
            return False, 0

        try:
            failed_key = cls._failed_key(identifier)
            lockout_key = cls._lockout_key(identifier)

            count = int(await redis_client.incr(failed_key))
            if count == 1:
                await redis_client.expire(failed_key, cls.FAILED_WINDOW_SECONDS)

            if count >= cls.MAX_FAILED_ATTEMPTS:
                await redis_client.setex(lockout_key, cls.LOCKOUT_DURATION_SECONDS, "1")
                await redis_client.delete(failed_key)
                logger.warning(f"账户锁定触发: identifier={identifier}, failed_count={count}")
                return True, count

            return False, count
        except Exception as exc:
            logger.error(f"记录账户失败计数失败: {exc}")
            return False, 0

    @classmethod
    async def reset_failed_attempts(cls, identifier: str) -> None:
        """登录成功后重置失败计数和锁定状态。"""
        if not identifier:
            return

        redis_client = await get_redis_client(require_redis=False)
        if redis_client is None:
            return

        try:
            await redis_client.delete(cls._failed_key(identifier))
            await redis_client.delete(cls._lockout_key(identifier))
        except Exception as exc:
            logger.error(f"重置账户失败计数失败: {exc}")
