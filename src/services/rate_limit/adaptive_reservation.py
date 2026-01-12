"""
自适应预留比例管理器

根据学习置信度和当前负载动态计算缓存用户预留比例，
解决固定 30% 预留在学习初期和负载变化时的不适应问题。

核心思路：
1. 探测阶段：使用低预留，让系统快速学习真实并发限制
2. 稳定阶段：根据置信度和负载动态调整预留比例
3. 置信度计算：综合考虑连续成功次数、429冷却时间、调整历史稳定性
"""

import statistics
from dataclasses import dataclass, field
from datetime import datetime, timezone
from typing import TYPE_CHECKING, Any, Dict, Optional

from src.core.logger import logger

from src.config.constants import AdaptiveReservationDefaults

if TYPE_CHECKING:
    from src.models.database import ProviderAPIKey


@dataclass
class ReservationConfig:
    """预留比例配置（使用统一常量作为默认值）"""

    # 探测阶段配置
    probe_phase_requests: int = field(
        default_factory=lambda: AdaptiveReservationDefaults.PROBE_PHASE_REQUESTS
    )
    probe_reservation: float = field(
        default_factory=lambda: AdaptiveReservationDefaults.PROBE_RESERVATION
    )

    # 稳定阶段配置
    stable_min_reservation: float = field(
        default_factory=lambda: AdaptiveReservationDefaults.STABLE_MIN_RESERVATION
    )
    stable_max_reservation: float = field(
        default_factory=lambda: AdaptiveReservationDefaults.STABLE_MAX_RESERVATION
    )

    # 置信度计算参数
    success_count_for_full_confidence: int = field(
        default_factory=lambda: AdaptiveReservationDefaults.SUCCESS_COUNT_FOR_FULL_CONFIDENCE
    )
    cooldown_hours_for_full_confidence: int = field(
        default_factory=lambda: AdaptiveReservationDefaults.COOLDOWN_HOURS_FOR_FULL_CONFIDENCE
    )

    # 负载阈值
    low_load_threshold: float = field(
        default_factory=lambda: AdaptiveReservationDefaults.LOW_LOAD_THRESHOLD
    )
    high_load_threshold: float = field(
        default_factory=lambda: AdaptiveReservationDefaults.HIGH_LOAD_THRESHOLD
    )


@dataclass
class ReservationResult:
    """预留比例计算结果"""

    ratio: float  # 最终预留比例
    phase: str  # 当前阶段: "probe" | "stable"
    confidence: float  # 置信度 (0-1)
    load_factor: float  # 负载因子 (0-1)
    details: Dict[str, Any]  # 详细信息


