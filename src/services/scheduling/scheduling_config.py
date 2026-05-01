"""
调度配置 (SchedulingConfig)

从 CacheAwareScheduler 提取的常量定义和模式管理逻辑。
"""

from __future__ import annotations

from src.core.logger import logger


class SchedulingConfig:
    """调度配置：管理调度模式的常量、归一化和运行时更新。"""

    # 调度模式常量
    SCHEDULING_MODE_FIXED_ORDER = "fixed_order"  # 固定顺序模式：严格按优先级，忽略缓存
    SCHEDULING_MODE_CACHE_AFFINITY = "cache_affinity"  # 缓存亲和模式：优先缓存，同优先级哈希分散
    SCHEDULING_MODE_LOAD_BALANCE = "load_balance"  # 负载均衡模式：忽略缓存，同优先级随机轮换
    ALLOWED_SCHEDULING_MODES = {
        SCHEDULING_MODE_FIXED_ORDER,
        SCHEDULING_MODE_CACHE_AFFINITY,
        SCHEDULING_MODE_LOAD_BALANCE,
    }

    @classmethod
    def normalize_scheduling_mode(cls, mode: str | None) -> str:
        normalized = (mode or "").strip().lower()
        if normalized not in cls.ALLOWED_SCHEDULING_MODES:
            if normalized:
                logger.warning(
                    "[SchedulingConfig] 无效的调度模式 '{}'，回退为 cache_affinity", mode
                )
            return cls.SCHEDULING_MODE_CACHE_AFFINITY
        return normalized

    def __init__(
        self,
        scheduling_mode: str | None = None,
    ) -> None:
        self.scheduling_mode = self.normalize_scheduling_mode(
            scheduling_mode or self.SCHEDULING_MODE_CACHE_AFFINITY
        )
        logger.debug("[SchedulingConfig] 初始化调度模式: {}", self.scheduling_mode)

    def set_scheduling_mode(self, mode: str | None) -> None:
        """运行时更新调度模式"""
        normalized = self.normalize_scheduling_mode(mode)
        if normalized == self.scheduling_mode:
            return
        self.scheduling_mode = normalized
        logger.debug("[SchedulingConfig] 切换调度模式为: {}", self.scheduling_mode)
