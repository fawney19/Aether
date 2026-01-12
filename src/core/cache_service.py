"""
缓存服务 - 统一的缓存抽象层
"""

import json
from datetime import timedelta
from typing import Any, Optional

from src.clients.redis_client import get_redis_client
from src.core.logger import logger



class CacheService:
    """缓存服务"""

    @staticmethod
    async def get(key: str) -> Optional[Any]:
        """
        从缓存获取数据

        Args:
            key: 缓存键

        Returns:
            缓存的值，如果不存在则返回 None
        """
        try:
            redis = await get_redis_client(require_redis=False)
            if not redis:
                return None

            value = await redis.get(key)
            if value:
                # 尝试 JSON 反序列化
                try:
                    return json.loads(value)
                except (json.JSONDecodeError, TypeError):
                    # 非 JSON 值：统一返回字符串，避免上层出现 bytes/str 混用
                    if isinstance(value, (bytes, bytearray)):
                        try:
                            return value.decode("utf-8")
                        except Exception:
                            return value
                    return value

            return None

        except Exception as e:
            logger.warning(f"缓存读取失败: {key} - {e}")
            return None

    @staticmethod
    async def set(key: str, value: Any, ttl_seconds: int = 60) -> bool:
        """
        设置缓存

        Args:
            key: 缓存键
            value: 缓存值
            ttl_seconds: 过期时间（秒），默认 60 秒

        Returns:
            是否设置成功
        """
        try:
            redis = await get_redis_client(require_redis=False)
            if not redis:
                return False

            # JSON 序列化
            if isinstance(value, (dict, list)):
                value = json.dumps(value)
            elif not isinstance(value, (str, bytes)):
                value = str(value)

            await redis.setex(key, ttl_seconds, value)
            return True

        except Exception as e:
            logger.warning(f"缓存写入失败: {key} - {e}")
            return False

    @staticmethod
    async def delete(key: str) -> bool:
        """
        删除缓存

        Args:
            key: 缓存键

        Returns:
            是否删除成功
        """
        try:
            redis = await get_redis_client(require_redis=False)
            if not redis:
                return False

            await redis.delete(key)
            return True

        except Exception as e:
            logger.warning(f"缓存删除失败: {key} - {e}")
            return False

    @staticmethod
    async def delete_pattern(pattern: str, batch_size: int = 100) -> int:
        """
        删除匹配模式的所有缓存

        使用 SCAN 遍历并分批删除，避免阻塞 Redis

        Args:
            pattern: 缓存键模式（支持 * 通配符）
            batch_size: 每批删除的最大键数量

        Returns:
            删除的键数量
        """
        try:
            redis = await get_redis_client(require_redis=False)
            if not redis:
                return 0

            # 使用 SCAN 遍历匹配的键
            deleted_count = 0
            cursor = 0
            while True:
                cursor, keys = await redis.scan(cursor, match=pattern, count=batch_size)
                if keys:
                    # 分批删除，避免单次删除过多键导致 Redis 阻塞
                    for i in range(0, len(keys), batch_size):
                        batch = keys[i : i + batch_size]
                        await redis.delete(*batch)
                        deleted_count += len(batch)
                if cursor == 0:
                    break

            if deleted_count > 0:
                logger.debug(f"缓存模式删除成功: {pattern}, 删除 {deleted_count} 个键")

            return deleted_count

        except Exception as e:
            logger.warning(f"缓存模式删除失败: {pattern} - {e}")
            return 0

    @staticmethod
    async def exists(key: str) -> bool:
        """
        检查缓存是否存在

        Args:
            key: 缓存键

        Returns:
            是否存在
        """
        try:
            redis = await get_redis_client(require_redis=False)
            if not redis:
                return False

            return await redis.exists(key) > 0

        except Exception as e:
            logger.warning(f"缓存检查失败: {key} - {e}")
            return False

    @staticmethod
    async def incr(key: str, ttl_seconds: Optional[int] = None) -> int:
        """
        递增缓存值

        Args:
            key: 缓存键
            ttl_seconds: 可选，如果提供则刷新 TTL

        Returns:
            递增后的值，如果失败返回 0
        """
        try:
            redis = await get_redis_client(require_redis=False)
            if not redis:
                return 0

            result = await redis.incr(key)
            # 如果提供了 TTL，刷新过期时间
            if ttl_seconds is not None:
                await redis.expire(key, ttl_seconds)
            return result

        except Exception as e:
            logger.warning(f"缓存递增失败: {key} - {e}")
            return 0


# 缓存键前缀
class CacheKeys:
    """缓存键定义"""

    # User 缓存（TTL 60秒）
    USER_BY_ID = "user:id:{user_id}"
    USER_BY_EMAIL = "user:email:{email}"

    # API Key 缓存（TTL 30秒）
    APIKEY_HASH = "apikey:hash:{key_hash}"
    APIKEY_AUTH = "apikey:auth:{key_hash}"  # 认证结果缓存

    # Provider 配置缓存（TTL 300秒）
    PROVIDER_BY_ID = "provider:id:{provider_id}"
    ENDPOINT_BY_ID = "endpoint:id:{endpoint_id}"
    API_KEY_BY_ID = "api_key:id:{api_key_id}"

    @staticmethod
    def user_by_id(user_id: str) -> str:
        """User ID 缓存键"""
        return CacheKeys.USER_BY_ID.format(user_id=user_id)

    @staticmethod
    def user_by_email(email: str) -> str:
        """User Email 缓存键"""
        return CacheKeys.USER_BY_EMAIL.format(email=email)

    @staticmethod
    def apikey_hash(key_hash: str) -> str:
        """API Key Hash 缓存键"""
        return CacheKeys.APIKEY_HASH.format(key_hash=key_hash)

    @staticmethod
    def apikey_auth(key_hash: str) -> str:
        """API Key 认证结果缓存键"""
        return CacheKeys.APIKEY_AUTH.format(key_hash=key_hash)

    @staticmethod
    def provider_by_id(provider_id: str) -> str:
        """Provider ID 缓存键"""
        return CacheKeys.PROVIDER_BY_ID.format(provider_id=provider_id)

    @staticmethod
    def endpoint_by_id(endpoint_id: str) -> str:
        """Endpoint ID 缓存键"""
        return CacheKeys.ENDPOINT_BY_ID.format(endpoint_id=endpoint_id)

    @staticmethod
    def api_key_by_id(api_key_id: str) -> str:
        """API Key ID 缓存键"""
        return CacheKeys.API_KEY_BY_ID.format(api_key_id=api_key_id)
