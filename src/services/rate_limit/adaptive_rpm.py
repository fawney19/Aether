"""
自适应 RPM 调整器 - 基于边界记忆的 RPM 限制调整

核心算法：边界记忆 + 渐进探测
- 触发 429 时记录边界（last_rpm_peak），这就是真实上限
- 缩容策略：新限制 = 边界 - 步长，而非乘性减少
- 扩容策略：不超过已知边界，除非是探测性扩容
- 探测性扩容：长时间无 429 时尝试突破边界

设计原则：
1. 快速收敛：一次 429 就能找到接近真实的限制
2. 避免过度保守：不会因为多次 429 而无限下降
3. 安全探测：允许在稳定后尝试更高 RPM
"""

from datetime import datetime, timezone
from typing import Any, Dict, List, Optional, cast

from sqlalchemy.orm import Session

from src.config.constants import RPMDefaults
from src.core.batch_committer import get_batch_committer
from src.core.logger import logger
from src.models.database import ProviderAPIKey
from src.services.rate_limit.detector import RateLimitInfo, RateLimitType


class AdaptiveStrategy:
    """自适应策略类型"""

    AIMD = "aimd"  # 加性增-乘性减 (Additive Increase Multiplicative Decrease)
    CONSERVATIVE = "conservative"  # 保守策略（只减不增）
    AGGRESSIVE = "aggressive"  # 激进策略（快速探测）


