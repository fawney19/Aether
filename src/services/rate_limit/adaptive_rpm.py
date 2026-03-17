"""
自适应 RPM 调整器 - 基于置信度衰减的 RPM 限制学习

核心算法：多次观察确认 + 置信度衰减
- 收到 429 时记录观察（本地 RPM + 上游 header 限制值）
- 多次一致的观察才确认限制（header 需 2 次，无 header 需 3 次）
- confidence 随时间自然衰减，限制永远不会固化
- confidence 低于阈值时停止本地 RPM 限制执行，让上游 429 透传

设计原则：
1. 限制永远不固化 -- confidence 需要持续的 429 观察来维持
2. 即使有上游 header 也要多次确认
3. 学习期间 429 直接透传给客户端
4. 优先使用上游 header 声明的限制值，而非本地 RPM 计数
"""

from __future__ import annotations

from datetime import datetime, timezone
from statistics import median
from typing import Any, cast

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

    核心算法：多次观察确认 + 置信度衰减
    - 收到 429 时记录观察（本地 RPM 计数 + 上游 header 限制值）
    - 有 header 的观察需 MIN_HEADER_CONFIRMATIONS 次一致才确认
    - 无 header 的观察需 MIN_CONSISTENT_OBSERVATIONS 次一致才确认
    - confidence 随时间自然衰减（CONFIDENCE_DECAY_PER_MINUTE）
    - confidence < ENFORCEMENT_CONFIDENCE_THRESHOLD 时停止本地限制执行

    扩容条件（满足任一即可）：
    1. 利用率扩容：窗口内高利用率比例 >= 60%，且当前限制 < 边界
    2. 探测性扩容：距上次 429 超过 30 分钟，可以尝试突破边界
    """

    # 默认配置
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

    # 置信度学习参数
    MIN_CONSISTENT_OBSERVATIONS = RPMDefaults.MIN_CONSISTENT_OBSERVATIONS
    MIN_HEADER_CONFIRMATIONS = RPMDefaults.MIN_HEADER_CONFIRMATIONS
    OBSERVATION_CONSISTENCY_THRESHOLD = RPMDefaults.OBSERVATION_CONSISTENCY_THRESHOLD
    HEADER_LIMIT_SAFETY_MARGIN = RPMDefaults.HEADER_LIMIT_SAFETY_MARGIN
    OBSERVATION_LIMIT_SAFETY_MARGIN = RPMDefaults.OBSERVATION_LIMIT_SAFETY_MARGIN
    ENFORCEMENT_CONFIDENCE_THRESHOLD = RPMDefaults.ENFORCEMENT_CONFIDENCE_THRESHOLD
    CONFIDENCE_DECAY_PER_MINUTE = RPMDefaults.CONFIDENCE_DECAY_PER_MINUTE

    def __init__(self, strategy: str = AdaptiveStrategy.AIMD):
        """
        初始化自适应 RPM 管理器

        Args:
            strategy: 调整策略
        """
        self.strategy = strategy

    @staticmethod
    def _persist_metadata_update(db: Session) -> None:
        """
        延后持久化自适应学习元数据，避免在请求热路径上同步 flush。

        这些更新只修改既有 ProviderAPIKey 行上的统计/学习字段，不依赖数据库生成值。
        请求级事务会在中间件统一 commit，后台会话则由 BatchCommitter 负责后续提交。
        """
        get_batch_committer().mark_dirty(db)

    # ==================== 429 处理 ====================

    def handle_429_error(
        self,
        db: Session,
        key: ProviderAPIKey,
        rate_limit_info: RateLimitInfo,
        current_rpm: int | None = None,
    ) -> int | None:
        """
        处理 429 错误，记录观察并基于一致性评估是否设置限制

        不再单次 429 就设限，而是：
        1. 记录 429 观察（本地 RPM + 上游 header 限制值）
        2. 评估历史观察的一致性
        3. 一致性达标时设置 learned_rpm_limit 并赋予 confidence
        4. 一致性不够时保持学习期（429 透传给客户端）

        Returns:
            调整后的 RPM 限制，或 None（学习期间）
        """
        is_adaptive = key.rpm_limit is None

        if not is_adaptive:
            logger.debug(f"Key {key.id} 设置了固定 RPM 限制 ({key.rpm_limit})，跳过自适应调整")
            return int(key.rpm_limit)  # type: ignore[arg-type]

        # 更新 429 统计
        key.last_429_at = datetime.now(timezone.utc)  # type: ignore[assignment]
        key.last_429_type = rate_limit_info.limit_type  # type: ignore[assignment]

        # 清空利用率采样窗口
        key.utilization_samples = []  # type: ignore[assignment]

        if rate_limit_info.limit_type == RateLimitType.RPM:
            key.rpm_429_count = int(key.rpm_429_count or 0) + 1  # type: ignore[assignment]

            upstream_limit = rate_limit_info.limit_value

            # 记录 429 观察
            self._record_429_observation(key, current_rpm, upstream_limit)

            # 评估观察一致性，决定是否设置/更新限制
            evaluated_limit, confidence = self._evaluate_observations(key)

            old_limit = key.learned_rpm_limit

            if evaluated_limit is not None and confidence >= self.ENFORCEMENT_CONFIDENCE_THRESHOLD:
                # 一致性达标，设置限制
                self._record_adjustment(
                    key,
                    old_limit=old_limit or 0,
                    new_limit=evaluated_limit,
                    reason="rpm_429",
                    current_rpm=current_rpm,
                    upstream_limit=upstream_limit,
                    confidence=round(confidence, 3),
                    learning_source="header" if upstream_limit else "observation",
                )
                key.learned_rpm_limit = evaluated_limit  # type: ignore[assignment]

                # 更新 last_rpm_peak：优先使用 upstream header
                if upstream_limit and upstream_limit > 0:
                    key.last_rpm_peak = upstream_limit  # type: ignore[assignment]
                elif current_rpm and current_rpm > 0:
                    key.last_rpm_peak = current_rpm  # type: ignore[assignment]

                logger.warning(
                    f"[RPM] 限制已确认: Key {key.id[:8]}... | "
                    f"当前 RPM: {current_rpm} | "
                    f"上游 header: {upstream_limit} | "
                    f"调整: {old_limit} -> {evaluated_limit} | "
                    f"confidence: {confidence:.2f}"
                )
            else:
                # 一致性不够，保持学习期
                logger.info(
                    f"[RPM] 学习中: Key {key.id[:8]}... | "
                    f"当前 RPM: {current_rpm} | "
                    f"上游 header: {upstream_limit} | "
                    f"观察已记录，暂不设限"
                )

        elif rate_limit_info.limit_type == RateLimitType.CONCURRENT:
            key.concurrent_429_count = int(key.concurrent_429_count or 0) + 1  # type: ignore[assignment]
            logger.info(
                f"[CONCURRENT] 并发限制触发: Key {key.id[:8]}... | "
                f"不调整 RPM 限制（这是并发问题，非 RPM 问题）"
            )

        else:
            # 未知类型：保守处理（仅在已有学习值时减少）
            old_limit = key.learned_rpm_limit
            if old_limit is not None:
                logger.warning(
                    f"[UNKNOWN] 未知429类型: Key {key.id[:8]}... | "
                    f"当前 RPM: {current_rpm} | "
                    f"保守减少 RPM: {old_limit} -> {max(int(old_limit * 0.95), self.MIN_RPM_LIMIT)}"
                )
            else:
                logger.info(
                    f"[UNKNOWN] 未知429类型: Key {key.id[:8]}... | "
                    f"当前 RPM: {current_rpm} | "
                    f"无学习值，跳过调整"
                )
            if old_limit is not None:
                new_limit = max(int(old_limit * 0.95), self.MIN_RPM_LIMIT)
                self._record_adjustment(
                    key,
                    old_limit=int(old_limit),
                    new_limit=new_limit,
                    reason="unknown_429",
                    current_rpm=current_rpm,
                )
                key.learned_rpm_limit = new_limit  # type: ignore[assignment]

        self._persist_metadata_update(db)

        return key.learned_rpm_limit if key.learned_rpm_limit is not None else None

    # ==================== 观察记录与评估 ====================

    def _record_429_observation(
        self,
        key: ProviderAPIKey,
        current_rpm: int | None,
        upstream_limit: int | None,
    ) -> None:
        """在 adjustment_history 中记录一次 429 观察"""
        history: list[dict[str, Any]] = list(key.adjustment_history or [])

        observation: dict[str, Any] = {
            "type": "429_observation",
            "timestamp": datetime.now(timezone.utc).isoformat(),
            "current_rpm": current_rpm,
            "upstream_limit": upstream_limit,
        }
        history.append(observation)

        key.adjustment_history = self._trim_history(history)  # type: ignore[assignment]

    def _evaluate_observations(self, key: ProviderAPIKey) -> tuple[int | None, float]:
        """
        评估历史 429 观察的一致性，决定是否确认限制

        优先使用有 header 的观察（upstream_limit），其次使用纯本地观察（current_rpm）。

        Returns:
            (limit, confidence):
            - limit: 新确认的限制值，或 None（一致性不够，不设/不更新限制）
            - confidence: 置信度分数 0.0~1.0
        """
        history: list[dict[str, Any]] = list(key.adjustment_history or [])
        observations = [h for h in history if h.get("type") == "429_observation"]

        if not observations:
            return None, 0.0

        # 优先评估有 header 的观察
        header_obs = [
            o
            for o in observations
            if o.get("upstream_limit") is not None and o["upstream_limit"] > 0
        ]
        if len(header_obs) >= self.MIN_HEADER_CONFIRMATIONS:
            recent = header_obs[-self.MIN_HEADER_CONFIRMATIONS * 2 :]
            values = [o["upstream_limit"] for o in recent]
            last_n = values[-self.MIN_HEADER_CONFIRMATIONS :]

            if self._check_consistency(last_n):
                limit_val = int(median(last_n) * self.HEADER_LIMIT_SAFETY_MARGIN)
                limit_val = max(limit_val, self.MIN_RPM_LIMIT)
                limit_val = min(limit_val, self.MAX_RPM_LIMIT)
                return limit_val, 0.8

        # 其次评估纯本地观察（无 header）
        local_obs = [
            o for o in observations if o.get("current_rpm") is not None and o["current_rpm"] > 0
        ]
        if len(local_obs) >= self.MIN_CONSISTENT_OBSERVATIONS:
            recent = local_obs[-self.MIN_CONSISTENT_OBSERVATIONS * 2 :]
            values = [o["current_rpm"] for o in recent]
            last_n = values[-self.MIN_CONSISTENT_OBSERVATIONS :]

            if self._check_consistency(last_n):
                limit_val = int(median(last_n) * self.OBSERVATION_LIMIT_SAFETY_MARGIN)
                limit_val = max(limit_val, self.MIN_RPM_LIMIT)
                limit_val = min(limit_val, self.MAX_RPM_LIMIT)
                return limit_val, 0.6

        # 一致性不够，不设/不更新限制（已有的 learned_rpm_limit 不在此处处理）
        return None, 0.0

    def _check_consistency(self, values: list[int]) -> bool:
        """检查一组数值是否在 OBSERVATION_CONSISTENCY_THRESHOLD 偏差范围内"""
        if not values:
            return False
        med = median(values)
        if med <= 0:
            return False
        return all(abs(v - med) / med <= self.OBSERVATION_CONSISTENCY_THRESHOLD for v in values)

    # ==================== 置信度计算 ====================

    def get_confidence(self, key: ProviderAPIKey) -> float:
        """
        计算当前 confidence 分数（0.0~1.0），包含时间衰减

        confidence 基于最后一次 429 评估的基础值，随时间自然衰减。
        确保限制永远不会固化：长时间没有新 429 观察 → confidence 降至 0。

        Returns:
            当前 confidence（0.0~1.0）
        """
        if key.learned_rpm_limit is None:
            return 0.0

        # 从历史中获取基础 confidence
        base_confidence = self._get_base_confidence(key)

        if base_confidence <= 0:
            return 0.0

        # 时间衰减
        if key.last_429_at is not None:
            last_429_at = cast(datetime, key.last_429_at)
            minutes_since = max(
                0.0, (datetime.now(timezone.utc) - last_429_at).total_seconds() / 60.0
            )
            time_decay = minutes_since * self.CONFIDENCE_DECAY_PER_MINUTE
        else:
            time_decay = 1.0  # 没有 429 记录，直接衰减到 0

        final = max(0.0, base_confidence - time_decay)
        return min(final, 1.0)

    def is_enforcement_active(self, key: ProviderAPIKey) -> bool:
        """confidence 是否达到执行阈值，达标才执行本地 RPM 限制"""
        return self.get_confidence(key) >= self.ENFORCEMENT_CONFIDENCE_THRESHOLD

    def get_effective_limit(self, key: ProviderAPIKey) -> int | None:
        """
        获取 key 当前有效的 RPM 限制（统一入口）

        - rpm_limit=NULL（自适应）：learned_rpm_limit + confidence 达标才返回
        - rpm_limit=数字（固定）：直接返回固定值
        - 其余情况返回 None（不限制）
        """
        if key.rpm_limit is not None:
            return int(key.rpm_limit)
        # 自适应模式
        if key.learned_rpm_limit is not None and self.is_enforcement_active(key):
            return int(key.learned_rpm_limit)
        return None

    def _get_base_confidence(self, key: ProviderAPIKey) -> float:
        """从最近的 adjustment 记录中获取基础 confidence"""
        history: list[dict[str, Any]] = list(key.adjustment_history or [])

        # 从最新的 adjustment 记录中查找 confidence
        for record in reversed(history):
            if record.get("type") != "429_observation" and "confidence" in record:
                return float(record["confidence"])

        # 没有 confidence 记录（旧数据迁移）：尝试从观察中重新评估
        evaluated_limit, confidence = self._evaluate_observations(key)
        if confidence > 0:
            return confidence

        # 有 learned_rpm_limit 但无法从观察中确认（旧数据），给予低基线置信度
        if key.learned_rpm_limit is not None:
            return 0.3

        return 0.0

    # ==================== 成功处理 ====================

    def handle_success(
        self,
        db: Session,
        key: ProviderAPIKey,
        current_rpm: int,
    ) -> int | None:
        """
        处理成功请求，基于滑动窗口利用率考虑增加 RPM 限制

        Returns:
            调整后的 RPM 限制（如果有调整），否则返回 None
        """
        is_adaptive = key.rpm_limit is None

        if not is_adaptive:
            return None

        # 未碰壁学习前，不主动设置限制
        if key.learned_rpm_limit is None:
            return None

        # confidence 太低时不做扩容逻辑（系统已在自由运行模式）
        confidence = self.get_confidence(key)
        if confidence < self.ENFORCEMENT_CONFIDENCE_THRESHOLD:
            return None

        current_limit = int(key.learned_rpm_limit)

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
                confidence=round(confidence, 3),
            )

            # 更新限制
            key.learned_rpm_limit = new_limit  # type: ignore[assignment]

            # 如果是探测性扩容，更新探测时间
            if is_probe:
                key.last_probe_increase_at = now  # type: ignore[assignment]

            # 扩容后清空采样窗口，重新开始收集
            key.utilization_samples = []  # type: ignore[assignment]

            self._persist_metadata_update(db)

            return new_limit

        # 定期持久化采样数据（每5个采样保存一次）
        if len(samples) % 5 == 0:
            self._persist_metadata_update(db)

        return None

    # ==================== 滑动窗口 ====================

    def _update_utilization_window(
        self, key: ProviderAPIKey, now_ts: float, utilization: float
    ) -> list[dict[str, Any]]:
        """更新利用率滑动窗口"""
        samples: list[dict[str, Any]] = list(key.utilization_samples or [])

        samples.append({"ts": now_ts, "util": round(utilization, 3)})

        cutoff_ts = now_ts - self.UTILIZATION_WINDOW_SECONDS
        samples = [s for s in samples if s["ts"] > cutoff_ts]

        if len(samples) > self.UTILIZATION_WINDOW_SIZE:
            samples = samples[-self.UTILIZATION_WINDOW_SIZE :]

        key.utilization_samples = samples  # type: ignore[assignment]
        return samples

    # ==================== 扩容条件 ====================

    def _check_increase_conditions(
        self,
        key: ProviderAPIKey,
        samples: list[dict[str, Any]],
        now: datetime,
        known_boundary: int | None = None,
    ) -> str | None:
        """检查是否满足扩容条件"""
        if self._is_in_cooldown(key):
            return None

        current_limit = int(key.learned_rpm_limit or self.DEFAULT_INITIAL_LIMIT)

        # 条件1：滑动窗口扩容（不超过边界）
        if len(samples) >= self.MIN_SAMPLES_FOR_DECISION:
            high_util_count = sum(1 for s in samples if s["util"] >= self.UTILIZATION_THRESHOLD)
            high_util_ratio = high_util_count / len(samples)

            if high_util_ratio >= self.HIGH_UTILIZATION_RATIO:
                if known_boundary:
                    if current_limit < known_boundary:
                        return "high_utilization"
                else:
                    return "high_utilization"

        # 条件2：探测性扩容
        if self._should_probe_increase(key, samples, now):
            return "probe_increase"

        return None

    def _should_probe_increase(
        self, key: ProviderAPIKey, samples: list[dict[str, Any]], now: datetime
    ) -> bool:
        """检查是否应该进行探测性扩容"""
        probe_interval_seconds = self.PROBE_INCREASE_INTERVAL_MINUTES * 60

        if key.last_429_at:
            last_429_at = cast(datetime, key.last_429_at)
            time_since_429 = (now - last_429_at).total_seconds()
            if time_since_429 < probe_interval_seconds:
                return False

        if key.last_probe_increase_at:
            last_probe = cast(datetime, key.last_probe_increase_at)
            time_since_probe = (now - last_probe).total_seconds()
            if time_since_probe < probe_interval_seconds:
                return False

        if len(samples) < self.PROBE_INCREASE_MIN_REQUESTS:
            return False

        avg_util = sum(s["util"] for s in samples) / len(samples)
        if avg_util < 0.3:
            return False

        return True

    def _is_in_cooldown(self, key: ProviderAPIKey) -> bool:
        """检查是否在 429 错误后的冷却期内"""
        if key.last_429_at is None:
            return False

        last_429_at = cast(datetime, key.last_429_at)
        time_since_429 = (datetime.now(timezone.utc) - last_429_at).total_seconds()
        cooldown_seconds = RPMDefaults.COOLDOWN_AFTER_429_MINUTES * 60

        return bool(time_since_429 < cooldown_seconds)

    # ==================== 限制调整 ====================

    def _increase_limit(
        self,
        current_limit: int,
        known_boundary: int | None = None,
        is_probe: bool = False,
    ) -> int:
        """增加 RPM 限制（考虑边界保护）"""
        if is_probe:
            new_limit = current_limit + 1
        else:
            new_limit = current_limit + self.INCREASE_STEP
            if known_boundary:
                if new_limit > known_boundary:
                    new_limit = known_boundary

        new_limit = min(new_limit, self.MAX_RPM_LIMIT)

        if new_limit <= current_limit:
            return current_limit

        return new_limit

    # ==================== 历史记录 ====================

    def _record_adjustment(
        self,
        key: ProviderAPIKey,
        old_limit: int,
        new_limit: int,
        reason: str,
        **extra_data: Any,
    ) -> None:
        """记录 RPM 调整历史"""
        history: list[dict[str, Any]] = list(key.adjustment_history or [])

        record = {
            "timestamp": datetime.now(timezone.utc).isoformat(),
            "old_limit": old_limit,
            "new_limit": new_limit,
            "reason": reason,
            **extra_data,
        }
        history.append(record)

        key.adjustment_history = self._trim_history(history)  # type: ignore[assignment]

    def _trim_history(self, history: list[dict[str, Any]]) -> list[dict[str, Any]]:
        """
        截断历史记录，优先保留 429_observation（学习数据源）

        策略：超出 MAX_HISTORY_RECORDS 时，先淘汰最旧的非观察记录，
        仍超限则淘汰最旧的观察记录。
        """
        if len(history) <= self.MAX_HISTORY_RECORDS:
            return history

        observations = [h for h in history if h.get("type") == "429_observation"]
        adjustments = [h for h in history if h.get("type") != "429_observation"]

        # 按时间戳排序（最新在后）
        observations.sort(key=lambda h: h.get("timestamp", ""))
        adjustments.sort(key=lambda h: h.get("timestamp", ""))

        # 保留尽可能多的观察记录：先缩减 adjustment，再缩减 observation
        overflow = len(history) - self.MAX_HISTORY_RECORDS
        trim_adj = min(overflow, len(adjustments))
        adjustments = adjustments[trim_adj:]
        overflow -= trim_adj

        if overflow > 0:
            observations = observations[overflow:]

        merged = observations + adjustments
        merged.sort(key=lambda h: h.get("timestamp", ""))
        return merged

    # ==================== 统计与管理 ====================

    def get_adjustment_stats(self, key: ProviderAPIKey) -> dict[str, Any]:
        """获取调整统计信息"""
        history: list[dict[str, Any]] = list(key.adjustment_history or [])
        samples: list[dict[str, Any]] = list(key.utilization_samples or [])

        is_adaptive = key.rpm_limit is None
        effective_limit = self.get_effective_limit(key) if is_adaptive else int(key.rpm_limit)  # type: ignore

        avg_utilization: float | None = None
        high_util_ratio: float | None = None
        if samples:
            avg_utilization = sum(s["util"] for s in samples) / len(samples)
            high_util_count = sum(1 for s in samples if s["util"] >= self.UTILIZATION_THRESHOLD)
            high_util_ratio = high_util_count / len(samples)

        last_429_at_str: str | None = None
        if key.last_429_at:
            last_429_at_str = cast(datetime, key.last_429_at).isoformat()

        last_probe_at_str: str | None = None
        if key.last_probe_increase_at:
            last_probe_at_str = cast(datetime, key.last_probe_increase_at).isoformat()

        known_boundary = key.last_rpm_peak

        # 观察统计
        observations = [h for h in history if h.get("type") == "429_observation"]
        header_observations = [
            o
            for o in observations
            if o.get("upstream_limit") is not None and o["upstream_limit"] > 0
        ]
        latest_upstream = header_observations[-1]["upstream_limit"] if header_observations else None

        confidence = self.get_confidence(key) if is_adaptive else None
        enforcement_active = (
            confidence >= self.ENFORCEMENT_CONFIDENCE_THRESHOLD if confidence is not None else None
        )

        return {
            "adaptive_mode": is_adaptive,
            "rpm_limit": key.rpm_limit,
            "effective_limit": effective_limit,
            "learned_limit": key.learned_rpm_limit,
            # 边界记忆相关
            "known_boundary": known_boundary,
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
            # 置信度相关
            "learning_confidence": round(confidence, 3) if confidence is not None else None,
            "enforcement_active": enforcement_active,
            "observation_count": len(observations),
            "header_observation_count": len(header_observations),
            "latest_upstream_limit": latest_upstream,
        }

    def reset_learning(self, db: Session, key: ProviderAPIKey) -> None:
        """重置学习状态（管理员功能）"""
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

        self._persist_metadata_update(db)


# 全局单例
_adaptive_rpm_manager: AdaptiveRPMManager | None = None


def get_adaptive_rpm_manager() -> AdaptiveRPMManager:
    """获取全局自适应 RPM 管理器单例"""
    global _adaptive_rpm_manager
    if _adaptive_rpm_manager is None:
        _adaptive_rpm_manager = AdaptiveRPMManager()
    return _adaptive_rpm_manager


# 向后兼容别名
AdaptiveConcurrencyManager = AdaptiveRPMManager
get_adaptive_manager = get_adaptive_rpm_manager