class AdaptiveReservationManager:
    """
    自适应预留比例管理器

    工作原理：
    1. 探测阶段（请求数 < 阈值）：
       - 使用低预留比例（10%），不浪费资源
       - 让系统快速探测真实并发限制

    2. 稳定阶段（请求数 >= 阈值）：
       - 根据置信度和负载动态计算预留比例
       - 置信度高 + 负载高 = 高预留（保护缓存用户）
       - 置信度低或负载低 = 低预留（避免浪费）

    置信度因素：
    - 连续成功次数：越多说明当前限制越准确
    - 429冷却时间：距离上次429越久越稳定
    - 调整历史稳定性：最近调整的方差越小越稳定
    """

    def __init__(self, config: Optional[ReservationConfig] = None):
        self.config = config or ReservationConfig()
        self._cache: Dict[str, ReservationResult] = {}  # 简单的内存缓存

    def calculate_reservation(
        self,
        key: "ProviderAPIKey",
        current_usage: int = 0,
        effective_limit: Optional[int] = None,
    ) -> ReservationResult:
        """
        计算当前应使用的预留比例

        Args:
            key: ProviderAPIKey 对象
            current_usage: 当前使用量（RPM 计数）
            effective_limit: 有效限制（学习值或配置值）

        Returns:
            ReservationResult 包含预留比例和详细信息
        """
        # 计算总请求数（用于判断阶段）
        total_requests = self._get_total_requests(key)

        # 计算负载率
        load_ratio = self._calculate_load_ratio(current_usage, effective_limit)

        # 阶段1: 探测阶段
        if total_requests < self.config.probe_phase_requests:
            return ReservationResult(
                ratio=self.config.probe_reservation,
                phase="probe",
                confidence=0.0,
                load_factor=load_ratio,
                details={
                    "total_requests": total_requests,
                    "probe_threshold": self.config.probe_phase_requests,
                    "reason": "探测阶段，使用低预留让系统学习真实限制",
                },
            )

        # 阶段2: 稳定阶段
        confidence = self._calculate_confidence(key)
        ratio = self._calculate_stable_ratio(confidence, load_ratio)

        return ReservationResult(
            ratio=ratio,
            phase="stable",
            confidence=confidence,
            load_factor=load_ratio,
            details={
                "total_requests": total_requests,
                "confidence_factors": self._get_confidence_breakdown(key),
                "reason": self._get_ratio_reason(confidence, load_ratio),
            },
        )

    def _get_total_requests(self, key: "ProviderAPIKey") -> int:
        """获取总请求数（用于判断是否过了探测阶段）"""
        # 使用总请求计数作为基准
        request_count = key.request_count or 0

        # 如果 request_count 为 0，使用 429 计数 + 成功计数作为近似值
        if request_count == 0:
            concurrent_429 = key.concurrent_429_count or 0
            rpm_429 = key.rpm_429_count or 0
            success_count = key.success_count or 0
            # 调整历史中的记录数也可以参考
            history_count = len(key.adjustment_history or []) * 10
            return concurrent_429 + rpm_429 + success_count + history_count

        return request_count

    def _calculate_load_ratio(
        self, current_usage: int, effective_limit: Optional[int]
    ) -> float:
        """计算当前负载率"""
        if not effective_limit or effective_limit <= 0:
            return 0.0
        return min(current_usage / effective_limit, 1.0)

    def _calculate_confidence(self, key: "ProviderAPIKey") -> float:
        """
        计算学习值的置信度 (0-1)

        三个因素各占一定权重：
        - 成功率：40%（基于总成功数/总请求数）
        - 429冷却时间：30%
        - 调整历史稳定性：30%
        """
        scores = self._get_confidence_breakdown(key)
        return min(
            scores["success_score"] + scores["cooldown_score"] + scores["stability_score"], 1.0
        )

    def _get_confidence_breakdown(self, key: "ProviderAPIKey") -> Dict[str, float]:
        """获取置信度各因素的详细分数"""
        # 因素1: 成功率（权重 40%）
        # 使用成功率而非连续成功次数，更准确反映 Key 的稳定性
        request_count = key.request_count or 0
        success_count = key.success_count or 0

        if request_count >= self.config.success_count_for_full_confidence:
            # 请求数足够时，根据成功率计算
            success_rate = success_count / request_count if request_count > 0 else 0
            success_score = success_rate * 0.4
        elif request_count > 0:
            # 请求数不足时，按比例折算
            progress_ratio = request_count / self.config.success_count_for_full_confidence
            success_rate = success_count / request_count
            success_score = success_rate * progress_ratio * 0.4
        else:
            success_score = 0.0

        # 因素2: 429冷却时间（权重 30%）
        if key.last_429_at:
            now = datetime.now(timezone.utc)
            # 确保 last_429_at 有时区信息
            last_429 = key.last_429_at
            if last_429.tzinfo is None:
                last_429 = last_429.replace(tzinfo=timezone.utc)
            hours_since_429 = (now - last_429).total_seconds() / 3600
            cooldown_ratio = min(
                hours_since_429 / self.config.cooldown_hours_for_full_confidence, 1.0
            )
            cooldown_score = cooldown_ratio * 0.3
        else:
            # 从未触发 429，给满分
            cooldown_score = 0.3

        # 因素3: 调整历史稳定性（权重 30%）
        history = key.adjustment_history or []
        if len(history) >= 3:
            # 取最近的调整记录
            recent = history[-5:] if len(history) >= 5 else history
            limits = [h.get("new_limit", 0) for h in recent if h.get("new_limit")]

            if len(limits) >= 2:
                try:
                    variance = statistics.variance(limits)
                    # 方差越小越稳定，方差为10时分数接近0
                    stability_ratio = max(0, 1 - variance / 10)
                    stability_score = stability_ratio * 0.3
                except statistics.StatisticsError:
                    stability_score = 0.15
            else:
                stability_score = 0.15
        else:
            # 历史数据不足，给一半分
            stability_score = 0.15

        # 计算成功率用于返回
        success_rate_pct = (success_count / request_count * 100) if request_count > 0 else None

        return {
            "success_score": round(success_score, 3),
            "cooldown_score": round(cooldown_score, 3),
            "stability_score": round(stability_score, 3),
            "request_count": request_count,
            "success_count": success_count,
            "success_rate": round(success_rate_pct, 1) if success_rate_pct is not None else None,
            "hours_since_429": (
                round(
                    (
                        datetime.now(timezone.utc) - key.last_429_at.replace(tzinfo=timezone.utc)
                    ).total_seconds()
                    / 3600,
                    1,
                )
                if key.last_429_at
                else None
            ),
            "history_count": len(history),
        }

    def _calculate_stable_ratio(self, confidence: float, load_ratio: float) -> float:
        """
        计算稳定阶段的预留比例

        策略：
        - 低负载（<50%）：使用最小预留，槽位充足无需过多预留
        - 中等负载（50-80%）：根据置信度线性增加预留
        - 高负载（>80%）：根据置信度使用较高预留保护缓存用户
        """
        min_r = self.config.stable_min_reservation
        max_r = self.config.stable_max_reservation

        if load_ratio < self.config.low_load_threshold:
            # 低负载：使用最小预留
            return min_r

        if load_ratio < self.config.high_load_threshold:
            # 中等负载：根据置信度和负载线性插值
            # 负载越高、置信度越高，预留越多
            load_factor = (load_ratio - self.config.low_load_threshold) / (
                self.config.high_load_threshold - self.config.low_load_threshold
            )
            return min_r + confidence * load_factor * (max_r - min_r)

        # 高负载：根据置信度决定预留比例
        # 置信度高 → 接近最大预留
        # 置信度低 → 保守预留（避免基于不准确的学习值过度预留）
        return min_r + confidence * (max_r - min_r)

    def _get_ratio_reason(self, confidence: float, load_ratio: float) -> str:
        """生成预留比例的解释"""
        if load_ratio < self.config.low_load_threshold:
            return f"低负载({load_ratio:.0%})，使用最小预留"

        if confidence < 0.3:
            return f"置信度低({confidence:.0%})，保守预留避免浪费"

        if confidence > 0.7 and load_ratio > self.config.high_load_threshold:
            return f"高置信度({confidence:.0%})+高负载({load_ratio:.0%})，使用较高预留保护缓存用户"

        return f"置信度{confidence:.0%}，负载{load_ratio:.0%}，动态计算预留"

    def get_stats(self) -> Dict[str, Any]:
        """获取管理器统计信息"""
        return {
            "config": {
                "probe_phase_requests": self.config.probe_phase_requests,
                "probe_reservation": self.config.probe_reservation,
                "stable_min_reservation": self.config.stable_min_reservation,
                "stable_max_reservation": self.config.stable_max_reservation,
                "low_load_threshold": self.config.low_load_threshold,
                "high_load_threshold": self.config.high_load_threshold,
            },
        }


# 全局单例
_reservation_manager: Optional[AdaptiveReservationManager] = None


def get_adaptive_reservation_manager() -> AdaptiveReservationManager:
    """获取全局自适应预留管理器单例"""
    global _reservation_manager
    if _reservation_manager is None:
        _reservation_manager = AdaptiveReservationManager()
    return _reservation_manager


def reset_adaptive_reservation_manager():
    """重置全局单例（用于测试）"""
    global _reservation_manager
    _reservation_manager = None
