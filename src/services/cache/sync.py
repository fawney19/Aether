"""
缓存同步服务（Redis Pub/Sub）

提供分布式缓存失效同步功能，用于多实例部署场景。
当一个实例修改数据并失效本地缓存时，通过 Redis pub/sub 通知其他实例同步失效。

使用场景：
1. 多实例部署时，确保所有实例的缓存一致性
2. GlobalModel/Model 变更时，同步失效所有实例的缓存
"""

from __future__ import annotations

import asyncio
from collections.abc import Callable
from typing import Any

import redis.asyncio as aioredis

from src.clients.redis_client import get_redis_client_sync
from src.core.logger import logger
from src.utils import json_helper as json


class CacheSyncService:
    """
    缓存同步服务

    通过 Redis pub/sub 实现分布式缓存失效同步
    """

    # Redis 频道名称
    CHANNEL_GLOBAL_MODEL = "cache:invalidate:global_model"
    CHANNEL_MODEL = "cache:invalidate:model"
    CHANNEL_CLEAR_ALL = "cache:invalidate:clear_all"

    def __init__(self, redis_client: aioredis.Redis):
        """
        初始化缓存同步服务

        Args:
            redis_client: Redis 客户端实例
        """
        self._redis = redis_client
        self._pubsub: aioredis.client.PubSub | None = None
        self._listener_task: asyncio.Task | None = None
        self._handlers: dict[str, Callable] = {}
        self._running = False

    async def start(self) -> Any:
        """启动缓存同步服务（订阅 Redis 频道）"""
        if self._running:
            logger.warning("[CacheSync] 服务已在运行")
            return

        try:
            self._pubsub = self._redis.pubsub()

            # 订阅所有缓存失效频道
            await self._pubsub.subscribe(
                self.CHANNEL_GLOBAL_MODEL,
                self.CHANNEL_MODEL,
                self.CHANNEL_CLEAR_ALL,
            )

            # 启动监听任务
            self._listener_task = asyncio.create_task(self._listen())
            self._running = True

            logger.info(
                "[CacheSync] 缓存同步服务已启动，订阅频道: "
                f"{self.CHANNEL_GLOBAL_MODEL}, "
                f"{self.CHANNEL_MODEL}, {self.CHANNEL_CLEAR_ALL}"
            )
        except Exception as e:
            logger.error(f"[CacheSync] 启动失败: {e}")
            raise

    async def stop(self) -> Any:
        """停止缓存同步服务"""
        if not self._running:
            return

        self._running = False

        # 取消监听任务
        if self._listener_task:
            self._listener_task.cancel()
            try:
                await self._listener_task
            except asyncio.CancelledError:
                pass

        # 取消订阅
        if self._pubsub:
            await self._pubsub.unsubscribe()
            await self._pubsub.close()

        logger.info("[CacheSync] 缓存同步服务已停止")

    def register_handler(self, channel: str, handler: Callable) -> None:
        """
        注册缓存失效处理器

        Args:
            channel: Redis 频道名称
            handler: 处理函数（接收消息数据作为参数）
        """
        self._handlers[channel] = handler
        logger.debug(f"[CacheSync] 注册处理器: {channel}")

    async def _listen(self) -> None:
        """监听 Redis pub/sub 消息"""
        logger.info("[CacheSync] 开始监听缓存失效消息")

        try:
            async for message in self._pubsub.listen():
                if message["type"] == "message":
                    channel = message["channel"]
                    data = message["data"]

                    # 解析消息
                    try:
                        payload = json.loads(data)
                        logger.debug(f"[CacheSync] 收到消息: {channel} -> {payload}")

                        # 调用注册的处理器
                        if channel in self._handlers:
                            handler = self._handlers[channel]
                            await handler(payload)
                        else:
                            logger.warning(f"[CacheSync] 未找到处理器: {channel}")
                    except json.JSONDecodeError as e:
                        logger.error(f"[CacheSync] 消息解析失败: {data}, 错误: {e}")
                    except Exception as e:
                        logger.error(f"[CacheSync] 处理消息失败: {channel}, 错误: {e}")
        except asyncio.CancelledError:
            logger.info("[CacheSync] 监听任务已取消")
        except Exception as e:
            logger.error(f"[CacheSync] 监听失败: {e}")

    async def publish_global_model_changed(self, model_name: str) -> Any:
        """发布 GlobalModel 变更通知"""
        await self._publish(self.CHANNEL_GLOBAL_MODEL, {"model_name": model_name})

    async def publish_model_changed(self, provider_id: str, global_model_id: str) -> Any:
        """发布 Model 变更通知"""
        await self._publish(
            self.CHANNEL_MODEL, {"provider_id": provider_id, "global_model_id": global_model_id}
        )

    async def publish_clear_all(self) -> Any:
        """发布清空所有缓存通知"""
        await self._publish(self.CHANNEL_CLEAR_ALL, {})

    async def _publish(self, channel: str, data: dict) -> None:
        """发布消息到 Redis 频道"""
        try:
            message = json.dumps(data)
            await self._redis.publish(channel, message)
            logger.debug(f"[CacheSync] 发布消息: {channel} -> {data}")
        except Exception as e:
            logger.error(f"[CacheSync] 发布消息失败: {channel}, 错误: {e}")


# 全局单例
_cache_sync_service: CacheSyncService | None = None


async def get_cache_sync_service(
    redis_client: aioredis.Redis | None = None,
) -> CacheSyncService | None:
    """
    获取缓存同步服务实例

    Args:
        redis_client: Redis 客户端实例（首次调用时需要提供）

    Returns:
        CacheSyncService 实例，如果 Redis 不可用返回 None
    """
    global _cache_sync_service

    if _cache_sync_service is None:
        if redis_client is None:
            # 尝试获取全局 Redis 客户端
            redis_client = get_redis_client_sync()

        if redis_client is None:
            logger.warning("[CacheSync] Redis 不可用，分布式缓存同步已禁用")
            return None

        _cache_sync_service = CacheSyncService(redis_client)
        logger.info("[CacheSync] 缓存同步服务已初始化")

    return _cache_sync_service


async def close_cache_sync_service() -> None:
    """关闭缓存同步服务"""
    global _cache_sync_service

    if _cache_sync_service:
        await _cache_sync_service.stop()
        _cache_sync_service = None