class AdaptiveRPMManager:
    """
    自适应 RPM 管理器

    核心算法：边界记忆 + 渐进探测
    - 触发 429 时记录边界（last_rpm_peak = 触发时的 RPM）
    - 缩容：新限制 = 边界 - 步长（快速收敛到真实限制附近）
    - 扩容：不超过边界（即 last_rpm_peak），允许回到边界值尝试
    - 探测性扩容：长时间（30分钟）无 429 时，可以尝试 +1 突破边界

    扩容条件（满足任一即可）：
    1. 利用率扩容：窗口内高利用率比例 >= 60%，且当前限制 < 边界
    2. 探测性扩容：距上次 429 超过 30 分钟，可以尝试突破边界

    关键特性：
    1. 快速收敛：一次 429 就能学到接近真实的限制值
    2. 边界保护：普通扩容不会超过已知边界
    3. 安全探测：长时间稳定后允许尝试更高 RPM
    4. 区分并发限制和 RPM 限制
    """

    # 默认配置 - 使用统一常量
    DEFAULT_INITIAL_LIMIT = RPMDefaults.INITIAL_LIMIT
    MIN_RPM_LIMIT = RPMDefaults.MIN_RPM_LIMIT
    MAX_RPM_LIMIT = RPMDefaults.MAX_RPM_LIMIT

    # AIMD 参数
    INCREASE_STEP = RPMDefaults.INCREASE_STEP

    # 滑动窗口参数
    UTILIZATION_WINDOW_SIZE = RPMDefaults.UTILIZATION_WINDOW_SIZE
    UTILIZATION_WINDOW_SECONDS = RPMDefaults.UTILIZATION_WINDOW_SECONDS
    UTILIZATION_THRESHOLD = RPMDefaults.UTILIZATION_THRESHOLD
    HIGH_UTILIZATION_RATIO = RPMDefaults.HIGH_UTILIZATION_RATIO
    MIN_SAMPLES_FOR_DECISION = RPMDefaults.MIN_SAMPLES_FOR_DECISION

    # 探测性扩容参数
    PROBE_INCREASE_INTERVAL_MINUTES = RPMDefaults.PROBE_INCREASE_INTERVAL_MINUTES
    PROBE_INCREASE_MIN_REQUESTS = RPMDefaults.PROBE_INCREASE_MIN_REQUESTS

    # 记录历史数量
    MAX_HISTORY_RECORDS = 20

    def __init__(self, strategy: str = AdaptiveStrategy.AIMD):
        """
        初始化自适应 RPM 管理器

        Args:
            strategy: 调整策略
        """
        self.strategy = strategy

    def handle_429_error(
        self,
        db: Session,
        key: ProviderAPIKey,
        rate_limit_info: RateLimitInfo,
        current_rpm: Optional[int] = None,
    ) -> int:
        """
        处理429错误，调整 RPM 限制

        Args:
            db: 数据库会话
            key: API Key对象
            rate_limit_info: 速率限制信息
            current_rpm: 当前分钟内的请求数

        Returns:
            调整后的 RPM 限制
        """
        # rpm_limit=NULL 表示启用自适应，rpm_limit=数字 表示固定限制
        is_adaptive = key.rpm_limit is None

        if not is_adaptive:
            logger.debug(
                f"Key {key.id} 设置了固定 RPM 限制 ({key.rpm_limit})，跳过自适应调整"
            )
            return int(key.rpm_limit)  # type: ignore[arg-type]

        # 更新429统计
        key.last_429_at = datetime.now(timezone.utc)  # type: ignore[assignment]
        key.last_429_type = rate_limit_info.limit_type  # type: ignore[assignment]
        # 仅在 RPM 限制且拿到 RPM 数时记录边界
        if (
            rate_limit_info.limit_type == RateLimitType.RPM
            and current_rpm is not None
            and current_rpm > 0
        ):
            key.last_rpm_peak = current_rpm  # type: ignore[assignment]

        # 遇到 429 错误，清空利用率采样窗口（重新开始收集）
        key.utilization_samples = []  # type: ignore[assignment]

        if rate_limit_info.limit_type == RateLimitType.RPM:
            # RPM 限制：减少 RPM 限制
            key.rpm_429_count = int(key.rpm_429_count or 0) + 1  # type: ignore[assignment]

            # 获取当前有效限制（自适应模式使用 learned_rpm_limit）
            old_limit = int(key.learned_rpm_limit or self.DEFAULT_INITIAL_LIMIT)
            new_limit = self._decrease_limit(old_limit, current_rpm)

            logger.warning(
                f"[RPM] RPM 限制触发: Key {key.id[:8]}... | "
                f"当前 RPM: {current_rpm} | "
                f"调整: {old_limit} -> {new_limit}"
            )

            # 记录调整历史
            self._record_adjustment(
                key,
                old_limit=old_limit,
                new_limit=new_limit,
                reason="rpm_429",
                current_rpm=current_rpm,
            )

            # 更新学习到的 RPM 限制
            key.learned_rpm_limit = new_limit  # type: ignore[assignment]

        elif rate_limit_info.limit_type == RateLimitType.CONCURRENT:
            # 并发限制：不调整 RPM，只记录
            key.concurrent_429_count = int(key.concurrent_429_count or 0) + 1  # type: ignore[assignment]

            logger.info(
                f"[CONCURRENT] 并发限制触发: Key {key.id[:8]}... | "
                f"不调整 RPM 限制（这是并发问题，非 RPM 问题）"
            )

        else:
            # 未知类型：保守处理，轻微减少
            logger.warning(
                f"[UNKNOWN] 未知429类型: Key {key.id[:8]}... | "
                f"当前 RPM: {current_rpm} | "
                f"保守减少 RPM"
            )

            old_limit = int(key.learned_rpm_limit or self.DEFAULT_INITIAL_LIMIT)
            new_limit = max(int(old_limit * 0.9), self.MIN_RPM_LIMIT)  # 减少10%

            self._record_adjustment(
                key,
                old_limit=old_limit,
                new_limit=new_limit,
                reason="unknown_429",
                current_rpm=current_rpm,
            )

            key.learned_rpm_limit = new_limit  # type: ignore[assignment]

        db.flush()
        get_batch_committer().mark_dirty(db)

        return int(key.learned_rpm_limit or self.DEFAULT_INITIAL_LIMIT)

    def handle_success(
        self,
        db: Session,
        key: ProviderAPIKey,
        current_rpm: int,
    ) -> Optional[int]:
        """
        处理成功请求，基于滑动窗口利用率考虑增加 RPM 限制

        Args:
            db: 数据库会话
            key: API Key对象
            current_rpm: 当前分钟内的请求数（必需，用于计算利用率）

        Returns:
            调整后的 RPM 限制（如果有调整），否则返回 None
        """
        # rpm_limit=NULL 表示启用自适应
        is_adaptive = key.rpm_limit is None

        if not is_adaptive:
            return None

        current_limit = int(key.learned_rpm_limit or self.DEFAULT_INITIAL_LIMIT)

        # 获取已知边界（上次触发 429 时的 RPM）
        known_boundary = key.last_rpm_peak

        # 计算当前利用率
        utilization = float(current_rpm / current_limit) if current_limit > 0 else 0.0

        now = datetime.now(timezone.utc)
        now_ts = now.timestamp()

        # 更新滑动窗口
        samples = self._update_utilization_window(key, now_ts, utilization)

        # 检查是否满足扩容条件
        increase_reason = self._check_increase_conditions(key, samples, now, known_boundary)

        if increase_reason and current_limit < self.MAX_RPM_LIMIT:
            old_limit = current_limit
            is_probe = increase_reason == "probe_increase"
            new_limit = self._increase_limit(current_limit, known_boundary, is_probe)

            # 如果没有实际增长（已达边界），跳过
            if new_limit <= old_limit:
                return None

            # 计算窗口统计用于日志
            avg_util = sum(s["util"] for s in samples) / len(samples) if samples else 0
            high_util_count = sum(1 for s in samples if s["util"] >= self.UTILIZATION_THRESHOLD)
            high_util_ratio = high_util_count / len(samples) if samples else 0

            boundary_info = f"边界: {known_boundary}" if known_boundary else "无边界"
            logger.info(
                f"[INCREASE] {increase_reason}: Key {key.id[:8]}... | "
                f"窗口采样: {len(samples)} | "
                f"平均利用率: {avg_util:.1%} | "
                f"高利用率比例: {high_util_ratio:.1%} | "
                f"{boundary_info} | "
                f"调整: {old_limit} -> {new_limit}"
            )

            # 记录调整历史
            self._record_adjustment(
                key,
                old_limit=old_limit,
                new_limit=new_limit,
                reason=increase_reason,
                avg_utilization=round(avg_util, 2),
                high_util_ratio=round(high_util_ratio, 2),
                sample_count=len(samples),
                current_rpm=current_rpm,
                known_boundary=known_boundary,
            )

            # 更新限制
            key.learned_rpm_limit = new_limit  # type: ignore[assignment]

            # 如果是探测性扩容，更新探测时间
            if is_probe:
                key.last_probe_increase_at = now  # type: ignore[assignment]

            # 扩容后清空采样窗口，重新开始收集
            key.utilization_samples = []  # type: ignore[assignment]

            db.flush()
            get_batch_committer().mark_dirty(db)

            return new_limit

        # 定期持久化采样数据（每5个采样保存一次）
        if len(samples) % 5 == 0:
            db.flush()
            get_batch_committer().mark_dirty(db)

        return None

    def _update_utilization_window(
        self, key: ProviderAPIKey, now_ts: float, utilization: float
    ) -> List[Dict[str, Any]]:
        """
        更新利用率滑动窗口

        Args:
            key: API Key对象
            now_ts: 当前时间戳
            utilization: 当前利用率

        Returns:
            更新后的采样列表
        """
        samples: List[Dict[str, Any]] = list(key.utilization_samples or [])

        # 添加新采样
        samples.append({"ts": now_ts, "util": round(utilization, 3)})

        # 移除过期采样（超过时间窗口）
        cutoff_ts = now_ts - self.UTILIZATION_WINDOW_SECONDS
        samples = [s for s in samples if s["ts"] > cutoff_ts]

        # 限制采样数量
        if len(samples) > self.UTILIZATION_WINDOW_SIZE:
            samples = samples[-self.UTILIZATION_WINDOW_SIZE:]

        # 更新到 key 对象
        key.utilization_samples = samples  # type: ignore[assignment]

        return samples

    def _check_increase_conditions(
        self,
        key: ProviderAPIKey,
        samples: List[Dict[str, Any]],
        now: datetime,
        known_boundary: Optional[int] = None,
    ) -> Optional[str]:
        """
        检查是否满足扩容条件

        Args:
            key: API Key对象
            samples: 利用率采样列表
            now: 当前时间
            known_boundary: 已知边界（触发 429 时的 RPM）

        Returns:
            扩容原因（如果满足条件），否则返回 None
        """
        # 检查是否在冷却期
        if self._is_in_cooldown(key):
            return None

        current_limit = int(key.learned_rpm_limit or self.DEFAULT_INITIAL_LIMIT)

        # 条件1：滑动窗口扩容（不超过边界）
        if len(samples) >= self.MIN_SAMPLES_FOR_DECISION:
            high_util_count = sum(1 for s in samples if s["util"] >= self.UTILIZATION_THRESHOLD)
            high_util_ratio = high_util_count / len(samples)

            if high_util_ratio >= self.HIGH_UTILIZATION_RATIO:
                # 检查是否还有扩容空间（边界保护）
                if known_boundary:
                    # 允许扩容到边界值（而非 boundary - 1），因为缩容时已经 -步长 了
                    if current_limit < known_boundary:
                        return "high_utilization"
                    # 已达边界，不触发普通扩容
                else:
                    # 无边界信息，允许扩容
                    return "high_utilization"

        # 条件2：探测性扩容（长时间无 429 且有流量，可以突破边界）
        if self._should_probe_increase(key, samples, now):
            return "probe_increase"

        return None

    def _should_probe_increase(
        self, key: ProviderAPIKey, samples: List[Dict[str, Any]], now: datetime
    ) -> bool:
        """
        检查是否应该进行探测性扩容

        条件：
        1. 距上次 429 超过 PROBE_INCREASE_INTERVAL_MINUTES 分钟
        2. 距上次探测性扩容超过 PROBE_INCREASE_INTERVAL_MINUTES 分钟
        3. 期间有足够的请求量（采样数 >= PROBE_INCREASE_MIN_REQUESTS）
        4. 平均利用率 > 30%（说明确实有使用需求）

        Args:
            key: API Key对象
            samples: 利用率采样列表
            now: 当前时间

        Returns:
            是否应该探测性扩容
        """
        probe_interval_seconds = self.PROBE_INCREASE_INTERVAL_MINUTES * 60

        # 检查距上次 429 的时间
        if key.last_429_at:
            last_429_at = cast(datetime, key.last_429_at)
            time_since_429 = (now - last_429_at).total_seconds()
            if time_since_429 < probe_interval_seconds:
                return False

        # 检查距上次探测性扩容的时间
        if key.last_probe_increase_at:
            last_probe = cast(datetime, key.last_probe_increase_at)
            time_since_probe = (now - last_probe).total_seconds()
            if time_since_probe < probe_interval_seconds:
                return False

        # 检查请求量
        if len(samples) < self.PROBE_INCREASE_MIN_REQUESTS:
            return False

        # 检查平均利用率（确保确实有使用需求）
        avg_util = sum(s["util"] for s in samples) / len(samples)
        if avg_util < 0.3:  # 至少 30% 利用率
            return False

        return True

    def _is_in_cooldown(self, key: ProviderAPIKey) -> bool:
        """
        检查是否在 429 错误后的冷却期内

        Args:
            key: API Key对象

        Returns:
            True 如果在冷却期内，否则 False
        """
        if key.last_429_at is None:
            return False

        last_429_at = cast(datetime, key.last_429_at)
        time_since_429 = (datetime.now(timezone.utc) - last_429_at).total_seconds()
        cooldown_seconds = RPMDefaults.COOLDOWN_AFTER_429_MINUTES * 60

        return bool(time_since_429 < cooldown_seconds)

    def _decrease_limit(
        self,
        current_limit: int,
        current_rpm: Optional[int] = None,
    ) -> int:
        """
        减少 RPM 限制（基于边界记忆策略）

        策略：
        - 如果知道触发 429 时的 RPM，新限制 = RPM * 0.90（保留 10% 安全边际）
        - 10% 的安全边际更保守，考虑到：
          1. RPM 报告可能存在延迟，实际触发时的 RPM 可能略高于报告值
          2. 上游 API 的限制可能有波动
          3. 避免频繁在边界附近触发 429
        - 相比固定步长，百分比方式更适应不同量级的限制值
        """
        if current_rpm is not None and current_rpm > 0:
            # 边界记忆策略：新限制 = 触发边界 * 0.90（10% 安全边际）
            candidate = int(current_rpm * 0.90)
        else:
            # 没有 RPM 信息时，减少 10%
            candidate = int(current_limit * 0.9)

        # 保证不会"缩容变扩容"
        candidate = min(candidate, current_limit - 1)

        new_limit = max(candidate, self.MIN_RPM_LIMIT)

        return new_limit

    def _increase_limit(
        self,
        current_limit: int,
        known_boundary: Optional[int] = None,
        is_probe: bool = False,
    ) -> int:
        """
        增加 RPM 限制（考虑边界保护）

        策略：
        - 普通扩容：每次 +INCREASE_STEP，但不超过 known_boundary
        - 探测性扩容：每次只 +1，可以突破边界，但要谨慎

        Args:
            current_limit: 当前限制
            known_boundary: 已知边界（last_rpm_peak），即触发 429 时的 RPM
            is_probe: 是否是探测性扩容（可以突破边界）
        """
        if is_probe:
            # 探测模式：每次只 +1，谨慎突破边界
            new_limit = current_limit + 1
        else:
            # 普通模式：每次 +INCREASE_STEP
            new_limit = current_limit + self.INCREASE_STEP

            # 边界保护：普通扩容不超过 known_boundary
            if known_boundary:
                if new_limit > known_boundary:
                    new_limit = known_boundary

        # 全局上限保护
        new_limit = min(new_limit, self.MAX_RPM_LIMIT)

        # 确保有增长（否则返回原值表示不扩容）
        if new_limit <= current_limit:
            return current_limit

        return new_limit

    def _record_adjustment(
        self,
        key: ProviderAPIKey,
        old_limit: int,
        new_limit: int,
        reason: str,
        **extra_data: Any,
    ) -> None:
        """
        记录 RPM 调整历史

        Args:
            key: API Key对象
            old_limit: 原限制
            new_limit: 新限制
            reason: 调整原因
            **extra_data: 额外数据
        """
        history: List[Dict[str, Any]] = list(key.adjustment_history or [])

        record = {
            "timestamp": datetime.now(timezone.utc).isoformat(),
            "old_limit": old_limit,
            "new_limit": new_limit,
            "reason": reason,
            **extra_data,
        }
        history.append(record)

        # 保留最近N条记录
        if len(history) > self.MAX_HISTORY_RECORDS:
            history = history[-self.MAX_HISTORY_RECORDS:]

        key.adjustment_history = history  # type: ignore[assignment]

    def get_adjustment_stats(self, key: ProviderAPIKey) -> Dict[str, Any]:
        """
        获取调整统计信息

        Args:
            key: API Key对象

        Returns:
            统计信息
        """
        history: List[Dict[str, Any]] = list(key.adjustment_history or [])
        samples: List[Dict[str, Any]] = list(key.utilization_samples or [])

        # rpm_limit=NULL 表示自适应，否则为固定限制
        is_adaptive = key.rpm_limit is None
        current_limit = int(key.learned_rpm_limit or self.DEFAULT_INITIAL_LIMIT)
        effective_limit = current_limit if is_adaptive else int(key.rpm_limit)  # type: ignore

        # 计算窗口统计
        avg_utilization: Optional[float] = None
        high_util_ratio: Optional[float] = None
        if samples:
            avg_utilization = sum(s["util"] for s in samples) / len(samples)
            high_util_count = sum(1 for s in samples if s["util"] >= self.UTILIZATION_THRESHOLD)
            high_util_ratio = high_util_count / len(samples)

        last_429_at_str: Optional[str] = None
        if key.last_429_at:
            last_429_at_str = cast(datetime, key.last_429_at).isoformat()

        last_probe_at_str: Optional[str] = None
        if key.last_probe_increase_at:
            last_probe_at_str = cast(datetime, key.last_probe_increase_at).isoformat()

        # 边界信息
        known_boundary = key.last_rpm_peak

        return {
            "adaptive_mode": is_adaptive,
            "rpm_limit": key.rpm_limit,  # NULL=自适应，数字=固定限制
            "effective_limit": effective_limit,  # 当前有效限制
            "learned_limit": key.learned_rpm_limit,  # 学习到的限制
            # 边界记忆相关
            "known_boundary": known_boundary,  # 触发 429 时的 RPM（已知上限）
            "concurrent_429_count": int(key.concurrent_429_count or 0),
            "rpm_429_count": int(key.rpm_429_count or 0),
            "last_429_at": last_429_at_str,
            "last_429_type": key.last_429_type,
            "adjustment_count": len(history),
            "recent_adjustments": history[-5:] if history else [],
            # 滑动窗口相关
            "window_sample_count": len(samples),
            "window_avg_utilization": round(avg_utilization, 3) if avg_utilization else None,
            "window_high_util_ratio": round(high_util_ratio, 3) if high_util_ratio else None,
            "utilization_threshold": self.UTILIZATION_THRESHOLD,
            "high_util_ratio_threshold": self.HIGH_UTILIZATION_RATIO,
            "min_samples_for_decision": self.MIN_SAMPLES_FOR_DECISION,
            # 探测性扩容相关
            "last_probe_increase_at": last_probe_at_str,
            "probe_increase_interval_minutes": self.PROBE_INCREASE_INTERVAL_MINUTES,
        }

    def reset_learning(self, db: Session, key: ProviderAPIKey) -> None:
        """
        重置学习状态（管理员功能）

        Args:
            db: 数据库会话
            key: API Key对象
        """
        logger.info(f"[RESET] 重置学习状态: Key {key.id[:8]}...")

        key.learned_rpm_limit = None  # type: ignore[assignment]
        key.concurrent_429_count = 0  # type: ignore[assignment]
        key.rpm_429_count = 0  # type: ignore[assignment]
        key.last_429_at = None  # type: ignore[assignment]
        key.last_429_type = None  # type: ignore[assignment]
        key.last_rpm_peak = None  # type: ignore[assignment]
        key.adjustment_history = []  # type: ignore[assignment]
        key.utilization_samples = []  # type: ignore[assignment]
        key.last_probe_increase_at = None  # type: ignore[assignment]

        db.flush()
        get_batch_committer().mark_dirty(db)


# 全局单例
_adaptive_rpm_manager: Optional[AdaptiveRPMManager] = None


def get_adaptive_rpm_manager() -> AdaptiveRPMManager:
    """获取全局自适应 RPM 管理器单例"""
    global _adaptive_rpm_manager
    if _adaptive_rpm_manager is None:
        _adaptive_rpm_manager = AdaptiveRPMManager()
    return _adaptive_rpm_manager


# 向后兼容别名
AdaptiveConcurrencyManager = AdaptiveRPMManager
get_adaptive_manager = get_adaptive_rpm_manager
