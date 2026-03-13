"""
并发控制检查器 (ConcurrencyChecker)

从 CacheAwareScheduler 提取的 RPM 限流和动态预留逻辑。
"""

from __future__ import annotations

import math
from typing import Any

from src.core.logger import logger
from src.models.database import ProviderAPIKey
from src.services.rate_limit.adaptive_reservation import AdaptiveReservationManager
from src.services.rate_limit.adaptive_rpm import get_adaptive_rpm_manager
from src.services.scheduling.schemas import ConcurrencySnapshot


class ConcurrencyChecker:
    """并发控制检查器，封装 RPM 限流和动态预留逻辑。"""

    def __init__(
        self,
        concurrency_manager: Any,
        reservation_manager: AdaptiveReservationManager,
    ) -> None:
        self._concurrency_manager = concurrency_manager
        self._reservation_manager = reservation_manager

    @staticmethod
    def get_effective_rpm_limit(key: ProviderAPIKey) -> int | None:
        """获取有效的 RPM 限制（委托给 AdaptiveRPMManager 统一逻辑）"""
        return get_adaptive_rpm_manager().get_effective_limit(key)

    async def check_available(
        self,
        key: ProviderAPIKey,
        is_cached_user: bool = False,
        user: Any | None = None,
    ) -> tuple[bool, ConcurrencySnapshot]:
        """
        检查 RPM 限制是否可用（先检查 User 级别，再检查 Key 级别）

        核心逻辑 - 双重限制机制:
        - User 级别：先检查用户整体 RPM 限制（NULL = 不限制）
        - Key 级别：再检查 ProviderAPIKey RPM 限制（动态预留机制）
        - 两者取最小：均需满足才能通过

        核心逻辑 - 动态缓存预留机制:
        - 总槽位: 有效 RPM 限制（固定值或学习到的值）
        - 预留比例: 由 AdaptiveReservationManager 根据置信度和负载动态计算
        - 缓存用户可用: 全部槽位
        - 新用户可用: 总槽位 x (1 - 动态预留比例)

        Args:
            key: ProviderAPIKey对象
            is_cached_user: 是否是缓存用户
            user: User对象（可选），用于检查用户级别 RPM 限制

        Returns:
            (是否可用, 并发快照)
        """
        # ---- 第一步：检查 User 级别 RPM ----
        user_count: int | None = None
        user_limit: int | None = None
        user_reservation_ratio: float | None = None

        if user is not None and self._concurrency_manager:
            user_rpm_limit = getattr(user, "rpm_limit", None)
            if user_rpm_limit is not None:
                user_limit = int(user_rpm_limit)
                user_count = await self._concurrency_manager.get_user_rpm_count(
                    user_id=str(user.id),
                )

                # 使用默认缓存预留比例
                from src.config.settings import config

                user_reservation_ratio = config.cache_reservation_ratio

                # 检查用户限制（含缓存预留）
                if is_cached_user:
                    if user_count >= user_limit:
                        logger.debug(
                            "User {}... RPM 限制已满 (缓存用户, {}/{})",
                            str(user.id)[:8],
                            user_count,
                            user_limit,
                        )
                        snapshot = ConcurrencySnapshot(
                            key_current=0,
                            key_limit=None,
                            is_cached_user=is_cached_user,
                            user_current=user_count,
                            user_limit=user_limit,
                            user_reservation_ratio=user_reservation_ratio,
                        )
                        return False, snapshot
                else:
                    available_for_new = max(
                        1, math.floor(user_limit * (1 - user_reservation_ratio))
                    )
                    if user_count >= available_for_new:
                        logger.debug(
                            "User {}... 新用户配额已满 ({}/{}, 总{}, 预留{:.0%})",
                            str(user.id)[:8],
                            user_count,
                            available_for_new,
                            user_limit,
                            user_reservation_ratio,
                        )
                        snapshot = ConcurrencySnapshot(
                            key_current=0,
                            key_limit=None,
                            is_cached_user=is_cached_user,
                            user_current=user_count,
                            user_limit=available_for_new,
                            user_reservation_ratio=user_reservation_ratio,
                        )
                        return False, snapshot

        # ---- 第二步：检查 Key 级别 RPM ----
        effective_key_limit = self.get_effective_rpm_limit(key)

        logger.debug(
            "            -> 并发检查: _concurrency_manager={}, "
            "is_cached_user={}, effective_limit={}, user_limit={}",
            self._concurrency_manager is not None,
            is_cached_user,
            effective_key_limit,
            user_limit,
        )

        if not self._concurrency_manager:
            # 并发管理器不可用，直接返回True
            logger.debug("            -> 无并发管理器，直接通过")
            snapshot = ConcurrencySnapshot(
                key_current=0,
                key_limit=effective_key_limit,
                is_cached_user=is_cached_user,
                user_current=user_count,
                user_limit=user_limit,
                user_reservation_ratio=user_reservation_ratio,
            )
            return True, snapshot

        # 获取当前 RPM 计数
        key_count = await self._concurrency_manager.get_key_rpm_count(
            key_id=str(key.id),
        )

        can_use = True

        # 计算动态预留比例
        reservation_result = self._reservation_manager.calculate_reservation(
            key=key,
            current_usage=key_count,
            effective_limit=effective_key_limit,
        )

        available_for_new = None
        reservation_ratio = reservation_result.ratio

        # 检查Key级别限制（使用动态预留比例）
        if effective_key_limit is not None:
            if is_cached_user:
                # 缓存用户: 可以使用全部槽位
                if key_count >= effective_key_limit:
                    can_use = False
            else:
                # 新用户: 只能使用 (1 - 动态预留比例) 的槽位
                # 使用 max 确保至少有 1 个槽位可用

                # 与 ConcurrencyManager 的 Lua 脚本保持一致：使用 floor 计算新用户可用槽位
                available_for_new = max(
                    1, math.floor(effective_key_limit * (1 - reservation_ratio))
                )
                if key_count >= available_for_new:
                    logger.debug(
                        "Key {}... 新用户配额已满 " "({}/{}, 总{}, 预留{:.0%}[{}])",
                        key.id[:8],
                        key_count,
                        available_for_new,
                        effective_key_limit,
                        reservation_ratio,
                        reservation_result.phase,
                    )
                    can_use = False

        key_limit_for_snapshot: int | None
        if is_cached_user:
            key_limit_for_snapshot = effective_key_limit
        elif effective_key_limit is not None:
            key_limit_for_snapshot = (
                available_for_new if available_for_new is not None else effective_key_limit
            )
        else:
            key_limit_for_snapshot = None

        snapshot = ConcurrencySnapshot(
            key_current=key_count,
            key_limit=key_limit_for_snapshot,
            is_cached_user=is_cached_user,
            reservation_ratio=reservation_ratio,
            reservation_phase=reservation_result.phase,
            reservation_confidence=reservation_result.confidence,
            load_factor=reservation_result.load_factor,
            user_current=user_count,
            user_limit=user_limit,
            user_reservation_ratio=user_reservation_ratio,
        )

        return can_use, snapshot

    def get_reservation_stats(self) -> dict[str, Any]:
        """获取动态预留管理器的统计信息"""
        return self._reservation_manager.get_stats()
