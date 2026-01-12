"""
健康监控器 - Endpoint 和 Key 的健康度追踪（按 API 格式区分）

功能：
1. 基于滑动窗口的错误率计算（按 API 格式独立）
2. 三态熔断器：关闭 -> 打开 -> 半开 -> 关闭（按 API 格式独立）
3. 半开状态允许少量请求验证服务恢复
4. 提供健康度查询和管理 API

数据结构：
- health_by_format: {"CLAUDE": {"health_score": 1.0, "consecutive_failures": 0, ...}, ...}
- circuit_breaker_by_format: {"CLAUDE": {"open": false, "open_at": null, ...}, ...}
"""

import os
from datetime import datetime, timedelta, timezone
from typing import Any, Dict, List, Optional, Tuple

from sqlalchemy import case, func
from sqlalchemy.orm import Session

from src.config.constants import CircuitBreakerDefaults
from src.core.batch_committer import get_batch_committer
from src.core.logger import logger
from src.core.metrics import health_open_circuits
from src.models.database import ProviderAPIKey, ProviderEndpoint


class CircuitState:
    """熔断器状态"""

    CLOSED = "closed"  # 关闭（正常）
    OPEN = "open"  # 打开（熔断）
    HALF_OPEN = "half_open"  # 半开（验证恢复）


# 默认健康度数据结构
def _default_health_data() -> Dict[str, Any]:
    return {
        "health_score": 1.0,
        "consecutive_failures": 0,
        "last_failure_at": None,
        "request_results_window": [],
    }


# 默认熔断器数据结构
def _default_circuit_data() -> Dict[str, Any]:
    return {
        "open": False,
        "open_at": None,
        "next_probe_at": None,
        "half_open_until": None,
        "half_open_successes": 0,
        "half_open_failures": 0,
    }


