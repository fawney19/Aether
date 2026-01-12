"""
RPM 限制管理器 - 支持 Redis 或内存的 Key 级别 RPM 限制

功能：
1. ProviderAPIKey 级别的 RPM 限制（按分钟窗口计数）
2. 分布式环境下优先使用 Redis，多实例共享
3. 在开发/单实例场景下自动降级为内存计数
4. 支持缓存用户优先级（预留槽位机制）
"""

import asyncio
import math
import os
import time
from contextlib import asynccontextmanager
from typing import Optional

import redis.asyncio as aioredis

from src.config.constants import RPMDefaults
from src.core.logger import logger


class ConcurrencyManager:
    """Key RPM 限制管理器"""

    _instance: Optional["ConcurrencyManager"] = None
    _redis: Optional[aioredis.Redis] = None
    _key_rpm_bucket_seconds: int = 60
    _key_rpm_key_ttl_seconds: int = 120  # 2 分钟，足够覆盖当前分钟与边界

    def __new__(cls):
        """单例模式"""
        if cls._instance is None:
            cls._instance = super().__new__(cls)
        return cls._instance

    def __init__(self):
        """初始化内存后端结构（只执行一次）"""
        if hasattr(self, "_memory_initialized"):
            return

        self._memory_lock: asyncio.Lock = asyncio.Lock()
        # Key RPM 计数器：{key_id: (bucket, count)}，bucket = floor(now / 60)
        self._memory_key_rpm_counts: dict[str, tuple[int, int]] = {}
        self._owns_redis: bool = False
        self._last_cleanup_bucket: int = 0  # 上次清理时的 bucket，用于定期清理过期数据
        self._last_cleanup_time: float = 0  # 上次清理的时间戳，用于强制定期清理
        self._cleanup_interval_seconds: int = 300  # 强制清理间隔（5 分钟）
        self._cleanup_task: Optional[asyncio.Task] = None  # 后台清理任务

        # 内存模式下的最大条目限制，防止内存泄漏（支持环境变量覆盖）
        self._max_memory_rpm_entries: int = int(
            os.getenv("RPM_MAX_MEMORY_ENTRIES", str(RPMDefaults.MAX_MEMORY_RPM_ENTRIES))
        )
        # 早期告警阈值（达到此比例时记录警告）
        self._memory_warning_threshold: float = float(
            os.getenv("RPM_MEMORY_WARNING_THRESHOLD", str(RPMDefaults.MEMORY_WARNING_THRESHOLD))
        )
        self._memory_initialized = True

    async def initialize(self) -> None:
        """初始化 Redis 连接"""
        if self._redis is not None:
            return

        try:
            # 复用全局 Redis 客户端（带熔断/降级），避免重复创建连接池
            from src.clients.redis_client import get_redis_client

            self._redis = await get_redis_client(require_redis=False)
            self._owns_redis = False
            if self._redis:
                logger.info("[OK] ConcurrencyManager 已复用全局 Redis 客户端")
            else:
                logger.warning("[WARN] Redis 不可用，RPM 限制降级为内存模式（仅在单实例环境下安全）")
                # 内存模式下启动后台清理任务
                self._start_background_cleanup()
        except Exception as e:
            logger.error(f"[ERROR] 获取全局 Redis 客户端失败: {e}")
            logger.warning("[WARN] RPM 限制将降级为内存模式（仅在单实例环境下安全）")
            self._redis = None
            self._owns_redis = False
            # 内存模式下启动后台清理任务
            self._start_background_cleanup()

    def _start_background_cleanup(self) -> None:
        """启动后台定期清理任务（仅内存模式需要）"""
        if self._cleanup_task is not None:
            return  # 已经启动

        async def cleanup_loop():
            """后台清理循环"""
            while True:
                try:
                    await asyncio.sleep(60)  # 每分钟检查一次
                    async with self._memory_lock:
                        current_bucket = self._get_rpm_bucket()
                        self._cleanup_expired_memory_rpm_counts(current_bucket, force=False)
                except asyncio.CancelledError:
                    break
                except Exception as e:
                    logger.debug(f"后台清理任务异常: {e}")

        try:
            self._cleanup_task = asyncio.create_task(cleanup_loop())
            logger.debug("[OK] 内存模式后台清理任务已启动")
        except RuntimeError:
            # 没有事件循环时忽略
            pass

    async def close(self) -> None:
        """关闭 Redis 连接"""
        # 停止后台清理任务
        if self._cleanup_task is not None:
            self._cleanup_task.cancel()
            try:
                await self._cleanup_task
            except asyncio.CancelledError:
                pass
            self._cleanup_task = None

        if self._redis and self._owns_redis:
            await self._redis.close()
            logger.info("ConcurrencyManager Redis 连接已关闭")
        self._redis = None
        self._owns_redis = False

    @classmethod
    def _get_rpm_bucket(cls, now_ts: Optional[float] = None) -> int:
        """获取当前 RPM 计数桶（按分钟）"""
        ts = now_ts if now_ts is not None else time.time()
        return int(ts // cls._key_rpm_bucket_seconds)

    @classmethod
    def _get_key_key(cls, key_id: str, bucket: Optional[int] = None) -> str:
        """获取 ProviderAPIKey RPM 计数的 Redis Key（按分钟桶）"""
        b = bucket if bucket is not None else cls._get_rpm_bucket()
        return f"rpm:key:{key_id}:{b}"

    def _get_memory_key_rpm_count(self, key_id: str, bucket: int) -> int:
        """获取内存模式下 Key 在指定 bucket 的 RPM 计数"""
        stored = self._memory_key_rpm_counts.get(key_id)
        if not stored:
            return 0
        stored_bucket, count = stored
        if stored_bucket != bucket:
            # 旧桶数据已过期，删除以防止内存泄漏
            del self._memory_key_rpm_counts[key_id]
            return 0
        return count

    def _set_memory_key_rpm_count(self, key_id: str, bucket: int, count: int) -> None:
        """设置内存模式下 Key 在指定 bucket 的 RPM 计数"""
        current_size = len(self._memory_key_rpm_counts)
        warning_threshold = int(self._max_memory_rpm_entries * self._memory_warning_threshold)
        high_threshold = int(self._max_memory_rpm_entries * 0.8)
        critical_threshold = int(self._max_memory_rpm_entries * 0.95)

        # 分级告警：根据使用率记录不同级别的日志
        if current_size >= critical_threshold and key_id not in self._memory_key_rpm_counts:
            logger.critical(
                f"[CRITICAL] 内存 RPM 计数器接近上限 ({current_size}/{self._max_memory_rpm_entries})，"
                f"强烈建议启用 Redis！继续增长可能导致 RPM 限制失效"
            )
        elif current_size >= high_threshold and key_id not in self._memory_key_rpm_counts:
            # 每 100 个条目告警一次，避免日志过多
            if current_size % 100 == 0:
                logger.error(
                    f"[ERROR] 内存 RPM 计数器使用率过高 ({current_size}/{self._max_memory_rpm_entries})，"
                    f"建议启用 Redis"
                )
        elif current_size >= warning_threshold and key_id not in self._memory_key_rpm_counts:
            if current_size == warning_threshold:
                logger.warning(
                    f"[WARN] 内存 RPM 计数器达到 {self._memory_warning_threshold:.0%} 阈值 "
                    f"({current_size}/{self._max_memory_rpm_entries})，建议启用 Redis"
                )

        # 检查是否超过最大条目限制
        if (
            key_id not in self._memory_key_rpm_counts
            and current_size >= self._max_memory_rpm_entries
        ):
            # 触发强制清理
            self._cleanup_expired_memory_rpm_counts(bucket, force=True)
            # 如果清理后仍然超过限制，执行 LRU 淘汰（删除最旧的 20%）
            if len(self._memory_key_rpm_counts) >= self._max_memory_rpm_entries:
                evict_count = max(1, self._max_memory_rpm_entries // 5)
                # 按 bucket（时间）排序，删除最旧的
                sorted_keys = sorted(
                    self._memory_key_rpm_counts.items(),
                    key=lambda x: x[1][0]  # 按 bucket 排序
                )
                for k, _ in sorted_keys[:evict_count]:
                    del self._memory_key_rpm_counts[k]
                logger.warning(
                    f"[WARN] 内存 RPM 计数器达到上限，已淘汰 {evict_count} 个最旧条目"
                )
        self._memory_key_rpm_counts[key_id] = (bucket, count)

    def _cleanup_expired_memory_rpm_counts(self, current_bucket: int, force: bool = False) -> None:
        """
        清理内存中过期的 RPM 计数（必须在持有 _memory_lock 时调用）

        清理策略：
        - 常规清理：每分钟最多执行一次（当 bucket 变化时）
        - 强制清理：每 5 分钟执行一次（防止长时间无请求导致内存泄漏）
        """
        now = time.time()

        # 检查是否需要清理
        should_cleanup = (
            current_bucket != self._last_cleanup_bucket  # 分钟切换
            or force  # 强制清理
            or (now - self._last_cleanup_time > self._cleanup_interval_seconds)  # 超时清理
        )

        if not should_cleanup:
            return

        self._last_cleanup_bucket = current_bucket
        self._last_cleanup_time = now
        expired_keys = []

        for key_id, (stored_bucket, _count) in self._memory_key_rpm_counts.items():
            if stored_bucket < current_bucket:
                expired_keys.append(key_id)

        for key_id in expired_keys:
            del self._memory_key_rpm_counts[key_id]

        if expired_keys:
            logger.debug(f"[CLEANUP] 清理了 {len(expired_keys)} 个过期的内存 RPM 计数")

    async def get_key_rpm_count(self, key_id: str) -> int:
        """
        获取 Key 当前 RPM 计数

        Args:
            key_id: ProviderAPIKey ID

        Returns:
            当前分钟窗口内的请求数
        """
        if self._redis is None:
            async with self._memory_lock:
                bucket = self._get_rpm_bucket()
                # 定期清理过期数据，避免内存泄漏
                self._cleanup_expired_memory_rpm_counts(bucket)
                return self._get_memory_key_rpm_count(key_id, bucket)

        try:
            key_key = self._get_key_key(key_id)
            result = await self._redis.get(key_key)
            return int(result) if result else 0
        except Exception as e:
            logger.error(f"获取 RPM 计数失败: {e}")
            return 0

    async def check_rpm_available(
        self,
        key_id: str,
        key_rpm_limit: Optional[int],
        is_cached_user: bool = False,
        cache_reservation_ratio: Optional[float] = None,
    ) -> bool:
        """
        检查是否可以通过 RPM 限制（不实际增加计数）

        Args:
            key_id: ProviderAPIKey ID
            key_rpm_limit: Key RPM 限制（每分钟最大请求数，None 表示不限制）
            is_cached_user: 是否是缓存用户
            cache_reservation_ratio: 缓存预留比例

        Returns:
            是否可用（True/False）
        """
        if key_rpm_limit is None:
            return True

        # 从配置读取默认值
        from src.config.settings import config

        if cache_reservation_ratio is None:
            cache_reservation_ratio = config.cache_reservation_ratio

        key_count = await self.get_key_rpm_count(key_id)

        if is_cached_user:
            return key_count < key_rpm_limit
        else:
            # 新用户只能使用 (1 - cache_reservation_ratio) 的槽位
            available_for_new = max(1, math.floor(key_rpm_limit * (1 - cache_reservation_ratio)))
            return key_count < available_for_new

    async def acquire_rpm_slot(
        self,
        key_id: str,
        key_rpm_limit: Optional[int],
        is_cached_user: bool = False,
        cache_reservation_ratio: Optional[float] = None,
    ) -> bool:
        """
        尝试获取 RPM 槽位（支持缓存用户优先级）

        Args:
            key_id: ProviderAPIKey ID
            key_rpm_limit: Key RPM 限制（每分钟最大请求数，None 表示不限制）
            is_cached_user: 是否是缓存用户（缓存用户可使用全部槽位）
            cache_reservation_ratio: 缓存预留比例，None 时从配置读取

        Returns:
            是否成功获取（True/False）

        缓存预留机制说明:
        - 假设 key_rpm_limit = 100, cache_reservation_ratio = 0.3
        - 新用户最多使用: 70 RPM (100 * (1 - 0.3))
        - 缓存用户最多使用: 100 RPM（全部）
        - 预留的 30 RPM 专门给缓存用户，保证他们的请求优先
        """
        # 从配置读取默认值
        from src.config.settings import config

        if cache_reservation_ratio is None:
            cache_reservation_ratio = config.cache_reservation_ratio

        if self._redis is None:
            async with self._memory_lock:
                bucket = self._get_rpm_bucket()
                # 定期清理过期数据，避免内存泄漏
                self._cleanup_expired_memory_rpm_counts(bucket)

                key_count = self._get_memory_key_rpm_count(key_id, bucket)

                # Key RPM 限制，包含缓存预留
                if key_rpm_limit is not None:
                    if is_cached_user:
                        if key_count >= key_rpm_limit:
                            return False
                    else:
                        # 新用户只能使用 (1 - cache_reservation_ratio) 的槽位
                        available_for_new = max(
                            1, math.floor(key_rpm_limit * (1 - cache_reservation_ratio))
                        )
                        if key_count >= available_for_new:
                            return False

                # 通过限制，更新计数
                self._set_memory_key_rpm_count(key_id, bucket, key_count + 1)
                return True

        bucket = self._get_rpm_bucket()
        key_key = self._get_key_key(key_id, bucket=bucket)

        try:
            # 使用 Lua 脚本保证原子性（支持缓存预留逻辑）
            lua_script = """
            local key_key = KEYS[1]
            local key_max = tonumber(ARGV[1])
            local key_ttl = tonumber(ARGV[2])
            local is_cached = tonumber(ARGV[3])  -- 0=新用户, 1=缓存用户
            local cache_ratio = tonumber(ARGV[4])  -- 缓存预留比例

            -- 获取当前值
            local key_count = tonumber(redis.call('GET', key_key) or '0')

            -- 检查 key 限制（支持缓存预留）
            if key_max >= 0 then
                if is_cached == 0 then
                    -- 新用户：只能使用 (1 - cache_ratio) 的槽位
                    local available_for_new = math.max(1, math.floor(key_max * (1 - cache_ratio)))
                    if key_count >= available_for_new then
                        return 0  -- 失败：新用户配额已满
                    end
                else
                    -- 缓存用户：可以使用全部槽位
                    if key_count >= key_max then
                        return 0  -- 失败：总配额已满
                    end
                end
            end

            -- 增加计数
            redis.call('INCR', key_key)
            redis.call('EXPIRE', key_key, key_ttl)

            return 1  -- 成功
            """

            # 执行脚本
            result = await self._redis.eval(
                lua_script,
                1,  # 1 个 KEY
                key_key,
                key_rpm_limit if key_rpm_limit is not None else -1,
                self._key_rpm_key_ttl_seconds,
                1 if is_cached_user else 0,  # 缓存用户标志
                cache_reservation_ratio,  # 预留比例
            )

            success = result == 1

            if success:
                user_type = "缓存用户" if is_cached_user else "新用户"
                logger.debug(f"[OK] 获取 RPM 槽位成功: key={key_id}, 类型={user_type}")
            else:
                key_count = await self.get_key_rpm_count(key_id)

                # 计算新用户可用 RPM
                if key_rpm_limit and not is_cached_user:
                    available_for_new = int(key_rpm_limit * (1 - cache_reservation_ratio))
                    user_info = f"新用户配额={available_for_new}, 当前={key_count}"
                else:
                    user_info = f"缓存用户, 当前={key_count}/{key_rpm_limit}"

                logger.warning(f"[WARN] RPM 限制已达上限: key={key_id}({user_info})")

            return success

        except Exception as e:
            logger.error(f"获取 RPM 槽位失败，降级到内存模式: {e}")
            # Redis 异常时降级到内存模式进行保守限流
            async with self._memory_lock:
                bucket = self._get_rpm_bucket()
                self._cleanup_expired_memory_rpm_counts(bucket)

                key_count = self._get_memory_key_rpm_count(key_id, bucket)

                # 降级模式下使用更保守的限制（50%）
                fallback_rpm_limit = (
                    max(1, key_rpm_limit // 2) if key_rpm_limit is not None else None
                )

                if fallback_rpm_limit is not None and key_count >= fallback_rpm_limit:
                    logger.warning(
                        f"[FALLBACK] Key RPM 达到降级限制: {key_count}/{fallback_rpm_limit}"
                    )
                    return False

                # 更新内存计数
                self._set_memory_key_rpm_count(key_id, bucket, key_count + 1)
                logger.debug(f"[FALLBACK] 使用内存模式获取 RPM 槽位: key={key_id}")
                return True

    @asynccontextmanager
    async def rpm_guard(
        self,
        key_id: str,
        key_rpm_limit: Optional[int],
        is_cached_user: bool = False,
        cache_reservation_ratio: Optional[float] = None,
    ):
        """
        RPM 限制上下文管理器（支持缓存用户优先级）

        用法：
            async with manager.rpm_guard(
                key_id, key_rpm_limit,
                is_cached_user=True  # 缓存用户
            ):
                # 执行请求
                response = await send_request(...)

        如果获取失败，会抛出 ConcurrencyLimitError 异常

        注意：RPM 是按分钟窗口计数，不需要在请求结束后释放
        """
        # 从配置读取默认值
        from src.config.settings import config

        if cache_reservation_ratio is None:
            cache_reservation_ratio = config.cache_reservation_ratio

        # 尝试获取槽位（传递缓存用户参数）
        acquired = await self.acquire_rpm_slot(
            key_id,
            key_rpm_limit,
            is_cached_user,
            cache_reservation_ratio,
        )

        if not acquired:
            from src.core.exceptions import ConcurrencyLimitError

            user_type = "缓存用户" if is_cached_user else "新用户"
            raise ConcurrencyLimitError(
                f"RPM 限制已达上限: key={key_id}, 类型={user_type}"
            )

        # 记录开始时间和状态
        import time

        slot_acquired_at = time.time()
        exception_occurred = False

        try:
            yield  # 执行请求
        except Exception:
            # 记录异常
            exception_occurred = True
            raise
        finally:
            # 计算槽位占用时长
            slot_duration = time.time() - slot_acquired_at

            # 记录 Prometheus 指标
            try:
                from src.core.metrics import (
                    concurrency_slot_duration_seconds,
                    concurrency_slot_release_total,
                )

                # 记录槽位占用时长分布
                concurrency_slot_duration_seconds.labels(
                    key_id=key_id[:8] if key_id else "unknown",  # 只记录前8位
                    exception=str(exception_occurred),
                ).observe(slot_duration)

                # 记录槽位释放计数
                concurrency_slot_release_total.labels(
                    key_id=key_id[:8] if key_id else "unknown", exception=str(exception_occurred)
                ).inc()

                # 告警：槽位占用时间过长（超过 60 秒）
                if slot_duration > 60:
                    logger.warning(
                        f"[WARN] 请求耗时过长: "
                        f"key_id={key_id[:8] if key_id else 'unknown'}..., "
                        f"duration={slot_duration:.1f}s, "
                        f"exception={exception_occurred}"
                    )

            except Exception as metric_error:
                # 指标记录失败不应影响业务逻辑
                logger.debug(f"记录指标失败: {metric_error}")

            # 注意：RPM 计数不需要在请求结束后释放，它会在分钟窗口过期后自动重置

    async def reset_key_rpm(self, key_id: str) -> None:
        """
        重置 Key RPM 计数（管理功能，慎用）

        Args:
            key_id: ProviderAPIKey ID
        """
        if self._redis is None:
            async with self._memory_lock:
                self._memory_key_rpm_counts.pop(key_id, None)
                logger.info(f"[RESET] 重置 Key RPM 计数(内存): {key_id}")
            return

        try:
            deleted_count = await self._scan_and_delete(f"rpm:key:{key_id}:*")
            logger.info(f"[RESET] 重置 Key RPM 计数: {key_id}, 删除 {deleted_count} 个键")
        except Exception as e:
            logger.error(f"重置 Key RPM 计数失败: {e}")

    async def reset_all_rpm(self) -> None:
        """重置所有 Key RPM 计数（管理功能，慎用）"""
        if self._redis is None:
            async with self._memory_lock:
                count = len(self._memory_key_rpm_counts)
                self._memory_key_rpm_counts.clear()
                if count:
                    logger.info(f"[RESET] 重置所有 Key RPM 计数(内存): {count} 个")
            return

        try:
            deleted_count = await self._scan_and_delete("rpm:key:*")
            if deleted_count:
                logger.info(f"[RESET] 重置所有 Key RPM 计数: {deleted_count} 个")
        except Exception as e:
            logger.error(f"重置所有 Key RPM 计数失败: {e}")

    async def _scan_and_delete(self, pattern: str, batch_size: int = 100) -> int:
        """使用 SCAN 遍历并分批删除匹配的键，避免阻塞 Redis"""
        if self._redis is None:
            return 0

        deleted_count = 0
        cursor = 0
        while True:
            cursor, keys = await self._redis.scan(cursor, match=pattern, count=batch_size)
            if keys:
                # 分批删除，每批最多 batch_size 个
                for i in range(0, len(keys), batch_size):
                    batch = keys[i : i + batch_size]
                    await self._redis.delete(*batch)
                    deleted_count += len(batch)
            if cursor == 0:
                break
        return deleted_count


# 全局单例
_concurrency_manager: Optional[ConcurrencyManager] = None


async def get_concurrency_manager() -> ConcurrencyManager:
    """获取全局 ConcurrencyManager 实例"""
    global _concurrency_manager

    if _concurrency_manager is None:
        _concurrency_manager = ConcurrencyManager()
        await _concurrency_manager.initialize()

    return _concurrency_manager