class HealthMonitor:
    """健康监控器（滑动窗口 + 半开状态模式，按 API 格式区分）"""

    # === 滑动窗口配置 ===
    WINDOW_SIZE = int(os.getenv("HEALTH_WINDOW_SIZE", str(CircuitBreakerDefaults.WINDOW_SIZE)))
    WINDOW_SECONDS = int(
        os.getenv("HEALTH_WINDOW_SECONDS", str(CircuitBreakerDefaults.WINDOW_SECONDS))
    )
    MIN_REQUESTS = int(
        os.getenv("HEALTH_MIN_REQUESTS", str(CircuitBreakerDefaults.MIN_REQUESTS_FOR_DECISION))
    )
    ERROR_RATE_THRESHOLD = float(
        os.getenv("HEALTH_ERROR_RATE_THRESHOLD", str(CircuitBreakerDefaults.ERROR_RATE_THRESHOLD))
    )

    # === 半开状态配置 ===
    HALF_OPEN_DURATION = int(
        os.getenv(
            "HEALTH_HALF_OPEN_DURATION", str(CircuitBreakerDefaults.HALF_OPEN_DURATION_SECONDS)
        )
    )
    HALF_OPEN_SUCCESS_THRESHOLD = int(
        os.getenv(
            "HEALTH_HALF_OPEN_SUCCESS", str(CircuitBreakerDefaults.HALF_OPEN_SUCCESS_THRESHOLD)
        )
    )
    HALF_OPEN_FAILURE_THRESHOLD = int(
        os.getenv(
            "HEALTH_HALF_OPEN_FAILURE", str(CircuitBreakerDefaults.HALF_OPEN_FAILURE_THRESHOLD)
        )
    )

    # === 恢复配置 ===
    INITIAL_RECOVERY_SECONDS = int(
        os.getenv(
            "HEALTH_INITIAL_RECOVERY_SECONDS", str(CircuitBreakerDefaults.INITIAL_RECOVERY_SECONDS)
        )
    )
    RECOVERY_BACKOFF = int(
        os.getenv(
            "HEALTH_RECOVERY_BACKOFF", str(CircuitBreakerDefaults.RECOVERY_BACKOFF_MULTIPLIER)
        )
    )
    MAX_RECOVERY_SECONDS = int(
        os.getenv("HEALTH_MAX_RECOVERY_SECONDS", str(CircuitBreakerDefaults.MAX_RECOVERY_SECONDS))
    )

    # === 兼容旧参数（用于健康度展示）===
    SUCCESS_INCREMENT = float(
        os.getenv("HEALTH_SUCCESS_INCREMENT", str(CircuitBreakerDefaults.SUCCESS_INCREMENT))
    )
    FAILURE_DECREMENT = float(
        os.getenv("HEALTH_FAILURE_DECREMENT", str(CircuitBreakerDefaults.FAILURE_DECREMENT))
    )
    PROBE_RECOVERY_SCORE = float(
        os.getenv("HEALTH_PROBE_RECOVERY_SCORE", str(CircuitBreakerDefaults.PROBE_RECOVERY_SCORE))
    )

    # === 其他配置 ===
    ALLOW_AUTO_RECOVER = os.getenv("HEALTH_AUTO_RECOVER_ENABLED", "true").lower() == "true"
    CIRCUIT_HISTORY_LIMIT = int(os.getenv("HEALTH_CIRCUIT_HISTORY_LIMIT", "200"))

    # 进程级别状态缓存
    _circuit_history: List[Dict[str, Any]] = []
    _open_circuit_keys: int = 0

    # ==================== 数据访问辅助方法 ====================

    @classmethod
    def _get_health_data(cls, key: ProviderAPIKey, api_format: str) -> Dict[str, Any]:
        """获取指定格式的健康度数据，不存在则返回默认值"""
        health_by_format = key.health_by_format or {}
        if api_format not in health_by_format:
            return _default_health_data()
        return health_by_format[api_format]

    @classmethod
    def _set_health_data(cls, key: ProviderAPIKey, api_format: str, data: Dict[str, Any]) -> None:
        """设置指定格式的健康度数据"""
        health_by_format = dict(key.health_by_format or {})
        health_by_format[api_format] = data
        key.health_by_format = health_by_format  # type: ignore[assignment]

    @classmethod
    def _get_circuit_data(cls, key: ProviderAPIKey, api_format: str) -> Dict[str, Any]:
        """获取指定格式的熔断器数据，不存在则返回默认值"""
        circuit_by_format = key.circuit_breaker_by_format or {}
        if api_format not in circuit_by_format:
            return _default_circuit_data()
        return circuit_by_format[api_format]

    @classmethod
    def _set_circuit_data(cls, key: ProviderAPIKey, api_format: str, data: Dict[str, Any]) -> None:
        """设置指定格式的熔断器数据"""
        circuit_by_format = dict(key.circuit_breaker_by_format or {})
        circuit_by_format[api_format] = data
        key.circuit_breaker_by_format = circuit_by_format  # type: ignore[assignment]

    # ==================== 核心方法 ====================

    @classmethod
    def record_success(
        cls,
        db: Session,
        key_id: Optional[str] = None,
        api_format: Optional[str] = None,
        response_time_ms: Optional[int] = None,
    ) -> None:
        """记录成功请求（按 API 格式）

        Args:
            db: 数据库会话
            key_id: Key ID（必需）
            api_format: API 格式（必需，用于区分不同格式的健康度）
            response_time_ms: 响应时间（可选）

        Note:
            api_format 在逻辑上是必需的，但为了向后兼容保持 Optional 签名。
            如果未提供，会尝试从 Key 的 api_formats 中获取第一个格式作为 fallback。
        """
        try:
            if not key_id:
                return

            key = db.query(ProviderAPIKey).filter(ProviderAPIKey.id == key_id).first()
            if not key:
                return

            # api_format 兼容处理：如果未提供，尝试使用 Key 的第一个格式
            effective_api_format = api_format
            if not effective_api_format:
                if key.api_formats and len(key.api_formats) > 0:
                    effective_api_format = key.api_formats[0]
                    logger.debug(
                        f"record_success: api_format 未提供，使用默认格式 {effective_api_format}"
                    )
                else:
                    logger.warning(
                        f"record_success: api_format 未提供且 Key 无可用格式: key_id={key_id[:8]}..."
                    )
                    return

            now = datetime.now(timezone.utc)
            now_ts = now.timestamp()

            # 获取当前格式的健康度数据
            health_data = cls._get_health_data(key, effective_api_format)
            circuit_data = cls._get_circuit_data(key, effective_api_format)

            # 1. 更新滑动窗口
            window = health_data.get("request_results_window") or []
            window.append({"ts": now_ts, "ok": True})
            cutoff_ts = now_ts - cls.WINDOW_SECONDS
            window = [r for r in window if r["ts"] > cutoff_ts]
            if len(window) > cls.WINDOW_SIZE:
                window = window[-cls.WINDOW_SIZE :]
            health_data["request_results_window"] = window

            # 2. 更新健康度（用于展示）
            current_score = float(health_data.get("health_score") or 0)
            new_score = min(current_score + cls.SUCCESS_INCREMENT, 1.0)
            health_data["health_score"] = new_score

            # 3. 更新统计
            health_data["consecutive_failures"] = 0
            health_data["last_failure_at"] = None

            # 4. 处理熔断器状态
            state = cls._get_circuit_state_from_data(circuit_data, now)

            if state == CircuitState.HALF_OPEN:
                # 半开状态：记录成功
                circuit_data["half_open_successes"] = int(
                    circuit_data.get("half_open_successes") or 0
                ) + 1

                if circuit_data["half_open_successes"] >= cls.HALF_OPEN_SUCCESS_THRESHOLD:
                    # 达到成功阈值，关闭熔断器
                    cls._close_circuit_data(circuit_data, health_data, reason="半开状态验证成功")
                    cls._push_circuit_event(
                        {
                            "event": "closed",
                            "key_id": key.id,
                            "api_format": effective_api_format,
                            "reason": "半开状态验证成功",
                            "timestamp": now.isoformat(),
                        }
                    )
                    logger.info(
                        f"[CLOSED] Key 熔断器关闭: {key.id[:8]}.../{effective_api_format} | 原因: 半开状态验证成功"
                    )

            elif state == CircuitState.OPEN:
                # 打开状态下的成功（探测成功），进入半开状态
                cls._enter_half_open_data(circuit_data, now)
                cls._push_circuit_event(
                    {
                        "event": "half_open",
                        "key_id": key.id,
                        "api_format": effective_api_format,
                        "timestamp": now.isoformat(),
                    }
                )
                logger.info(
                    f"[HALF-OPEN] Key 进入半开状态: {key.id[:8]}.../{effective_api_format} | "
                    f"需要 {cls.HALF_OPEN_SUCCESS_THRESHOLD} 次成功关闭熔断器"
                )

            # 保存数据
            cls._set_health_data(key, effective_api_format, health_data)
            cls._set_circuit_data(key, effective_api_format, circuit_data)

            # 更新全局统计
            key.success_count = int(key.success_count or 0) + 1  # type: ignore[assignment]
            key.request_count = int(key.request_count or 0) + 1  # type: ignore[assignment]
            if response_time_ms:
                key.total_response_time_ms = int(key.total_response_time_ms or 0) + response_time_ms  # type: ignore[assignment]

            db.flush()
            get_batch_committer().mark_dirty(db)

        except Exception as e:
            logger.error(f"记录成功请求失败: {e}")
            db.rollback()

    @classmethod
    def record_failure(
        cls,
        db: Session,
        key_id: Optional[str] = None,
        api_format: Optional[str] = None,
        error_type: Optional[str] = None,
    ) -> None:
        """记录失败请求（按 API 格式）

        Args:
            db: 数据库会话
            key_id: Key ID（必需）
            api_format: API 格式（必需，用于区分不同格式的健康度）
            error_type: 错误类型（可选）

        Note:
            api_format 在逻辑上是必需的，但为了向后兼容保持 Optional 签名。
            如果未提供，会尝试从 Key 的 api_formats 中获取第一个格式作为 fallback。
        """
        try:
            if not key_id:
                return

            key = db.query(ProviderAPIKey).filter(ProviderAPIKey.id == key_id).first()
            if not key:
                return

            # api_format 兼容处理：如果未提供，尝试使用 Key 的第一个格式
            effective_api_format = api_format
            if not effective_api_format:
                if key.api_formats and len(key.api_formats) > 0:
                    effective_api_format = key.api_formats[0]
                    logger.debug(
                        f"record_failure: api_format 未提供，使用默认格式 {effective_api_format}"
                    )
                else:
                    logger.warning(
                        f"record_failure: api_format 未提供且 Key 无可用格式: key_id={key_id[:8]}..."
                    )
                    return

            now = datetime.now(timezone.utc)
            now_ts = now.timestamp()

            # 获取当前格式的健康度数据
            health_data = cls._get_health_data(key, effective_api_format)
            circuit_data = cls._get_circuit_data(key, effective_api_format)

            # 1. 更新滑动窗口
            window = health_data.get("request_results_window") or []
            window.append({"ts": now_ts, "ok": False})
            cutoff_ts = now_ts - cls.WINDOW_SECONDS
            window = [r for r in window if r["ts"] > cutoff_ts]
            if len(window) > cls.WINDOW_SIZE:
                window = window[-cls.WINDOW_SIZE :]
            health_data["request_results_window"] = window

            # 2. 更新健康度（用于展示）
            current_score = float(health_data.get("health_score") or 1)
            new_score = max(current_score - cls.FAILURE_DECREMENT, 0.0)
            health_data["health_score"] = new_score

            # 3. 更新统计
            health_data["consecutive_failures"] = (
                int(health_data.get("consecutive_failures") or 0) + 1
            )
            health_data["last_failure_at"] = now.isoformat()

            # 4. 处理熔断器状态
            state = cls._get_circuit_state_from_data(circuit_data, now)

            if state == CircuitState.HALF_OPEN:
                # 半开状态：记录失败
                circuit_data["half_open_failures"] = int(
                    circuit_data.get("half_open_failures") or 0
                ) + 1

                if circuit_data["half_open_failures"] >= cls.HALF_OPEN_FAILURE_THRESHOLD:
                    # 达到失败阈值，重新打开熔断器
                    # 注意：半开状态本身就是打开状态的子状态，不需要增加计数
                    consecutive = int(health_data.get("consecutive_failures") or 0)
                    recovery_seconds = cls._calculate_recovery_seconds(consecutive)
                    cls._open_circuit_data(
                        circuit_data, now, recovery_seconds, reason="半开状态验证失败"
                    )
                    cls._push_circuit_event(
                        {
                            "event": "opened",
                            "key_id": key.id,
                            "api_format": effective_api_format,
                            "reason": "半开状态验证失败",
                            "recovery_seconds": recovery_seconds,
                            "timestamp": now.isoformat(),
                        }
                    )
                    logger.warning(
                        f"[OPEN] Key 熔断器打开: {key.id[:8]}.../{effective_api_format} | 原因: 半开状态验证失败 | "
                        f"{recovery_seconds}秒后进入半开状态"
                    )

            elif state == CircuitState.CLOSED:
                # 关闭状态：检查是否需要打开熔断器
                error_rate = cls._calculate_error_rate_from_window(window, now_ts)

                if len(window) >= cls.MIN_REQUESTS and error_rate >= cls.ERROR_RATE_THRESHOLD:
                    consecutive = int(health_data.get("consecutive_failures") or 0)
                    recovery_seconds = cls._calculate_recovery_seconds(consecutive)
                    reason = f"错误率 {error_rate:.0%} 超过阈值 {cls.ERROR_RATE_THRESHOLD:.0%}"
                    cls._open_circuit_data(circuit_data, now, recovery_seconds, reason=reason)
                    cls._open_circuit_keys += 1
                    health_open_circuits.set(cls._open_circuit_keys)
                    cls._push_circuit_event(
                        {
                            "event": "opened",
                            "key_id": key.id,
                            "api_format": effective_api_format,
                            "reason": reason,
                            "recovery_seconds": recovery_seconds,
                            "timestamp": now.isoformat(),
                        }
                    )
                    logger.warning(
                        f"[OPEN] Key 熔断器打开: {key.id[:8]}.../{effective_api_format} | 原因: {reason} | "
                        f"{recovery_seconds}秒后进入半开状态"
                    )

            # 保存数据
            cls._set_health_data(key, effective_api_format, health_data)
            cls._set_circuit_data(key, effective_api_format, circuit_data)

            # 更新全局统计
            key.error_count = int(key.error_count or 0) + 1  # type: ignore[assignment]
            key.request_count = int(key.request_count or 0) + 1  # type: ignore[assignment]
            key.last_error_at = now  # type: ignore[assignment]

            logger.debug(
                f"[WARN] Key 健康度下降: {key_id[:8]}.../{effective_api_format} -> {new_score:.2f} "
                f"(连续失败 {health_data['consecutive_failures']} 次, error_type={error_type})"
            )

            db.flush()
            get_batch_committer().mark_dirty(db)

        except Exception as e:
            logger.error(f"记录失败请求失败: {e}")
            db.rollback()

    # ==================== 滑动窗口方法 ====================

    @classmethod
    def _calculate_error_rate_from_window(
        cls, window: List[Dict[str, Any]], now_ts: float
    ) -> float:
        """从窗口数据计算错误率"""
        if not window:
            return 0.0

        cutoff_ts = now_ts - cls.WINDOW_SECONDS
        valid_records = [r for r in window if r["ts"] > cutoff_ts]

        if not valid_records:
            return 0.0

        failures = sum(1 for r in valid_records if not r["ok"])
        return failures / len(valid_records)

    # ==================== 熔断器状态方法（操作数据字典）====================

    @classmethod
    def _get_circuit_state_from_data(cls, circuit_data: Dict[str, Any], now: datetime) -> str:
        """从数据字典获取当前熔断器状态"""
        if not circuit_data.get("open"):
            return CircuitState.CLOSED

        # 检查是否在半开状态
        half_open_until_str = circuit_data.get("half_open_until")
        if half_open_until_str:
            half_open_until = datetime.fromisoformat(half_open_until_str)
            if now < half_open_until:
                return CircuitState.HALF_OPEN

        # 检查是否到了探测时间（进入半开）
        next_probe_str = circuit_data.get("next_probe_at")
        if next_probe_str:
            next_probe_at = datetime.fromisoformat(next_probe_str)
            if now >= next_probe_at:
                return CircuitState.HALF_OPEN

        return CircuitState.OPEN

    @classmethod
    def _open_circuit_data(
        cls,
        circuit_data: Dict[str, Any],
        now: datetime,
        recovery_seconds: int,
        reason: str,
    ) -> None:
        """打开熔断器（操作数据字典）"""
        circuit_data["open"] = True
        circuit_data["open_at"] = now.isoformat()
        circuit_data["half_open_until"] = None
        circuit_data["half_open_successes"] = 0
        circuit_data["half_open_failures"] = 0
        circuit_data["next_probe_at"] = (now + timedelta(seconds=recovery_seconds)).isoformat()

    @classmethod
    def _enter_half_open_data(cls, circuit_data: Dict[str, Any], now: datetime) -> None:
        """进入半开状态（操作数据字典）"""
        circuit_data["half_open_until"] = (
            now + timedelta(seconds=cls.HALF_OPEN_DURATION)
        ).isoformat()
        circuit_data["half_open_successes"] = 0
        circuit_data["half_open_failures"] = 0

    @classmethod
    def _close_circuit_data(
        cls, circuit_data: Dict[str, Any], health_data: Dict[str, Any], reason: str
    ) -> None:
        """关闭熔断器（操作数据字典）"""
        circuit_data["open"] = False
        circuit_data["open_at"] = None
        circuit_data["next_probe_at"] = None
        circuit_data["half_open_until"] = None
        circuit_data["half_open_successes"] = 0
        circuit_data["half_open_failures"] = 0

        # 快速恢复健康度
        current_score = float(health_data.get("health_score") or 0)
        health_data["health_score"] = max(current_score, cls.PROBE_RECOVERY_SCORE)

        cls._open_circuit_keys = max(0, cls._open_circuit_keys - 1)
        health_open_circuits.set(cls._open_circuit_keys)

    @classmethod
    def _calculate_recovery_seconds(cls, consecutive_failures: int) -> int:
        """计算恢复等待时间（指数退避）"""
        exponent = min(consecutive_failures // 5, 4)
        seconds = cls.INITIAL_RECOVERY_SECONDS * (cls.RECOVERY_BACKOFF**exponent)
        return min(int(seconds), cls.MAX_RECOVERY_SECONDS)

    # ==================== 状态查询方法 ====================

    @classmethod
    def is_circuit_breaker_closed(
        cls, resource: ProviderAPIKey, api_format: Optional[str] = None
    ) -> bool:
        """检查熔断器是否允许请求通过（按 API 格式）"""
        if not api_format:
            # 兼容旧调用：检查是否有任何格式的熔断器开启
            circuit_by_format = resource.circuit_breaker_by_format or {}
            for fmt, circuit_data in circuit_by_format.items():
                if circuit_data.get("open"):
                    return False
            return True

        circuit_data = cls._get_circuit_data(resource, api_format)

        if not circuit_data.get("open"):
            return True

        now = datetime.now(timezone.utc)
        state = cls._get_circuit_state_from_data(circuit_data, now)

        # 半开状态允许请求通过
        if state == CircuitState.HALF_OPEN:
            return True

        # 检查是否到了探测时间
        next_probe_str = circuit_data.get("next_probe_at")
        if next_probe_str:
            next_probe_at = datetime.fromisoformat(next_probe_str)
            if now >= next_probe_at:
                # 自动进入半开状态
                cls._enter_half_open_data(circuit_data, now)
                cls._set_circuit_data(resource, api_format, circuit_data)
                return True

        return False

    @classmethod
    def get_circuit_breaker_status(
        cls, resource: ProviderAPIKey, api_format: Optional[str] = None
    ) -> Tuple[bool, Optional[str]]:
        """获取熔断器详细状态（按 API 格式）"""
        if not api_format:
            # 兼容旧调用：返回第一个开启的熔断器状态
            circuit_by_format = resource.circuit_breaker_by_format or {}
            for fmt, circuit_data in circuit_by_format.items():
                if circuit_data.get("open"):
                    return cls._get_status_from_circuit_data(circuit_data)
            return True, None

        circuit_data = cls._get_circuit_data(resource, api_format)
        return cls._get_status_from_circuit_data(circuit_data)

    @classmethod
    def _get_status_from_circuit_data(
        cls, circuit_data: Dict[str, Any]
    ) -> Tuple[bool, Optional[str]]:
        """从熔断器数据获取状态描述"""
        if not circuit_data.get("open"):
            return True, None

        now = datetime.now(timezone.utc)
        state = cls._get_circuit_state_from_data(circuit_data, now)

        if state == CircuitState.HALF_OPEN:
            successes = int(circuit_data.get("half_open_successes") or 0)
            return True, f"半开状态({successes}/{cls.HALF_OPEN_SUCCESS_THRESHOLD}成功)"

        next_probe_str = circuit_data.get("next_probe_at")
        if next_probe_str:
            next_probe_at = datetime.fromisoformat(next_probe_str)
            if now >= next_probe_at:
                return True, None

            remaining = next_probe_at - now
            remaining_seconds = int(remaining.total_seconds())
            if remaining_seconds >= 60:
                time_str = f"{remaining_seconds // 60}min{remaining_seconds % 60}s"
            else:
                time_str = f"{remaining_seconds}s"
            return False, f"熔断中({time_str}后半开)"

        return False, "熔断中"

    @classmethod
    def get_key_health(
        cls, db: Session, key_id: str, api_format: Optional[str] = None
    ) -> Optional[Dict[str, Any]]:
        """获取 Key 健康状态（支持按格式查询）"""
        try:
            key = db.query(ProviderAPIKey).filter(ProviderAPIKey.id == key_id).first()
            if not key:
                return None

            now = datetime.now(timezone.utc)
            now_ts = now.timestamp()

            avg_response_time_ms = (
                int(key.total_response_time_ms or 0) / int(key.success_count or 1)
                if key.success_count
                else 0
            )

            # 全局统计
            result = {
                "key_id": key.id,
                "is_active": key.is_active,
                "statistics": {
                    "request_count": int(key.request_count or 0),
                    "success_count": int(key.success_count or 0),
                    "error_count": int(key.error_count or 0),
                    "success_rate": (
                        int(key.success_count or 0) / int(key.request_count or 1)
                        if key.request_count
                        else 0.0
                    ),
                    "avg_response_time_ms": round(avg_response_time_ms, 2),
                },
            }

            # 按格式的健康度数据
            health_by_format = key.health_by_format or {}
            circuit_by_format = key.circuit_breaker_by_format or {}

            if api_format:
                # 查询单个格式
                health_data = cls._get_health_data(key, api_format)
                circuit_data = cls._get_circuit_data(key, api_format)
                window = health_data.get("request_results_window") or []
                valid_window = [r for r in window if r["ts"] > now_ts - cls.WINDOW_SECONDS]

                result["api_format"] = api_format
                result["health_score"] = float(health_data.get("health_score") or 1.0)
                result["error_rate"] = cls._calculate_error_rate_from_window(window, now_ts)
                result["window_size"] = len(valid_window)
                result["consecutive_failures"] = int(
                    health_data.get("consecutive_failures") or 0
                )
                result["last_failure_at"] = health_data.get("last_failure_at")
                result["circuit_breaker"] = {
                    "state": cls._get_circuit_state_from_data(circuit_data, now),
                    "open": circuit_data.get("open", False),
                    "open_at": circuit_data.get("open_at"),
                    "next_probe_at": circuit_data.get("next_probe_at"),
                    "half_open_until": circuit_data.get("half_open_until"),
                    "half_open_successes": int(circuit_data.get("half_open_successes") or 0),
                    "half_open_failures": int(circuit_data.get("half_open_failures") or 0),
                }
            else:
                # 返回所有格式的健康度数据
                formats_health = {}
                for fmt in (key.api_formats or []):
                    health_data = health_by_format.get(fmt, _default_health_data())
                    circuit_data = circuit_by_format.get(fmt, _default_circuit_data())
                    window = health_data.get("request_results_window") or []
                    valid_window = [r for r in window if r["ts"] > now_ts - cls.WINDOW_SECONDS]

                    formats_health[fmt] = {
                        "health_score": float(health_data.get("health_score") or 1.0),
                        "error_rate": cls._calculate_error_rate_from_window(window, now_ts),
                        "window_size": len(valid_window),
                        "consecutive_failures": int(
                            health_data.get("consecutive_failures") or 0
                        ),
                        "last_failure_at": health_data.get("last_failure_at"),
                        "circuit_breaker": {
                            "state": cls._get_circuit_state_from_data(circuit_data, now),
                            "open": circuit_data.get("open", False),
                            "open_at": circuit_data.get("open_at"),
                            "next_probe_at": circuit_data.get("next_probe_at"),
                            "half_open_until": circuit_data.get("half_open_until"),
                            "half_open_successes": int(
                                circuit_data.get("half_open_successes") or 0
                            ),
                            "half_open_failures": int(
                                circuit_data.get("half_open_failures") or 0
                            ),
                        },
                    }

                result["health_by_format"] = formats_health

                # 计算整体健康度（取最低值）
                if formats_health:
                    result["health_score"] = min(
                        h["health_score"] for h in formats_health.values()
                    )
                    result["any_circuit_open"] = any(
                        h["circuit_breaker"]["open"] for h in formats_health.values()
                    )
                else:
                    result["health_score"] = 1.0
                    result["any_circuit_open"] = False

            return result

        except Exception as e:
            logger.error(f"获取 Key 健康状态失败: {e}")
            return None

    @classmethod
    def get_endpoint_health(cls, db: Session, endpoint_id: str) -> Optional[Dict[str, Any]]:
        """获取 Endpoint 健康状态"""
        try:
            endpoint = (
                db.query(ProviderEndpoint).filter(ProviderEndpoint.id == endpoint_id).first()
            )
            if not endpoint:
                return None

            return {
                "endpoint_id": endpoint.id,
                "health_score": float(endpoint.health_score or 1.0),
                "consecutive_failures": int(endpoint.consecutive_failures or 0),
                "last_failure_at": (
                    endpoint.last_failure_at.isoformat() if endpoint.last_failure_at else None
                ),
                "is_active": endpoint.is_active,
            }

        except Exception as e:
            logger.error(f"获取 Endpoint 健康状态失败: {e}")
            return None

    # ==================== 管理方法 ====================

    @classmethod
    def reset_health(
        cls, db: Session, key_id: Optional[str] = None, api_format: Optional[str] = None
    ) -> bool:
        """重置健康度（支持按格式重置）"""
        try:
            if key_id:
                key = db.query(ProviderAPIKey).filter(ProviderAPIKey.id == key_id).first()
                if key:
                    if api_format:
                        # 重置单个格式
                        cls._set_health_data(key, api_format, _default_health_data())
                        cls._set_circuit_data(key, api_format, _default_circuit_data())
                        logger.info(f"[RESET] 重置 Key 健康度: {key_id}/{api_format}")
                    else:
                        # 重置所有格式
                        key.health_by_format = {}  # type: ignore[assignment]
                        key.circuit_breaker_by_format = {}  # type: ignore[assignment]
                        logger.info(f"[RESET] 重置 Key 所有格式健康度: {key_id}")

            db.flush()
            get_batch_committer().mark_dirty(db)
            return True

        except Exception as e:
            logger.error(f"重置健康度失败: {e}")
            db.rollback()
            return False

    @classmethod
    def manually_enable(cls, db: Session, key_id: Optional[str] = None) -> bool:
        """手动启用 Key"""
        try:
            if key_id:
                key = db.query(ProviderAPIKey).filter(ProviderAPIKey.id == key_id).first()
                if key and not key.is_active:
                    key.is_active = True  # type: ignore[assignment]
                    # 重置所有格式的健康度
                    key.health_by_format = {}  # type: ignore[assignment]
                    key.circuit_breaker_by_format = {}  # type: ignore[assignment]
                    logger.info(f"[OK] 手动启用 Key: {key_id}")

            db.flush()
            get_batch_committer().mark_dirty(db)
            return True

        except Exception as e:
            logger.error(f"手动启用失败: {e}")
            db.rollback()
            return False

    @classmethod
    def get_all_health_status(cls, db: Session) -> Dict[str, Any]:
        """获取所有健康状态摘要"""
        try:
            endpoint_stats = db.query(
                func.count(ProviderEndpoint.id).label("total"),
                func.sum(case((ProviderEndpoint.is_active == True, 1), else_=0)).label("active"),
                func.sum(case((ProviderEndpoint.health_score < 0.5, 1), else_=0)).label(
                    "unhealthy"
                ),
            ).first()

            # 统计 Key（需要遍历 JSON 字段计算熔断状态）
            keys = db.query(ProviderAPIKey).all()
            total_keys = len(keys)
            active_keys = sum(1 for k in keys if k.is_active)
            unhealthy_keys = 0
            circuit_open_keys = 0

            for key in keys:
                health_by_format = key.health_by_format or {}
                circuit_by_format = key.circuit_breaker_by_format or {}

                # 检查是否有任何格式健康度低于 0.5
                for fmt, health_data in health_by_format.items():
                    if float(health_data.get("health_score") or 1.0) < 0.5:
                        unhealthy_keys += 1
                        break

                # 检查是否有任何格式熔断器开启
                for fmt, circuit_data in circuit_by_format.items():
                    if circuit_data.get("open"):
                        circuit_open_keys += 1
                        break

            return {
                "endpoints": {
                    "total": endpoint_stats.total or 0 if endpoint_stats else 0,
                    "active": int(endpoint_stats.active or 0) if endpoint_stats else 0,
                    "unhealthy": int(endpoint_stats.unhealthy or 0) if endpoint_stats else 0,
                },
                "keys": {
                    "total": total_keys,
                    "active": active_keys,
                    "unhealthy": unhealthy_keys,
                    "circuit_open": circuit_open_keys,
                },
            }

        except Exception as e:
            logger.error(f"获取健康状态摘要失败: {e}")
            return {
                "endpoints": {"total": 0, "active": 0, "unhealthy": 0},
                "keys": {"total": 0, "active": 0, "unhealthy": 0, "circuit_open": 0},
            }

    # ==================== 历史记录方法 ====================

    @classmethod
    def _push_circuit_event(cls, event: Dict[str, Any]) -> None:
        cls._circuit_history.append(event)
        if len(cls._circuit_history) > cls.CIRCUIT_HISTORY_LIMIT:
            cls._circuit_history.pop(0)

    @classmethod
    def get_circuit_history(cls, limit: int = 50) -> List[Dict[str, Any]]:
        if limit <= 0:
            return []
        return cls._circuit_history[-limit:]

    # ==================== 兼容旧方法 ====================

    @classmethod
    def is_eligible_for_probe(
        cls,
        db: Session,
        endpoint_id: Optional[str] = None,
        key_id: Optional[str] = None,
        api_format: Optional[str] = None,
    ) -> bool:
        """检查是否有资格进行探测（按 API 格式）"""
        if not cls.ALLOW_AUTO_RECOVER:
            return False

        if endpoint_id:
            return False  # Endpoint 不支持探测

        if key_id:
            key = db.query(ProviderAPIKey).filter(ProviderAPIKey.id == key_id).first()
            if key:
                if api_format:
                    circuit_data = cls._get_circuit_data(key, api_format)
                    if circuit_data.get("open"):
                        now = datetime.now(timezone.utc)
                        state = cls._get_circuit_state_from_data(circuit_data, now)
                        return state == CircuitState.HALF_OPEN
                else:
                    # 兼容旧调用：检查是否有任何格式处于半开状态
                    circuit_by_format = key.circuit_breaker_by_format or {}
                    now = datetime.now(timezone.utc)
                    for fmt, circuit_data in circuit_by_format.items():
                        if circuit_data.get("open"):
                            state = cls._get_circuit_state_from_data(circuit_data, now)
                            if state == CircuitState.HALF_OPEN:
                                return True

        return False

    # ==================== 便捷方法 ====================

    @classmethod
    def get_health_score(
        cls, key: ProviderAPIKey, api_format: Optional[str] = None
    ) -> float:
        """获取指定格式的健康度分数"""
        if not api_format:
            # 返回所有格式中的最低健康度
            health_by_format = key.health_by_format or {}
            if not health_by_format:
                return 1.0
            return min(
                float(h.get("health_score") or 1.0) for h in health_by_format.values()
            )

        health_data = cls._get_health_data(key, api_format)
        return float(health_data.get("health_score") or 1.0)

    @classmethod
    def is_any_circuit_open(cls, key: ProviderAPIKey) -> bool:
        """检查是否有任何格式的熔断器开启"""
        circuit_by_format = key.circuit_breaker_by_format or {}
        for circuit_data in circuit_by_format.values():
            if circuit_data.get("open"):
                return True
        return False


# 全局健康监控器实例
health_monitor = HealthMonitor()
health_open_circuits.set(0)
